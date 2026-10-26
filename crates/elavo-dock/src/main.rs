//! The Elavo dock. Spec: elavo-architecture-and-planning-internal,
//! planning/desktop/dock.md.

mod badges;
mod files;
mod menus;
mod model;
mod others;
mod pins;
mod stacks;
mod trash;
mod view;

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use elavo_common::toplevels::{self, Controller, Toplevel};
use elavo_common::{AppEntry, AppIndex, ABOUT_TITLE, PANEL_APP_ID};
use elavo_dock_settings::Settings;
use gio_unix::prelude::*;
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use wayland_client::backend::ObjectId;

use model::{Item, Sections};
use others::Other;
use view::{Bounce, DockView, Host, Intent, Payload, Section, Tile, TileIcon};

const APP_ID: &str = "io.elavo.Dock";
const STYLE: &str = include_str!("style.css");
/// How long an icon bounces when the application never shows a window.
const LAUNCH_TIMEOUT: Duration = Duration::from_secs(12);
const APP_PREFIX: &str = "app:";
const OTHER_PREFIX: &str = "other:";
const WINDOW_PREFIX: &str = "window:";
pub const TRASH_KEY: &str = "trash";
/// About Elavo. Shown while that window is open, then dropped. Not a recent.
pub const ABOUT_KEY: &str = "about";

pub struct Dock {
    host: Rc<HostRef>,
    app: gtk::Application,
    settings: RefCell<Settings>,
    monitors: RefCell<Vec<gio::FileMonitor>>,
    index: RefCell<AppIndex>,
    pins: RefCell<Vec<String>>,
    recents: RefCell<Vec<String>>,
    others: RefCell<Vec<Other>>,
    folders: RefCell<HashMap<String, stacks::Folder>>,
    /// In the order windows first appeared.
    toplevels: RefCell<Vec<Toplevel>>,
    last_active: RefCell<HashMap<ObjectId, u64>>,
    minimized_at: RefCell<HashMap<ObjectId, u64>>,
    counter: Cell<u64>,
    controller: Option<Controller>,
    views: RefCell<Vec<(gtk::ApplicationWindow, DockView)>>,
    sections: RefCell<Sections<ObjectId>>,
    launching: RefCell<HashMap<String, Instant>>,
    badges: badges::Badges,
    trash: trash::Trash,
    dragging: RefCell<Option<String>>,
    settings_window: glib::WeakRef<gtk::Window>,
}

impl Dock {
    fn new(app: &gtk::Application) -> Rc<Self> {
        let display = gdk::Display::default().expect("no display");
        // The bar uses the theme's colors. GTK loads the dark sheet only
        // when this is set, which System Settings does for itself. The dock
        // has to do it too, or it stays on the light sheet.
        follow_color_scheme();
        let css = gtk::CssProvider::new();
        css.load_from_string(STYLE);
        gtk::style_context_add_provider_for_display(
            &display,
            &css,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );

        let (controller, events) = match toplevels::start() {
            Ok((c, rx)) => (Some(c), Some(rx)),
            Err(err) => {
                glib::g_warning!("elavo-dock", "no window list: {err}");
                (None, None)
            }
        };

        let dock = Rc::new_cyclic(|weak| Self {
            host: Rc::new(HostRef(weak.clone())),
            app: app.clone(),
            settings: RefCell::new(Settings::load()),
            monitors: RefCell::default(),
            index: RefCell::new(AppIndex::load()),
            pins: RefCell::new(pins::load()),
            recents: RefCell::new(pins::load_recents()),
            others: RefCell::new(others::load()),
            folders: RefCell::default(),
            toplevels: RefCell::default(),
            last_active: RefCell::default(),
            minimized_at: RefCell::default(),
            counter: Cell::new(0),
            controller,
            views: RefCell::default(),
            sections: RefCell::new(Sections {
                apps: Vec::new(),
                recents: Vec::new(),
                minimized: Vec::new(),
            }),
            launching: RefCell::default(),
            badges: badges::Badges::default(),
            trash: trash::Trash::default(),
            dragging: RefCell::default(),
            settings_window: glib::WeakRef::new(),
        });
        dock.drop_background_recents();

        if let Some(events) = events {
            let weak = Rc::downgrade(&dock);
            glib::spawn_future_local(async move {
                while let Ok(event) = events.recv().await {
                    let Some(dock) = weak.upgrade() else {
                        break;
                    };
                    dock.on_toplevel(event);
                }
            });
        }

        let weak = Rc::downgrade(&dock);
        if let Some(monitor) = Settings::watch(move |settings| {
            if let Some(dock) = weak.upgrade() {
                dock.apply_settings(settings.clone());
            }
        }) {
            dock.monitors.borrow_mut().push(monitor);
        }

        let weak = Rc::downgrade(&dock);
        dock.badges.start(move || {
            if let Some(dock) = weak.upgrade() {
                dock.refresh();
            }
        });
        let weak = Rc::downgrade(&dock);
        dock.trash.start(move || {
            if let Some(dock) = weak.upgrade() {
                dock.refresh();
            }
        });
        dock.load_folders();

        let weak = Rc::downgrade(&dock);
        display.monitors().connect_items_changed(move |_, _, _, _| {
            if let Some(dock) = weak.upgrade() {
                dock.rebuild_views();
            }
        });
        let weak = Rc::downgrade(&dock);
        gtk::IconTheme::for_display(&display).connect_changed(move |_| {
            if let Some(dock) = weak.upgrade() {
                dock.rebuild_views();
            }
        });
        dock.rebuild_views();
        dock
    }

