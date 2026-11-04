//! After a file drop on an application icon, make that app active.
//!
//! Running apps are raised immediately through foreign-toplevel activate.
//! A short watch then focuses the first *new* toplevel of that app, covering
//! apps that open a fresh window for the drop and cold launches where the
//! XDG activation token is ignored (common with Electron).

use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::time::{Duration, Instant};

/// How long after a drop we still raise a newly appeared window of the app.
pub const TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Clone, Debug)]
pub struct Watch<K> {
    /// Toplevel keys that already belonged to the app at drop time.
    pub existing: HashSet<K>,
    pub until: Instant,
}

impl<K: Eq + Hash> Watch<K> {
    pub fn new(existing: HashSet<K>, now: Instant) -> Self {
        Self {
            existing,
            until: now + TIMEOUT,
        }
    }
}

/// Returns whether `key` is a new toplevel that should be activated for
/// `desktop_id`, and removes the watch when it fires or has expired.
pub fn take_new_to_focus<K: Eq + Hash + Clone>(
    pending: &mut HashMap<String, Watch<K>>,
    desktop_id: &str,
    key: &K,
    now: Instant,
) -> bool {
    let Some(watch) = pending.get(desktop_id) else {
        return false;
    };
    if now > watch.until {
        pending.remove(desktop_id);
        return false;
    }
    if watch.existing.contains(key) {
        return false;
    }
    pending.remove(desktop_id);
    true
}

/// Drop expired watches; same cleanup rhythm as the launch bounce.
pub fn expire<K>(pending: &mut HashMap<String, Watch<K>>, now: Instant) {
    pending.retain(|_, w| now <= w.until);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_new_toplevel_is_taken_once() {
        let mut pending = HashMap::new();
        pending.insert(
            "cursor.desktop".into(),
            Watch::new(HashSet::from([1u32, 2]), Instant::now()),
        );
        assert!(!take_new_to_focus(
            &mut pending,
            "cursor.desktop",
            &1,
            Instant::now()
        ));
        assert!(take_new_to_focus(
            &mut pending,
            "cursor.desktop",
            &3,
            Instant::now()
        ));
        assert!(!take_new_to_focus(
            &mut pending,
            "cursor.desktop",
            &4,
            Instant::now()
        ));
        assert!(pending.is_empty());
    }

    #[test]
    fn cold_launch_has_no_existing_windows() {
        let mut pending = HashMap::new();
        pending.insert(
            "app.desktop".into(),
            Watch::new(HashSet::new(), Instant::now()),
        );
        assert!(take_new_to_focus(
            &mut pending,
            "app.desktop",
            &10,
            Instant::now()
        ));
    }

    #[test]
    fn expired_watch_is_dropped_without_focus() {
        let mut pending = HashMap::new();
        let past = Instant::now() - TIMEOUT - Duration::from_secs(1);
        pending.insert(
            "app.desktop".into(),
            Watch {
                existing: HashSet::new(),
                until: past + TIMEOUT,
            },
        );
        assert!(!take_new_to_focus(
            &mut pending,
            "app.desktop",
            &1,
            Instant::now()
        ));
        assert!(pending.is_empty());
    }

    #[test]
    fn expire_sweeps_stale_entries() {
        let mut pending = HashMap::new();
        let now = Instant::now();
        pending.insert(
            "fresh.desktop".into(),
            Watch::new(HashSet::<u32>::new(), now),
        );
        pending.insert(
            "stale.desktop".into(),
            Watch {
                existing: HashSet::<u32>::new(),
                until: now - Duration::from_secs(1),
            },
        );
        expire(&mut pending, now);
        assert!(pending.contains_key("fresh.desktop"));
        assert!(!pending.contains_key("stale.desktop"));
    }
}
