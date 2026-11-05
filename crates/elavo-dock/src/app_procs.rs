//! Finding an application's processes by the systemd unit it runs in.
//!
//! Desktops and Flatpak start apps in transient user units named after the
//! desktop id, so the cgroup of each process says which app it belongs to,
//! whoever launched it.

/// The unit name (last path component) from a `/proc/<pid>/cgroup` file,
/// using the cgroup v2 `0::` line.
pub fn unit_of(cgroup: &str) -> Option<&str> {
    cgroup
        .lines()
        .find_map(|line| line.strip_prefix("0::"))
        .and_then(|path| path.rsplit('/').next())
        .filter(|unit| !unit.is_empty())
}

/// Whether `unit` is an app unit for desktop id `id` (without `.desktop`):
/// `app-flatpak-<id>-<n>.scope`, `app-<launcher>-<id>-<random>.scope`,
/// `app-<launcher>-<id>@<random>.service` or `app-<id>...`, with `<id>`
/// plain or systemd-escaped. The id must be a whole component, so `org.foo`
/// never matches `org.foobar`.
pub fn unit_matches(unit: &str, id: &str) -> bool {
    if id.is_empty() {
        return false;
    }
    let Some(rest) = unit.strip_prefix("app-") else {
        return false;
    };
    let body = if let Some(body) = rest.strip_suffix(".scope") {
        body
    } else if let Some(body) = rest.strip_suffix(".service") {
        body
    } else {
        return false;
    };
    let escaped = id.replace('-', "\\x2d");
    let mut candidates = vec![body];
    if let Some((name, _)) = body.split_once('@') {
        candidates.push(name);
    }
    if let Some((name, _)) = body
        .rsplit_once('-')
        .filter(|(_, tail)| !tail.is_empty() && tail.chars().all(|c| c.is_ascii_alphanumeric()))
    {
        candidates.push(name);
    }
    candidates.iter().any(|name| {
        [id, escaped.as_str()].iter().any(|form| {
            *name == *form
                || name.strip_suffix(form).is_some_and(|launcher| {
                    launcher
                        .strip_suffix('-')
                        .is_some_and(|l| !l.is_empty() && !l.contains('-'))
                })
        })
    })
}

/// One process as seen in `/proc`.
pub struct Proc<'a> {
    pub pid: i32,
    pub uid: u32,
    pub cgroup: &'a str,
}

/// PIDs among `procs` running in a unit of `id`, owned by `uid`, and neither
/// PID 1 nor `own_pid`.
pub fn matching_pids<'a>(
    procs: impl IntoIterator<Item = Proc<'a>>,
    id: &str,
    uid: u32,
    own_pid: i32,
) -> Vec<i32> {
    procs
        .into_iter()
        .filter(|p| p.pid > 1 && p.pid != own_pid && p.uid == uid)
        .filter(|p| unit_of(p.cgroup).is_some_and(|unit| unit_matches(unit, id)))
        .map(|p| p.pid)
        .collect()
}

/// Scan `/proc` for processes of desktop id `id` owned by the current user.
pub fn scan(id: &str) -> Vec<i32> {
    use std::os::unix::fs::MetadataExt;
    let Ok(proc) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    let entries: Vec<(i32, u32, String)> = proc
        .flatten()
        .filter_map(|entry| {
            let pid = entry.file_name().to_string_lossy().parse::<i32>().ok()?;
            let uid = entry.metadata().ok()?.uid();
            let cgroup = std::fs::read_to_string(entry.path().join("cgroup")).ok()?;
            Some((pid, uid, cgroup))
        })
        .collect();
    let uid = unsafe { libc::getuid() };
    matching_pids(
        entries.iter().map(|(pid, uid, cgroup)| Proc {
            pid: *pid,
            uid: *uid,
            cgroup,
        }),
        id,
        uid,
        std::process::id() as i32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const BRAVE: &str = "com.brave.Browser";

    fn cg(unit: &str) -> String {
        format!("0::/user.slice/user-1000.slice/user@1000.service/app.slice/{unit}\n")
    }

    #[test]
    fn reads_v2_unit() {
        assert_eq!(
            unit_of(&cg("app-flatpak-com.brave.Browser-123.scope")),
            Some("app-flatpak-com.brave.Browser-123.scope")
        );
        assert_eq!(unit_of("1:name=systemd:/x\n"), None);
    }

    #[test]
    fn flatpak_scope() {
        assert!(unit_matches(
            "app-flatpak-com.brave.Browser-48213.scope",
            BRAVE
        ));
        assert!(!unit_matches(
            "app-flatpak-com.brave.Browser-48213.slice",
            BRAVE
        ));
    }

    #[test]
    fn launcher_scopes_and_services() {
        assert!(unit_matches("app-gnome-org.foo-4821.scope", "org.foo"));
        assert!(unit_matches("app-elavo-org.foo@a1b2c3.service", "org.foo"));
        assert!(unit_matches("app-org.foo@a1b2c3.service", "org.foo"));
        assert!(unit_matches("app-org.foo-a1b2.scope", "org.foo"));
    }

    #[test]
    fn escaped_ids() {
        assert!(unit_matches("app-gnome-my\\x2dapp-99.scope", "my-app"));
        assert!(unit_matches(
            "app-flatpak-org.x.My\\x2dApp-7.scope",
            "org.x.My-App"
        ));
        assert!(!unit_matches(
            "app-gnome-my\\x2dapp\\x2dpro-99.scope",
            "my-app"
        ));
    }

    #[test]
    fn no_prefix_matches() {
        assert!(!unit_matches("app-flatpak-org.foobar-12.scope", "org.foo"));
        assert!(!unit_matches("app-gnome-org.foobar@x.service", "org.foo"));
        assert!(!unit_matches("app-gnome-xorg.foo-12.scope", "org.foo"));
        assert!(!unit_matches("app-a-b-org.foo-12.scope", "org.foo"));
        assert!(!unit_matches("session-2.scope", "org.foo"));
        assert!(!unit_matches("app-flatpak-org.foo-1.scope", ""));
    }

    #[test]
    fn excludes_self_init_and_other_users() {
        let brave = cg("app-flatpak-com.brave.Browser-1.scope");
        let other = cg("app-flatpak-org.other-1.scope");
        let procs = [
            (1, 1000, brave.as_str()),
            (50, 1000, brave.as_str()),
            (51, 1000, brave.as_str()),
            (52, 0, brave.as_str()),
            (53, 1000, other.as_str()),
        ];
        let found = matching_pids(
            procs
                .iter()
                .map(|&(pid, uid, cgroup)| Proc { pid, uid, cgroup }),
            BRAVE,
            1000,
            51,
        );
        assert_eq!(found, vec![50]);
    }
}