    fn weak_host(&self) -> Weak<dyn Host> {
        let weak: Weak<HostRef> = Rc::downgrade(&self.host);
        weak
    }

    fn next(&self) -> u64 {
        let n = self.counter.get() + 1;
        self.counter.set(n);
        n
    }

    fn on_toplevel(self: &Rc<Self>, event: toplevels::Event) {
        match event {
            toplevels::Event::Changed(toplevel) => {
                glib::g_debug!(
                    "elavo-dock",
                    "window app_id={:?} title={:?} activated={} minimized={} -> {:?}",
                    toplevel.app_id,
                    toplevel.title,
                    toplevel.activated,
                    toplevel.minimized,
                    self.index
                        .borrow()
                        .match_app_id(&toplevel.app_id)
                        .map(|e| e.id.clone()),
                );
                let mut list = self.toplevels.borrow_mut();
                let (was_active, was_minimized) =
                    match list.iter_mut().find(|t| t.key == toplevel.key) {
                        Some(existing) => {
                            let was = (existing.activated, existing.minimized);
                            *existing = toplevel.clone();
                            was
                        }
                        None => {
                            // A new window may belong to an application
                            // installed after the dock started.
                            if self.index.borrow().match_app_id(&toplevel.app_id).is_none() {
                                self.index.replace(AppIndex::load());
                            }
                            list.push(toplevel.clone());
                            if toplevel.app_id != PANEL_APP_ID
                                && !model::background_service(&toplevel.app_id)
                            {
                                self.touch_recent(&toplevel.app_id);
                            }
                            (false, false)
                        }
                    };
                drop(list);
                if toplevel.activated && !was_active {
                    let n = self.next();
                    self.last_active
                        .borrow_mut()
                        .insert(toplevel.key.clone(), n);
                }
                if toplevel.minimized && !was_minimized {
                    let n = self.next();
                    self.minimized_at
                        .borrow_mut()
                        .insert(toplevel.key.clone(), n);
                } else if !toplevel.minimized {
                    self.minimized_at.borrow_mut().remove(&toplevel.key);
                }
            }
            toplevels::Event::Closed(key) => {
                self.toplevels.borrow_mut().retain(|t| t.key != key);
                self.last_active.borrow_mut().remove(&key);
                self.minimized_at.borrow_mut().remove(&key);
            }
        }
        self.refresh();
    }

    /// Drop a blacklisted service left in the recents file by an older dock.
    fn drop_background_recents(&self) {
        let mut recents = self.recents.borrow_mut();
        let before = recents.len();
        recents.retain(|id| !model::background_service(id));
        if recents.len() != before {
            pins::save_recents(&recents);
        }
    }

    fn touch_recent(&self, app_id: &str) {
        if model::background_service(app_id) {
            return;
        }
        let Some(id) = self
            .index
            .borrow()
            .match_app_id(app_id)
            .filter(|entry| !model::background_service(&entry.id))
            .map(|e| e.id.clone())
        else {
            return;
        };
        let mut recents = self.recents.borrow_mut();
        if recents.first() != Some(&id) {
            model::touch_recent(&mut recents, &id, pins::MAX_RECENTS * 4);
            pins::save_recents(&recents);
        }
    }

