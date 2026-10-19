//! Which rows Alt+Tab shows. No GTK here, so the order can be tested
//! without a display.

use std::collections::HashSet;

use elavo_switcher_settings::{Layout, Scope, Settings, SortOrder};
#[cfg(test)]
use elavo_switcher_settings::AppRule;

#[derive(Clone, Debug)]
pub struct WinSnap {
    pub key: u64,
    pub title: String,
    pub minimized: bool,
    pub fullscreen: bool,
    pub focus: u64,
    pub outputs: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct AppSnap {
    /// Desktop id when we know it, otherwise the Wayland `app_id`.
    pub key: String,
    pub name: String,
    pub focus: u64,
    pub launch_order: u64,
    pub windows: Vec<WinSnap>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub key: String,
    pub name: String,
    pub title: String,
    pub window_key: Option<u64>,
    pub minimized: bool,
    pub fullscreen: bool,
    /// Commit launches this application instead of activating a window.
    pub launch: bool,
    pub badge: Option<String>,
    pub playing: bool,
    pub letter: Option<char>,
}

pub struct ListInput<'a> {
    pub apps: &'a [AppSnap],
    pub settings: &'a Settings,
    pub scope: Option<Scope>,
    pub focused_key: Option<&'a str>,
    pub output: Option<&'a str>,
    pub text: &'a str,
    pub dock_order: &'a [String],
    /// Installed applications, `(desktop id, name)`, for search.
    pub launchable: &'a [(String, String)],
    /// Recently quit applications, newest first.
    pub closed: &'a [(String, String)],
    pub badges: &'a [(String, String)],
    pub playing: &'a HashSet<String>,
}

pub fn per_window(layout: Layout) -> bool {
    matches!(layout, Layout::List | Layout::Previews)
}

/// The row Alt+Tab highlights first. The focused application is index 0,
/// so one Tab lands on the previous one.
pub fn initial_index(len: usize, backward: bool) -> Option<usize> {
    if len == 0 {
        None
    } else if backward {
        Some(len - 1)
    } else if len > 1 {
        Some(1)
    } else {
        Some(0)
    }
}

/// Windows of one application, most recently focused first. Alt+` walks this
/// list. The order has to stay put for the whole cycle: resorting by focus
/// after each activation makes the window you just left the next one, so the
/// key only ever toggles the two most recent windows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowCycle {
    pub order: Vec<u64>,
    pub index: usize,
}

impl WindowCycle {
    pub fn start(windows: &[WinSnap], forward: bool) -> Option<Self> {
        if windows.len() < 2 {
            return None;
        }
        let mut ranked: Vec<&WinSnap> = windows.iter().collect();
        ranked.sort_by(|a, b| b.focus.cmp(&a.focus).then(a.key.cmp(&b.key)));
        let order: Vec<u64> = ranked.into_iter().map(|window| window.key).collect();
        let index = if forward { 1 } else { order.len() - 1 };
        Some(Self { order, index })
    }

    pub fn current(&self) -> Option<u64> {
        self.order.get(self.index).copied()
    }

    /// Move to the next window. `None` when a window has opened or closed and
    /// the caller should start a new cycle.
    pub fn advance(&mut self, windows: &[WinSnap], forward: bool) -> Option<u64> {
        let mut live: Vec<u64> = windows.iter().map(|window| window.key).collect();
        live.sort_unstable();
        let mut frozen = self.order.clone();
        frozen.sort_unstable();
        if live != frozen || self.order.len() < 2 {
            return None;
        }
        let len = self.order.len();
        let delta = if forward { 1 } else { len - 1 };
        self.index = (self.index + delta) % len;
        self.current()
    }
}

pub fn alt_tab_skipped(app: &AppSnap, settings: &Settings) -> bool {
    let Some(rule) = settings.rules.iter().find(|rule| rule.id == app.key) else {
        return false;
    };
    match rule.skip_fullscreen_only {
        Some(false) => true,
        Some(true) => app
            .windows
            .iter()
            .any(|window| window.focus == app.focus && window.fullscreen),
        None => false,
    }
}

