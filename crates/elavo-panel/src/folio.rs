//! Folio's own menus, for the desktop and the shell windows named in
//! `FOLIO_APPS`. Folio is the Finder of Elavo, and Elavo Files is its file
//! browser: a focused Files window shows Files' own exported menus under a
//! menu titled Folio (see `appmenu.rs` and [`Folio::files_app_menu`]),
//! and any other application shows the menu it exports.
//!
//! With the desktop focused, file commands go to `elavo-desktop`
//! (`io.elavo.Desktop`), which draws the Desktop folder and mounted
//! volumes, and the rest go to Files (`io.elavo.Files`) as its
//! `org.gtk.Actions`, which starts Files over D-Bus when it is not running.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};

/// Elavo Files, Folio's file browser.
pub const FILES: &str = "io.elavo.Files";

const FILES_PATH: &str = "/io/elavo/Files";
/// Undo and Redo on the desktop: Files' `io.elavo.Files.FileOperations`,
/// whose history every Files window shares. Its `CanUndo`, `CanRedo`,
/// `UndoLabel` and `RedoLabel` properties gray and name the items.
const FILE_OPS_PATH: &str = "/io/elavo/Files/FileOperations";
const FILE_OPS_IFACE: &str = "io.elavo.Files.FileOperations";
const FILE_MANAGER: &str = "org.freedesktop.FileManager1";
const FILE_MANAGER_PATH: &str = "/org/freedesktop/FileManager1";
const DESKTOP_BUS: &str = "io.elavo.Desktop";
const DESKTOP_PATH: &str = "/io/elavo/Desktop";
const DESKTOP_IFACE: &str = "io.elavo.Desktop";

/// Focused windows that show Folio's menus: the shell surfaces that are
/// Folio itself. Files is Folio too, but shows its own exported menus
/// under a Folio menu. A new application is not added: it exports its own
/// menu, and the panel shows that.
const FOLIO_APPS: &[&str] = &[
    "io.elavo.Desktop",
    "io.elavo.Panel",
    "io.elavo.PanelPreview",
    "io.elavo.Dock",
    "io.elavo.Lens",
    "io.elavo.Switcher",
    "io.elavo.Applications",
];

/// One window, so the panel does not offer New Window. A second launch
/// raises the window that is already open.
pub fn keeps_one_window(app_id: &str) -> bool {
    matches!(app_id, "io.elavo.SystemSettings" | "io.elavo.AppBay")
}

/// Nothing focused counts as the desktop, which is Folio. A focused window
/// shows Folio's own menus only when its id is in [`FOLIO_APPS`].
pub fn is_folio(app_id: Option<&str>) -> bool {
    match app_id {
        None => true,
        Some(id) => FOLIO_APPS.contains(&id),
    }
}

/// A Files window.
pub fn is_files(app_id: Option<&str>) -> bool {
    app_id == Some(FILES)
}

