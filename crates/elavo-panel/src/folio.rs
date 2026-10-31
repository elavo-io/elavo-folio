//! Folio's own menus. The desktop, a Nautilus window, and Folio's own
//! dialogs are one application here, so the bar keeps these menus instead
//! of the focused window's.
//!
//! Nautilus exports almost none of its menu over D-Bus. Items that act on
//! the focused window go through the Folio Files plugin
//! (`io.elavo.FolioFiles`), which runs inside Nautilus and calls the
//! window's own actions. With no Files window focused, the same file
//! commands go to `elavo-desktop` (`io.elavo.Desktop`), which draws the
//! Desktop folder and mounted volumes.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};

pub const NAUTILUS: &str = "org.gnome.Nautilus";

const NAUTILUS_PATH: &str = "/org/gnome/Nautilus";
const FILE_OPS_PATH: &str = "/org/gnome/Nautilus/FileOperations2";
const FILE_OPS_IFACE: &str = "org.gnome.Nautilus.FileOperations2";
const FILE_MANAGER: &str = "org.freedesktop.FileManager1";
const FILE_MANAGER_PATH: &str = "/org/freedesktop/FileManager1";
const FOLIO_FILES: &str = "io.elavo.FolioFiles";
const FOLIO_FILES_PATH: &str = "/io/elavo/FolioFiles";
const DESKTOP_BUS: &str = "io.elavo.Desktop";
const DESKTOP_PATH: &str = "/io/elavo/Desktop";
const DESKTOP_IFACE: &str = "io.elavo.Desktop";

/// Nothing focused, Nautilus, or one of Folio's own windows (the panel's
/// dialogs, the dock, the desktop). System Settings is its own application.
pub(crate) const SYSTEM_SETTINGS: &str = "io.elavo.SystemSettings";

pub fn is_folio(app_id: Option<&str>) -> bool {
    match app_id {
        None => true,
        Some(NAUTILUS) => true,
        Some(SYSTEM_SETTINGS) => false,
        Some(id) => id.starts_with("io.elavo."),
    }
}

/// A window that belongs to Folio: Nautilus, or one of Folio's own programs.
/// System Settings is not one of them, so Hide Others can hide it.
pub fn is_folio_window(app_id: &str) -> bool {
    app_id != SYSTEM_SETTINGS && (app_id == NAUTILUS || app_id.starts_with("io.elavo."))
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
    Computer,
    Network,
    Applications,
    /// Connect to Server opens the same screen as Computer.
    Connect,
}

