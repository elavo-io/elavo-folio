//! `switcher.conf` is a GKeyFile with one `[Switcher]` group. A missing or
//! invalid key keeps the macOS-style default, so an older file keeps working.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;

const GROUP: &str = "Switcher";
const FILE_NAME: &str = "switcher.conf";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Layout {
    #[default]
    Strip,
    List,
    Grid,
    Previews,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SortOrder {
    #[default]
    RecentApps,
    RecentWindows,
    Alphabetical,
    Launch,
    Dock,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Shelf {
    #[default]
    Off,
    List,
    Grid,
    Previews,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DisplayMode {
    Cursor,
    #[default]
    Focused,
    First,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FontScale {
    ExtraSmall,
    Small,
    #[default]
    Standard,
    Large,
    ExtraLarge,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FontFace {
    #[default]
    System,
    Rounded,
    Serif,
    Monospace,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Truncation {
    Head,
    Middle,
    #[default]
    Tail,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SelectionColor {
    #[default]
    Default,
    Blue,
    Purple,
    Pink,
    Red,
    Orange,
    Yellow,
    Green,
    Graphite,
    Custom,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    All,
    Output,
    App,
    Minimized,
}

impl Scope {
    pub fn as_str(self) -> &'static str {
        match self {
            Scope::All => "all",
            Scope::Output => "output",
            Scope::App => "app",
            Scope::Minimized => "minimized",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Scope::All => "All windows",
            Scope::Output => "This output",
            Scope::App => "This application",
            Scope::Minimized => "Minimized only",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "all" => Scope::All,
            "output" => Scope::Output,
            "app" => Scope::App,
            "minimized" => Scope::Minimized,
            _ => return None,
        })
    }

    pub const ALL: &'static [Scope] = &[Scope::All, Scope::Output, Scope::App, Scope::Minimized];
}

macro_rules! coded {
    ($ty:ident { $($name:ident => $code:literal => $label:literal),* $(,)? }) => {
        impl $ty {
            pub fn as_str(self) -> &'static str {
                match self {
                    $(Self::$name => $code,)*
                }
            }

            pub fn label(self) -> &'static str {
                match self {
                    $(Self::$name => $label,)*
                }
            }

            pub fn parse(value: &str) -> Option<Self> {
                Some(match value {
                    $($code => Self::$name,)*
                    _ => return None,
                })
            }

            pub const ALL: &'static [Self] = &[$(Self::$name,)*];
        }
    };
}

coded!(Layout {
    Strip => "strip" => "Icons",
    List => "list" => "List",
    Grid => "grid" => "Grid",
    Previews => "previews" => "Previews",
});
coded!(Appearance {
    System => "system" => "System",
    Light => "light" => "Light",
    Dark => "dark" => "Dark",
});
coded!(SortOrder {
    RecentApps => "mru" => "Recent applications",
    RecentWindows => "mru-windows" => "Recent windows",
    Alphabetical => "name" => "Name",
    Launch => "launch" => "Launch order",
    Dock => "dock" => "Dock order",
});
coded!(Shelf {
    Off => "off" => "Off",
    List => "list" => "List",
    Grid => "grid" => "Grid",
    Previews => "previews" => "Previews",
});
coded!(DisplayMode {
    Cursor => "cursor" => "Pointer",
    Focused => "focused" => "Focused window",
    First => "first" => "First output",
});
coded!(FontScale {
    ExtraSmall => "extra-small" => "Extra small",
    Small => "small" => "Small",
    Standard => "standard" => "Standard",
    Large => "large" => "Large",
    ExtraLarge => "extra-large" => "Extra large",
});
coded!(FontFace {
    System => "system" => "System",
    Rounded => "rounded" => "Rounded",
    Serif => "serif" => "Serif",
    Monospace => "monospace" => "Monospace",
});
coded!(Truncation {
    Head => "head" => "Beginning",
    Middle => "middle" => "Middle",
    Tail => "tail" => "End",
});
coded!(SelectionColor {
    Default => "default" => "Default",
    Blue => "blue" => "Blue",
    Purple => "purple" => "Purple",
    Pink => "pink" => "Pink",
    Red => "red" => "Red",
    Orange => "orange" => "Orange",
    Yellow => "yellow" => "Yellow",
    Green => "green" => "Green",
    Graphite => "graphite" => "Graphite",
    Custom => "custom" => "Custom",
});

/// One application rule. `id` is a desktop-file id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppRule {
    pub id: String,
    pub pinned: bool,
    pub hidden: bool,
    /// `Some(true)` skips Alt+Tab only while a fullscreen window of this
    /// application is focused. `Some(false)` always skips it.
    pub skip_fullscreen_only: Option<bool>,
}

/// An extra shortcut that opens a filtered strip. `key` is a labwc key name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScopedShortcut {
    pub key: String,
    pub scope: Scope,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub layout: Layout,
    pub panel_scale: i32,
    pub list_width: i32,
    pub grid_max_columns: i32,
    pub grid_single_row: bool,
    pub appearance: Appearance,
    pub opacity: i32,
    pub corner_radius: i32,
    pub selection_color: SelectionColor,
    pub selection_color_hex: String,
    pub font_scale: FontScale,
    pub font_face: FontFace,
    pub bold_selected_label: bool,
    pub show_application_names: bool,
    pub show_window_status: bool,
    pub title_truncation: Truncation,
    pub reveal_delay_ms: i32,
    pub stay_open_on_release: bool,
    pub stay_open_on_quick_tap: bool,
    pub sort_order: SortOrder,
    pub show_minimized: bool,
    pub sink_minimized: bool,
    pub show_windowless: bool,
    pub show_unread_badges: bool,
    pub show_recently_closed: bool,
    pub recently_closed_limit: i32,
    pub fuzzy_search: bool,
    pub fuzzy_rank_best: bool,
    pub search_includes_launchable: bool,
    pub letter_hints: bool,
    pub letter_chain_ms: i32,
    pub scroll_to_switch: bool,
    pub scroll_reverse: bool,
    pub click_outside_to_dismiss: bool,
    pub hover_selection: bool,
    pub hover_actions: bool,
    pub hover_close: bool,
    pub hover_minimize: bool,
    pub hover_maximize: bool,
    pub hover_quit: bool,
    pub window_drill: bool,
    pub window_shelf: Shelf,
    pub window_shelf_single: bool,
    pub vim_navigation: bool,
    pub grave_reverses: bool,
    pub show_audio: bool,
    pub sound_on_commit: bool,
    pub display_mode: DisplayMode,
    pub rules: Vec<AppRule>,
    pub scoped: Vec<ScopedShortcut>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            layout: Layout::Strip,
            panel_scale: 100,
            list_width: 100,
            grid_max_columns: 0,
            grid_single_row: false,
            appearance: Appearance::System,
            opacity: 92,
            corner_radius: 0,
            selection_color: SelectionColor::Default,
            selection_color_hex: String::new(),
            font_scale: FontScale::Standard,
            font_face: FontFace::System,
            bold_selected_label: false,
            show_application_names: false,
            show_window_status: false,
            title_truncation: Truncation::Tail,
            reveal_delay_ms: 10,
            stay_open_on_release: false,
            stay_open_on_quick_tap: false,
            sort_order: SortOrder::RecentApps,
            show_minimized: true,
            sink_minimized: false,
            show_windowless: false,
            show_unread_badges: false,
            show_recently_closed: false,
            recently_closed_limit: 5,
            fuzzy_search: false,
            fuzzy_rank_best: false,
            search_includes_launchable: false,
            letter_hints: false,
            letter_chain_ms: 1000,
            scroll_to_switch: false,
            scroll_reverse: false,
            click_outside_to_dismiss: false,
            hover_selection: false,
            hover_actions: false,
            hover_close: true,
            hover_minimize: true,
            hover_maximize: true,
            hover_quit: true,
            window_drill: false,
            window_shelf: Shelf::Off,
            window_shelf_single: false,
            vim_navigation: false,
            grave_reverses: false,
            show_audio: false,
            sound_on_commit: false,
            display_mode: DisplayMode::Focused,
            rules: Vec::new(),
            scoped: Vec::new(),
        }
    }
}

