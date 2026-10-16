//! `com.canonical.AppMenu.Registrar`. Qt applications, and GTK3 ones with
//! appmenu-gtk-module, export a `com.canonical.dbusmenu` menu and register it
//! here when this name has an owner, and then stop drawing their own menu
//! bar. On Wayland the window ID they send is not an X11 window (Qt sends 1),
//! so a registration is matched to a window by the process behind the
//! sender: a Flatpak's app ID from its systemd scope, otherwise the process
//! and executable names.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};

const NAME: &str = "com.canonical.AppMenu.Registrar";
const PATH: &str = "/com/canonical/AppMenu/Registrar";
const DBUS_TIMEOUT_MS: i32 = 1000;
const RECOVER_TIMEOUT_MS: i32 = 300;
const XML: &str = r#"<node>
  <interface name="com.canonical.AppMenu.Registrar">
    <method name="RegisterWindow">
      <arg type="u" name="windowId" direction="in"/>
      <arg type="o" name="menuObjectPath" direction="in"/>
    </method>
    <method name="UnregisterWindow">
      <arg type="u" name="windowId" direction="in"/>
    </method>
    <method name="GetMenuForWindow">
      <arg type="u" name="windowId" direction="in"/>
      <arg type="s" name="service" direction="out"/>
      <arg type="o" name="menuObjectPath" direction="out"/>
    </method>
  </interface>
</node>"#;

struct Entry {
    sender: String,
    window: u32,
    path: String,
    /// Lowercase names of the sender's process, compared with `app_id`s.
    names: Vec<String>,
    serial: u64,
}

pub struct Registrar {
    entries: RefCell<Vec<Entry>>,
    serial: Cell<u64>,
    changed: Box<dyn Fn()>,
    /// Kept for the life of the panel.
    keep: RefCell<Vec<gio::SignalSubscription>>,
}

impl Registrar {
    /// `changed` runs after a registration is added or removed.
    pub fn start(changed: impl Fn() + 'static) -> Rc<Self> {
        let registrar = Rc::new(Self {
            entries: RefCell::default(),
            serial: Cell::new(0),
            changed: Box::new(changed),
            keep: RefCell::default(),
        });
        let weak = Rc::downgrade(&registrar);
        glib::spawn_future_local(async move {
            let bus = match gio::bus_get_future(gio::BusType::Session).await {
                Ok(bus) => bus,
                Err(err) => {
                    glib::g_warning!("elavo-panel", "registrar: no session bus: {err}");
                    return;
                }
            };
            if let Some(registrar) = weak.upgrade() {
                registrar.serve(&bus);
            }
        });
        registrar
    }

    /// The menu of the most recent registration whose process matches.
    pub fn find(&self, wanted: &[String]) -> Option<(String, String)> {
        self.entries
            .borrow()
            .iter()
            .filter(|e| e.names.iter().any(|n| wanted.contains(n)))
            .max_by_key(|e| e.serial)
            .map(|e| (e.sender.clone(), e.path.clone()))
    }

    fn serve(self: &Rc<Self>, bus: &gio::DBusConnection) {
        let info = gio::DBusNodeInfo::for_xml(XML)
            .ok()
            .and_then(|node| node.lookup_interface(NAME));
        let Some(info) = info else {
            return;
        };
        let weak = Rc::downgrade(self);
        let registered = bus
            .register_object(PATH, &info)
            .method_call(move |bus, sender, _, _, method, params, invocation| {
                let Some(registrar) = weak.upgrade() else {
                    return;
                };
                registrar.call(&bus, sender.unwrap_or_default(), method, &params, invocation);
            })
            .build();
        if let Err(err) = registered {
            glib::g_warning!("elavo-panel", "registrar: {err}");
            return;
        }
        let weak = Rc::downgrade(self);
        gio::bus_own_name_on_connection(
            bus,
            NAME,
            gio::BusNameOwnerFlags::NONE,
            move |bus, _| {
                if let Some(registrar) = weak.upgrade() {
                    registrar.recover(&bus);
                }
            },
            |_, _| glib::g_warning!("elavo-panel", "registrar: {NAME} is owned elsewhere"),
        );
        // Drop an application's menus when it leaves the bus.
        let weak = Rc::downgrade(self);
        let gone = bus.subscribe_to_signal(
            Some("org.freedesktop.DBus"),
            Some("org.freedesktop.DBus"),
            Some("NameOwnerChanged"),
            Some("/org/freedesktop/DBus"),
            None,
            gio::DBusSignalFlags::NONE,
            move |signal| {
                let Some((name, _, new)) = signal.parameters.get::<(String, String, String)>()
                else {
                    return;
                };
                if !name.starts_with(':') || !new.is_empty() {
                    return;
                }
                if let Some(registrar) = weak.upgrade() {
                    registrar.remove(|e| e.sender == name);
                }
            },
        );
        self.keep.borrow_mut().push(gone);
    }