    fn apply_settings(self: &Rc<Self>, settings: Settings) {
        let old = self.settings.replace(settings.clone());
        if old.display != settings.display {
            self.rebuild_views();
            return;
        }
        for (_, view) in self.views.borrow().iter() {
            view.set_settings(&settings);
        }
        self.refresh();
    }

    fn compute(&self) -> Sections<ObjectId> {
        let index = self.index.borrow();
        let toplevels = self.toplevels.borrow();
        let settings = self.settings.borrow();
        let minimized_at = self.minimized_at.borrow();
        let windows: Vec<model::Window<ObjectId>> = toplevels
            .iter()
            .filter(|t| t.app_id != PANEL_APP_ID)
            .map(|t| model::Window {
                key: t.key.clone(),
                app_id: &t.app_id,
                activated: t.activated,
                minimized: t.minimized,
            })
            .collect();
        let mut sections = model::sections(
            &self.pins.borrow(),
            &self.recents.borrow(),
            |id| index.get(id).is_some(),
            &windows,
            |app_id| {
                index
                    .match_app_id(app_id)
                    .filter(|e| e.shown)
                    .map(|e| e.id.clone())
            },
            &model::Options {
                recents: settings.show_recents,
                max_recents: pins::MAX_RECENTS,
                minimized_tiles: !settings.minimize_to_application,
            },
        );
        sections
            .minimized
            .sort_by_key(|k| minimized_at.get(k).copied().unwrap_or(0));
        sections
    }

    /// Recompute what the dock shows and hand it to every view.
    fn refresh(self: &Rc<Self>) {
        let sections = self.compute();
        {
            let mut launching = self.launching.borrow_mut();
            let running: Vec<&str> = sections
                .apps
                .iter()
                .chain(&sections.recents)
                .filter(|i| i.running())
                .filter_map(|i| i.desktop_id.as_deref())
                .collect();
            launching
                .retain(|id, at| !running.contains(&id.as_str()) && at.elapsed() < LAUNCH_TIMEOUT);
        }
        self.sections.replace(sections);
        let tiles = self.tiles();
        for (_, view) in self.views.borrow().iter() {
            view.set_tiles(tiles.clone());
        }
    }

    fn tiles(&self) -> Vec<Tile> {
        let sections = self.sections.borrow();
        let settings = self.settings.borrow();
        let mut tiles: Vec<Tile> = Vec::new();
        for item in &sections.apps {
            tiles.push(self.app_tile(item, Section::Apps, &settings));
        }
        if !sections.recents.is_empty() {
            tiles.push(Tile::divider(0));
            for item in &sections.recents {
                tiles.push(self.app_tile(item, Section::Recents, &settings));
            }
        }
        if let Some(about) = self.about_tile(&settings) {
            tiles.push(about);
        }
        tiles.push(Tile::divider(1));
        let folders = self.folders.borrow();
        for other in self.others.borrow().iter() {
            let Some(folder) = folders.get(&other.uri) else {
                continue;
            };
            let icon = if folder.is_dir
                && other.display == others::Display::Stack
                && !folder.preview.is_empty()
            {
                TileIcon::Stack(folder.preview.clone())
            } else {
                TileIcon::Icon(folder.icon.clone())
            };
            tiles.push(Tile {
                key: format!("{OTHER_PREFIX}{}", other.uri),
                section: Section::Others,
                icon: Some(icon),
                label: folder.name.clone(),
                indicator: false,
                badge: None,
                progress: None,
                bounce: Bounce::None,
                draggable: true,
            });
        }
        let toplevels = self.toplevels.borrow();
        for key in &sections.minimized {
            let Some(window) = toplevels.iter().find(|t| &t.key == key) else {
                continue;
            };
            let index = self.index.borrow();
            let entry = index.match_app_id(&window.app_id);
            tiles.push(Tile {
                key: window_key(key),
                section: Section::Windows,
                icon: Some(TileIcon::Window(app_icon(entry, Some(&window.app_id)))),
                label: if window.title.is_empty() {
                    entry.map(|e| e.name.clone()).unwrap_or_default()
                } else {
                    window.title.clone()
                },
                indicator: false,
                badge: None,
                progress: None,
                bounce: Bounce::None,
                draggable: false,
            });
        }
        let full = self.trash.full();
        tiles.push(Tile {
            key: TRASH_KEY.to_owned(),
            section: Section::Trash,
            icon: Some(TileIcon::Icon(
                gio::ThemedIcon::from_names(if full {
                    &["user-trash-full", "user-trash"]
                } else {
                    &["user-trash"]
                })
                .upcast(),
            )),
            label: "Trash".to_owned(),
            indicator: false,
            badge: None,
            progress: None,
            bounce: Bounce::None,
            draggable: false,
        });
        tiles
    }

