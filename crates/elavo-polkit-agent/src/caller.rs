//! Which application asked. polkitd passes the subject's and the caller's
//! process IDs in the details. The subject is often pkexec or a shell, so
//! walk up its parents until one is an installed application.

use std::collections::HashMap;
use std::path::Path;

use gtk::gio;
use gtk::prelude::*;

/// Launchers and wrappers that stand between an application and the
/// request.
const SKIP: &[&str] = &[
    "pkexec",
    "sudo",
    "sh",
    "bash",
    "dash",
    "zsh",
    "fish",
    "env",
    "systemd",
    "flatpak-spawn",
    "bwrap",
    "toolbox",
    "podman",
    "conmon",
    "timeout",
    "nice",
];
const MAX_DEPTH: usize = 8;

pub struct Caller {
    pub name: String,
    pub icon: Option<gio::Icon>,
}

pub fn find(details: &HashMap<String, String>) -> Option<Caller> {
    let pid = ["polkit.subject-pid", "polkit.caller-pid"]
        .iter()
        .filter_map(|key| details.get(*key))
        .find_map(|pid| pid.parse::<i32>().ok())?;
    let apps = gio::AppInfo::all();
    let mut pid = pid;
    for _ in 0..MAX_DEPTH {
        if pid <= 1 {
            break;
        }
        if let Some(app) = flatpak_app(pid) {
            return Some(caller(&app));
        }
        for name in names(pid) {
            if SKIP.contains(&name.as_str()) {
                continue;
            }
            let found = apps.iter().find(|app| {
                app.executable()
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n == name)
            });
            if let Some(app) = found {
                return Some(caller(app));
            }
        }
        pid = parent(pid)?;
    }
    None
}

fn caller(app: &gio::AppInfo) -> Caller {
    Caller {
        name: app.display_name().to_string(),
        icon: app.icon(),
    }
}

/// argv[0]'s file name, then the kernel's short name.
fn names(pid: i32) -> Vec<String> {
    let mut names = Vec::new();
    if let Ok(cmdline) = std::fs::read(format!("/proc/{pid}/cmdline")) {
        if let Some(argv0) = cmdline.split(|b| *b == 0).next() {
            if let Some(name) = Path::new(&*String::from_utf8_lossy(argv0)).file_name() {
                names.push(name.to_string_lossy().into_owned());
            }
        }
    }
    if let Ok(comm) = std::fs::read_to_string(format!("/proc/{pid}/comm")) {
        let comm = comm.trim().to_owned();
        if !comm.is_empty() && !names.contains(&comm) {
            names.push(comm);
        }
    }
    names
}

/// A Flatpak runs in `app-flatpak-<id>-<n>.scope`.
fn flatpak_app(pid: i32) -> Option<gio::AppInfo> {
    let cgroup = std::fs::read_to_string(format!("/proc/{pid}/cgroup")).ok()?;
    let scope = cgroup
        .split('/')
        .find_map(|part| part.trim().strip_prefix("app-flatpak-"))?;
    let id = scope.strip_suffix(".scope").unwrap_or(scope);
    let id = id.rsplit_once('-').map_or(id, |(id, _)| id);
    gio_unix::DesktopAppInfo::new(&format!("{id}.desktop")).map(|app| app.upcast())
}

/// The fourth field of `/proc/<pid>/stat`, after the parenthesized name.
fn parent(pid: i32) -> Option<i32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    parse_parent(&stat)
}

fn parse_parent(stat: &str) -> Option<i32> {
    let rest = &stat[stat.rfind(')')? + 1..];
    rest.split_whitespace().nth(1)?.parse().ok()
}

#[cfg(test)]
mod tests {
    #[test]
    fn parent_skips_names_with_spaces() {
        assert_eq!(super::parse_parent("42 (a b) S 7 42 42 0"), Some(7));
    }
}
