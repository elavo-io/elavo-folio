//! Pieces shared by the panel and the dock: the application index, the
//! match from a window's `app_id` to a desktop ID, launching, and the window
//! list.

pub mod apps;
pub mod blur;
pub mod matching;
pub mod ranking;
pub mod toplevels;

pub use apps::{launch, AppEntry, AppIndex};
pub use gio_unix::DesktopAppInfo;

/// About Elavo is a toplevel of the panel process, so its Wayland app id is
/// the panel's. The title is a word joiner: the bar shows no name, and the
/// dock can tell this window from a dialog.
pub const PANEL_APP_ID: &str = "io.elavo.Panel";
pub const ABOUT_TITLE: &str = "\u{2060}";