    fn about_window(&self) -> Option<Toplevel> {
        self.toplevels
            .borrow()
            .iter()
            .find(|t| t.app_id == PANEL_APP_ID && t.title == ABOUT_TITLE)
            .cloned()
    }

    fn about_tile(&self, settings: &Settings) -> Option<Tile> {
        self.about_window()?;
        Some(Tile {
            key: ABOUT_KEY.to_owned(),
            section: Section::Apps,
            icon: Some(TileIcon::Icon(gio::ThemedIcon::new(PANEL_APP_ID).upcast())),
            label: "About Elavo".to_owned(),
            indicator: settings.show_indicators,
            badge: None,
            progress: None,
            bounce: Bounce::None,
            draggable: false,
        })
    }

    fn app_tile(&self, item: &Item<ObjectId>, section: Section, settings: &Settings) -> Tile {
        let entry = self.entry(item);
        let launching = item
            .desktop_id
            .as_ref()
            .is_some_and(|id| self.launching.borrow().contains_key(id));
        let badge = item
            .desktop_id
            .as_deref()
            .and_then(|id| self.badges.get(id));
        let urgent = badge.as_ref().is_some_and(|b| b.urgent) && !item.focused;
        Tile {
            key: app_key(item),
            section,
            icon: Some(TileIcon::Icon(app_icon(
                entry.as_ref(),
                item.app_id.as_deref(),
            ))),
            label: entry
                .as_ref()
                .map(|e| e.name.clone())
                .or_else(|| item.app_id.clone())
                .unwrap_or_default(),
            indicator: settings.show_indicators && (item.running() || launching),
            badge: badge.as_ref().and_then(|b| b.count_label()),
            progress: badge.as_ref().and_then(|b| b.progress),
            bounce: if launching {
                Bounce::Launch
            } else if urgent {
                Bounce::Attention
            } else {
                Bounce::None
            },
            draggable: item.desktop_id.is_some(),
        }
    }

    fn rebuild_views(self: &Rc<Self>) {
        for (window, _) in self.views.take() {
            window.destroy();
        }
        let display = gdk::Display::default().expect("no display");
        let monitors: Vec<gdk::Monitor> = (0..display.monitors().n_items())
            .filter_map(|i| display.monitors().item(i).and_downcast::<gdk::Monitor>())
            .collect();
        let settings = self.settings.borrow().clone();
        let chosen: Vec<&gdk::Monitor> = match &settings.display {
            Some(connector) => monitors
                .iter()
                .filter(|m| m.connector().as_deref() == Some(connector.as_str()))
                .collect(),
            None => Vec::new(),
        };
        let targets: Vec<&gdk::Monitor> = if chosen.is_empty() {
            monitors.iter().collect()
        } else {
            chosen
        };
        let views: Vec<_> = targets
            .into_iter()
            .map(|monitor| DockView::window(&self.app, monitor, self.weak_host(), &settings))
            .collect();
        self.views.replace(views);
        self.refresh();
    }

    fn load_folders(self: &Rc<Self>) {
        let others = self.others.borrow().clone();
        let mut folders = self.folders.borrow_mut();
        folders.retain(|uri, _| others.iter().any(|o| &o.uri == uri));
        for other in &others {
            let weak = Rc::downgrade(self);
            let uri = other.uri.clone();
            let folder = stacks::Folder::load(other, move || {
                if let Some(dock) = weak.upgrade() {
                    dock.reload_folder(&uri);
                }
            });
            folders.insert(other.uri.clone(), folder);
        }
    }