    /// Applications register once, when their menu bar is created. Ones that
    /// registered with an earlier panel still export their menus, so find
    /// them where Qt (`/MenuBar/<n>`) and libdbusmenu (`/com/canonical/menu/<n>`)
    /// put them.
    fn recover(self: &Rc<Self>, bus: &gio::DBusConnection) {
        let weak = Rc::downgrade(self);
        let bus = bus.clone();
        glib::spawn_future_local(async move {
            let names = bus
                .call_future(
                    Some("org.freedesktop.DBus"),
                    "/org/freedesktop/DBus",
                    "org.freedesktop.DBus",
                    "ListNames",
                    None,
                    glib::VariantTy::new("(as)").ok(),
                    gio::DBusCallFlags::NONE,
                    DBUS_TIMEOUT_MS,
                )
                .await
                .ok()
                .and_then(|reply| reply.get::<(Vec<String>,)>())
                .map(|(names,)| names)
                .unwrap_or_default();
            let own = bus.unique_name().map(|n| n.to_string());
            for sender in names.into_iter().filter(|n| n.starts_with(':')) {
                if Some(&sender) == own.as_ref() {
                    continue;
                }
                let weak = weak.clone();
                let bus = bus.clone();
                glib::spawn_future_local(async move {
                    for root in ["/MenuBar", "/com/canonical/menu"] {
                        let menus = exported_menus(&bus, &sender, root).await;
                        if menus.is_empty() {
                            continue;
                        }
                        let names = process_names(&bus, &sender).await;
                        let Some(registrar) = weak.upgrade() else {
                            return;
                        };
                        for (window, path) in menus {
                            registrar.add(sender.clone(), window, path, names.clone());
                        }
                    }
                });
            }
        });
    }

    fn call(
        self: &Rc<Self>,
        bus: &gio::DBusConnection,
        sender: &str,
        method: &str,
        params: &glib::Variant,
        invocation: gio::DBusMethodInvocation,
    ) {
        match method {
            "RegisterWindow" => {
                let Some((window, path)) = params.get::<(u32, glib::variant::ObjectPath)>() else {
                    return;
                };
                invocation.return_value(None);
                let weak = Rc::downgrade(self);
                let bus = bus.clone();
                let sender = sender.to_owned();
                glib::spawn_future_local(async move {
                    let names = process_names(&bus, &sender).await;
                    if let Some(registrar) = weak.upgrade() {
                        registrar.add(sender, window, path.as_str().to_owned(), names);
                    }
                });
            }
            "UnregisterWindow" => {
                let window = params.get::<(u32,)>().map(|(w,)| w);
                invocation.return_value(None);
                self.remove(|e| e.sender == sender && Some(e.window) == window);
            }
            "GetMenuForWindow" => {
                let window = params.get::<(u32,)>().map(|(w,)| w);
                let found = self
                    .entries
                    .borrow()
                    .iter()
                    .filter(|e| Some(e.window) == window)
                    .max_by_key(|e| e.serial)
                    .map(|e| (e.sender.clone(), e.path.clone()));
                let (service, path) = found.unwrap_or_else(|| (String::new(), "/".to_owned()));
                let path = glib::variant::ObjectPath::try_from(path)
                    .unwrap_or_else(|_| glib::variant::ObjectPath::try_from("/").unwrap());
                invocation.return_value(Some(&(service, path).to_variant()));
            }
            _ => invocation.return_dbus_error(
                "org.freedesktop.DBus.Error.UnknownMethod",
                &format!("no method {method}"),
            ),
        }
    }

