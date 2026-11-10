//! Who can answer a request. polkitd sends `unix-user` and `unix-group`
//! identities; a group stands for its members.

use std::ffi::{CStr, CString};

use gtk::glib;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Person {
    pub uid: u32,
    pub user: String,
    pub real_name: String,
}

impl Person {
    pub fn from_uid(uid: u32) -> Option<Self> {
        let mut buf = vec![0u8; 4096];
        loop {
            let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
            let mut out = std::ptr::null_mut();
            let rc = unsafe {
                libc::getpwuid_r(uid, &mut pwd, buf.as_mut_ptr().cast(), buf.len(), &mut out)
            };
            if rc == libc::ERANGE && buf.len() < 1 << 20 {
                buf.resize(buf.len() * 2, 0);
                continue;
            }
            if rc != 0 || out.is_null() {
                return None;
            }
            let user = unsafe { CStr::from_ptr(pwd.pw_name) }
                .to_string_lossy()
                .into_owned();
            let gecos = if pwd.pw_gecos.is_null() {
                String::new()
            } else {
                unsafe { CStr::from_ptr(pwd.pw_gecos) }
                    .to_string_lossy()
                    .into_owned()
            };
            return Some(Self {
                uid,
                user,
                real_name: real_name(&gecos),
            });
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        let name = CString::new(name).ok()?;
        let mut buf = vec![0u8; 4096];
        loop {
            let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
            let mut out = std::ptr::null_mut();
            let rc = unsafe {
                libc::getpwnam_r(
                    name.as_ptr(),
                    &mut pwd,
                    buf.as_mut_ptr().cast(),
                    buf.len(),
                    &mut out,
                )
            };
            if rc == libc::ERANGE && buf.len() < 1 << 20 {
                buf.resize(buf.len() * 2, 0);
                continue;
            }
            if rc != 0 || out.is_null() {
                return None;
            }
            return Self::from_uid(pwd.pw_uid);
        }
    }

    /// The real name when the account has one, else the user name.
    pub fn display_name(&self) -> &str {
        if self.real_name.is_empty() {
            &self.user
        } else {
            &self.real_name
        }
    }
}

/// The first GECOS field is the real name.
fn real_name(gecos: &str) -> String {
    gecos.split(',').next().unwrap_or("").trim().to_owned()
}

fn group_members(gid: u32) -> Vec<String> {
    let mut buf = vec![0u8; 4096];
    loop {
        let mut grp: libc::group = unsafe { std::mem::zeroed() };
        let mut out = std::ptr::null_mut();
        let rc = unsafe {
            libc::getgrgid_r(gid, &mut grp, buf.as_mut_ptr().cast(), buf.len(), &mut out)
        };
        if rc == libc::ERANGE && buf.len() < 1 << 20 {
            buf.resize(buf.len() * 2, 0);
            continue;
        }
        if rc != 0 || out.is_null() || grp.gr_mem.is_null() {
            return Vec::new();
        }
        let mut members = Vec::new();
        let mut i = 0;
        loop {
            let member = unsafe { *grp.gr_mem.add(i) };
            if member.is_null() {
                break;
            }
            members.push(
                unsafe { CStr::from_ptr(member) }
                    .to_string_lossy()
                    .into_owned(),
            );
            i += 1;
        }
        return members;
    }
}

/// The people behind polkitd's `a(sa{sv})` identities, in its order and
/// without repeats, and the person at the keyboard when they are one of
/// them.
pub fn people(identities: &glib::Variant, me: u32) -> (Vec<Person>, Option<usize>) {
    let mut uids: Vec<u32> = Vec::new();
    for i in 0..identities.n_children() {
        let identity = identities.child_value(i);
        if identity.n_children() < 2 {
            continue;
        }
        let kind = identity.child_value(0);
        let dict = glib::VariantDict::new(Some(&identity.child_value(1)));
        match kind.str().unwrap_or("") {
            "unix-user" => {
                if let Ok(Some(uid)) = dict.lookup::<u32>("uid") {
                    uids.push(uid);
                }
            }
            "unix-group" => {
                if let Ok(Some(gid)) = dict.lookup::<u32>("gid") {
                    uids.extend(
                        group_members(gid)
                            .iter()
                            .filter_map(|name| Person::from_name(name))
                            .map(|p| p.uid),
                    );
                }
            }
            _ => {}
        }
    }
    let mut seen = Vec::new();
    uids.retain(|uid| {
        let new = !seen.contains(uid);
        seen.push(*uid);
        new
    });
    let people: Vec<Person> = uids.into_iter().filter_map(Person::from_uid).collect();
    let start = people.iter().position(|p| p.uid == me);
    (people, start)
}

/// The listed person a typed name means: their user name or real name,
/// ignoring case and surrounding spaces.
pub fn find(people: &[Person], typed: &str) -> Option<usize> {
    let typed = typed.trim();
    if typed.is_empty() {
        return None;
    }
    let same = |a: &str| !a.is_empty() && a.to_lowercase() == typed.to_lowercase();
    people
        .iter()
        .position(|p| same(&p.user))
        .or_else(|| people.iter().position(|p| same(&p.real_name)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gtk::prelude::*;
    use std::collections::HashMap;

    #[test]
    fn real_name_is_the_first_gecos_field() {
        assert_eq!(real_name("Kevin Doyle,,,"), "Kevin Doyle");
        assert_eq!(real_name(""), "");
    }

    #[test]
    fn root_is_found_and_starts_when_it_is_me() {
        let root = HashMap::from([("uid".to_owned(), 0u32.to_variant())]);
        let identities = vec![
            ("unix-user".to_owned(), root.clone()),
            ("unix-user".to_owned(), root),
        ]
        .to_variant();
        let (people, start) = people(&identities, 0);
        assert_eq!(people.len(), 1);
        assert_eq!(people[0].user, "root");
        assert_eq!(start, Some(0));
    }

    #[test]
    fn typed_names_match_user_or_real_name() {
        let people = vec![
            Person {
                uid: 1000,
                user: "kevin".into(),
                real_name: "Kevin Doyle".into(),
            },
            Person {
                uid: 1001,
                user: "josh".into(),
                real_name: String::new(),
            },
        ];
        assert_eq!(find(&people, "kevin"), Some(0));
        assert_eq!(find(&people, "  Kevin Doyle "), Some(0));
        assert_eq!(find(&people, "JOSH"), Some(1));
        assert_eq!(find(&people, "root"), None);
        assert_eq!(find(&people, ""), None);
    }
}