    fn reload_folder(self: &Rc<Self>, uri: &str) {
        let Some(other) = self.others.borrow().iter().find(|o| o.uri == uri).cloned() else {
            return;
        };
        if let Some(folder) = self.folders.borrow_mut().get_mut(uri) {
            folder.refresh(&other);
        }
        self.refresh();
    }

    fn entry(&self, item: &Item<ObjectId>) -> Option<AppEntry> {
        item.desktop_id
            .as_deref()
            .and_then(|id| self.index.borrow().get(id).cloned())
    }

    pub fn item(&self, key: &str) -> Option<Item<ObjectId>> {
        let sections = self.sections.borrow();
        sections
            .apps
            .iter()
            .chain(&sections.recents)
            .find(|i| app_key(i) == key)
            .cloned()
    }

    fn has_windows(&self, desktop_id: &str) -> bool {
        let sections = self.sections.borrow();
        sections
            .apps
            .iter()
            .chain(&sections.recents)
            .any(|i| i.desktop_id.as_deref() == Some(desktop_id) && i.running())
    }

    pub fn other(&self, key: &str) -> Option<Other> {
        let uri = key.strip_prefix(OTHER_PREFIX)?;
        self.others.borrow().iter().find(|o| o.uri == uri).cloned()
    }

    pub fn is_folder(&self, uri: &str) -> bool {
        self.folders.borrow().get(uri).is_some_and(|f| f.is_dir)
    }

    pub fn window(&self, key: &str) -> Option<Toplevel> {
        let id = key.strip_prefix(WINDOW_PREFIX)?;
        self.toplevels
            .borrow()
            .iter()
            .find(|t| t.key.protocol_id().to_string() == id)
            .cloned()
    }

    /// Windows of an application, least recently used first.
    pub fn windows_of(&self, item: &Item<ObjectId>) -> Vec<Toplevel> {
        let last_active = self.last_active.borrow();
        let mut windows: Vec<Toplevel> = self
            .toplevels
            .borrow()
            .iter()
            .filter(|t| item.windows.contains(&t.key))
            .cloned()
            .collect();
        windows.sort_by_key(|t| last_active.get(&t.key).copied().unwrap_or(0));
        windows
    }

    pub fn controller(&self) -> Option<&Controller> {
        self.controller.as_ref()
    }

    /// As on a Mac: an application that is not running launches. One that
    /// is brings all its windows forward, the most recently used on top. If
    /// every window is minimized, the most recent one comes back.
    fn activate_app(self: &Rc<Self>, item: &Item<ObjectId>) {
        let windows = self.windows_of(item);
        let Some(controller) = self.controller.as_ref().filter(|_| !windows.is_empty()) else {
            self.launch(item, None);
            return;
        };
        let visible: Vec<&Toplevel> = windows.iter().filter(|t| !t.minimized).collect();
        if visible.is_empty() {
            let restore = windows
                .iter()
                .max_by_key(|t| self.minimized_at.borrow().get(&t.key).copied().unwrap_or(0));
            if let Some(window) = restore {
                controller.activate(window);
            }
            return;
        }
        for window in visible {
            controller.activate(window);
        }
    }

    pub fn launch(self: &Rc<Self>, item: &Item<ObjectId>, action: Option<&str>) {
        let Some(entry) = self.entry(item) else {
            return;
        };
        if action.is_none() && !item.running() {
            if self.launching.borrow().contains_key(&entry.id) {
                return;
            }
            self.launching
                .borrow_mut()
                .insert(entry.id.clone(), Instant::now());
            let weak = Rc::downgrade(self);
            glib::timeout_add_local_once(LAUNCH_TIMEOUT, move || {
                if let Some(dock) = weak.upgrade() {
                    dock.refresh();
                }
            });
            self.refresh();
        }
        elavo_common::launch(&entry.info, action);
    }

    /// Super+N: the Nth application, pins first.
    fn activate_slot(self: &Rc<Self>, slot: usize) {
        let item = {
            let sections = self.sections.borrow();
            sections
                .apps
                .iter()
                .chain(&sections.recents)
                .nth(slot)
                .cloned()
        };
        if let Some(item) = item {
            self.activate_app(&item);
        }
    }

