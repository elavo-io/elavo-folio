//! System Settings pages Lens can open. System Settings lists them with
//! `elavo-system-settings --list-panes`, one tab-separated line per page:
//! the ID, the title, the icon name, and the search words joined by `;`.
//! An older System Settings prints only the ID and title, which still works.
//! A page opens with `elavo-system-settings --pane ID`.

use gtk::gio;

const SETTINGS: &str = "elavo-system-settings";
const FALLBACK_ICON: &str = "preferences-system-symbolic";

#[derive(Clone, Debug, PartialEq)]
pub struct Page {
    pub id: String,
    pub title: String,
    pub icon: String,
    pub keywords: Vec<String>,
}

impl Page {
    /// Search rank, on the same scale as applications. A query of several
    /// words, like "dark mode", matches when each word matches the page.
    pub fn score(&self, query: &str) -> Option<u32> {
        let query = query.trim();
        if query.is_empty() {
            return None;
        }
        let rank =
            |q: &str| elavo_common::ranking::score(&self.title, None, &self.keywords, &self.id, q);
        if let Some(score) = rank(query) {
            return Some(score);
        }
        let words: Vec<&str> = query.split_whitespace().collect();
        if words.len() < 2 {
            return None;
        }
        // Each word may match a different field; rank by the weakest match,
        // a little below a match on the whole query.
        let scores: Option<Vec<u32>> = words.iter().map(|w| rank(w)).collect();
        scores?.into_iter().min().map(|s| s.saturating_sub(50))
    }
}

pub fn parse(text: &str) -> Vec<Page> {
    text.lines()
        .filter_map(|line| {
            let mut cols = line.split('\t');
            let id = cols.next()?.trim();
            let title = cols.next()?.trim();
            if id.is_empty() || title.is_empty() || id.starts_with('-') {
                return None;
            }
            let icon = cols.next().map(str::trim).filter(|i| !i.is_empty());
            let keywords = cols
                .next()
                .map(|k| {
                    k.split(';')
                        .map(str::trim)
                        .filter(|w| !w.is_empty())
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            Some(Page {
                id: id.to_owned(),
                title: title.to_owned(),
                icon: icon.unwrap_or(FALLBACK_ICON).to_owned(),
                keywords,
            })
        })
        .collect()
}

/// Ask System Settings for its pages without blocking Lens. `done` gets
/// nothing when System Settings is missing or fails.
pub fn load(done: impl FnOnce(Vec<Page>) + 'static) {
    let process = gio::Subprocess::newv(
        &[SETTINGS.as_ref(), "--list-panes".as_ref()],
        gio::SubprocessFlags::STDOUT_PIPE | gio::SubprocessFlags::STDERR_SILENCE,
    );
    let Ok(process) = process else {
        done(Vec::new());
        return;
    };
    process.communicate_utf8_async(None, gio::Cancellable::NONE, move |result| {
        let pages = match result {
            Ok((Some(out), _)) => parse(&out),
            _ => Vec::new(),
        };
        done(pages);
    });
}

pub fn open(id: &str) {
    let _ = gio::Subprocess::newv(
        &[SETTINGS.as_ref(), "--pane".as_ref(), id.as_ref()],
        gio::SubprocessFlags::NONE,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = "wifi\tWi-Fi\tnetwork-wireless-symbolic\twifi;wireless;known networks\n\
        general\tGeneral\tpreferences-system-symbolic\t\n\
        appearance\tAppearance\tpreferences-desktop-appearance-symbolic\tdark;light;accent\n";

    #[test]
    fn reads_the_settings_list() {
        let pages = parse(LIST);
        assert_eq!(pages.len(), 3);
        assert_eq!(pages[0].id, "wifi");
        assert_eq!(pages[0].title, "Wi-Fi");
        assert_eq!(pages[0].keywords, ["wifi", "wireless", "known networks"]);
        assert!(pages[1].keywords.is_empty());
    }

    #[test]
    fn reads_an_older_settings_list() {
        let pages = parse("sound\tSound\nbad\n\n");
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].icon, FALLBACK_ICON);
        assert!(pages[0].keywords.is_empty());
    }

    #[test]
    fn finds_pages_by_title_word_and_phrase() {
        let pages = parse(LIST);
        let find = |q: &str| {
            pages
                .iter()
                .filter(|p| p.score(q).is_some())
                .map(|p| p.id.as_str())
                .collect::<Vec<_>>()
        };
        assert_eq!(find("wi-fi"), ["wifi"]);
        assert_eq!(find("wifi"), ["wifi"]);
        assert_eq!(find("known"), ["wifi"]);
        assert_eq!(find("dark"), ["appearance"]);
        assert_eq!(find("appearance dark"), ["appearance"]);
        assert!(find("dark mode").is_empty());
        assert!(find("").is_empty());
    }
}
