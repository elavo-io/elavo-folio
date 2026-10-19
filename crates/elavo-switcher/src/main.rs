//! Alt+Tab. One icon per running application, held while Alt is down.
//! Alt+` cycles that application's windows and draws nothing.
//! Spec: elavo-architecture-and-planning-internal, planning/desktop/app-switcher.md.

mod audio;
mod badges;
mod model;
mod preview;

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Instant;

use elavo_common::{apps::launch, toplevels, AppIndex};
use elavo_switcher_settings::{
    Appearance, DisplayMode, FontFace, FontScale, Layout, Scope, SelectionColor, Settings, Shelf,
    Truncation,
};
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use model::{
    alt_tab_skipped, entries, initial_index, AppSnap, Entry, ListInput, WinSnap, WindowCycle,
};
use wayland_client::backend::ObjectId;

const APP_ID: &str = "io.elavo.Switcher";

struct LiveWindow {
    id: u64,
    toplevel: toplevels::Toplevel,
    focus: u64,
}

struct Open {
    rows: Vec<Entry>,
    selection: usize,
    origin: Option<String>,
    scope: Option<Scope>,
    sticky: bool,
    holding: bool,
    /// The overlay has keyboard focus, so modifier state is the real one.
    grabbed: bool,
    keys_owned: bool,
    query: String,
    shelf: bool,
    shelf_at: usize,
    generation: u64,
    last_step: Instant,
}

struct Switcher {
    window: gtk::ApplicationWindow,
    panel: gtk::Box,
    tiles: gtk::Box,
    shelf_box: gtk::Box,
    name: gtk::Label,
    actions: gtk::Box,
    css: gtk::CssProvider,
    controller: Option<toplevels::Controller>,
    index: RefCell<AppIndex>,
    windows: RefCell<HashMap<ObjectId, LiveWindow>>,
    next_id: Cell<u64>,
    focus_clock: Cell<u64>,
    launch_clock: Cell<u64>,
    launched: RefCell<HashMap<String, u64>>,
    closed: RefCell<Vec<(String, String)>>,
    settings: RefCell<Settings>,
    badges: badges::Badges,
    playing: RefCell<HashSet<String>>,
    shots: RefCell<HashMap<(String, String), gdk::MemoryTexture>>,
    previews: preview::Previews,
    open: RefCell<Option<Open>>,
    hovered: Cell<Option<usize>>,
    /// Alt+` order, frozen so a run visits every window of the application.
    grave_cycle: RefCell<Option<(String, WindowCycle, Instant)>>,
    _monitor: RefCell<Option<gio::FileMonitor>>,
}

impl Switcher {
    fn settings(&self) -> Settings {
        self.settings.borrow().clone()
    }