pub fn entries(input: &ListInput<'_>) -> Vec<Entry> {
    let settings = input.settings;
    let per_window = per_window(settings.layout) || matches!(input.scope, Some(Scope::Minimized));
    let mut rows: Vec<Row> = Vec::new();

    for app in input.apps {
        if settings
            .rules
            .iter()
            .any(|rule| rule.hidden && rule.id == app.key)
        {
            continue;
        }
        if matches!(input.scope, Some(Scope::App)) && input.focused_key != Some(app.key.as_str()) {
            continue;
        }
        let windows: Vec<&WinSnap> = app
            .windows
            .iter()
            .filter(|window| {
                if !settings.show_minimized && window.minimized {
                    return false;
                }
                if matches!(input.scope, Some(Scope::Minimized)) && !window.minimized {
                    return false;
                }
                if matches!(input.scope, Some(Scope::Output)) {
                    if let Some(output) = input.output {
                        if !window.outputs.is_empty()
                            && !window.outputs.iter().any(|name| name == output)
                        {
                            return false;
                        }
                    }
                }
                true
            })
            .collect();
        if windows.is_empty() {
            continue;
        }
        if per_window {
            for window in windows {
                rows.push(Row::from_window(app, window, input));
            }
        } else if let Some(window) = windows.iter().max_by_key(|window| window.focus) {
            rows.push(Row::from_window(app, window, input));
        }
    }

    if settings.show_windowless && input.text.is_empty() {
        for id in input.dock_order {
            if rows.iter().any(|row| row.entry.key == *id) {
                continue;
            }
            if settings
                .rules
                .iter()
                .any(|rule| rule.hidden && rule.id == *id)
            {
                continue;
            }
            let name = input
                .launchable
                .iter()
                .find(|(launch_id, _)| launch_id == id)
                .map(|(_, name)| name.clone())
                .unwrap_or_else(|| id.trim_end_matches(".desktop").to_owned());
            rows.push(Row::launch(id.clone(), name, input));
        }
    }

    sort_rows(&mut rows, settings.sort_order, input.dock_order);

    let pinned: Vec<String> = settings
        .rules
        .iter()
        .filter(|rule| rule.pinned)
        .map(|rule| rule.id.clone())
        .collect();
    if !pinned.is_empty() {
        rows.sort_by_key(|row| {
            pinned
                .iter()
                .position(|id| id == &row.entry.key)
                .unwrap_or(usize::MAX)
        });
    }
    if settings.sink_minimized {
        rows.sort_by_key(|row| row.entry.minimized && !row.entry.launch);
    }

    if settings.fuzzy_search && !input.text.is_empty() {
        rows.retain(|row| row.rank > 0);
        if settings.fuzzy_rank_best {
            rows.sort_by(|a, b| b.rank.cmp(&a.rank));
        }
        if settings.search_includes_launchable {
            let mut added = 0;
            for (id, name) in input.launchable {
                if added == 8 || rows.iter().any(|row| row.entry.key == *id) {
                    continue;
                }
                if let Some(rank) = rank(name, "", input.text) {
                    rows.push(Row {
                        entry: Entry {
                            key: id.clone(),
                            name: name.clone(),
                            title: String::new(),
                            window_key: None,
                            minimized: false,
                            fullscreen: false,
                            launch: true,
                            badge: None,
                            playing: false,
                            letter: None,
                        },
                        focus: 0,
                        launch_order: u64::MAX,
                        rank,
                    });
                    added += 1;
                }
            }
            if settings.fuzzy_rank_best {
                rows.sort_by(|a, b| b.rank.cmp(&a.rank));
            }
        }
    }

    if settings.show_recently_closed {
        let mut left = settings.recently_closed_limit.max(0) as usize;
        for (id, name) in input.closed {
            if left == 0 || rows.iter().any(|row| row.entry.key == *id) {
                continue;
            }
            if !input.text.is_empty() && rank(name, "", input.text).is_none() {
                continue;
            }
            rows.push(Row::launch(id.clone(), name.clone(), input));
            left -= 1;
        }
    }

    if settings.letter_hints && input.text.is_empty() {
        let mut used = HashSet::new();
        for row in &mut rows {
            let letter = row
                .entry
                .name
                .chars()
                .find(|c| c.is_ascii_alphabetic())
                .map(|c| c.to_ascii_lowercase())
                .filter(|c| used.insert(*c));
            row.entry.letter = letter;
        }
    }

    rows.into_iter().map(|row| row.entry).collect()
}

