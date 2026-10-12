//! `dock.conf` is a GKeyFile with one `[Dock]` group. Unknown keys are kept
//! by nobody and ignored by everybody; missing or invalid keys take the
//! default, so an older file keeps working.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;

pub const MIN_ICON_SIZE: i32 = 16;
pub const MAX_ICON_SIZE: i32 = 128;

const GROUP: &str = "Dock";
const FILE_NAME: &str = "dock.conf";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Position {
    Left,
    #[default]
    Bottom,
    Right,
}

impl Position {
    pub const ALL: [Position; 3] = [Position::Left, Position::Bottom, Position::Right];

    pub fn as_str(self) -> &'static str {
        match self {
            Position::Left => "left",
            Position::Bottom => "bottom",
            Position::Right => "right",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Position::Left => "Left",
            Position::Bottom => "Bottom",
            Position::Right => "Right",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.as_str() == value)
    }

    pub fn vertical(self) -> bool {
        self != Position::Bottom
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    /// Logical pixels.
    pub icon_size: i32,
    pub magnification: bool,
    /// The size of the icon under the pointer. Never smaller than `icon_size`.
    pub magnified_size: i32,
    pub position: Position,
    pub autohide: bool,
    pub animate_launch: bool,
    pub show_indicators: bool,
    pub show_recents: bool,
    /// Off: a minimized window gets its own tile after the divider.
    pub minimize_to_application: bool,
    /// `None` puts a dock on every display; otherwise the connector name.
    pub display: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            icon_size: 48,
            magnification: false,
            magnified_size: 96,
            position: Position::Bottom,
            autohide: false,
            animate_launch: true,
            show_indicators: true,
            show_recents: true,
            minimize_to_application: false,
            display: None,
        }
    }
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
        let string = |key: &str| file.string(GROUP, key).ok().map(|s| s.to_string());
        let display = string("Display").filter(|d| !d.is_empty() && d != "all");
        Self {
            icon_size: int("IconSize", d.icon_size),
            magnification: bool("Magnification", d.magnification),
            magnified_size: int("MagnifiedSize", d.magnified_size),
            position: string("Position")
                .and_then(|p| Position::parse(&p))
                .unwrap_or(d.position),
            autohide: bool("Autohide", d.autohide),
            animate_launch: bool("AnimateLaunch", d.animate_launch),
            show_indicators: bool("ShowIndicators", d.show_indicators),
            show_recents: bool("ShowRecents", d.show_recents),
            minimize_to_application: bool("MinimizeToApplication", d.minimize_to_application),
            display,
        }
        .clamped()
    }

    pub fn to_text(&self) -> String {
        let file = glib::KeyFile::new();
        file.set_integer(GROUP, "IconSize", self.icon_size);
        file.set_boolean(GROUP, "Magnification", self.magnification);
        file.set_integer(GROUP, "MagnifiedSize", self.magnified_size);
        file.set_string(GROUP, "Position", self.position.as_str());
        file.set_boolean(GROUP, "Autohide", self.autohide);
        file.set_boolean(GROUP, "AnimateLaunch", self.animate_launch);
        file.set_boolean(GROUP, "ShowIndicators", self.show_indicators);
        file.set_boolean(GROUP, "ShowRecents", self.show_recents);
        file.set_boolean(GROUP, "MinimizeToApplication", self.minimize_to_application);
        file.set_string(GROUP, "Display", self.display.as_deref().unwrap_or("all"));
        file.to_data().to_string()
    }

    /// Writes atomically, so a watcher never reads half a file.
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
            glib::g_warning!("elavo-dock", "saving {}: {err}", path.display());
        }
    }

    pub fn clamped(mut self) -> Self {
        self.icon_size = self.icon_size.clamp(MIN_ICON_SIZE, MAX_ICON_SIZE);
        self.magnified_size = self.magnified_size.clamp(MIN_ICON_SIZE, MAX_ICON_SIZE);
        self
    }

    /// The largest icon the dock draws: the magnified size when magnification
    /// is on, otherwise the icon size.
    pub fn peak_size(&self) -> i32 {
        if self.magnification {
            self.magnified_size.max(self.icon_size)
        } else {
            self.icon_size
        }
    }

    /// Load, change, save. For callers that edit one field.
    pub fn update(f: impl FnOnce(&mut Settings)) {
        let mut settings = Self::load();
        let before = settings.clone();
        f(&mut settings);
        let settings = settings.clamped();
        if settings != before {
            settings.save();
        }
    }

    /// Calls `f` with the new settings whenever the file changes to something
    /// different. Keep the monitor alive for as long as `f` should run.
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
    fn missing_keys_take_defaults() {
        assert_eq!(Settings::parse("[Dock]\n"), Settings::default());
        assert_eq!(Settings::parse("not a key file"), Settings::default());
    }

    #[test]
    fn round_trip() {
        let settings = Settings {
            icon_size: 64,
            magnification: true,
            magnified_size: 120,
            position: Position::Left,
            autohide: true,
            animate_launch: false,
            show_indicators: false,
            show_recents: false,
            minimize_to_application: true,
            display: Some("eDP-1".into()),
        };
        assert_eq!(Settings::parse(&settings.to_text()), settings);
    }

    #[test]
    fn out_of_range_values_clamp() {
        let s = Settings::parse("[Dock]\nIconSize=4\nMagnifiedSize=900\nPosition=top\n");
        assert_eq!(s.icon_size, MIN_ICON_SIZE);
        assert_eq!(s.magnified_size, MAX_ICON_SIZE);
        assert_eq!(s.position, Position::Bottom);
    }

    #[test]
    fn peak_size_never_below_icon_size() {
        let mut s = Settings {
            magnification: true,
            icon_size: 80,
            magnified_size: 40,
            ..Settings::default()
        };
        assert_eq!(s.peak_size(), 80);
        s.magnification = false;
        s.magnified_size = 128;
        assert_eq!(s.peak_size(), 80);
    }
}
