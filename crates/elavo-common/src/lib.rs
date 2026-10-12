//! Pieces shared by the panel and the dock: the application index, the
//! match from a window's `app_id` to a desktop ID, and launching.

pub mod apps;
pub mod matching;
pub mod ranking;

pub use apps::{launch, AppEntry, AppIndex};
pub use gio_unix::DesktopAppInfo;
