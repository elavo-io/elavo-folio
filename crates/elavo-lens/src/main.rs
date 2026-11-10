//! Lens: Super+Space search for applications, System Settings pages, and
//! quick calculations, with an Applications section that browses every
//! installed app.
//! Spec: elavo-architecture-and-planning-internal, planning/desktop/lens.md.

mod calc;
mod pages;

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::{Rc, Weak};

use elavo_common::{AppEntry, AppIndex};
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

const APP_ID: &str = "io.elavo.Lens";
const STYLE: &str = include_str!("style.css");
/// Sized so Lens, with the Applications view open, fits a 1366x768 screen
/// between the panel and the Dock.
const WIDTH: i32 = 560;
const MAX_APPLICATIONS: usize = 8;
const MAX_PAGES: usize = 4;
const GRID_ICON: i32 = 44;
const GRID_COLUMNS: u32 = 7;
const LIST_HEIGHT: i32 = 360;
const GRID_HEIGHT: i32 = 330;
/// Keeps a dragged Lens this far inside the screen edges.
const EDGE: f64 = 8.0;
/// The bar with its tallest results open (the Applications grid and its
/// pills), measured at 1366x768. Lens keeps this much on screen wherever it
/// is dragged, so the results never open past the bottom edge.
const PANEL_HEIGHT: f64 = 430.0;
/// Corner radii of the bar and the results card, matched by style.css.
const BAR_RADIUS: i32 = 22;
const RESULTS_RADIUS: i32 = 18;

/// The category pills above the Applications grid, in order, with the
/// freedesktop main categories each one covers. An application in several
/// appears under each. Pills with no applications are not shown.
const CATEGORIES: &[(&str, &[&str])] = &[
    ("Productivity", &["Office"]),
    ("Internet", &["Network"]),
    ("Media", &["AudioVideo", "Audio", "Video"]),
    ("Graphics", &["Graphics"]),
    ("Games", &["Game"]),
    ("Developer", &["Development"]),
    ("Education", &["Education", "Science"]),
    ("Utilities", &["Utility"]),
    ("System", &["System", "Settings"]),
];

/// Whether `app` belongs under the pill `label`.
fn in_category(app: &AppEntry, label: &str) -> bool {
    let Some((_, wanted)) = CATEGORIES.iter().find(|(l, _)| *l == label) else {
        return false;
    };
    let categories = app.info.categories();
    let categories = categories.as_deref().unwrap_or_default();
    categories
        .split(';')
        .any(|c| wanted.iter().any(|w| w.eq_ignore_ascii_case(c.trim())))
}

/// Where Lens sits when it has not been dragged: centered, a fifth of the
/// way down, but not under the panel.
fn default_origin(screen_w: f64, screen_h: f64) -> (f64, f64) {
    (
        (screen_w - f64::from(WIDTH)) / 2.0,
        (screen_h * 0.2).clamp(72.0, 200.0),
    )
}

/// Keep the panel on screen: `x`, `y` are the column's top-left corner and
/// `panel_h` how tall it can grow.
fn clamp_origin(x: f64, y: f64, screen_w: f64, screen_h: f64, panel_h: f64) -> (f64, f64) {
    let max_x = (screen_w - f64::from(WIDTH) - EDGE).max(EDGE);
    let max_y = (screen_h - panel_h - EDGE).max(EDGE);
    (x.clamp(EDGE, max_x), y.clamp(EDGE, max_y))
}

/// `$XDG_STATE_HOME/elavo/lens-position`: where the person last dragged
/// Lens, as the bar's center across and its top down, each a fraction of the
/// screen, so it lands in the same spot on any screen size. No file means
/// the default spot.
fn position_path() -> std::path::PathBuf {
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| glib::home_dir().join(".local/state"));
    state.join("elavo").join("lens-position")
}

fn parse_position(text: &str) -> Option<(f64, f64)> {
    let mut parts = text.split_whitespace().map(str::parse::<f64>);
    let (fx, fy) = (parts.next()?.ok()?, parts.next()?.ok()?);
    let ok = |v: f64| v.is_finite() && (0.0..=1.0).contains(&v);
    (ok(fx) && ok(fy)).then_some((fx, fy))
}

fn load_position() -> Option<(f64, f64)> {
    parse_position(&std::fs::read_to_string(position_path()).ok()?)
}