#[derive(Clone, Copy, Debug)]
enum Route {
    About,
    /// An `org.gtk.Actions` action on `org.gnome.Nautilus`.
    Nautilus(&'static str),
    /// `org.gnome.Nautilus.FileOperations2`. `EmptyTrash` asks first.
    FileOp(&'static str),
    Show(Place),
    /// `win.go-home` in the focused Nautilus window, or a new window otherwise.
    Home,
    /// An action on the focused Nautilus window, via `io.elavo.FolioFiles`.
    /// `folio.previous-tab` and `folio.next-tab` switch tabs in the plugin.
    Files(&'static str),
    /// An action on the desktop, via `io.elavo.Desktop`, when Files is not focused.
    Desktop(&'static str),
    /// `slot.files-view-mode`. List is 1, grid (icons) is 2.
    FilesMode(u32),
    Wm(Wm),
    Disabled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Needs {
    Always,
    /// A Nautilus window is focused.
    Focused,
    /// A Nautilus window, or the desktop when Files is not focused.
    Surface,
    /// The desktop only. Off while a Files window is focused.
    DesktopOnly,
    /// At least one Nautilus window exists.
    Windows,
    /// At least two Nautilus windows exist.
    Two,
    Never,
}

struct Item {
    menu: &'static str,
    section: u8,
    id: &'static str,
    label: &'static str,
    needs: Needs,
    route: Route,
}

const fn files(action: &'static str) -> Route {
    Route::Files(action)
}

fn files_action(route: Route) -> Option<&'static str> {
    match route {
        Route::Files(action) => Some(action),
        Route::FilesMode(_) => Some("slot.files-view-mode"),
        _ => None,
    }
}

const MENUS: &[&str] = &["Folio", "File", "Edit", "View", "Go", "Window", "Help"];

const ITEMS: &[Item] = &[
    Item {
        menu: "Folio",
        section: 0,
        id: "about",
        label: "About Folio",
        needs: Needs::Always,
        route: Route::About,
    },
    Item {
        menu: "Folio",
        section: 1,
        id: "settings",
        label: "Settings…",
        needs: Needs::Always,
        route: Route::Nautilus("preferences"),
    },
    Item {
        menu: "Folio",
        section: 2,
        id: "empty-trash",
        label: "Empty Trash…",
        needs: Needs::Always,
        route: Route::FileOp("EmptyTrash"),
    },
    Item {
        menu: "Folio",
        section: 3,
        id: "hide",
        label: "Hide Folio",
        needs: Needs::Windows,
        route: Route::Wm(Wm::HideFolio),
    },
    Item {
        menu: "Folio",
        section: 3,
        id: "hide-others",
        label: "Hide Others",
        needs: Needs::Always,
        route: Route::Wm(Wm::HideOthers),
    },
    Item {
        menu: "Folio",
        section: 3,
        id: "show-all",
        label: "Show All",
        needs: Needs::Always,
        route: Route::Wm(Wm::ShowAll),
    },
    Item {
        menu: "File",
        section: 0,
        id: "new-window",
        label: "New Folio Window",
        needs: Needs::Always,
        route: Route::Nautilus("new-window"),
    },
    Item {
        menu: "File",
        section: 0,
        id: "new-folder",
        label: "New Folder",
        needs: Needs::Surface,
        route: files("view.new-folder"),
    },
    Item {
        menu: "File",
        section: 0,
        id: "new-tab",
        label: "New Tab",
        needs: Needs::Focused,
        route: files("win.new-tab"),
    },
    Item {
        menu: "File",
        section: 0,
        id: "open",
        label: "Open",
        needs: Needs::Surface,
        route: files("view.open-with-default-application"),
    },
    Item {
        menu: "File",
        section: 1,
        id: "close",
        label: "Close Window",
        needs: Needs::Focused,
        route: Route::Wm(Wm::Close),
    },
    Item {
        menu: "File",
        section: 2,
        id: "get-info",
        label: "Get Info",
        needs: Needs::Surface,
        route: files("view.properties"),
    },
    Item {
        menu: "File",
        section: 2,
        id: "rename",
        label: "Rename…",
        needs: Needs::Surface,
        route: files("view.rename"),
    },
    Item {
        menu: "File",
        section: 3,
        id: "make-link",
        label: "Make Link",
        needs: Needs::Focused,
        route: files("view.create-link-in-place"),
    },
    Item {
        menu: "File",
        section: 3,
        id: "quick-look",
        label: "Quick Look",
        needs: Needs::Surface,
        route: files("view.preview-selection"),
    },
    Item {
        menu: "File",
        section: 3,
        id: "add-to-sidebar",
        label: "Add to Sidebar",
        needs: Needs::Focused,
        route: files("slot.bookmark-current-directory"),
    },
    Item {
        menu: "File",
        section: 4,
        id: "trash",
        label: "Move to Trash",
        needs: Needs::Surface,
        route: files("view.move-to-trash"),
    },
    Item {
        menu: "File",
        section: 5,
        id: "find",
        label: "Find",
        needs: Needs::Focused,
        route: files("slot.focus-search"),
    },
    Item {
        menu: "Edit",
        section: 0,
        id: "undo",
        label: "Undo",
        needs: Needs::Always,
        route: Route::FileOp("Undo"),
    },
    Item {
        menu: "Edit",
        section: 0,
        id: "redo",
        label: "Redo",
        needs: Needs::Always,
        route: Route::FileOp("Redo"),
    },
    Item {
        menu: "Edit",
        section: 1,
        id: "cut",
        label: "Cut",
        needs: Needs::Surface,
        route: files("view.cut"),
    },
    Item {
        menu: "Edit",
        section: 1,
        id: "copy",
        label: "Copy",
        needs: Needs::Surface,
        route: files("view.copy"),
    },
    Item {
        menu: "Edit",
        section: 1,
        id: "paste",
        label: "Paste",
        needs: Needs::Surface,
        route: files("view.paste"),
    },
    Item {
        menu: "Edit",
        section: 2,
        id: "select-all",
        label: "Select All",
        needs: Needs::Surface,
        route: files("view.select-all"),
    },
    Item {
        menu: "View",
        section: 0,
        id: "icons",
        label: "as Icons",
        needs: Needs::Focused,
        route: Route::FilesMode(2),
    },
    Item {
        menu: "View",
        section: 0,
        id: "list",
        label: "as List",
        needs: Needs::Focused,
        route: Route::FilesMode(1),
    },
    Item {
        menu: "View",
        section: 1,
        id: "sidebar",
        label: "Show/Hide Sidebar",
        needs: Needs::Focused,
        route: files("win.toggle-sidebar"),
    },
    Item {
        menu: "View",
        section: 2,
        id: "fullscreen",
        label: "Enter Full Screen",
        needs: Needs::Focused,
        route: Route::Wm(Wm::Fullscreen),
    },
    Item {
        menu: "View",
        section: 3,
        id: "use-stacks",
        label: "Use Stacks",
        needs: Needs::DesktopOnly,
        route: Route::Desktop("use-stacks"),
    },
    Item {
        menu: "Go",
        section: 0,
        id: "back",
        label: "Back",
        needs: Needs::Focused,
        route: files("slot.back"),
    },
    Item {
        menu: "Go",
        section: 0,
        id: "forward",
        label: "Forward",
        needs: Needs::Focused,
        route: files("slot.forward"),
    },
    Item {
        menu: "Go",
        section: 0,
        id: "enclosing",
        label: "Enclosing Folder",
        needs: Needs::Focused,
        route: files("slot.up"),
    },
    Item {
        menu: "Go",
        section: 1,
        id: "recents",
        label: "Recents",
        needs: Needs::Always,
        route: Route::Show(Place::Recents),
    },
    Item {
        menu: "Go",
        section: 1,
        id: "documents",
        label: "Documents",
        needs: Needs::Always,
        route: Route::Show(Place::Documents),
    },
    Item {
        menu: "Go",
        section: 1,
        id: "desktop",
        label: "Desktop",
        needs: Needs::Always,
        route: Route::Show(Place::Desktop),
    },
    Item {
        menu: "Go",
        section: 1,
        id: "downloads",
        label: "Downloads",
        needs: Needs::Always,
        route: Route::Show(Place::Downloads),
    },
    Item {
        menu: "Go",
        section: 1,
        id: "home",
        label: "Home",
        needs: Needs::Always,
        route: Route::Home,
    },
    Item {
        menu: "Go",
        section: 1,
        id: "computer",
        label: "Computer",
        needs: Needs::Always,
        route: Route::Show(Place::Computer),
    },
    Item {
        menu: "Go",
        section: 1,
        id: "network",
        label: "Network",
        needs: Needs::Always,
        route: Route::Show(Place::Network),
    },
    Item {
        menu: "Go",
        section: 1,
        id: "applications",
        label: "Applications",
        needs: Needs::Always,
        route: Route::Show(Place::Applications),
    },
    Item {
        menu: "Go",
        section: 2,
        id: "go-to-folder",
        label: "Go to Folder…",
        needs: Needs::Focused,
        route: files("toolbar.edit-location"),
    },
    Item {
        menu: "Go",
        section: 2,
        id: "connect",
        label: "Connect to Server…",
        needs: Needs::Always,
        route: Route::Show(Place::Connect),
    },
    Item {
        menu: "Window",
        section: 0,
        id: "minimize",
        label: "Minimize",
        needs: Needs::Focused,
        route: Route::Wm(Wm::Minimize),
    },
    Item {
        menu: "Window",
        section: 0,
        id: "zoom",
        label: "Zoom",
        needs: Needs::Focused,
        route: Route::Wm(Wm::Maximize),
    },
    Item {
        menu: "Window",
        section: 1,
        id: "cycle",
        label: "Cycle Through Windows",
        needs: Needs::Two,
        route: Route::Wm(Wm::Cycle),
    },
    Item {
        menu: "Window",
        section: 1,
        id: "progress",
        label: "Show Progress Window",
        needs: Needs::Always,
        route: Route::Nautilus("show-file-transfers"),
    },
    Item {
        menu: "Window",
        section: 1,
        id: "previous-tab",
        label: "Show Previous Tab",
        needs: Needs::Focused,
        route: files("folio.previous-tab"),
    },
    Item {
        menu: "Window",
        section: 1,
        id: "next-tab",
        label: "Show Next Tab",
        needs: Needs::Focused,
        route: files("folio.next-tab"),
    },
    Item {
        menu: "Window",
        section: 2,
        id: "bring-all-to-front",
        label: "Bring All to Front",
        needs: Needs::Windows,
        route: Route::Wm(Wm::BringAll),
    },
    Item {
        menu: "Help",
        section: 0,
        id: "help",
        label: "Folio Help",
        needs: Needs::Never,
        route: Route::Disabled,
    },
    Item {
        menu: "Help",
        section: 0,
        id: "shortcuts",
        label: "Keyboard Shortcuts",
        needs: Needs::Always,
        route: Route::Nautilus("shortcuts"),
    },
];

type WmHandler = Box<dyn Fn(Wm)>;
type FocusHandler = Box<dyn Fn(bool)>;

pub struct Folio {
    actions: gio::SimpleActionGroup,
    model: gio::Menu,
    nautilus_focused: Cell<bool>,
    /// Bumped when focus leaves Nautilus so a late Enabled reply is dropped.
    query_gen: Cell<u64>,
    query_inflight: Cell<bool>,
    query_pending: Cell<bool>,
    /// Bumped when a Files window takes focus so a late desktop reply is dropped.
    desktop_gen: Cell<u64>,
    desktop_inflight: Cell<bool>,
    desktop_pending: Cell<bool>,
    desktop_signals: RefCell<Vec<gio::SignalSubscription>>,
    desktop_focus: RefCell<Option<FocusHandler>>,
    wm: RefCell<Option<WmHandler>>,
}

impl Folio {
    pub fn new(window_list: &gio::Menu) -> Rc<Self> {
        let folio = Rc::new(Self {
            actions: gio::SimpleActionGroup::new(),
            model: gio::Menu::new(),
            nautilus_focused: Cell::new(false),
            query_gen: Cell::new(0),
            query_inflight: Cell::new(false),
            query_pending: Cell::new(false),
            desktop_gen: Cell::new(0),
            desktop_inflight: Cell::new(false),
            desktop_pending: Cell::new(false),
            desktop_signals: RefCell::new(Vec::new()),
            desktop_focus: RefCell::new(None),
            wm: RefCell::new(None),
        });
        folio.build(window_list);
        folio.set_context(false, 0);
        folio.listen_desktop();
        folio
    }

    pub fn model(&self) -> &gio::Menu {
        &self.model
    }

    pub fn actions(&self) -> &gio::SimpleActionGroup {
        &self.actions
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

    pub fn set_context(self: &Rc<Self>, nautilus_focused: bool, nautilus_windows: usize) {
        let was_focused = self.nautilus_focused.get();
        self.nautilus_focused.set(nautilus_focused);
        if nautilus_focused {
            self.desktop_gen.set(self.desktop_gen.get().wrapping_add(1));
            self.desktop_pending.set(false);
        } else {
            self.query_gen.set(self.query_gen.get().wrapping_add(1));
            self.query_pending.set(false);
        }
        for item in ITEMS {
            // While Files stays focused, keep the plugin's last answer for
            // window actions so a window-list update does not flash them on.
            if nautilus_focused && was_focused && files_action(item.route).is_some() {
                continue;
            }
            let enabled = match item.needs {
                Needs::Always => true,
                Needs::Focused | Needs::Surface => nautilus_focused,
                Needs::DesktopOnly => false,
                Needs::Windows => nautilus_windows > 0,
                Needs::Two => nautilus_windows >= 2,
                Needs::Never => false,
            };
            self.set_enabled(item.id, enabled);
        }
        if nautilus_focused {
            self.query_files();
        } else {
            self.query_desktop();
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

    /// Ask the plugin which window actions can run, and gray the rest.
    /// A burst of focus events waits for the call in flight, then asks once more.
    pub(crate) fn query_files(self: &Rc<Self>) {
        if !self.nautilus_focused.get() {
            return;
        }
        if self.query_inflight.get() {
            self.query_pending.set(true);
            return;
        }
        self.query_inflight.set(true);
        self.query_pending.set(false);
        let generation = self.query_gen.get();
        let actions: Vec<String> = ITEMS
            .iter()
            .filter_map(|item| files_action(item.route).map(str::to_owned))
            .collect();
        let ids: Vec<&'static str> = ITEMS
            .iter()
            .filter(|item| files_action(item.route).is_some())
            .map(|item| item.id)
            .collect();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let reply = files_enabled(actions).await;
            let Some(folio) = weak.upgrade() else {
                return;
            };
            folio.query_inflight.set(false);
            if folio.query_gen.get() != generation || !folio.nautilus_focused.get() {
                return;
            }
            match reply {
                Ok(flags) => {
                    for (id, enabled) in ids.into_iter().zip(flags) {
                        folio.set_enabled(id, enabled);
                    }
                }
                Err(err) => {
                    glib::g_warning!("elavo-panel", "folio files: {err}");
                    for id in ids {
                        folio.set_enabled(id, false);
                    }
                }
            }
            if folio.query_pending.get() {
                folio.query_files();
            }
        });
    }

    /// Ask the desktop which file commands can run on the current selection.
    fn query_desktop(self: &Rc<Self>) {
        if self.nautilus_focused.get() {
            return;
        }
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
            let Some(command) = desktop_command(item.route) else {
                continue;
            };
            if !matches!(item.needs, Needs::Surface | Needs::DesktopOnly) {
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
            if folio.desktop_gen.get() != generation || folio.nautilus_focused.get() {
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
                    if folio.nautilus_focused.get() {
                        return;
                    }
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
            Route::Nautilus(action) => nautilus_action(action),
            Route::FileOp(method) => file_op(method),
            Route::Show(place) => {
                if let Some(uri) = place.uri() {
                    show_folder(uri);
                }
            }
            Route::Home => {
                if self.nautilus_focused.get() {
                    files_call("win.go-home", None);
                } else if let Some(uri) = home_uri() {
                    show_folder(uri);
                }
            }
            Route::Files(action) => {
                if self.nautilus_focused.get() {
                    files_call(action, None);
                } else if let Some(command) = desktop_command(Route::Files(action)) {
                    desktop_call(command);
                }
            }
            Route::Desktop(action) => {
                if !self.nautilus_focused.get() {
                    desktop_call(action);
                }
            }
            Route::FilesMode(mode) => files_call("slot.files-view-mode", Some(mode)),
            Route::Wm(wm) => {
                if let Some(handler) = self.wm.borrow().as_ref() {
                    handler(wm);
                }
            }
            Route::Disabled => {}
        }
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
            Place::Computer | Place::Connect => Some("other-locations:///".to_owned()),
            Place::Network => Some("network:///".to_owned()),
            Place::Applications => {
                applications_dir().map(|path| gio::File::for_path(path).uri().to_string())
            }
        }
    }
}

fn nautilus_action(action: &'static str) {
    glib::spawn_future_local(async move {
        let call = async {
            let bus = gio::bus_get_future(gio::BusType::Session).await?;
            let text = format!("('{action}', @av [], @a{{sv}} {{}})");
            let parameters = glib::Variant::parse(None, &text).map_err(|err| {
                glib::Error::new(gio::IOErrorEnum::InvalidArgument, err.message())
            })?;
            bus.call_future(
                Some(NAUTILUS),
                NAUTILUS_PATH,
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
            glib::g_warning!("elavo-panel", "nautilus {action}: {err}");
        }
    });
}

fn file_op(method: &'static str) {
    glib::spawn_future_local(async move {
        let call = async {
            let bus = gio::bus_get_future(gio::BusType::Session).await?;
            let parameters = if method == "EmptyTrash" {
                (true,).to_variant()
            } else {
                glib::Variant::parse(None, "@a{sv} {}").map_err(|err| {
                    glib::Error::new(gio::IOErrorEnum::InvalidArgument, err.message())
                })?
            };
            bus.call_future(
                Some(NAUTILUS),
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
            glib::g_warning!("elavo-panel", "nautilus {method}: {err}");
        }
    });
}

fn files_call(action: &'static str, target: Option<u32>) {
    glib::spawn_future_local(async move {
        let call = async {
            let bus = gio::bus_get_future(gio::BusType::Session).await?;
            // D-Bus rejects an empty tuple, so "no target" is an empty array.
            let target = match target {
                Some(mode) => vec![mode.to_variant()],
                None => Vec::new(),
            };
            bus.call_future(
                Some(FOLIO_FILES),
                FOLIO_FILES_PATH,
                FOLIO_FILES,
                "Activate",
                Some(&(action.to_string(), target).to_variant()),
                None,
                gio::DBusCallFlags::NONE,
                -1,
            )
            .await
        };
        if let Err(err) = call.await {
            glib::g_warning!("elavo-panel", "folio files {action}: {err}");
        }
    });
}

async fn files_enabled(actions: Vec<String>) -> Result<Vec<bool>, glib::Error> {
    let bus = gio::bus_get_future(gio::BusType::Session).await?;
    let reply = bus
        .call_future(
            Some(FOLIO_FILES),
            FOLIO_FILES_PATH,
            FOLIO_FILES,
            "Enabled",
            Some(&(actions,).to_variant()),
            None,
            gio::DBusCallFlags::NONE,
            -1,
        )
        .await?;
    let array = reply.child_value(0);
    let mut flags = Vec::with_capacity(array.n_children() as usize);
    for index in 0..array.n_children() {
        flags.push(array.child_value(index).get::<bool>().unwrap_or(false));
    }
    Ok(flags)
}

fn desktop_command(route: Route) -> Option<&'static str> {
    match route {
        Route::Files(action) => Some(match action {
            "view.new-folder" => "new-folder",
            "view.open-with-default-application" => "open",
            "view.properties" => "get-info",
            "view.preview-selection" => "quick-look",
            "view.rename" => "rename",
            "view.move-to-trash" => "move-to-trash",
            "view.cut" => "cut",
            "view.copy" => "copy",
            "view.paste" => "paste",
            "view.select-all" => "select-all",
            _ => return None,
        }),
        Route::Desktop(action) => Some(action),
        _ => None,
    }
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
    fn folio_is_the_desktop_nautilus_and_elavo_windows() {
        assert!(is_folio(None));
        assert!(is_folio(Some(NAUTILUS)));
        assert!(is_folio(Some("io.elavo.Panel")));
        assert!(is_folio(Some("io.elavo.Dock")));
        assert!(!is_folio(Some("io.elavo.SystemSettings")));
        assert!(!is_folio_window("io.elavo.SystemSettings"));
        assert!(is_folio_window("io.elavo.Panel"));
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
            let _ = item.route;
        }
        assert_eq!(seen.len(), ITEMS.len());
    }

    #[test]
    fn files_actions_name_a_group_and_an_action() {
        for item in ITEMS {
            match item.route {
                Route::Files(action) => {
                    let (prefix, name) = action
                        .split_once('.')
                        .unwrap_or_else(|| panic!("{} has no group", item.id));
                    assert!(!prefix.is_empty(), "{}", item.id);
                    assert!(!name.is_empty() && !name.contains('.'), "{}", item.id);
                }
                Route::FilesMode(mode) => assert!(mode == 1 || mode == 2, "{}", item.id),
                _ => {}
            }
        }
    }

    #[test]
    fn desktop_commands_cover_the_file_surface() {
        for item in ITEMS {
            if item.needs == Needs::Surface {
                assert!(
                    desktop_command(item.route).is_some(),
                    "{} has no desktop command",
                    item.id
                );
            }
        }
        assert!(ITEMS.iter().any(|item| {
            item.id == "use-stacks" && item.needs == Needs::DesktopOnly && item.menu == "View"
        }));
    }

    #[test]
    fn window_commands_stay_off_the_desktop() {
        for id in [
            "new-tab",
            "close",
            "make-link",
            "add-to-sidebar",
            "find",
            "icons",
            "list",
            "sidebar",
            "fullscreen",
            "back",
            "forward",
            "enclosing",
            "go-to-folder",
            "minimize",
            "zoom",
            "previous-tab",
            "next-tab",
        ] {
            let item = ITEMS.iter().find(|item| item.id == id).unwrap();
            assert_eq!(item.needs, Needs::Focused, "{id}");
        }
    }
}
