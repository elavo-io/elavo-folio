use std::collections::HashSet;
use std::path::{Path, PathBuf};

use gio_unix::prelude::*;
use gio_unix::DesktopAppInfo;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;

use crate::matching::{match_app_id, Candidate};

#[derive(Clone)]
pub struct AppEntry {
    /// Desktop-file ID, including `.desktop`.
    pub id: String,
    pub name: String,
    pub generic_name: Option<String>,
    pub keywords: Vec<String>,
    pub wm_class: Option<String>,
    /// False for `NoDisplay=true` and for `OnlyShowIn`/`NotShowIn` that
    /// excludes this desktop. The dock still uses those to name a window.
    pub shown: bool,
    pub info: DesktopAppInfo,
}

impl AppEntry {
    pub fn icon(&self) -> Option<gio::Icon> {
        self.info.icon()
    }

    pub fn matches(&self, query: &str) -> bool {
        self.score(query).is_some()
    }

    /// Search rank for `query`, higher is better, `None` for no match.
    pub fn score(&self, query: &str) -> Option<u32> {
        crate::ranking::score(
            &self.name,
            self.generic_name.as_deref(),
            &self.keywords,
            &self.id,
            query,
        )
    }

    pub fn has_action(&self, action: &str) -> bool {
        self.info.list_actions().iter().any(|a| a == action)
    }
}

/// Every application desktop file in the XDG data dirs and both Flatpak
/// export trees, first match per desktop ID, in XDG precedence order.
pub struct AppIndex {
    entries: Vec<AppEntry>,
    /// Desktop IDs an AppImage gave itself when it ran from an Applications
    /// entry, and the entry's ID.
    aliases: Vec<(String, String)>,
}

fn application_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![
        glib::user_data_dir().join("applications"),
        glib::user_data_dir().join("flatpak/exports/share/applications"),
    ];
    dirs.extend(
        glib::system_data_dirs()
            .into_iter()
            .map(|d| d.join("applications")),
    );
    dirs.push(PathBuf::from("/var/lib/flatpak/exports/share/applications"));
    // AppImages in /Applications, exported by elavo-applications. The
    // session's XDG_DATA_DIRS names it too, once environment.d has run.
    dirs.push(PathBuf::from(
        "/var/lib/elavo-applications/exports/share/applications",
    ));
    dirs.push(PathBuf::from("/usr/share/applications"));

    let mut seen = HashSet::new();
    dirs.retain(|d| seen.insert(d.clone()));
    dirs
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = read.flatten().map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            collect(root, &path, out);
        } else if path.extension().is_some_and(|e| e == "desktop") {
            if let Ok(rel) = path.strip_prefix(root) {
                let id = rel.to_string_lossy().replace('/', "-");
                out.push((id, path));
            }
        }
    }
}

/// This account's Applications folder.
pub fn home_applications_dir() -> PathBuf {
    glib::home_dir().join("Applications")
}

/// The Applications folder every account shares.
pub fn shared_applications_dir() -> PathBuf {
    std::env::var_os("ELAVO_SHARED_APPLICATIONS")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/Applications"))
}

/// Every `*.eapp` entry's `launch.desktop` in both Applications folders,
/// this account's first. The ID is the entry's `X-Elavo-Id`, the desktop ID
/// its application has everywhere else, so a pin made from an entry and one
/// made from the application are the same pin.
fn bundle_entries() -> Vec<AppEntry> {
    bundles().into_iter().map(|(_, entry)| entry).collect()
}

/// [`bundle_entries`] with each entry's resolved directory.
fn bundles() -> Vec<(PathBuf, AppEntry)> {
    let mut seen = HashSet::new();
    let mut entries = Vec::new();
    for folder in [home_applications_dir(), shared_applications_dir()] {
        let Ok(read) = std::fs::read_dir(&folder) else {
            continue;
        };
        let mut dirs: Vec<PathBuf> = read
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "eapp"))
            .collect();
        dirs.sort();
        for dir in dirs {
            let launch = dir.join("launch.desktop");
            let Some(info) = DesktopAppInfo::from_filename(&launch) else {
                continue;
            };
            let id = match info.string("X-Elavo-Id") {
                Some(id) => format!("{id}.desktop"),
                None => match dir.file_stem() {
                    Some(stem) => format!("{}.desktop", stem.to_string_lossy()),
                    None => continue,
                },
            };
            if seen.insert(id.clone()) {
                let resolved = dir.canonicalize().unwrap_or(dir);
                entries.extend(entry(id, &launch).map(|e| (resolved, e)));
            }
        }
    }
    entries
}

