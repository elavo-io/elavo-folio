//! App Switcher settings: `$XDG_CONFIG_HOME/elavo/switcher.conf`, which the
//! switcher watches, and the page System Settings embeds.

mod config;
mod page;
mod probe;
mod scoped;

pub use config::{
    AppRule, Appearance, DisplayMode, FontFace, FontScale, Layout, Scope, ScopedShortcut,
    SelectionColor, Settings, Shelf, SortOrder, Truncation,
};
pub use page::page;
pub use probe::previews_available;
