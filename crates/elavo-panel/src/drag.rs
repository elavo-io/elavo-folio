//! Press the sun or a menu title, slide onto an item, and release to choose
//! it, as on a Mac.
//!
//! A menu opened while the button is held does not grab the pointer. A grab
//! moves the pointer onto the menu's surface, and wlroots then drops the
//! release, because that surface never saw the press. Without the grab, labwc
//! keeps sending motion and the release to the panel's surface, and the rows
//! under the pointer are found from panel-surface coordinates. When the drag
//! ends, the menus get their grab back, so a menu left open closes on a click
//! elsewhere.
//!
//! A drag ends on its release, on the next press, or when the menus close.
//! Hovering after that never opens a menu.

use std::cell::{Cell, RefCell};
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gdk, glib};

/// How long the pointer rests on a submenu row before it opens, GTK's own
/// menu delay, so sliding across the menu does not flash each submenu.
const SUBMENU_DELAY: Duration = Duration::from_millis(80);
/// The row under a held pointer. Styled like a hovered row.
const DRAG_OVER: &str = "elavo-drag-over";
/// `GTK_BUTTON_ROLE_TITLE`. gtk4-rs does not export the model button's role.
const ROLE_TITLE: i32 = 3;

#[derive(Default)]
struct Drag {
    active: Cell<bool>,
    on_opener: Cell<bool>,
    opener: RefCell<Option<gtk::Widget>>,
    row: RefCell<Option<gtk::Widget>>,
    /// The row under the pointer and the rows whose submenus lead to it.
    lit: RefCell<Vec<gtk::Widget>>,
    submenu_timer: RefCell<Option<glib::SourceId>>,
    /// Menus whose grab is off for this drag.
    held: RefCell<Vec<glib::WeakRef<gtk::Popover>>>,
    /// The last event handled. The panel window and a menu can both see one
    /// event; it is handled once. Holding it keeps its address from being
    /// reused by the next event.
    last_event: RefCell<Option<gdk::Event>>,
}

thread_local! {
    static DRAG: Drag = Drag::default();
}

fn debug() -> bool {
    std::env::var_os("ELAVO_PANEL_DEBUG_DRAG").is_some()
}

macro_rules! trace {
    ($($arg:tt)*) => {
        if debug() {
            eprintln!("drag: {}", format!($($arg)*));
        }
    };
}

/// Watch a bar window for presses on the sun and the menu titles.
pub fn track_panel(window: &gtk::Widget) {
    listen(window);
}

/// Watch a menu, for events the compositor delivers to its surface.
pub fn track_menu(popover: &gtk::Popover) {
    listen(popover.upcast_ref());
    // A menu closed by a click elsewhere, or by Escape, ends any drag
    // still waiting for its release. Checked after the frame, because
    // switching titles closes one menu before opening the next.
    popover.connect_closed(|popover| {
        let Some(panel) = popover.root().map(|root| root.upcast::<gtk::Widget>()) else {
            return;
        };
        glib::idle_add_local_once(move || {
            if open_popovers(&panel).is_empty() {
                DRAG.with(|drag| cancel(drag, "menus closed"));
            }
        });
    });
}

fn listen(widget: &gtk::Widget) {
    let events = gtk::EventControllerLegacy::new();
    events.set_propagation_phase(gtk::PropagationPhase::Capture);
    events.connect_event(|controller, event| {
        let Some(widget) = controller.widget() else {
            return glib::Propagation::Proceed;
        };
        let Some(panel) = widget.root().map(|root| root.upcast::<gtk::Widget>()) else {
            return glib::Propagation::Proceed;
        };
        DRAG.with(|drag| {
            let seen = drag
                .last_event
                .replace(Some(event.clone()))
                .is_some_and(|last| last.as_ptr() == event.as_ptr());
            if seen {
                return glib::Propagation::Proceed;
            }
            handle(drag, &panel, event)
        })
    });
    widget.add_controller(events);
}