struct Row {
    entry: Entry,
    focus: u64,
    launch_order: u64,
    rank: u32,
}

impl Row {
    fn from_window(app: &AppSnap, window: &WinSnap, input: &ListInput<'_>) -> Self {
        let per_window = per_window(input.settings.layout);
        Self {
            focus: if per_window { window.focus } else { app.focus },
            launch_order: app.launch_order,
            rank: rank(&app.name, &window.title, input.text).unwrap_or(0),
            entry: Entry {
                key: app.key.clone(),
                name: app.name.clone(),
                title: window.title.clone(),
                window_key: Some(window.key),
                minimized: window.minimized,
                fullscreen: window.fullscreen,
                launch: false,
                badge: input
                    .settings
                    .show_unread_badges
                    .then(|| {
                        input
                            .badges
                            .iter()
                            .find(|(id, _)| id == &app.key)
                            .map(|(_, count)| count.clone())
                    })
                    .flatten(),
                playing: input.settings.show_audio && input.playing.contains(&app.key),
                letter: None,
            },
        }
    }

    fn launch(id: String, name: String, input: &ListInput<'_>) -> Self {
        Self {
            focus: 0,
            launch_order: u64::MAX,
            rank: rank(&name, "", input.text).unwrap_or(0),
            entry: Entry {
                key: id,
                name,
                title: String::new(),
                window_key: None,
                minimized: false,
                fullscreen: false,
                launch: true,
                badge: None,
                playing: false,
                letter: None,
            },
        }
    }
}

fn rank(name: &str, title: &str, query: &str) -> Option<u32> {
    let query = query.trim();
    if query.is_empty() {
        return None;
    }
    let name = name.to_lowercase();
    let query = query.to_lowercase();
    if name.starts_with(&query) {
        Some(300)
    } else if name.contains(&query) {
        Some(200)
    } else if title.to_lowercase().contains(&query) {
        Some(100)
    } else {
        None
    }
}

fn sort_rows(rows: &mut [Row], order: SortOrder, dock: &[String]) {
    match order {
        SortOrder::RecentApps | SortOrder::RecentWindows => {
            rows.sort_by(|a, b| b.focus.cmp(&a.focus).then(a.entry.name.cmp(&b.entry.name)));
        }
        SortOrder::Alphabetical => rows.sort_by(|a, b| {
            a.entry
                .name
                .to_lowercase()
                .cmp(&b.entry.name.to_lowercase())
        }),
        SortOrder::Launch => rows.sort_by_key(|row| row.launch_order),
        SortOrder::Dock => rows.sort_by_key(|row| {
            dock.iter()
                .position(|id| id == &row.entry.key)
                .unwrap_or(usize::MAX)
        }),
    }
}

