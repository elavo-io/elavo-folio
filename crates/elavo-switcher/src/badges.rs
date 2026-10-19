//! Badges from `com.canonical.Unity.LauncherEntry`, the same signal the dock uses.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gtk::{gio, glib};

#[derive(Default)]
pub struct Badges {
    entries: Rc<RefCell<HashMap<String, String>>>,
    subscription: RefCell<Option<(gio::DBusConnection, gio::SignalSubscriptionId)>>,
}

impl Badges {
    pub fn pairs(&self) -> Vec<(String, String)> {
        self.entries
            .borrow()
            .iter()
            .map(|(id, count)| (id.clone(), count.clone()))
            .collect()
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
                let Some(id) = uri.strip_prefix("application://") else {
                    return;
                };
                let id = if id.ends_with(".desktop") {
                    id.to_owned()
                } else {
                    format!("{id}.desktop")
                };
                let count = properties
                    .lookup_value("count", None)
                    .and_then(|v| v.get::<i64>().or_else(|| v.get::<i32>().map(i64::from)));
                let visible = properties
                    .lookup_value("count-visible", None)
                    .and_then(|v| v.get::<bool>())
                    .unwrap_or(true);
                let mut entries = entries.borrow_mut();
                if let (Some(count), true) = (count, visible) {
                    if count > 0 {
                        let label = if count > 999 {
                            "999+".to_owned()
                        } else {
                            count.to_string()
                        };
                        entries.insert(id, label);
                        drop(entries);
                        changed();
                        return;
                    }
                }
                if !visible || count.is_some() {
                    entries.remove(&id);
                    drop(entries);
                    changed();
                }
            },
        );
        self.subscription.replace(Some((bus, id)));
    }
}
