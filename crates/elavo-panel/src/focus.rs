//! Which application has the focused window, from the foreign-toplevel
//! events. Generic over the window key so it can be tested without a
//! compositor.

use std::collections::HashMap;
use std::hash::Hash;

pub struct Focus<K> {
    windows: HashMap<K, String>,
    active: Option<K>,
}

impl<K> Default for Focus<K> {
    fn default() -> Self {
        Self {
            windows: HashMap::new(),
            active: None,
        }
    }
}

impl<K: Eq + Hash + Clone> Focus<K> {
    pub fn changed(&mut self, key: K, app_id: &str, activated: bool) {
        if activated {
            self.active = Some(key.clone());
        } else if self.active.as_ref() == Some(&key) {
            self.active = None;
        }
        self.windows.insert(key, app_id.to_owned());
    }

    pub fn closed(&mut self, key: &K) {
        self.windows.remove(key);
        if self.active.as_ref() == Some(key) {
            self.active = None;
        }
    }

    /// The focused window's `app_id`. None on the desktop, on an empty
    /// workspace, and for a window that has not sent an `app_id`.
    pub fn focused(&self) -> Option<&str> {
        self.active
            .as_ref()
            .and_then(|k| self.windows.get(k))
            .map(String::as_str)
            .filter(|id| !id.is_empty())
    }

    pub fn active(&self) -> Option<&K> {
        self.active.as_ref()
    }

    pub fn windows_of(&self, app_id: &str) -> usize {
        self.windows.values().filter(|id| *id == app_id).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_focused_at_start() {
        let focus = Focus::<u32>::default();
        assert_eq!(focus.focused(), None);
    }

    #[test]
    fn activate_and_deactivate() {
        let mut focus = Focus::default();
        focus.changed(1, "foot", true);
        assert_eq!(focus.focused(), Some("foot"));
        focus.changed(1, "foot", false);
        assert_eq!(focus.focused(), None);
    }

    #[test]
    fn switch_apps_in_either_order() {
        let mut focus = Focus::default();
        focus.changed(1, "foot", true);
        focus.changed(2, "firefox", true);
        focus.changed(1, "foot", false);
        assert_eq!(focus.focused(), Some("firefox"));

        focus.changed(2, "firefox", false);
        focus.changed(1, "foot", true);
        assert_eq!(focus.focused(), Some("foot"));
    }

    #[test]
    fn closing_the_focused_window_clears_it() {
        let mut focus = Focus::default();
        focus.changed(1, "foot", false);
        focus.changed(2, "firefox", true);
        focus.closed(&2);
        assert_eq!(focus.focused(), None);
        focus.closed(&1);
        assert_eq!(focus.windows_of("foot"), 0);
    }

    #[test]
    fn closing_another_window_keeps_focus() {
        let mut focus = Focus::default();
        focus.changed(1, "foot", false);
        focus.changed(2, "firefox", true);
        focus.closed(&1);
        assert_eq!(focus.focused(), Some("firefox"));
    }

    #[test]
    fn counts_windows_per_app() {
        let mut focus = Focus::default();
        focus.changed(1, "foot", false);
        focus.changed(2, "foot", true);
        focus.changed(3, "firefox", false);
        assert_eq!(focus.windows_of("foot"), 2);
        assert_eq!(focus.windows_of("firefox"), 1);
    }

    #[test]
    fn empty_app_id_is_not_focus() {
        let mut focus = Focus::default();
        focus.changed(1, "", true);
        assert_eq!(focus.focused(), None);
    }
}
