//! The global menu for the focused application: a menu named after it (New
//! Window, Hide, Quit), the application's own menus when it exports them,
//! and a Window menu. The panel's own menus act on windows over
//! foreign-toplevel, so every application gets them.
//!
//! labwc implements neither `gtk_shell1` nor `org_kde_kwin_appmenu`, so no
//! window says where its menus are. Two sources are tried for the focused
//! window's `app_id`: a dbusmenu registered with the panel's AppMenu
//! Registrar by a matching process (Qt), then a GtkApplication that owns the
//! ID on the session bus and exports its menubar under the object path
//! derived from it.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;
use std::time::Duration;

use elavo_common::toplevels::{self, Controller, ObjectId, Toplevel};
use elavo_common::AppIndex;
use gtk::prelude::*;
use gtk::{gio, glib};

use crate::dbusmenu::DbusMenu;
use crate::focus::Focus;
use crate::folio;
use crate::registrar::Registrar;

const DBUS_TIMEOUT_MS: i32 = 1000;
const MAX_TITLE_CHARS: usize = 60;
/// Switching windows deactivates one and activates the other in separate
/// events. Waiting this long keeps the bar from showing an empty region in
/// between.
const SETTLE: Duration = Duration::from_millis(30);

#[derive(Clone)]
struct Menus {
    model: gio::MenuModel,
    app: gio::ActionGroup,
    win: Option<gio::ActionGroup>,
    /// The application has its own Window menu, and the panel's items are a
    /// section at its end.
    merges_window: bool,
    /// Keeps a dbusmenu source following the application's changes.
    _dbusmenu: Option<Rc<DbusMenu>>,
}

struct Shown {
    /// The desktop, Nautilus, or a Folio window: Folio's own menus.
    folio: bool,
    /// A Nautilus window is the focused one, so items that act on it are on.
    nautilus_focused: bool,
    app_id: String,
    windows: usize,
    name: String,
    desktop_id: Option<String>,
    menus: Option<Menus>,
}

pub struct AppMenu {
    regions: RefCell<Vec<glib::WeakRef<gtk::Box>>>,
    focus: RefCell<Focus<ObjectId>>,
    /// In the order windows first appeared.
    toplevels: RefCell<Vec<Toplevel>>,
    controller: RefCell<Option<Controller>>,
    index: RefCell<AppIndex>,
    misses: RefCell<HashSet<String>>,
    registrar: RefCell<Option<Rc<Registrar>>>,
    shown: RefCell<Option<Shown>>,
    /// `elavo.*`: the panel's own items, kept apart from the application's
    /// `app.*` and `win.*`.
    actions: gio::SimpleActionGroup,
    folio: Rc<folio::Folio>,
    /// The focused application's windows, at the end of the Window menu.
    window_list: gio::Menu,
    window_menu: gio::Menu,
    listed: RefCell<Vec<(String, String)>>,
    generation: Cell<u64>,
    queued: Cell<bool>,
    /// The desktop has the keyboard. It counts as no window focused.
    desktop_focused: Cell<bool>,
}