fn handle(drag: &Drag, panel: &gtk::Widget, event: &gdk::Event) -> glib::Propagation {
    let kind = event.event_type();
    let primary = event
        .downcast_ref::<gdk::ButtonEvent>()
        .is_some_and(|button| button.button() == gdk::BUTTON_PRIMARY);
    let point = panel_point(panel, event);
    if debug()
        && (drag.active.get()
            || matches!(
                kind,
                gdk::EventType::ButtonPress | gdk::EventType::ButtonRelease
            ))
    {
        let depth = event.surface().map(|s| popup_depth(&s)).unwrap_or(99);
        trace!(
            "{kind:?} surface-depth {depth} at {:?} panel {point:?} mods {:?}",
            event.position(),
            event.modifier_state()
        );
    }
    let Some(point) = point else {
        return glib::Propagation::Proceed;
    };
    match kind {
        gdk::EventType::ButtonPress if primary => press(drag, panel, point),
        gdk::EventType::MotionNotify if drag.active.get() => {
            follow(drag, panel, point);
            glib::Propagation::Proceed
        }
        gdk::EventType::ButtonRelease if primary && drag.active.get() => {
            follow(drag, panel, point);
            release(drag, panel)
        }
        _ => glib::Propagation::Proceed,
    }
}

fn press(drag: &Drag, panel: &gtk::Widget, point: (f64, f64)) -> glib::Propagation {
    cancel(drag, "new press");
    let Some(opener) = opener_at(panel, point) else {
        return glib::Propagation::Proceed;
    };
    // A second press on the open sun closes it, as a click does.
    if let Some(button) = opener.downcast_ref::<gtk::MenuButton>() {
        if button.popover().is_some_and(|menu| menu.is_visible()) {
            trace!("press on the open sun: close");
            button.popdown();
            return glib::Propagation::Stop;
        }
    }
    // The menu opens on the press, so a held press can reach its items. An
    // open menu is closed first, so it reopens without its grab. The press
    // and its release are kept from the sun and the titles, which would
    // otherwise toggle the menu on the release, or wait for a release they
    // never get.
    trace!("press on an opener: open");
    close_all(panel);
    start(drag, panel, &opener);
    open(&opener);
    glib::Propagation::Stop
}

fn start(drag: &Drag, panel: &gtk::Widget, opener: &gtk::Widget) {
    drag.active.set(true);
    drag.on_opener.set(true);
    drag.opener.replace(Some(opener.clone()));
    // Every title's menu, because GTK opens the next title's menu itself
    // when the pointer crosses onto it.
    for menu in bar_menus(panel) {
        hold(drag, &menu);
    }
}

/// Opens `menu` without a grab for the rest of the drag. Only a closed menu
/// can change, because GTK closes a menu whose grab changes.
fn hold(drag: &Drag, menu: &gtk::Popover) {
    if menu.get_visible() || !menu.is_autohide() {
        return;
    }
    menu.set_autohide(false);
    drag.held.borrow_mut().push(menu.downgrade());
}

/// Closes the menus opened during the drag and gives them their grab back.
/// A submenu keeps its own visible flag when its parent closes, and would
/// open again with the parent, so each menu is closed by its own flag,
/// submenus first.
fn unhold(drag: &Drag) {
    let menus: Vec<gtk::Popover> = drag
        .held
        .take()
        .iter()
        .filter_map(glib::WeakRef::upgrade)
        .collect();
    for menu in menus.iter().rev() {
        if menu.get_visible() {
            menu.popdown();
        }
    }
    for menu in &menus {
        menu.set_autohide(true);
    }
}

fn follow(drag: &Drag, panel: &gtk::Widget, point: (f64, f64)) {
    if let Some(opener) = opener_at(panel, point) {
        drag.on_opener.set(true);
        set_row(drag, None);
        if drag.opener.borrow().as_ref() != Some(&opener) {
            trace!("slide to another title");
            close_others(panel, &opener);
            open(&opener);
            drag.opener.replace(Some(opener));
        }
        return;
    }
    drag.on_opener.set(false);
    set_row(drag, row_at(panel, point));
}

