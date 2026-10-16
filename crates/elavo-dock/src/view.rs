//! One dock surface. Icons are drawn by hand, so they can magnify, bounce,
//! and slide apart without rebuilding widgets; the shelf, dividers, dots,
//! badges, and the name label are CSS-styled child widgets the view places
//! each frame, so the theme still owns how they look.
//!
//! Layout runs along a main axis (left to right for a bottom dock, top to
//! bottom for a side dock) and a cross axis measured outward from the screen
//! edge. The surface spans the whole edge and is tall enough for magnified
//! icons, bounces, and the label; the input region keeps the transparent
//! part of it from taking clicks meant for windows.

use std::f64::consts::PI;
use std::rc::Weak;

use elavo_dock_settings::{Position, Settings};
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{cairo, gdk, gio, glib, graphene, gsk};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

const ITEM_MIME: &str = "application/x-elavo-dock-item";
/// Seconds per bounce.
const BOUNCE_PERIOD: f64 = 0.62;
const SHOW_DELAY_MS: u64 = 220;
const HIDE_DELAY_MS: u64 = 300;
/// How far past the shelf a dragged dock item reads "Remove".
const REMOVE_DISTANCE: f64 = 24.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Apps,
    Recents,
    Divider,
    Others,
    Windows,
    Trash,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Bounce {
    #[default]
    None,
    /// Until the application shows a window.
    Launch,
    /// While the application asks for attention.
    Attention,
}

#[derive(Clone)]
pub enum TileIcon {
    Icon(gio::Icon),
    /// Up to three icons piled on each other, front first.
    Stack(Vec<gio::Icon>),
    /// A minimized window: a card with the application's icon.
    Window(gio::Icon),
}

#[derive(Clone)]
pub struct Tile {
    pub key: String,
    pub section: Section,
    /// `None` for dividers.
    pub icon: Option<TileIcon>,
    pub label: String,
    pub indicator: bool,
    pub badge: Option<String>,
    pub progress: Option<f64>,
    pub bounce: Bounce,
    pub draggable: bool,
}

impl Tile {
    pub fn divider(n: usize) -> Self {
        Self {
            key: format!("divider:{n}"),
            section: Section::Divider,
            icon: None,
            label: String::new(),
            indicator: false,
            badge: None,
            progress: None,
            bounce: Bounce::None,
            draggable: false,
        }
    }
}

/// What a drop would do, decided while the pointer moves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Intent {
    /// A dock item dragged within the applications section, to `slot` among
    /// the other applications there.
    MoveApp {
        key: String,
        slot: usize,
    },
    MoveOther {
        key: String,
        slot: usize,
    },
    /// Application files from outside: pin them at `slot`.
    Pin {
        slot: usize,
    },
    /// Files or folders from outside: add them after the divider.
    AddOther {
        slot: usize,
    },
    OpenWith(String),
    IntoFolder(String),
    Trash,
}

#[derive(Clone, Debug)]
pub enum Payload {
    /// A dock item; the key is in [`Host::dragging`].
    Internal,
    /// Desktop IDs of dropped application files.
    Apps(Vec<String>),
    Files(Vec<gio::File>),
    /// The file list has not arrived yet.
    Pending,
}

/// The dock behind every view.
pub trait Host {
    fn activate(&self, view: &DockView, key: &str);
    fn menu(&self, view: &DockView, key: &str);
    fn drag_begin(&self, key: &str);
    /// The item was dropped where nothing took it.
    fn drag_removed(&self, key: &str);
    fn drag_end(&self);
    fn dragging(&self) -> Option<String>;
    fn classify(&self, files: &[gio::File]) -> Payload;
    fn can_open(&self, key: &str, files: &[gio::File]) -> bool;
    fn accept_drop(&self, intent: Intent, payload: Payload) -> bool;
    /// The divider was dragged; `size` is the requested icon size.
    fn resize(&self, size: i32);
}

#[derive(Clone, Copy, Default)]
struct Anim {
    value: f64,
    target: f64,
}

impl Anim {
    fn at(value: f64) -> Self {
        Self {
            value,
            target: value,
        }
    }

    /// Exponential approach; `rate` is per second. Returns whether it moved.
    fn step(&mut self, dt: f64, rate: f64) -> bool {
        if self.value == self.target {
            return false;
        }
        self.value += (self.target - self.value) * (1.0 - (-dt * rate).exp());
        if (self.value - self.target).abs() < 0.002 {
            self.value = self.target;
        }
        true
    }
}

#[derive(Clone, Copy, Default)]
struct Geometry {
    position: Position,
    size: f64,
    peak: f64,
    gap: f64,
    pad_main: f64,
    /// Between the screen edge side of the shelf and the icons; holds the dot.
    pad_low: f64,
    pad_high: f64,
    margin: f64,
    divider_len: f64,
    headroom: f64,
    label_room: f64,
    magnify: bool,
}

impl Geometry {
    fn new(s: &Settings) -> Self {
        let size = f64::from(s.icon_size);
        let peak = f64::from(s.peak_size());
        Self {
            position: s.position,
            size,
            peak,
            gap: (size * 0.08).round().max(2.0),
            pad_main: (size * 0.12).round().max(4.0),
            pad_low: (size * 0.16).round().max(6.0),
            pad_high: (size * 0.1).round().max(4.0),
            margin: 4.0,
            divider_len: (size * 0.4).round().max(10.0),
            headroom: (peak - size).max(size * 0.62),
            label_room: if s.position.vertical() { 300.0 } else { 46.0 },
            magnify: s.magnification && peak > size,
        }
    }

    fn thickness(&self) -> f64 {
        self.pad_low + self.size + self.pad_high
    }

    /// Distance from the screen edge to the far side of the shelf.
    fn reserved(&self) -> f64 {
        self.margin + self.thickness()
    }

    fn cross_extent(&self) -> i32 {
        (self.reserved() + self.headroom + self.label_room).ceil() as i32
    }
}

struct TileState {
    tile: Tile,
    presence: Anim,
    /// Room opened before this tile during a drag.
    gap: Anim,
    leaving: bool,
    bounce_start: Option<f64>,
    bounce_end: Option<f64>,
    bounce_off: f64,
    start: f64,
    len: f64,
    scale: f64,
    dot: gtk::Box,
    badge: gtk::Label,
    divider: Option<gtk::Separator>,
}

impl TileState {
    fn unparent(&self) {
        self.dot.unparent();
        self.badge.unparent();
        if let Some(divider) = &self.divider {
            divider.unparent();
        }
    }
}

#[derive(Default)]
struct Dnd {
    drop: Option<gdk::Drop>,
    payload: Option<Payload>,
    intent: Option<Intent>,
    /// Main-axis position of the pointer.
    u: f64,
}

#[derive(Default)]
pub struct State {
    settings: Settings,
    geom: Geometry,
    tiles: Vec<TileState>,
    /// Main and cross position of the pointer while it is over the dock.
    pointer: Option<(f64, f64)>,
    last_u: f64,
    mag: Anim,
    /// 0 shown, 1 hidden.
    hide: Anim,
    hovered: Option<String>,
    pressed: Option<String>,
    long_pressed: bool,
    menu_key: Option<String>,
    menus_open: usize,
    drag_key: Option<String>,
    drag_remove_label: Option<gtk::Label>,
    dnd: Option<Dnd>,
    /// Icon size and pointer distance from the screen edge when a divider
    /// drag began.
    resizing: Option<(i32, f64)>,
    last_frame: Option<i64>,
    width: f64,
    height: f64,
    shelf: (f64, f64),
    input: Option<(i32, i32, i32, i32)>,
    timer: Option<glib::SourceId>,
    /// The tile a drag is starting from, and its icon.
    prepared: Option<(String, Option<gio::Icon>)>,
}

impl State {
    fn main_len(&self) -> f64 {
        if self.geom.position.vertical() {
            self.height
        } else {
            self.width
        }
    }

