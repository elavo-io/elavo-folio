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
        entries.sort_by_key(|e| e.name.to_lowercase());
        Self { entries }
    }

    /// The installs in the Applications folders: each `*.eapp` directory's
    /// `launch.desktop`. Only this account's folder for now; the shared
    /// folder joins once its path is decided.
    pub fn load_applications_folders() -> Self {
        let mut entries = Vec::new();
        let Ok(read) = std::fs::read_dir(home_applications_dir()) else {
            return Self { entries };
        };
        let mut dirs: Vec<PathBuf> = read
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "eapp"))
            .collect();
        dirs.sort();
        for dir in dirs {
            let Some(stem) = dir.file_stem() else {
                continue;
            };
            let id = format!("{}.desktop", stem.to_string_lossy());
            entries.extend(entry(id, &dir.join("launch.desktop")));
        }
        entries.sort_by_key(|e| e.name.to_lowercase());
        Self { entries }
    }

    /// Applications the launcher lists.
    pub fn shown(&self) -> impl Iterator<Item = &AppEntry> {
        self.entries.iter().filter(|e| e.shown)
    }

    pub fn get(&self, id: &str) -> Option<&AppEntry> {
        self.entries.iter().find(|e| e.id == id)
    }

    /// Desktop ID for a window's `app_id`. Shown entries win over hidden ones
    /// so `foot` groups with `foot.desktop`, not `foot-server.desktop`.
    pub fn match_app_id(&self, app_id: &str) -> Option<&AppEntry> {
        let to_candidates = |shown: bool| -> Vec<Candidate> {
            self.entries
                .iter()
                .filter(|e| e.shown == shown)
                .map(|e| Candidate {
                    id: &e.id,
                    wm_class: e.wm_class.as_deref(),
                })
                .collect()
        };
        let id = match_app_id(app_id, &to_candidates(true))
            .or_else(|| match_app_id(app_id, &to_candidates(false)))?;
        self.get(id)
    }
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
