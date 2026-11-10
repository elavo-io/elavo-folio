//! Lens: Super+Space search for applications, System Settings pages, and
//! quick calculations, with an Applications section that browses every
//! installed app.
//! Spec: elavo-architecture-and-planning-internal, planning/desktop/lens.md.

mod calc;
mod file_search;
mod menu;
mod pages;
mod units;
mod usage;

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
const MAX_FILES: usize = 4;
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

/// The App Launcher opens Lens itself, so Lens does not list it.
const LAUNCHER: &str = elavo_common::pins::LAUNCHER;
/// Where an application installed from App Bay is updated or removed.
const APP_BAY: &str = "io.elavo.AppBay.desktop";
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
    state_dir().join("lens-position")
}

/// `$XDG_STATE_HOME/elavo`, where Lens keeps what it remembers.
fn state_dir() -> std::path::PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| glib::home_dir().join(".local/state"))
        .join("elavo")
}

/// `$XDG_STATE_HOME/elavo/lens-usage`: which applications were opened from
/// Lens, how often, and when. See [`usage`].
fn usage_path() -> std::path::PathBuf {
    state_dir().join("lens-usage")
}

/// Now, in Unix seconds.
fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
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
    /// A file or folder, in its default application.
    OpenFile(std::path::PathBuf),
    /// One of the application's desktop actions, such as New Window.
    LaunchWith(AppEntry, String),
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
    /// A drag that started on the bar is under way.
    moving: Cell<bool>,
    /// The dragged bar and results have turned opaque and the blur is off.
    lifted: Cell<bool>,
    /// A saved position still to be turned into an offset, once the
    /// screen size is known.
    saved_position: Cell<Option<(f64, f64)>>,
    list: gtk::ListBox,
    list_scroller: gtk::ScrolledWindow,
    grid: gtk::FlowBox,
    grid_scroller: gtk::ScrolledWindow,
    /// Launch history, read each time Lens opens.
    usage: RefCell<usage::Usage>,
    apps: RefCell<Vec<AppEntry>>,
    /// Action for each list row, by row index. Headings have none.
    actions: RefCell<Vec<Option<Action>>>,
    /// An application is being dragged out, to the Dock.
    dragging: Cell<bool>,
    /// System Settings pages, refreshed each time Lens opens.
    pages: RefCell<Vec<pages::Page>>,
    /// The menu on an application, while it is open.
    menu: RefCell<Option<gtk::Popover>>,
    /// Recently used files, read when files are first searched after Lens
    /// opens.
    recent_files: RefCell<Vec<file_search::FileEntry>>,
    /// What the file scan found since Lens opened. Empty until it is done.
    scanned_files: RefCell<Rc<Vec<file_search::FileEntry>>>,
    /// The file scan has started since Lens opened.
    scan_started: Cell<bool>,
    /// Counts the times Lens opened, so a scan from an earlier time is
    /// thrown away.
    scan_serial: Cell<u32>,
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
            moving: Cell::new(false),
            lifted: Cell::new(false),
            saved_position: Cell::new(load_position()),
            list,
            list_scroller,
            grid,
            grid_scroller,
            usage: RefCell::default(),
            apps: RefCell::default(),
            actions: RefCell::default(),
            dragging: Cell::new(false),
            pages: RefCell::default(),
            menu: RefCell::new(None),
            recent_files: RefCell::default(),
            scanned_files: RefCell::default(),
            scan_started: Cell::new(false),
            scan_serial: Cell::new(0),
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
        let bar = if self.dragging.get() || self.lifted.get() {
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
        if self.results.is_visible() && !self.dragging.get() && !self.lifted.get() {
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
            // A click beside an open menu only closes the menu.
            if s.menu.borrow().is_some() {
                return;
            }
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
    ///
    /// The drag is read on the fullscreen root, which never moves. Read on
    /// the bar itself, each step would move the bar under the pointer and
    /// change the next step's offset, and Lens would shake.
    fn connect_move(self: &Rc<Self>, root: &gtk::Box) {
        let drag = gtk::GestureDrag::new();
        let weak = Rc::downgrade(self);
        let root_ref = root.downgrade();
        drag.connect_drag_begin(move |gesture, x, y| {
            let (Some(s), Some(root)) = (weak.upgrade(), root_ref.upgrade()) else {
                return;
            };
            let point = gtk::graphene::Point::new(x as f32, y as f32);
            let on_bar = s
                .bar
                .compute_bounds(&root)
                .is_some_and(|b| b.contains_point(&point));
            let on_control = root.pick(x, y, gtk::PickFlags::DEFAULT).is_some_and(|w| {
                w.is_ancestor(&s.entry)
                    || w == *s.entry.upcast_ref::<gtk::Widget>()
                    || w.is_ancestor(&s.apps_button)
                    || w == *s.apps_button.upcast_ref::<gtk::Widget>()
            });
            if !on_bar || on_control {
                gesture.set_state(gtk::EventSequenceState::Denied);
                return;
            }
            s.moving.set(true);
            s.drag_start.set(s.offset.get());
            s.lift();
        });
        let weak = Rc::downgrade(self);
        drag.connect_drag_update(move |_, dx, dy| {
            if let Some(s) = weak.upgrade().filter(|s| s.moving.get()) {
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
            if !s.moving.replace(false) {
                return;
            }
            s.drop_down();
            if s.offset.get() != s.drag_start.get() {
                s.remember_position();
            }
        });
        root.add_controller(drag);

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
        // An open menu takes the arrows, Return, and Escape, which closes
        // only the menu.
        if self.menu.borrow().is_some() {
            return glib::Propagation::Proceed;
        }
        let apps_mode = self.apps_button.is_active();
        match key {
            gdk::Key::Menu => {
                self.open_menu_on_selection();
                glib::Propagation::Stop
            }
            gdk::Key::F10 if modifiers.contains(gdk::ModifierType::SHIFT_MASK) => {
                self.open_menu_on_selection();
                glib::Propagation::Stop
            }
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
            // The field's text, not the field, holds the focus.
            gdk::Key::Down
                if apps_mode
                    && self
                        .entry
                        .state_flags()
                        .contains(gtk::StateFlags::FOCUS_WITHIN) =>
            {
                if let Some(child) = self.first_visible_grid_child() {
                    self.grid.select_child(&child);
                    child.grab_focus();
                }
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        }
    }

    /// The application on a tile of the grid.
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
            self.add_menu_gestures(&tile, app);
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

    /// The installed application opened most often from Lens.
    fn most_launched(&self) -> Option<AppEntry> {
        let apps = self.apps.borrow();
        let usage = self.usage.borrow();
        let id = usage.most_launched(|id| apps.iter().any(|a| a.id == id))?;
        apps.iter().find(|a| a.id == id).cloned()
    }

    /// Remember that `app` was opened from Lens. Failing to save only means
    /// this launch is not remembered.
    fn record_launch(&self, app: &AppEntry) {
        let mut usage = self.usage.borrow_mut();
        usage.record(&app.id, unix_now());
        let _ = usage.save(&usage_path());
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
            self.add_menu_gestures(&row, app);
        }
    }

    fn file_row(&self, file: &file_search::FileEntry) {
        let title = gtk::Label::builder()
            .label(file.name.as_str())
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::Middle)
            .css_classes(["elavo-lens-title"])
            .build();
        let icon = gtk::Image::builder()
            .pixel_size(16)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .css_classes(["elavo-lens-page-icon", "elavo-lens-file-icon"])
            .build();
        if file.is_dir {
            icon.set_icon_name(Some("folder-symbolic"));
        } else {
            let (kind, _) = gio::content_type_guess(Some(&file.path), None::<&[u8]>);
            icon.set_from_gicon(&gio::content_type_get_symbolic_icon(&kind));
        }
        let folder = file_search::folder_label(&file.path, &glib::home_dir());
        self.row(
            icon,
            &title,
            Some(&folder),
            Action::OpenFile(file.path.clone()),
        );
        if let Some(row) = self.list.last_child() {
            let path = file.path.clone();
            self.add_menu_opener(&row, move |lens, widget, at| {
                lens.open_file_menu(&path, widget, at);
            });
        }
    }

    /// Start this opening's file scan, unless it has started. Results
    /// arrive later and refresh the list if the search still wants files.
    fn search_files(&self) {
        if self.scan_started.replace(true) {
            return;
        }
        self.recent_files.replace(recent_files());
        let serial = self.scan_serial.get();
        let home = glib::home_dir();
        let places = file_places(&home);
        let scan = gio::spawn_blocking(move || file_search::scan(&home, &places));
        let lens = self.me.clone();
        glib::spawn_future_local(async move {
            let Ok(found) = scan.await else {
                return;
            };
            let Some(lens) = lens.upgrade() else {
                return;
            };
            if lens.scan_serial.get() != serial {
                return;
            }
            lens.scanned_files.replace(Rc::new(found));
            let wants_files = lens.entry.text().trim().chars().count() >= file_search::MIN_QUERY;
            if lens.window.is_visible()
                && !lens.apps_button.is_active()
                && wants_files
                && lens.menu.borrow().is_none()
            {
                lens.refresh();
            }
        });
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

    /// The compositor's blur trails a moving surface, so from the press that
    /// starts a drag, the blur is off and the bar and results are opaque.
    fn lift(&self) {
        self.lifted.set(true);
        self.sync_blur();
        self.column.add_css_class("elavo-lens-moving");
    }

    /// Undoes [`Lens::lift`] where the bar was dropped: the blur comes back
    /// under the still opaque bar and results, which then fade back to
    /// see-through.
    fn drop_down(&self) {
        self.lifted.set(false);
        self.sync_blur();
        self.column.remove_css_class("elavo-lens-moving");
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
            // Before anything is typed, the application opened most often
            // is the Top Hit, so Return opens it.
            let top = self.most_launched();
            if let Some(app) = &top {
                self.heading("Top Hit");
                self.app_row(app);
                self.move_selection(0);
            }
            self.results.set_visible(top.is_some());
            self.sync_blur();
            return;
        }

        let conversion = units::convert(&query);
        let answer = conversion
            .is_none()
            .then(|| calc::evaluate(&query))
            .flatten();
        if let Some(conversion) = &conversion {
            self.heading("Conversion");
            let title = gtk::Label::builder()
                .label(conversion.result.as_str())
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
                Some(&format!("{} · Press Return to copy", conversion.expression)),
                Action::Copy(conversion.result.clone()),
            );
        }
        if let Some(answer) = answer.clone() {
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
        let mut files = if query.trim().chars().count() >= file_search::MIN_QUERY {
            self.search_files();
            file_search::rank(
                &query,
                &self.recent_files.borrow(),
                &self.scanned_files.borrow(),
                MAX_FILES + 1,
            )
        } else {
            Vec::new()
        };
        // A file is the Top Hit only when nothing else answers.
        let file_on_top =
            ranked.is_empty() && found.is_empty() && conversion.is_none() && answer.is_none();
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
        } else if file_on_top && !files.is_empty() {
            let top = files.remove(0);
            self.heading("Top Hit");
            self.file_row(&top);
        }
        files.truncate(MAX_FILES);
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
        if !files.is_empty() {
            self.heading("Files");
            for file in &files {
                self.file_row(file);
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
            Action::Launch(app) => {
                elavo_common::launch(&app.info, None);
                self.record_launch(app);
            }
            Action::Copy(text) => WidgetExt::display(&self.window).clipboard().set_text(text),
            Action::OpenPage(id) => pages::open(id),
            Action::OpenFile(path) => {
                let uri = gio::File::for_path(path).uri();
                elavo_common::files::open_uri(&uri);
                // Opened from Lens, it counts as recently used.
                gtk::RecentManager::default().add_item(&uri);
            }
            Action::LaunchWith(app, action) => {
                elavo_common::launch(&app.info, Some(action));
                self.record_launch(app);
            }
        }
        self.hide();
    }

    /// A right click or a long press on an application opens its menu. The
    /// press is claimed, so it neither launches the application nor starts
    /// a drag to the Dock.
    fn add_menu_gestures(&self, widget: &impl IsA<gtk::Widget>, app: &AppEntry) {
        let id = app.id.clone();
        self.add_menu_opener(widget, move |lens, widget, at| {
            lens.open_menu(&id, widget, at);
        });
    }

    /// Call `open` with the widget and the point pressed on a right click
    /// or a long press on `widget`.
    fn add_menu_opener(
        &self,
        widget: &impl IsA<gtk::Widget>,
        open: impl Fn(&Lens, &gtk::Widget, Option<(f64, f64)>) + 'static,
    ) {
        let open = Rc::new(open);
        let click = gtk::GestureClick::builder()
            .button(gdk::BUTTON_SECONDARY)
            .build();
        let lens = self.me.clone();
        let on_click = open.clone();
        click.connect_pressed(move |gesture, _, x, y| {
            gesture.set_state(gtk::EventSequenceState::Claimed);
            if let (Some(lens), Some(widget)) = (lens.upgrade(), gesture.widget()) {
                on_click(&lens, &widget, Some((x, y)));
            }
        });
        widget.add_controller(click);

        let press = gtk::GestureLongPress::builder().touch_only(false).build();
        let lens = self.me.clone();
        press.connect_pressed(move |gesture, x, y| {
            gesture.set_state(gtk::EventSequenceState::Claimed);
            if let (Some(lens), Some(widget)) = (lens.upgrade(), gesture.widget()) {
                open(&lens, &widget, Some((x, y)));
            }
        });
        widget.add_controller(press);
    }

    /// The menu on a file or folder: Open, and Show in Files.
    fn open_file_menu(&self, path: &std::path::Path, anchor: &gtk::Widget, at: Option<(f64, f64)>) {
        self.close_menu();
        self.select(anchor);
        let (popover, items) = new_menu();
        let open = path.to_path_buf();
        self.menu_item(&items, &popover, "Open", move |lens| {
            lens.run(&Action::OpenFile(open.clone()));
        });
        let uri = gio::File::for_path(path).uri().to_string();
        self.menu_item(&items, &popover, "Show in Files", move |lens| {
            elavo_common::files::show_in_files(&uri);
            lens.hide();
        });
        self.present_menu(&popover, &items, anchor, at);
    }

    /// The Menu key or Shift+F10 opens the menu on the chosen tile or row.
    fn open_menu_on_selection(&self) {
        if self.apps_button.is_active() {
            let focused = GtkWindowExt::focus(&self.window).and_then(|w| tile_of(&w));
            let child = focused.or_else(|| self.grid.selected_children().into_iter().next());
            if let Some(child) = child {
                if let Some(app) = self.grid_app(&child) {
                    self.open_menu(&app.id, child.upcast_ref(), None);
                }
            }
        } else if let Some(row) = self.list.selected_row() {
            let action = self
                .actions
                .borrow()
                .get(row.index() as usize)
                .cloned()
                .flatten();
            match action {
                Some(Action::Launch(app)) => self.open_menu(&app.id, row.upcast_ref(), None),
                Some(Action::OpenFile(path)) => self.open_file_menu(&path, row.upcast_ref(), None),
                _ => {}
            }
        }
    }

    /// Open the menu on the application `id`, beside `anchor`, at `at` in
    /// its coordinates when the pointer opened it.
    fn open_menu(&self, id: &str, anchor: &gtk::Widget, at: Option<(f64, f64)>) {
        let Some(app) = self.apps.borrow().iter().find(|a| a.id == id).cloned() else {
            return;
        };
        self.close_menu();
        self.select(anchor);

        let (popover, items) = new_menu();

        let ids: Vec<String> = app
            .info
            .list_actions()
            .iter()
            .map(|a| a.to_string())
            .collect();
        let actions = menu::desktop_actions(&ids);
        if let Some(action) = &actions.new_window {
            let (app, action) = (app.clone(), action.clone());
            self.menu_item(&items, &popover, "New Window", move |lens| {
                lens.run(&Action::LaunchWith(app.clone(), action.clone()));
            });
        }
        for action in &actions.others {
            let label = app.info.action_name(action);
            let (app, action) = (app.clone(), action.clone());
            self.menu_item(&items, &popover, &label, move |lens| {
                lens.run(&Action::LaunchWith(app.clone(), action.clone()));
            });
        }
        let launches = items.first_child().is_some();

        // Files is always in the Dock, and the App Launcher is not listed.
        let pinnable = app.id != elavo_common::pins::FILES && app.id != LAUNCHER;
        let app_bay = app_bay_manages(&app).then(|| self.app_bay()).flatten();
        if launches && (pinnable || app_bay.is_some()) {
            items.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        }
        if pinnable {
            let pinned = elavo_common::pins::load_saved().contains(&app.id);
            let id = app.id.clone();
            // Lens stays open, so the person sees the Dock take the change.
            self.menu_item(&items, &popover, menu::pin_label(pinned), move |_| {
                let mut pins = elavo_common::pins::load_saved();
                if elavo_common::pins::set_pin(&mut pins, &id, !pinned) {
                    elavo_common::pins::save(&pins);
                }
            });
        }
        if let Some(info) = app_bay {
            let target = app_bay_id(&app);
            self.menu_item(&items, &popover, "Show in App Bay", move |lens| {
                open_app_bay(&info, target.as_deref());
                lens.hide();
            });
        }
        self.present_menu(&popover, &items, anchor, at);
    }

    /// Show a menu made with [`new_menu`] beside `anchor`, at `at` when the
    /// pointer opened it, with its first item focused. An empty menu is not
    /// shown.
    fn present_menu(
        &self,
        popover: &gtk::Popover,
        items: &gtk::Box,
        anchor: &gtk::Widget,
        at: Option<(f64, f64)>,
    ) {
        let Some(first) = items.first_child() else {
            return;
        };

        popover.set_child(Some(items));
        popover.set_parent(anchor);
        if let Some((x, y)) = at {
            popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        }
        let lens = self.me.clone();
        popover.connect_closed(move |popover| {
            let Some(lens) = lens.upgrade() else {
                return;
            };
            // Unparented once the click that closed it is over, so the
            // outside click sees the menu was open.
            let popover = popover.clone();
            glib::idle_add_local_once(move || {
                popover.unparent();
                let mut menu = lens.menu.borrow_mut();
                if menu.as_ref() == Some(&popover) {
                    *menu = None;
                }
            });
        });
        self.menu.replace(Some(popover.clone()));
        popover.popup();
        first.grab_focus();
    }

    /// One menu item. Choosing it closes the menu, then runs `choose`.
    fn menu_item(
        &self,
        items: &gtk::Box,
        popover: &gtk::Popover,
        label: &str,
        choose: impl Fn(&Lens) + 'static,
    ) {
        let button = gtk::Button::builder()
            .child(&gtk::Label::builder().label(label).xalign(0.0).build())
            .build();
        let lens = self.me.clone();
        let popover = popover.downgrade();
        button.connect_clicked(move |_| {
            if let Some(popover) = popover.upgrade() {
                popover.popdown();
            }
            if let Some(lens) = lens.upgrade() {
                choose(&lens);
            }
        });
        items.append(&button);
    }

    fn close_menu(&self) {
        let menu = self.menu.borrow().clone();
        if let Some(menu) = menu {
            menu.popdown();
        }
    }

    /// Select the tile or row a menu opens on, so it is clear which
    /// application the menu is for.
    fn select(&self, widget: &gtk::Widget) {
        if let Some(child) = tile_of(widget) {
            self.grid.select_child(&child);
        } else if let Some(row) = widget.downcast_ref::<gtk::ListBoxRow>() {
            self.list.select_row(Some(row));
        }
    }

    /// App Bay, if it is installed.
    fn app_bay(&self) -> Option<elavo_common::DesktopAppInfo> {
        self.apps
            .borrow()
            .iter()
            .find(|a| a.id == APP_BAY)
            .map(|a| a.info.clone())
            .or_else(|| elavo_common::DesktopAppInfo::new(APP_BAY))
    }

    fn show(self: &Rc<Self>, applications: bool) {
        self.apps.replace(
            load_applications()
                .shown()
                .filter(|a| a.id != LAUNCHER)
                .cloned()
                .collect(),
        );
        self.usage.replace(usage::Usage::load(&usage_path()));
        // Files are found again on the first search of this opening.
        self.scan_serial.set(self.scan_serial.get().wrapping_add(1));
        self.scan_started.set(false);
        self.scanned_files.replace(Rc::default());
        self.recent_files.replace(Vec::new());
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
        self.close_menu();
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

/// The grid tile `widget` is, or is inside.
fn tile_of(widget: &gtk::Widget) -> Option<gtk::FlowBoxChild> {
    widget
        .downcast_ref::<gtk::FlowBoxChild>()
        .cloned()
        .or_else(|| {
            widget
                .ancestor(gtk::FlowBoxChild::static_type())
                .and_downcast()
        })
}

/// A menu to fill with [`Lens::menu_item`] and show with
/// [`Lens::present_menu`].
fn new_menu() -> (gtk::Popover, gtk::Box) {
    let popover = gtk::Popover::builder()
        .has_arrow(false)
        .position(gtk::PositionType::Bottom)
        .css_classes(["elavo-lens-menu"])
        .build();
    let items = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .build();
    (popover, items)
}

/// Where the file scan looks below the home folder: the usual folders,
/// as named in `user-dirs.dirs`, or their standard names.
fn file_places(home: &std::path::Path) -> Vec<std::path::PathBuf> {
    use glib::UserDirectory as Dir;
    [
        (Dir::Desktop, "Desktop"),
        (Dir::Documents, "Documents"),
        (Dir::Downloads, "Downloads"),
        (Dir::Pictures, "Pictures"),
        (Dir::Music, "Music"),
        (Dir::Videos, "Videos"),
    ]
    .into_iter()
    .map(|(dir, name)| glib::user_special_dir(dir).unwrap_or_else(|| home.join(name)))
    .collect()
}

/// Recently used local files that still exist, most recent first.
fn recent_files() -> Vec<file_search::FileEntry> {
    let mut items: Vec<(i64, file_search::FileEntry)> = gtk::RecentManager::default()
        .items()
        .into_iter()
        .filter_map(|item| {
            let path = gio::File::for_uri(&item.uri()).path()?;
            let meta = std::fs::metadata(&path).ok()?;
            let when = item.modified().to_unix();
            Some((
                when,
                file_search::FileEntry {
                    name: item.display_name().to_string(),
                    is_dir: meta.is_dir(),
                    path,
                    recent: true,
                },
            ))
        })
        .collect();
    items.sort_by_key(|(when, _)| std::cmp::Reverse(*when));
    items.into_iter().map(|(_, entry)| entry).collect()
}

/// Whether App Bay installed `app`, so it can update or remove it: a
/// Flatpak, or an Applications entry whose `entry.json` names a Flatpak or
/// an AppImage.
fn app_bay_manages(app: &AppEntry) -> bool {
    let flatpak = app.info.string("X-Flatpak");
    let json = entry_json(app);
    menu::managed_by_app_bay(
        flatpak.as_deref(),
        json.as_deref().and_then(menu::entry_kind),
    )
}

/// The ID App Bay opens `app`'s page by, see [`menu::app_bay_id`].
fn app_bay_id(app: &AppEntry) -> Option<String> {
    let flatpak = app.info.string("X-Flatpak");
    menu::app_bay_id(flatpak.as_deref(), entry_json(app).as_deref())
}

/// The `entry.json` beside `app`'s desktop file, when it is an `.eapp`.
fn entry_json(app: &AppEntry) -> Option<String> {
    app.info
        .filename()
        .and_then(|path| {
            path.parent()
                .filter(|p| p.extension().is_some_and(|e| e == "eapp"))
                .map(|bundle| bundle.join("entry.json"))
        })
        .and_then(|json| std::fs::read_to_string(json).ok())
}

/// Open App Bay on `id`'s page: `elavo-app-bay --app ID`, which an open
/// App Bay takes over. Without an ID, or when that cannot start, App Bay
/// opens as usual.
fn open_app_bay(info: &elavo_common::DesktopAppInfo, id: Option<&str>) {
    if let Some(id) = id {
        let argv: [&std::ffi::OsStr; 3] = ["elavo-app-bay".as_ref(), "--app".as_ref(), id.as_ref()];
        match gio::Subprocess::newv(&argv, gio::SubprocessFlags::NONE) {
            Ok(_) => return,
            Err(err) => eprintln!("lens: elavo-app-bay --app {id}: {err}"),
        }
    }
    elavo_common::launch(info, None);
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
