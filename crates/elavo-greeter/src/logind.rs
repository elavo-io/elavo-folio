//! Sleep, Restart, and Shut Down, and the session's locked hint.

use gtk::prelude::*;
use gtk::{gio, glib};

const LOGIN1: &str = "org.freedesktop.login1";
const MANAGER_PATH: &str = "/org/freedesktop/login1";
const MANAGER: &str = "org.freedesktop.login1.Manager";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Power {
    Sleep,
    Restart,
    ShutDown,
}

impl Power {
    fn method(self) -> &'static str {
        match self {
            Power::Sleep => "Suspend",
            Power::Restart => "Reboot",
            Power::ShutDown => "PowerOff",
        }
    }
}

async fn call(
    path: &str,
    interface: &str,
    method: &str,
    args: glib::Variant,
) -> Result<glib::Variant, glib::Error> {
    let bus = gio::bus_get_future(gio::BusType::System).await?;
    bus.call_future(
        Some(LOGIN1),
        path,
        interface,
        method,
        Some(&args),
        None,
        gio::DBusCallFlags::ALLOW_INTERACTIVE_AUTHORIZATION,
        -1,
    )
    .await
}

pub fn power(action: Power) {
    glib::spawn_future_local(async move {
        if let Err(e) = call(MANAGER_PATH, MANAGER, action.method(), (true,).to_variant()).await {
            glib::g_warning!("elavo-greeter", "{}: {e}", action.method());
        }
    });
}

/// Tell logind whether the person's graphical session is locked. The lock
/// client runs from a user service, outside that session, so the session
/// is found through the user's display session.
pub async fn set_locked_hint(locked: bool) {
    let display = call(
        "/org/freedesktop/login1/user/self",
        "org.freedesktop.DBus.Properties",
        "Get",
        ("org.freedesktop.login1.User", "Display").to_variant(),
    )
    .await;
    let path = display.ok().and_then(|reply| {
        let value = reply.child_value(0).as_variant()?;
        let path = value.child_value(1);
        path.str().map(str::to_owned)
    });
    let Some(path) = path.filter(|p| p != "/") else {
        return;
    };
    if let Err(e) = call(
        &path,
        "org.freedesktop.login1.Session",
        "SetLockedHint",
        (locked,).to_variant(),
    )
    .await
    {
        glib::g_warning!("elavo-greeter", "SetLockedHint: {e}");
    }
}
