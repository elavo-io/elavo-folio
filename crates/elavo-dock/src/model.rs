//! What the dock shows, left to right as on a Mac: the applications section
//! (pins, then running applications that are not pinned), the recent
//! applications section, then a divider, files and folders, minimized
//! windows, and the trash. This module computes the application sections and
//! the minimized windows; the dock adds the rest. Windows group by desktop ID;
//! a window whose `app_id` matches no desktop file groups by `app_id`.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item<K> {
    /// Desktop-file ID, or `None` for a window with no desktop file.
    pub desktop_id: Option<String>,
    /// Used for the icon when there is no desktop file.
    pub app_id: Option<String>,
    pub pinned: bool,
    /// In the recent applications section.
    pub recent: bool,
    pub windows: Vec<K>,
    pub focused: bool,
}

impl<K> Item<K> {
    pub fn running(&self) -> bool {
        !self.windows.is_empty()
    }

    /// Stable identity for animations and drag state.
    pub fn key(&self) -> String {
        match (&self.desktop_id, &self.app_id) {
            (Some(id), _) => id.clone(),
            (None, Some(app_id)) => format!("app-id:{app_id}"),
            (None, None) => String::new(),
        }
    }
}

pub struct Window<'a, K> {
    pub key: K,
    pub app_id: &'a str,
    pub activated: bool,
    pub minimized: bool,
}

