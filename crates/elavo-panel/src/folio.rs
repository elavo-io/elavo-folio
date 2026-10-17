//! Folio's own menus. The desktop, a Nautilus window, and Folio's own
//! dialogs are one application here, so the bar keeps these menus instead
//! of the focused window's.
//!
//! Nautilus exports almost none of its menu over D-Bus. Items that act on
//! the focused window go through the Folio Files plugin
//! (`io.elavo.FolioFiles`), which runs inside Nautilus and calls the
//! window's own actions.

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

/// Nothing focused, Nautilus, or one of Folio's own windows (the panel's
/// dialogs, Dock Settings).
pub fn is_folio(app_id: Option<&str>) -> bool {
    match app_id {
        None => true,
        Some(NAUTILUS) => true,
        Some(id) => id.starts_with("io.elavo."),
    }
}

/// A window that belongs to Folio: Nautilus, or one of Folio's own programs.
pub fn is_folio_window(app_id: &str) -> bool {
    app_id == NAUTILUS || app_id.starts_with("io.elavo.")
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
        needs: Needs::Focused,
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
        needs: Needs::Focused,
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
        needs: Needs::Focused,
        route: files("view.properties"),
    },
    Item {
        menu: "File",
        section: 2,
        id: "rename",
        label: "Rename…",
        needs: Needs::Focused,
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
        needs: Needs::Focused,
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
        needs: Needs::Focused,
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
        needs: Needs::Focused,
        route: files("view.cut"),
    },
    Item {
        menu: "Edit",
        section: 1,
        id: "copy",
        label: "Copy",
        needs: Needs::Focused,
        route: files("view.copy"),
    },
    Item {
        menu: "Edit",
        section: 1,
        id: "paste",
        label: "Paste",
        needs: Needs::Focused,
        route: files("view.paste"),
    },
    Item {
        menu: "Edit",
        section: 2,
        id: "select-all",
        label: "Select All",
        needs: Needs::Focused,
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

pub struct Folio {
    actions: gio::SimpleActionGroup,
    model: gio::Menu,
    nautilus_focused: Cell<bool>,
    /// Bumped when focus leaves Nautilus so a late Enabled reply is dropped.
    query_gen: Cell<u64>,
    query_inflight: Cell<bool>,
    query_pending: Cell<bool>,
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
            wm: RefCell::new(None),
        });
        folio.build(window_list);
        folio.set_context(false, 0);
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

    pub fn set_context(self: &Rc<Self>, nautilus_focused: bool, nautilus_windows: usize) {
        let was_focused = self.nautilus_focused.get();
        self.nautilus_focused.set(nautilus_focused);
        if !nautilus_focused {
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
                Needs::Focused => nautilus_focused,
                Needs::Windows => nautilus_windows > 0,
                Needs::Two => nautilus_windows >= 2,
                Needs::Never => false,
            };
            self.set_enabled(item.id, enabled);
        }
        if nautilus_focused {
            self.query_files();
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

    fn build(self: &Rc<Self>, window_list: &gio::Menu) {
        let applications = applications_dir().is_some();
        for item in ITEMS {
            let action = gio::SimpleAction::new(item.id, None);
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
            Route::Files(action) => files_call(action, None),
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

fn applications_dir() -> Option<std::path::PathBuf> {
    let path = glib::home_dir().join("Applications");
    path.is_dir().then_some(path)
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
}
