//! Context menus, laid out as the Mac dock's are. Right-click or press and
//! hold opens them.

use std::rc::Rc;

use elavo_dock_settings::{Position, Settings};
use gtk::prelude::*;
use gtk::{gio, glib};

use crate::others::{Display, Sort, View};
use crate::{files, trash, Dock, DockView, ABOUT_KEY, TRASH_KEY};

/// Builds a menu and the actions behind it under the `dock.` prefix.
struct Menu {
    actions: gio::SimpleActionGroup,
    root: gio::Menu,
    section: gio::Menu,
    n: usize,
}

impl Menu {
    fn new() -> Self {
        Self {
            actions: gio::SimpleActionGroup::new(),
            root: gio::Menu::new(),
            section: gio::Menu::new(),
            n: 0,
        }
    }

    fn name(&mut self) -> String {
        self.n += 1;
        format!("a{}", self.n)
    }

    /// Registers an action and returns its detailed name.
    fn action(&mut self, f: impl Fn() + 'static) -> String {
        let name = self.name();
        let action = gio::SimpleAction::new(&name, None);
        action.connect_activate(move |_, _| f());
        self.actions.add_action(&action);
        format!("dock.{name}")
    }

    fn item(&mut self, label: &str, f: impl Fn() + 'static) {
        let action = self.action(f);
        self.section.append(Some(label), Some(&action));
    }

    fn disabled(&mut self, label: &str) {
        let name = self.name();
        let action = gio::SimpleAction::new(&name, None);
        action.set_enabled(false);
        self.actions.add_action(&action);
        self.section
            .append(Some(label), Some(&format!("dock.{name}")));
    }

    fn check(&mut self, menu: &gio::Menu, label: &str, on: bool, f: impl Fn(bool) + 'static) {
        let name = self.name();
        let action = gio::SimpleAction::new_stateful(&name, None, &on.to_variant());
        action.connect_activate(move |action, _| {
            let now = !action
                .state()
                .and_then(|s| s.get::<bool>())
                .unwrap_or(false);
            action.set_state(&now.to_variant());
            f(now);
        });
        self.actions.add_action(&action);
        menu.append(Some(label), Some(&format!("dock.{name}")));
    }

    /// Radio items: `choices` are (value, label); `f` gets the chosen value.
    fn radio(
        &mut self,
        menu: &gio::Menu,
        choices: &[(String, String)],
        current: &str,
        f: impl Fn(&str) + 'static,
    ) {
        let name = self.name();
        let action = gio::SimpleAction::new_stateful(
            &name,
            Some(glib::VariantTy::STRING),
            &current.to_variant(),
        );
        action.connect_activate(move |action, value| {
            if let Some(value) = value.and_then(|v| v.get::<String>()) {
                action.set_state(&value.to_variant());
                f(&value);
            }
        });
        self.actions.add_action(&action);
        for (value, label) in choices {
            let item = gio::MenuItem::new(Some(label), None);
            item.set_action_and_target_value(
                Some(&format!("dock.{name}")),
                Some(&value.to_variant()),
            );
            menu.append_item(&item);
        }
    }

    fn submenu(&mut self, label: &str, menu: &gio::Menu) {
        self.section.append_submenu(Some(label), menu);
    }

    /// End the current section; the menu draws a separator between them.
    fn separator(&mut self) {
        if self.section.n_items() > 0 {
            let section = std::mem::replace(&mut self.section, gio::Menu::new());
            self.root.append_section(None, &section);
        }
    }

    fn add_section(&mut self, label: Option<&str>, section: &gio::Menu) {
        self.separator();
        self.root.append_section(label, section);
    }

    fn popup(mut self, view: &DockView, key: &str) {
        self.separator();
        if self.root.n_items() == 0 {
            return;
        }
        // Nested: submenus open beside the menu, as on a Mac, instead of
        // sliding in place.
        let popover = gtk::PopoverMenu::from_model_full(&self.root, gtk::PopoverMenuFlags::NESTED);
        popover.set_has_arrow(false);
        popover.add_css_class("elavo-dock-menu");
        popover.insert_action_group("dock", Some(&self.actions));
        view.popup(popover.upcast_ref(), key);
    }
}