    pub fn set_pinned(self: &Rc<Self>, id: &str, pinned: bool) {
        {
            let mut pins = self.pins.borrow_mut();
            let present = pins.iter().any(|p| p == id);
            if pinned && !present {
                pins.push(id.to_owned());
            } else if !pinned && present {
                pins.retain(|p| p != id);
            } else {
                return;
            }
            pins::save(&pins);
        }
        self.refresh();
    }

    pub fn forget_recent(self: &Rc<Self>, id: &str) {
        {
            let mut recents = self.recents.borrow_mut();
            recents.retain(|r| r != id);
            pins::save_recents(&recents);
        }
        self.refresh();
    }

    /// Pin `ids` before visible application slot `slot`.
    fn pin_at(self: &Rc<Self>, ids: &[String], slot: usize, exclude: Option<&str>) {
        {
            let mut pins = self.pins.borrow_mut();
            let visible: Vec<String> = self
                .sections
                .borrow()
                .apps
                .iter()
                .filter(|i| i.pinned)
                .filter_map(|i| i.desktop_id.clone())
                .filter(|id| Some(id.as_str()) != exclude)
                .collect();
            let mut to = model::pin_index(&pins, &visible, slot);
            for id in ids {
                model::place_pin(&mut pins, id, to);
                to = pins.iter().position(|p| p == id).map_or(to, |at| at + 1);
            }
            pins::save(&pins);
        }
        self.refresh();
    }

    pub fn update_other(self: &Rc<Self>, uri: &str, f: impl FnOnce(&mut Other)) {
        {
            let mut items = self.others.borrow_mut();
            let Some(item) = items.iter_mut().find(|o| o.uri == uri) else {
                return;
            };
            f(item);
            others::save(&items);
        }
        self.reload_folder(uri);
    }

    pub fn remove_other(self: &Rc<Self>, uri: &str) {
        {
            let mut items = self.others.borrow_mut();
            items.retain(|o| o.uri != uri);
            others::save(&items);
        }
        self.load_folders();
        self.refresh();
    }

    fn place_others(self: &Rc<Self>, uris: &[String], slot: usize) {
        {
            let mut items = self.others.borrow_mut();
            let mut slot = slot;
            for uri in uris {
                let existing = items.iter().position(|o| &o.uri == uri);
                let item = match existing {
                    Some(from) => {
                        if from < slot {
                            slot -= 1;
                        }
                        items.remove(from)
                    }
                    None => Other::new(uri.clone()),
                };
                let at = slot.min(items.len());
                items.insert(at, item);
                slot += 1;
            }
            others::save(&items);
        }
        self.load_folders();
        self.refresh();
    }

    /// System Settings' Desktop & Dock pane when it is installed, or the
    /// dock's own settings window.
    pub fn open_settings(&self) {
        if gio_unix::DesktopAppInfo::new("io.elavo.SystemSettings.desktop").is_some() {
            let argv = ["elavo-system-settings", "--pane", "dock"].map(std::ffi::OsStr::new);
            match gio::Subprocess::newv(&argv, gio::SubprocessFlags::NONE) {
                Ok(_) => return,
                Err(err) => glib::g_warning!("elavo-dock", "opening System Settings: {err}"),
            }
        }
        if let Some(window) = self.settings_window.upgrade() {
            window.present();
            return;
        }
        let window = elavo_dock_settings::window(&self.app);
        self.settings_window.set(Some(&window));
        window.present();
    }

    fn desktop_id_for(&self, file: &gio::File) -> Option<String> {
        let path = file.path()?;
        let desktop = if path.extension().is_some_and(|e| e == "eapp") {
            path.join("launch.desktop")
        } else if path.extension().is_some_and(|e| e == "desktop") {
            path.clone()
        } else {
            return None;
        };
        let index = self.index.borrow();
        if let Some(entry) = index
            .shown()
            .find(|e| e.info.filename().as_deref() == Some(desktop.as_path()))
        {
            return Some(entry.id.clone());
        }
        let info = gio_unix::DesktopAppInfo::from_filename(&desktop)?;
        let elavo_id = info.string("X-Elavo-Id").map(|s| format!("{s}.desktop"));
        let basename = desktop.file_name()?.to_string_lossy().into_owned();
        [elavo_id, Some(basename)]
            .into_iter()
            .flatten()
            .find(|id| index.get(id).is_some())
    }
}

pub fn app_key(item: &Item<ObjectId>) -> String {
    format!("{APP_PREFIX}{}", item.key())
}

