//! The menus that open from the bar.
//!
//! When the compositor offers blur, each one is the window color at 75%,
//! from `$elavo-panel-menu-bg` in the
//! Elavo theme, and the compositor blurs what is behind it. Without blur
//! the menus are opaque. A menu that opens from the bar sits 10px below the
//! panel, and at least 10px in from the left edge of the screen.

use std::cell::{Cell, RefCell};
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use glib::translate::IntoGlib;
use gtk::prelude::*;
use gtk::{gdk, glib};

/// `ELAVO_PANEL_MENU_BLUR=0` keeps menus opaque and does not ask the
/// compositor to blur. labwc builds before the mapped-surface check in
/// labwc-mods fault when a closed menu still holds a blur.
pub(crate) fn menu_blur_enabled() -> bool {
    std::env::var("ELAVO_PANEL_MENU_BLUR").ok().as_deref() != Some("0")
}

/// Gap under the panel, and the closest a menu may come to the left edge.
const GAP: i32 = 10;
/// `$elavo-popover-radius`. The blur follows the menu's rounded corners.
const RADIUS: i32 = 14;

struct BlurSlot {
    blur: elavo_common::blur::BackgroundBlur,
    /// The popup surface this effect was bound to. A reposition can replace
    /// the surface without a new widget, and the old effect then does nothing.
    surface: usize,
    part: (i32, i32, i32, i32, i32),
}

thread_local! {
    static BLURS: RefCell<HashMap<usize, BlurSlot>> = RefCell::new(HashMap::new());
    static TICKING: RefCell<HashSet<usize>> = RefCell::new(HashSet::new());
    /// The corrected offset for each bar button. Status menus are built anew
    /// on every click; starting from this one opens them where they end up
    /// instead of moving them after the first frame.
    static OFFSETS: RefCell<HashMap<usize, (i32, i32)>> = RefCell::new(HashMap::new());
}

fn debug_enabled() -> bool {
    thread_local! {
        static ON: bool = std::env::var("ELAVO_PANEL_BLUR_DEBUG").ok().as_deref() == Some("1");
    }
    ON.with(|on| *on)
}

macro_rules! trace {
    ($($arg:tt)*) => {
        if debug_enabled() {
            glib::g_message!("elavo-panel", "menu {}", format!($($arg)*));
        }
    };
}

