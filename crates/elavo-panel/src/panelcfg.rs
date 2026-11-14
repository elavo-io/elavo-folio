//! `~/.config/elavo/panel.conf`. System Settings writes `[Status] ShowVolume`,
//! and the battery menu writes `[Status] ShowBatteryPercentage`. A missing
//! key means the volume icon stays in the bar and the percentage does not.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;

const FILE_NAME: &str = "panel.conf";
const GROUP: &str = "Status";
const KEY: &str = "ShowVolume";
const PERCENT_KEY: &str = "ShowBatteryPercentage";

pub fn show_volume() -> bool {
    show_volume_in(&std::fs::read_to_string(path()).unwrap_or_default())
}

/// `true` when the key is missing or the file cannot be read.
pub fn show_volume_in(text: &str) -> bool {
    let file = glib::KeyFile::new();
    if text.is_empty() || file.load_from_data(text, glib::KeyFileFlags::NONE).is_err() {
        return true;
    }
    file.boolean(GROUP, KEY).unwrap_or(true)
}

pub fn show_battery_percentage() -> bool {
    show_battery_percentage_in(&std::fs::read_to_string(path()).unwrap_or_default())
}

/// `false` when the key is missing or the file cannot be read.
pub fn show_battery_percentage_in(text: &str) -> bool {
    let file = glib::KeyFile::new();
    if text.is_empty() || file.load_from_data(text, glib::KeyFileFlags::NONE).is_err() {
        return false;
    }
    file.boolean(GROUP, PERCENT_KEY).unwrap_or(false)
}

/// Writes the key and keeps the rest of the file.
pub fn set_show_battery_percentage(shown: bool) {
    let file = glib::KeyFile::new();
    let path = path();
    let _ = file.load_from_file(
        &path,
        glib::KeyFileFlags::KEEP_COMMENTS | glib::KeyFileFlags::KEEP_TRANSLATIONS,
    );
    file.set_boolean(GROUP, PERCENT_KEY, shown);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(err) = file.save_to_file(&path) {
        glib::g_warning!("elavo-panel", "panel.conf: {err}");
    }
}

fn path() -> std::path::PathBuf {
    glib::user_config_dir().join("elavo").join(FILE_NAME)
}

/// Calls `f` when `ShowVolume` or `ShowBatteryPercentage` changes. Keep the
/// monitor for as long as `f` should run.
pub fn watch(f: impl Fn() + 'static) -> Option<gio::FileMonitor> {
    let file_path = path();
    let dir = file_path.parent()?.to_path_buf();
    let _ = std::fs::create_dir_all(&dir);
    let monitor = gio::File::for_path(&dir)
        .monitor_directory(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE)
        .ok()?;
    let last = Rc::new(RefCell::new((show_volume(), show_battery_percentage())));
    monitor.connect_changed(move |_, file, other, _| {
        let ours = |candidate: Option<&gio::File>| {
            candidate
                .and_then(|f| f.basename())
                .is_some_and(|name| name.as_os_str() == FILE_NAME)
        };
        if !ours(Some(file)) && !ours(other) {
            return;
        }
        let now = (show_volume(), show_battery_percentage());
        if *last.borrow() != now {
            last.replace(now);
            f();
        }
    });
    Some(monitor)
}

#[cfg(test)]
mod tests {
    use super::{show_battery_percentage_in, show_volume_in};

    #[test]
    fn a_missing_key_shows_the_volume_icon() {
        assert!(show_volume_in(""));
        assert!(show_volume_in("[Status]\n"));
        assert!(!show_volume_in("[Status]\nShowVolume=false\n"));
        assert!(show_volume_in("[Status]\nShowVolume=true\n"));
    }

    #[test]
    fn the_battery_percentage_is_off_unless_asked_for() {
        assert!(!show_battery_percentage_in(""));
        assert!(!show_battery_percentage_in("[Status]\nShowVolume=false\n"));
        assert!(show_battery_percentage_in(
            "[Status]\nShowBatteryPercentage=true\n"
        ));
    }
}
