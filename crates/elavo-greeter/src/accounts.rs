//! The people who can sign in: AccountsService when it answers, otherwise
//! the ordinary accounts in `/etc/passwd`. Directory accounts (Active
//! Directory, FreeIPA) are in neither list, so the ones that have used this
//! computer are added: a home in `/home`, or a wallpaper their session
//! published for the greeter. SELinux keeps the greeter (`xdm_t`) from
//! listing `/home`, so on the image the published wallpapers are what
//! finds them. Their UIDs are large (hundreds of millions and up), so a
//! person is any account from UID 1000 with a login shell, except nobody.
//! Pictures are AccountsService's `IconFile`, which any user may read.

use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::path::PathBuf;

use gtk::gio;
use gtk::glib::Variant;
use gtk::prelude::*;

const ACCOUNTS: &str = "org.freedesktop.Accounts";
const ACCOUNTS_PATH: &str = "/org/freedesktop/Accounts";
const USER: &str = "org.freedesktop.Accounts.User";
const LAST_USER: &str = "/var/lib/elavo/greeter/last-user";
const HOMES: &str = "/home";
/// The first UID for people. Below are system accounts.
const FIRST_UID: u32 = 1000;
/// nobody, and the two "no UID" values.
const NOT_PEOPLE: [u32; 3] = [65534, u32::MAX - 1, u32::MAX];

/// Whether an account with this UID and shell is a person.
fn is_person(uid: u32, shell: &str) -> bool {
    uid >= FIRST_UID
        && !NOT_PEOPLE.contains(&uid)
        && !shell.ends_with("nologin")
        && !shell.ends_with("false")
}

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
    let bus = system_bus();
    let mut accounts = bus
        .as_ref()
        .and_then(|bus| {
            let reply = call(bus, ACCOUNTS_PATH, ACCOUNTS, "ListCachedUsers", None)?;
            let paths = reply
                .child_value(0)
                .get::<Vec<gtk::glib::variant::ObjectPath>>()?;
            Some(
                paths
                    .iter()
                    .filter_map(|p| from_path(bus, p.as_str()))
                    .collect::<Vec<_>>(),
            )
        })
        .filter(|a| !a.is_empty())
        .unwrap_or_else(|| {
            std::fs::read_to_string("/etc/passwd")
                .map(|text| from_passwd(&text))
                .unwrap_or_default()
        });
    let known = home_names()
        .into_iter()
        .filter_map(|name| lookup(Key::Name(&name)))
        .chain(
            published_uids()
                .into_iter()
                .filter_map(|uid| lookup(Key::Uid(uid))),
        );
    for found in known {
        if accounts.iter().any(|a| a.uid == found.uid) {
            continue;
        }
        accounts.push(with_picture(bus.as_ref(), found));
    }
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
    found.or_else(|| lookup(Key::Uid(uid))).unwrap_or_else(|| {
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

/// People in a passwd file: from UID 1000 with a login shell. No upper
/// limit, so directory accounts in an `/etc/passwd`-style list are kept.
fn from_passwd(text: &str) -> Vec<Account> {
    from_passwd_all(text)
        .into_iter()
        .filter(|a| is_person(a.uid, "/bin/sh"))
        .collect()
}

/// Folder names in `/home`.
fn home_names() -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(HOMES) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| !n.starts_with('.'))
        .collect();
    names.sort();
    names
}

/// UIDs whose sessions published a wallpaper for the greeter.
fn published_uids() -> Vec<u32> {
    let Ok(entries) = std::fs::read_dir(crate::wallpaper::PUBLISHED) else {
        return Vec::new();
    };
    let mut uids: Vec<u32> = entries
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.parse().ok())
        .collect();
    uids.sort_unstable();
    uids
}

/// `found`, from the system's user database (sssd included), with
/// AccountsService's name and picture when it knows the account.
fn with_picture(bus: Option<&gio::DBusConnection>, found: Account) -> Account {
    let from_accounts = bus.and_then(|bus| {
        let reply = call(
            bus,
            ACCOUNTS_PATH,
            ACCOUNTS,
            "FindUserByName",
            Some((found.name.as_str(),).to_variant()),
        )?;
        let path = reply.child_value(0);
        from_path(bus, path.str()?)
    });
    match from_accounts {
        Some(account) if account.uid == found.uid => account,
        _ => found,
    }
}

enum Key<'a> {
    Name(&'a str),
    Uid(u32),
}

/// One account from the system's user database, if it is a person.
fn lookup(key: Key) -> Option<Account> {
    let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
    let mut result: *mut libc::passwd = std::ptr::null_mut();
    let mut buffer = vec![0 as libc::c_char; 16 * 1024];
    let status = match key {
        Key::Name(name) => {
            let name = CString::new(name).ok()?;
            unsafe {
                libc::getpwnam_r(
                    name.as_ptr(),
                    &mut entry,
                    buffer.as_mut_ptr(),
                    buffer.len(),
                    &mut result,
                )
            }
        }
        Key::Uid(uid) => unsafe {
            libc::getpwuid_r(
                uid,
                &mut entry,
                buffer.as_mut_ptr(),
                buffer.len(),
                &mut result,
            )
        },
    };
    if status != 0 || result.is_null() {
        return None;
    }
    let text = |p: *const libc::c_char| {
        if p.is_null() {
            String::new()
        } else {
            unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
        }
    };
    let shell = text(entry.pw_shell);
    if !is_person(entry.pw_uid, &shell) {
        return None;
    }
    Some(Account {
        uid: entry.pw_uid,
        name: text(entry.pw_name),
        real_name: text(entry.pw_gecos)
            .split(',')
            .next()
            .unwrap_or("")
            .to_owned(),
        icon: None,
    })
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
odd:x:65534:65534::/:/bin/bash
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
    fn directory_uids_are_people() {
        // Samba AD and FreeIPA in the test lab.
        assert!(is_person(1_738_201_105, "/bin/bash"));
        assert!(is_person(282_600_005, "/bin/bash"));
        assert!(is_person(60_100, "/bin/bash"));
        assert!(!is_person(999, "/bin/bash"));
        assert!(!is_person(65534, "/bin/bash"));
        assert!(!is_person(u32::MAX - 1, "/bin/bash"));
        assert!(!is_person(1000, "/sbin/nologin"));
        let text = "ada:*:1738201105:1738200513:Ada Lovelace:/home/ada:/bin/bash\n";
        assert_eq!(from_passwd(text)[0].uid, 1_738_201_105);
    }

    #[test]
    fn root_is_not_a_person() {
        assert_eq!(lookup(Key::Uid(0)), None);
        assert_eq!(lookup(Key::Name("elavo-no-such-user")), None);
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
