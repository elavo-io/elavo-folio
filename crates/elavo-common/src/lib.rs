//! Pieces shared by the panel and the dock: the application index, the
//! match from a window's `app_id` to a desktop ID, launching, and the window
//! list.

pub mod apps;
pub mod matching;
pub mod ranking;
pub mod toplevels;

pub use apps::{launch, AppEntry, AppIndex};
pub use gio_unix::DesktopAppInfo;