/// Jump to the next row whose hint or name starts with `letter`.
pub fn letter_index(entries: &[Entry], letter: char, from: usize) -> Option<usize> {
    let letter = letter.to_ascii_lowercase();
    let matches = |entry: &Entry| {
        entry.letter == Some(letter)
            || entry
                .name
                .chars()
                .next()
                .is_some_and(|c| c.to_ascii_lowercase() == letter)
    };
    entries
        .iter()
        .enumerate()
        .cycle()
        .skip(from.saturating_add(1))
        .take(entries.len())
        .find(|(_, entry)| matches(entry))
        .map(|(index, _)| index)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(key: u64, focus: u64, minimized: bool) -> WinSnap {
        WinSnap {
            key,
            title: format!("w{key}"),
            minimized,
            fullscreen: false,
            focus,
            outputs: vec!["eDP-1".into()],
        }
    }

    fn app(key: &str, focus: u64, windows: Vec<WinSnap>) -> AppSnap {
        AppSnap {
            key: key.into(),
            name: key.trim_end_matches(".desktop").into(),
            focus,
            launch_order: focus,
            windows,
        }
    }

    fn input<'a>(apps: &'a [AppSnap], settings: &'a Settings, dock: &'a [String]) -> ListInput<'a> {
        ListInput {
            apps,
            settings,
            scope: None,
            focused_key: Some("files.desktop"),
            output: Some("eDP-1"),
            text: "",
            dock_order: dock,
            launchable: &[],
            closed: &[],
            badges: &[],
            playing: {
                use std::sync::OnceLock;
                static EMPTY: OnceLock<HashSet<String>> = OnceLock::new();
                EMPTY.get_or_init(HashSet::new)
            },
        }
    }

    #[test]
    fn recent_order_puts_the_focused_app_first() {
        let apps = vec![
            app("files.desktop", 5, vec![window(1, 5, false)]),
            app("foot.desktop", 4, vec![window(2, 4, false)]),
            app("web.desktop", 1, vec![window(3, 1, false)]),
        ];
        let settings = Settings::default();
        let rows = entries(&input(&apps, &settings, &[]));
        assert_eq!(
            rows.iter().map(|row| row.key.as_str()).collect::<Vec<_>>(),
            vec!["files.desktop", "foot.desktop", "web.desktop"]
        );
        assert_eq!(initial_index(rows.len(), false), Some(1));
        assert_eq!(initial_index(rows.len(), true), Some(2));
    }

    #[test]
    fn hidden_and_minimized_follow_the_settings() {
        let apps = vec![
            app("files.desktop", 3, vec![window(1, 3, false)]),
            app("foot.desktop", 2, vec![window(2, 2, true)]),
        ];
        let mut settings = Settings::default();
        settings.rules.push(AppRule {
            id: "files.desktop".into(),
            pinned: false,
            hidden: true,
            skip_fullscreen_only: None,
        });
        settings.show_minimized = false;
        let rows = entries(&input(&apps, &settings, &[]));
        assert!(rows.is_empty());
    }

    #[test]
    fn window_cycle_wraps_and_ignores_a_single_window() {
        assert!(WindowCycle::start(&[window(1, 1, false)], true).is_none());
        let windows = vec![window(1, 5, false), window(2, 4, true), window(3, 1, false)];
        let mut cycle = WindowCycle::start(&windows, true).unwrap();
        assert_eq!(cycle.current(), Some(2));
        assert_eq!(cycle.advance(&windows, true), Some(3));
        assert_eq!(cycle.advance(&windows, true), Some(1));
        assert_eq!(cycle.advance(&windows, true), Some(2));
        assert_eq!(cycle.advance(&windows, false), Some(1));
    }

    #[test]
    fn pinned_apps_lead() {
        let apps = vec![
            app("files.desktop", 5, vec![window(1, 5, false)]),
            app("foot.desktop", 1, vec![window(2, 1, false)]),
        ];
        let mut settings = Settings::default();
        settings.rules.push(AppRule {
            id: "foot.desktop".into(),
            pinned: true,
            hidden: false,
            skip_fullscreen_only: None,
        });
        let rows = entries(&input(&apps, &settings, &[]));
        assert_eq!(rows[0].key, "foot.desktop");
    }
}
