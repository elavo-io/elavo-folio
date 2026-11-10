//! Opening a file or folder, and showing a file in the file manager. Shared
//! by the Dock and Lens.

use gtk::prelude::*;
use gtk::{gdk, gio, glib};

fn context() -> Option<gdk::AppLaunchContext> {
    gdk::Display::default().map(|d| d.app_launch_context())
}

/// Open `uri` in its default application.
pub fn open_uri(uri: &str) {
    let uri = uri.to_owned();
    let context = context();
    glib::spawn_future_local(async move {
        if let Err(err) = gio::AppInfo::launch_default_for_uri_future(&uri, context.as_ref()).await
        {
            glib::g_warning!("elavo", "opening {uri}: {err}");
        }
    });
}

/// Select `uri` in the file manager, or open its folder if the file manager
/// does not answer `org.freedesktop.FileManager1`.
pub fn show_in_files(uri: &str) {
    let uri = uri.to_owned();
    glib::spawn_future_local(async move {
        let shown = match gio::bus_get_future(gio::BusType::Session).await {
            Ok(bus) => bus
                .call_future(
                    Some("org.freedesktop.FileManager1"),
                    "/org/freedesktop/FileManager1",
                    "org.freedesktop.FileManager1",
                    "ShowItems",
                    Some(&(vec![uri.clone()], "").to_variant()),
                    None,
                    gio::DBusCallFlags::NONE,
                    5000,
                )
                .await
                .is_ok(),
            Err(_) => false,
        };
        if !shown {
            if let Some(parent) = gio::File::for_uri(&uri).parent() {
                open_uri(&parent.uri());
            }
        }
    });
}
