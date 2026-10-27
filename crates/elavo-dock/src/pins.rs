//! `$XDG_CONFIG_HOME/elavo/dock-pins`: one desktop-file ID per line, in dock
//! order. Blank lines and lines starting with `#` are ignored.
//! `$XDG_STATE_HOME/elavo/dock-recents` has the same format, most recently
//! used first.

use std::path::{Path, PathBuf};

use gtk::glib;

/// Files. Always the first pin, and not one the person can move or remove.
pub const FILES: &str = "org.gnome.Nautilus.desktop";
/// Opens Lens on its applications grid. Placed to the right of Files until
/// the person moves or removes it.
pub const LAUNCHER: &str = "io.elavo.AppLauncher.desktop";

const DEFAULT_PINS: &[&str] = &[
    FILES,
    LAUNCHER,
    "org.mozilla.firefox.desktop",
    "foot.desktop",
];

pub const MAX_RECENTS: usize = 3;

fn pins_path() -> PathBuf {
    glib::user_config_dir().join("elavo").join("dock-pins")
}

fn recents_path() -> PathBuf {
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| glib::home_dir().join(".local/state"));
    state.join("elavo").join("dock-recents")
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

pub fn load() -> Vec<String> {
    let path = pins_path();
    let mut pins = match std::fs::read_to_string(&path) {
        Ok(text) => parse(&text),
        Err(_) => default_pins(),
    };
    let offer = !offered_path().is_file();
    let before = pins.clone();
    arrange(&mut pins, offer);
    if pins != before || !path.is_file() {
        save(&pins);
    }
    if offer {
        write_atomic(&offered_path(), "");
    }
    pins
}

fn default_pins() -> Vec<String> {
    DEFAULT_PINS.iter().map(|s| s.to_string()).collect()
}

/// The marker that the launcher has already been offered once. Removing the
/// pin after that stays removed.
fn offered_path() -> PathBuf {
    recents_path().with_file_name("dock-app-launcher-offered")
}

/// Files stays at the front. When `offer_launcher` is set and the launcher
/// is not already pinned, it is inserted directly after Files.
pub fn arrange(pins: &mut Vec<String>, offer_launcher: bool) {
    lock_files(pins);
    if offer_launcher && !pins.iter().any(|p| p == LAUNCHER) {
        pins.insert(1, LAUNCHER.to_owned());
    }
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

/// A drop on the far left lands to the right of Files.
pub fn after_files(index: usize) -> usize {
    index.max(1)
}

/// A launch bounce waits for a window. App Launcher only reveals Lens, which
/// is already running and does not open one.
pub fn waits_for_a_window(id: &str) -> bool {
    id != LAUNCHER
}

pub fn save(pins: &[String]) {
    write_lines(&pins_path(), pins);
}

pub fn load_recents() -> Vec<String> {
    std::fs::read_to_string(recents_path())
        .map(|text| parse(&text))
        .unwrap_or_default()
}

pub fn save_recents(recents: &[String]) {
    write_lines(&recents_path(), recents);
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
        glib::g_warning!("elavo-dock", "saving {}: {err}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::{after_files, arrange, parse, set_pin, waits_for_a_window, FILES, LAUNCHER};

    #[test]
    fn parse_skips_comments_blanks_and_duplicates() {
        let text = "# pins\nfoot.desktop\n\n  org.mozilla.firefox.desktop \nfoot.desktop\n";
        assert_eq!(
            parse(text),
            vec!["foot.desktop", "org.mozilla.firefox.desktop"]
        );
    }

    #[test]
    fn files_stays_first_and_launcher_is_offered_once() {
        let mut pins = vec!["org.mozilla.firefox.desktop".to_owned(), FILES.to_owned()];
        arrange(&mut pins, true);
        assert_eq!(
            pins,
            vec![
                FILES.to_owned(),
                LAUNCHER.to_owned(),
                "org.mozilla.firefox.desktop".to_owned()
            ]
        );
        pins.remove(1);
        arrange(&mut pins, false);
        assert_eq!(
            pins,
            vec![FILES.to_owned(), "org.mozilla.firefox.desktop".to_owned()]
        );
        assert!(!set_pin(&mut pins, FILES, false));
        assert_eq!(pins[0], FILES);
        assert!(set_pin(&mut pins, "foot.desktop", true));
        assert_eq!(pins[0], FILES);
        assert_eq!(pins.last().map(String::as_str), Some("foot.desktop"));
        assert_eq!(after_files(0), 1);
        assert_eq!(after_files(3), 3);
        assert!(!waits_for_a_window(LAUNCHER));
        assert!(waits_for_a_window(FILES));
    }
}