/// The `.eapp` directory a desktop file's program is in, when something
/// other than the Applications service wrote it. AppImages such as Patchy
/// write their own desktop file and icons the first time they run.
fn runs_from_bundle(info: &DesktopAppInfo) -> Option<PathBuf> {
    if info.string("X-Elavo-Id").is_some() {
        return None;
    }
    // `executable()` keeps the quotes around a quoted path.
    let exec = info
        .string("Exec")
        .and_then(|e| glib::shell_parse_argv(e.as_str()).ok())
        .and_then(|argv| argv.into_iter().next())
        .map(PathBuf::from);
    let try_exec = info.string("TryExec").map(|s| PathBuf::from(s.as_str()));
    [exec, try_exec].into_iter().flatten().find_map(|program| {
        program
            .ancestors()
            .find(|a| a.extension().is_some_and(|e| e == "eapp"))
            .map(Path::to_path_buf)
    })
}

/// None for `Hidden=true` and for files that are not applications.
fn entry(id: String, path: &Path) -> Option<AppEntry> {
    let info = DesktopAppInfo::from_filename(path)?;
    Some(AppEntry {
        id,
        name: info.display_name().to_string(),
        generic_name: info.generic_name().map(|s| s.to_string()),
        keywords: info.keywords().iter().map(|k| k.to_string()).collect(),
        wm_class: info.startup_wm_class().map(|s| s.to_string()),
        shown: info.should_show(),
        info,
    })
}

impl AppIndex {
    pub fn load() -> Self {
        let mut seen = HashSet::new();
        let mut entries = Vec::new();
        for dir in application_dirs() {
            let mut files = Vec::new();
            collect(&dir, &dir, &mut files);
            for (id, path) in files {
                if !seen.insert(id.clone()) {
                    continue;
                }
                entries.extend(entry(id, &path));
            }
        }
        let bundles = bundles();
        // A desktop file an AppImage wrote for itself names the entry it
        // runs from, so its windows and recents belong to that entry. Once
        // the entry is gone, so is the application.
        let mut aliases = Vec::new();
        entries.retain(|e| {
            let Some(dir) = runs_from_bundle(&e.info) else {
                return true;
            };
            let dir = dir.canonicalize().ok();
            if let Some((_, bundle)) = bundles.iter().find(|(d, _)| Some(d) == dir.as_ref()) {
                aliases.push((e.id.clone(), bundle.id.clone()));
            }
            false
        });
        // An entry in an Applications folder carries the icon picked for it
        // there, so its launch.desktop stands in for the application's own.
        for (_, bundle) in bundles {
            match entries.iter_mut().find(|e| e.id == bundle.id) {
                Some(existing) => *existing = bundle,
                None => entries.push(bundle),
            }
        }
        entries.sort_by_key(|e| e.name.to_lowercase());
        Self { entries, aliases }
    }

    /// The installs in the Applications folders: each `*.eapp` directory's
    /// `launch.desktop`, in this account's folder and the shared one.
    pub fn load_applications_folders() -> Self {
        let mut entries = bundle_entries();
        entries.sort_by_key(|e| e.name.to_lowercase());
        Self {
            entries,
            aliases: Vec::new(),
        }
    }

    /// Applications the launcher lists.
    pub fn shown(&self) -> impl Iterator<Item = &AppEntry> {
        self.entries.iter().filter(|e| e.shown)
    }

    pub fn get(&self, id: &str) -> Option<&AppEntry> {
        self.entries.iter().find(|e| e.id == id)
    }

