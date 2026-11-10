//! What Lens remembers about the applications opened from it: how often each
//! one was opened, and when it was last. The Top Hit of an empty search
//! comes from this.
//!
//! The file holds one application per line: the launch count, the last
//! launch in Unix seconds, and the desktop file ID, separated by spaces.
//! Lines that do not read that way are skipped, so a damaged file loses only
//! those lines.

use std::path::Path;

/// The most applications remembered. Past this, the ones opened longest ago
/// are forgotten, so the file stays small.
pub const MAX_ENTRIES: usize = 200;

const HEADER: &str = "# Lens launches: count, last launch (Unix seconds), desktop file ID\n";

#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    id: String,
    count: u32,
    last: u64,
}

/// Launch history, kept most recently opened first.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    entries: Vec<Entry>,
}

/// A desktop file ID can be stored on one line with the numbers before it.
fn storable(id: &str) -> bool {
    !id.is_empty() && !id.contains(char::is_whitespace)
}

impl Usage {
    /// Read the file's text. An application listed twice is counted once,
    /// with both counts added and the later launch kept.
    pub fn parse(text: &str) -> Self {
        let mut usage = Self::default();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let parts: Vec<&str> = line.split_whitespace().collect();
            let [count, last, id] = parts[..] else {
                continue;
            };
            let (Ok(count), Ok(last)) = (count.parse::<u32>(), last.parse::<u64>()) else {
                continue;
            };
            if count == 0 || !storable(id) {
                continue;
            }
            match usage.entries.iter_mut().find(|e| e.id == id) {
                Some(entry) => {
                    entry.count = entry.count.saturating_add(count);
                    entry.last = entry.last.max(last);
                }
                None => usage.entries.push(Entry {
                    id: id.to_owned(),
                    count,
                    last,
                }),
            }
        }
        usage.tidy();
        usage
    }

    /// The text [`Usage::parse`] reads back.
    pub fn to_text(&self) -> String {
        let mut text = String::from(HEADER);
        for e in &self.entries {
            text.push_str(&format!("{} {} {}\n", e.count, e.last, e.id));
        }
        text
    }

    /// Note that the application `id` was opened at `now`, in Unix seconds.
    pub fn record(&mut self, id: &str, now: u64) {
        if !storable(id) {
            return;
        }
        match self.entries.iter_mut().find(|e| e.id == id) {
            Some(entry) => {
                entry.count = entry.count.saturating_add(1);
                entry.last = now;
            }
            None => self.entries.push(Entry {
                id: id.to_owned(),
                count: 1,
                last: now,
            }),
        }
        self.tidy();
    }

    /// The application opened most often, of those `present` accepts. Of
    /// two opened as often, the one opened more recently wins.
    pub fn most_launched(&self, present: impl Fn(&str) -> bool) -> Option<&str> {
        // Entries are most recent first, so the first of the highest count
        // is also the most recent of them.
        let mut best: Option<&Entry> = None;
        for entry in self.entries.iter().filter(|e| present(&e.id)) {
            if best.is_none_or(|b| entry.count > b.count) {
                best = Some(entry);
            }
        }
        best.map(|e| e.id.as_str())
    }

    /// Most recent first, ties by ID so the file is stable, and no more
    /// than [`MAX_ENTRIES`].
    fn tidy(&mut self) {
        self.entries
            .sort_by(|a, b| b.last.cmp(&a.last).then_with(|| a.id.cmp(&b.id)));
        self.entries.truncate(MAX_ENTRIES);
    }

    /// The history at `path`. A missing or unreadable file is no history.
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .map(|text| Self::parse(&text))
            .unwrap_or_default()
    }

    /// Write the history to `path` through a temporary file beside it, so
    /// the file is never left half written.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut temporary = path.as_os_str().to_owned();
        temporary.push(".tmp");
        std::fs::write(&temporary, self.to_text())?;
        std::fs::rename(&temporary, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The IDs in the order the file keeps them: most recently opened first.
    fn ids(usage: &Usage) -> Vec<&str> {
        usage.entries.iter().map(|e| e.id.as_str()).collect()
    }

    #[test]
    fn reads_sound_lines_and_skips_the_rest() {
        let usage = Usage::parse(
            "# comment\n\
             3 100 org.example.Files.desktop\n\
             \n\
             not a line\n\
             x 100 bad-count.desktop\n\
             2 later bad-time.desktop\n\
             0 100 never.desktop\n\
             1 2 3 too-many.desktop\n\
             5 50 org.example.Web.desktop\n\
             1 200 org.example.Files.desktop\n",
        );
        assert_eq!(
            ids(&usage),
            ["org.example.Files.desktop", "org.example.Web.desktop"]
        );
        assert_eq!(usage.entries[0].count, 4);
        assert_eq!(usage.entries[0].last, 200);
        assert_eq!(Usage::parse(""), Usage::default());
    }

    #[test]
    fn text_reads_back_the_same() {
        let mut usage = Usage::default();
        usage.record("a.desktop", 10);
        usage.record("b.desktop", 20);
        usage.record("a.desktop", 30);
        assert_eq!(Usage::parse(&usage.to_text()), usage);
    }

    #[test]
    fn recording_counts_and_moves_to_the_front() {
        let mut usage = Usage::default();
        usage.record("a.desktop", 10);
        usage.record("b.desktop", 20);
        assert_eq!(ids(&usage), ["b.desktop", "a.desktop"]);
        usage.record("a.desktop", 30);
        assert_eq!(ids(&usage), ["a.desktop", "b.desktop"]);
        assert_eq!(usage.entries[0].count, 2);
        usage.record("", 40);
        usage.record("has space.desktop", 40);
        assert_eq!(usage.entries.len(), 2);
    }

    #[test]
    fn most_launched_breaks_ties_by_recency() {
        let mut usage = Usage::parse("3 100 a.desktop\n3 300 b.desktop\n1 400 c.desktop\n");
        assert_eq!(usage.most_launched(|_| true), Some("b.desktop"));
        assert_eq!(
            usage.most_launched(|id| id != "b.desktop"),
            Some("a.desktop")
        );
        usage.record("a.desktop", 500);
        assert_eq!(usage.most_launched(|_| true), Some("a.desktop"));
        assert_eq!(usage.most_launched(|_| false), None);
        assert_eq!(Usage::default().most_launched(|_| true), None);
    }

    #[test]
    fn forgets_the_oldest_past_the_cap() {
        let mut usage = Usage::default();
        for i in 0..(MAX_ENTRIES as u64 + 10) {
            usage.record(&format!("app{i}.desktop"), i);
        }
        assert_eq!(usage.entries.len(), MAX_ENTRIES);
        assert_eq!(ids(&usage).first().copied(), Some("app209.desktop"));
        assert!(ids(&usage).iter().all(|id| *id != "app0.desktop"));
    }

    #[test]
    fn saves_and_loads_through_a_file() {
        let dir = std::env::temp_dir().join(format!("elavo-lens-usage-{}", std::process::id()));
        let path = dir.join("elavo").join("lens-usage");
        assert_eq!(Usage::load(&path), Usage::default());
        let mut usage = Usage::default();
        usage.record("a.desktop", 10);
        usage.save(&path).unwrap();
        assert_eq!(Usage::load(&path), usage);
        assert!(!dir.join("elavo").join("lens-usage.tmp").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