impl AppMenu {
    pub fn new() -> Rc<Self> {
        let window_list = gio::Menu::new();
        let folio = folio::Folio::new(&window_list);
        let menu = Rc::new(Self {
            regions: RefCell::default(),
            focus: RefCell::default(),
            toplevels: RefCell::default(),
            controller: RefCell::default(),
            index: RefCell::new(AppIndex::load()),
            misses: RefCell::default(),
            registrar: RefCell::default(),
            shown: RefCell::default(),
            actions: gio::SimpleActionGroup::new(),
            folio,
            window_menu: window_menu(&window_list),
            window_list,
            listed: RefCell::default(),
            generation: Cell::new(0),
            queued: Cell::new(false),
            desktop_focused: Cell::new(false),
        });
        {
            let weak = Rc::downgrade(&menu);
            menu.folio.set_wm_handler(move |wm| {
                if let Some(menu) = weak.upgrade() {
                    menu.folio_wm(wm);
                }
            });
            let weak = Rc::downgrade(&menu);
            menu.folio.set_desktop_focus_handler(move |on| {
                if let Some(menu) = weak.upgrade() {
                    if menu.desktop_focused.replace(on) != on {
                        menu.refresh_shown(false);
                    }
                }
            });
        }
        menu.install_actions();
        {
            let weak = Rc::downgrade(&menu);
            let registrar = Registrar::start(move || {
                if let Some(menu) = weak.upgrade() {
                    menu.refresh_shown(true);
                }
            });
            menu.registrar.replace(Some(registrar));
        }
        match toplevels::start() {
            Ok((controller, events)) => {
                menu.controller.replace(Some(controller));
                let weak = Rc::downgrade(&menu);
                glib::spawn_future_local(async move {
                    while let Ok(event) = events.recv().await {
                        let Some(menu) = weak.upgrade() else {
                            break;
                        };
                        menu.on_event(event);
                    }
                });
            }
            Err(err) => {
                glib::g_warning!("elavo-panel", "no window list, so no global menu: {err}");
            }
        }
        // Folio is showing before the first window event arrives.
        menu.update();
        menu
    }

    fn install_actions(self: &Rc<Self>) {
        let add = |name: &str, f: fn(&AppMenu)| {
            let action = gio::SimpleAction::new(name, None);
            let weak = Rc::downgrade(self);
            action.connect_activate(move |_, _| {
                if let Some(menu) = weak.upgrade() {
                    f(&menu);
                }
            });
            self.actions.add_action(&action);
        };
        add("new-window", AppMenu::new_window);
        add("hide", |m| m.each_window(Controller::minimize));
        add("hide-others", AppMenu::hide_others);
        add("show-all", AppMenu::show_all);
        add("quit", |m| m.each_window(Controller::close));
        add("bring-all-to-front", AppMenu::bring_all_to_front);
        add("minimize", |m| m.focused_window(Controller::minimize));
        add("zoom", |m| m.focused_window(Controller::toggle_maximized));
        add("close", |m| m.focused_window(Controller::close));

        let window = gio::SimpleAction::new_stateful(
            "window",
            Some(glib::VariantTy::STRING),
            &"".to_variant(),
        );
        let weak = Rc::downgrade(self);
        window.connect_activate(move |_, target| {
            let (Some(menu), Some(id)) = (weak.upgrade(), target.and_then(|t| t.str())) else {
                return;
            };
            let toplevel = menu
                .toplevels
                .borrow()
                .iter()
                .find(|t| window_id(t) == id)
                .cloned();
            let controller = menu.controller.borrow();
            if let (Some(toplevel), Some(controller)) = (toplevel, controller.as_ref()) {
                controller.activate(&toplevel);
            }
        });
        self.actions.add_action(&window);
    }

    /// The focused application's windows, in the order they appeared.
    /// Folio's windows are Nautilus's, including when the desktop is focused.
    fn app_windows(&self) -> Vec<Toplevel> {
        let focused = self.focus.borrow().focused().map(str::to_owned);
        if folio::is_folio(focused.as_deref()) {
            return self.nautilus_windows();
        }
        let app_id = focused.unwrap_or_default();
        self.toplevels
            .borrow()
            .iter()
            .filter(|t| t.app_id == app_id)
            .cloned()
            .collect()
    }

    fn nautilus_windows(&self) -> Vec<Toplevel> {
        self.toplevels
            .borrow()
            .iter()
            .filter(|t| t.app_id == folio::NAUTILUS)
            .cloned()
            .collect()
    }

    fn folio_wm(&self, wm: folio::Wm) {
        match wm {
            folio::Wm::HideFolio => self.each_window(Controller::minimize),
            folio::Wm::HideOthers => self.minimize_unless(folio::is_folio_window),
            folio::Wm::ShowAll => self.show_all(),
            folio::Wm::Close => self.focused_window(Controller::close),
            folio::Wm::Minimize => self.focused_window(Controller::minimize),
            folio::Wm::Maximize => self.focused_window(Controller::toggle_maximized),
            folio::Wm::Fullscreen => self.focused_window(Controller::toggle_fullscreen),
            folio::Wm::Cycle => self.cycle_nautilus(),
            folio::Wm::BringAll => self.bring_all_to_front(),
        }
    }