    /// Desktop ID for a window's `app_id`. Shown entries win over hidden ones
    /// so `foot` groups with `foot.desktop`, not `foot-server.desktop`. A
    /// hidden match names a window for the panel's menus; the dock shows only
    /// a shown one.
    pub fn match_app_id(&self, app_id: &str) -> Option<&AppEntry> {
        let to_candidates = |shown: bool| -> Vec<Candidate> {
            let aliases = self
                .aliases
                .iter()
                .filter(|(_, to)| self.get(to).is_some_and(|e| e.shown == shown))
                .map(|(alias, _)| Candidate {
                    id: alias,
                    wm_class: None,
                });
            self.entries
                .iter()
                .filter(|e| e.shown == shown)
                .map(|e| Candidate {
                    id: &e.id,
                    wm_class: e.wm_class.as_deref(),
                })
                .chain(aliases)
                .collect()
        };
        let id = match_app_id(app_id, &to_candidates(true))
            .or_else(|| match_app_id(app_id, &to_candidates(false)))?;
        let id = self
            .aliases
            .iter()
            .find(|(alias, _)| alias == id)
            .map_or(id, |(_, to)| to.as_str());
        self.get(id)
    }
}

/// Calls `changed` shortly after an application is installed, removed, or
/// given a new icon. Keep the monitors alive for as long as it should.
pub fn watch(changed: impl Fn() + 'static) -> Vec<gio::FileMonitor> {
    let changed = std::rc::Rc::new(changed);
    let pending = std::rc::Rc::new(std::cell::Cell::new(false));
    let mut dirs = application_dirs();
    dirs.push(home_applications_dir());
    dirs.push(shared_applications_dir());
    dirs.into_iter()
        .filter(|d| d.is_dir())
        .filter_map(|dir| {
            let monitor = gio::File::for_path(&dir)
                .monitor_directory(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE)
                .ok()?;
            let (changed, pending) = (changed.clone(), pending.clone());
            monitor.connect_changed(move |_, _, _, _| {
                if pending.replace(true) {
                    return;
                }
                let (changed, pending) = (changed.clone(), pending.clone());
                glib::timeout_add_local_once(std::time::Duration::from_millis(500), move || {
                    pending.set(false);
                    changed();
                });
            });
            Some(monitor)
        })
        .collect()
}

pub fn launch(info: &DesktopAppInfo, action: Option<&str>) {
    let context = gtk::gdk::Display::default().map(|d| d.app_launch_context());
    match action {
        Some(action) => info.launch_action(action, context.as_ref()),
        None => {
            if let Err(err) = info.launch(&[], context.as_ref()) {
                glib::g_warning!("elavo", "launching {:?} failed: {err}", info.id());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(extra: &str) -> DesktopAppInfo {
        let file = glib::KeyFile::new();
        let text = format!("[Desktop Entry]\nType=Application\nName=Patchy\n{extra}\n");
        file.load_from_data(&text, glib::KeyFileFlags::NONE)
            .unwrap();
        DesktopAppInfo::from_keyfile(&file).unwrap()
    }

    /// GIO skips a desktop file whose program does not exist.
    fn bundle(test: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir()
            .join(format!("elavo-common-{test}-{}", std::process::id()))
            .join("Patchy.eapp");
        std::fs::create_dir_all(&dir).unwrap();
        for program in ["Patchy(2).AppImage", "run"] {
            let path = dir.join(program);
            std::fs::write(&path, "#!/bin/sh\n").unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        dir
    }

    #[test]
    fn a_desktop_file_an_appimage_wrote_for_itself_names_its_entry() {
        let dir = bundle("own");
        let own = info(&format!(
            "Exec=\"{}\" %F",
            dir.join("Patchy(2).AppImage").display()
        ));
        assert_eq!(runs_from_bundle(&own), Some(dir.clone()));
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
    }

    #[test]
    fn exports_and_other_applications_are_left_alone() {
        let dir = bundle("export");
        let export = info(&format!(
            "Exec={} %F\nX-Elavo-Id=elavo-appimage-patchy",
            dir.join("run").display()
        ));
        assert_eq!(runs_from_bundle(&export), None);
        assert_eq!(runs_from_bundle(&info("Exec=sh -c true")), None);
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
    }
}
