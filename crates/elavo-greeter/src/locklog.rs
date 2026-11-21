//! The lock screen's log, `~/.local/state/elavo/lock.log`.
//!
//! The lock runs from elavo-idle.service, and nothing it printed used to
//! reach a place anyone looked. `open` points stderr at the log, so GTK's
//! and GLib's warnings land there too, next to the lock's own notes. The
//! log never holds what is typed into the password field.

use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;

use gtk::glib;

/// Set once stderr is the log, so a lock started by the keeper does not
/// open it again.
const OPEN_VAR: &str = "ELAVO_GREETER_LOCK_LOG";
/// The log is moved to `lock.log.1` when it passes this size.
const MAX_BYTES: u64 = 512 * 1024;

fn path() -> Option<PathBuf> {
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))?;
    Some(state.join("elavo").join("lock.log"))
}

/// Send stderr to the log. Keeps stderr as it was when the log cannot be
/// opened.
pub fn open() {
    if std::env::var_os(OPEN_VAR).is_some() {
        return;
    }
    let Some(path) = path() else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_BYTES) {
        let _ = std::fs::rename(&path, path.with_extension("log.1"));
    }
    let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(&path)
    else {
        return;
    };
    unsafe {
        libc::dup2(file.as_raw_fd(), libc::STDERR_FILENO);
    }
    std::env::set_var(OPEN_VAR, "1");
}

/// One line in the log, with the local time and the process.
pub fn note(text: &str) {
    let time = glib::DateTime::now_local()
        .ok()
        .and_then(|now| {
            let ms = now.microsecond() / 1000;
            now.format("%F %T").ok().map(|s| format!("{s}.{ms:03}"))
        })
        .unwrap_or_default();
    let mut err = std::io::stderr().lock();
    let _ = writeln!(err, "{time} elavo-greeter[{}]: {text}", std::process::id());
}