/// Save where Lens is, or forget it when `None`. Failing to save only means
/// Lens opens in the default spot next time.
fn save_position(position: Option<(f64, f64)>) {
    let path = position_path();
    match position {
        Some((fx, fy)) => {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(&path, format!("{fx:.4} {fy:.4}\n"));
        }
        None => {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// The column's top-left corner for a saved position on this screen.
fn origin_from_position((fx, fy): (f64, f64), screen_w: f64, screen_h: f64) -> (f64, f64) {
    (fx * screen_w - f64::from(WIDTH) / 2.0, fy * screen_h)
}

/// The saved position for the column's top-left corner on this screen.
fn position_from_origin((x, y): (f64, f64), screen_w: f64, screen_h: f64) -> (f64, f64) {
    (
        ((x + f64::from(WIDTH) / 2.0) / screen_w).clamp(0.0, 1.0),
        (y / screen_h).clamp(0.0, 1.0),
    )
}

/// `org.gnome.desktop.interface` `color-scheme`. Only `prefer-dark` is dark.
fn scheme_is_dark(value: &str) -> bool {
    value == "prefer-dark"
}

/// Load the theme's dark sheet while the color scheme is dark, and switch
/// when it changes, as the panel and the Dock do.
fn follow_color_scheme() {
    let Some(source) = gio::SettingsSchemaSource::default() else {
        return;
    };
    let Some(schema) = source.lookup("org.gnome.desktop.interface", true) else {
        return;
    };
    if !schema.has_key("color-scheme") {
        return;
    }
    let desktop = gio::Settings::new("org.gnome.desktop.interface");
    let apply = {
        let desktop = desktop.clone();
        move || {
            if let Some(settings) = gtk::Settings::default() {
                settings.set_gtk_application_prefer_dark_theme(scheme_is_dark(
                    desktop.string("color-scheme").as_str(),
                ));
            }
        }
    };
    apply();
    desktop.connect_changed(Some("color-scheme"), move |_, _| apply());
    thread_local! {
        static HELD: RefCell<Option<gio::Settings>> = const { RefCell::new(None) };
    }
    HELD.with(|held| *held.borrow_mut() = Some(desktop));
}

#[derive(Clone)]
enum Action {
    Launch(AppEntry),
    Copy(String),
    /// A System Settings page, by its `--pane` ID.
    OpenPage(String),
}

/// What System Settings' Lens pane chose in `elavo/lens.conf`: the
/// Applications folders (the default) or every installed app. The shared
/// folder holds the system's applications, so without it Lens lists every
/// installed app rather than only this account's own entries.
fn load_applications() -> AppIndex {
    let file = glib::KeyFile::new();
    let path = glib::user_config_dir().join("elavo/lens.conf");
    let all = file
        .load_from_file(&path, glib::KeyFileFlags::NONE)
        .ok()
        .and_then(|()| file.string("Lens", "Applications").ok())
        .is_some_and(|v| v.trim().eq_ignore_ascii_case("all"));
    if all || !elavo_common::apps::shared_applications_dir().is_dir() {
        AppIndex::load()
    } else {
        AppIndex::load_applications_folders()
    }
}

/// x, y, width, height, corner radius, in window coordinates.
type BlurPart = (i32, i32, i32, i32, i32);

struct Lens {
    window: gtk::ApplicationWindow,
    column: gtk::Box,
    bar: gtk::Box,
    blur: RefCell<Option<elavo_common::blur::BackgroundBlur>>,
    blur_parts: RefCell<Vec<BlurPart>>,
    entry: gtk::Entry,
    apps_button: gtk::ToggleButton,
    results: gtk::Box,
    pills: gtk::Box,
    pill_scroller: gtk::ScrolledWindow,
    /// The chosen category pill; `None` is All.
    category: RefCell<Option<&'static str>>,
    /// How far the column has been dragged from where it starts.
    offset: Cell<(f64, f64)>,
    drag_start: Cell<(f64, f64)>,
    /// A saved position still to be turned into an offset, once the
    /// screen size is known.
    saved_position: Cell<Option<(f64, f64)>>,
    list: gtk::ListBox,
    list_scroller: gtk::ScrolledWindow,
    grid: gtk::FlowBox,
    grid_scroller: gtk::ScrolledWindow,
    apps: RefCell<Vec<AppEntry>>,
    /// Action for each list row, by row index. Headings have none.
    actions: RefCell<Vec<Option<Action>>>,
    /// An application is being dragged out, to the Dock.
    dragging: Cell<bool>,
    /// System Settings pages, refreshed each time Lens opens.
    pages: RefCell<Vec<pages::Page>>,
    me: Weak<Lens>,
}

impl Lens {
    fn new(app: &gtk::Application) -> Rc<Self> {
        let display = gdk::Display::default().expect("no display");
        let css = gtk::CssProvider::new();
        css.load_from_string(STYLE);
        gtk::style_context_add_provider_for_display(
            &display,
            &css,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );

        let window = gtk::ApplicationWindow::builder()
            .application(app)
            .title("Lens")
            .decorated(false)
            .css_classes(["elavo-lens"])
            .build();
        window.init_layer_shell();
        window.set_namespace(Some("elavo-lens"));
        window.set_layer(Layer::Overlay);
        for edge in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
            window.set_anchor(edge, true);
        }
        window.set_keyboard_mode(KeyboardMode::Exclusive);

        follow_color_scheme();

        let entry = gtk::Entry::builder()
            .placeholder_text("Search")
            .hexpand(true)
            .build();
        let apps_button = gtk::ToggleButton::builder()
            .icon_name("view-app-grid-symbolic")
            .tooltip_text("Applications (Ctrl+1)")
            .valign(gtk::Align::Center)
            .focusable(false)
            .build();
        let bar = gtk::Box::builder()
            .css_classes(["elavo-lens-bar"])
            .spacing(10)
            .build();
        bar.append(
            &gtk::Image::builder()
                .icon_name("system-search-symbolic")
                .pixel_size(18)
                .build(),
        );
        bar.append(&entry);
        bar.append(&apps_button);

        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::Single)
            .activate_on_single_click(true)
            .build();
        let list_scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(LIST_HEIGHT)
            .child(&list)
            .build();

        let grid = gtk::FlowBox::builder()
            .css_classes(["elavo-lens-grid"])
            .homogeneous(true)
            .min_children_per_line(GRID_COLUMNS)
            .max_children_per_line(GRID_COLUMNS)
            .row_spacing(2)
            .column_spacing(2)
            .selection_mode(gtk::SelectionMode::Single)
            .activate_on_single_click(true)
            .valign(gtk::Align::Start)
            .build();
        let grid_scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .min_content_height(GRID_HEIGHT)
            .max_content_height(GRID_HEIGHT)
            .child(&grid)
            .build();

        let pills = gtk::Box::builder()
            .css_classes(["elavo-lens-pills"])
            .spacing(6)
            .build();
        let pill_scroller = gtk::ScrolledWindow::builder()
            .css_classes(["elavo-lens-pill-scroller"])
            .hscrollbar_policy(gtk::PolicyType::External)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .child(&pills)
            .build();

        let results = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .css_classes(["elavo-lens-results"])
            .visible(false)
            .build();
        results.append(&list_scroller);
        results.append(&pill_scroller);
        results.append(&grid_scroller);

        let column = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .css_classes(["elavo-lens-column"])
            .halign(gtk::Align::Start)
            .valign(gtk::Align::Start)
            .width_request(WIDTH)
            .build();
        column.append(&bar);
        column.append(&results);

        let root = gtk::Box::builder().hexpand(true).vexpand(true).build();
        root.append(&column);
        window.set_child(Some(&root));

        let lens = Rc::new_cyclic(|me| Self {
            me: me.clone(),
            window,
            column,
            bar,
            blur: RefCell::new(None),
            blur_parts: RefCell::default(),
            entry,
            apps_button,
            results,
            pills,
            pill_scroller,
            category: RefCell::new(None),
            offset: Cell::new((0.0, 0.0)),
            drag_start: Cell::new((0.0, 0.0)),
            saved_position: Cell::new(load_position()),
            list,
            list_scroller,
            grid,
            grid_scroller,
            apps: RefCell::default(),
            actions: RefCell::default(),
            dragging: Cell::new(false),
            pages: RefCell::default(),
        });
        lens.connect(&root);
        lens.connect_move(&root);
        lens.sync_blur();
        lens
    }

    /// Blur the bar, and the results card when it is open. The fullscreen
    /// layer around them stays clear. The panel does not use this.
    fn sync_blur(&self) {
        let mut parts = Vec::new();
        let bar = if self.dragging.get() {
            None
        } else {
            self.bar.compute_bounds(&self.window)
        };
        if let Some(bounds) = bar {
            parts.push((
                bounds.x().round() as i32,
                bounds.y().round() as i32,
                bounds.width().round().max(0.0) as i32,
                bounds.height().round().max(0.0) as i32,
                BAR_RADIUS,
            ));
        }
        if self.results.is_visible() && !self.dragging.get() {
            if let Some(bounds) = self.results.compute_bounds(&self.window) {
                parts.push((
                    bounds.x().round() as i32,
                    bounds.y().round() as i32,
                    bounds.width().round().max(0.0) as i32,
                    bounds.height().round().max(0.0) as i32,
                    RESULTS_RADIUS,
                ));
            }
        }
        if self.blur.borrow().is_none() {
            if let Some(blur) =
                elavo_common::blur::BackgroundBlur::for_widget(self.window.upcast_ref())
            {
                *self.blur.borrow_mut() = Some(blur);
            }
        }
        if let Some(blur) = self.blur.borrow().as_ref() {
            if *self.blur_parts.borrow() == parts {
                return;
            }
            blur.set_rounds(&parts);
            *self.blur_parts.borrow_mut() = parts;
            // The region applies on the next surface commit.
            self.window.queue_draw();
        }
    }

    fn connect(self: &Rc<Self>, root: &gtk::Box) {
        let weak = Rc::downgrade(self);
        self.window.connect_realize(move |_| {
            if let Some(lens) = weak.upgrade() {
                lens.sync_blur();
            }
        });
        // Bounds are only known after layout, which runs after the calls
        // above. Runs only while mapped; unchanged parts are not resent.
        let weak = Rc::downgrade(self);
        self.window
            .add_tick_callback(move |_, _| match weak.upgrade() {
                Some(lens) => {
                    lens.place();
                    lens.sync_blur();
                    glib::ControlFlow::Continue
                }
                None => glib::ControlFlow::Break,
            });
        let weak = Rc::downgrade(self);
        self.entry.connect_changed(move |_| {
            if let Some(s) = weak.upgrade() {
                s.refresh();
            }
        });

        let weak = Rc::downgrade(self);
        self.entry.connect_activate(move |_| {
            if let Some(s) = weak.upgrade() {
                s.activate_selection();
            }
        });

        let weak = Rc::downgrade(self);
        self.apps_button.connect_toggled(move |_| {
            if let Some(s) = weak.upgrade() {
                s.refresh();
                s.entry.grab_focus();
            }
        });

        let weak = Rc::downgrade(self);
        self.list.connect_row_activated(move |_, row| {
            if let Some(s) = weak.upgrade() {
                s.run_row(row);
            }
        });

        let weak = Rc::downgrade(self);
        self.grid.set_filter_func(move |child| {
            let Some(s) = weak.upgrade() else {
                return true;
            };
            let query = s.entry.text();
            let category = *s.category.borrow();
            s.grid_app(child).is_some_and(|app| {
                app.matches(&query) && category.is_none_or(|c| in_category(&app, c))
            })
        });

        let weak = Rc::downgrade(self);
        self.grid.connect_child_activated(move |_, child| {
            if let Some(s) = weak.upgrade() {
                if let Some(app) = s.grid_app(child) {
                    s.run(&Action::Launch(app));
                }
            }
        });

        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, modifiers| {
            let Some(s) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            s.on_key(key, modifiers)
        });
        self.window.add_controller(keys);

        let click = gtk::GestureClick::new();
        let weak = Rc::downgrade(self);
        let root_ref = root.downgrade();
        click.connect_pressed(move |_, _, x, y| {
            let (Some(s), Some(root)) = (weak.upgrade(), root_ref.upgrade()) else {
                return;
            };
            let inside = s
                .column
                .compute_bounds(&root)
                .is_some_and(|b| b.contains_point(&gtk::graphene::Point::new(x as f32, y as f32)));
            if !inside {
                s.hide();
            }
        });
        root.add_controller(click);
    }

    /// Drag the bar to move Lens. A press on the field or the button stays
    /// theirs. A double click on the bar puts Lens back where it starts.
    fn connect_move(self: &Rc<Self>, root: &gtk::Box) {
        let drag = gtk::GestureDrag::new();
        let weak = Rc::downgrade(self);
        drag.connect_drag_begin(move |gesture, x, y| {
            let Some(s) = weak.upgrade() else {
                return;
            };
            let on_control = s.bar.pick(x, y, gtk::PickFlags::DEFAULT).is_some_and(|w| {
                w.is_ancestor(&s.entry)
                    || w == *s.entry.upcast_ref::<gtk::Widget>()
                    || w.is_ancestor(&s.apps_button)
                    || w == *s.apps_button.upcast_ref::<gtk::Widget>()
            });
            if on_control {
                gesture.set_state(gtk::EventSequenceState::Denied);
                return;
            }
            s.drag_start.set(s.offset.get());
        });
        let weak = Rc::downgrade(self);
        drag.connect_drag_update(move |_, dx, dy| {
            if let Some(s) = weak.upgrade() {
                let (sx, sy) = s.drag_start.get();
                s.offset.set((sx + dx, sy + dy));
                s.place();
            }
        });
        let weak = Rc::downgrade(self);
        drag.connect_drag_end(move |_, _, _| {
            let Some(s) = weak.upgrade() else {
                return;
            };
            // A press on the field or button, or a click, moved nothing.
            if s.offset.get() == s.drag_start.get() {
                return;
            }
            s.remember_position();
        });
        self.bar.add_controller(drag);

        let reset = gtk::GestureClick::new();
        let weak = Rc::downgrade(self);
        reset.connect_pressed(move |_, presses, _, _| {
            if presses == 2 {
                if let Some(s) = weak.upgrade() {
                    s.offset.set((0.0, 0.0));
                    s.saved_position.set(None);
                    s.place();
                    save_position(None);
                }
            }
        });
        self.bar.add_controller(reset);
        let _ = root;
    }

    /// Position the column from its offset, keeping the bar on screen. The
    /// stored offset is clamped too, so dragging back from an edge moves at once.
    fn place(&self) {
        let Some(root) = self.window.child() else {
            return;
        };
        let (w, h) = (f64::from(root.width()), f64::from(root.height()));
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let (x0, y0) = default_origin(w, h);
        if let Some(saved) = self.saved_position.take() {
            let (x, y) = origin_from_position(saved, w, h);
            self.offset.set((x - x0, y - y0));
        }
        let (dx, dy) = self.offset.get();
        let panel_h = PANEL_HEIGHT.max(f64::from(self.column.height()));
        let (x, y) = clamp_origin(x0 + dx, y0 + dy, w, h, panel_h);
        self.offset.set((x - x0, y - y0));
        self.column.set_margin_start(x.round() as i32);
        self.column.set_margin_top(y.round() as i32);
    }

    /// Save where the column is now, so Lens opens there next time.
    fn remember_position(&self) {
        let Some(root) = self.window.child() else {
            return;
        };
        let (w, h) = (f64::from(root.width()), f64::from(root.height()));
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let (x0, y0) = default_origin(w, h);
        let (dx, dy) = self.offset.get();
        save_position(Some(position_from_origin((x0 + dx, y0 + dy), w, h)));
    }

    /// One pill for All and one for each category that has applications.
    fn populate_pills(self: &Rc<Self>) {
        while let Some(child) = self.pills.first_child() {
            self.pills.remove(&child);
        }
        let chosen = *self.category.borrow();
        let present: Vec<&'static str> = CATEGORIES
            .iter()
            .map(|(label, _)| *label)
            .filter(|label| self.apps.borrow().iter().any(|a| in_category(a, label)))
            .collect();
        let chosen = chosen.filter(|c| present.contains(c));
        self.category.replace(chosen);

        let mut group: Option<gtk::ToggleButton> = None;
        for label in std::iter::once(None).chain(present.into_iter().map(Some)) {
            let pill = gtk::ToggleButton::builder()
                .label(label.unwrap_or("All"))
                .focusable(false)
                .active(label == chosen)
                .build();
            if let Some(first) = group.as_ref() {
                pill.set_group(Some(first));
            } else {
                group = Some(pill.clone());
            }
            let weak = Rc::downgrade(self);
            pill.connect_toggled(move |pill| {
                if !pill.is_active() {
                    return;
                }
                if let Some(s) = weak.upgrade() {
                    s.category.replace(label);
                    s.grid.invalidate_filter();
                    s.grid.unselect_all();
                    s.grid_scroller.vadjustment().set_value(0.0);
                }
            });
            self.pills.append(&pill);
        }
    }

    fn on_key(&self, key: gdk::Key, modifiers: gdk::ModifierType) -> glib::Propagation {
        let apps_mode = self.apps_button.is_active();
        match key {
            gdk::Key::Escape => {
                if !self.entry.text().is_empty() {
                    self.entry.set_text("");
                } else if apps_mode {
                    self.apps_button.set_active(false);
                } else {
                    self.hide();
                }
                glib::Propagation::Stop
            }
            gdk::Key::_1 if modifiers.contains(gdk::ModifierType::CONTROL_MASK) => {
                self.apps_button.set_active(!apps_mode);
                glib::Propagation::Stop
            }
            gdk::Key::Down | gdk::Key::Up if !apps_mode => {
                self.move_selection(if key == gdk::Key::Down { 1 } else { -1 });
                glib::Propagation::Stop
            }
            gdk::Key::Down if apps_mode && self.entry.has_focus() => {
                if let Some(child) = self.first_visible_grid_child() {
                    self.grid.select_child(&child);
                    child.grab_focus();
                }
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        }
    }

    fn grid_app(&self, child: &gtk::FlowBoxChild) -> Option<AppEntry> {
        let id = child.child()?.widget_name();
        self.apps
            .borrow()
            .iter()
            .find(|a| a.id == id.as_str())
            .cloned()
    }

    fn first_visible_grid_child(&self) -> Option<gtk::FlowBoxChild> {
        let mut child = self.grid.first_child();
        while let Some(widget) = child {
            if let Some(fb) = widget.downcast_ref::<gtk::FlowBoxChild>() {
                if fb.is_child_visible() {
                    return Some(fb.clone());
                }
            }
            child = widget.next_sibling();
        }
        None
    }

    fn populate_grid(&self) {
        while let Some(child) = self.grid.first_child() {
            self.grid.remove(&child);
        }
        for app in self.apps.borrow().iter() {
            let tile = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(4)
                .name(app.id.as_str())
                .tooltip_text(app.name.as_str())
                .build();
            tile.append(&app_image(app, GRID_ICON));
            tile.add_controller(self.drag_source(app));
            tile.append(
                &gtk::Label::builder()
                    .label(app.name.as_str())
                    .ellipsize(gtk::pango::EllipsizeMode::End)
                    .max_width_chars(9)
                    .justify(gtk::Justification::Center)
                    .build(),
            );
            self.grid.append(&tile);
        }
    }

    fn heading(&self, text: &str) {
        let row = gtk::ListBoxRow::builder()
            .selectable(false)
            .activatable(false)
            .css_classes(["elavo-lens-heading"])
            .child(&gtk::Label::builder().label(text).xalign(0.0).build())
            .build();
        self.list.append(&row);
        self.actions.borrow_mut().push(None);
    }

    fn row(&self, icon: gtk::Image, title: &gtk::Label, subtitle: Option<&str>, action: Action) {
        let text = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .valign(gtk::Align::Center)
            .build();
        text.append(title);
        if let Some(subtitle) = subtitle {
            text.append(
                &gtk::Label::builder()
                    .label(subtitle)
                    .xalign(0.0)
                    .ellipsize(gtk::pango::EllipsizeMode::End)
                    .css_classes(["elavo-lens-subtitle"])
                    .build(),
            );
        }
        let line = gtk::Box::builder().spacing(12).build();
        line.append(&icon);
        line.append(&text);
        self.list
            .append(&gtk::ListBoxRow::builder().child(&line).build());
        self.actions.borrow_mut().push(Some(action));
    }

    fn app_row(&self, app: &AppEntry) {
        let title = gtk::Label::builder()
            .label(app.name.as_str())
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["elavo-lens-title"])
            .build();
        self.row(
            app_image(app, 28),
            &title,
            app.generic_name.as_deref(),
            Action::Launch(app.clone()),
        );
        if let Some(row) = self.list.last_child() {
            row.add_controller(self.drag_source(app));
        }
    }

    fn page_row(&self, page: &pages::Page) {
        let title = gtk::Label::builder()
            .label(page.title.as_str())
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["elavo-lens-title"])
            .build();
        let icon = gtk::Image::builder()
            .icon_name(page.icon.as_str())
            .pixel_size(16)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .css_classes(["elavo-lens-page-icon"])
            .build();
        self.row(
            icon,
            &title,
            Some("System Settings"),
            Action::OpenPage(page.id.clone()),
        );
    }

    /// Dragging an application out of Lens offers its Applications entry,
    /// or its desktop file, as a link; the Dock pins it.
    fn drag_source(&self, app: &AppEntry) -> gtk::DragSource {
        let source = gtk::DragSource::builder()
            .actions(gdk::DragAction::LINK | gdk::DragAction::COPY)
            .build();
        let file = app.info.filename().map(|path| {
            match path
                .parent()
                .filter(|p| p.extension().is_some_and(|e| e == "eapp"))
            {
                Some(bundle) => gio::File::for_path(bundle),
                None => gio::File::for_path(&path),
            }
        });
        let Some(file) = file else {
            return source;
        };
        let list = gdk::FileList::from_array(&[file]);
        source.set_content(Some(&gdk::ContentProvider::for_value(&list.to_value())));
        let icon = app.icon();
        let lens = self.me.clone();
        source.connect_drag_begin(move |source, _| {
            let Some(lens) = lens.upgrade() else {
                return;
            };
            // The tile is about to be hidden, so the drag icon is drawn from
            // the theme rather than from the widget.
            if let Some(icon) = icon.as_ref() {
                let theme = gtk::IconTheme::for_display(&WidgetExt::display(&lens.window));
                let paintable = theme.lookup_by_gicon(
                    icon,
                    GRID_ICON,
                    lens.window.scale_factor(),
                    gtk::TextDirection::None,
                    gtk::IconLookupFlags::empty(),
                );
                source.set_icon(Some(&paintable), GRID_ICON / 2, GRID_ICON / 2);
            }
            lens.step_aside();
        });
        let lens = self.me.clone();
        source.connect_drag_end(move |_, _, _| {
            if let Some(lens) = lens.upgrade() {
                lens.hide();
            }
        });
        source
    }

    /// Lens covers the screen above the Dock. While an application is
    /// dragged out of it, Lens draws nothing, takes no input, and sits below
    /// the Dock's layer, so the drop reaches the Dock.
    fn step_aside(&self) {
        self.dragging.set(true);
        self.column.set_opacity(0.0);
        self.sync_blur();
        if let Some(surface) = self.window.surface() {
            surface.set_input_region(Some(&gtk::cairo::Region::create()));
        }
        self.window.set_keyboard_mode(KeyboardMode::None);
        self.window.set_layer(Layer::Bottom);
    }

    /// Undoes [`Lens::step_aside`] while Lens is hidden.
    fn come_back(&self) {
        if !self.dragging.replace(false) {
            return;
        }
        self.column.set_opacity(1.0);
        if let Some(surface) = self.window.surface() {
            surface.set_input_region(None);
        }
        self.window.set_layer(Layer::Overlay);
        self.window.set_keyboard_mode(KeyboardMode::Exclusive);
    }

    fn refresh(&self) {
        let apps_mode = self.apps_button.is_active();
        self.list_scroller.set_visible(!apps_mode);
        self.pill_scroller.set_visible(apps_mode);
        self.grid_scroller.set_visible(apps_mode);
        if apps_mode {
            self.grid.invalidate_filter();
            self.grid.unselect_all();
            self.results.set_visible(true);
            self.sync_blur();
            return;
        }

        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        self.actions.borrow_mut().clear();

        let query = self.entry.text();
        if query.trim().is_empty() {
            self.results.set_visible(false);
            self.sync_blur();
            return;
        }

        if let Some(answer) = calc::evaluate(&query) {
            self.heading("Calculator");
            let title = gtk::Label::builder()
                .label(format!("= {answer}"))
                .xalign(0.0)
                .selectable(false)
                .css_classes(["elavo-lens-calc"])
                .build();
            let icon = gtk::Image::builder()
                .icon_name("accessories-calculator-symbolic")
                .pixel_size(28)
                .build();
            self.row(
                icon,
                &title,
                Some("Press Return to copy"),
                Action::Copy(answer),
            );
        }

        let mut ranked: Vec<(u32, AppEntry)> = {
            let apps = self.apps.borrow();
            let mut scored: Vec<(u32, &AppEntry)> = apps
                .iter()
                .filter_map(|a| a.score(&query).map(|s| (s, a)))
                .collect();
            scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.name.cmp(&b.1.name)));
            scored
                .into_iter()
                .take(MAX_APPLICATIONS + 1)
                .map(|(s, a)| (s, a.clone()))
                .collect()
        };
        let mut found: Vec<(u32, pages::Page)> = {
            let pages = self.pages.borrow();
            let mut scored: Vec<(u32, &pages::Page)> = pages
                .iter()
                .filter_map(|p| p.score(&query).map(|s| (s, p)))
                .collect();
            scored.sort_by_key(|s| std::cmp::Reverse(s.0));
            scored
                .into_iter()
                .take(MAX_PAGES + 1)
                .map(|(s, p)| (s, p.clone()))
                .collect()
        };
        // The best match of either kind is the Top Hit. A page wins a tie, so
        // "wifi" opens Wi-Fi rather than System Settings, whose search
        // words include every page's.
        let page_on_top = match (ranked.first(), found.first()) {
            (Some((app, _)), Some((page, _))) => page >= app,
            (None, Some(_)) => true,
            _ => false,
        };
        if page_on_top {
            let (_, page) = found.remove(0);
            self.heading("Top Hit");
            self.page_row(&page);
            ranked.truncate(MAX_APPLICATIONS);
        } else if !ranked.is_empty() {
            let (_, top) = ranked.remove(0);
            self.heading("Top Hit");
            self.app_row(&top);
            found.truncate(MAX_PAGES);
        }
        if !ranked.is_empty() {
            self.heading("Applications");
            for (_, app) in &ranked {
                self.app_row(app);
            }
        }
        if !found.is_empty() {
            self.heading("Settings");
            for (_, page) in &found {
                self.page_row(page);
            }
        }

        if self.actions.borrow().is_empty() {
            self.heading("No Results");
        }
        self.results.set_visible(true);
        self.move_selection(0);
        self.sync_blur();
    }

    /// Select the next selectable row in `step` direction; 0 selects the first.
    fn move_selection(&self, step: i32) {
        let actions = self.actions.borrow();
        let count = actions.len() as i32;
        let current = self.list.selected_row().map(|r| r.index());
        let mut index = match (current, step) {
            (Some(i), s) if s != 0 => i + s,
            _ => 0,
        };
        let direction = if step < 0 { -1 } else { 1 };
        while (0..count).contains(&index) {
            if actions[index as usize].is_some() {
                if let Some(row) = self.list.row_at_index(index) {
                    self.list.select_row(Some(&row));
                }
                return;
            }
            index += direction;
        }
    }

    fn activate_selection(&self) {
        if self.apps_button.is_active() {
            let child = self
                .grid
                .selected_children()
                .into_iter()
                .next()
                .or_else(|| self.first_visible_grid_child());
            if let Some(app) = child.and_then(|c| self.grid_app(&c)) {
                self.run(&Action::Launch(app));
            }
        } else if let Some(row) = self.list.selected_row() {
            self.run_row(&row);
        }
    }

    fn run_row(&self, row: &gtk::ListBoxRow) {
        let action = self
            .actions
            .borrow()
            .get(row.index() as usize)
            .cloned()
            .flatten();
        if let Some(action) = action {
            self.run(&action);
        }
    }

    fn run(&self, action: &Action) {
        match action {
            Action::Launch(app) => elavo_common::launch(&app.info, None),
            Action::Copy(text) => WidgetExt::display(&self.window).clipboard().set_text(text),
            Action::OpenPage(id) => pages::open(id),
        }
        self.hide();
    }

    fn show(self: &Rc<Self>, applications: bool) {
        self.apps
            .replace(load_applications().shown().cloned().collect());
        self.populate_grid();
        self.populate_pills();
        self.load_pages();
        self.entry.set_text("");
        if self.apps_button.is_active() == applications {
            self.refresh();
        } else {
            self.apps_button.set_active(applications);
        }
        self.window.present();
        self.entry.grab_focus();
    }

    /// Ask System Settings for its pages again, in case a page came or went
    /// (Touchscreen controls follows the touchscreen). Until the answer
    /// arrives, the last list is searched.
    fn load_pages(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        pages::load(move |pages| {
            let Some(s) = weak.upgrade() else {
                return;
            };
            if pages.is_empty() && !s.pages.borrow().is_empty() {
                return;
            }
            s.pages.replace(pages);
            if s.window.is_visible()
                && !s.apps_button.is_active()
                && !s.entry.text().trim().is_empty()
            {
                s.refresh();
            }
        });
    }

    fn hide(&self) {
        self.window.set_visible(false);
        self.come_back();
    }

    fn toggle(self: &Rc<Self>, applications: bool) {
        if !self.window.is_visible() {
            self.show(applications);
        } else if applications && !self.apps_button.is_active() {
            self.apps_button.set_active(true);
        } else {
            self.hide();
        }
    }
}