    fn hide_others(&self) {
        let Some(app_id) = self.focus.borrow().focused().map(str::to_owned) else {
            return;
        };
        self.minimize_unless(|id| id == app_id);
    }

    fn minimize_unless(&self, keep: impl Fn(&str) -> bool) {
        let windows: Vec<Toplevel> = self
            .toplevels
            .borrow()
            .iter()
            .filter(|t| !keep(&t.app_id))
            .cloned()
            .collect();
        if let Some(controller) = self.controller.borrow().as_ref() {
            for window in &windows {
                controller.minimize(window);
            }
        }
    }

    fn show_all(&self) {
        let windows: Vec<Toplevel> = self
            .toplevels
            .borrow()
            .iter()
            .filter(|t| t.minimized)
            .cloned()
            .collect();
        if let Some(controller) = self.controller.borrow().as_ref() {
            for window in &windows {
                controller.restore(window);
            }
        }
    }

    fn cycle_nautilus(&self) {
        let active = self.focus.borrow().active().cloned();
        let windows = self.nautilus_windows();
        if windows.len() < 2 {
            return;
        }
        let index = windows
            .iter()
            .position(|t| Some(&t.key) == active.as_ref())
            .unwrap_or(0);
        let next = windows[(index + 1) % windows.len()].clone();
        if let Some(controller) = self.controller.borrow().as_ref() {
            controller.activate(&next);
        }
    }

    fn each_window(&self, f: fn(&Controller, &Toplevel)) {
        if let Some(controller) = &*self.controller.borrow() {
            for toplevel in self.app_windows() {
                f(controller, &toplevel);
            }
        }
    }

    fn focused_window(&self, f: fn(&Controller, &Toplevel)) {
        let Some(key) = self.focus.borrow().active().cloned() else {
            return;
        };
        let toplevel = self
            .toplevels
            .borrow()
            .iter()
            .find(|t| t.key == key)
            .cloned();
        if let (Some(toplevel), Some(controller)) = (toplevel, &*self.controller.borrow()) {
            f(controller, &toplevel);
        }
    }

    /// Raise every window of the application, the focused one last so it
    /// stays on top and keeps focus.
    fn bring_all_to_front(&self) {
        let Some(controller) = &*self.controller.borrow() else {
            return;
        };
        let active = self.focus.borrow().active().cloned();
        let (focused, others): (Vec<_>, Vec<_>) = self
            .app_windows()
            .into_iter()
            .partition(|t| Some(&t.key) == active.as_ref());
        for toplevel in others.iter().chain(&focused) {
            controller.activate(toplevel);
        }
    }

    fn new_window(&self) {
        let Some(id) = self
            .shown
            .borrow()
            .as_ref()
            .and_then(|s| s.desktop_id.clone())
        else {
            return;
        };
        let index = self.index.borrow();
        let Some(entry) = index.get(&id) else {
            return;
        };
        let action = entry.has_action("new-window").then_some("new-window");
        elavo_common::launch(&entry.info, action);
    }

    /// Region 2 for one bar.
    pub fn widget(&self) -> gtk::Box {
        let region = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .css_classes(["elavo-panel-appregion"])
            .build();
        self.fill(&region);
        let mut regions = self.regions.borrow_mut();
        regions.retain(|r| r.upgrade().is_some());
        regions.push(region.downgrade());
        region
    }

    fn on_event(self: &Rc<Self>, event: toplevels::Event) {
        match event {
            toplevels::Event::Changed(toplevel) => {
                self.focus.borrow_mut().changed(
                    toplevel.key.clone(),
                    &toplevel.app_id,
                    toplevel.activated,
                );
                let mut list = self.toplevels.borrow_mut();
                match list.iter_mut().find(|t| t.key == toplevel.key) {
                    Some(existing) => *existing = toplevel,
                    None => list.push(toplevel),
                }
            }
            toplevels::Event::Closed(key) => {
                self.focus.borrow_mut().closed(&key);
                self.toplevels.borrow_mut().retain(|t| t.key != key);
            }
        }
        if !self.queued.replace(true) {
            let weak = Rc::downgrade(self);
            glib::timeout_add_local_once(SETTLE, move || {
                if let Some(menu) = weak.upgrade() {
                    menu.queued.set(false);
                    menu.update();
                }
            });
        }
    }

