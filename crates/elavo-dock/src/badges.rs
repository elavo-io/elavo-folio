//! Badges, progress, and attention from `com.canonical.Unity.LauncherEntry`,
//! the signal Linux applications send to docks (Electron's `setBadgeCount`,
//! Telegram, Thunderbird, and others). The Mac equivalents are the dock tile
//! badge, the download progress bar, and the bouncing icon.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gtk::{gio, glib};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Entry {
    pub count: Option<i64>,
    pub progress: Option<f64>,
    pub urgent: bool,
}

impl Entry {
    pub fn count_label(&self) -> Option<String> {
        match self.count? {
            n if n <= 0 => None,
            n if n > 999 => Some("999+".to_owned()),
            n => Some(n.to_string()),
        }
    }

    fn is_empty(&self) -> bool {
        *self == Entry::default()
    }
}

#[derive(Default)]
pub struct Badges {
    entries: Rc<RefCell<HashMap<String, Entry>>>,
    subscription: RefCell<Option<(gio::DBusConnection, gio::SignalSubscriptionId)>>,
}

/// `application://foo.desktop` to `foo.desktop`.
fn desktop_id(app_uri: &str) -> Option<String> {
    let id = app_uri.strip_prefix("application://")?;
    Some(if id.ends_with(".desktop") {
        id.to_owned()
    } else {
        format!("{id}.desktop")
    })
}

fn apply(entry: &mut Entry, properties: &glib::VariantDict) {
    let lookup = |key: &str| properties.lookup_value(key, None);
    let flag = |key: &str| lookup(key).and_then(|v| v.get::<bool>());
    if let Some(count) =
        lookup("count").and_then(|v| v.get::<i64>().or_else(|| v.get::<i32>().map(i64::from)))
    {
        entry.count = Some(count);
    }
    if let Some(visible) = flag("count-visible") {
        if !visible {
            entry.count = None;
        } else if entry.count.is_none() {
            entry.count = Some(0);
        }
    }
    if let Some(progress) = lookup("progress").and_then(|v| v.get::<f64>()) {
        entry.progress = Some(progress);
    }
    if flag("progress-visible") == Some(false) {
        entry.progress = None;
    }
    if let Some(urgent) = flag("urgent") {
        entry.urgent = urgent;
    }
}

impl Badges {
    pub fn get(&self, id: &str) -> Option<Entry> {
        self.entries.borrow().get(id).cloned()
    }

    pub fn start(&self, changed: impl Fn() + 'static) {
        let Ok(bus) = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE) else {
            return;
        };
        let entries = self.entries.clone();
        #[allow(deprecated)]
        let id = bus.signal_subscribe(
            None,
            Some("com.canonical.Unity.LauncherEntry"),
            Some("Update"),
            None,
            None,
            gio::DBusSignalFlags::NONE,
            move |_, _, _, _, _, params| {
                let Some((uri, properties)) = params.get::<(String, glib::VariantDict)>() else {
                    return;
                };
                let Some(id) = desktop_id(&uri) else {
                    return;
                };
                {
                    let mut entries = entries.borrow_mut();
                    let entry = entries.entry(id.clone()).or_default();
                    apply(entry, &properties);
                    if entry.is_empty() {
                        entries.remove(&id);
                    }
                }
                changed();
            },
        );
        self.subscription.replace(Some((bus, id)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_to_desktop_id() {
        assert_eq!(
            desktop_id("application://foo.desktop").as_deref(),
            Some("foo.desktop")
        );
        assert_eq!(
            desktop_id("application://org.x.Y").as_deref(),
            Some("org.x.Y.desktop")
        );
        assert_eq!(desktop_id("file:///x"), None);
    }

    #[test]
    fn properties_update_entry() {
        let mut entry = Entry::default();
        let dict = glib::VariantDict::new(None);
        dict.insert("count", 7i64);
        dict.insert("count-visible", true);
        dict.insert("progress", 0.25f64);
        dict.insert("progress-visible", true);
        apply(&mut entry, &dict);
        assert_eq!(entry.count_label().as_deref(), Some("7"));
        assert_eq!(entry.progress, Some(0.25));

        let dict = glib::VariantDict::new(None);
        dict.insert("count-visible", false);
        dict.insert("progress-visible", false);
        dict.insert("urgent", true);
        apply(&mut entry, &dict);
        assert_eq!(entry.count_label(), None);
        assert_eq!(entry.progress, None);
        assert!(entry.urgent);
    }
}
