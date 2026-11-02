//! `~/.config/elavo/panel.conf`. System Settings writes `[Status] ShowVolume`.
//! A missing key means the volume icon stays in the bar.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;

const FILE_NAME: &str = "panel.conf";
const GROUP: &str = "Status";
const KEY: &str = "ShowVolume";

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

fn path() -> std::path::PathBuf {
    glib::user_config_dir().join("elavo").join(FILE_NAME)
}

/// Calls `f` when `ShowVolume` changes. Keep the monitor for as long as `f`
/// should run.
pub fn watch(f: impl Fn(bool) + 'static) -> Option<gio::FileMonitor> {
    let file_path = path();
    let dir = file_path.parent()?.to_path_buf();
    let _ = std::fs::create_dir_all(&dir);
    let monitor = gio::File::for_path(&dir)
        .monitor_directory(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE)
        .ok()?;
    let last = Rc::new(RefCell::new(show_volume()));
    monitor.connect_changed(move |_, file, other, _| {
        let ours = |candidate: Option<&gio::File>| {
            candidate
                .and_then(|f| f.basename())
                .is_some_and(|name| name.as_os_str() == FILE_NAME)
        };
        if !ours(Some(file)) && !ours(other) {
            return;
        }
        let shown = show_volume();
        if *last.borrow() != shown {
            last.replace(shown);
            f(shown);
        }
    });
    Some(monitor)
}

#[cfg(test)]
mod tests {
    use super::show_volume_in;

    #[test]
    fn a_missing_key_shows_the_volume_icon() {
        assert!(show_volume_in(""));
        assert!(show_volume_in("[Status]\n"));
        assert!(!show_volume_in("[Status]\nShowVolume=false\n"));
        assert!(show_volume_in("[Status]\nShowVolume=true\n"));
    }
}