fn split_list(value: &str) -> Vec<String> {
    value
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

fn join_list(values: &[String]) -> String {
    values.join(";")
}

impl Settings {
    pub fn path() -> PathBuf {
        glib::user_config_dir().join("elavo").join(FILE_NAME)
    }

    pub fn load() -> Self {
        let file = glib::KeyFile::new();
        match file.load_from_file(Self::path(), glib::KeyFileFlags::NONE) {
            Ok(()) => Self::from_key_file(&file),
            Err(_) => Self::default(),
        }
    }

    pub fn parse(text: &str) -> Self {
        let file = glib::KeyFile::new();
        match file.load_from_data(text, glib::KeyFileFlags::NONE) {
            Ok(()) => Self::from_key_file(&file),
            Err(_) => Self::default(),
        }
    }

    fn from_key_file(file: &glib::KeyFile) -> Self {
        let d = Self::default();
        let int = |key: &str, default: i32| file.integer(GROUP, key).unwrap_or(default);
        let bool = |key: &str, default: bool| file.boolean(GROUP, key).unwrap_or(default);
        let string = |key: &str| {
            file.string(GROUP, key)
                .ok()
                .map(|s| s.to_string())
                .unwrap_or_default()
        };
        fn coded<T: Copy>(
            file: &glib::KeyFile,
            key: &str,
            default: T,
            parse: fn(&str) -> Option<T>,
        ) -> T {
            file.string(GROUP, key)
                .ok()
                .and_then(|value| parse(value.trim()))
                .unwrap_or(default)
        }
        let mut settings = Self {
            layout: coded(file, "Layout", d.layout, Layout::parse),
            panel_scale: int("PanelScale", d.panel_scale),
            list_width: int("ListWidth", d.list_width),
            grid_max_columns: int("GridMaxColumns", d.grid_max_columns),
            grid_single_row: bool("GridSingleRow", d.grid_single_row),
            appearance: coded(file, "Appearance", d.appearance, Appearance::parse),
            opacity: int("Opacity", d.opacity),
            corner_radius: int("CornerRadius", d.corner_radius),
            selection_color: coded(
                file,
                "SelectionColor",
                d.selection_color,
                SelectionColor::parse,
            ),
            selection_color_hex: string("SelectionColorHex"),
            font_scale: coded(file, "FontScale", d.font_scale, FontScale::parse),
            font_face: coded(file, "FontFace", d.font_face, FontFace::parse),
            bold_selected_label: bool("BoldSelectedLabel", d.bold_selected_label),
            show_application_names: bool("ShowApplicationNames", d.show_application_names),
            show_window_status: bool("ShowWindowStatus", d.show_window_status),
            title_truncation: coded(
                file,
                "TitleTruncation",
                d.title_truncation,
                Truncation::parse,
            ),
            reveal_delay_ms: int("RevealDelay", d.reveal_delay_ms),
            stay_open_on_release: bool("StayOpenOnRelease", d.stay_open_on_release),
            stay_open_on_quick_tap: bool("StayOpenOnQuickTap", d.stay_open_on_quick_tap),
            sort_order: coded(file, "SortOrder", d.sort_order, SortOrder::parse),
            show_minimized: bool("ShowMinimized", d.show_minimized),
            sink_minimized: bool("SinkMinimized", d.sink_minimized),
            show_windowless: bool("ShowWindowless", d.show_windowless),
            show_unread_badges: bool("ShowUnreadBadges", d.show_unread_badges),
            show_recently_closed: bool("ShowRecentlyClosed", d.show_recently_closed),
            recently_closed_limit: int("RecentlyClosedLimit", d.recently_closed_limit),
            fuzzy_search: bool("FuzzySearch", d.fuzzy_search),
            fuzzy_rank_best: bool("FuzzyRankBest", d.fuzzy_rank_best),
            search_includes_launchable: bool(
                "SearchIncludesLaunchable",
                d.search_includes_launchable,
            ),
            letter_hints: bool("LetterHints", d.letter_hints),
            letter_chain_ms: int("LetterChain", d.letter_chain_ms),
            scroll_to_switch: bool("ScrollToSwitch", d.scroll_to_switch),
            scroll_reverse: bool("ScrollReverse", d.scroll_reverse),
            click_outside_to_dismiss: bool("ClickOutsideToDismiss", d.click_outside_to_dismiss),
            hover_selection: bool("HoverSelection", d.hover_selection),
            hover_actions: bool("HoverActions", d.hover_actions),
            hover_close: bool("HoverClose", d.hover_close),
            hover_minimize: bool("HoverMinimize", d.hover_minimize),
            hover_maximize: bool("HoverMaximize", d.hover_maximize),
            hover_quit: bool("HoverQuit", d.hover_quit),
            window_drill: bool("WindowDrill", d.window_drill),
            window_shelf: coded(file, "WindowShelf", d.window_shelf, Shelf::parse),
            window_shelf_single: bool("WindowShelfSingle", d.window_shelf_single),
            vim_navigation: bool("VimNavigation", d.vim_navigation),
            grave_reverses: bool("GraveReverses", d.grave_reverses),
            show_audio: bool("ShowAudio", d.show_audio),
            sound_on_commit: bool("SoundOnCommit", d.sound_on_commit),
            display_mode: coded(file, "DisplayMode", d.display_mode, DisplayMode::parse),
            rules: Vec::new(),
            scoped: Vec::new(),
        };
        let pinned = split_list(&string("Pinned"));
        let hidden = split_list(&string("Hidden"));
        let mut ids: Vec<String> = pinned.iter().chain(hidden.iter()).cloned().collect();
        for item in split_list(&string("Skip")) {
            let id = item.split(':').next().unwrap_or("").trim();
            if !id.is_empty() && !ids.iter().any(|existing| existing == id) {
                ids.push(id.to_owned());
            }
        }
        settings.rules = ids
            .into_iter()
            .map(|id| {
                let skip = split_list(&string("Skip")).into_iter().find_map(|item| {
                    let (rule_id, how) = item.split_once(':')?;
                    (rule_id.trim() == id).then_some(how.trim() == "fullscreen")
                });
                AppRule {
                    pinned: pinned.iter().any(|p| p == &id),
                    hidden: hidden.iter().any(|p| p == &id),
                    skip_fullscreen_only: skip,
                    id,
                }
            })
            .filter(|rule| rule.pinned || rule.hidden || rule.skip_fullscreen_only.is_some())
            .collect();
        settings.scoped = split_list(&string("Scoped"))
            .into_iter()
            .filter_map(|item| {
                let (key, scope) = item.split_once(':')?;
                let key = key.trim();
                if !crate::scoped::key_allowed(key) {
                    return None;
                }
                Some(ScopedShortcut {
                    key: key.to_owned(),
                    scope: Scope::parse(scope.trim())?,
                })
            })
            .collect();
        settings.clamped()
    }

    pub fn to_text(&self) -> String {
        let file = glib::KeyFile::new();
        let set = |key: &str, value: &str| file.set_string(GROUP, key, value);
        set("Layout", self.layout.as_str());
        file.set_integer(GROUP, "PanelScale", self.panel_scale);
        file.set_integer(GROUP, "ListWidth", self.list_width);
        file.set_integer(GROUP, "GridMaxColumns", self.grid_max_columns);
        file.set_boolean(GROUP, "GridSingleRow", self.grid_single_row);
        set("Appearance", self.appearance.as_str());
        file.set_integer(GROUP, "Opacity", self.opacity);
        file.set_integer(GROUP, "CornerRadius", self.corner_radius);
        set("SelectionColor", self.selection_color.as_str());
        set("SelectionColorHex", &self.selection_color_hex);
        set("FontScale", self.font_scale.as_str());
        set("FontFace", self.font_face.as_str());
        file.set_boolean(GROUP, "BoldSelectedLabel", self.bold_selected_label);
        file.set_boolean(GROUP, "ShowApplicationNames", self.show_application_names);
        file.set_boolean(GROUP, "ShowWindowStatus", self.show_window_status);
        set("TitleTruncation", self.title_truncation.as_str());
        file.set_integer(GROUP, "RevealDelay", self.reveal_delay_ms);
        file.set_boolean(GROUP, "StayOpenOnRelease", self.stay_open_on_release);
        file.set_boolean(GROUP, "StayOpenOnQuickTap", self.stay_open_on_quick_tap);
        set("SortOrder", self.sort_order.as_str());
        file.set_boolean(GROUP, "ShowMinimized", self.show_minimized);
        file.set_boolean(GROUP, "SinkMinimized", self.sink_minimized);
        file.set_boolean(GROUP, "ShowWindowless", self.show_windowless);
        file.set_boolean(GROUP, "ShowUnreadBadges", self.show_unread_badges);
        file.set_boolean(GROUP, "ShowRecentlyClosed", self.show_recently_closed);
        file.set_integer(GROUP, "RecentlyClosedLimit", self.recently_closed_limit);
        file.set_boolean(GROUP, "FuzzySearch", self.fuzzy_search);
        file.set_boolean(GROUP, "FuzzyRankBest", self.fuzzy_rank_best);
        file.set_boolean(
            GROUP,
            "SearchIncludesLaunchable",
            self.search_includes_launchable,
        );
        file.set_boolean(GROUP, "LetterHints", self.letter_hints);
        file.set_integer(GROUP, "LetterChain", self.letter_chain_ms);
        file.set_boolean(GROUP, "ScrollToSwitch", self.scroll_to_switch);
        file.set_boolean(GROUP, "ScrollReverse", self.scroll_reverse);
        file.set_boolean(
            GROUP,
            "ClickOutsideToDismiss",
            self.click_outside_to_dismiss,
        );
        file.set_boolean(GROUP, "HoverSelection", self.hover_selection);
        file.set_boolean(GROUP, "HoverActions", self.hover_actions);
        file.set_boolean(GROUP, "HoverClose", self.hover_close);
        file.set_boolean(GROUP, "HoverMinimize", self.hover_minimize);
        file.set_boolean(GROUP, "HoverMaximize", self.hover_maximize);
        file.set_boolean(GROUP, "HoverQuit", self.hover_quit);
        file.set_boolean(GROUP, "WindowDrill", self.window_drill);
        set("WindowShelf", self.window_shelf.as_str());
        file.set_boolean(GROUP, "WindowShelfSingle", self.window_shelf_single);
        file.set_boolean(GROUP, "VimNavigation", self.vim_navigation);
        file.set_boolean(GROUP, "GraveReverses", self.grave_reverses);
        file.set_boolean(GROUP, "ShowAudio", self.show_audio);
        file.set_boolean(GROUP, "SoundOnCommit", self.sound_on_commit);
        set("DisplayMode", self.display_mode.as_str());
        let pinned: Vec<String> = self
            .rules
            .iter()
            .filter(|r| r.pinned)
            .map(|r| r.id.clone())
            .collect();
        let hidden: Vec<String> = self
            .rules
            .iter()
            .filter(|r| r.hidden)
            .map(|r| r.id.clone())
            .collect();
        let skip: Vec<String> = self
            .rules
            .iter()
            .filter_map(|r| {
                r.skip_fullscreen_only.map(|fullscreen| {
                    format!(
                        "{}:{}",
                        r.id,
                        if fullscreen { "fullscreen" } else { "always" }
                    )
                })
            })
            .collect();
        let scoped: Vec<String> = self
            .scoped
            .iter()
            .map(|s| format!("{}:{}", s.key, s.scope.as_str()))
            .collect();
        set("Pinned", &join_list(&pinned));
        set("Hidden", &join_list(&hidden));
        set("Skip", &join_list(&skip));
        set("Scoped", &join_list(&scoped));
        file.to_data().to_string()
    }

    pub fn save(&self) {
        let path = Self::path();
        let result = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .map_err(|e| e.to_string())
            .and_then(|_| {
                glib::file_set_contents(&path, self.to_text().as_bytes()).map_err(|e| e.to_string())
            });
        if let Err(err) = result {
            glib::g_warning!("elavo-switcher", "saving {}: {err}", path.display());
        }
    }

    pub fn clamped(mut self) -> Self {
        self.panel_scale = self.panel_scale.clamp(35, 150);
        self.list_width = self.list_width.clamp(30, 100);
        self.grid_max_columns = self.grid_max_columns.clamp(0, 12);
        self.opacity = self.opacity.clamp(30, 100);
        self.corner_radius = self.corner_radius.clamp(-1, 40);
        self.reveal_delay_ms = self.reveal_delay_ms.clamp(0, 500);
        self.recently_closed_limit = self.recently_closed_limit.clamp(0, 50);
        self.letter_chain_ms = self.letter_chain_ms.clamp(200, 3000);
        self.rules
            .retain(|rule| !rule.id.is_empty() && !rule.id.contains(';'));
        self.scoped
            .retain(|shortcut| crate::scoped::key_allowed(&shortcut.key));
        self
    }

    pub fn update(f: impl FnOnce(&mut Settings)) {
        let mut settings = Self::load();
        let before = settings.clone();
        f(&mut settings);
        let settings = settings.clamped();
        if settings != before {
            let scoped_changed = settings.scoped != before.scoped;
            settings.save();
            if scoped_changed {
                crate::scoped::apply(&settings.scoped);
            }
        }
    }

    pub fn watch(f: impl Fn(&Settings) + 'static) -> Option<gio::FileMonitor> {
        let path = Self::path();
        let dir = path.parent()?.to_path_buf();
        let _ = std::fs::create_dir_all(&dir);
        let monitor = gio::File::for_path(&dir)
            .monitor_directory(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE)
            .ok()?;
        let last = Rc::new(RefCell::new(Self::load()));
        monitor.connect_changed(move |_, file, other, _| {
            let ours = |f: Option<&gio::File>| {
                f.and_then(|f| f.basename())
                    .is_some_and(|name| name.as_os_str() == FILE_NAME)
            };
            if !ours(Some(file)) && !ours(other) {
                return;
            }
            let settings = Self::load();
            if *last.borrow() != settings {
                last.replace(settings.clone());
                f(&settings);
            }
        });
        Some(monitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_keys_are_the_macos_switcher() {
        assert_eq!(Settings::parse("[Switcher]\n"), Settings::default());
        assert_eq!(Settings::parse("not a key file"), Settings::default());
        let d = Settings::default();
        assert_eq!(d.layout, Layout::Strip);
        assert!(!d.fuzzy_search);
        assert!(!d.hover_actions);
        assert!(d.show_minimized);
        assert_eq!(d.reveal_delay_ms, 10);
    }

    #[test]
    fn round_trip() {
        let settings = Settings {
            layout: Layout::Grid,
            panel_scale: 80,
            list_width: 70,
            grid_max_columns: 4,
            grid_single_row: true,
            appearance: Appearance::Dark,
            opacity: 60,
            corner_radius: 12,
            selection_color: SelectionColor::Custom,
            selection_color_hex: "#112233".into(),
            font_scale: FontScale::Large,
            font_face: FontFace::Serif,
            bold_selected_label: true,
            show_application_names: true,
            show_window_status: true,
            title_truncation: Truncation::Middle,
            reveal_delay_ms: 120,
            stay_open_on_release: true,
            stay_open_on_quick_tap: true,
            sort_order: SortOrder::Dock,
            show_minimized: false,
            sink_minimized: true,
            show_windowless: true,
            show_unread_badges: true,
            show_recently_closed: true,
            recently_closed_limit: 3,
            fuzzy_search: true,
            fuzzy_rank_best: true,
            search_includes_launchable: true,
            letter_hints: true,
            letter_chain_ms: 800,
            scroll_to_switch: true,
            scroll_reverse: true,
            click_outside_to_dismiss: true,
            hover_selection: true,
            hover_actions: true,
            hover_close: false,
            hover_minimize: true,
            hover_maximize: false,
            hover_quit: true,
            window_drill: true,
            window_shelf: Shelf::List,
            window_shelf_single: true,
            vim_navigation: true,
            grave_reverses: true,
            show_audio: true,
            sound_on_commit: true,
            display_mode: DisplayMode::Cursor,
            rules: vec![AppRule {
                id: "foot.desktop".into(),
                pinned: true,
                hidden: false,
                skip_fullscreen_only: Some(true),
            }],
            scoped: vec![ScopedShortcut {
                key: "C-A-m".into(),
                scope: Scope::Minimized,
            }],
        };
        assert_eq!(Settings::parse(&settings.to_text()), settings);
    }

    #[test]
    fn out_of_range_values_clamp() {
        let s = Settings::parse("[Switcher]\nPanelScale=1\nOpacity=200\nLayout=nope\n");
        assert_eq!(s.panel_scale, 35);
        assert_eq!(s.opacity, 100);
        assert_eq!(s.layout, Layout::Strip);
    }
}
