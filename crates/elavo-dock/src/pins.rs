//! `$XDG_CONFIG_HOME/elavo/dock-pins`: one desktop-file ID per line, in dock
//! order. Blank lines and lines starting with `#` are ignored.
//! `$XDG_STATE_HOME/elavo/dock-recents` has the same format, most recently
//! used first.

use std::path::{Path, PathBuf};

use gtk::glib;

const DEFAULT_PINS: &[&str] = &[
    "org.gnome.Nautilus.desktop",
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
    match std::fs::read_to_string(pins_path()) {
        Ok(text) => parse(&text),
        Err(_) => {
            let pins: Vec<String> = DEFAULT_PINS.iter().map(|s| s.to_string()).collect();
            save(&pins);
            pins
        }
    }
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
    use super::parse;

    #[test]
    fn parse_skips_comments_blanks_and_duplicates() {
        let text = "# pins\nfoot.desktop\n\n  org.mozilla.firefox.desktop \nfoot.desktop\n";
        assert_eq!(
            parse(text),
            vec!["foot.desktop", "org.mozilla.firefox.desktop"]
        );
    }
}