pub struct Options {
    /// Show the recent applications section.
    pub recents: bool,
    /// The section shows running applications plus recently used ones up to
    /// this many in total.
    pub max_recents: usize,
    /// Give each minimized window its own tile.
    pub minimized_tiles: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Sections<K> {
    pub apps: Vec<Item<K>>,
    pub recents: Vec<Item<K>>,
    /// Minimized windows, in the order given.
    pub minimized: Vec<K>,
}

/// `known` says whether a desktop ID is installed; uninstalled pins are
/// skipped but stay in the pin file. `resolve` maps an `app_id` to a desktop
/// ID, or returns `None` when the window has no shown desktop file. `recent`
/// is most recently used first.
///
/// [`background_service`] is omitted everywhere: no pin, no running icon, no
/// recent, no minimized tile. `xdg-desktop-portal-gtk` is that list. Its
/// dialogs belong to whatever application asked for them.
pub fn sections<K: Clone>(
    pins: &[String],
    recent: &[String],
    known: impl Fn(&str) -> bool,
    windows: &[Window<K>],
    resolve: impl Fn(&str) -> Option<String>,
    options: &Options,
) -> Sections<K> {
    let (mut apps, mut running) = items(pins, &known, windows, resolve);
    let mut recents = Vec::new();
    if options.recents {
        for item in &mut running {
            item.recent = true;
        }
        let extra = options.max_recents.saturating_sub(running.len());
        let shown = |id: &str| {
            apps.iter()
                .chain(&running)
                .any(|i| i.desktop_id.as_deref() == Some(id))
        };
        let quit: Vec<Item<K>> = recent
            .iter()
            .filter(|id| known(id) && !shown(id) && !background_service(id))
            .take(extra)
            .map(|id| Item {
                desktop_id: Some(id.clone()),
                app_id: None,
                pinned: false,
                recent: true,
                windows: Vec::new(),
                focused: false,
            })
            .collect();
        recents = running;
        recents.extend(quit);
    } else {
        apps.append(&mut running);
    }
    let minimized = if options.minimized_tiles {
        windows
            .iter()
            .filter(|w| w.minimized && !background_service(w.app_id))
            .map(|w| w.key.clone())
            .collect()
    } else {
        Vec::new()
    };
    Sections {
        apps,
        recents,
        minimized,
    }
}

/// Pins, and running applications that are not pinned.
fn items<K: Clone>(
    pins: &[String],
    known: impl Fn(&str) -> bool,
    windows: &[Window<K>],
    resolve: impl Fn(&str) -> Option<String>,
) -> (Vec<Item<K>>, Vec<Item<K>>) {
    let mut pinned: Vec<Item<K>> = pins
        .iter()
        .filter(|id| known(id) && !background_service(id))
        .map(|id| Item {
            desktop_id: Some(id.clone()),
            app_id: None,
            pinned: true,
            recent: false,
            windows: Vec::new(),
            focused: false,
        })
        .collect();
    let mut running: Vec<Item<K>> = Vec::new();

    for window in windows {
        if background_service(window.app_id) {
            continue;
        }
        let desktop_id = resolve(window.app_id);
        if desktop_id.as_deref().is_some_and(background_service)
            || (desktop_id.is_none() && window.app_id.is_empty())
        {
            continue;
        }
        let item = match desktop_id.as_deref() {
            Some(id) => pinned
                .iter_mut()
                .chain(running.iter_mut())
                .find(|i| i.desktop_id.as_deref() == Some(id)),
            None => running
                .iter_mut()
                .find(|i| i.desktop_id.is_none() && i.app_id.as_deref() == Some(window.app_id)),
        };
        let item = match item {
            Some(item) => item,
            None => {
                running.push(Item {
                    desktop_id,
                    app_id: Some(window.app_id.to_owned()),
                    pinned: false,
                    recent: false,
                    windows: Vec::new(),
                    focused: false,
                });
                running.last_mut().expect("just pushed")
            }
        };
        if item.app_id.is_none() {
            item.app_id = Some(window.app_id.to_owned());
        }
        item.windows.push(window.key.clone());
        item.focused |= window.activated && !window.minimized;
    }
    (pinned, running)
}

/// Where a pin dragged to slot `slot` of the visible pins goes in the pin
/// file, which may also hold pins for uninstalled applications. `visible` is
/// the visible pins in order.
pub fn pin_index(pins: &[String], visible: &[String], slot: usize) -> usize {
    match visible.get(slot) {
        Some(id) => pins.iter().position(|p| p == id).unwrap_or(pins.len()),
        None => match visible.last() {
            Some(last) => pins
                .iter()
                .position(|p| p == last)
                .map_or(pins.len(), |i| i + 1),
            None => pins.len(),
        },
    }
}

/// Move or insert `id` so it lands before `to` (an index into `pins` before
/// the move).
pub fn place_pin(pins: &mut Vec<String>, id: &str, to: usize) {
    let to = match pins.iter().position(|p| p == id) {
        Some(from) => {
            pins.remove(from);
            if from < to {
                to - 1
            } else {
                to
            }
        }
        None => to,
    };
    pins.insert(to.min(pins.len()), id.to_owned());
}

/// `xdg-desktop-portal-gtk` opens file choosers and other dialogs for
/// whichever application asked. Those windows are not a dock application,
/// whether the app id is empty or the desktop file's name.
pub fn background_service(id: &str) -> bool {
    let id = id.strip_suffix(".desktop").unwrap_or(id);
    id == "xdg-desktop-portal-gtk"
}

/// Most recently used first, without duplicates, at most `max` long.
pub fn touch_recent(recent: &mut Vec<String>, id: &str, max: usize) {
    recent.retain(|r| r != id);
    recent.insert(0, id.to_owned());
    recent.truncate(max);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolve(app_id: &str) -> Option<String> {
        match app_id {
            "foot" => Some("foot.desktop".into()),
            "org.mozilla.firefox" => Some("org.mozilla.firefox.desktop".into()),
            "org.gnome.Nautilus" => Some("org.gnome.Nautilus.desktop".into()),
            "gedit" => Some("gedit.desktop".into()),
            _ => None,
        }
    }

    fn pins() -> Vec<String> {
        vec![
            "org.gnome.Nautilus.desktop".into(),
            "org.mozilla.firefox.desktop".into(),
            "gone.desktop".into(),
        ]
    }

    fn known(id: &str) -> bool {
        id != "gone.desktop"
    }

    fn window(key: u32, app_id: &str) -> Window<'_, u32> {
        Window {
            key,
            app_id,
            activated: false,
            minimized: false,
        }
    }

    const NO_RECENTS: Options = Options {
        recents: false,
        max_recents: 3,
        minimized_tiles: false,
    };

    #[test]
    fn pins_without_windows() {
        let s = sections::<u32>(&pins(), &[], known, &[], resolve, &NO_RECENTS);
        assert_eq!(s.apps.len(), 2);
        assert!(s.apps.iter().all(|i| i.pinned && !i.running()));
        assert!(s.recents.is_empty());
    }

    #[test]
    fn a_window_without_an_app_id_is_not_shown() {
        let windows = [window(1, "")];
        let s = sections(&pins(), &[], known, &windows, |_| None, &NO_RECENTS);
        assert_eq!(s.apps.len(), 2);
        assert!(s.apps.iter().all(|i| i.pinned));
    }

    #[test]
    fn portal_dialogs_are_not_a_dock_item() {
        let windows = [
            window(1, "xdg-desktop-portal-gtk"),
            Window {
                minimized: true,
                ..window(2, "xdg-desktop-portal-gtk.desktop")
            },
            window(3, "portal-dialog"),
        ];
        let pins = vec!["xdg-desktop-portal-gtk.desktop".to_owned()];
        let recent = vec!["xdg-desktop-portal-gtk.desktop".to_owned()];
        let options = Options {
            recents: true,
            minimized_tiles: true,
            ..NO_RECENTS
        };
        let resolve = |app_id: &str| {
            (app_id == "portal-dialog").then(|| "xdg-desktop-portal-gtk.desktop".to_owned())
        };
        let s = sections(&pins, &recent, known, &windows, resolve, &options);
        assert!(s.apps.is_empty());
        assert!(s.recents.is_empty());
        assert!(s.minimized.is_empty());
    }

