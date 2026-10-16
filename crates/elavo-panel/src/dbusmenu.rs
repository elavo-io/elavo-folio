//! A `com.canonical.dbusmenu` menu as a `gio::Menu` for the menu bar. Each
//! item is an action in the `app` group named after its dbusmenu ID, and
//! activating it sends the application a "clicked" event. The model is
//! refilled in place when the application reports a change, so the bar
//! follows it without being rebuilt.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gio, glib};

const IFACE: &str = "com.canonical.dbusmenu";
const DBUS_TIMEOUT_MS: i32 = 1000;
/// Several property signals usually arrive together.
const RELOAD_DELAY: Duration = Duration::from_millis(100);

pub struct DbusMenu {
    bus: gio::DBusConnection,
    sender: String,
    path: String,
    root: gio::Menu,
    actions: gio::SimpleActionGroup,
    /// The panel's Window items, appended to the application's own top-level
    /// Window menu so the bar does not show two.
    window_items: gio::MenuModel,
    has_window_menu: Cell<bool>,
    reloading: Cell<bool>,
    watch: RefCell<Option<gio::SignalSubscription>>,
}

struct Node {
    id: i32,
    props: glib::VariantDict,
    children: Vec<Node>,
}

impl DbusMenu {
    pub async fn load(sender: &str, path: &str, window_items: gio::MenuModel) -> Option<Rc<Self>> {
        let bus = gio::bus_get_future(gio::BusType::Session).await.ok()?;
        let menu = Rc::new(Self {
            bus,
            sender: sender.to_owned(),
            path: path.to_owned(),
            root: gio::Menu::new(),
            actions: gio::SimpleActionGroup::new(),
            window_items,
            has_window_menu: Cell::new(false),
            reloading: Cell::new(false),
            watch: RefCell::default(),
        });
        let layout = menu.layout().await?;
        menu.fill(&layout);
        let weak = Rc::downgrade(&menu);
        let watch = menu.bus.subscribe_to_signal(
            Some(sender),
            Some(IFACE),
            None,
            Some(path),
            None,
            gio::DBusSignalFlags::NONE,
            move |signal| {
                if matches!(
                    signal.signal_name,
                    "LayoutUpdated" | "ItemsPropertiesUpdated"
                ) {
                    if let Some(menu) = weak.upgrade() {
                        menu.queue_reload();
                    }
                }
            },
        );
        menu.watch.replace(Some(watch));
        Some(menu)
    }

    pub fn model(&self) -> gio::MenuModel {
        self.root.clone().upcast()
    }

    pub fn actions(&self) -> gio::ActionGroup {
        self.actions.clone().upcast()
    }

    pub fn has_window_menu(&self) -> bool {
        self.has_window_menu.get()
    }

    fn queue_reload(self: &Rc<Self>) {
        if self.reloading.replace(true) {
            return;
        }
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(RELOAD_DELAY, move || {
            glib::spawn_future_local(async move {
                let Some(menu) = weak.upgrade() else {
                    return;
                };
                menu.reloading.set(false);
                if let Some(layout) = menu.layout().await {
                    menu.fill(&layout);
                }
            });
        });
    }

    /// The whole tree. Submenus some applications fill only when asked
    /// (`AboutToShow`) are asked once, then the tree is read again.
    async fn layout(&self) -> Option<Node> {
        let mut layout = self.get_layout().await?;
        let empty = empty_submenus(&layout);
        if !empty.is_empty() {
            let group = self
                .call("AboutToShowGroup", (empty.clone(),).to_variant(), "(aiai)")
                .await;
            if group.is_none() {
                for id in &empty {
                    self.call("AboutToShow", (*id,).to_variant(), "(b)").await;
                }
            }
            layout = self.get_layout().await?;
        }
        Some(layout)
    }

    async fn get_layout(&self) -> Option<Node> {
        let reply = self
            .call(
                "GetLayout",
                (0i32, -1i32, Vec::<String>::new()).to_variant(),
                "(u(ia{sv}av))",
            )
            .await?;
        parse(&reply.try_child_value(1)?)
    }

    async fn call(&self, method: &str, args: glib::Variant, reply: &str) -> Option<glib::Variant> {
        self.bus
            .call_future(
                Some(&self.sender),
                &self.path,
                IFACE,
                method,
                Some(&args),
                glib::VariantTy::new(reply).ok(),
                gio::DBusCallFlags::NONE,
                DBUS_TIMEOUT_MS,
            )
            .await
            .ok()
    }

    fn fill(&self, layout: &Node) {
        let mut actions = Vec::new();
        self.has_window_menu.set(false);
        let built = self.build(&layout.children, &mut actions, true);
        for name in self.actions.list_actions() {
            self.actions.remove_action(&name);
        }
        for action in &actions {
            self.actions.add_action(action);
        }
        self.root.remove_all();
        self.root.append_section(None, &built);
    }