    fn add(&self, sender: String, window: u32, path: String, names: Vec<String>) {
        let serial = self.serial.get() + 1;
        self.serial.set(serial);
        {
            let mut entries = self.entries.borrow_mut();
            entries.retain(|e| !(e.sender == sender && e.window == window));
            entries.push(Entry {
                sender,
                window,
                path,
                names,
                serial,
            });
        }
        (self.changed)();
    }

    fn remove(&self, gone: impl Fn(&Entry) -> bool) {
        let removed = {
            let mut entries = self.entries.borrow_mut();
            let before = entries.len();
            entries.retain(|e| !gone(e));
            entries.len() != before
        };
        if removed {
            (self.changed)();
        }
    }
}

/// Numbered children of `root` that implement dbusmenu, oldest first. The
/// number stands in for the window ID the application registered.
async fn exported_menus(bus: &gio::DBusConnection, sender: &str, root: &str) -> Vec<(u32, String)> {
    let Some(xml) = introspect(bus, sender, root).await else {
        return Vec::new();
    };
    let mut menus: Vec<(u32, String)> = xml
        .split("<node name=\"")
        .skip(1)
        .filter_map(|rest| rest.split_once('"'))
        .filter_map(|(name, _)| {
            let number = u32::from_str_radix(name, 16).ok().or_else(|| name.parse().ok())?;
            Some((number, format!("{root}/{name}")))
        })
        .collect();
    let mut found = Vec::new();
    menus.sort();
    for (window, path) in menus {
        if introspect(bus, sender, &path)
            .await
            .is_some_and(|xml| xml.contains("com.canonical.dbusmenu"))
        {
            found.push((window, path));
        }
    }
    found
}

/// Introspection with a short timeout: some peers answer slowly or never.
async fn introspect(bus: &gio::DBusConnection, sender: &str, path: &str) -> Option<String> {
    bus.call_future(
        Some(sender),
        path,
        "org.freedesktop.DBus.Introspectable",
        "Introspect",
        None,
        glib::VariantTy::new("(s)").ok(),
        gio::DBusCallFlags::NO_AUTO_START,
        RECOVER_TIMEOUT_MS,
    )
    .await
    .ok()
    .and_then(|reply| reply.get::<(String,)>())
    .map(|(xml,)| xml)
}

async fn process_names(bus: &gio::DBusConnection, sender: &str) -> Vec<String> {
    let pid = bus
        .call_future(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "GetConnectionUnixProcessID",
            Some(&(sender,).to_variant()),
            glib::VariantTy::new("(u)").ok(),
            gio::DBusCallFlags::NONE,
            DBUS_TIMEOUT_MS,
        )
        .await
        .ok()
        .and_then(|reply| reply.get::<(u32,)>())
        .map(|(pid,)| pid);
    pid.map(names_of_pid).unwrap_or_default()
}

/// A Flatpak's sender is its xdg-dbus-proxy, which runs in the app's scope,
/// `app-flatpak-<app id>-<n>.scope`.
fn names_of_pid(pid: u32) -> Vec<String> {
    let mut names = Vec::new();
    if let Ok(cgroup) = std::fs::read_to_string(format!("/proc/{pid}/cgroup")) {
        if let Some(id) = cgroup.lines().find_map(flatpak_scope) {
            names.push(id.to_lowercase());
        }
    }
    if let Ok(comm) = std::fs::read_to_string(format!("/proc/{pid}/comm")) {
        names.push(comm.trim().to_lowercase());
    }
    if let Ok(exe) = std::fs::read_link(format!("/proc/{pid}/exe")) {
        if let Some(name) = exe.file_name().and_then(|n| n.to_str()) {
            names.push(name.to_lowercase());
        }
    }
    names.retain(|n| !n.is_empty());
    names.dedup();
    names
}

fn flatpak_scope(line: &str) -> Option<&str> {
    let scope = line.rsplit('/').next()?;
    let rest = scope.strip_prefix("app-flatpak-")?.strip_suffix(".scope")?;
    let (id, instance) = rest.rsplit_once('-')?;
    instance.bytes().all(|b| b.is_ascii_digit()).then_some(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flatpak_scopes() {
        assert_eq!(
            flatpak_scope("0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-flatpak-com.brave.Browser-750805322.scope"),
            Some("com.brave.Browser")
        );
        assert_eq!(flatpak_scope("0::/user.slice/user@1000.service/app.slice/app-foot-1.scope"), None);
    }
}