    /// Widget coordinates to (main, cross).
    fn to_uv(&self, x: f64, y: f64) -> (f64, f64) {
        match self.geom.position {
            Position::Bottom => (x, self.height - y),
            Position::Left => (y, x),
            Position::Right => (y, self.width - x),
        }
    }

    /// A (main, cross) box to a widget rectangle.
    fn to_rect(&self, u: f64, v: f64, du: f64, dv: f64) -> graphene::Rect {
        let (x, y, w, h) = match self.geom.position {
            Position::Bottom => (u, self.height - v - dv, du, dv),
            Position::Left => (v, u, dv, du),
            Position::Right => (self.width - v - dv, u, dv, du),
        };
        graphene::Rect::new(x as f32, y as f32, w as f32, h as f32)
    }

    /// The surface is always as deep as the largest dock needs. Compositors
    /// keep reporting pointer positions against the old size until the
    /// pointer moves, so a surface that resized under it (as dragging the
    /// divider does) would misplace presses. The input region keeps the
    /// unused part from taking events.
    fn cross_extent(&self) -> i32 {
        let extent = self.geom.cross_extent();
        let largest = Settings {
            icon_size: elavo_dock_settings::MAX_ICON_SIZE,
            ..self.settings.clone()
        };
        extent.max(Geometry::new(&largest).cross_extent())
    }

    fn hide_offset(&self) -> f64 {
        self.hide.value * (self.geom.reserved() + 2.0)
    }

    fn base_len(&self, t: &TileState) -> f64 {
        let g = &self.geom;
        let full = if t.tile.section == Section::Divider {
            g.divider_len
        } else {
            g.size + g.gap
        };
        full * t.presence.value
    }

    fn layout(&mut self) {
        let main_len = self.main_len();
        let g = self.geom;
        let base: Vec<f64> = self.tiles.iter().map(|t| self.base_len(t)).collect();
        let total: f64 = self
            .tiles
            .iter()
            .zip(&base)
            .map(|(t, len)| t.gap.value + len)
            .sum::<f64>()
            + 2.0 * g.pad_main;
        let origin = (main_len - total) / 2.0;
        let mut starts = Vec::with_capacity(base.len());
        let mut cursor = origin + g.pad_main;
        for (t, len) in self.tiles.iter().zip(&base) {
            cursor += t.gap.value;
            starts.push(cursor);
            cursor += len;
        }

        let mag = self.mag.value;
        let factor = g.peak / g.size;
        let range = (g.size + g.peak) * 1.35;
        let u = self.last_u;
        for (i, t) in self.tiles.iter_mut().enumerate() {
            let divider = t.tile.section == Section::Divider;
            t.scale = if divider || mag <= 0.0 {
                1.0
            } else {
                let d = (u - (starts[i] + base[i] / 2.0)).abs();
                let f = if d < range {
                    (1.0 + (PI * d / range).cos()) / 2.0
                } else {
                    0.0
                };
                1.0 + (factor - 1.0) * f * mag
            };
            t.len = if divider {
                base[i]
            } else {
                (g.size * t.scale + g.gap) * t.presence.value
            };
            t.start = starts[i];
        }

        let n = self.tiles.len();
        if mag <= 0.0 || n == 0 {
            self.shelf = (origin, origin + total);
            return;
        }
        // Anchor at the pointer: the point of the unmagnified dock under the
        // pointer stays under it, and the dock grows away on both sides.
        let ext = |i: usize| self.tiles[i].gap.value + base[i];
        let lo = starts[0] - self.tiles[0].gap.value;
        let hi = starts[n - 1] + base[n - 1];
        let a = u.clamp(lo, hi);
        let k = (0..n).find(|&i| a < starts[i] + base[i]).unwrap_or(n - 1);
        let k_lo = starts[k] - self.tiles[k].gap.value;
        let phi = if ext(k) > 0.0 {
            (a - k_lo) / ext(k)
        } else {
            0.0
        };
        let mag_ext = self.tiles[k].gap.value + self.tiles[k].len;
        let k_start = a - phi * mag_ext;
        self.tiles[k].start = k_start + self.tiles[k].gap.value;
        let mut cursor = k_start + mag_ext;
        for t in &mut self.tiles[k + 1..] {
            cursor += t.gap.value;
            t.start = cursor;
            cursor += t.len;
        }
        let right = cursor;
        let mut cursor = k_start;
        for t in self.tiles[..k].iter_mut().rev() {
            cursor -= t.len;
            t.start = cursor;
            cursor -= t.gap.value;
        }
        self.shelf = (cursor - g.pad_main, right + g.pad_main);
    }

    /// Main start, cross start, and size of a tile's icon.
    ///
    /// A resting icon is centered in the shelf. Magnification grows it from
    /// that top edge, toward the desktop. Bounce offsets from the center.
    fn icon_box(&self, t: &TileState) -> (f64, f64, f64) {
        let g = &self.geom;
        let z = g.size * t.scale * t.presence.value;
        let center = t.start + t.len / 2.0;
        let rest_z = g.size * t.presence.value;
        let rest_v = g.margin + (g.thickness() - rest_z) / 2.0 - self.hide_offset();
        (center - z / 2.0, rest_v + t.bounce_off, z)
    }

    fn icon_rect(&self, t: &TileState) -> graphene::Rect {
        let (u, v, z) = self.icon_box(t);
        self.to_rect(u, v, z, z)
    }

    fn hit(&self, u: f64, v: f64) -> Option<usize> {
        let g = &self.geom;
        self.tiles.iter().position(|t| {
            if t.leaving || t.presence.value < 0.5 {
                return false;
            }
            let (_, iv, z) = self.icon_box(t);
            let top = (iv + z + g.pad_high).max(g.reserved());
            u >= t.start && u < t.start + t.len && v >= 0.0 && v <= top
        })
    }

    fn index(&self, key: &str) -> Option<usize> {
        self.tiles
            .iter()
            .position(|t| !t.leaving && t.tile.key == key)
    }

    fn input_rect(&self) -> (i32, i32, i32, i32) {
        let g = &self.geom;
        let (lo, hi) = self.shelf;
        let rect = if self.hide.target >= 1.0 && self.hide.value > 0.5 {
            self.to_rect(lo, 0.0, hi - lo, 2.0)
        } else {
            let mut top = g.reserved();
            if self.dnd.is_some() || self.drag_key.is_some() {
                top += g.size;
            }
            for t in &self.tiles {
                let (_, v, z) = self.icon_box(t);
                top = top.max(v + z);
            }
            top -= self.hide_offset();
            self.to_rect(lo, 0.0, hi - lo, top.max(2.0))
        };
        (
            rect.x().floor() as i32,
            rect.y().floor() as i32,
            rect.width().ceil() as i32,
            rect.height().ceil() as i32,
        )
    }

    /// Slot among the visible tiles of `sections` (skipping `skip`) for main
    /// position `u`.
    fn slot(&self, sections: &[Section], skip: Option<&str>, u: f64) -> usize {
        self.tiles
            .iter()
            .filter(|t| !t.leaving && sections.contains(&t.tile.section))
            .filter(|t| Some(t.tile.key.as_str()) != skip)
            .filter(|t| t.start + t.len / 2.0 < u)
            .count()
    }

    /// Main-axis position of the divider before recent apps.
    fn recents_begin(&self) -> f64 {
        self.tiles
            .iter()
            .find(|t| !t.leaving && t.tile.key == "divider:0")
            .map_or(f64::INFINITY, |t| t.start + t.len / 2.0)
    }

    /// Main-axis position of the last divider, where files and folders start.
    fn others_begin(&self) -> f64 {
        self.tiles
            .iter()
            .rev()
            .find(|t| !t.leaving && t.tile.section == Section::Divider)
            .map_or(f64::INFINITY, |t| t.start)
    }