/// A window Hide Others leaves up: Files, or one of the shell surfaces in
/// [`FOLIO_APPS`].
pub fn is_folio_window(app_id: &str) -> bool {
    app_id == FILES || FOLIO_APPS.contains(&app_id)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wm {
    HideFolio,
    HideOthers,
    ShowAll,
    Close,
    Minimize,
    Maximize,
    Fullscreen,
    Cycle,
    BringAll,
}

#[derive(Clone, Copy, Debug)]
enum Place {
    Recents,
    Documents,
    Desktop,
    Downloads,
    Home,
    Computer,
    Network,
    Applications,
}

#[derive(Clone, Copy, Debug)]
enum Route {
    About,
    /// An application action of Files (`org.gtk.Actions` on
    /// `io.elavo.Files`). `win-*` act on the Files window focused last, or
    /// open one.
    Files(&'static str),
    /// `io.elavo.Files.FileOperations`, for the desktop's own operations.
    FileOp(&'static str),
    Show(Place),
    /// A command of the desktop, via `io.elavo.Desktop`.
    Desktop(&'static str),
    Wm(Wm),
    Disabled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Needs {
    Always,
    /// The desktop says whether it can run now (its selection).
    Desktop,
    /// At least one Files window exists.
    Windows,
    /// At least two Files windows exist.
    Two,
    /// Only for a focused window, which the desktop is not.
    Never,
    /// Files' shared history has something to undo.
    Undo,
    /// Files' shared history has something to redo.
    Redo,
}

/// Undo and Redo as Files last told: whether each can run, and its label
/// (“Undo Move to Trash”).
#[derive(Clone, Debug, PartialEq, Eq)]
struct UndoState {
    can_undo: bool,
    can_redo: bool,
    undo_label: String,
    redo_label: String,
}

impl Default for UndoState {
    /// Files isn't running: nothing to undo.
    fn default() -> Self {
        Self {
            can_undo: false,
            can_redo: false,
            undo_label: "Undo".to_owned(),
            redo_label: "Redo".to_owned(),
        }
    }
}

impl UndoState {
    /// Take in what a `PropertiesChanged` or `GetAll` reply says.
    fn update(&mut self, props: &glib::Variant) {
        for index in 0..props.n_children() {
            let entry = props.child_value(index);
            let (Some(name), Some(value)) = (
                entry.child_value(0).get::<String>(),
                entry.child_value(1).as_variant(),
            ) else {
                continue;
            };
            match name.as_str() {
                "CanUndo" => self.can_undo = value.get().unwrap_or(false),
                "CanRedo" => self.can_redo = value.get().unwrap_or(false),
                "UndoLabel" => self.undo_label = value.get().unwrap_or_else(|| "Undo".to_owned()),
                "RedoLabel" => self.redo_label = value.get().unwrap_or_else(|| "Redo".to_owned()),
                _ => {}
            }
        }
    }
}

struct Item {
    menu: &'static str,
    section: u8,
    id: &'static str,
    label: &'static str,
    needs: Needs,
    route: Route,
}

const fn item(
    menu: &'static str,
    section: u8,
    id: &'static str,
    label: &'static str,
    needs: Needs,
    route: Route,
) -> Item {
    Item {
        menu,
        section,
        id,
        label,
        needs,
        route,
    }
}

const MENUS: &[&str] = &["Folio", "File", "Edit", "View", "Go", "Window", "Help"];

use Needs::{Always, Desktop, Never, Two, Windows};
use Route::{Files, Show};

const ITEMS: &[Item] = &[
    item("Folio", 0, "about", "About Folio", Always, Route::About),
    item(
        "Folio",
        1,
        "settings",
        "Settings…",
        Always,
        Files("settings"),
    ),
    item(
        "Folio",
        2,
        "empty-trash",
        "Empty Trash…",
        Always,
        Files("empty-trash"),
    ),
    item(
        "Folio",
        3,
        "hide",
        "Hide Folio",
        Windows,
        Route::Wm(Wm::HideFolio),
    ),
    item(
        "Folio",
        3,
        "hide-others",
        "Hide Others",
        Always,
        Route::Wm(Wm::HideOthers),
    ),
    item(
        "Folio",
        3,
        "show-all",
        "Show All",
        Always,
        Route::Wm(Wm::ShowAll),
    ),
    item(
        "File",
        0,
        "new-window",
        "New Folio Window",
        Always,
        Files("new-window"),
    ),
    item(
        "File",
        0,
        "new-folder",
        "New Folder",
        Desktop,
        Route::Desktop("new-folder"),
    ),
    item("File", 0, "open", "Open", Desktop, Route::Desktop("open")),
    item(
        "File",
        0,
        "close",
        "Close Window",
        Never,
        Route::Wm(Wm::Close),
    ),
    item(
        "File",
        1,
        "get-info",
        "Get Info",
        Desktop,
        Route::Desktop("get-info"),
    ),
    item(
        "File",
        1,
        "rename",
        "Rename…",
        Desktop,
        Route::Desktop("rename"),
    ),
    item(
        "File",
        2,
        "quick-look",
        "Quick Look",
        Desktop,
        Route::Desktop("quick-look"),
    ),
    item(
        "File",
        3,
        "trash",
        "Move to Trash",
        Desktop,
        Route::Desktop("move-to-trash"),
    ),
    item("File", 4, "find", "Find", Always, Files("win-search")),
    item(
        "Edit",
        0,
        "undo",
        "Undo",
        Needs::Undo,
        Route::FileOp("Undo"),
    ),
    item(
        "Edit",
        0,
        "redo",
        "Redo",
        Needs::Redo,
        Route::FileOp("Redo"),
    ),
    item("Edit", 1, "cut", "Cut", Desktop, Route::Desktop("cut")),
    item("Edit", 1, "copy", "Copy", Desktop, Route::Desktop("copy")),
    item(
        "Edit",
        1,
        "paste",
        "Paste",
        Desktop,
        Route::Desktop("paste"),
    ),
    item(
        "Edit",
        2,
        "select-all",
        "Select All",
        Desktop,
        Route::Desktop("select-all"),
    ),
    item(
        "View",
        0,
        "use-stacks",
        "Use Stacks",
        Desktop,
        Route::Desktop("use-stacks"),
    ),
    item(
        "View",
        1,
        "fullscreen",
        "Enter Full Screen",
        Never,
        Route::Wm(Wm::Fullscreen),
    ),
    item("Go", 0, "recents", "Recents", Always, Show(Place::Recents)),
    item(
        "Go",
        0,
        "documents",
        "Documents",
        Always,
        Show(Place::Documents),
    ),
    item("Go", 0, "desktop", "Desktop", Always, Show(Place::Desktop)),
    item(
        "Go",
        0,
        "downloads",
        "Downloads",
        Always,
        Show(Place::Downloads),
    ),
    item("Go", 0, "home", "Home", Always, Show(Place::Home)),
    item(
        "Go",
        0,
        "computer",
        "Computer",
        Always,
        Show(Place::Computer),
    ),
    item("Go", 0, "network", "Network", Always, Show(Place::Network)),
    item(
        "Go",
        0,
        "applications",
        "Applications",
        Always,
        Show(Place::Applications),
    ),
    item(
        "Go",
        1,
        "go-to-folder",
        "Go to Folder…",
        Always,
        Files("win-go-to-folder"),
    ),
    item(
        "Go",
        1,
        "connect",
        "Connect to Server…",
        Always,
        Files("win-connect-server"),
    ),
    item(
        "Window",
        0,
        "minimize",
        "Minimize",
        Never,
        Route::Wm(Wm::Minimize),
    ),
    item("Window", 0, "zoom", "Zoom", Never, Route::Wm(Wm::Maximize)),
    item(
        "Window",
        1,
        "cycle",
        "Cycle Through Windows",
        Two,
        Route::Wm(Wm::Cycle),
    ),
    item(
        "Window",
        1,
        "progress",
        "Show Progress Window",
        Windows,
        Files("win-show-progress"),
    ),
    item(
        "Window",
        2,
        "bring-all-to-front",
        "Bring All to Front",
        Windows,
        Route::Wm(Wm::BringAll),
    ),
    item("Help", 0, "help", "Folio Help", Never, Route::Disabled),
    item(
        "Help",
        0,
        "shortcuts",
        "Keyboard Shortcuts",
        Always,
        Files("shortcuts"),
    ),
];

type WmHandler = Box<dyn Fn(Wm)>;
type FocusHandler = Box<dyn Fn(bool)>;

pub struct Folio {
    actions: gio::SimpleActionGroup,
    model: gio::Menu,
    /// Bumped on every context change so a late desktop reply is dropped.
    desktop_gen: Cell<u64>,
    desktop_inflight: Cell<bool>,
    desktop_pending: Cell<bool>,
    desktop_signals: RefCell<Vec<gio::SignalSubscription>>,
    desktop_focus: RefCell<Option<FocusHandler>>,
    wm: RefCell<Option<WmHandler>>,
    undo: RefCell<UndoState>,
    /// The Edit menu's Undo and Redo, relabeled as the history changes.
    undo_section: RefCell<Option<gio::Menu>>,
    files_signals: RefCell<Vec<gio::SignalSubscription>>,
}

impl Folio {
    pub fn new(window_list: &gio::Menu) -> Rc<Self> {
        let folio = Rc::new(Self {
            actions: gio::SimpleActionGroup::new(),
            model: gio::Menu::new(),
            desktop_gen: Cell::new(0),
            desktop_inflight: Cell::new(false),
            desktop_pending: Cell::new(false),
            desktop_signals: RefCell::new(Vec::new()),
            desktop_focus: RefCell::new(None),
            wm: RefCell::new(None),
            undo: RefCell::default(),
            undo_section: RefCell::new(None),
            files_signals: RefCell::new(Vec::new()),
        });
        folio.build(window_list);
        folio.set_context(0);
        folio.refresh();
        folio.listen_desktop();
        folio.listen_files();
        folio
    }

    pub fn model(&self) -> &gio::Menu {
        &self.model
    }

    pub fn actions(&self) -> &gio::SimpleActionGroup {
        &self.actions
    }

    /// The menu titled Folio in front of a Files window's own menus.
    /// `folio.*` are these actions; `app.*` are Files' own, from its
    /// exported action group.
    pub fn files_app_menu() -> gio::Menu {
        let menu = gio::Menu::new();
        let section = |items: &[(&str, &str, Option<&str>)]| {
            let section = gio::Menu::new();
            for (label, action, accel) in items {
                let item = gio::MenuItem::new(Some(&mnemonic(label)), Some(action));
                if let Some(accel) = accel {
                    item.set_attribute_value("accel", Some(&accel.to_variant()));
                }
                section.append_item(&item);
            }
            section
        };
        menu.append_section(None, &section(&[("About Folio", "folio.about", None)]));
        menu.append_section(
            None,
            &section(&[("Settings…", "app.settings", Some("<Control>comma"))]),
        );
        menu.append_section(None, &section(&[("Empty Trash…", "app.empty-trash", None)]));
        menu.append_section(
            None,
            &section(&[
                ("Hide Folio", "folio.hide", None),
                ("Hide Others", "folio.hide-others", None),
                ("Show All", "folio.show-all", None),
            ]),
        );
        menu
    }

    /// `wm` runs the window commands, which need the panel's window list.
    pub fn set_wm_handler(&self, wm: impl Fn(Wm) + 'static) {
        self.wm.replace(Some(Box::new(wm)));
    }

    /// `focus` learns when the desktop takes or gives up the keyboard. labwc
    /// leaves the last window activated while a layer surface has the
    /// keyboard, so the window list alone would still name that window.
    pub fn set_desktop_focus_handler(&self, focus: impl Fn(bool) + 'static) {
        self.desktop_focus.replace(Some(Box::new(focus)));
    }

    /// `files_windows` is how many Files windows are open, for Hide Folio,
    /// Cycle Through Windows and Bring All to Front.
    pub fn set_context(self: &Rc<Self>, files_windows: usize) {
        for item in ITEMS {
            let enabled = match item.needs {
                // The desktop's answer arrives in query_desktop.
                Needs::Desktop => continue,
                Needs::Always => true,
                Needs::Windows => files_windows > 0,
                Needs::Two => files_windows >= 2,
                Needs::Never => false,
                Needs::Undo => self.undo.borrow().can_undo,
                Needs::Redo => self.undo.borrow().can_redo,
            };
            self.set_enabled(item.id, enabled);
        }
    }

    fn set_enabled(&self, id: &str, enabled: bool) {
        if let Some(action) = self
            .actions
            .lookup_action(id)
            .and_then(|action| action.downcast::<gio::SimpleAction>().ok())
        {
            action.set_enabled(enabled);
        }
    }

    /// Refresh which desktop commands can run, as a menu opens.
    pub(crate) fn refresh(self: &Rc<Self>) {
        self.query_desktop();
    }

    /// Ask the desktop which file commands can run on the current selection.
    fn query_desktop(self: &Rc<Self>) {
        if self.desktop_inflight.get() {
            self.desktop_pending.set(true);
            return;
        }
        self.desktop_inflight.set(true);
        self.desktop_pending.set(false);
        let generation = self.desktop_gen.get();
        let mut actions = Vec::new();
        let mut ids = Vec::new();
        for item in ITEMS {
            let Route::Desktop(command) = item.route else {
                continue;
            };
            if item.needs != Needs::Desktop {
                continue;
            }
            actions.push(command.to_string());
            ids.push(item.id);
        }
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let reply = desktop_enabled(actions).await;
            let stacks = desktop_stacks().await.ok();
            let Some(folio) = weak.upgrade() else {
                return;
            };
            folio.desktop_inflight.set(false);
            if folio.desktop_gen.get() != generation {
                return;
            }
            if let Some(on) = stacks {
                folio.set_stacks_state(on);
            }
            match reply {
                Ok(flags) => {
                    for (id, enabled) in ids.into_iter().zip(flags) {
                        folio.set_enabled(id, enabled);
                    }
                }
                Err(err) => {
                    glib::g_warning!("elavo-panel", "desktop: {err}");
                    for id in ids {
                        folio.set_enabled(id, false);
                    }
                }
            }
            if folio.desktop_pending.get() {
                folio.query_desktop();
            }
        });
    }

    fn set_stacks_state(&self, on: bool) {
        if let Some(action) = self
            .actions
            .lookup_action("use-stacks")
            .and_then(|action| action.downcast::<gio::SimpleAction>().ok())
        {
            action.set_state(&on.to_variant());
        }
    }

    fn listen_desktop(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let Ok(bus) = gio::bus_get_future(gio::BusType::Session).await else {
                return;
            };
            let Some(folio) = weak.upgrade() else {
                return;
            };
            let owner = Rc::downgrade(&folio);
            let subscription = bus.subscribe_to_signal(
                Some(DESKTOP_BUS),
                Some(DESKTOP_IFACE),
                Some("Changed"),
                Some(DESKTOP_PATH),
                None,
                gio::DBusSignalFlags::NONE,
                move |signal| {
                    let Some(folio) = owner.upgrade() else {
                        return;
                    };
                    let params = signal.parameters;
                    if params.n_children() > 0 {
                        if let Some(on) = params.child_value(0).get::<bool>() {
                            folio.set_stacks_state(on);
                        }
                    }
                    folio.query_desktop();
                },
            );
            let owner = Rc::downgrade(&folio);
            let focus = bus.subscribe_to_signal(
                Some(DESKTOP_BUS),
                Some(DESKTOP_IFACE),
                Some("Focused"),
                Some(DESKTOP_PATH),
                None,
                gio::DBusSignalFlags::NONE,
                move |signal| {
                    let Some(folio) = owner.upgrade() else {
                        return;
                    };
                    let params = signal.parameters;
                    let Some(on) = (params.n_children() > 0)
                        .then(|| params.child_value(0).get::<bool>())
                        .flatten()
                    else {
                        return;
                    };
                    let handler = folio.desktop_focus.borrow();
                    if let Some(handler) = handler.as_ref() {
                        handler(on);
                    }
                },
            );
            folio
                .desktop_signals
                .borrow_mut()
                .extend([subscription, focus]);
            let owner = Rc::downgrade(&folio);
            gio::bus_watch_name_on_connection(
                &bus,
                DESKTOP_BUS,
                gio::BusNameWatcherFlags::NONE,
                |_, _, _| {},
                move |_, _| {
                    let Some(folio) = owner.upgrade() else {
                        return;
                    };
                    let handler = folio.desktop_focus.borrow();
                    if let Some(handler) = handler.as_ref() {
                        handler(false);
                    }
                },
            );
        });
    }

    /// Follow Files' shared Undo history: its properties while it runs,
    /// nothing to undo while it doesn't. Files isn't started for this.
    fn listen_files(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let Ok(bus) = gio::bus_get_future(gio::BusType::Session).await else {
                return;
            };
            let Some(folio) = weak.upgrade() else {
                return;
            };
            let owner = Rc::downgrade(&folio);
            let changed = bus.subscribe_to_signal(
                Some(FILES),
                Some("org.freedesktop.DBus.Properties"),
                Some("PropertiesChanged"),
                Some(FILE_OPS_PATH),
                Some(FILE_OPS_IFACE),
                gio::DBusSignalFlags::NONE,
                move |signal| {
                    let Some(folio) = owner.upgrade() else {
                        return;
                    };
                    let params = signal.parameters;
                    if params.n_children() > 1 {
                        let mut state = folio.undo.borrow().clone();
                        state.update(&params.child_value(1));
                        folio.set_undo_state(state);
                    }
                },
            );
            folio.files_signals.borrow_mut().push(changed);
            let appeared = Rc::downgrade(&folio);
            let vanished = Rc::downgrade(&folio);
            gio::bus_watch_name_on_connection(
                &bus,
                FILES,
                gio::BusNameWatcherFlags::NONE,
                move |bus, _, _| {
                    let weak = appeared.clone();
                    let bus = bus.clone();
                    glib::spawn_future_local(async move {
                        let reply = bus
                            .call_future(
                                Some(FILES),
                                FILE_OPS_PATH,
                                "org.freedesktop.DBus.Properties",
                                "GetAll",
                                Some(&(FILE_OPS_IFACE,).to_variant()),
                                None,
                                gio::DBusCallFlags::NO_AUTO_START,
                                -1,
                            )
                            .await;
                        let Some(folio) = weak.upgrade() else {
                            return;
                        };
                        match reply {
                            Ok(reply) => {
                                let mut state = folio.undo.borrow().clone();
                                state.update(&reply.child_value(0));
                                folio.set_undo_state(state);
                            }
                            Err(err) => glib::g_warning!("elavo-panel", "files undo: {err}"),
                        }
                    });
                },
                move |_, _| {
                    if let Some(folio) = vanished.upgrade() {
                        folio.set_undo_state(UndoState::default());
                    }
                },
            );
        });
    }

    fn set_undo_state(&self, state: UndoState) {
        if *self.undo.borrow() == state {
            return;
        }
        self.set_enabled("undo", state.can_undo);
        self.set_enabled("redo", state.can_redo);
        if let Some(section) = self.undo_section.borrow().as_ref() {
            fill_undo_section(section, &state);
        }
        self.undo.replace(state);
    }

    fn build(self: &Rc<Self>, window_list: &gio::Menu) {
        let applications = applications_dir().is_some();
        for item in ITEMS {
            let action = if item.id == "use-stacks" {
                gio::SimpleAction::new_stateful(item.id, None, &false.to_variant())
            } else {
                gio::SimpleAction::new(item.id, None)
            };
            let weak = Rc::downgrade(self);
            let route = item.route;
            action.connect_activate(move |_, _| {
                if let Some(folio) = weak.upgrade() {
                    folio.run(route);
                }
            });
            self.actions.add_action(&action);
        }

        for menu_name in MENUS {
            let submenu = gio::Menu::new();
            let mut sections: Vec<u8> = ITEMS
                .iter()
                .filter(|item| item.menu == *menu_name)
                .map(|item| item.section)
                .collect();
            sections.dedup();
            for section in sections {
                let section_menu = gio::Menu::new();
                for item in ITEMS.iter().filter(|item| {
                    item.menu == *menu_name
                        && item.section == section
                        && (item.id != "applications" || applications)
                }) {
                    section_menu.append(
                        Some(&mnemonic(item.label)),
                        Some(&format!("folio.{}", item.id)),
                    );
                }
                if *menu_name == "Edit" && section == 0 {
                    fill_undo_section(&section_menu, &self.undo.borrow());
                    self.undo_section.replace(Some(section_menu.clone()));
                }
                if section_menu.n_items() > 0 {
                    submenu.append_section(None, &section_menu);
                }
            }
            if *menu_name == "Window" {
                submenu.append_section(None, window_list);
            }
            self.model.append_submenu(Some(menu_name), &submenu);
        }
    }

    fn run(&self, route: Route) {
        match route {
            Route::About => crate::menu::open_settings("about"),
            Route::Files(action) => files_action(action),
            Route::FileOp(method) => file_op(method),
            Route::Show(place) => {
                if let Some(uri) = place.uri() {
                    show_folder(uri);
                }
            }
            Route::Desktop(action) => desktop_call(action),
            Route::Wm(wm) => {
                if let Some(handler) = self.wm.borrow().as_ref() {
                    handler(wm);
                }
            }
            Route::Disabled => {}
        }
    }
}

