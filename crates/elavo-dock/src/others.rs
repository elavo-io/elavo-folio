//! The files and folders after the divider. `$XDG_CONFIG_HOME/elavo/dock-others`
//! has one item per line: a URI, then tab-separated `key=value` options, as in
//! `file:///home/me/Downloads` TAB `sort=date-added` TAB `display=stack` TAB
//! `view=automatic`.
//!
//! Options apply to folders (stacks). Unknown options are ignored.

use std::path::PathBuf;

use gtk::glib;

use crate::pins::write_atomic;

/// An option with a file spelling and a menu label. `ALL` is menu order.
macro_rules! choice {
    ($name:ident, default $default:ident, $($variant:ident = $text:literal, $label:literal;)+) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum $name {
            $($variant,)+
        }

        impl Default for $name {
            fn default() -> Self {
                $name::$default
            }
        }

        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$variant,)+];

            pub fn as_str(self) -> &'static str {
                match self {
                    $($name::$variant => $text,)+
                }
            }

            pub fn label(self) -> &'static str {
                match self {
                    $($name::$variant => $label,)+
                }
            }

            pub fn parse(text: &str) -> Option<Self> {
                Self::ALL.iter().copied().find(|v| v.as_str() == text)
            }
        }
    };
}

choice!(Sort, default DateAdded,
    Name = "name", "Name";
    DateAdded = "date-added", "Date Added";
    DateModified = "date-modified", "Date Modified";
    DateCreated = "date-created", "Date Created";
    Kind = "kind", "Kind";
);

choice!(Display, default Stack,
    Folder = "folder", "Folder";
    Stack = "stack", "Stack";
);

choice!(View, default Automatic,
    Fan = "fan", "Fan";
    Grid = "grid", "Grid";
    List = "list", "List";
    Automatic = "automatic", "Automatic";
);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Other {
    pub uri: String,
    pub sort: Sort,
    pub display: Display,
    pub view: View,
}

impl Other {
    pub fn new(uri: String) -> Self {
        Self {
            uri,
            sort: Sort::default(),
            display: Display::default(),
            view: View::default(),
        }
    }

    fn to_line(&self) -> String {
        format!(
            "{}\tsort={}\tdisplay={}\tview={}",
            self.uri,
            self.sort.as_str(),
            self.display.as_str(),
            self.view.as_str()
        )
    }
}

fn path() -> PathBuf {
    glib::user_config_dir().join("elavo").join("dock-others")
}

pub fn parse(text: &str) -> Vec<Other> {
    let mut items: Vec<Other> = Vec::new();
    for line in text.lines() {
        let mut fields = line.split('\t').map(str::trim);
        let Some(uri) = fields
            .next()
            .filter(|u| !u.is_empty() && !u.starts_with('#'))
        else {
            continue;
        };
        if items.iter().any(|i| i.uri == uri) {
            continue;
        }
        let mut item = Other::new(uri.to_owned());
        for field in fields {
            match field.split_once('=') {
                Some(("sort", v)) => item.sort = Sort::parse(v).unwrap_or_default(),
                Some(("display", v)) => item.display = Display::parse(v).unwrap_or_default(),
                Some(("view", v)) => item.view = View::parse(v).unwrap_or_default(),
                _ => {}
            }
        }
        items.push(item);
    }
    items
}

pub fn load() -> Vec<Other> {
    match std::fs::read_to_string(path()) {
        Ok(text) => parse(&text),
        Err(_) => {
            let downloads = glib::user_special_dir(glib::UserDirectory::Downloads)
                .unwrap_or_else(|| glib::home_dir().join("Downloads"));
            let items = match glib::filename_to_uri(&downloads, None) {
                Ok(uri) => vec![Other::new(uri.to_string())],
                Err(_) => Vec::new(),
            };
            save(&items);
            items
        }
    }
}

pub fn save(items: &[Other]) {
    let mut text: String = items.iter().map(|i| i.to_line() + "\n").collect();
    if text.is_empty() {
        text.push_str("# Files and folders after the dock's divider.\n");
    }
    write_atomic(&path(), &text);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_options_and_defaults() {
        let text = "file:///a\tsort=name\tview=grid\tcolor=red\n\n#x\nfile:///b\nfile:///a\n";
        let items = parse(text);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].sort, Sort::Name);
        assert_eq!(items[0].view, View::Grid);
        assert_eq!(items[0].display, Display::Stack);
        assert_eq!(items[1], Other::new("file:///b".into()));
    }

    #[test]
    fn round_trip() {
        let item = Other {
            uri: "file:///home/me/Documents".into(),
            sort: Sort::Kind,
            display: Display::Folder,
            view: View::List,
        };
        assert_eq!(parse(&item.to_line()), vec![item]);
    }
}