fn release(drag: &Drag, panel: &gtk::Widget) -> glib::Propagation {
    let row = drag.row.borrow().clone();
    let kind = row.as_ref().map(row_kind);
    let opener = drag.opener.borrow().clone();
    let choice = release_choice(kind, drag.on_opener.get());
    trace!("release: {choice:?}");
    match choice {
        Release::Activate => {
            if let Some(row) = row {
                row.emit_by_name::<()>("clicked", &[]);
            }
            cancel(drag, "released");
        }
        // A click on a title: the menu stays open, as a click leaves it.
        // It reopens with its grab.
        Release::LeaveOpen => {
            cancel(drag, "released");
            if let Some(opener) = opener {
                open(&opener);
            }
        }
        Release::Dismiss => {
            cancel(drag, "released");
            close_all(panel);
        }
    }
    glib::Propagation::Stop
}

fn cancel(drag: &Drag, why: &str) {
    if drag.active.replace(false) {
        trace!("end: {why}");
    }
    drag.on_opener.set(false);
    drag.opener.take();
    unhold(drag);
    if let Some(timer) = drag.submenu_timer.take() {
        timer.remove();
    }
    drag.row.take();
    light(drag, Vec::new());
}

fn set_row(drag: &Drag, next: Option<gtk::Widget>) {
    if *drag.row.borrow() == next {
        return;
    }
    if let Some(timer) = drag.submenu_timer.take() {
        timer.remove();
    }
    let previous = drag.row.take();
    // Leaving a submenu's row for another row of the same menu closes the
    // submenu. Moving into the submenu, or across the gap on the way to it,
    // keeps it.
    if let (Some(previous), Some(row)) = (&previous, &next) {
        if let Some(submenu) = row_popover(previous) {
            if !row.is_ancestor(&submenu) && submenu.is_visible() {
                trace!("close the submenu of the row left");
                submenu.popdown();
            }
        }
    }
    trace!(
        "row: {:?}",
        next.as_ref()
            .map(|row| row.property::<Option<String>>("text"))
    );
    let lit = match (&next, &previous) {
        (Some(row), _) => lit_path(row, row_kind(row) != RowKind::Inactive),
        (None, Some(previous)) => lit_path(
            previous,
            row_popover(previous).is_some_and(|menu| menu.is_visible()),
        ),
        (None, None) => Vec::new(),
    };
    light(drag, lit);
    if let Some(row) = &next {
        if row_kind(row) == RowKind::Submenu
            && !row_popover(row).is_some_and(|menu| menu.is_visible())
        {
            let weak = row.downgrade();
            let timer = glib::timeout_add_local_once(SUBMENU_DELAY, move || {
                let Some(row) = weak.upgrade() else {
                    return;
                };
                let still = DRAG.with(|drag| {
                    drag.submenu_timer.take();
                    let still = drag.active.get() && drag.row.borrow().as_ref() == Some(&row);
                    if still {
                        if let Some(submenu) = row_popover(&row) {
                            hold(drag, &submenu);
                        }
                    }
                    still
                });
                if still {
                    trace!("open the submenu");
                    row.emit_by_name::<()>("clicked", &[]);
                }
            });
            drag.submenu_timer.replace(Some(timer));
        }
    }
    drag.row.replace(next);
}

/// The rows that lead to `row`, with `row` itself when `with_row` is set.
/// A submenu is a child of its row, so the path stays lit while the pointer
/// is in the submenu.
fn lit_path(row: &gtk::Widget, with_row: bool) -> Vec<gtk::Widget> {
    let mut rows = Vec::new();
    if with_row {
        rows.push(row.clone());
    }
    let mut current = row.parent();
    while let Some(widget) = current {
        if widget.css_name().as_str() == "modelbutton" {
            rows.push(widget.clone());
        }
        current = widget.parent();
    }
    rows
}