/// The Edit menu's first section: Undo and Redo, named for what they do.
fn fill_undo_section(section: &gio::Menu, state: &UndoState) {
    section.remove_all();
    for (label, action) in [
        (&state.undo_label, "folio.undo"),
        (&state.redo_label, "folio.redo"),
    ] {
        section.append(Some(&mnemonic(label)), Some(action));
    }
}

fn mnemonic(label: &str) -> String {
    label.replace('_', "__")
}

/// The shared Applications folder, or this account's own on a system that
/// does not have the shared one.
fn applications_dir() -> Option<std::path::PathBuf> {
    [
        elavo_common::apps::shared_applications_dir(),
        elavo_common::apps::home_applications_dir(),
    ]
    .into_iter()
    .find(|p| p.is_dir())
}

fn home_uri() -> Option<String> {
    Some(gio::File::for_path(glib::home_dir()).uri().to_string())
}

impl Place {
    fn uri(self) -> Option<String> {
        let special = |dir: glib::UserDirectory| {
            glib::user_special_dir(dir).map(|path| gio::File::for_path(path).uri().to_string())
        };
        match self {
            Place::Recents => Some("recent:///".to_owned()),
            Place::Documents => special(glib::UserDirectory::Documents),
            Place::Desktop => special(glib::UserDirectory::Desktop),
            Place::Downloads => special(glib::UserDirectory::Downloads),
            Place::Home => home_uri(),
            // Files has no Computer view: Home, with the drives under
            // Locations in its sidebar.
            Place::Computer => home_uri(),
            Place::Network => Some("network:///".to_owned()),
            Place::Applications => {
                applications_dir().map(|path| gio::File::for_path(path).uri().to_string())
            }
        }
    }
}

