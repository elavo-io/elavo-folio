//! `$XDG_CONFIG_HOME/elavo/dock-pins`: the Dock's pinned applications, one
//! desktop-file ID per line, in Dock order. Blank lines and lines starting
//! with `#` are ignored.
//!
//! The Dock owns the order and its first-run defaults. Lens also pins and
//! unpins from here, and the Dock watches the file, so a change from either
//! shows at once.

use std::path::{Path, PathBuf};

use gtk::prelude::*;
use gtk::{gio, glib};

/// Files. Always the first pin, and not one the person can move or remove.
pub const FILES: &str = "org.gnome.Nautilus.desktop";
/// Opens Lens on its applications grid. Placed to the right of Files until
/// the person moves or removes it.
pub const LAUNCHER: &str = "io.elavo.AppLauncher.desktop";

/// The pins before the person changes any.
pub const DEFAULT_PINS: &[&str] = &[
    FILES,
    LAUNCHER,
    "org.mozilla.firefox.desktop",
    "foot.desktop",
];

pub fn pins_path() -> PathBuf {
    glib::user_config_dir().join("elavo").join("dock-pins")
}

pub fn default_pins() -> Vec<String> {
    DEFAULT_PINS.iter().map(|s| s.to_string()).collect()
}

pub fn parse(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') || lines.iter().any(|p| p == line) {
            continue;
        }
        lines.push(line.to_owned());
    }
    lines
}

/// The saved pins, or `None` when there is no file yet.
pub fn read_saved() -> Option<Vec<String>> {
    std::fs::read_to_string(pins_path())
        .ok()
        .map(|text| parse(&text))
}

/// The saved pins, or the defaults the Dock starts with, Files first.
pub fn load_saved() -> Vec<String> {
    let mut pins = read_saved().unwrap_or_else(default_pins);
    lock_files(&mut pins);
    pins
}

/// `org.gnome.Nautilus.desktop` is index 0.
pub fn lock_files(pins: &mut Vec<String>) {
    pins.retain(|p| p != FILES);
    pins.insert(0, FILES.to_owned());
}

/// Add or remove a pin. Files cannot be removed. Returns whether the list
/// changed.
pub fn set_pin(pins: &mut Vec<String>, id: &str, pinned: bool) -> bool {
    if id == FILES && !pinned {
        return false;
    }
    let present = pins.iter().any(|p| p == id);
    if pinned && !present {
        pins.push(id.to_owned());
    } else if !pinned && present {
        pins.retain(|p| p != id);
    } else {
        return false;
    }
    lock_files(pins);
    true
}

pub fn save(pins: &[String]) {
    write_lines(&pins_path(), pins);
}

/// Call `changed` when the pins file is written, by the Dock or by Lens.
/// The monitor must be kept for as long as the calls are wanted.
pub fn watch(changed: impl Fn() + 'static) -> Option<gio::FileMonitor> {
    let monitor = gio::File::for_path(pins_path())
        .monitor_file(gio::FileMonitorFlags::NONE, gio::Cancellable::NONE)
        .ok()?;
    monitor.connect_changed(move |_, _, _, event| {
        // A save writes a temporary file and renames it over this one,
        // which arrives as a creation.
        if matches!(
            event,
            gio::FileMonitorEvent::Created | gio::FileMonitorEvent::ChangesDoneHint
        ) {
            changed();
        }
    });
    Some(monitor)
}

pub fn write_lines(path: &Path, lines: &[String]) {
    let mut text = lines.join("\n");
    text.push('\n');
    write_atomic(path, &text);
}

pub fn write_atomic(path: &Path, text: &str) {
    let tmp = path.with_extension("tmp");
    let result = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|_| std::fs::write(&tmp, text))
        .and_then(|_| std::fs::rename(&tmp, path));
    if let Err(err) = result {
        glib::g_warning!("elavo", "saving {}: {err}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_skips_comments_blanks_and_duplicates() {
        let text = "# pins\nfoot.desktop\n\n  org.mozilla.firefox.desktop \nfoot.desktop\n";
        assert_eq!(
            parse(text),
            vec!["foot.desktop", "org.mozilla.firefox.desktop"]
        );
    }

    #[test]
    fn files_cannot_be_unpinned_and_stays_first() {
        let mut pins = vec!["foot.desktop".to_owned()];
        assert!(set_pin(&mut pins, "a.desktop", true));
        assert_eq!(pins, [FILES, "foot.desktop", "a.desktop"]);
        assert!(!set_pin(&mut pins, "a.desktop", true));
        assert!(!set_pin(&mut pins, FILES, false));
        assert!(set_pin(&mut pins, "foot.desktop", false));
        assert_eq!(pins, [FILES, "a.desktop"]);
        assert!(!set_pin(&mut pins, "foot.desktop", false));
    }
}