    fn apps(&self) -> Vec<AppSnap> {
        let index = self.index.borrow();
        let mut grouped: Vec<AppSnap> = Vec::new();
        for window in self.windows.borrow().values() {
            if window.toplevel.app_id.is_empty() {
                continue;
            }
            let matched = index.match_app_id(&window.toplevel.app_id);
            let key = matched
                .map(|entry| entry.id.clone())
                .unwrap_or_else(|| window.toplevel.app_id.clone());
            let name = matched
                .map(|entry| entry.name.clone())
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| window.toplevel.app_id.clone());
            let snap = WinSnap {
                key: window.id,
                title: window.toplevel.title.clone(),
                minimized: window.toplevel.minimized,
                fullscreen: window.toplevel.fullscreen,
                focus: window.focus,
                outputs: window.toplevel.outputs.clone(),
            };
            if let Some(app) = grouped.iter_mut().find(|app| app.key == key) {
                app.focus = app.focus.max(window.focus);
                app.windows.push(snap);
            } else {
                grouped.push(AppSnap {
                    focus: window.focus,
                    launch_order: self.launched.borrow().get(&key).copied().unwrap_or(0),
                    key,
                    name,
                    windows: vec![snap],
                });
            }
        }
        grouped
    }

    fn focused_key(&self) -> Option<String> {
        self.apps()
            .into_iter()
            .max_by_key(|app| app.focus)
            .map(|app| app.key)
    }

    fn focused_output(&self) -> Option<String> {
        self.windows
            .borrow()
            .values()
            .filter(|window| window.toplevel.activated)
            .max_by_key(|window| window.focus)
            .and_then(|window| window.toplevel.outputs.first().cloned())
    }

    fn begin(self: &Rc<Self>, backward: bool, scope: Option<Scope>) {
        let settings = self.settings();
        if scope.is_none() {
            if let Some(app) = self
                .apps()
                .iter()
                .find(|app| self.focused_key().as_deref() == Some(app.key.as_str()))
            {
                if alt_tab_skipped(app, &settings) {
                    return;
                }
            }
        }
        let mut open = self.open.borrow_mut();
        if let Some(session) = open.as_mut() {
            if !session.keys_owned {
                let len = session.rows.len() as i32;
                if len > 0 {
                    let delta = if backward { -1 } else { 1 };
                    session.selection = (session.selection as i32 + delta).rem_euclid(len) as usize;
                    session.last_step = Instant::now();
                }
            }
            drop(open);
            self.paint();
            return;
        }
        let rows = self.rows_for(scope, "");
        let Some(selection) = initial_index(rows.len(), backward) else {
            return;
        };
        let origin = self.focused_key();
        *open = Some(Open {
            rows,
            selection,
            origin,
            scope,
            sticky: scope.is_some() || settings.stay_open_on_release,
            holding: settings.reveal_delay_ms > 0 && scope.is_none(),
            grabbed: false,
            keys_owned: false,
            query: String::new(),
            shelf: settings.window_shelf != Shelf::Off,
            shelf_at: 0,
            generation: self.focus_clock.get(),
            last_step: Instant::now(),
        });
        drop(open);
        self.present();
    }

    fn rows_for(&self, scope: Option<Scope>, text: &str) -> Vec<Entry> {
        let settings = self.settings();
        let apps = self.apps();
        let dock = dock_pins();
        let launchable = launchable_apps(&self.index.borrow());
        let playing = self.playing_keys(&apps);
        let badges = self.badges.pairs();
        let closed = self.closed.borrow().clone();
        let focused = self.focused_key();
        let output = self.focused_output();
        entries(&ListInput {
            apps: &apps,
            settings: &settings,
            scope,
            focused_key: focused.as_deref(),
            output: output.as_deref(),
            text,
            dock_order: &dock,
            launchable: &launchable,
            closed: &closed,
            badges: &badges,
            playing: &playing,
        })
    }

    fn playing_keys(&self, apps: &[AppSnap]) -> HashSet<String> {
        let names = self.playing.borrow();
        apps.iter()
            .filter(|app| {
                names.iter().any(|name| {
                    name.eq_ignore_ascii_case(&app.name)
                        || name.eq_ignore_ascii_case(app.key.trim_end_matches(".desktop"))
                })
            })
            .map(|app| app.key.clone())
            .collect()
    }

    fn present(self: &Rc<Self>) {
        self.place();
        self.paint();
        let holding = self.open.borrow().as_ref().is_some_and(|open| open.holding);
        self.panel.set_visible(!holding);
        self.window.set_keyboard_mode(KeyboardMode::Exclusive);
        self.window.present();
        self.request_shots();
        // A tap can release Alt before this surface is focused, so the
        // release is never delivered. If that is still true shortly after
        // the surface maps, commit instead of keeping the keyboard.
        let generation = self
            .open
            .borrow()
            .as_ref()
            .map(|open| open.generation)
            .unwrap_or(0);
        let weak = Rc::downgrade(self);
        glib::timeout_add_local(std::time::Duration::from_millis(80), move || {
            let Some(switcher) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            let pending = switcher
                .open
                .borrow()
                .as_ref()
                .is_some_and(|open| open.generation == generation && !open.grabbed);
            if pending {
                switcher.settle(alt_down());
            }
            glib::ControlFlow::Break
        });
    }

    /// Alt's state is only real once this client has the keyboard. A reading
    /// taken before that is empty, and committing on it hid the strip before
    /// it painted. A release that arrives after the grab still commits.
    fn on_grab(self: &Rc<Self>, mods: gdk::ModifierType) {
        let alt = mods.contains(gdk::ModifierType::ALT_MASK);
        // An early empty reading, before the surface is focused, is not a tap.
        if !alt && !self.window.is_active() {
            return;
        }
        self.settle(alt);
    }

    fn settle(self: &Rc<Self>, alt: bool) {
        let (sticky, holding, generation) = {
            let mut open = self.open.borrow_mut();
            let Some(session) = open.as_mut() else {
                return;
            };
            if session.grabbed {
                return;
            }
            session.grabbed = true;
            (session.sticky, session.holding, session.generation)
        };
        let settings = self.settings();
        if !alt {
            if settings.stay_open_on_quick_tap || sticky {
                if let Some(open) = self.open.borrow_mut().as_mut() {
                    open.holding = false;
                }
                self.panel.set_visible(true);
            } else {
                self.commit(true);
            }
            return;
        }
        if !holding {
            return;
        }
        let weak = Rc::downgrade(self);
        glib::timeout_add_local(
            std::time::Duration::from_millis(settings.reveal_delay_ms as u64),
            move || {
                let Some(switcher) = weak.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                let still = switcher
                    .open
                    .borrow()
                    .as_ref()
                    .is_some_and(|open| open.holding && open.generation == generation);
                if still {
                    if let Some(open) = switcher.open.borrow_mut().as_mut() {
                        open.holding = false;
                    }
                    switcher.panel.set_visible(true);
                }
                glib::ControlFlow::Break
            },
        );
    }

    fn place(&self) {
        let display = gdk::Display::default();
        let Some(display) = display else {
            return;
        };
        let monitors = display.monitors();
        let monitor = match self.settings().display_mode {
            DisplayMode::First => monitors.item(0).and_downcast::<gdk::Monitor>(),
            DisplayMode::Cursor => pointer_monitor(&display)
                .or_else(|| monitors.item(0).and_downcast::<gdk::Monitor>()),
            DisplayMode::Focused => {
                let output = self.focused_output();
                (0..monitors.n_items())
                    .find_map(|i| {
                        let monitor = monitors.item(i)?.downcast::<gdk::Monitor>().ok()?;
                        let connector = monitor.connector().map(|c| c.to_string());
                        (connector.as_deref() == output.as_deref()).then_some(monitor)
                    })
                    .or_else(|| monitors.item(0).and_downcast())
            }
        };
        self.window.set_monitor(monitor.as_ref());
    }

    fn request_shots(&self) {
        if self.settings().layout != Layout::Previews
            || !elavo_switcher_settings::previews_available()
        {
            return;
        }
        let rows = self
            .open
            .borrow()
            .as_ref()
            .map(|open| open.rows.clone())
            .unwrap_or_default();
        for row in rows {
            if row.launch {
                continue;
            }
            let app_id = self
                .windows
                .borrow()
                .values()
                .find(|window| Some(window.id) == row.window_key)
                .map(|window| window.toplevel.app_id.clone())
                .unwrap_or_default();
            if !app_id.is_empty() {
                self.previews.request(&app_id, &row.title);
            }
        }
    }

    fn paint(self: &Rc<Self>) {
        let settings = self.settings();
        let open = self.open.borrow();
        let Some(session) = open.as_ref() else {
            return;
        };
        self.apply_css(&settings);
        while let Some(child) = self.tiles.first_child() {
            self.tiles.remove(&child);
        }
        while let Some(child) = self.shelf_box.first_child() {
            self.shelf_box.remove(&child);
        }
        let icon = (64 * settings.panel_scale / 100).clamp(24, 160);
        let per_window = matches!(settings.layout, Layout::List | Layout::Previews);
        self.tiles.set_orientation(if per_window {
            gtk::Orientation::Vertical
        } else {
            gtk::Orientation::Horizontal
        });
        if settings.layout == Layout::List {
            self.tiles
                .set_width_request(420 * settings.list_width / 100);
        } else {
            self.tiles.set_width_request(-1);
        }
        if settings.layout == Layout::Grid {
            self.tiles.set_orientation(gtk::Orientation::Horizontal);
        }
        let selected = session.rows.get(session.selection);
        for (index, row) in session.rows.iter().enumerate() {
            self.tiles
                .append(&self.tile(row, index, icon, index == session.selection));
        }
        let label = selected
            .map(|row| {
                if settings.layout == Layout::Previews && !row.title.is_empty() {
                    format!("{} — {}", display_name(&row.name), row.title)
                } else {
                    display_name(&row.name)
                }
            })
            .unwrap_or_default();
        self.name.set_label(&label);
        self.name.set_visible(!label.is_empty());
        self.actions.set_visible(settings.hover_actions);
        let flags = [
            settings.hover_close,
            settings.hover_minimize,
            settings.hover_maximize,
            settings.hover_quit,
        ];
        let mut child = self.actions.first_child();
        let mut index = 0;
        while let Some(widget) = child {
            child = widget.next_sibling();
            widget.set_visible(flags.get(index).copied().unwrap_or(false));
            index += 1;
        }
        let show_shelf =
            session.shelf && (settings.window_shelf != Shelf::Off || settings.window_drill);
        self.shelf_box.set_visible(show_shelf);
        if show_shelf {
            if let Some(row) = selected {
                self.fill_shelf(row, icon);
            }
        }
    }

    fn tile(self: &Rc<Self>, row: &Entry, index: usize, icon: i32, selected: bool) -> gtk::Widget {
        let settings = self.settings();
        let button = gtk::Button::builder()
            .css_classes(["elavo-switcher-icon"])
            .build();
        if selected {
            button.add_css_class("selected");
        }
        let list = settings.layout == Layout::List;
        let content = gtk::Box::builder()
            .orientation(if list {
                gtk::Orientation::Horizontal
            } else {
                gtk::Orientation::Vertical
            })
            .spacing(6)
            .halign(if list {
                gtk::Align::Fill
            } else {
                gtk::Align::Center
            })
            .hexpand(list)
            .build();
        if let Some(texture) = self.shots.borrow().get(&(
            self.windows
                .borrow()
                .values()
                .find(|window| Some(window.id) == row.window_key)
                .map(|window| window.toplevel.app_id.clone())
                .unwrap_or_default(),
            row.title.clone(),
        )) {
            if settings.layout == Layout::Previews {
                let picture = gtk::Picture::for_paintable(texture);
                picture.set_size_request(icon * 2, (icon as f64 * 1.4) as i32);
                picture.set_can_shrink(true);
                content.append(&picture);
            }
        }
        if settings.layout != Layout::Previews || content.first_child().is_none() {
            content.append(&app_icon(&self.index.borrow(), &row.key, icon));
        }
        if settings.show_application_names || list {
            let texts = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .halign(if list {
                    gtk::Align::Start
                } else {
                    gtk::Align::Center
                })
                .hexpand(list)
                .build();
            let caption = gtk::Label::builder()
                .label(display_name(&row.name))
                .ellipsize(ellipsize(settings.title_truncation))
                .halign(if list {
                    gtk::Align::Start
                } else {
                    gtk::Align::Center
                })
                .hexpand(list)
                .build();
            texts.append(&caption);
            // A window title that starts with the open file must not replace
            // the application name. Cursor's title is "file - project - Cursor",
            // and a short list row was keeping only the file.
            if list && !row.title.is_empty() && row.title != row.name {
                let title = gtk::Label::builder()
                    .label(&row.title)
                    .ellipsize(ellipsize(settings.title_truncation))
                    .halign(gtk::Align::Start)
                    .hexpand(true)
                    .css_classes(["elavo-switcher-title"])
                    .build();
                texts.append(&title);
            }
            content.append(&texts);
        }
        if settings.show_window_status && (row.minimized || row.fullscreen) {
            content.append(&gtk::Image::from_icon_name(if row.minimized {
                "window-minimize-symbolic"
            } else {
                "view-fullscreen-symbolic"
            }));
        }
        if let Some(badge) = &row.badge {
            content.append(
                &gtk::Label::builder()
                    .label(badge)
                    .css_classes(["elavo-switcher-badge"])
                    .build(),
            );
        }
        if row.playing {
            content.append(&gtk::Image::from_icon_name("audio-volume-high-symbolic"));
        }
        if let Some(letter) = row.letter {
            content.append(
                &gtk::Label::builder()
                    .label(letter.to_string())
                    .css_classes(["elavo-switcher-letter"])
                    .build(),
            );
        }
        button.set_child(Some(&content));
        let weak = Rc::downgrade(self);
        button.connect_clicked(move |_| {
            if let Some(switcher) = weak.upgrade() {
                if let Some(open) = switcher.open.borrow_mut().as_mut() {
                    open.selection = index;
                }
                switcher.commit(true);
            }
        });
        if settings.hover_selection {
            let weak = Rc::downgrade(self);
            let motion = gtk::EventControllerMotion::new();
            motion.connect_enter(move |_, _, _| {
                if let Some(switcher) = weak.upgrade() {
                    switcher.hovered.set(Some(index));
                    if let Some(open) = switcher.open.borrow_mut().as_mut() {
                        open.selection = index;
                    }
                    switcher.paint();
                }
            });
            button.add_controller(motion);
        }
        button.upcast()
    }

    fn fill_shelf(self: &Rc<Self>, row: &Entry, icon: i32) {
        let apps = self.apps();
        let Some(app) = apps.iter().find(|app| app.key == row.key) else {
            return;
        };
        let settings = self.settings();
        if app.windows.len() < 2 && !settings.window_shelf_single && !settings.window_drill {
            return;
        }
        let at = self
            .open
            .borrow()
            .as_ref()
            .map(|open| open.shelf_at)
            .unwrap_or(0);
        for (index, window) in app.windows.iter().enumerate() {
            let button = gtk::Button::with_label(if window.title.is_empty() {
                row.name.as_str()
            } else {
                window.title.as_str()
            });
            if index == at {
                button.add_css_class("suggested-action");
            }
            let weak = Rc::downgrade(self);
            let key = window.key;
            button.connect_clicked(move |_| {
                if let Some(switcher) = weak.upgrade() {
                    switcher.activate_id(key);
                    switcher.finish(false);
                }
            });
            let _ = icon;
            self.shelf_box.append(&button);
        }
    }

    fn apply_css(&self, settings: &Settings) {
        let dark = match settings.appearance {
            Appearance::Dark => true,
            Appearance::Light => false,
            Appearance::System => {
                gtk::Settings::default().is_some_and(|s| s.is_gtk_application_prefer_dark_theme())
            }
        };
        let alpha = f64::from(settings.opacity) / 100.0;
        let (red, green, blue, fg) = if dark {
            (28, 28, 30, "white")
        } else {
            (245, 245, 247, "black")
        };
        let radius = match settings.corner_radius {
            -1 => 0,
            0 => (16 * settings.panel_scale / 100).clamp(8, 28),
            n => n,
        };
        let selection = selection_color(settings, dark);
        let size = font_px(settings.font_scale);
        let family = match settings.font_face {
            FontFace::System => String::new(),
            FontFace::Rounded => "font-family: sans-serif;".into(),
            FontFace::Serif => "font-family: serif;".into(),
            FontFace::Monospace => "font-family: monospace;".into(),
        };
        let weight = if settings.bold_selected_label {
            "font-weight: bold;"
        } else {
            ""
        };
        self.css.load_from_string(&format!(
            "
            .elavo-switcher {{ background: transparent; }}
            .elavo-switcher-panel {{
                background: rgba({red}, {green}, {blue}, {alpha});
                color: {fg};
                border-radius: {radius}px;
                padding: 18px 22px 14px;
            }}
            .elavo-switcher-icon {{
                background: transparent;
                border-radius: 12px;
                padding: 6px;
                box-shadow: none;
            }}
            .elavo-switcher-icon.selected {{ background: {selection}; }}
            .elavo-switcher-name {{ font-size: {size}px; {family} {weight} }}
            .elavo-switcher-title {{ opacity: 0.65; }}
            .elavo-switcher-badge {{
                background: #e35d5d;
                color: white;
                border-radius: 9px;
                padding: 0 5px;
                font-size: 11px;
            }}
            .elavo-switcher-letter {{ font-size: 11px; opacity: 0.7; }}
            "
        ));
    }

    fn step(self: &Rc<Self>, delta: i32) {
        let mut open = self.open.borrow_mut();
        let Some(session) = open.as_mut() else {
            return;
        };
        let len = session.rows.len() as i32;
        if len == 0 {
            return;
        }
        session.selection = (session.selection as i32 + delta).rem_euclid(len) as usize;
        session.shelf_at = 0;
        session.last_step = Instant::now();
        drop(open);
        self.paint();
    }

    fn on_key(self: &Rc<Self>, key: gdk::Key, mods: gdk::ModifierType) {
        if self.open.borrow().is_none() {
            return;
        }
        if let Some(open) = self.open.borrow_mut().as_mut() {
            open.keys_owned = true;
        }
        let settings = self.settings();
        let shift = mods.contains(gdk::ModifierType::SHIFT_MASK);
        match key {
            gdk::Key::Alt_L | gdk::Key::Alt_R => {}
            gdk::Key::Escape => {
                let query = self
                    .open
                    .borrow()
                    .as_ref()
                    .is_some_and(|open| !open.query.is_empty());
                if query {
                    self.set_query("");
                } else {
                    self.finish(false);
                }
            }
            gdk::Key::Return | gdk::Key::KP_Enter => self.commit(true),
            gdk::Key::Tab | gdk::Key::ISO_Left_Tab => {
                self.step(if shift || key == gdk::Key::ISO_Left_Tab {
                    -1
                } else {
                    1
                })
            }
            gdk::Key::Right => self.step(1),
            gdk::Key::Left => self.step(-1),
            gdk::Key::Down => self.shelf_move(1, true),
            gdk::Key::Up => self.shelf_move(-1, false),
            gdk::Key::h | gdk::Key::l | gdk::Key::j | gdk::Key::k
                if settings.vim_navigation && !shift =>
            {
                match key {
                    gdk::Key::h => self.step(-1),
                    gdk::Key::l => self.step(1),
                    gdk::Key::j => self.shelf_move(1, true),
                    gdk::Key::k => self.shelf_move(-1, false),
                    _ => {}
                }
            }
            gdk::Key::q | gdk::Key::Q => self.quit_selected(),
            gdk::Key::h | gdk::Key::H => self.hide_selected(),
            gdk::Key::grave => {
                if settings.grave_reverses {
                    self.step(-1);
                } else {
                    self.cycle_origin(true);
                }
            }
            gdk::Key::BackSpace if settings.fuzzy_search => {
                let mut query = self
                    .open
                    .borrow()
                    .as_ref()
                    .map(|open| open.query.clone())
                    .unwrap_or_default();
                query.pop();
                self.set_query(&query);
            }
            other => {
                let Some(ch) = other.to_unicode() else {
                    return;
                };
                if settings.fuzzy_search && (ch.is_alphanumeric() || ch == ' ') {
                    let mut query = self
                        .open
                        .borrow()
                        .as_ref()
                        .map(|open| open.query.clone())
                        .unwrap_or_default();
                    query.push(ch);
                    self.set_query(&query);
                } else if settings.letter_hints && ch.is_ascii_alphabetic() {
                    let rows = self
                        .open
                        .borrow()
                        .as_ref()
                        .map(|open| open.rows.clone())
                        .unwrap_or_default();
                    let from = self
                        .open
                        .borrow()
                        .as_ref()
                        .map(|open| open.selection)
                        .unwrap_or(0);
                    if let Some(index) = model::letter_index(&rows, ch, from) {
                        if let Some(open) = self.open.borrow_mut().as_mut() {
                            open.selection = index;
                        }
                        self.paint();
                    }
                }
            }
        }
    }

    fn on_release(self: &Rc<Self>, key: gdk::Key) {
        if !matches!(key, gdk::Key::Alt_L | gdk::Key::Alt_R) {
            return;
        }
        let Some(open) = self
            .open
            .borrow()
            .as_ref()
            .map(|open| (open.sticky, open.holding))
        else {
            return;
        };
        let settings = self.settings();
        if open.0 {
            return;
        }
        if open.1 && settings.stay_open_on_quick_tap {
            if let Some(session) = self.open.borrow_mut().as_mut() {
                session.holding = false;
            }
            self.panel.set_visible(true);
            return;
        }
        self.commit(true);
    }

    fn set_query(self: &Rc<Self>, query: &str) {
        let scope = self.open.borrow().as_ref().and_then(|open| open.scope);
        let rows = self.rows_for(scope, query);
        if let Some(open) = self.open.borrow_mut().as_mut() {
            open.query = query.to_owned();
            open.rows = rows;
            open.selection = 0;
        }
        self.paint();
    }

    fn shelf_move(self: &Rc<Self>, delta: i32, enter: bool) {
        let settings = self.settings();
        if enter && (settings.window_drill || settings.window_shelf != Shelf::Off) {
            if let Some(open) = self.open.borrow_mut().as_mut() {
                open.shelf = true;
            }
        }
        if !enter {
            if let Some(open) = self.open.borrow_mut().as_mut() {
                if delta < 0 && open.shelf_at == 0 {
                    open.shelf = false;
                }
            }
        }
        let step_instead = {
            let mut open = self.open.borrow_mut();
            let Some(session) = open.as_mut() else {
                return;
            };
            if session.shelf {
                if delta < 0 {
                    session.shelf_at = session.shelf_at.saturating_sub(1);
                } else {
                    session.shelf_at = session.shelf_at.saturating_add(delta as usize);
                }
                false
            } else {
                !enter
            }
        };
        if step_instead {
            self.step(delta);
            return;
        }
        self.paint();
    }

    fn quit_selected(&self) {
        self.for_selected_windows(|switcher, window| {
            if let Some(controller) = &switcher.controller {
                controller.close(&window.toplevel);
            }
        });
    }

    fn hide_selected(&self) {
        self.for_selected_windows(|switcher, window| {
            if let Some(controller) = &switcher.controller {
                controller.minimize(&window.toplevel);
            }
        });
    }

    fn for_selected_windows(&self, f: impl Fn(&Switcher, &LiveWindow)) {
        let key = self
            .open
            .borrow()
            .as_ref()
            .and_then(|open| open.rows.get(open.selection).map(|row| row.key.clone()));
        let Some(key) = key else {
            return;
        };
        let owned: Vec<(u64, toplevels::Toplevel)> = self
            .windows
            .borrow()
            .values()
            .filter(|window| self.window_key(window) == key)
            .map(|window| (window.id, window.toplevel.clone()))
            .collect();
        for (id, toplevel) in owned {
            let live = LiveWindow {
                id,
                toplevel,
                focus: 0,
            };
            f(self, &live);
        }
    }

    fn window_key(&self, window: &LiveWindow) -> String {
        self.index
            .borrow()
            .match_app_id(&window.toplevel.app_id)
            .map(|entry| entry.id.clone())
            .unwrap_or_else(|| window.toplevel.app_id.clone())
    }

    fn cycle_windows(&self, forward: bool) {
        let origin = self
            .open
            .borrow()
            .as_ref()
            .and_then(|open| open.origin.clone())
            .or_else(|| self.focused_key());
        let Some(origin) = origin else {
            return;
        };
        let apps = self.apps();
        let Some(app) = apps.iter().find(|app| app.key == origin) else {
            return;
        };
        let now = Instant::now();
        let mut slot = self.grave_cycle.borrow_mut();
        let continued = slot.as_mut().and_then(|(app_key, cycle, at)| {
            let recent = now.duration_since(*at) < std::time::Duration::from_millis(1000);
            (app_key == &origin && recent)
                .then(|| cycle.advance(&app.windows, forward))
                .flatten()
                .inspect(|_| *at = now)
        });
        let id = if let Some(id) = continued {
            id
        } else if let Some(cycle) = WindowCycle::start(&app.windows, forward) {
            let Some(id) = cycle.current() else {
                return;
            };
            *slot = Some((origin, cycle, now));
            id
        } else {
            return;
        };
        drop(slot);
        self.activate_id(id);
    }

    fn cycle_origin(&self, forward: bool) {
        self.cycle_windows(forward);
    }

    fn activate_id(&self, id: u64) {
        let Some(controller) = &self.controller else {
            return;
        };
        if let Some(window) = self
            .windows
            .borrow()
            .values()
            .find(|window| window.id == id)
        {
            controller.activate(&window.toplevel);
        }
    }

    fn commit(self: &Rc<Self>, activate: bool) {
        if !activate {
            self.finish(false);
            return;
        }
        let chosen = self.open.borrow().as_ref().and_then(|open| {
            if open.shelf {
                let row = open.rows.get(open.selection)?;
                let apps = self.apps();
                let app = apps.iter().find(|app| app.key == row.key)?;
                let window = app
                    .windows
                    .get(open.shelf_at)
                    .or_else(|| app.windows.first())?;
                Some((row.key.clone(), Some(window.key), false))
            } else {
                let row = open.rows.get(open.selection)?;
                Some((row.key.clone(), row.window_key, row.launch))
            }
        });
        let sound = chosen.is_some() && self.settings().sound_on_commit;
        // Drop the grab before activating, or the overlay keeps the keyboard
        // and the chosen window never receives focus.
        self.finish(false);
        if let Some((key, window, launch_app)) = chosen {
            if launch_app {
                if let Some(info) = self
                    .index
                    .borrow()
                    .get(&key)
                    .map(|entry| entry.info.clone())
                {
                    launch(&info, None);
                }
            } else if let Some(id) = window {
                self.activate_id(id);
            }
            if sound {
                audio::play_commit_sound();
            }
        }
    }

    fn finish(&self, _cancelled_marker: bool) {
        self.open.replace(None);
        self.window.set_keyboard_mode(KeyboardMode::None);
        self.window.set_visible(false);
        self.panel.set_visible(false);
    }

    fn on_toplevel(self: &Rc<Self>, event: toplevels::Event) {
        match event {
            toplevels::Event::Changed(toplevel) => {
                let mut windows = self.windows.borrow_mut();
                let id = if let Some(existing) = windows.get(&toplevel.key) {
                    existing.id
                } else {
                    let id = self.next_id.get() + 1;
                    self.next_id.set(id);
                    id
                };
                let was_active = windows
                    .get(&toplevel.key)
                    .is_some_and(|window| window.toplevel.activated);
                let mut focus = windows
                    .get(&toplevel.key)
                    .map(|window| window.focus)
                    .unwrap_or(0);
                if toplevel.activated && !was_active {
                    let clock = self.focus_clock.get() + 1;
                    self.focus_clock.set(clock);
                    focus = clock;
                }
                let matched = self
                    .index
                    .borrow()
                    .match_app_id(&toplevel.app_id)
                    .map(|e| e.id.clone());
                if let Some(key) = matched {
                    self.launched.borrow_mut().entry(key).or_insert_with(|| {
                        let clock = self.launch_clock.get() + 1;
                        self.launch_clock.set(clock);
                        clock
                    });
                }
                windows.insert(
                    toplevel.key.clone(),
                    LiveWindow {
                        id,
                        toplevel,
                        focus,
                    },
                );
            }
            toplevels::Event::Closed(key) => {
                let removed = self.windows.borrow_mut().remove(&key);
                if let Some(window) = removed {
                    let id = self.window_key(&window);
                    let still = self
                        .windows
                        .borrow()
                        .values()
                        .any(|other| self.window_key(other) == id);
                    if !still {
                        let name = self
                            .index
                            .borrow()
                            .get(&id)
                            .map(|entry| entry.name.clone())
                            .unwrap_or_else(|| id.clone());
                        let mut closed = self.closed.borrow_mut();
                        closed.retain(|(existing, _)| existing != &id);
                        closed.insert(0, (id, name));
                        closed.truncate(50);
                    }
                }
            }
        }
    }

    fn on_scroll(self: &Rc<Self>, dy: f64) {
        let settings = self.settings();
        if !settings.scroll_to_switch || dy == 0.0 {
            return;
        }
        let delta = if dy > 0.0 { 1 } else { -1 };
        let delta = if settings.scroll_reverse {
            -delta
        } else {
            delta
        };
        self.step(delta);
    }

    fn hover_action(self: &Rc<Self>, kind: Hover) {
        let key = {
            let open = self.open.borrow();
            let Some(session) = open.as_ref() else {
                return;
            };
            let index = self.hovered.get().unwrap_or(session.selection);
            session
                .rows
                .get(index)
                .map(|row| (row.key.clone(), row.window_key))
        };
        let Some((app_key, window_key)) = key else {
            return;
        };
        let targets: Vec<toplevels::Toplevel> = self
            .windows
            .borrow()
            .values()
            .filter(|window| {
                self.window_key(window) == app_key
                    && (kind == Hover::Quit || Some(window.id) == window_key)
            })
            .map(|window| window.toplevel.clone())
            .collect();
        let Some(controller) = &self.controller else {
            return;
        };
        for toplevel in &targets {
            match kind {
                Hover::Close | Hover::Quit => controller.close(toplevel),
                Hover::Minimize => controller.minimize(toplevel),
                Hover::Maximize => controller.toggle_maximized(toplevel),
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Hover {
    Close,
    Minimize,
    Maximize,
    Quit,
}

fn app_icon(index: &AppIndex, id: &str, size: i32) -> gtk::Image {
    let image = gtk::Image::builder().pixel_size(size).build();
    match index.get(id).and_then(|entry| entry.icon()) {
        Some(icon) => image.set_from_gicon(&icon),
        None => image.set_icon_name(Some("application-x-executable")),
    }
    image
}

fn ellipsize(truncation: Truncation) -> gtk::pango::EllipsizeMode {
    match truncation {
        Truncation::Head => gtk::pango::EllipsizeMode::Start,
        Truncation::Middle => gtk::pango::EllipsizeMode::Middle,
        Truncation::Tail => gtk::pango::EllipsizeMode::End,
    }
}

fn font_px(scale: FontScale) -> i32 {
    match scale {
        FontScale::ExtraSmall => 11,
        FontScale::Small => 12,
        FontScale::Standard => 13,
        FontScale::Large => 16,
        FontScale::ExtraLarge => 18,
    }
}

fn selection_color(settings: &Settings, dark: bool) -> String {
    match settings.selection_color {
        SelectionColor::Default => {
            if dark {
                "alpha(white, 0.88)".into()
            } else {
                "alpha(black, 0.10)".into()
            }
        }
        SelectionColor::Blue => "#3584e4".into(),
        SelectionColor::Purple => "#9141ac".into(),
        SelectionColor::Pink => "#d56199".into(),
        SelectionColor::Red => "#e01b24".into(),
        SelectionColor::Orange => "#ff7800".into(),
        SelectionColor::Yellow => "#f6d32d".into(),
        SelectionColor::Green => "#33d17a".into(),
        SelectionColor::Graphite => "#9a9996".into(),
        SelectionColor::Custom => {
            let hex = settings.selection_color_hex.trim();
            if hex.len() == 7
                && hex.starts_with('#')
                && hex[1..].chars().all(|c| c.is_ascii_hexdigit())
            {
                hex.to_owned()
            } else if dark {
                "alpha(white, 0.88)".into()
            } else {
                "alpha(black, 0.10)".into()
            }
        }
    }
}

fn display_name(name: &str) -> String {
    if name.contains('.') || name.contains(' ') {
        return name.to_owned();
    }
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().to_string() + chars.as_str(),
        None => String::new(),
    }
}

fn alt_down() -> bool {
    let Some(seat) = gdk::Display::default().and_then(|display| display.default_seat()) else {
        return false;
    };
    let held = |device: Option<gdk::Device>| {
        device.is_some_and(|device| {
            device
                .modifier_state()
                .contains(gdk::ModifierType::ALT_MASK)
        })
    };
    held(seat.keyboard()) || held(seat.pointer())
}

fn pointer_monitor(display: &gdk::Display) -> Option<gdk::Monitor> {
    let pointer = display.default_seat()?.pointer()?;
    let (surface, x, y) = pointer.surface_at_position();
    let surface = surface?;
    surface.device_position(&pointer)?;
    let _ = (x, y);
    display.monitor_at_surface(&surface)
}

fn dock_pins() -> Vec<String> {
    let path = glib::user_config_dir().join("elavo/dock-pins");
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

fn launchable_apps(index: &AppIndex) -> Vec<(String, String)> {
    index
        .shown()
        .map(|entry| (entry.id.clone(), entry.name.clone()))
        .collect()
}

fn main() -> glib::ExitCode {
    let app = gtk::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();

    let switcher: Rc<std::cell::OnceCell<Rc<Switcher>>> = Rc::default();
    app.connect_command_line(move |app, cmdline| {
        let switcher = switcher.get_or_init(|| {
            std::mem::forget(app.hold());
            build(app)
        });
        let args: Vec<String> = cmdline
            .arguments()
            .into_iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        let has = |flag: &str| args.iter().any(|arg| arg == flag);
        let value = |flag: &str| {
            args.iter()
                .position(|arg| arg == flag)
                .and_then(|i| args.get(i + 1))
                .cloned()
        };
        if has("--next") {
            switcher.begin(false, None);
        } else if has("--previous") {
            switcher.begin(true, None);
        } else if has("--window-next") {
            switcher.cycle_windows(true);
        } else if has("--window-previous") {
            switcher.cycle_windows(false);
        } else if let Some(scope) = value("--scoped").as_deref().and_then(Scope::parse) {
            switcher.begin(false, Some(scope));
        }
        glib::ExitCode::SUCCESS
    });
    app.run()
}

fn build(app: &gtk::Application) -> Rc<Switcher> {
    let display = gdk::Display::default().expect("no display");
    let css = gtk::CssProvider::new();
    gtk::style_context_add_provider_for_display(
        &display,
        &css,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("App Switcher")
        .decorated(false)
        .css_classes(["elavo-switcher"])
        .build();
    window.init_layer_shell();
    window.set_namespace(Some("elavo-switcher"));
    window.set_layer(Layer::Overlay);
    window.set_keyboard_mode(KeyboardMode::None);
    window.set_exclusive_zone(-1);
    for edge in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
        window.set_anchor(edge, true);
    }

    let tiles = gtk::Box::builder()
        .spacing(4)
        .halign(gtk::Align::Center)
        .build();
    let name = gtk::Label::builder()
        .css_classes(["elavo-switcher-name"])
        .halign(gtk::Align::Center)
        .build();
    let shelf_box = gtk::Box::builder()
        .spacing(6)
        .halign(gtk::Align::Center)
        .build();
    let actions = gtk::Box::builder()
        .spacing(6)
        .halign(gtk::Align::Center)
        .build();
    let panel = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .css_classes(["elavo-switcher-panel"])
        .build();
    panel.append(&tiles);
    panel.append(&name);
    panel.append(&shelf_box);
    panel.append(&actions);
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.set_halign(gtk::Align::Center);
    root.set_valign(gtk::Align::Center);
    root.set_hexpand(true);
    root.set_vexpand(true);
    root.append(&panel);
    window.set_child(Some(&root));

    let (controller, events) = match toplevels::start() {
        Ok(pair) => (Some(pair.0), Some(pair.1)),
        Err(err) => {
            glib::g_warning!("elavo-switcher", "no window list: {err}");
            (None, None)
        }
    };
    let (previews, shots) = preview::start();

    let switcher = Rc::new(Switcher {
        window: window.clone(),
        panel,
        tiles,
        shelf_box,
        name,
        actions: actions.clone(),
        css,
        controller,
        index: RefCell::new(AppIndex::load()),
        windows: RefCell::default(),
        next_id: Cell::new(0),
        focus_clock: Cell::new(0),
        launch_clock: Cell::new(0),
        launched: RefCell::default(),
        closed: RefCell::default(),
        settings: RefCell::new(Settings::load()),
        badges: badges::Badges::default(),
        playing: RefCell::default(),
        shots: RefCell::default(),
        previews,
        open: RefCell::new(None),
        hovered: Cell::new(None),
        grave_cycle: RefCell::new(None),
        _monitor: RefCell::new(None),
    });

    for (label, kind) in [
        ("Close", Hover::Close),
        ("Minimize", Hover::Minimize),
        ("Maximize", Hover::Maximize),
        ("Quit", Hover::Quit),
    ] {
        let button = gtk::Button::with_label(label);
        let weak = Rc::downgrade(&switcher);
        button.connect_clicked(move |_| {
            if let Some(switcher) = weak.upgrade() {
                switcher.hover_action(kind);
            }
        });
        actions.append(&button);
    }

    if let Some(events) = events {
        let weak = Rc::downgrade(&switcher);
        glib::spawn_future_local(async move {
            while let Ok(event) = events.recv().await {
                let Some(switcher) = weak.upgrade() else {
                    break;
                };
                switcher.on_toplevel(event);
            }
        });
    }
    let weak = Rc::downgrade(&switcher);
    glib::spawn_future_local(async move {
        while let Ok(shot) = shots.recv().await {
            let Some(switcher) = weak.upgrade() else {
                break;
            };
            let bytes = glib::Bytes::from_owned(shot.rgba);
            let texture = gdk::MemoryTexture::new(
                shot.width,
                shot.height,
                gdk::MemoryFormat::R8g8b8a8,
                &bytes,
                (shot.width as usize) * 4,
            );
            switcher
                .shots
                .borrow_mut()
                .insert((shot.app_id, shot.title), texture);
            if switcher.open.borrow().is_some() {
                switcher.paint();
            }
        }
    });

    let weak = Rc::downgrade(&switcher);
    switcher.badges.start(move || {
        if let Some(switcher) = weak.upgrade() {
            if switcher.open.borrow().is_some() && switcher.settings().show_unread_badges {
                let scope = switcher.open.borrow().as_ref().and_then(|open| open.scope);
                let query = switcher
                    .open
                    .borrow()
                    .as_ref()
                    .map(|open| open.query.clone())
                    .unwrap_or_default();
                let rows = switcher.rows_for(scope, &query);
                if let Some(open) = switcher.open.borrow_mut().as_mut() {
                    let selected = open.rows.get(open.selection).map(|row| row.key.clone());
                    open.rows = rows;
                    if let Some(key) = selected {
                        if let Some(index) = open.rows.iter().position(|row| row.key == key) {
                            open.selection = index;
                        }
                    }
                }
                switcher.paint();
            }
        }
    });

    let weak = Rc::downgrade(&switcher);
    glib::timeout_add_local(std::time::Duration::from_secs(2), move || {
        if let Some(switcher) = weak.upgrade() {
            if switcher.settings().show_audio {
                switcher.playing.replace(audio::playing_names());
            }
        }
        glib::ControlFlow::Continue
    });

    let weak = Rc::downgrade(&switcher);
    switcher._monitor.replace(Settings::watch(move |settings| {
        if let Some(switcher) = weak.upgrade() {
            switcher.settings.replace(settings.clone());
        }
    }));

    let keys = gtk::EventControllerKey::new();
    let weak = Rc::downgrade(&switcher);
    keys.connect_key_pressed(move |_, key, _, mods| {
        if let Some(switcher) = weak.upgrade() {
            switcher.on_key(key, mods);
        }
        glib::Propagation::Stop
    });
    let weak = Rc::downgrade(&switcher);
    keys.connect_key_released(move |_, key, _, _| {
        if let Some(switcher) = weak.upgrade() {
            switcher.on_release(key);
        }
    });
    let weak = Rc::downgrade(&switcher);
    keys.connect_modifiers(move |_, mods| {
        if let Some(switcher) = weak.upgrade() {
            switcher.on_grab(mods);
        }
        glib::Propagation::Proceed
    });
    window.add_controller(keys);

    let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
    let weak = Rc::downgrade(&switcher);
    scroll.connect_scroll(move |_, _, dy| {
        if let Some(switcher) = weak.upgrade() {
            switcher.on_scroll(dy);
        }
        glib::Propagation::Stop
    });
    window.add_controller(scroll);

    let click = gtk::GestureClick::new();
    let weak = Rc::downgrade(&switcher);
    let win = window.clone();
    click.connect_pressed(move |_, _, x, y| {
        let Some(switcher) = weak.upgrade() else {
            return;
        };
        if !switcher.settings().click_outside_to_dismiss {
            return;
        }
        let on_button = win
            .pick(x, y, gtk::PickFlags::DEFAULT)
            .is_some_and(|widget| widget.ancestor(gtk::Button::static_type()).is_some());
        if !on_button {
            switcher.finish(false);
        }
    });
    window.add_controller(click);

    // The hover-action row follows which buttons the person turned on.
    let weak = Rc::downgrade(&switcher);
    switcher.actions.connect_realize(move |_| {
        let _ = weak.upgrade();
    });
    let switcher_actions = switcher.clone();
    let refresh_actions = move || {
        let settings = switcher_actions.settings();
        let mut child = switcher_actions.actions.first_child();
        let flags = [
            settings.hover_close,
            settings.hover_minimize,
            settings.hover_maximize,
            settings.hover_quit,
        ];
        let mut i = 0;
        while let Some(widget) = child {
            child = widget.next_sibling();
            widget.set_visible(flags.get(i).copied().unwrap_or(false));
            i += 1;
        }
    };
    refresh_actions();

    switcher
}