/// Activate an application action of Files over `org.gtk.Actions`. D-Bus
/// starts Files first when it is not running.
fn files_action(action: &'static str) {
    glib::spawn_future_local(async move {
        let call = async {
            let bus = gio::bus_get_future(gio::BusType::Session).await?;
            let text = format!("('{action}', @av [], @a{{sv}} {{}})");
            let parameters = glib::Variant::parse(None, &text).map_err(|err| {
                glib::Error::new(gio::IOErrorEnum::InvalidArgument, err.message())
            })?;
            bus.call_future(
                Some(FILES),
                FILES_PATH,
                "org.gtk.Actions",
                "Activate",
                Some(&parameters),
                None,
                gio::DBusCallFlags::NONE,
                -1,
            )
            .await
        };
        if let Err(err) = call.await {
            glib::g_warning!("elavo-panel", "files {action}: {err}");
        }
    });
}

fn file_op(method: &'static str) {
    glib::spawn_future_local(async move {
        let call = async {
            let bus = gio::bus_get_future(gio::BusType::Session).await?;
            // `(a{sv} platform_data)`: the arguments go in a tuple.
            let parameters =
                (std::collections::HashMap::<String, glib::Variant>::new(),).to_variant();
            bus.call_future(
                Some(FILES),
                FILE_OPS_PATH,
                FILE_OPS_IFACE,
                method,
                Some(&parameters),
                None,
                gio::DBusCallFlags::NONE,
                -1,
            )
            .await
        };
        if let Err(err) = call.await {
            glib::g_warning!("elavo-panel", "files {method}: {err}");
        }
    });
}