fn light(drag: &Drag, rows: Vec<gtk::Widget>) {
    for row in drag.lit.take() {
        row.remove_css_class(DRAG_OVER);
    }
    for row in &rows {
        row.add_css_class(DRAG_OVER);
    }
    drag.lit.replace(rows);
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum RowKind {
    /// A row that runs an action. Releasing on it chooses it.
    Command,
    /// A row that opens another menu. Resting on it opens that menu.
    Submenu,
    /// A disabled row, or a submenu's title row.
    Inactive,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Release {
    Activate,
    Dismiss,
    LeaveOpen,
}

fn kind_of(sensitive: bool, submenu: bool, title: bool) -> RowKind {
    if !sensitive || title {
        RowKind::Inactive
    } else if submenu {
        RowKind::Submenu
    } else {
        RowKind::Command
    }
}

/// Release on a command runs it. Release on a title leaves its menu open.
/// Anywhere else closes the menus.
fn release_choice(row: Option<RowKind>, on_opener: bool) -> Release {
    match row {
        Some(RowKind::Command) => Release::Activate,
        _ if on_opener => Release::LeaveOpen,
        _ => Release::Dismiss,
    }
}

fn row_kind(row: &gtk::Widget) -> RowKind {
    let submenu = row_popover(row).is_some()
        || row
            .property::<Option<String>>("menu-name")
            .is_some_and(|name| !name.is_empty());
    let role = row.property_value("role");
    let title = unsafe { glib::gobject_ffi::g_value_get_enum(role.as_ptr()) } == ROLE_TITLE;
    kind_of(row.is_sensitive(), submenu, title)
}

fn row_popover(row: &gtk::Widget) -> Option<gtk::Popover> {
    row.property("popover")
}

fn open(opener: &gtk::Widget) {
    if let Some(button) = opener.downcast_ref::<gtk::MenuButton>() {
        button.popup();
    } else {
        opener.activate();
    }
}

fn close_others(panel: &gtk::Widget, opener: &gtk::Widget) {
    let keep = opener_menu(opener);
    for menu in open_popovers(panel) {
        if Some(&menu) != keep.as_ref() {
            menu.popdown();
        }
    }
}

fn close_all(panel: &gtk::Widget) {
    for menu in open_popovers(panel) {
        menu.popdown();
    }
}

/// The sun's menu and every menu title's menu.
fn bar_menus(panel: &gtk::Widget) -> Vec<gtk::Popover> {
    let mut menus = Vec::new();
    collect_openers(panel, &mut menus);
    menus
}

fn collect_openers(widget: &gtk::Widget, out: &mut Vec<gtk::Popover>) {
    let mut child = widget.first_child();
    while let Some(current) = child {
        let opener = current.is::<gtk::MenuButton>()
            || current
                .parent()
                .is_some_and(|parent| parent.is::<gtk::PopoverMenuBar>());
        if opener {
            out.extend(opener_menu(&current));
        } else if !current.is::<gtk::Popover>() {
            collect_openers(&current, out);
        }
        child = current.next_sibling();
    }
}

fn opener_menu(opener: &gtk::Widget) -> Option<gtk::Popover> {
    if let Some(button) = opener.downcast_ref::<gtk::MenuButton>() {
        return button.popover();
    }
    let mut child = opener.first_child();
    while let Some(widget) = child {
        if let Some(popover) = widget.downcast_ref::<gtk::Popover>() {
            return Some(popover.clone());
        }
        child = widget.next_sibling();
    }
    None
}

/// The sun or the menu title at `point`, in panel-surface coordinates.
fn opener_at(panel: &gtk::Widget, point: (f64, f64)) -> Option<gtk::Widget> {
    let (dx, dy) = panel.native()?.surface_transform();
    let picked = panel.pick(point.0 - dx, point.1 - dy, gtk::PickFlags::INSENSITIVE)?;
    let mut current = Some(picked);
    let mut menu_button = None;
    while let Some(widget) = current {
        if widget.is::<gtk::Popover>() {
            return None;
        }
        if widget
            .parent()
            .is_some_and(|parent| parent.is::<gtk::PopoverMenuBar>())
        {
            return Some(widget);
        }
        if widget.is::<gtk::MenuButton>() {
            menu_button = Some(widget.clone());
        }
        current = widget.parent();
    }
    menu_button
}

/// The menu row at `point`, in panel-surface coordinates. A submenu sits
/// above its parent, so the deepest menu wins.
fn row_at(panel: &gtk::Widget, point: (f64, f64)) -> Option<gtk::Widget> {
    let panel_surface = panel.native()?.surface()?;
    let mut best: Option<(usize, gtk::Widget)> = None;
    for menu in open_popovers(panel) {
        let Some(surface) = menu.native().and_then(|native| native.surface()) else {
            continue;
        };
        let Some(origin) = origin_on(&panel_surface, &surface) else {
            continue;
        };
        let (dx, dy) = menu.surface_transform();
        let (x, y) = popup_local(point, origin, (dx, dy));
        let Some(row) = menu
            .pick(x, y, gtk::PickFlags::INSENSITIVE)
            .and_then(|picked| model_button(&picked))
        else {
            continue;
        };
        let depth = popup_depth(&surface);
        if best.as_ref().is_none_or(|(have, _)| depth >= *have) {
            best = Some((depth, row));
        }
    }
    best.map(|(_, row)| row)
}

/// A point on the panel's surface, in that surface's coordinates, for an
/// event on the panel or on one of its menus.
fn panel_point(panel: &gtk::Widget, event: &gdk::Event) -> Option<(f64, f64)> {
    let (x, y) = event.position()?;
    let surface = event.surface()?;
    let panel_surface = panel.native()?.surface()?;
    let (ox, oy) = origin_on(&panel_surface, &surface)?;
    Some((ox + x, oy + y))
}

/// A popup point in the menu's widget coordinates. `origin` is the popup
/// surface's position on the panel surface, and `transform` is the shadow
/// GTK keeps outside the widget.
fn popup_local(point: (f64, f64), origin: (f64, f64), transform: (f64, f64)) -> (f64, f64) {
    (
        point.0 - origin.0 - transform.0,
        point.1 - origin.1 - transform.1,
    )
}

fn origin_on(root: &gdk::Surface, surface: &gdk::Surface) -> Option<(f64, f64)> {
    let mut x = 0.0;
    let mut y = 0.0;
    let mut current = surface.clone();
    while &current != root {
        let popup = current.downcast::<gdk::Popup>().ok()?;
        x += f64::from(popup.position_x());
        y += f64::from(popup.position_y());
        current = popup.parent()?;
    }
    Some((x, y))
}

fn popup_depth(surface: &gdk::Surface) -> usize {
    let mut depth = 0;
    let mut current = surface.clone();
    while let Ok(popup) = current.downcast::<gdk::Popup>() {
        depth += 1;
        let Some(parent) = popup.parent() else {
            break;
        };
        current = parent;
    }
    depth
}

fn model_button(widget: &gtk::Widget) -> Option<gtk::Widget> {
    let mut current = Some(widget.clone());
    while let Some(widget) = current {
        if widget.css_name().as_str() == "modelbutton" {
            return Some(widget);
        }
        if widget.is::<gtk::Popover>() {
            return None;
        }
        current = widget.parent();
    }
    None
}

fn open_popovers(widget: &gtk::Widget) -> Vec<gtk::Popover> {
    let mut menus = Vec::new();
    collect(widget, &mut menus);
    menus
}

fn collect(widget: &gtk::Widget, out: &mut Vec<gtk::Popover>) {
    let mut child = widget.first_child();
    while let Some(current) = child {
        if let Some(menu) = current.downcast_ref::<gtk::Popover>() {
            if menu.is_visible() && menu.has_css_class("elavo-panel-dropdown") {
                out.push(menu.clone());
            }
        }
        collect(&current, out);
        child = current.next_sibling();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_held_release_chooses_the_command_under_it() {
        assert_eq!(
            release_choice(Some(RowKind::Command), false),
            Release::Activate
        );
        assert_eq!(
            release_choice(Some(RowKind::Submenu), false),
            Release::Dismiss
        );
        assert_eq!(
            release_choice(Some(RowKind::Inactive), false),
            Release::Dismiss
        );
        assert_eq!(release_choice(None, false), Release::Dismiss);
    }

    #[test]
    fn a_release_on_the_title_leaves_the_menu_open() {
        assert_eq!(release_choice(None, true), Release::LeaveOpen);
    }

    #[test]
    fn a_disabled_row_or_a_title_is_not_a_command() {
        assert_eq!(kind_of(false, false, false), RowKind::Inactive);
        assert_eq!(kind_of(true, false, true), RowKind::Inactive);
        assert_eq!(kind_of(true, true, false), RowKind::Submenu);
        assert_eq!(kind_of(true, false, false), RowKind::Command);
    }

    #[test]
    fn a_menu_point_leaves_out_the_menu_origin_and_its_shadow() {
        assert_eq!(
            popup_local((200.0, 60.0), (192.0, 48.0), (8.0, 8.0)),
            (0.0, 4.0)
        );
    }
}