/// Style `popover` and, when `from_bar` is set, keep it off the panel and
/// the left edge. Nested submenus pass `from_bar` as false: they still
/// blur, and they keep the position GTK gives them beside their parent.
pub fn prepare(popover: &gtk::Popover, from_bar: bool) {
    if popover.has_css_class("elavo-panel-dropdown") {
        return;
    }
    popover.add_css_class("elavo-panel-dropdown");
    popover.set_has_arrow(false);
    if from_bar {
        // Start keeps the menu under its title. The default centers it on
        // the button, and a menu wider than the button then lands on the
        // screen edge.
        popover.set_halign(gtk::Align::Start);
        let offset = popover
            .parent()
            .and_then(|parent| OFFSETS.with(|map| map.borrow().get(&widget_key(&parent)).copied()))
            .unwrap_or((0, GAP));
        popover.set_offset(offset.0, offset.1);
    }

    popover.connect_show(|popover| walk(popover.upcast_ref(), false));

    let placed = Rc::new(Cell::new(false));
    let tries = Rc::new(Cell::new(0_i32));
    let layout: Rc<RefCell<Option<(gdk::Surface, glib::SignalHandlerId)>>> = Rc::default();
    let placed_on_unmap = Rc::clone(&placed);
    let tries_on_unmap = Rc::clone(&tries);
    let layout_on_unmap = Rc::clone(&layout);
    popover.connect_unmap(move |popover| {
        trace!("{:#x} unmap", widget_key(popover.upcast_ref()));
        placed_on_unmap.set(false);
        tries_on_unmap.set(0);
        if let Some((surface, id)) = layout_on_unmap.take() {
            surface.disconnect(id);
        }
        release_blur(popover);
    });
    // GTK sizes a new menu in its surface's layout, which runs after map
    // and before the first frame is drawn. A region sent from here goes out
    // with that frame; one sent at map has only the unallocated size.
    let layout_on_map = Rc::clone(&layout);
    popover.connect_map(move |popover| {
        if !menu_blur_enabled() {
            return;
        }
        let Some(surface) = popover.native().and_then(|native| native.surface()) else {
            return;
        };
        let weak = popover.downgrade();
        let id = surface.connect_layout(move |_, _, _| {
            if let Some(popover) = weak.upgrade() {
                sync_blur(&popover);
            }
        });
        if let Some((old, old_id)) = layout_on_map.replace(Some((surface, id))) {
            old.disconnect(old_id);
        }
    });

    popover.connect_map(move |popover| {
        let key = widget_key(popover.upcast_ref());
        trace!(
            "{key:#x} map offset {:?} popup {:?} contents {:?}",
            popover.offset(),
            popup_of(popover).map(|p| (p.position_x(), p.position_y())),
            contents_rect(popover)
        );
        let start = TICKING.with(|ticking| ticking.borrow_mut().insert(key));
        // A reopened menu keeps its last size, so its blur can go out with
        // the first frame. The tick corrects it if the size changed.
        if menu_blur_enabled() {
            sync_blur(popover);
        }
        if !start {
            return;
        }
        let placed = Rc::clone(&placed);
        let tries = Rc::clone(&tries);
        popover.add_tick_callback(move |widget, _| {
            let Some(popover) = widget.downcast_ref::<gtk::Popover>() else {
                return glib::ControlFlow::Break;
            };
            if !popover.is_mapped() {
                TICKING.with(|ticking| {
                    ticking
                        .borrow_mut()
                        .remove(&widget_key(popover.upcast_ref()));
                });
                return glib::ControlFlow::Break;
            }
            walk(popover.upcast_ref(), false);
            if from_bar && !placed.get() {
                match place_bar_menu(popover) {
                    Some(false) => placed.set(true),
                    Some(true) => {
                        let n = tries.get();
                        tries.set(n + 1);
                        // One correction, then one check. More than that
                        // walks the menu down the screen.
                        if n >= 1 {
                            placed.set(true);
                        }
                    }
                    None => {}
                }
            }
            // The region is double-buffered state. A static menu does not
            // commit again on its own, so a region sent after its first
            // frame would wait for the pointer to touch a row.
            if menu_blur_enabled() && sync_blur(popover) {
                popover.queue_draw();
            }
            glib::ControlFlow::Continue
        });
    });
    popover.connect_destroy(drop_blur);
    crate::drag::track_menu(popover);
    walk(popover.upcast_ref(), false);
}

/// Prepare every menu under `widget`. Menus found here open from the bar.
pub fn prepare_tree(widget: &gtk::Widget) {
    walk(widget, true);
}

fn walk(widget: &gtk::Widget, from_bar: bool) {
    let mut child = widget.first_child();
    while let Some(current) = child {
        if let Some(popover) = current.downcast_ref::<gtk::Popover>() {
            prepare(popover, from_bar);
        } else {
            walk(&current, from_bar);
        }
        child = current.next_sibling();
    }
}

/// Move a bar menu so its visible box is `GAP` below the panel and at
/// least `GAP` from the left of the screen. `None` until the popup has
/// been positioned. `Some(true)` when the offset just changed.
fn place_bar_menu(popover: &gtk::Popover) -> Option<bool> {
    let popup = popup_of(popover)?;
    let (contents_x, contents_y, width, _) = contents_rect(popover)?;
    if width < 1 {
        return None;
    }
    let window = popover
        .root()
        .and_then(|root| root.downcast::<gtk::Window>().ok())?;
    let panel_bottom = window.height();
    if panel_bottom <= 0 {
        return None;
    }
    let next = menu_correction(
        popup.position_x(),
        popup.position_y(),
        contents_x,
        contents_y,
        panel_bottom,
        popover.offset(),
    )?;
    let shifted = next != popover.offset();
    if shifted {
        trace!(
            "{:#x} move popup ({}, {}) offset {:?} -> {next:?}",
            widget_key(popover.upcast_ref()),
            popup.position_x(),
            popup.position_y(),
            popover.offset()
        );
        popover.set_offset(next.0, next.1);
    }
    if let Some(parent) = popover.parent() {
        OFFSETS.with(|map| map.borrow_mut().insert(widget_key(&parent), next));
    }
    Some(shifted)
}

/// The offset that puts the visible menu `GAP` below `panel_bottom` and at
/// least `GAP` from the screen's left. `None` while the popup is still at
/// the origin, which is where it sits before GTK positions it.
fn menu_correction(
    popup_x: i32,
    popup_y: i32,
    contents_x: i32,
    contents_y: i32,
    panel_bottom: i32,
    offset: (i32, i32),
) -> Option<(i32, i32)> {
    if popup_x == 0 && popup_y == 0 {
        return None;
    }
    let left = popup_x + contents_x;
    let top = popup_y + contents_y;
    let dx = if left < GAP { GAP - left } else { 0 };
    let dy = panel_bottom + GAP - top;
    Some((offset.0 + dx, offset.1 + dy))
}