    /// Refill the window list in place, so an open Window menu follows title
    /// changes without the bar being rebuilt.
    fn sync_windows(&self) {
        let fallback = self
            .shown
            .borrow()
            .as_ref()
            .map(|s| s.name.clone())
            .unwrap_or_default();
        let listed: Vec<(String, String)> = self
            .app_windows()
            .iter()
            .map(|t| {
                let title = if t.title.is_empty() {
                    &fallback
                } else {
                    &t.title
                };
                (window_id(t), menu_label(&shorten(title, MAX_TITLE_CHARS)))
            })
            .collect();
        if *self.listed.borrow() != listed {
            self.window_list.remove_all();
            for (id, title) in &listed {
                let item = gio::MenuItem::new(Some(title), None);
                item.set_action_and_target_value(Some("elavo.window"), Some(&id.to_variant()));
                self.window_list.append_item(&item);
            }
            self.listed.replace(listed);
        }
        let active = self
            .focus
            .borrow()
            .active()
            .map(|k| k.protocol_id().to_string())
            .unwrap_or_default();
        self.actions
            .change_action_state("window", &active.to_variant());
    }

    fn update(self: &Rc<Self>) {
        self.refresh_shown(false);
        self.sync_windows();
    }

    /// `force` probes the menus again for the same application, after a
    /// registration with the Registrar came or went.
    fn refresh_shown(self: &Rc<Self>, force: bool) {
        let (focused, nautilus_windows) = {
            let focus = self.focus.borrow();
            let focused = if self.desktop_focused.get() {
                None
            } else {
                focus.focused().map(str::to_owned)
            };
            let nautilus_windows = focus.windows_of(folio::NAUTILUS);
            (focused, nautilus_windows)
        };
        if folio::is_folio(focused.as_deref()) {
            let nautilus_focused = focused.as_deref() == Some(folio::NAUTILUS);
            self.folio.set_context(nautilus_focused, nautilus_windows);
            let same = self.shown.borrow().as_ref().is_some_and(|shown| {
                shown.folio
                    && shown.nautilus_focused == nautilus_focused
                    && shown.windows == nautilus_windows
            });
            if same {
                return;
            }
            // Drop a menu probe that was in flight for the previous application.
            self.generation.set(self.generation.get() + 1);
            self.shown.replace(Some(Shown {
                folio: true,
                nautilus_focused,
                app_id: String::new(),
                windows: nautilus_windows,
                name: "Folio".to_owned(),
                desktop_id: None,
                menus: None,
            }));
            self.redraw();
            return;
        }
        let app_id = focused.unwrap_or_default();
        let windows = self.focus.borrow().windows_of(&app_id);
        match self.shown.borrow().as_ref() {
            Some(shown)
                if !force && !shown.folio && shown.app_id == app_id && shown.windows == windows =>
            {
                return;
            }
            _ => {}
        }
        let generation = self.generation.get() + 1;
        self.generation.set(generation);

        let (name, desktop_id) = self.lookup(&app_id);
        let desktop_stem = desktop_id
            .as_deref()
            .map(|id| id.strip_suffix(".desktop").unwrap_or(id).to_owned());
        let registered = {
            let wm_class = desktop_id
                .as_deref()
                .and_then(|id| self.index.borrow().get(id).and_then(|e| e.wm_class.clone()));
            let wanted = process_names_for(&app_id, desktop_stem.as_deref(), wm_class.as_deref());
            self.registrar
                .borrow()
                .as_ref()
                .and_then(|registrar| registrar.find(&wanted))
        };
        // Keep the menus of the same application while the new window count
        // is probed, so a second window does not make them blink.
        let previous = self
            .shown
            .take()
            .filter(|s| s.app_id == app_id)
            .and_then(|s| s.menus);
        self.shown.replace(Some(Shown {
            folio: false,
            nautilus_focused: false,
            app_id: app_id.clone(),
            windows,
            name,
            desktop_id,
            menus: previous,
        }));
        self.redraw();

        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let mut menus = None;
            if let Some((sender, path)) = registered {
                let window_menu = weak.upgrade().map(|m| m.window_menu.clone());
                if let Some(window_menu) = window_menu {
                    menus = DbusMenu::load(&sender, &path, window_menu.upcast())
                        .await
                        .map(|source| Menus {
                            model: source.model(),
                            app: source.actions(),
                            win: None,
                            merges_window: source.has_window_menu(),
                            _dbusmenu: Some(source),
                        });
                }
            }
            if menus.is_none() {
                menus = find_menus(&app_id, desktop_stem.as_deref(), windows).await;
            }
            let Some(menu) = weak.upgrade() else {
                return;
            };
            if menu.generation.get() != generation {
                return;
            }
            if let Some(shown) = menu.shown.borrow_mut().as_mut() {
                shown.menus = menus;
            }
            menu.redraw();
        });
    }

    /// The display name and the desktop ID.
    fn lookup(&self, app_id: &str) -> (String, Option<String>) {
        if self.index.borrow().match_app_id(app_id).is_none()
            && self.misses.borrow_mut().insert(app_id.to_owned())
        {
            // An application installed after the panel started.
            self.index.replace(AppIndex::load());
        }
        match self.index.borrow().match_app_id(app_id) {
            Some(entry) => (entry.name.clone(), Some(entry.id.clone())),
            None => (fallback_name(app_id), None),
        }
    }

    fn redraw(&self) {
        for region in self.regions.borrow().iter().filter_map(|r| r.upgrade()) {
            self.fill(&region);
        }
    }

    fn fill(&self, region: &gtk::Box) {
        while let Some(child) = region.first_child() {
            region.remove(&child);
        }
        let shown = self.shown.borrow();
        let Some(shown) = shown.as_ref() else {
            return;
        };
        let bar = if shown.folio {
            let bar = gtk::PopoverMenuBar::from_model(Some(self.folio.model()));
            bar.insert_action_group("folio", Some(self.folio.actions()));
            // Refresh which items can run as the menu opens. The reply lands
            // after the popover, and the items follow the action's enabled flag.
            let folio = Rc::clone(&self.folio);
            let click = gtk::GestureClick::new();
            click.connect_pressed(move |_, _, _, _| folio.query_files());
            bar.add_controller(click);
            bar
        } else {
            gtk::PopoverMenuBar::from_model(Some(&self.model(shown)))
        };
        bar.add_css_class("elavo-panel-appmenu");
        // The window list's items are `elavo.window` for both menus.
        bar.insert_action_group("elavo", Some(&self.actions));
        if let Some(menus) = &shown.menus {
            bar.insert_action_group("app", Some(&menus.app));
            bar.insert_action_group("win", menus.win.as_ref());
            prepare_as_menus_arrive(&bar, &menus.model);
        }
        region.append(&bar);
        crate::dropdown::watch_menu_bar(&bar);
        crate::dropdown::prepare_tree(bar.upcast_ref());
    }

    /// The application menu, then the application's own menus, then Window.
    /// A menubar merges top-level sections, so the exported menubar is a
    /// section here.
    fn model(&self, shown: &Shown) -> gio::Menu {
        let name = menu_label(&shown.name);
        let app = gio::Menu::new();
        if shown.desktop_id.is_some() {
            app.append_section(None, &section(&[("New Window", "elavo.new-window")]));
        }
        app.append_section(
            None,
            &section(&[
                (&format!("Hide {name}"), "elavo.hide"),
                ("Hide Others", "elavo.hide-others"),
                ("Show All", "elavo.show-all"),
            ]),
        );
        app.append_section(None, &section(&[(&format!("Quit {name}"), "elavo.quit")]));

        let bar = gio::Menu::new();
        bar.append_submenu(Some(&name), &app);
        if let Some(menus) = &shown.menus {
            bar.append_section(None, &menus.model);
        }
        // An application's own Window menu already ends with these items.
        if !shown.menus.as_ref().is_some_and(|m| m.merges_window) {
            bar.append_submenu(Some("Window"), &self.window_menu);
        }
        bar
    }
}

