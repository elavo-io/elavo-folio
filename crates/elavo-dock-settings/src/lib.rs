//! Dock settings: `$XDG_CONFIG_HOME/elavo/dock.conf`, which the dock watches,
//! and the page that edits it. System Settings (`elavo-system-settings`)
//! embeds the page as its Desktop & Dock pane, and the dock's Dock Settings
//! item opens that pane; without System Settings the dock shows the page in
//! its own window. Neither talks to the dock directly: writing
//! the file is the whole interface.

mod config;
mod page;

pub use config::{Position, Settings, MAX_ICON_SIZE, MIN_ICON_SIZE};
pub use page::{page, window};
