//! Launches the dock did not start, so their icons bounce too.
//!
//! GLib announces every launch it spawns from a desktop file with
//! `org.gtk.gio.DesktopAppInfo.Launched` on the session bus. Files, the
//! desktop, Lens, the panel, Applications and `gio launch` all go through it.
//! An application GLib starts over D-Bus (`DBusActivatable=true`) gets no
//! such signal; it takes its bus name instead, which `NameOwnerChanged`
//! reports.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gtk::{gio, glib};

/// Where a launch was seen.
pub enum Seen {
    /// The desktop file GLib launched (a path, or a bare desktop ID).
    DesktopFile(PathBuf),
    /// A well-known bus name that just got its first owner.
    BusName(String),
}

#[derive(Default)]
pub struct Launches {
    subscriptions: Rc<RefCell<Vec<(gio::DBusConnection, gio::SignalSubscriptionId)>>>,
}

/// The desktop file in a `Launched` signal's parameters, `(aysxasa{sv})`.
pub fn launched_desktop_file(params: &glib::Variant) -> Option<PathBuf> {
    let file = params.try_child_value(0)?;
    let bytes: Vec<u8> = file.get()?;
    let bytes = bytes.strip_suffix(&[0]).unwrap_or(&bytes).to_vec();
    if bytes.is_empty() {
        return None;
    }
    use std::os::unix::ffi::OsStringExt;
    Some(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
}

/// The name in a `NameOwnerChanged` that went from no owner to one, unless
/// it is a unique name.
pub fn new_owner(params: &glib::Variant) -> Option<String> {
    let (name, old, new) = params.get::<(String, String, String)>()?;
    (old.is_empty() && !new.is_empty() && !name.starts_with(':')).then_some(name)
}

impl Launches {
    pub fn start(&self, seen: impl Fn(Seen) + 'static) {
        let Ok(bus) = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE) else {
            return;
        };
        let seen = Rc::new(seen);
        let mut subscriptions = self.subscriptions.borrow_mut();
        let on_launch = seen.clone();
        #[allow(deprecated)]
        let id = bus.signal_subscribe(
            None,
            Some("org.gtk.gio.DesktopAppInfo"),
            Some("Launched"),
            Some("/org/gtk/gio/DesktopAppInfo"),
            None,
            gio::DBusSignalFlags::NONE,
            move |_, _, _, _, _, params| {
                if let Some(path) = launched_desktop_file(params) {
                    on_launch(Seen::DesktopFile(path));
                }
            },
        );
        subscriptions.push((bus.clone(), id));
        #[allow(deprecated)]
        let id = bus.signal_subscribe(
            Some("org.freedesktop.DBus"),
            Some("org.freedesktop.DBus"),
            Some("NameOwnerChanged"),
            Some("/org/freedesktop/DBus"),
            None,
            gio::DBusSignalFlags::NONE,
            move |_, _, _, _, _, params| {
                if let Some(name) = new_owner(params) {
                    seen(Seen::BusName(name));
                }
            },
        );
        subscriptions.push((bus, id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gtk::prelude::*;

    #[test]
    fn reads_the_desktop_file_from_launched() {
        let uris: Vec<String> = Vec::new();
        let extras = glib::VariantDict::new(None);
        let params = glib::Variant::tuple_from_iter([
            glib::Variant::array_from_fixed_array(
                b"/usr/share/applications/io.elavo.Music.desktop\0",
            ),
            "".to_variant(),
            42i64.to_variant(),
            uris.to_variant(),
            extras.end(),
        ]);
        assert_eq!(
            launched_desktop_file(&params),
            Some(PathBuf::from(
                "/usr/share/applications/io.elavo.Music.desktop"
            ))
        );
    }

    #[test]
    fn only_a_first_owner_of_a_well_known_name_counts() {
        let v = |a: &str, b: &str, c: &str| (a, b, c).to_variant();
        assert_eq!(
            new_owner(&v("org.gnome.TextEditor", "", ":1.9")).as_deref(),
            Some("org.gnome.TextEditor")
        );
        assert_eq!(new_owner(&v("org.gnome.TextEditor", ":1.9", "")), None);
        assert_eq!(new_owner(&v(":1.9", "", ":1.9")), None);
    }
}