fn desktop_call(action: &'static str) {
    glib::spawn_future_local(async move {
        let call = async {
            let bus = gio::bus_get_future(gio::BusType::Session).await?;
            bus.call_future(
                Some(DESKTOP_BUS),
                DESKTOP_PATH,
                DESKTOP_IFACE,
                "Activate",
                Some(&(action.to_string(),).to_variant()),
                None,
                gio::DBusCallFlags::NONE,
                -1,
            )
            .await
        };
        if let Err(err) = call.await {
            glib::g_warning!("elavo-panel", "desktop {action}: {err}");
        }
    });
}

async fn desktop_enabled(actions: Vec<String>) -> Result<Vec<bool>, glib::Error> {
    let bus = gio::bus_get_future(gio::BusType::Session).await?;
    let reply = bus
        .call_future(
            Some(DESKTOP_BUS),
            DESKTOP_PATH,
            DESKTOP_IFACE,
            "Enabled",
            Some(&(actions,).to_variant()),
            None,
            gio::DBusCallFlags::NONE,
            -1,
        )
        .await?;
    let array = reply.child_value(0);
    let mut flags = Vec::with_capacity(array.n_children());
    for index in 0..array.n_children() {
        flags.push(array.child_value(index).get::<bool>().unwrap_or(false));
    }
    Ok(flags)
}