    #[test]
    fn running_apps_follow_pins_without_recents() {
        let windows = [
            window(1, "org.mozilla.firefox"),
            Window {
                activated: true,
                ..window(2, "foot")
            },
            window(3, "org.mozilla.firefox"),
            window(4, "mystery"),
            window(5, "foot"),
        ];
        let s = sections(&pins(), &[], known, &windows, resolve, &NO_RECENTS);
        assert_eq!(s.apps.len(), 4);
        assert_eq!(s.apps[1].windows, vec![1, 3]);
        assert!(!s.apps[1].focused);
        assert_eq!(s.apps[2].desktop_id.as_deref(), Some("foot.desktop"));
        assert_eq!(s.apps[2].windows, vec![2, 5]);
        assert!(s.apps[2].focused);
        assert!(!s.apps[2].pinned);
        assert_eq!(s.apps[3].desktop_id, None);
        assert_eq!(s.apps[3].key(), "app-id:mystery");
    }

    #[test]
    fn recents_hold_running_then_recently_quit() {
        let options = Options {
            recents: true,
            ..NO_RECENTS
        };
        let recent: Vec<String> = [
            "foot.desktop",
            "gedit.desktop",
            "org.mozilla.firefox.desktop",
            "gone.desktop",
            "x.desktop",
        ]
        .map(String::from)
        .into();
        let windows = [window(1, "foot")];
        let s = sections(&pins(), &recent, known, &windows, resolve, &options);
        assert_eq!(s.apps.len(), 2);
        let ids: Vec<_> = s.recents.iter().map(|i| i.key()).collect();
        // foot is running; Firefox is pinned; gone is not installed.
        assert_eq!(ids, ["foot.desktop", "gedit.desktop", "x.desktop"]);
        assert!(s.recents.iter().all(|i| i.recent && !i.pinned));
        assert!(s.recents[0].running() && !s.recents[1].running());
    }

    #[test]
    fn many_running_apps_push_out_recently_quit() {
        let options = Options {
            recents: true,
            max_recents: 1,
            ..NO_RECENTS
        };
        let windows = [window(1, "foot"), window(2, "mystery")];
        let recent = vec!["gedit.desktop".to_owned()];
        let s = sections(&pins(), &recent, known, &windows, resolve, &options);
        assert_eq!(s.recents.len(), 2);
    }

    #[test]
    fn minimized_windows_get_tiles_when_asked() {
        let windows = [
            Window {
                minimized: true,
                activated: true,
                ..window(1, "foot")
            },
            window(2, "foot"),
        ];
        let s = sections(&pins(), &[], known, &windows, resolve, &NO_RECENTS);
        assert!(s.minimized.is_empty());
        assert!(!s.apps[2].focused, "a minimized window is not focused");
        let options = Options {
            minimized_tiles: true,
            ..NO_RECENTS
        };
        let s = sections(&pins(), &[], known, &windows, resolve, &options);
        assert_eq!(s.minimized, vec![1]);
    }

    #[test]
    fn pin_index_skips_hidden_pins() {
        let pins: Vec<String> = ["a", "gone", "b", "c"].map(String::from).into();
        let visible: Vec<String> = ["a", "b", "c"].map(String::from).into();
        assert_eq!(pin_index(&pins, &visible, 0), 0);
        assert_eq!(pin_index(&pins, &visible, 1), 2);
        assert_eq!(pin_index(&pins, &visible, 3), 4);
        assert_eq!(pin_index(&[], &[], 0), 0);
    }

    #[test]
    fn place_pin_moves_and_inserts() {
        let mut pins: Vec<String> = ["a", "b", "c"].map(String::from).into();
        place_pin(&mut pins, "a", 3);
        assert_eq!(pins, ["b", "c", "a"]);
        place_pin(&mut pins, "a", 0);
        assert_eq!(pins, ["a", "b", "c"]);
        place_pin(&mut pins, "new", 1);
        assert_eq!(pins, ["a", "new", "b", "c"]);
    }

    #[test]
    fn touch_recent_is_mru() {
        let mut recent = vec!["a".to_owned(), "b".to_owned()];
        touch_recent(&mut recent, "b", 2);
        touch_recent(&mut recent, "c", 2);
        assert_eq!(recent, ["c", "b"]);
    }
}