/// An application's exported menus fill in over D-Bus after the bar is
/// built, and the bar makes a menu for each top item as it arrives. Those
/// menus are styled, placed, and blurred like the rest once they exist.
fn prepare_as_menus_arrive(bar: &gtk::PopoverMenuBar, model: &gio::MenuModel) {
    let weak = bar.downgrade();
    let handler = model.connect_items_changed(move |_, _, _, _| {
        let weak = weak.clone();
        // The bar adds its item for the change in its own handler.
        if let Some(bar) = weak.upgrade() {
            // Guard the new titles before the next pointer crossing. The
            // idle below only styles menus that exist by then.
            crate::dropdown::watch_menu_bar(&bar);
        }
        glib::idle_add_local_once(move || {
            if let Some(bar) = weak.upgrade() {
                crate::dropdown::prepare_tree(bar.upcast_ref());
            }
        });
    });
    let model = model.clone();
    let handler = RefCell::new(Some(handler));
    bar.connect_destroy(move |_| {
        if let Some(handler) = handler.take() {
            model.disconnect(handler);
        }
    });
}

fn section(items: &[(&str, &str)]) -> gio::Menu {
    let menu = gio::Menu::new();
    for (label, action) in items {
        menu.append(Some(label), Some(action));
    }
    menu
}