/// Keep the blur region on the menu's surface. True when a new region went
/// out, which takes effect with the surface's next commit.
fn sync_blur(popover: &gtk::Popover) -> bool {
    let Some(surface) = popover.native().and_then(|native| native.surface()) else {
        return false;
    };
    // The panel window is not a popup. Blur only the menu's own surface.
    if surface.downcast_ref::<gdk::Popup>().is_none() {
        return false;
    }
    let Some((x, y, width, height)) = contents_rect(popover) else {
        return false;
    };
    if width <= 0 || height <= 0 {
        return false;
    }
    // Widget coordinates start inside the popover's shadow. The blur region
    // is in surface coordinates, which start at the shadow's outer edge.
    let (dx, dy) = popover.surface_transform();
    let part = (
        x + dx.round() as i32,
        y + dy.round() as i32,
        width,
        height,
        RADIUS,
    );
    let key = widget_key(popover.upcast_ref());
    let surface_id = surface.as_ptr() as usize;
    let stale = BLURS.with(|map| {
        map.borrow()
            .get(&key)
            .is_some_and(|slot| slot.surface != surface_id)
    });
    if stale {
        trace!("{key:#x} surface changed");
        release_blur(popover);
    }
    BLURS.with(|map| {
        let mut map = map.borrow_mut();
        if let Entry::Vacant(entry) = map.entry(key) {
            match elavo_common::blur::BackgroundBlur::for_widget(popover.upcast_ref()) {
                Some(blur) => {
                    trace!("{key:#x} attach surface {surface_id:#x}");
                    entry.insert(BlurSlot {
                        blur,
                        surface: surface_id,
                        part: (0, 0, 0, 0, -1),
                    });
                }
                None => trace!("{key:#x} attach failed"),
            }
        }
        let Some(slot) = map.get_mut(&key) else {
            return false;
        };
        if slot.part == part {
            return false;
        }
        trace!("{key:#x} region {:?} -> {part:?}", slot.part);
        slot.blur.set_rounds(&[part]);
        slot.part = part;
        true
    })
}

fn release_blur(popover: &gtk::Popover) {
    let key = widget_key(popover.upcast_ref());
    BLURS.with(|map| {
        if let Some(slot) = map.borrow_mut().remove(&key) {
            trace!("{key:#x} release");
            slot.blur.release();
        }
    });
}

fn drop_blur(popover: &gtk::Popover) {
    release_blur(popover);
    let key = widget_key(popover.upcast_ref());
    TICKING.with(|ticking| {
        ticking.borrow_mut().remove(&key);
    });
}

/// The menu body, in the popover's own coordinates. The arrow is a sibling
/// and is not part of the fill.
fn contents_rect(popover: &gtk::Popover) -> Option<(i32, i32, i32, i32)> {
    let mut child = popover.first_child();
    while let Some(widget) = child {
        if widget.css_name().as_str() == "contents" {
            let bounds = widget.compute_bounds(popover.upcast_ref::<gtk::Widget>())?;
            return Some((
                bounds.x().round() as i32,
                bounds.y().round() as i32,
                bounds.width().round().max(0.0) as i32,
                bounds.height().round().max(0.0) as i32,
            ));
        }
        child = widget.next_sibling();
    }
    None
}

fn popup_of(popover: &gtk::Popover) -> Option<gdk::Popup> {
    popover.native()?.surface()?.downcast::<gdk::Popup>().ok()
}

fn widget_key(widget: &gtk::Widget) -> usize {
    widget.as_ptr() as usize
}

/// Where `GtkPopoverMenuBar.active_item` sits, when the bar is exactly the
/// widget plus its three pointers (model, tracker, active item). Any other
/// layout is left alone: guessing the offset would write into the bar.
fn active_item_offset(widget_size: u32, bar_size: u32) -> Option<usize> {
    let pointer = std::mem::size_of::<usize>() as u32;
    if bar_size.checked_sub(widget_size) != Some(pointer.saturating_mul(3)) {
        return None;
    }
    Some(widget_size as usize + 2 * pointer as usize)
}

