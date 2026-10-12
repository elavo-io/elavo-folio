//! Dock settings: `$XDG_CONFIG_HOME/elavo/dock.conf`, which the dock watches,
//! and the page that edits it. The dock shows the page in its own window
//! (Dock Settings in the divider menu); `elavo-settings` embeds the same page
//! as its Desktop and Dock pane. Neither talks to the dock directly: writing
//! the file is the whole interface.

mod config;
mod page;

pub use config::{Position, Settings, MAX_ICON_SIZE, MIN_ICON_SIZE};
pub use page::{page, window};