pub fn show(dock: &Rc<Dock>, view: &DockView, key: &str) {
    let menu = Menu::new();
    if key.starts_with("divider:") {
        divider(dock, menu, view, key);
    } else if let Some(item) = dock.item(key) {
        app(dock, menu, view, key, &item);
    } else if let Some(other) = dock.other(key) {
        other_menu(dock, menu, view, key, &other);
    } else if let Some(window) = dock.window(key) {
        let mut menu = menu;
        let weak = Rc::downgrade(dock);
        let w = window.clone();
        menu.item("Open", move || {
            if let Some(c) = weak.upgrade().as_ref().and_then(|d| d.controller()) {
                c.activate(&w);
            }
        });
        let weak = Rc::downgrade(dock);
        menu.item("Close", move || {
            if let Some(c) = weak.upgrade().as_ref().and_then(|d| d.controller()) {
                c.close(&window);
            }
        });
        menu.popup(view, key);
    } else if key == ABOUT_KEY {
        let mut menu = menu;
        let weak = Rc::downgrade(dock);
        menu.item("Open", move || {
            let Some(dock) = weak.upgrade() else {
                return;
            };
            let Some(window) = dock.about_window() else {
                return;
            };
            if let Some(controller) = dock.controller() {
                controller.activate(&window);
            }
        });
        let weak = Rc::downgrade(dock);
        menu.item("Close", move || {
            let Some(dock) = weak.upgrade() else {
                return;
            };
            let Some(window) = dock.about_window() else {
                return;
            };
            if let Some(controller) = dock.controller() {
                controller.close(&window);
            }
        });
        menu.popup(view, key);
    } else if key == TRASH_KEY {
        let mut menu = menu;
        menu.item("Open", || files::open_uri("trash:///"));
        menu.separator();
        if dock.trash.full() {
            menu.item("Empty Trash", trash::empty);
        } else {
            menu.disabled("Empty Trash");
        }
        menu.popup(view, key);
    }
}

fn divider(dock: &Rc<Dock>, mut menu: Menu, view: &DockView, key: &str) {
    let settings = dock.settings.borrow().clone();
    menu.item(
        if settings.autohide {
            "Turn Hiding Off"
        } else {
            "Turn Hiding On"
        },
        || Settings::update(|s| s.autohide = !s.autohide),
    );
    menu.item(
        if settings.magnification {
            "Turn Magnification Off"
        } else {
            "Turn Magnification On"
        },
        || Settings::update(|s| s.magnification = !s.magnification),
    );
    let positions = gio::Menu::new();
    let choices: Vec<(String, String)> = Position::ALL
        .iter()
        .map(|p| (p.as_str().to_owned(), p.label().to_owned()))
        .collect();
    menu.radio(&positions, &choices, settings.position.as_str(), |value| {
        if let Some(position) = Position::parse(value) {
            Settings::update(|s| s.position = position);
        }
    });
    menu.submenu("Position on Screen", &positions);
    menu.separator();
    let weak = Rc::downgrade(dock);
    menu.item("Dock Settings…", move || {
        if let Some(dock) = weak.upgrade() {
            dock.open_settings();
        }
    });
    menu.popup(view, key);
}

fn app(
    dock: &Rc<Dock>,
    mut menu: Menu,
    view: &DockView,
    key: &str,
    item: &crate::model::Item<wayland_client::backend::ObjectId>,
) {
    let entry = item
        .desktop_id
        .as_deref()
        .and_then(|id| dock.index.borrow().get(id).cloned());
    let windows = dock.windows_of(item);

    // Open windows, most recently used first. The diamond marks a minimized
    // window, as on a Mac.
    if !windows.is_empty() {
        let list = gio::Menu::new();
        let choices: Vec<(String, String)> = windows
            .iter()
            .rev()
            .enumerate()
            .map(|(i, w)| {
                let title = if w.title.is_empty() {
                    entry
                        .as_ref()
                        .map_or("Window".to_owned(), |e| e.name.clone())
                } else {
                    w.title.clone()
                };
                let title = if w.minimized {
                    format!("◆ {title}")
                } else {
                    title
                };
                (i.to_string(), title)
            })
            .collect();
        let current = windows
            .iter()
            .rev()
            .position(|w| w.activated && !w.minimized)
            .map(|i| i.to_string())
            .unwrap_or_default();
        let weak = Rc::downgrade(dock);
        let ordered: Vec<_> = windows.iter().rev().cloned().collect();
        menu.radio(&list, &choices, &current, move |value| {
            let window = value.parse::<usize>().ok().and_then(|i| ordered.get(i));
            if let (Some(dock), Some(window)) = (weak.upgrade(), window) {
                if let Some(controller) = dock.controller() {
                    controller.activate(window);
                }
            }
        });
        menu.add_section(None, &list);
    }

    if let Some(entry) = entry.as_ref() {
        menu.separator();
        for action in entry.info.list_actions() {
            let label = entry.info.action_name(&action).to_string();
            let weak = Rc::downgrade(dock);
            let it = item.clone();
            let action = action.to_string();
            menu.item(&label, move || {
                if let Some(dock) = weak.upgrade() {
                    dock.launch(&it, Some(&action));
                }
            });
        }
    }

    menu.separator();
    if let (Some(entry), Some(id)) = (entry.as_ref(), item.desktop_id.clone()) {
        let options = gio::Menu::new();
        let weak = Rc::downgrade(dock);
        let pin_id = id.clone();
        menu.check(&options, "Keep in Dock", item.pinned, move |on| {
            if let Some(dock) = weak.upgrade() {
                dock.set_pinned(&pin_id, on);
            }
        });
        if item.recent && !item.running() {
            let weak = Rc::downgrade(dock);
            let recent_id = id.clone();
            let remove = menu.action(move || {
                if let Some(dock) = weak.upgrade() {
                    dock.forget_recent(&recent_id);
                }
            });
            options.append(Some("Remove from Dock"), Some(&remove));
        }
        let login = gio::Menu::new();
        let info = entry.info.clone();
        let login_id = id.clone();
        menu.check(
            &login,
            "Open at Login",
            files::opens_at_login(&id),
            move |on| {
                files::set_open_at_login(&info, &login_id, on);
            },
        );
        options.append_section(None, &login);
        if let Some(path) = entry.info.filename() {
            let uri = gio::File::for_path(path).uri().to_string();
            let reveal = gio::Menu::new();
            reveal.append(
                Some("Show in Files"),
                Some(&menu.action(move || files::show_in_files(&uri))),
            );
            options.append_section(None, &reveal);
        }
        menu.submenu("Options", &options);
    }

    menu.separator();
    if item.running() {
        let weak = Rc::downgrade(dock);
        let it = item.clone();
        menu.item("Hide", move || {
            let Some(dock) = weak.upgrade() else {
                return;
            };
            if let Some(controller) = dock.controller() {
                for window in dock.windows_of(&it) {
                    controller.minimize(&window);
                }
            }
        });
        let weak = Rc::downgrade(dock);
        let it = item.clone();
        menu.item("Quit", move || {
            let Some(dock) = weak.upgrade() else {
                return;
            };
            if let Some(controller) = dock.controller() {
                for window in dock.windows_of(&it) {
                    controller.close(&window);
                }
            }
        });
    } else if entry.is_some() {
        let weak = Rc::downgrade(dock);
        let it = item.clone();
        menu.item("Open", move || {
            if let Some(dock) = weak.upgrade() {
                dock.launch(&it, None);
            }
        });
    }
    menu.popup(view, key);
}