fn menu_bar_active_offset() -> Option<usize> {
    unsafe {
        let mut widget = std::mem::zeroed();
        let mut bar = std::mem::zeroed();
        glib::gobject_ffi::g_type_query(gtk::Widget::static_type().into_glib(), &mut widget);
        glib::gobject_ffi::g_type_query(gtk::PopoverMenuBar::static_type().into_glib(), &mut bar);
        active_item_offset(widget.instance_size, bar.instance_size)
    }
}

/// GTK's menu bar keeps the title it opened and never clears it when that
/// title is destroyed before its menu has mapped. The next slide across the
/// bar then reads the freed title: menus stop opening, and the panel crashes.
/// Titles are guarded as they appear, and a destroyed open title asks the
/// next title the pointer enters to open.
pub fn watch_menu_bar(bar: &gtk::PopoverMenuBar) {
    let Some(offset) = menu_bar_active_offset() else {
        glib::g_warning!(
            "elavo-panel",
            "menu bar layout is not the one this panel knows; not guarding it"
        );
        return;
    };
    guard_bar_items(bar, offset);
}

fn guard_bar_items(bar: &gtk::PopoverMenuBar, offset: usize) {
    let mut child = bar.first_child();
    while let Some(item) = child {
        child = item.next_sibling();
        if unsafe { item.data::<bool>("elavo-menu-guard") }.is_some() {
            continue;
        }
        unsafe { item.set_data("elavo-menu-guard", true) };
        let bar_ref = bar.downgrade();
        item.connect_destroy(move |item| clear_open_title(&bar_ref, item, offset));
        let bar_ref = bar.downgrade();
        let enter = gtk::EventControllerMotion::new();
        enter.set_propagation_limit(gtk::PropagationLimit::None);
        enter.connect_enter(move |controller, _, _| {
            let Some(bar) = bar_ref.upgrade() else {
                return;
            };
            let reopen = unsafe { bar.data::<bool>("elavo-menu-reopen") }
                .is_some_and(|flag| unsafe { *flag.as_ref() });
            if !reopen {
                return;
            }
            unsafe { bar.set_data("elavo-menu-reopen", false) };
            if let Some(item) = controller.widget() {
                item.activate();
            }
        });
        item.add_controller(enter);
    }
}

/// `item` is still alive here. Drop the bar's pointer at `offset` when it is
/// this title, and remember to open whichever title the pointer enters next.
fn clear_open_title(bar: &glib::WeakRef<gtk::PopoverMenuBar>, item: &gtk::Widget, offset: usize) {
    let Some(bar) = bar.upgrade() else {
        return;
    };
    let mut open = false;
    let mut child = item.first_child();
    while let Some(widget) = child {
        open = widget
            .downcast_ref::<gtk::Popover>()
            .is_some_and(|popover| popover.is_visible() || popover.is_mapped());
        if open {
            break;
        }
        child = widget.next_sibling();
    }
    unsafe {
        let slot = bar.as_ptr().cast::<u8>().add(offset).cast::<*mut ()>();
        if *slot != item.as_ptr().cast() {
            return;
        }
        *slot = std::ptr::null_mut();
    }
    if open {
        unsafe { bar.set_data("elavo-menu-reopen", true) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_until_the_popup_is_placed() {
        assert_eq!(menu_correction(0, 0, 0, 0, 28, (0, 0)), None);
    }

    #[test]
    fn drops_the_menu_below_the_panel_and_in_from_the_left() {
        assert_eq!(menu_correction(0, 28, 0, 0, 28, (0, 0)), Some((10, 10)));
    }

    #[test]
    fn leaves_a_menu_that_is_already_in_place() {
        assert_eq!(menu_correction(10, 38, 0, 0, 28, (0, 10)), Some((0, 10)));
    }

    #[test]
    fn only_moves_a_menu_that_is_clear_of_the_left_edge_down() {
        assert_eq!(menu_correction(80, 28, 0, 0, 28, (0, 0)), Some((0, 10)));
    }

    #[test]
    fn the_open_title_is_the_bar_s_third_pointer() {
        let pointer = std::mem::size_of::<usize>() as u32;
        assert_eq!(
            active_item_offset(100, 100 + pointer * 3),
            Some(100 + 2 * pointer as usize)
        );
        assert_eq!(active_item_offset(100, 100 + pointer * 4), None);
        assert_eq!(active_item_offset(200, 100), None);
    }

    #[test]
    fn counts_the_shadow_inset_as_part_of_the_visible_box() {
        assert_eq!(menu_correction(0, 20, 8, 8, 28, (0, 0)), Some((2, 10)));
    }
}
