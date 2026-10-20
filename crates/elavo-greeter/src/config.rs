//! `/etc/elavo/greeter.conf`: what the login window shows. System Settings'
//! Lock Screen pane writes it through its helper.
//!
//! ```text
//! [Login]
//! Show=list|name
//! ```

use gtk::glib;

pub const PATH: &str = "/etc/elavo/greeter.conf";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Show {
    /// Pictures and names to choose from.
    #[default]
    List,
    /// A Name field and a Password field.
    Name,
}

pub fn load() -> Show {
    let file = glib::KeyFile::new();
    if file.load_from_file(PATH, glib::KeyFileFlags::NONE).is_err() {
        return Show::default();
    }
    parse(file.string("Login", "Show").ok().as_deref())
}

fn parse(value: Option<&str>) -> Show {
    match value.map(str::trim) {
        Some("name") => Show::Name,
        _ => Show::List,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_unless_name() {
        assert_eq!(parse(None), Show::List);
        assert_eq!(parse(Some("list")), Show::List);
        assert_eq!(parse(Some(" name ")), Show::Name);
        assert_eq!(parse(Some("names")), Show::List);
    }
}