/// The panel's Window items, acting on windows over foreign-toplevel.
fn window_menu(window_list: &gio::Menu) -> gio::Menu {
    let window = gio::Menu::new();
    window.append_section(
        None,
        &section(&[
            ("Minimize", "elavo.minimize"),
            ("Zoom", "elavo.zoom"),
            ("Close Window", "elavo.close"),
        ]),
    );
    window.append_section(
        None,
        &section(&[("Bring All to Front", "elavo.bring-all-to-front")]),
    );
    window.append_section(None, window_list);
    window
}

fn window_id(toplevel: &Toplevel) -> String {
    toplevel.key.protocol_id().to_string()
}

/// A name for an `app_id` with no desktop file: `cursor` is Cursor and
/// `io.elavo.MenuTest` is MenuTest.
fn fallback_name(app_id: &str) -> String {
    let last = app_id.rsplit('.').next().unwrap_or(app_id);
    let mut chars = last.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => app_id.to_owned(),
    }
}

/// Menu labels take `_` as a mnemonic marker.
fn menu_label(text: &str) -> String {
    text.replace('_', "__")
}

fn shorten(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut short: String = text.chars().take(max - 1).collect();
    short.push('…');
    short
}

/// GTK's object path for an application ID: `org.gnome.Text-Editor` is
/// exported at `/org/gnome/Text_Editor`.
pub fn object_path(app_id: &str) -> Option<String> {
    if app_id.is_empty() {
        return None;
    }
    let path = format!("/{}", app_id.replace('.', "/").replace('-', "_"));
    glib::Variant::is_object_path(&path).then_some(path)
}

async fn find_menus(app_id: &str, desktop_stem: Option<&str>, windows: usize) -> Option<Menus> {
    let bus = gio::bus_get_future(gio::BusType::Session).await.ok()?;
    let mut name = None;
    for candidate in std::iter::once(app_id).chain(desktop_stem.filter(|d| *d != app_id)) {
        if gio::dbus_is_name(candidate)
            && !candidate.starts_with(':')
            && has_owner(&bus, candidate).await
        {
            name = Some(candidate);
            break;
        }
    }
    let name = name?;
    let path = object_path(name)?;
    let model = gio::DBusMenuModel::get(&bus, Some(name), &format!("{path}/menus/menubar"));
    let app = gio::DBusActionGroup::get(&bus, Some(name), &path);
    // Without gtk_shell1 there is no telling which GTK window is focused, so
    // `win.*` actions are only wired when there is no choice to make.
    let win = if windows == 1 {
        only_window(&bus, name, &path)
            .await
            .map(|p| gio::DBusActionGroup::get(&bus, Some(name), &p).upcast())
    } else {
        None
    };
    Some(Menus {
        model: model.upcast(),
        app: app.upcast(),
        win,
        merges_window: false,
        _dbusmenu: None,
    })
}

