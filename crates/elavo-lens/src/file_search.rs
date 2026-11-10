//! Files in search: recently used files, and files found by name in the
//! home folder's usual places. There is no indexer. Each time Lens opens,
//! the first search of two or more characters starts one bounded scan off
//! the main thread, and its list is kept until Lens closes.

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};

/// How many folders deep below each place the scan looks.
pub const MAX_DEPTH: usize = 4;
/// The scan stops after this many files and folders, so a huge home folder
/// costs a bounded amount of time and memory.
pub const MAX_ENTRIES: usize = 20_000;
/// Searches shorter than this do not look at files; one letter matches
/// too much to be useful.
pub const MIN_QUERY: usize = 2;
/// Folders that hold thousands of files nobody looks for by name. Hidden
/// folders (`.git`, `.cache`, `.var` with Flatpak data) are skipped too.
pub const SKIPPED_DIRS: &[&str] = &[
    "node_modules",
    "target",
    "__pycache__",
    "site-packages",
    "venv",
];
/// A recently used file ranks this much above an equal match the scan found.
const RECENT_BONUS: u32 = 250;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileEntry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    /// From the recently used list rather than the scan.
    pub recent: bool,
}

pub fn skip_dir(name: &str) -> bool {
    name.starts_with('.') || SKIPPED_DIRS.contains(&name)
}

/// List the files and folders directly in `home`, and those up to
/// [`MAX_DEPTH`] folders down inside each of `places`. Hidden entries and
/// [`SKIPPED_DIRS`] are left out. A symbolic link is listed only when it
/// points inside `home`, and the scan never goes through one, so it cannot
/// leave the home folder or loop.
pub fn scan(home: &Path, places: &[PathBuf]) -> Vec<FileEntry> {
    let Ok(home) = home.canonicalize() else {
        return Vec::new();
    };
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut found = Vec::new();
    let mut queue: VecDeque<(PathBuf, usize)> = VecDeque::new();
    // The home folder itself only to its top level.
    queue.push_back((home.clone(), MAX_DEPTH));
    for place in places {
        match place.canonicalize() {
            Ok(place) if place.starts_with(&home) && place != home => {
                queue.push_back((place, 1));
            }
            _ => {}
        }
    }
    while let Some((dir, depth)) = queue.pop_front() {
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in read.flatten() {
            if found.len() >= MAX_ENTRIES {
                return found;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            let is_dir = if kind.is_symlink() {
                match path.canonicalize() {
                    Ok(target) if target.starts_with(&home) => target.is_dir(),
                    _ => continue,
                }
            } else {
                kind.is_dir()
            };
            if (is_dir && skip_dir(&name)) || name.starts_with('.') {
                continue;
            }
            if !seen.insert(path.clone()) {
                continue;
            }
            if is_dir && !kind.is_symlink() && depth < MAX_DEPTH {
                queue.push_back((path.clone(), depth + 1));
            }
            found.push(FileEntry {
                path,
                name,
                is_dir,
                recent: false,
            });
        }
    }
    found
}

/// The best `limit` matches for `query`, by the shared search rank on the
/// name, recently used files first among equals. The same path is listed
/// once.
pub fn rank(
    query: &str,
    recent: &[FileEntry],
    scanned: &[FileEntry],
    limit: usize,
) -> Vec<FileEntry> {
    let query = query.trim();
    if query.chars().count() < MIN_QUERY {
        return Vec::new();
    }
    let mut seen: HashSet<&Path> = HashSet::new();
    let mut scored: Vec<(u32, &FileEntry)> = recent
        .iter()
        .chain(scanned)
        .filter(|e| seen.insert(&e.path))
        .filter_map(|e| {
            let score = elavo_common::ranking::score(&e.name, None, &[], "", query)?;
            Some((score + if e.recent { RECENT_BONUS } else { 0 }, e))
        })
        .collect();
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| b.1.recent.cmp(&a.1.recent))
            .then_with(|| a.1.name.len().cmp(&b.1.name.len()))
            .then_with(|| a.1.name.cmp(&b.1.name))
    });
    scored
        .into_iter()
        .take(limit)
        .map(|(_, e)| e.clone())
        .collect()
}

/// The folder `path` is in, with the home folder written `~`.
pub fn folder_label(path: &Path, home: &Path) -> String {
    let Some(folder) = path.parent() else {
        return String::new();
    };
    match folder.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => folder.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, recent: bool) -> FileEntry {
        let path = PathBuf::from(path);
        FileEntry {
            name: path.file_name().unwrap().to_string_lossy().into_owned(),
            path,
            is_dir: false,
            recent,
        }
    }

    #[test]
    fn skips_hidden_and_heavy_folders() {
        assert!(skip_dir(".git"));
        assert!(skip_dir(".var"));
        assert!(skip_dir("node_modules"));
        assert!(skip_dir("target"));
        assert!(!skip_dir("Documents"));
    }

    #[test]
    fn folder_labels_use_a_tilde() {
        let home = Path::new("/home/kim");
        assert_eq!(folder_label(Path::new("/home/kim/a.txt"), home), "~");
        assert_eq!(
            folder_label(Path::new("/home/kim/Documents/Work/a.txt"), home),
            "~/Documents/Work"
        );
        assert_eq!(folder_label(Path::new("/srv/a.txt"), home), "/srv");
    }

    #[test]
    fn ranks_by_name_and_prefers_recent_files() {
        let recent = [entry("/h/Old/report draft.txt", true)];
        let scanned = [
            entry("/h/Documents/report.odt", false),
            entry("/h/Documents/reports-2025.ods", false),
            entry("/h/Old/report draft.txt", false),
            entry("/h/Music/song.flac", false),
        ];
        let names: Vec<String> = rank("report", &recent, &scanned, 10)
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(
            names,
            ["report draft.txt", "report.odt", "reports-2025.ods"]
        );
        assert_eq!(rank("report", &recent, &scanned, 1).len(), 1);
        assert!(rank("r", &recent, &scanned, 10).is_empty());
        assert!(rank("zzz", &recent, &scanned, 10).is_empty());
    }

    #[test]
    fn scan_stays_in_home_and_skips_what_it_should() {
        let root = std::env::temp_dir().join(format!("elavo-lens-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join("home");
        let outside = root.join("outside");
        let make = |p: &Path| {
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "").unwrap();
        };
        make(&home.join("notes.txt"));
        make(&home.join(".hidden.txt"));
        make(&home.join("Documents/report.odt"));
        make(&home.join("Documents/a/b/c/d.txt"));
        make(&home.join("Documents/a/b/c/d/too-deep.txt"));
        make(&home.join("Documents/.git/config"));
        make(&home.join("Documents/node_modules/x.js"));
        make(&home.join("Other/not-scanned.txt"));
        make(&outside.join("secret.txt"));
        std::os::unix::fs::symlink(&outside, home.join("Documents/out")).unwrap();
        std::os::unix::fs::symlink(home.join("Other"), home.join("Documents/in")).unwrap();

        let found = scan(&home, &[home.join("Documents"), outside.clone()]);
        let names: HashSet<String> = found.iter().map(|e| e.name.clone()).collect();
        for wanted in [
            "notes.txt",
            "Documents",
            "Other",
            "report.odt",
            "d.txt",
            "in",
        ] {
            assert!(names.contains(wanted), "missing {wanted}");
        }
        for unwanted in [
            ".hidden.txt",
            "too-deep.txt",
            "config",
            "x.js",
            "not-scanned.txt",
            "secret.txt",
            "out",
        ] {
            assert!(!names.contains(unwanted), "listed {unwanted}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