    fn gap_for(&mut self, intent: Option<&Intent>) {
        let size = self.geom.size + self.geom.gap;
        let (section, skip, slot) = match intent {
            Some(Intent::MoveApp { key, slot }) => (Some(Section::Apps), Some(key.as_str()), *slot),
            Some(Intent::Pin { slot }) => (Some(Section::Apps), None, *slot),
            Some(Intent::MoveOther { key, slot }) => {
                (Some(Section::Others), Some(key.as_str()), *slot)
            }
            Some(Intent::AddOther { slot }) => (Some(Section::Others), None, *slot),
            _ => (None, None, 0),
        };
        // Before the slot-th tile of the section, or before whatever follows
        // the section when the slot is at its end. Tiles are in section order.
        let target = section.and_then(|section| {
            let mut seen = 0;
            self.tiles
                .iter()
                .enumerate()
                .filter(|(_, t)| !t.leaving && Some(t.tile.key.as_str()) != skip)
                .find(|(_, t)| {
                    if t.tile.section == section {
                        seen += 1;
                        seen > slot
                    } else {
                        t.tile.section.rank() > section.rank()
                    }
                })
                .map(|(i, _)| i)
        });
        for (i, t) in self.tiles.iter_mut().enumerate() {
            t.gap.target = if Some(i) == target { size } else { 0.0 };
        }
    }
}

impl Section {
    fn rank(self) -> u8 {
        match self {
            Section::Apps => 0,
            Section::Recents => 1,
            Section::Divider => 2,
            Section::Others => 3,
            Section::Windows => 4,
            Section::Trash => 5,
        }
    }
}

mod imp {
    use std::cell::{Cell, OnceCell, RefCell};
    use std::rc::Weak;

    use std::collections::HashMap;

    use gtk::glib;
    use gtk::prelude::*;
    use gtk::subclass::prelude::*;

    use super::{Host, State};

    #[derive(Default)]
    pub struct DockView {
        pub host: RefCell<Option<Weak<dyn Host>>>,
        pub state: RefCell<State>,
        /// Icon renderings by icon and lookup size. Separate from `state`
        /// because drawing fills it while `state` is borrowed.
        pub icons: RefCell<HashMap<(String, i32), gtk::IconPaintable>>,
        pub shelf: OnceCell<gtk::Box>,
        pub radius_css: OnceCell<gtk::CssProvider>,
        pub radius_px: Cell<i32>,
        pub label: OnceCell<gtk::Label>,
        pub tick: RefCell<Option<gtk::TickCallbackId>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for DockView {
        const NAME: &'static str = "ElavoDockView";
        type Type = super::DockView;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("elavo-dock-view");
        }
    }

    impl ObjectImpl for DockView {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            let shelf = gtk::Box::builder().css_classes(["elavo-dock-bar"]).build();
            shelf.set_parent(&*obj);
            self.shelf.set(shelf).expect("constructed once");
            let radius_css = gtk::CssProvider::new();
            if let Some(display) = gtk::gdk::Display::default() {
                gtk::style_context_add_provider_for_display(
                    &display,
                    &radius_css,
                    gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
                );
            }
            self.radius_css.set(radius_css).expect("constructed once");
            let label = gtk::Label::builder()
                .css_classes(["elavo-dock-label"])
                .opacity(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .max_width_chars(40)
                .build();
            label.set_parent(&*obj);
            self.label.set(label).expect("constructed once");
        }