fn other_menu(
    dock: &Rc<Dock>,
    mut menu: Menu,
    view: &DockView,
    key: &str,
    other: &crate::others::Other,
) {
    let uri = other.uri.clone();
    let folder = dock.is_folder(&uri);
    let name = dock
        .folders
        .borrow()
        .get(&uri)
        .map(|f| f.name.clone())
        .unwrap_or_default();

    if folder {
        let choose = |menu: &mut Menu,
                      section: &gio::Menu,
                      all: Vec<(String, String)>,
                      current: &str,
                      set: fn(&mut crate::others::Other, &str)| {
            let weak = Rc::downgrade(dock);
            let uri = uri.clone();
            menu.radio(section, &all, current, move |value| {
                if let Some(dock) = weak.upgrade() {
                    dock.update_other(&uri, |o| set(o, value));
                }
            });
        };
        let sort = gio::Menu::new();
        choose(
            &mut menu,
            &sort,
            Sort::ALL
                .iter()
                .map(|s| (s.as_str().to_owned(), s.label().to_owned()))
                .collect(),
            other.sort.as_str(),
            |o, v| o.sort = Sort::parse(v).unwrap_or_default(),
        );
        menu.add_section(Some("Sort by"), &sort);
        let display = gio::Menu::new();
        choose(
            &mut menu,
            &display,
            Display::ALL
                .iter()
                .map(|d| (d.as_str().to_owned(), d.label().to_owned()))
                .collect(),
            other.display.as_str(),
            |o, v| o.display = Display::parse(v).unwrap_or_default(),
        );
        menu.add_section(Some("Display as"), &display);
        let views = gio::Menu::new();
        choose(
            &mut menu,
            &views,
            View::ALL
                .iter()
                .map(|v| (v.as_str().to_owned(), v.label().to_owned()))
                .collect(),
            other.view.as_str(),
            |o, v| o.view = View::parse(v).unwrap_or_default(),
        );
        menu.add_section(Some("View content as"), &views);
    }

    menu.separator();
    let options = gio::Menu::new();
    let weak = Rc::downgrade(dock);
    let remove_uri = uri.clone();
    let remove = menu.action(move || {
        if let Some(dock) = weak.upgrade() {
            dock.remove_other(&remove_uri);
        }
    });
    options.append(Some("Remove from Dock"), Some(&remove));
    let reveal_uri = uri.clone();
    let reveal = gio::Menu::new();
    reveal.append(
        Some("Show in Files"),
        Some(&menu.action(move || files::show_in_files(&reveal_uri))),
    );
    options.append_section(None, &reveal);
    menu.submenu("Options", &options);
    menu.separator();
    let open_uri = uri.clone();
    menu.item(&format!("Open “{name}”"), move || {
        files::open_uri(&open_uri)
    });
    menu.popup(view, key);
}