fn app_image(app: &AppEntry, size: i32) -> gtk::Image {
    let image = gtk::Image::builder().pixel_size(size).build();
    match app.icon() {
        Some(icon) => image.set_from_gicon(&icon),
        None => image.set_icon_name(Some("application-x-executable")),
    }
    image
}

fn main() -> glib::ExitCode {
    let app = gtk::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();

    let lens: Rc<OnceCell<Rc<Lens>>> = Rc::default();
    app.connect_command_line(move |app, cmdline| {
        let lens = lens.get_or_init(|| {
            std::mem::forget(app.hold());
            Lens::new(app)
        });
        let args = cmdline.arguments();
        let has = |flag: &str| args.iter().any(|a| a == flag);
        if has("--toggle") {
            lens.toggle(false);
        } else if has("--applications") {
            if lens.window.is_visible() {
                lens.hide();
            } else {
                lens.show(true);
            }
        }
        glib::ExitCode::SUCCESS
    });
    app.run()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_prefer_dark_is_dark() {
        assert!(scheme_is_dark("prefer-dark"));
        assert!(!scheme_is_dark("prefer-light"));
        assert!(!scheme_is_dark("default"));
        assert!(!scheme_is_dark(""));
    }

    #[test]
    fn starts_centered_below_the_panel_on_a_small_screen() {
        let (x, y) = default_origin(1366.0, 768.0);
        assert_eq!(x, (1366.0 - f64::from(WIDTH)) / 2.0);
        assert!((150.0..=160.0).contains(&y));
        assert_eq!(default_origin(1366.0, 300.0).1, 72.0);
        assert_eq!(default_origin(2560.0, 1600.0).1, 200.0);
    }

    #[test]
    fn a_drag_keeps_the_panel_on_screen() {
        let bar = PANEL_HEIGHT;
        assert_eq!(
            clamp_origin(-500.0, -500.0, 1366.0, 768.0, bar),
            (EDGE, EDGE)
        );
        let (x, y) = clamp_origin(5000.0, 5000.0, 1366.0, 768.0, bar);
        assert_eq!(x, 1366.0 - f64::from(WIDTH) - EDGE);
        assert_eq!(y, 768.0 - bar - EDGE);
    }

    #[test]
    fn a_saved_position_lands_in_the_same_spot_on_any_screen() {
        let origin = (300.0, 120.0);
        let saved = position_from_origin(origin, 1366.0, 768.0);
        let back = origin_from_position(saved, 1366.0, 768.0);
        assert!((back.0 - origin.0).abs() < 1e-9 && (back.1 - origin.1).abs() < 1e-9);
        let big = origin_from_position(saved, 2732.0, 1536.0);
        assert!(
            (big.0 + f64::from(WIDTH) / 2.0 - 2.0 * (origin.0 + f64::from(WIDTH) / 2.0)).abs()
                < 1e-9
        );
        assert!((big.1 - 240.0).abs() < 1e-9);
    }

    #[test]
    fn only_a_sound_position_file_is_used() {
        assert_eq!(parse_position("0.5000 0.2000\n"), Some((0.5, 0.2)));
        assert_eq!(parse_position("0.5"), None);
        assert_eq!(parse_position("1.5 0.2"), None);
        assert_eq!(parse_position("NaN 0.2"), None);
        assert_eq!(parse_position(""), None);
    }

    #[test]
    fn every_pill_names_a_category() {
        for (label, categories) in CATEGORIES {
            assert!(!label.is_empty());
            assert!(!categories.is_empty());
        }
    }
}