        fn dispose(&self) {
            if let Some(tick) = self.tick.take() {
                tick.remove();
            }
            for t in self.state.borrow_mut().tiles.drain(..) {
                t.unparent();
            }
            while let Some(child) = self.obj().first_child() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for DockView {
        fn measure(&self, orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            let state = self.state.borrow();
            let cross = if state.geom.position.vertical() {
                gtk::Orientation::Horizontal
            } else {
                gtk::Orientation::Vertical
            };
            if orientation == cross {
                let extent = state.cross_extent();
                (extent, extent, -1, -1)
            } else {
                (1, 1, -1, -1)
            }
        }

        fn size_allocate(&self, width: i32, height: i32, _baseline: i32) {
            {
                let mut state = self.state.borrow_mut();
                let resized = state.width != f64::from(width) || state.height != f64::from(height);
                state.width = f64::from(width);
                state.height = f64::from(height);
                if resized {
                    state.layout();
                }
            }
            self.obj().allocate_children();
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let obj = self.obj();
            if let Some(shelf) = self.shelf.get() {
                obj.snapshot_child(shelf, snapshot);
            }
            let state = self.state.borrow();
            for t in &state.tiles {
                if let Some(divider) = &t.divider {
                    obj.snapshot_child(divider, snapshot);
                }
            }
            let highlight = state.dnd.as_ref().and_then(|d| match &d.intent {
                Some(super::Intent::OpenWith(k) | super::Intent::IntoFolder(k)) => Some(k.clone()),
                Some(super::Intent::Trash) => Some("trash".to_owned()),
                _ => None,
            });
            for t in &state.tiles {
                let key = Some(&t.tile.key);
                let dark = state.pressed.as_ref() == key
                    || state.menu_key.as_ref() == key
                    || highlight.as_ref() == key;
                obj.paint_icon(&state, t, dark, snapshot);
            }
            for t in &state.tiles {
                obj.snapshot_child(&t.dot, snapshot);
                if t.tile.badge.is_some() {
                    obj.snapshot_child(&t.badge, snapshot);
                }
                if let Some(progress) = t.tile.progress {
                    super::paint_progress(&state, t, progress, snapshot);
                }
            }
            if let Some(label) = self.label.get() {
                obj.snapshot_child(label, snapshot);
            }
        }
    }
}

glib::wrapper! {
    pub struct DockView(ObjectSubclass<imp::DockView>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

/// A bar along the bottom of the icon, filling left to right on every edge.
fn paint_progress(state: &State, t: &TileState, progress: f64, snapshot: &gtk::Snapshot) {
    let icon = state.icon_rect(t);
    let size = icon.width();
    if size < 8.0 {
        return;
    }
    let height = (size * 0.11).max(4.0);
    let trough = graphene::Rect::new(
        icon.x() + size * 0.1,
        icon.y() + size * 0.92 - height,
        size * 0.8,
        height,
    );
    let fill = graphene::Rect::new(
        trough.x(),
        trough.y(),
        trough.width() * progress.clamp(0.0, 1.0) as f32,
        height,
    );
    snapshot.push_rounded_clip(&gsk::RoundedRect::from_rect(trough, height / 2.0));
    snapshot.append_color(&gdk::RGBA::new(0.0, 0.0, 0.0, 0.55), &trough);
    snapshot.append_color(&gdk::RGBA::new(1.0, 1.0, 1.0, 0.95), &fill);
    snapshot.pop();
}

impl DockView {
    pub fn new(host: Weak<dyn Host>, settings: &Settings) -> Self {
        let view: Self = glib::Object::new();
        view.imp().host.replace(Some(host));
        view.set_settings(settings);
        view.connect_input();
        view
    }

    fn host(&self) -> Option<std::rc::Rc<dyn Host>> {
        self.imp().host.borrow().as_ref().and_then(Weak::upgrade)
    }

    fn state(&self) -> std::cell::RefMut<'_, State> {
        self.imp().state.borrow_mut()
    }

    /// A layer-shell window on `monitor` holding a new view.
    pub fn window(
        app: &gtk::Application,
        monitor: &gdk::Monitor,
        host: Weak<dyn Host>,
        settings: &Settings,
    ) -> (gtk::ApplicationWindow, DockView) {
        let window = gtk::ApplicationWindow::builder()
            .application(app)
            .title("Elavo Dock")
            .decorated(false)
            .css_classes(["elavo-dock"])
            .build();
        window.init_layer_shell();
        window.set_namespace(Some("elavo-dock"));
        window.set_layer(Layer::Top);
        window.set_monitor(Some(monitor));
        window.set_keyboard_mode(KeyboardMode::None);
        let view = DockView::new(host, settings);
        window.set_child(Some(&view));
        view.configure_surface(&window);
        let weak = view.downgrade();
        window.connect_realize(move |window| {
            let Some(surface) = window.surface() else {
                return;
            };
            let weak = weak.clone();
            surface.connect_layout(move |_, _, _| {
                if let Some(view) = weak.upgrade() {
                    view.state().input = None;
                    view.update_input_region();
                }
            });
        });
        window.present();
        (window, view)
    }

    /// `0.35 × thickness` (`21/60`). CSS cannot see the widget size.
    fn set_shelf_radius(&self, thickness: f64) {
        let radius = (thickness * 21.0 / 60.0).round() as i32;
        if radius <= 0 || self.imp().radius_px.get() == radius {
            return;
        }
        self.imp().radius_px.set(radius);
        if let Some(css) = self.imp().radius_css.get() {
            css.load_from_string(&format!(".elavo-dock-bar {{ border-radius: {radius}px; }}"));
        }
    }

    fn configure_surface(&self, window: &gtk::ApplicationWindow) {
        let (position, exclusive) = {
            let state = self.imp().state.borrow();
            let exclusive = if state.settings.autohide {
                0
            } else {
                state.geom.reserved().ceil() as i32
            };
            (state.geom.position, exclusive)
        };
        window.set_anchor(Edge::Bottom, true);
        window.set_anchor(Edge::Top, position.vertical());
        window.set_anchor(Edge::Left, position != Position::Right);
        window.set_anchor(Edge::Right, position != Position::Left);
        window.set_exclusive_zone(exclusive);
    }

    pub fn set_settings(&self, settings: &Settings) {
        {
            let mut state = self.state();
            let geom = Geometry::new(settings);
            if geom.position != state.geom.position {
                state.input = None;
            }
            state.geom = geom;
            state.settings = settings.clone();
            state.hide.target = if settings.autohide && state.pointer.is_none() {
                1.0
            } else {
                0.0
            };
            let vertical = geom.position.vertical();
            for t in &state.tiles {
                if let Some(divider) = &t.divider {
                    divider.set_orientation(if vertical {
                        gtk::Orientation::Horizontal
                    } else {
                        gtk::Orientation::Vertical
                    });
                }
            }
        }
        if let Some(window) = self.root().and_downcast::<gtk::ApplicationWindow>() {
            self.configure_surface(&window);
        }
        self.queue_resize();
        self.animate();
    }

    pub fn set_tiles(&self, tiles: Vec<Tile>) {
        let now = glib::monotonic_time() as f64 / 1e6;
        let first = self.imp().state.borrow().tiles.is_empty();
        let mut state = self.state();
        let mut old: Vec<TileState> = std::mem::take(&mut state.tiles);
        let mut merged: Vec<TileState> = Vec::with_capacity(tiles.len());
        let vertical = state.geom.position.vertical();
        for tile in tiles {
            let existing = old.iter().position(|t| t.tile.key == tile.key);
            let mut ts = match existing {
                Some(i) => {
                    let mut ts = old.remove(i);
                    ts.leaving = false;
                    ts.presence.target = 1.0;
                    ts
                }
                None => {
                    let dot = gtk::Box::builder().css_classes(["elavo-dock-dot"]).build();
                    dot.set_parent(self);
                    let badge = gtk::Label::builder()
                        .css_classes(["elavo-dock-badge"])
                        .build();
                    badge.set_parent(self);
                    let divider = (tile.section == Section::Divider).then(|| {
                        let divider = gtk::Separator::builder()
                            .orientation(if vertical {
                                gtk::Orientation::Horizontal
                            } else {
                                gtk::Orientation::Vertical
                            })
                            .css_classes(["elavo-dock-separator"])
                            .build();
                        divider.set_parent(self);
                        divider
                    });
                    TileState {
                        tile: tile.clone(),
                        presence: if first {
                            Anim::at(1.0)
                        } else {
                            Anim {
                                value: 0.0,
                                target: 1.0,
                            }
                        },
                        gap: Anim::default(),
                        leaving: false,
                        bounce_start: None,
                        bounce_end: None,
                        bounce_off: 0.0,
                        start: 0.0,
                        len: 0.0,
                        scale: 1.0,
                        dot,
                        badge,
                        divider,
                    }
                }
            };
            if ts.tile.badge != tile.badge {
                ts.badge.set_label(tile.badge.as_deref().unwrap_or(""));
            }
            ts.dot.set_opacity(if tile.indicator { 1.0 } else { 0.0 });
            ts.badge
                .set_opacity(if tile.badge.is_some() { 1.0 } else { 0.0 });
            match (tile.bounce, ts.bounce_start) {
                (Bounce::None, Some(start)) if ts.bounce_end.is_none() => {
                    let cycles = ((now - start) / BOUNCE_PERIOD).ceil().max(1.0);
                    ts.bounce_end = Some(start + cycles * BOUNCE_PERIOD);
                }
                (Bounce::Launch | Bounce::Attention, None) => {
                    ts.bounce_start = Some(now);
                    ts.bounce_end = None;
                }
                (Bounce::Launch | Bounce::Attention, Some(_)) => ts.bounce_end = None,
                _ => {}
            }
            ts.tile = tile;
            merged.push(ts);
        }
        // Tiles that went away shrink out where they were.
        for (i, mut ts) in old.into_iter().enumerate() {
            ts.leaving = true;
            ts.presence.target = 0.0;
            ts.gap.target = 0.0;
            let at = merged.len().min(i);
            merged.insert(at, ts);
        }
        state.tiles = merged;
        if let Some(key) = state.drag_key.clone() {
            if let Some(i) = state.index(&key) {
                state.tiles[i].presence.target = 0.0;
            }
        }
        if state.dnd.is_some() {
            let intent = state.dnd.as_ref().and_then(|d| d.intent.clone());
            state.gap_for(intent.as_ref());
        }
        drop(state);
        self.update_hover();
        self.animate();
    }

    /// The rectangle a menu or stack for `key` points at: a line across the
    /// tile just outside the shelf and its magnified icon.
    pub fn tile_rect(&self, key: &str) -> Option<gdk::Rectangle> {
        let state = self.imp().state.borrow();
        let i = state.index(key)?;
        let t = &state.tiles[i];
        let (_, v, z) = state.icon_box(t);
        let outside = (v + z).max(state.geom.reserved()) + 4.0;
        let rect = state.to_rect(t.start, outside, t.len, 1.0);
        Some(gdk::Rectangle::new(
            rect.x() as i32,
            rect.y() as i32,
            rect.width().max(1.0) as i32,
            rect.height().max(1.0) as i32,
        ))
    }

    /// Show `popover` against the tile for `key`. The view keeps itself
    /// shown and unmagnified until the popover closes.
    pub fn popup(&self, popover: &gtk::Popover, key: &str) {
        let Some(rect) = self.tile_rect(key) else {
            return;
        };
        let position = match self.imp().state.borrow().geom.position {
            Position::Bottom => gtk::PositionType::Top,
            Position::Left => gtk::PositionType::Right,
            Position::Right => gtk::PositionType::Left,
        };
        {
            let mut state = self.state();
            state.menus_open += 1;
            state.menu_key = Some(key.to_owned());
            state.pressed = None;
        }
        popover.set_parent(self);
        popover.set_pointing_to(Some(&rect));
        popover.set_position(position);
        let weak = self.downgrade();
        popover.connect_closed(move |popover| {
            if let Some(view) = weak.upgrade() {
                {
                    let mut state = view.state();
                    state.pointer = None;
                    state.menus_open = state.menus_open.saturating_sub(1);
                    if state.menus_open == 0 {
                        state.menu_key = None;
                    }
                }
                view.update_hover();
                view.schedule_autohide();
                view.animate();
            }
            let popover = popover.clone();
            glib::idle_add_local_once(move || popover.unparent());
        });
        self.update_hover();
        self.animate();
        popover.popup();
    }

    fn allocate_children(&self) {
        let imp = self.imp();
        let state = imp.state.borrow();
        let place = |child: &gtk::Widget, rect: &graphene::Rect| {
            let _ = child.preferred_size();
            child.allocate(
                rect.width().round().max(0.0) as i32,
                rect.height().round().max(0.0) as i32,
                -1,
                Some(
                    gsk::Transform::new()
                        .translate(&graphene::Point::new(rect.x().round(), rect.y().round())),
                ),
            );
        };
        let g = &state.geom;
        let hide = state.hide_offset();
        if let Some(shelf) = imp.shelf.get() {
            let (lo, hi) = state.shelf;
            let thickness = g.thickness();
            place(
                shelf.upcast_ref(),
                &state.to_rect(lo, g.margin - hide, (hi - lo).max(0.0), thickness),
            );
            self.set_shelf_radius(thickness);
        }
        for t in &state.tiles {
            let center = t.start + t.len / 2.0;
            if let Some(divider) = &t.divider {
                let inset = (g.pad_low + g.pad_high) / 2.0 * 0.6;
                let length = (g.thickness() - inset * 2.0) * t.presence.value;
                let line = 1.0;
                place(
                    divider.upcast_ref(),
                    &state.to_rect(center - line / 2.0, g.margin + inset - hide, line, length),
                );
            }
            let (_, natural) = t.dot.preferred_size();
            let (dw, dh) = (f64::from(natural.width()), f64::from(natural.height()));
            let (du, dv) = if g.position.vertical() {
                (dh, dw)
            } else {
                (dw, dh)
            };
            place(
                t.dot.upcast_ref(),
                &state.to_rect(
                    center - du / 2.0,
                    g.margin + (g.pad_low - dv) / 2.0 - hide,
                    du,
                    dv,
                ),
            );
            let (_, natural) = t.badge.preferred_size();
            let icon = state.icon_rect(t);
            let (bw, bh) = (natural.width() as f32, natural.height() as f32);
            let bw = bw.max(bh);
            place(
                t.badge.upcast_ref(),
                &graphene::Rect::new(
                    icon.x() + icon.width() - bw * 0.75,
                    icon.y() - bh * 0.2,
                    bw,
                    bh,
                ),
            );
        }
        if let Some(label) = imp.label.get() {
            let (_, natural) = label.preferred_size();
            let (lw, lh) = (f64::from(natural.width()), f64::from(natural.height()));
            let hovered = state.hovered.as_deref().and_then(|k| state.index(k));
            let rect = match hovered {
                Some(i) => {
                    let t = &state.tiles[i];
                    let (u, v, z) = state.icon_box(t);
                    let center = u + z / 2.0;
                    let gap = 6.0;
                    if g.position.vertical() {
                        state.to_rect(center - lh / 2.0, v + z + gap, lh, lw)
                    } else {
                        let x = (center - lw / 2.0).clamp(2.0, (state.width - lw - 2.0).max(2.0));
                        state.to_rect(x, v + z + gap, lw, lh)
                    }
                }
                None => graphene::Rect::new(0.0, 0.0, lw as f32, lh as f32),
            };
            place(label.upcast_ref(), &rect);
        }
        drop(state);
        let mut child = self.first_child();
        while let Some(widget) = child {
            child = widget.next_sibling();
            if let Some(popover) = widget.downcast_ref::<gtk::Popover>() {
                popover.present();
            }
        }
    }

    fn paint_icon(&self, state: &State, t: &TileState, dark: bool, snapshot: &gtk::Snapshot) {
        let Some(icon) = &t.tile.icon else {
            return;
        };
        let rect = state.icon_rect(t);
        let size = rect.width();
        if size < 1.0 {
            return;
        }
        if dark {
            let m = 0.62;
            snapshot.push_color_matrix(
                &graphene::Matrix::from_float([
                    m, 0.0, 0.0, 0.0, 0.0, m, 0.0, 0.0, 0.0, 0.0, m, 0.0, 0.0, 0.0, 0.0, 1.0,
                ]),
                &graphene::Vec4::zero(),
            );
        }
        let lookup = f64::from(size) * 1.0;
        match icon {
            TileIcon::Icon(gicon) => self.draw(snapshot, gicon, &rect, lookup),
            TileIcon::Stack(icons) => {
                let tilts = [0.0_f32, -7.0, 6.0];
                for (i, gicon) in icons.iter().enumerate().take(3).rev() {
                    let shrink = 1.0 - 0.06 * i as f32;
                    let w = size * shrink;
                    let r = graphene::Rect::new(
                        rect.x() + (size - w) / 2.0,
                        rect.y() + (size - w) / 2.0 - 2.0 * i as f32,
                        w,
                        w,
                    );
                    snapshot.save();
                    let center = graphene::Point::new(r.x() + w / 2.0, r.y() + w / 2.0);
                    snapshot.translate(&center);
                    snapshot.rotate(tilts[i]);
                    snapshot.translate(&graphene::Point::new(-center.x(), -center.y()));
                    self.draw(snapshot, gicon, &r, lookup);
                    snapshot.restore();
                }
            }
            TileIcon::Window(gicon) => {
                let inset = size * 0.08;
                let card = graphene::Rect::new(
                    rect.x() + inset,
                    rect.y() + inset * 1.6,
                    size - 2.0 * inset,
                    size - 3.2 * inset,
                );
                let fg = self.color();
                let radius = size * 0.08;
                snapshot.push_rounded_clip(&gsk::RoundedRect::from_rect(card, radius));
                snapshot.append_color(
                    &gdk::RGBA::new(fg.red(), fg.green(), fg.blue(), 0.16),
                    &card,
                );
                snapshot.pop();
                let rounded = gsk::RoundedRect::from_rect(card, radius);
                let edge = gdk::RGBA::new(fg.red(), fg.green(), fg.blue(), 0.28);
                snapshot.append_border(&rounded, &[1.0; 4], &[edge, edge, edge, edge]);
                let w = size * 0.56;
                let r = graphene::Rect::new(
                    rect.x() + (size - w) / 2.0,
                    rect.y() + (size - w) / 2.0,
                    w,
                    w,
                );
                self.draw(snapshot, gicon, &r, lookup * 0.56);
            }
        }
        if dark {
            snapshot.pop();
        }
    }

    fn draw(&self, snapshot: &gtk::Snapshot, gicon: &gio::Icon, rect: &graphene::Rect, size: f64) {
        // Look up in steps of 16 so magnification reuses a few renderings.
        let bucket = (((size.max(16.0)) / 16.0).ceil() * 16.0) as i32;
        let key = (
            IconExt::to_string(gicon)
                .map(|s| s.to_string())
                .unwrap_or_default(),
            bucket,
        );
        let paintable = self
            .imp()
            .icons
            .borrow_mut()
            .entry(key)
            .or_insert_with(|| {
                gtk::IconTheme::for_display(&self.display()).lookup_by_gicon(
                    gicon,
                    bucket,
                    self.scale_factor(),
                    gtk::TextDirection::None,
                    gtk::IconLookupFlags::empty(),
                )
            })
            .clone();
        snapshot.save();
        snapshot.translate(&graphene::Point::new(rect.x(), rect.y()));
        paintable.snapshot(snapshot, f64::from(rect.width()), f64::from(rect.height()));
        snapshot.restore();
    }

    /// Run the frame loop until everything settles.
    fn animate(&self) {
        let imp = self.imp();
        if imp.tick.borrow().is_some() {
            return;
        }
        self.imp().state.borrow_mut().last_frame = None;
        let id = self.add_tick_callback(|view, clock| {
            if view.frame(clock.frame_time()) {
                glib::ControlFlow::Continue
            } else {
                view.imp().tick.replace(None);
                glib::ControlFlow::Break
            }
        });
        imp.tick.replace(Some(id));
    }

    /// One animation frame. Returns whether another is needed.
    fn frame(&self, time: i64) -> bool {
        let mut state = self.state();
        let dt = state
            .last_frame
            .map_or(0.016, |last| ((time - last) as f64 / 1e6).clamp(0.0, 0.05));
        state.last_frame = Some(time);
        let now = time as f64 / 1e6;

        let interacting = state.dnd.is_some()
            || state.drag_key.is_some()
            || state.menus_open > 0
            || state.resizing.is_some();
        state.mag.target = if state.geom.magnify
            && state.pointer.is_some()
            && !interacting
            && state.hide.target == 0.0
        {
            1.0
        } else {
            0.0
        };
        let mut moving = state.mag.step(dt, 16.0);
        moving |= state.hide.step(dt, 11.0);
        let size = state.geom.size;
        let animate_bounce = state.settings.animate_launch;
        for t in &mut state.tiles {
            moving |= t.presence.step(dt, 13.0);
            moving |= t.gap.step(dt, 16.0);
            t.bounce_off = 0.0;
            let Some(start) = t.bounce_start else {
                continue;
            };
            if t.bounce_end.is_some_and(|end| now >= end) {
                t.bounce_start = None;
                t.bounce_end = None;
                continue;
            }
            moving = true;
            let attention = t.tile.bounce == Bounce::Attention;
            if !animate_bounce && !attention {
                continue;
            }
            let x = ((now - start) / BOUNCE_PERIOD).fract();
            let height = size * if attention { 0.62 } else { 0.5 };
            t.bounce_off = height * 4.0 * x * (1.0 - x);
        }
        let mut gone = Vec::new();
        state.tiles.retain(|t| {
            let keep = !(t.leaving && t.presence.value <= 0.0);
            if !keep {
                gone.push((t.dot.clone(), t.badge.clone(), t.divider.clone()));
            }
            keep
        });
        state.layout();
        let label_on = state.hovered.is_some()
            && state.pressed.is_none()
            && state.menus_open == 0
            && state.resizing.is_none()
            && state.drag_key.is_none()
            && state.hide.value < 0.2;
        drop(state);
        for (dot, badge, divider) in gone {
            dot.unparent();
            badge.unparent();
            if let Some(divider) = divider {
                divider.unparent();
            }
        }
        if let Some(label) = self.imp().label.get() {
            label.set_opacity(if label_on { 1.0 } else { 0.0 });
        }
        self.update_input_region();
        self.queue_allocate();
        self.queue_draw();
        moving
    }

    fn update_input_region(&self) {
        let rect = {
            let state = self.imp().state.borrow();
            if state.width <= 0.0 {
                return;
            }
            let rect = state.input_rect();
            if state.input == Some(rect) {
                return;
            }
            rect
        };
        let Some(surface) = self.native().and_then(|n| n.surface()) else {
            return;
        };
        let (x, y, w, h) = rect;
        let region =
            cairo::Region::create_rectangle(&cairo::RectangleInt::new(x, y, w.max(1), h.max(1)));
        surface.set_input_region(Some(&region));
        self.state().input = Some(rect);
    }

    fn update_hover(&self) {
        let (label, changed) = {
            let mut state = self.state();
            let target = state.dnd.as_ref().and_then(|d| match &d.intent {
                Some(Intent::OpenWith(k) | Intent::IntoFolder(k)) => Some(k.clone()),
                Some(Intent::Trash) => Some("trash".to_owned()),
                _ => None,
            });
            let hovered = match (target, state.pointer) {
                (Some(key), _) => Some(key),
                (None, Some((u, v))) if state.dnd.is_none() => state
                    .hit(u, v)
                    .map(|i| &state.tiles[i])
                    .filter(|t| t.tile.section != Section::Divider)
                    .map(|t| t.tile.key.clone()),
                _ => None,
            };
            let changed = hovered != state.hovered;
            state.hovered = hovered.clone();
            let label = hovered
                .and_then(|k| state.index(&k))
                .map(|i| state.tiles[i].tile.label.clone());
            (label, changed)
        };
        if let (true, Some(widget)) = (changed, self.imp().label.get()) {
            widget.set_label(label.as_deref().unwrap_or(""));
        }
        self.update_cursor();
        self.animate();
    }

    fn update_cursor(&self) {
        let name = {
            let state = self.imp().state.borrow();
            let on_divider = state.resizing.is_some()
                || state
                    .pointer
                    .and_then(|(u, v)| state.hit(u, v))
                    .is_some_and(|i| state.tiles[i].tile.section == Section::Divider);
            match (on_divider, state.geom.position.vertical()) {
                (true, false) => Some("ns-resize"),
                (true, true) => Some("ew-resize"),
                (false, _) => None,
            }
        };
        self.set_cursor_from_name(name);
    }

    fn set_pointer(&self, x: f64, y: f64) {
        {
            let mut state = self.state();
            let (u, v) = state.to_uv(x, y);
            state.pointer = Some((u, v));
            state.last_u = u;
        }
        self.update_hover();
    }

    /// Whether the event `controller` is handling happened on the dock's own
    /// surface rather than in one of its menus or stacks.
    fn owns_event(&self, controller: &impl IsA<gtk::EventController>) -> bool {
        let own = self.native().and_then(|n| n.surface());
        controller
            .current_event()
            .and_then(|e| e.surface())
            .is_none_or(|s| Some(s) == own)
    }

    fn key_at(&self, x: f64, y: f64) -> Option<(String, Section)> {
        let state = self.imp().state.borrow();
        let (u, v) = state.to_uv(x, y);
        state
            .hit(u, v)
            .map(|i| (state.tiles[i].tile.key.clone(), state.tiles[i].tile.section))
    }

    fn reveal(&self) {
        let mut state = self.state();
        if let Some(timer) = state.timer.take() {
            timer.remove();
        }
        state.hide.target = 0.0;
        drop(state);
        self.animate();
    }

    fn schedule_reveal(&self) {
        let mut state = self.state();
        if !state.settings.autohide || state.hide.target == 0.0 || state.timer.is_some() {
            return;
        }
        let weak = self.downgrade();
        state.timer = Some(glib::timeout_add_local_once(
            std::time::Duration::from_millis(SHOW_DELAY_MS),
            move || {
                if let Some(view) = weak.upgrade() {
                    view.state().timer = None;
                    if view.imp().state.borrow().pointer.is_some() {
                        view.reveal();
                    }
                }
            },
        ));
    }

    fn schedule_autohide(&self) {
        let mut state = self.state();
        let busy = state.pointer.is_some()
            || state.menus_open > 0
            || state.drag_key.is_some()
            || state.dnd.is_some()
            || state.resizing.is_some();
        if !state.settings.autohide || busy || state.hide.target == 1.0 {
            return;
        }
        if let Some(timer) = state.timer.take() {
            timer.remove();
        }
        let weak = self.downgrade();
        state.timer = Some(glib::timeout_add_local_once(
            std::time::Duration::from_millis(HIDE_DELAY_MS),
            move || {
                if let Some(view) = weak.upgrade() {
                    let mut state = view.state();
                    state.timer = None;
                    if state.pointer.is_none() && state.menus_open == 0 && state.dnd.is_none() {
                        state.hide.target = 1.0;
                        drop(state);
                        view.animate();
                    }
                }
            },
        ));
    }

    fn connect_input(&self) {
        let motion = gtk::EventControllerMotion::new();
        let weak = self.downgrade();
        motion.connect_enter(move |motion, x, y| {
            if let Some(view) = weak.upgrade() {
                if !view.owns_event(motion) {
                    return;
                }
                view.set_pointer(x, y);
                let hidden = view.imp().state.borrow().hide.target == 1.0;
                if hidden {
                    view.schedule_reveal();
                } else if let Some(timer) = view.state().timer.take() {
                    timer.remove();
                }
            }
        });
        let weak = self.downgrade();
        motion.connect_motion(move |motion, x, y| {
            if let Some(view) = weak.upgrade() {
                if view.owns_event(motion) {
                    view.set_pointer(x, y);
                }
            }
        });
        let weak = self.downgrade();
        motion.connect_leave(move |_| {
            if let Some(view) = weak.upgrade() {
                view.state().pointer = None;
                view.update_hover();
                view.schedule_autohide();
            }
        });
        self.add_controller(motion);

        let click = gtk::GestureClick::builder()
            .button(gdk::BUTTON_PRIMARY)
            .build();
        let weak = self.downgrade();
        click.connect_pressed(move |click, _, x, y| {
            if let Some(view) = weak.upgrade() {
                if !view.owns_event(click) {
                    return;
                }
                let key = view.key_at(x, y).filter(|(_, s)| *s != Section::Divider);
                let mut state = view.state();
                state.long_pressed = false;
                state.pressed = key.map(|(k, _)| k);
                drop(state);
                view.update_hover();
            }
        });
        let weak = self.downgrade();
        click.connect_released(move |_, _, x, y| {
            let Some(view) = weak.upgrade() else {
                return;
            };
            let pressed = view.state().pressed.take();
            let long = view.imp().state.borrow().long_pressed;
            let here = view.key_at(x, y).map(|(k, _)| k);
            view.update_hover();
            if let (Some(key), false) = (pressed, long) {
                if here.as_ref() == Some(&key) {
                    if let Some(host) = view.host() {
                        host.activate(&view, &key);
                    }
                }
            }
        });
        let weak = self.downgrade();
        click.connect_cancel(move |_, _| {
            if let Some(view) = weak.upgrade() {
                view.state().pressed = None;
                view.update_hover();
            }
        });
        self.add_controller(click);

        let hold = gtk::GestureLongPress::new();
        let weak = self.downgrade();
        hold.connect_pressed(move |hold, x, y| {
            let Some(view) = weak.upgrade().filter(|v| v.owns_event(hold)) else {
                return;
            };
            let Some((key, section)) = view.key_at(x, y) else {
                return;
            };
            view.state().long_pressed = true;
            if section != Section::Divider {
                if let Some(host) = view.host() {
                    host.menu(&view, &key);
                }
            }
        });
        self.add_controller(hold);

        let secondary = gtk::GestureClick::builder()
            .button(gdk::BUTTON_SECONDARY)
            .build();
        let weak = self.downgrade();
        secondary.connect_pressed(move |secondary, _, x, y| {
            let Some(view) = weak.upgrade().filter(|v| v.owns_event(secondary)) else {
                return;
            };
            if let (Some((key, _)), Some(host)) = (view.key_at(x, y), view.host()) {
                host.menu(&view, &key);
            }
        });
        self.add_controller(secondary);

        self.connect_resize();
        self.connect_drag_source();
        self.connect_drop_target();
    }

    /// Dragging the divider changes the icon size.
    fn connect_resize(&self) {
        let drag = gtk::GestureDrag::builder()
            .button(gdk::BUTTON_PRIMARY)
            .build();
        let weak = self.downgrade();
        drag.connect_drag_begin(move |gesture, x, y| {
            let Some(view) = weak.upgrade() else {
                return;
            };
            match view.key_at(x, y).filter(|_| view.owns_event(gesture)) {
                Some((_, Section::Divider)) => {
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                    let mut state = view.state();
                    let (_, v) = state.to_uv(x, y);
                    state.resizing = Some((state.settings.icon_size, v));
                }
                _ => {
                    gesture.set_state(gtk::EventSequenceState::Denied);
                }
            }
        });
        let weak = self.downgrade();
        drag.connect_drag_update(move |gesture, dx, dy| {
            let Some(view) = weak.upgrade() else {
                return;
            };
            let size = {
                let state = view.imp().state.borrow();
                let (Some((size, from)), Some((x, y))) = (state.resizing, gesture.start_point())
                else {
                    return;
                };
                let (_, to) = state.to_uv(x + dx, y + dy);
                size + (to - from).round() as i32
            };
            if let Some(host) = view.host() {
                host.resize(size);
            }
        });
        let weak = self.downgrade();
        drag.connect_drag_end(move |_, _, _| {
            if let Some(view) = weak.upgrade() {
                view.state().resizing = None;
                view.update_cursor();
                view.animate();
            }
        });
        self.add_controller(drag);
    }

    fn connect_drag_source(&self) {
        let source = gtk::DragSource::builder()
            .actions(gdk::DragAction::MOVE)
            .build();
        let weak = self.downgrade();
        source.connect_prepare(move |source, x, y| {
            let view = weak.upgrade().filter(|v| v.owns_event(source))?;
            let mut state = view.state();
            let (u, v) = state.to_uv(x, y);
            let t = &state.tiles[state.hit(u, v)?];
            if !t.tile.draggable || state.resizing.is_some() {
                return None;
            }
            let icon = match &t.tile.icon {
                Some(TileIcon::Icon(g) | TileIcon::Window(g)) => Some(g.clone()),
                Some(TileIcon::Stack(icons)) => icons.first().cloned(),
                None => None,
            };
            let key = t.tile.key.clone();
            state.prepared = Some((key.clone(), icon));
            Some(gdk::ContentProvider::for_bytes(
                ITEM_MIME,
                &glib::Bytes::from(key.as_bytes()),
            ))
        });
        let weak = self.downgrade();
        source.connect_drag_begin(move |_, drag| {
            let Some(view) = weak.upgrade() else {
                return;
            };
            let Some((key, icon)) = view.state().prepared.take() else {
                return;
            };
            let size = view.imp().state.borrow().geom.size as i32;
            let image = gtk::Image::builder().pixel_size(size).build();
            if let Some(gicon) = &icon {
                image.set_from_gicon(gicon);
            }
            let remove = gtk::Label::builder()
                .label("Remove")
                .css_classes(["elavo-dock-label"])
                .opacity(0.0)
                .build();
            let content = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(4)
                .build();
            content.append(&image);
            content.append(&remove);
            gtk::DragIcon::for_drag(drag).set_child(Some(&content));
            drag.set_hotspot(size / 2, size / 2);
            {
                let mut state = view.state();
                state.drag_key = Some(key.clone());
                state.drag_remove_label = Some(remove);
                state.pressed = None;
                if let Some(i) = state.index(&key) {
                    state.tiles[i].presence.target = 0.0;
                }
            }
            if let Some(host) = view.host() {
                host.drag_begin(&key);
            }
            view.update_hover();
        });
        let weak = self.downgrade();
        source.connect_drag_cancel(move |_, _, reason| {
            if let Some(view) = weak.upgrade() {
                if reason == gdk::DragCancelReason::UserCancelled {
                    if let Some(label) = &view.state().drag_remove_label {
                        label.set_opacity(0.0);
                    }
                }
            }
            false
        });
        let weak = self.downgrade();
        // Released while it reads "Remove", the item goes, whether or not the
        // window under the pointer took the drop.
        source.connect_drag_end(move |_, _, _| {
            let Some(view) = weak.upgrade() else {
                return;
            };
            let (key, remove) = {
                let mut state = view.state();
                let remove = state
                    .drag_remove_label
                    .take()
                    .is_some_and(|label| label.opacity() > 0.5);
                (state.drag_key.take(), remove)
            };
            if let Some(key) = &key {
                if remove {
                    if let Some(host) = view.host() {
                        host.drag_removed(key);
                    }
                }
                let mut state = view.state();
                if let Some(i) = state.index(key) {
                    state.tiles[i].presence.target = 1.0;
                }
            }
            if let Some(host) = view.host() {
                host.drag_end();
            }
            view.update_hover();
            view.schedule_autohide();
        });
        self.add_controller(source);
    }

    fn connect_drop_target(&self) {
        let formats = gdk::ContentFormatsBuilder::new()
            .add_mime_type(ITEM_MIME)
            .add_type(gdk::FileList::static_type())
            .build();
        let target = gtk::DropTargetAsync::new(
            Some(formats),
            gdk::DragAction::COPY | gdk::DragAction::MOVE | gdk::DragAction::LINK,
        );
        let weak = self.downgrade();
        target.connect_drag_enter(move |_, drop, x, y| {
            let Some(view) = weak.upgrade() else {
                return gdk::DragAction::empty();
            };
            view.reveal();
            let internal = drop.formats().contain_mime_type(ITEM_MIME);
            let payload = if internal {
                Payload::Internal
            } else {
                view.load_files(drop);
                Payload::Pending
            };
            {
                let mut state = view.state();
                state.dnd = Some(Dnd {
                    drop: Some(drop.clone()),
                    payload: Some(payload),
                    intent: None,
                    u: 0.0,
                });
                if let Some(label) = &state.drag_remove_label {
                    label.set_opacity(0.0);
                }
            }
            view.dnd_motion(drop, x, y)
        });
        let weak = self.downgrade();
        target.connect_drag_motion(move |_, drop, x, y| match weak.upgrade() {
            Some(view) => view.dnd_motion(drop, x, y),
            None => gdk::DragAction::empty(),
        });
        let weak = self.downgrade();
        target.connect_drag_leave(move |_, _| {
            if let Some(view) = weak.upgrade() {
                view.dnd_clear();
            }
        });
        let weak = self.downgrade();
        target.connect_drop(move |_, drop, x, y| {
            let Some(view) = weak.upgrade() else {
                return false;
            };
            view.dnd_drop(drop, x, y)
        });
        self.add_controller(target);
    }

    fn load_files(&self, drop: &gdk::Drop) {
        let weak = self.downgrade();
        let drop = drop.clone();
        glib::spawn_future_local(async move {
            let value = drop
                .read_value_future(gdk::FileList::static_type(), glib::Priority::DEFAULT)
                .await;
            let files = value
                .ok()
                .and_then(|v| v.get::<gdk::FileList>().ok())
                .map(|l| l.files())
                .unwrap_or_default();
            let Some(view) = weak.upgrade() else {
                return;
            };
            let Some(host) = view.host() else {
                return;
            };
            let payload = host.classify(&files);
            let u = {
                let mut state = view.state();
                let Some(dnd) = state
                    .dnd
                    .as_mut()
                    .filter(|d| d.drop.as_ref() == Some(&drop))
                else {
                    return;
                };
                dnd.payload = Some(payload);
                dnd.u
            };
            view.update_intent(u);
        });
    }

    fn dnd_motion(&self, drop: &gdk::Drop, x: f64, y: f64) -> gdk::DragAction {
        let u = {
            let mut state = self.state();
            let (u, v) = state.to_uv(x, y);
            let internal = matches!(
                state.dnd.as_ref().and_then(|d| d.payload.as_ref()),
                Some(Payload::Internal)
            );
            // The surface reaches well above the shelf; a dock item dragged
            // that far out is being removed, as if it had left the dock.
            let away = internal && v > state.geom.reserved() + REMOVE_DISTANCE;
            if let Some(label) = &state.drag_remove_label {
                label.set_opacity(if away { 1.0 } else { 0.0 });
            }
            if away {
                state.pointer = None;
                if let Some(dnd) = state.dnd.as_mut() {
                    dnd.intent = None;
                }
                state.gap_for(None);
                std::mem::drop(state);
                self.update_hover();
                return gdk::DragAction::empty();
            }
            state.pointer = Some((u, v));
            state.last_u = u;
            match state.dnd.as_mut() {
                Some(dnd) => dnd.u = u,
                None => return gdk::DragAction::empty(),
            }
            u
        };
        self.update_intent(u);
        let intent = self
            .imp()
            .state
            .borrow()
            .dnd
            .as_ref()
            .and_then(|d| d.intent.clone());
        action_for(intent.as_ref(), drop.actions())
    }

    fn update_intent(&self, u: f64) {
        let Some(host) = self.host() else {
            return;
        };
        let intent = {
            let state = self.imp().state.borrow();
            let Some(dnd) = state.dnd.as_ref() else {
                return;
            };
            let over = state
                .pointer
                .and_then(|(u, v)| state.hit(u, v))
                .map(|i| &state.tiles[i]);
            let after_divider = u > state.others_begin();
            match dnd.payload.as_ref() {
                Some(Payload::Internal) => host.dragging().and_then(|key| {
                    let section = state
                        .tiles
                        .iter()
                        .find(|t| t.tile.key == key)
                        .map(|t| t.tile.section)?;
                    match section {
                        Section::Others => Some(Intent::MoveOther {
                            slot: state.slot(&[Section::Others], Some(&key), u),
                            key,
                        }),
                        // A recent app is kept only once it is dragged in
                        // among the kept ones; elsewhere it snaps back.
                        Section::Recents if u > state.recents_begin() => None,
                        _ => Some(Intent::MoveApp {
                            slot: state.slot(&[Section::Apps], Some(&key), u),
                            key,
                        }),
                    }
                }),
                Some(Payload::Apps(_)) => Some(Intent::Pin {
                    slot: state.slot(&[Section::Apps], None, u),
                }),
                Some(Payload::Files(files)) => {
                    let centered = over.is_some_and(|t| {
                        let (iu, _, z) = state.icon_box(t);
                        (u - (iu + z / 2.0)).abs() < z * 0.3
                    });
                    match over.map(|t| (t.tile.section, t.tile.key.clone())) {
                        Some((Section::Trash, _)) => Some(Intent::Trash),
                        Some((Section::Apps | Section::Recents, key)) => {
                            host.can_open(&key, files).then_some(Intent::OpenWith(key))
                        }
                        Some((Section::Others, key)) if centered && host.can_open(&key, files) => {
                            Some(Intent::IntoFolder(key))
                        }
                        _ if after_divider => Some(Intent::AddOther {
                            slot: state.slot(&[Section::Others], None, u),
                        }),
                        _ => None,
                    }
                }
                Some(Payload::Pending) | None => None,
            }
        };
        {
            let mut state = self.state();
            if let Some(dnd) = state.dnd.as_mut() {
                dnd.intent = intent.clone();
            }
            state.gap_for(intent.as_ref());
        }
        self.update_hover();
    }

    fn dnd_clear(&self) {
        {
            let mut state = self.state();
            state.dnd = None;
            state.pointer = None;
            state.gap_for(None);
            if let Some(label) = &state.drag_remove_label {
                label.set_opacity(1.0);
            }
        }
        self.update_hover();
        self.schedule_autohide();
    }

    fn dnd_drop(&self, drop: &gdk::Drop, x: f64, y: f64) -> bool {
        self.dnd_motion(drop, x, y);
        let (intent, payload) = {
            let state = self.imp().state.borrow();
            let dnd = state.dnd.as_ref();
            (
                dnd.and_then(|d| d.intent.clone()),
                dnd.and_then(|d| d.payload.clone()),
            )
        };
        let Some(host) = self.host() else {
            return false;
        };
        match (intent, payload) {
            (Some(intent), Some(payload)) if !matches!(payload, Payload::Pending) => {
                let action = action_for(Some(&intent), drop.actions());
                // Close the gap in the same frame the dock gains the item.
                {
                    let mut state = self.state();
                    state.dnd = None;
                    for t in &mut state.tiles {
                        t.gap = Anim::at(0.0);
                    }
                }
                let done = host.accept_drop(intent, payload);
                self.settle_dragged();
                drop.finish(if done {
                    action
                } else {
                    gdk::DragAction::empty()
                });
                true
            }
            _ => {
                drop.finish(gdk::DragAction::empty());
                false
            }
        }
    }

    /// After a reorder the dragged tile is in its new place at full size.
    fn settle_dragged(&self) {
        let mut state = self.state();
        if let Some(key) = state.drag_key.clone() {
            if let Some(i) = state.index(&key) {
                state.tiles[i].presence = Anim::at(1.0);
            }
        }
        drop(state);
        self.animate();
    }
}

fn action_for(intent: Option<&Intent>, offered: gdk::DragAction) -> gdk::DragAction {
    let prefer = |order: &[gdk::DragAction]| {
        order
            .iter()
            .copied()
            .find(|a| offered.contains(*a))
            .unwrap_or(gdk::DragAction::empty())
    };
    use gdk::DragAction as A;
    match intent {
        None => A::empty(),
        Some(Intent::MoveApp { .. } | Intent::MoveOther { .. }) => prefer(&[A::MOVE]),
        Some(Intent::Pin { .. } | Intent::AddOther { .. }) => prefer(&[A::LINK, A::COPY]),
        // The dock moves or trashes the files itself; the source must not
        // also delete them.
        Some(Intent::OpenWith(_) | Intent::IntoFolder(_) | Intent::Trash) => {
            prefer(&[A::COPY, A::LINK, A::MOVE])
        }
    }
}
