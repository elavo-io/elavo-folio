//! `$XDG_CONFIG_HOME/elavo/dock-pins`: one desktop-file ID per line, in dock
//! order. Blank lines and lines starting with `#` are ignored.
//! `$XDG_STATE_HOME/elavo/dock-recents` has the same format, most recently
//! used first. Reading, writing and watching the pins live in
//! `elavo_common::pins`, which Lens shares.

use std::path::PathBuf;

use elavo_common::pins::{default_pins, pins_path};
pub use elavo_common::pins::{
    lock_files, parse, save, set_pin, write_atomic, write_lines, FILES, LAUNCHER,
};
use gtk::glib;

pub const MAX_RECENTS: usize = 3;

fn recents_path() -> PathBuf {
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| glib::home_dir().join(".local/state"));
    state.join("elavo").join("dock-recents")
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

/// A drop on the far left lands to the right of Files.
pub fn after_files(index: usize) -> usize {
    index.max(1)
}

/// A launch bounce waits for a window. App Launcher only reveals Lens, which
/// is already running and does not open one.
pub fn waits_for_a_window(id: &str) -> bool {
    id != LAUNCHER
}

pub fn load_recents() -> Vec<String> {
    std::fs::read_to_string(recents_path())
        .map(|text| parse(&text))
        .unwrap_or_default()
}

pub fn save_recents(recents: &[String]) {
    write_lines(&recents_path(), recents);
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
