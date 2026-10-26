//! Files the dock opens, reveals, moves, and trashes, and Open at Login.

use std::path::PathBuf;

use gio_unix::DesktopAppInfo;
use gtk::prelude::*;
use gtk::{gdk, gio, glib};

fn context() -> Option<gdk::AppLaunchContext> {
    gdk::Display::default().map(|d| d.app_launch_context())
}

pub fn open_uri(uri: &str) {
    let uri = uri.to_owned();
    let context = context();
    glib::spawn_future_local(async move {
        if let Err(err) = gio::AppInfo::launch_default_for_uri_future(&uri, context.as_ref()).await
        {
            glib::g_warning!("elavo-dock", "opening {uri}: {err}");
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

fn content_type(file: &gio::File) -> String {
    match file.query_info(
        "standard::content-type",
        gio::FileQueryInfoFlags::NONE,
        gio::Cancellable::NONE,
    ) {
        Ok(info) => info
            .content_type()
            .map(|c| c.to_string())
            .unwrap_or_default(),
        Err(_) => gio::content_type_guess(file.path().as_deref(), None)
            .0
            .to_string(),
    }
}

/// Whether the application says it opens every one of `files`.
pub fn app_opens(info: &DesktopAppInfo, files: &[gio::File]) -> bool {
    let supported = info.supported_types();
    if supported.is_empty() {
        return false;
    }
    files.iter().all(|file| {
        let kind = content_type(file);
        supported
            .iter()
            .any(|s| gio::content_type_is_a(&kind, s) || gio::content_type_is_mime_type(&kind, s))
    })
}

pub fn open_with(info: &DesktopAppInfo, files: &[gio::File]) {
    if let Err(err) = info.launch(files, context().as_ref()) {
        glib::g_warning!("elavo-dock", "opening files with {:?}: {err}", info.id());
    }
}

/// Moves on a worker thread: a move across file systems copies.
pub fn move_into(files: Vec<gio::File>, folder: gio::File) {
    let uris: Vec<String> = files.iter().map(|f| f.uri().to_string()).collect();
    let folder = folder.uri().to_string();
    std::thread::spawn(move || {
        let folder = gio::File::for_uri(&folder);
        for uri in uris {
            let file = gio::File::for_uri(&uri);
            let Some(name) = file.basename() else {
                continue;
            };
            let dest = folder.child(name);
            if let Err(err) = file.move_(
                &dest,
                gio::FileCopyFlags::NOFOLLOW_SYMLINKS,
                gio::Cancellable::NONE,
                None,
            ) {
                glib::g_warning!("elavo-dock", "moving {uri}: {err}");
            }
        }
    });
}

pub fn trash(files: Vec<gio::File>) {
    let uris: Vec<String> = files.iter().map(|f| f.uri().to_string()).collect();
    std::thread::spawn(move || {
        for uri in uris {
            if let Err(err) = gio::File::for_uri(&uri).trash(gio::Cancellable::NONE) {
                glib::g_warning!("elavo-dock", "trashing {uri}: {err}");
            }
        }
    });
}

/// Asks the Applications service (elavo-applications) to say that `name`
/// is open and has to be quit before it goes to the Trash.
pub fn tell_open(name: &str) {
    let Ok(bus) = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE) else {
        return;
    };
    let params = (
        "open",
        vec![glib::Variant::from(name).to_variant()],
        std::collections::HashMap::<String, glib::Variant>::new(),
    )
        .to_variant();
    bus.call(
        Some("io.elavo.Applications"),
        "/io/elavo/Applications",
        "org.freedesktop.Application",
        "ActivateAction",
        Some(&params),
        None,
        gio::DBusCallFlags::NONE,
        2000,
        gio::Cancellable::NONE,
        |result| {
            if let Err(err) = result {
                glib::g_warning!("elavo-dock", "telling the Applications service: {err}");
            }
        },
    );
}

fn autostart_path(id: &str) -> PathBuf {
    glib::user_config_dir().join("autostart").join(id)
}

/// Whether an XDG autostart entry for `id` is in effect.
pub fn opens_at_login(id: &str) -> bool {
    let user = autostart_path(id);
    if user.exists() {
        return DesktopAppInfo::from_filename(&user).is_some_and(|i| !i.is_hidden());
    }
    glib::system_config_dirs()
        .iter()
        .any(|dir| dir.join("autostart").join(id).exists())
}

/// Writes or removes `$XDG_CONFIG_HOME/autostart/<id>`. A system entry is
/// turned off with a `Hidden=true` copy, as the autostart spec says.
pub fn set_open_at_login(info: &DesktopAppInfo, id: &str, on: bool) {
    let path = autostart_path(id);
    let result = if on {
        let Some(source) = info.filename() else {
            return;
        };
        let _ = std::fs::remove_file(&path);
        path.parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|_| std::fs::copy(source, &path).map(|_| ()))
    } else {
        let _ = std::fs::remove_file(&path);
        if opens_at_login(id) {
            let text = format!(
                "[Desktop Entry]\nType=Application\nName={}\nHidden=true\n",
                info.name()
            );
            path.parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|_| std::fs::write(&path, text))
        } else {
            Ok(())
        }
    };
    if let Err(err) = result {
        glib::g_warning!("elavo-dock", "autostart {}: {err}", path.display());
    }
}
