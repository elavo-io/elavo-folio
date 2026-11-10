//! What the menu on an application offers: a right click, a long press, or
//! the Menu key opens it. The choices here are plain data, so they can be
//! tested without a display.

/// Desktop action IDs that open another window of the application. The
/// first one an application has becomes New Window.
pub const NEW_WINDOW_IDS: &[&str] = &["new-window", "new-empty-window", "new-instance"];
/// The application's other desktop actions listed under New Window, such as
/// Firefox's New Private Window, so the menu stays short.
pub const MAX_OTHER_ACTIONS: usize = 3;

/// An application's desktop actions, as the menu lists them.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct DesktopActions {
    /// The action behind New Window, if it has one.
    pub new_window: Option<String>,
    /// Up to [`MAX_OTHER_ACTIONS`] more, in the desktop file's order.
    pub others: Vec<String>,
}

pub fn desktop_actions(ids: &[String]) -> DesktopActions {
    let new_window = NEW_WINDOW_IDS
        .iter()
        .find(|wanted| ids.iter().any(|id| id == *wanted))
        .map(|id| id.to_string());
    let others = ids
        .iter()
        .filter(|id| !id.is_empty() && Some(id.as_str()) != new_window.as_deref())
        .take(MAX_OTHER_ACTIONS)
        .cloned()
        .collect();
    DesktopActions { new_window, others }
}

pub fn pin_label(pinned: bool) -> &'static str {
    if pinned {
        "Remove from Dock"
    } else {
        "Keep in Dock"
    }
}

/// The `"kind"` an Applications entry's `entry.json` gives: `flatpak`,
/// `appimage`, or `system`.
pub fn entry_kind(json: &str) -> Option<&str> {
    entry_field(json, "kind")
}

/// A string field of an Applications entry's `entry.json`.
fn entry_field<'a>(json: &'a str, key: &str) -> Option<&'a str> {
    let quoted = format!("\"{key}\"");
    let after = &json[json.find(&quoted)? + quoted.len()..];
    let after = after.trim_start().strip_prefix(':')?.trim_start();
    let value = after.strip_prefix('"')?;
    Some(&value[..value.find('"')?])
}

/// The ID `elavo-app-bay --app` opens for an app: its Flatpak ID, or
/// `appimage:` and the `entry.json` id for an AppImage. None when App Bay
/// would not know the app.
pub fn app_bay_id(flatpak: Option<&str>, entry_json: Option<&str>) -> Option<String> {
    if let Some(flatpak) = flatpak.map(str::trim).filter(|f| !f.is_empty()) {
        return Some(flatpak.to_owned());
    }
    let json = entry_json?;
    let id = entry_field(json, "id")
        .map(str::trim)
        .filter(|id| !id.is_empty())?;
    match entry_kind(json)? {
        "appimage" => Some(format!("appimage:{id}")),
        "flatpak" => Some(id.to_owned()),
        _ => None,
    }
}

/// App Bay installs, updates, and removes Flatpaks and AppImages. Software
/// that came with the system is not its to change.
pub fn managed_by_app_bay(flatpak: Option<&str>, kind: Option<&str>) -> bool {
    flatpak.is_some_and(|f| !f.trim().is_empty()) || matches!(kind, Some("flatpak" | "appimage"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn firefox_gets_new_window_and_its_other_actions() {
        let actions = desktop_actions(&ids(&[
            "new-window",
            "new-private-window",
            "open-profile-manager",
        ]));
        assert_eq!(actions.new_window.as_deref(), Some("new-window"));
        assert_eq!(
            actions.others,
            ids(&["new-private-window", "open-profile-manager"])
        );
    }

    #[test]
    fn other_names_for_a_new_window_count() {
        let actions = desktop_actions(&ids(&["new-empty-window"]));
        assert_eq!(actions.new_window.as_deref(), Some("new-empty-window"));
        assert!(actions.others.is_empty());
    }

    #[test]
    fn no_new_window_without_the_action_and_at_most_three_others() {
        let actions = desktop_actions(&ids(&["a", "b", "", "c", "d"]));
        assert_eq!(actions.new_window, None);
        assert_eq!(actions.others, ids(&["a", "b", "c"]));
        assert_eq!(desktop_actions(&[]), DesktopActions::default());
    }

    #[test]
    fn pin_label_follows_the_pin() {
        assert_eq!(pin_label(true), "Remove from Dock");
        assert_eq!(pin_label(false), "Keep in Dock");
    }

    #[test]
    fn reads_the_entry_kind() {
        let json = "{\n  \"kind\": \"appimage\",\n  \"id\": \"cursor\"\n}";
        assert_eq!(entry_kind(json), Some("appimage"));
        assert_eq!(entry_kind("{\"kind\":\"flatpak\"}"), Some("flatpak"));
        assert_eq!(entry_kind("{\"id\": \"x\"}"), None);
        assert_eq!(entry_kind("{\"kind\": 3}"), None);
        assert_eq!(entry_kind("{\"kind\": \"open"), None);
    }

    #[test]
    fn app_bay_ids_come_from_the_flatpak_or_the_entry() {
        let appimage = "{\n  \"kind\": \"appimage\",\n  \"id\": \"brave-origin\"\n}";
        let flatpak = "{\"kind\": \"flatpak\", \"id\": \"com.bitwarden.desktop\"}";
        let system = "{\"kind\": \"system\", \"id\": \"org.gnome.Nautilus\"}";
        assert_eq!(
            app_bay_id(Some("org.mozilla.firefox"), None).as_deref(),
            Some("org.mozilla.firefox")
        );
        assert_eq!(
            app_bay_id(Some("com.bitwarden.desktop"), Some(flatpak)).as_deref(),
            Some("com.bitwarden.desktop")
        );
        assert_eq!(
            app_bay_id(None, Some(appimage)).as_deref(),
            Some("appimage:brave-origin")
        );
        assert_eq!(
            app_bay_id(Some(" "), Some(flatpak)).as_deref(),
            Some("com.bitwarden.desktop")
        );
        assert_eq!(app_bay_id(None, Some(system)), None);
        assert_eq!(app_bay_id(None, Some("{\"kind\": \"appimage\"}")), None);
        assert_eq!(
            app_bay_id(None, Some("{\"kind\": \"appimage\", \"id\": \"\"}")),
            None
        );
        assert_eq!(app_bay_id(None, None), None);
    }

    #[test]
    fn the_id_field_is_not_confused_with_other_keys() {
        let json = "{\"kind\": \"appimage\", \"origin_id\": \"x\", \"id\": \"real\"}";
        assert_eq!(
            app_bay_id(None, Some(json)).as_deref(),
            Some("appimage:real")
        );
    }

    #[test]
    fn app_bay_manages_flatpaks_and_appimages_only() {
        assert!(managed_by_app_bay(Some("org.mozilla.firefox"), None));
        assert!(managed_by_app_bay(None, Some("appimage")));
        assert!(managed_by_app_bay(None, Some("flatpak")));
        assert!(!managed_by_app_bay(None, Some("system")));
        assert!(!managed_by_app_bay(Some(" "), None));
        assert!(!managed_by_app_bay(None, None));
    }
}