fn window_key(key: &ObjectId) -> String {
    format!("{WINDOW_PREFIX}{}", key.protocol_id())
}

fn app_icon(entry: Option<&AppEntry>, app_id: Option<&str>) -> gio::Icon {
    if let Some(icon) = entry.and_then(|e| e.icon()) {
        return icon;
    }
    // Not a fallback list: GTK tries every name in the current theme before
    // the next one, so Adwaita's generic icon would beat an application's
    // own icon in hicolor.
    let theme = gdk::Display::default().map(|d| gtk::IconTheme::for_display(&d));
    let name = app_id
        .filter(|id| theme.is_some_and(|t| t.has_icon(id)))
        .unwrap_or("application-x-executable");
    gio::ThemedIcon::new(name).upcast()
}

/// What the views hold: a weak handle, so a view never keeps the dock alive.
struct HostRef(Weak<Dock>);

impl HostRef {
    fn rc(&self) -> Option<Rc<Dock>> {
        self.0.upgrade()
    }
}

impl Host for HostRef {
    fn activate(&self, view: &DockView, key: &str) {
        let Some(dock) = self.rc() else {
            return;
        };
        if key == ABOUT_KEY {
            let window = dock.about_window();
            if let Some(window) = window {
                if let Some(controller) = dock.controller() {
                    controller.activate(&window);
                }
            }
        } else if let Some(item) = dock.item(key) {
            dock.activate_app(&item);
        } else if let Some(other) = dock.other(key) {
            if dock.is_folder(&other.uri) {
                stacks::show(&dock, view, key, &other);
            } else {
                files::open_uri(&other.uri);
            }
        } else if let Some(window) = dock.window(key) {
            if let Some(controller) = dock.controller() {
                controller.activate(&window);
            }
        } else if key == TRASH_KEY {
            files::open_uri("trash:///");
        }
    }

    fn menu(&self, view: &DockView, key: &str) {
        if let Some(dock) = self.rc() {
            menus::show(&dock, view, key);
        }
    }

    fn drag_begin(&self, key: &str) {
        if let Some(dock) = self.rc() {
            dock.dragging.replace(Some(key.to_owned()));
        }
    }

    fn drag_removed(&self, key: &str) {
        let Some(dock) = self.rc() else {
            return;
        };
        if let Some(item) = dock.item(key) {
            let Some(id) = item.desktop_id.as_deref() else {
                return;
            };
            if item.pinned {
                dock.set_pinned(id, false);
            } else if item.recent && !item.running() {
                dock.forget_recent(id);
            }
        } else if let Some(other) = dock.other(key) {
            dock.remove_other(&other.uri);
        }
    }

    fn drag_end(&self) {
        if let Some(dock) = self.rc() {
            dock.dragging.replace(None);
            dock.refresh();
        }
    }

    fn dragging(&self) -> Option<String> {
        self.rc().and_then(|dock| dock.dragging.borrow().clone())
    }

    fn classify(&self, files: &[gio::File]) -> Payload {
        let Some(dock) = self.rc().filter(|_| !files.is_empty()) else {
            return Payload::Files(Vec::new());
        };
        let ids: Vec<String> = files
            .iter()
            .filter_map(|f| dock.desktop_id_for(f))
            .collect();
        if ids.len() == files.len() {
            Payload::Apps(ids, files.to_vec())
        } else {
            Payload::Files(files.to_vec())
        }
    }

    fn can_open(&self, key: &str, files: &[gio::File]) -> bool {
        let Some(dock) = self.rc().filter(|_| !files.is_empty()) else {
            return false;
        };
        if let Some(other) = dock.other(key) {
            // A folder takes files that are not already in it and are not
            // the folder or one of its parents.
            let folder = gio::File::for_uri(&other.uri);
            return dock.is_folder(&other.uri)
                && files.iter().all(|f| {
                    !f.equal(&folder)
                        && !folder.has_prefix(f)
                        && f.parent().is_none_or(|p| !p.equal(&folder))
                });
        }
        let Some(entry) = dock.item(key).and_then(|i| dock.entry(&i)) else {
            return false;
        };
        files::app_opens(&entry.info, files)
    }

