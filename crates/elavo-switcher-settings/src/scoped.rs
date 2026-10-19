//! Extra shortcuts live in the user's labwc `rc.xml`, between markers, so a
//! later edit does not touch any other keybind. labwc is asked to reload
//! only when that block changes.

use std::path::PathBuf;

use crate::config::ScopedShortcut;

const START: &str = "<!-- elavo-switcher -->";
const END: &str = "<!-- /elavo-switcher -->";
const EMPTY: &str = "<?xml version=\"1.0\"?>\n<labwc_config>\n</labwc_config>\n";

pub fn key_allowed(key: &str) -> bool {
    if key.is_empty() || key.len() > 32 {
        return false;
    }
    const RESERVED: &[&str] = &[
        "A-Tab",
        "A-S-Tab",
        "S-A-Tab",
        "A-grave",
        "A-S-grave",
        "S-A-grave",
    ];
    if RESERVED.contains(&key) {
        return false;
    }
    let mut parts = key.split('-').collect::<Vec<_>>();
    let Some(name) = parts.pop() else {
        return false;
    };
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric()) {
        return false;
    }
    !parts.is_empty() && parts.iter().all(|m| matches!(*m, "A" | "S" | "W" | "C"))
}

fn user_rc() -> PathBuf {
    gtk::glib::user_config_dir().join("labwc").join("rc.xml")
}

/// Replace the marked block. `shortcuts` is already filtered by [`key_allowed`].
pub fn render_document(doc: &str, shortcuts: &[ScopedShortcut]) -> String {
    let base = if doc.contains("<labwc_config") {
        remove_block(doc)
    } else {
        EMPTY.to_owned()
    };
    if shortcuts.is_empty() {
        return base;
    }
    let mut block = format!("  {START}\n");
    for shortcut in shortcuts {
        if !key_allowed(&shortcut.key) {
            continue;
        }
        block.push_str(&format!(
            "  <keybind key=\"{}\">\n    <action name=\"Execute\" command=\"elavo-switcher --scoped {}\" />\n  </keybind>\n",
            shortcut.key,
            shortcut.scope.as_str()
        ));
    }
    block.push_str(&format!("  {END}\n"));
    insert_block(&base, &block)
}

fn remove_block(doc: &str) -> String {
    let Some(start) = doc.find(START) else {
        return doc.to_owned();
    };
    let Some(end_rel) = doc[start..].find(END) else {
        return doc.to_owned();
    };
    let end = start + end_rel + END.len();
    let mut out = String::new();
    out.push_str(doc[..start].trim_end_matches(' ').trim_end_matches('\n'));
    if !out.ends_with('\n') {
        out.push('\n');
    }
    let rest = doc[end..].trim_start_matches('\n');
    out.push_str(rest);
    out
}

fn insert_block(doc: &str, block: &str) -> String {
    if let Some(keyboard) = doc.find("<keyboard>") {
        let at = keyboard + "<keyboard>".len();
        let mut out = String::new();
        out.push_str(&doc[..at]);
        out.push('\n');
        out.push_str(block);
        out.push_str(&doc[at..]);
        return out;
    }
    if let Some(close) = doc.rfind("</labwc_config>") {
        let mut out = String::new();
        out.push_str(&doc[..close]);
        out.push_str("  <keyboard>\n");
        out.push_str(block);
        out.push_str("  </keyboard>\n");
        out.push_str(&doc[close..]);
        return out;
    }
    doc.to_owned()
}

pub fn apply(shortcuts: &[ScopedShortcut]) {
    let path = user_rc();
    let current = std::fs::read_to_string(&path).unwrap_or_else(|_| EMPTY.to_owned());
    let next = render_document(&current, shortcuts);
    if next == current {
        return;
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if std::fs::write(&path, &next).is_err() {
        return;
    }
    reconfigure_labwc();
}

fn my_uid() -> Option<u32> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        let Some(rest) = line.strip_prefix("Uid:") else {
            continue;
        };
        return rest.split_whitespace().next()?.parse().ok();
    }
    None
}

fn reconfigure_labwc() {
    let Some(uid) = my_uid() else {
        return;
    };
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<i32>() else {
            continue;
        };
        let Ok(status) = std::fs::read_to_string(entry.path().join("status")) else {
            continue;
        };
        let same_user = status.lines().any(|line| {
            line.strip_prefix("Uid:")
                .and_then(|rest| rest.split_whitespace().next())
                .and_then(|n| n.parse::<u32>().ok())
                == Some(uid)
        });
        if !same_user {
            continue;
        }
        let Ok(comm) = std::fs::read_to_string(entry.path().join("comm")) else {
            continue;
        };
        if comm.trim() == "labwc" {
            let _ = std::process::Command::new("kill")
                .args(["-HUP", &pid.to_string()])
                .status();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Scope;

    #[test]
    fn reserved_and_bare_keys_are_rejected() {
        assert!(!key_allowed("A-Tab"));
        assert!(!key_allowed("A-grave"));
        assert!(!key_allowed("x"));
        assert!(!key_allowed("A-Tab\""));
        assert!(key_allowed("C-A-m"));
        assert!(key_allowed("W-F1"));
    }

    #[test]
    fn block_replaces_itself_and_leaves_other_binds() {
        let doc = "<labwc_config>\n  <keyboard>\n  <keybind key=\"W-q\"></keybind>\n  <!-- elavo-switcher -->\n  <keybind key=\"C-A-a\"></keybind>\n  <!-- /elavo-switcher -->\n  </keyboard>\n</labwc_config>\n";
        let next = render_document(
            doc,
            &[ScopedShortcut {
                key: "C-A-m".into(),
                scope: Scope::Minimized,
            }],
        );
        assert!(next.contains("elavo-switcher --scoped minimized"));
        assert!(next.contains("W-q"));
        assert!(!next.contains("C-A-a"));
        let cleared = render_document(&next, &[]);
        assert!(!cleared.contains("elavo-switcher"));
        assert!(cleared.contains("W-q"));
    }

    #[test]
    fn empty_document_gains_a_keyboard() {
        let next = render_document(
            "",
            &[ScopedShortcut {
                key: "C-A-w".into(),
                scope: Scope::All,
            }],
        );
        assert!(next.contains("<keyboard>"));
        assert!(next.contains("--scoped all"));
    }
}