    /// Separators start a new section. `top` is the menu bar itself.
    fn build(&self, nodes: &[Node], actions: &mut Vec<gio::SimpleAction>, top: bool) -> gio::Menu {
        let menu = gio::Menu::new();
        let mut section = gio::Menu::new();
        for node in nodes {
            if !flag(&node.props, "visible", true) {
                continue;
            }
            if text(&node.props, "type").as_deref() == Some("separator") {
                if section.n_items() > 0 {
                    menu.append_section(None, &section);
                    section = gio::Menu::new();
                }
                continue;
            }
            let label = text(&node.props, "label").unwrap_or_default();
            if text(&node.props, "children-display").as_deref() == Some("submenu")
                || !node.children.is_empty()
            {
                let submenu = self.build(&node.children, actions, false);
                if top && label.replace('_', "").eq_ignore_ascii_case("window") {
                    submenu.append_section(None, &self.window_items);
                    self.has_window_menu.set(true);
                }
                section.append_submenu(Some(&label), &submenu);
                continue;
            }
            let name = format!("i{}", node.id);
            let action = match text(&node.props, "toggle-type").as_deref() {
                Some("checkmark" | "radio") => {
                    let on = node.props.lookup::<i32>("toggle-state").ok().flatten() == Some(1);
                    gio::SimpleAction::new_stateful(&name, None, &on.to_variant())
                }
                _ => gio::SimpleAction::new(&name, None),
            };
            action.set_enabled(flag(&node.props, "enabled", true));
            let (bus, sender, path, id) = (
                self.bus.clone(),
                self.sender.clone(),
                self.path.clone(),
                node.id,
            );
            action.connect_activate(move |_, _| clicked(&bus, &sender, &path, id));
            actions.push(action);
            let item = gio::MenuItem::new(Some(&label), Some(&format!("app.{name}")));
            if let Some(accel) = accel(&node.props) {
                item.set_attribute_value("accel", Some(&accel.to_variant()));
            }
            section.append_item(&item);
        }
        if section.n_items() > 0 {
            menu.append_section(None, &section);
        }
        menu
    }
}

fn clicked(bus: &gio::DBusConnection, sender: &str, path: &str, id: i32) {
    let args = (id, "clicked", 0i32.to_variant(), 0u32).to_variant();
    bus.call(
        Some(sender),
        path,
        IFACE,
        "Event",
        Some(&args),
        None,
        gio::DBusCallFlags::NONE,
        DBUS_TIMEOUT_MS,
        None::<&gio::Cancellable>,
        |_| {},
    );
}

/// `(ia{sv}av)`: an ID, its properties, and its children, each boxed.
fn parse(value: &glib::Variant) -> Option<Node> {
    let id = value.try_child_value(0)?.get::<i32>()?;
    let props = glib::VariantDict::new(Some(&value.try_child_value(1)?));
    let kids = value.try_child_value(2)?;
    let children = (0..kids.n_children())
        .filter_map(|i| kids.child_value(i).as_variant())
        .filter_map(|child| parse(&child))
        .collect();
    Some(Node {
        id,
        props,
        children,
    })
}

fn empty_submenus(node: &Node) -> Vec<i32> {
    let mut ids = Vec::new();
    for child in &node.children {
        if text(&child.props, "children-display").as_deref() == Some("submenu")
            && child.children.is_empty()
        {
            ids.push(child.id);
        }
        ids.extend(empty_submenus(child));
    }
    ids
}

fn text(props: &glib::VariantDict, key: &str) -> Option<String> {
    props.lookup::<String>(key).ok().flatten()
}

fn flag(props: &glib::VariantDict, key: &str, default: bool) -> bool {
    props.lookup::<bool>(key).ok().flatten().unwrap_or(default)
}

/// The first `shortcut` combination as a GTK accelerator, shown beside the
/// item. The application handles the key itself.
fn accel(props: &glib::VariantDict) -> Option<String> {
    let combos = props
        .lookup::<Vec<Vec<String>>>("shortcut")
        .ok()
        .flatten()?;
    let accel = to_accel(combos.first()?)?;
    gtk::accelerator_parse(&accel).is_some().then_some(accel)
}

fn to_accel(keys: &[String]) -> Option<String> {
    let (key, modifiers) = keys.split_last()?;
    let mut accel = String::new();
    for modifier in modifiers {
        accel.push_str(match modifier.as_str() {
            "Control" => "<Control>",
            "Alt" => "<Alt>",
            "Shift" => "<Shift>",
            "Super" => "<Super>",
            _ => return None,
        });
    }
    let key = match key.as_str() {
        "Del" => "Delete".to_owned(),
        "Ins" => "Insert".to_owned(),
        "Esc" => "Escape".to_owned(),
        "PgUp" => "Page_Up".to_owned(),
        "PgDown" => "Page_Down".to_owned(),
        "Backspace" => "BackSpace".to_owned(),
        k if k.chars().count() == 1 => k.to_lowercase(),
        k => k.to_owned(),
    };
    accel.push_str(&key);
    Some(accel)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn shortcuts_become_accelerators() {
        assert_eq!(
            to_accel(&keys(&["Control", "N"])).as_deref(),
            Some("<Control>n")
        );
        assert_eq!(
            to_accel(&keys(&["Control", "Shift", "S"])).as_deref(),
            Some("<Control><Shift>s")
        );
        assert_eq!(to_accel(&keys(&["Del"])).as_deref(), Some("Delete"));
        assert_eq!(to_accel(&keys(&["Hyper", "x"])), None);
    }
}
