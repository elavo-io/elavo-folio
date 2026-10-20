//! The people who can sign in: AccountsService when it answers, otherwise
//! the ordinary accounts in `/etc/passwd`. Pictures are AccountsService's
//! `IconFile`, which any user may read.

use std::collections::HashMap;
use std::path::PathBuf;

use gtk::gio;
use gtk::glib::Variant;
use gtk::prelude::*;

const ACCOUNTS: &str = "org.freedesktop.Accounts";
const ACCOUNTS_PATH: &str = "/org/freedesktop/Accounts";
const USER: &str = "org.freedesktop.Accounts.User";
const LAST_USER: &str = "/var/lib/elavo/greeter/last-user";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Account {
    pub uid: u32,
    pub name: String,
    pub real_name: String,
    pub icon: Option<PathBuf>,
}

impl Account {
    pub fn display_name(&self) -> &str {
        if self.real_name.is_empty() {
            &self.name
        } else {
            &self.real_name
        }
    }

    pub fn initial(&self) -> String {
        self.display_name()
            .chars()
            .next()
            .map(|c| c.to_uppercase().collect())
            .unwrap_or_default()
    }
}

fn call(
    bus: &gio::DBusConnection,
    path: &str,
    interface: &str,
    method: &str,
    args: Option<gtk::glib::Variant>,
) -> Option<gtk::glib::Variant> {
    bus.call_sync(
        Some(ACCOUNTS),
        path,
        interface,
        method,
        args.as_ref(),
        None,
        gio::DBusCallFlags::NONE,
        2000,
        gio::Cancellable::NONE,
    )
    .ok()
}

fn from_path(bus: &gio::DBusConnection, path: &str) -> Option<Account> {
    let reply = call(
        bus,
        path,
        "org.freedesktop.DBus.Properties",
        "GetAll",
        Some((USER,).to_variant()),
    )?;
    let props: HashMap<String, Variant> = reply.child_value(0).get()?;
    let get = |key: &str| props.get(key);
    if get("SystemAccount").and_then(|v| v.get::<bool>()) == Some(true) {
        return None;
    }
    let text = |key: &str| get(key).and_then(|v| v.get::<String>()).unwrap_or_default();
    let icon = PathBuf::from(text("IconFile"));
    Some(Account {
        uid: u32::try_from(get("Uid").and_then(|v| v.get::<u64>())?).ok()?,
        name: text("UserName"),
        real_name: text("RealName"),
        icon: icon.is_file().then_some(icon),
    })
}

fn system_bus() -> Option<gio::DBusConnection> {
    gio::bus_get_sync(gio::BusType::System, gio::Cancellable::NONE).ok()
}

/// Every account that can sign in, by name.
pub fn list() -> Vec<Account> {
    let mut accounts = system_bus()
        .and_then(|bus| {
            let reply = call(&bus, ACCOUNTS_PATH, ACCOUNTS, "ListCachedUsers", None)?;
            let paths = reply
                .child_value(0)
                .get::<Vec<gtk::glib::variant::ObjectPath>>()?;
            Some(
                paths
                    .iter()
                    .filter_map(|p| from_path(&bus, p.as_str()))
                    .collect::<Vec<_>>(),
            )
        })
        .filter(|a| !a.is_empty())
        .unwrap_or_else(|| {
            std::fs::read_to_string("/etc/passwd")
                .map(|text| from_passwd(&text))
                .unwrap_or_default()
        });
    accounts.sort_by_key(|a| a.display_name().to_lowercase());
    accounts
}

/// The account this process runs as, for the lock screen.
pub fn current() -> Account {
    let uid = unsafe { libc::getuid() };
    let found = system_bus().and_then(|bus| {
        let reply = call(
            &bus,
            ACCOUNTS_PATH,
            ACCOUNTS,
            "FindUserById",
            Some((i64::from(uid),).to_variant()),
        )?;
        let path = reply.child_value(0);
        from_path(&bus, path.str()?)
    });
    found.unwrap_or_else(|| {
        let name = std::env::var("USER").unwrap_or_default();
        let real_name = std::fs::read_to_string("/etc/passwd")
            .ok()
            .and_then(|text| {
                from_passwd_all(&text)
                    .into_iter()
                    .find(|a| a.uid == uid)
                    .map(|a| a.real_name)
            })
            .unwrap_or_default();
        Account {
            uid,
            name,
            real_name,
            icon: None,
        }
    })
}

fn from_passwd_all(text: &str) -> Vec<Account> {
    text.lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split(':').collect();
            if fields.len() < 7 {
                return None;
            }
            Some((
                Account {
                    uid: fields[2].parse().ok()?,
                    name: fields[0].to_owned(),
                    real_name: fields[4].split(',').next().unwrap_or("").to_owned(),
                    icon: None,
                },
                fields[6],
            ))
        })
        .filter(|(_, shell)| !shell.ends_with("nologin") && !shell.ends_with("false"))
        .map(|(account, _)| account)
        .collect()
}

/// Ordinary accounts: UIDs from 1000 to 59999 with a login shell.
fn from_passwd(text: &str) -> Vec<Account> {
    from_passwd_all(text)
        .into_iter()
        .filter(|a| (1000..60000).contains(&a.uid))
        .collect()
}

const DEMO_PEOPLE: [&str; 8] = [
    "Maya Chen",
    "Jordan Okafor",
    "Sam",
    "Priya Raman",
    "Alexandria Montgomery-Whitfield",
    "Leo Duarte",
    "Noor Haddad",
    "Ines Kowalski",
];

/// `count` made-up people for `--demo`. They have no pictures, so they
/// show their initials.
pub fn demo_people(count: usize) -> Vec<Account> {
    DEMO_PEOPLE
        .iter()
        .take(count)
        .enumerate()
        .map(|(i, real_name)| Account {
            uid: 60100 + i as u32,
            name: real_name
                .split(' ')
                .next()
                .unwrap_or_default()
                .to_lowercase(),
            real_name: (*real_name).to_owned(),
            icon: None,
        })
        .collect()
}

/// The user name that signed in last, if it is still an account.
pub fn last_user(accounts: &[Account]) -> Option<usize> {
    let name = std::fs::read_to_string(LAST_USER).ok()?;
    accounts.iter().position(|a| a.name == name.trim())
}

pub fn remember(name: &str) {
    let _ = std::fs::write(LAST_USER, format!("{name}\n"));
}

#[cfg(test)]
mod tests {
    use super::*;

    const PASSWD: &str = "\
root:x:0:0:Super User:/root:/bin/bash
greetd:x:968:968::/var/lib/greetd:/sbin/nologin
ada:x:1000:1000:Ada Lovelace,,,:/home/ada:/bin/bash
bot:x:1001:1001::/home/bot:/usr/bin/false
nobody:x:65534:65534:Kernel Overflow User:/:/sbin/nologin
";

    #[test]
    fn passwd_keeps_people_with_a_shell() {
        let accounts = from_passwd(PASSWD);
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].name, "ada");
        assert_eq!(accounts[0].real_name, "Ada Lovelace");
        assert_eq!(accounts[0].display_name(), "Ada Lovelace");
        assert_eq!(accounts[0].initial(), "A");
    }

    #[test]
    fn an_account_without_a_full_name_shows_its_user_name() {
        let account = Account {
            uid: 1000,
            name: "kevin".into(),
            real_name: String::new(),
            icon: None,
        };
        assert_eq!(account.display_name(), "kevin");
        assert_eq!(account.initial(), "K");
    }
}