    fn accept_drop(&self, intent: Intent, payload: Payload) -> bool {
        let Some(dock) = self.rc() else {
            return false;
        };
        match (intent, payload) {
            (Intent::MoveApp { key, slot }, Payload::Internal) => {
                let Some(id) = dock.item(&key).and_then(|i| i.desktop_id) else {
                    return false;
                };
                dock.pin_at(std::slice::from_ref(&id), slot, Some(&id));
                true
            }
            (Intent::MoveOther { key, slot }, Payload::Internal) => {
                let Some(other) = dock.other(&key) else {
                    return false;
                };
                dock.place_others(&[other.uri], slot);
                true
            }
            (Intent::Pin { slot }, Payload::Apps(ids, _)) => {
                dock.pin_at(&ids, slot, None);
                true
            }
            // An application that is open cannot be deleted; the
            // Applications service says why.
            (Intent::Trash, Payload::Apps(ids, files)) => {
                if let Some(open) = ids.iter().find(|id| dock.has_windows(id)) {
                    let name = dock
                        .index
                        .borrow()
                        .get(open)
                        .map(|e| e.name.clone())
                        .unwrap_or_else(|| open.trim_end_matches(".desktop").to_owned());
                    files::tell_open(&name);
                    return false;
                }
                files::trash(files);
                true
            }
            (Intent::AddOther { slot }, Payload::Files(files)) => {
                let uris: Vec<String> = files.iter().map(|f| f.uri().to_string()).collect();
                dock.place_others(&uris, slot);
                true
            }
            (Intent::OpenWith(key), Payload::Files(files)) => {
                let Some(entry) = dock.item(&key).and_then(|i| dock.entry(&i)) else {
                    return false;
                };
                files::open_with(&entry.info, &files);
                true
            }
            (Intent::IntoFolder(key), Payload::Files(files)) => {
                let Some(other) = dock.other(&key) else {
                    return false;
                };
                files::move_into(files, gio::File::for_uri(&other.uri));
                true
            }
            (Intent::Trash, Payload::Files(files)) => {
                files::trash(files);
                true
            }
            _ => false,
        }
    }

    fn resize(&self, size: i32) {
        Settings::update(|s| s.icon_size = size);
    }
}

struct Options {
    activate: Option<usize>,
    settings: bool,
}

fn parse_args(args: &[std::ffi::OsString]) -> Options {
    let args: Vec<String> = args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let value = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .and_then(|v| v.parse::<i64>().ok())
    };
    Options {
        activate: value("--activate").and_then(|n| usize::try_from(n - 1).ok()),
        settings: args.iter().any(|a| a == "--settings"),
    }
}

/// `org.gnome.desktop.interface` `color-scheme`. Anything else is light,
/// including an empty value and `prefer-light`.
fn scheme_is_dark(value: &str) -> bool {
    value == "prefer-dark"
}

/// Load the dark theme sheet while the color scheme is dark, and switch
/// when it changes. The settings object is kept so the signal stays connected.
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
            let Some(settings) = gtk::Settings::default() else {
                return;
            };
            settings.set_gtk_application_prefer_dark_theme(scheme_is_dark(
                desktop.string("color-scheme").as_str(),
            ));
        }
    };
    apply();
    desktop.connect_changed(Some("color-scheme"), move |_, _| apply());
    thread_local! {
        static HELD: RefCell<Option<gio::Settings>> = const { RefCell::new(None) };
    }
    HELD.with(|slot| slot.replace(Some(desktop)));
}

#[cfg(test)]
mod tests {
    use super::scheme_is_dark;

    #[test]
    fn only_prefer_dark_selects_the_dark_sheet() {
        assert!(scheme_is_dark("prefer-dark"));
        assert!(!scheme_is_dark("prefer-light"));
        assert!(!scheme_is_dark("default"));
        assert!(!scheme_is_dark(""));
    }
}

fn main() -> glib::ExitCode {
    let app = gtk::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();

    let dock: Rc<OnceCell<Rc<Dock>>> = Rc::default();
    app.connect_command_line(move |app, cmdline| {
        let options = parse_args(&cmdline.arguments());
        let dock = dock.get_or_init(|| {
            std::mem::forget(app.hold());
            Dock::new(app)
        });
        if let Some(slot) = options.activate {
            dock.activate_slot(slot);
        }
        if options.settings {
            dock.open_settings();
        }
        glib::ExitCode::SUCCESS
    });
    app.run()
}