async fn desktop_stacks() -> Result<bool, glib::Error> {
    let bus = gio::bus_get_future(gio::BusType::Session).await?;
    let reply = bus
        .call_future(
            Some(DESKTOP_BUS),
            DESKTOP_PATH,
            DESKTOP_IFACE,
            "Stacks",
            None,
            None,
            gio::DBusCallFlags::NONE,
            -1,
        )
        .await?;
    Ok(reply.child_value(0).get::<bool>().unwrap_or(false))
}

fn show_folder(uri: String) {
    glib::spawn_future_local(async move {
        let call = async {
            let bus = gio::bus_get_future(gio::BusType::Session).await?;
            bus.call_future(
                Some(FILE_MANAGER),
                FILE_MANAGER_PATH,
                FILE_MANAGER,
                "ShowFolders",
                Some(&(vec![uri.clone()], String::new()).to_variant()),
                None,
                gio::DBusCallFlags::NONE,
                -1,
            )
            .await
        };
        if let Err(err) = call.await {
            glib::g_warning!("elavo-panel", "show folder: {err}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folio_is_an_allowlist() {
        assert!(is_folio(None));
        assert!(is_folio(Some("io.elavo.Panel")));
        assert!(is_folio(Some("io.elavo.PanelPreview")));
        assert!(is_folio(Some("io.elavo.Dock")));
        assert!(is_folio(Some("io.elavo.Desktop")));
        assert!(is_folio(Some("io.elavo.Lens")));
        assert!(is_folio(Some("io.elavo.Switcher")));
        assert!(is_folio(Some("io.elavo.Applications")));
        // Files shows its own menus under a Folio menu.
        assert!(!is_folio(Some(FILES)));
        assert!(is_files(Some(FILES)));
        assert!(!is_files(None));
        assert!(is_folio_window(FILES));
        assert!(is_folio_window("io.elavo.Panel"));
        // Nautilus is an ordinary application now.
        assert!(!is_folio(Some("org.gnome.Nautilus")));
        assert!(!is_folio_window("org.gnome.Nautilus"));
        assert!(!is_folio(Some("io.elavo.SystemSettings")));
        assert!(!is_folio(Some("io.elavo.AppBay")));
        assert!(!is_folio(Some("io.elavo.Music")));
        assert!(!is_folio_window("io.elavo.SystemSettings"));
        assert!(!is_folio_window("io.elavo.AppBay"));
        assert!(!is_folio(Some("org.mozilla.firefox")));
        assert!(!is_folio(Some("cursor")));
        assert!(!is_folio(Some("")));
    }

    #[test]
    fn every_item_has_an_action() {
        let mut seen = std::collections::HashSet::new();
        for item in ITEMS {
            assert!(seen.insert(item.id), "duplicate action {}", item.id);
            assert!(!item.label.is_empty(), "{}", item.id);
            assert!(MENUS.contains(&item.menu), "{}", item.id);
        }
        assert_eq!(seen.len(), ITEMS.len());
    }

    #[test]
    fn nothing_goes_to_nautilus_windows() {
        for item in ITEMS {
            if let Route::Files(action) = item.route {
                assert!(!action.contains('.'), "{} names a group", item.id);
                assert!(!action.is_empty(), "{}", item.id);
            }
        }
        for id in [
            "settings",
            "new-window",
            "empty-trash",
            "connect",
            "shortcuts",
            "progress",
        ] {
            let item = ITEMS.iter().find(|item| item.id == id).unwrap();
            assert!(matches!(item.route, Route::Files(_)), "{id}");
        }
    }

    #[test]
    fn desktop_undo_goes_to_files() {
        assert!(FILE_OPS_PATH.starts_with("/io/elavo/Files/"));
        assert!(FILE_OPS_IFACE.starts_with(FILES));
        for id in ["undo", "redo"] {
            let item = ITEMS.iter().find(|item| item.id == id).unwrap();
            assert!(matches!(item.route, Route::FileOp(_)), "{id}");
        }
    }

    #[test]
    fn undo_follows_what_files_says() {
        let mut state = UndoState::default();
        assert!(!state.can_undo && !state.can_redo);
        assert_eq!(state.undo_label, "Undo");
        let props = glib::Variant::parse(
            None,
            "{'CanUndo': <true>, 'UndoLabel': <'Undo Move to Trash'>, 'Other': <1>}",
        )
        .unwrap();
        state.update(&props);
        assert!(state.can_undo);
        assert!(!state.can_redo);
        assert_eq!(state.undo_label, "Undo Move to Trash");
        assert_eq!(state.redo_label, "Redo");
        let items = ITEMS
            .iter()
            .filter(|item| item.id == "undo" || item.id == "redo");
        for item in items {
            assert!(
                matches!(item.needs, Needs::Undo | Needs::Redo),
                "{}",
                item.id
            );
        }
    }

    #[test]
    fn undo_section_names_the_action() {
        let section = gio::Menu::new();
        let state = UndoState {
            can_undo: true,
            can_redo: true,
            undo_label: "Undo Rename".to_owned(),
            redo_label: "Redo Move to Trash".to_owned(),
        };
        fill_undo_section(&section, &state);
        fill_undo_section(&section, &state);
        assert_eq!(section.n_items(), 2);
        let label = |i| {
            section
                .item_attribute_value(i, "label", None)
                .and_then(|v| v.get::<String>())
        };
        assert_eq!(label(0).as_deref(), Some("Undo Rename"));
        assert_eq!(label(1).as_deref(), Some("Redo Move to Trash"));
    }

    #[test]
    fn desktop_items_ask_the_desktop() {
        for item in ITEMS {
            assert_eq!(
                item.needs == Needs::Desktop,
                matches!(item.route, Route::Desktop(_)),
                "{}",
                item.id
            );
        }
        assert!(ITEMS.iter().any(|item| {
            item.id == "use-stacks" && item.needs == Needs::Desktop && item.menu == "View"
        }));
    }

    #[test]
    fn window_commands_stay_off_the_desktop() {
        for id in ["close", "fullscreen", "minimize", "zoom"] {
            let item = ITEMS.iter().find(|item| item.id == id).unwrap();
            assert_eq!(item.needs, Needs::Never, "{id}");
        }
        for id in [
            "new-tab",
            "previous-tab",
            "next-tab",
            "icons",
            "list",
            "back",
        ] {
            assert!(ITEMS.iter().all(|item| item.id != id), "{id}");
        }
    }

    #[test]
    fn computer_shows_home() {
        assert_eq!(Place::Computer.uri(), home_uri());
    }
}