/// Lowercase names a registered process may go by for this window: the
/// `app_id`, its last dotted part (`com.rtsoft.patchy` runs as `patchy`),
/// the desktop file stem, and its `StartupWMClass`.
fn process_names_for(
    app_id: &str,
    desktop_stem: Option<&str>,
    wm_class: Option<&str>,
) -> Vec<String> {
    let mut names: Vec<String> = [
        Some(app_id),
        app_id.rsplit('.').next(),
        desktop_stem,
        wm_class,
    ]
    .into_iter()
    .flatten()
    .filter(|n| !n.is_empty())
    .map(str::to_lowercase)
    .collect();
    names.dedup();
    names
}

async fn has_owner(bus: &gio::DBusConnection, name: &str) -> bool {
    bus.call_future(
        Some("org.freedesktop.DBus"),
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
        "NameHasOwner",
        Some(&(name,).to_variant()),
        glib::VariantTy::new("(b)").ok(),
        gio::DBusCallFlags::NONE,
        DBUS_TIMEOUT_MS,
    )
    .await
    .ok()
    .and_then(|reply| reply.get::<(bool,)>())
    .is_some_and(|(owned,)| owned)
}

/// The object path of the application's window when it exports exactly one.
async fn only_window(bus: &gio::DBusConnection, name: &str, path: &str) -> Option<String> {
    let windows = format!("{path}/window");
    let reply = bus
        .call_future(
            Some(name),
            &windows,
            "org.freedesktop.DBus.Introspectable",
            "Introspect",
            None,
            glib::VariantTy::new("(s)").ok(),
            gio::DBusCallFlags::NONE,
            DBUS_TIMEOUT_MS,
        )
        .await
        .ok()?;
    let (xml,) = reply.get::<(String,)>()?;
    match child_nodes(&xml).as_slice() {
        [only] => Some(format!("{windows}/{only}")),
        _ => None,
    }
}

/// Child node names in D-Bus introspection XML.
fn child_nodes(xml: &str) -> Vec<String> {
    xml.split("<node name=\"")
        .skip(1)
        .filter_map(|rest| rest.split_once('"'))
        .map(|(name, _)| name)
        .filter(|name| {
            !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        })
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_path_follows_gtk() {
        assert_eq!(
            object_path("org.gtk.Demo4").as_deref(),
            Some("/org/gtk/Demo4")
        );
        assert_eq!(
            object_path("org.gnome.Text-Editor").as_deref(),
            Some("/org/gnome/Text_Editor")
        );
        assert_eq!(object_path("foot").as_deref(), Some("/foot"));
    }

    #[test]
    fn object_path_rejects_invalid_ids() {
        assert_eq!(object_path("bad..id"), None);
        assert_eq!(object_path("trailing."), None);
        assert_eq!(object_path(""), None);
    }

    #[test]
    fn fallback_names() {
        assert_eq!(fallback_name("cursor"), "Cursor");
        assert_eq!(fallback_name("io.elavo.MenuTest"), "MenuTest");
        assert_eq!(fallback_name("GitHub Desktop"), "GitHub Desktop");
    }

    #[test]
    fn labels_keep_underscores() {
        assert_eq!(menu_label("my_app"), "my__app");
    }

    #[test]
    fn long_titles_are_shortened() {
        assert_eq!(shorten("Files", 10), "Files");
        assert_eq!(shorten("abcdefghijk", 5), "abcd…");
    }

    #[test]
    fn child_nodes_skip_the_root() {
        let xml = r#"<!DOCTYPE node>
<node name="/org/gtk/Demo4/window">
  <interface name="org.freedesktop.DBus.Introspectable"/>
  <node name="1"/>
  <node name="3"/>
</node>"#;
        assert_eq!(child_nodes(xml), ["1", "3"]);
        assert!(child_nodes("<node></node>").is_empty());
    }
}
