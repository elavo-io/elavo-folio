//! `~/.config/elavo/keyboard.conf`. A missing file means the feature is on.

use std::ffi::CString;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

pub const FILE_NAME: &str = "keyboard.conf";

pub fn config_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("elavo").join(FILE_NAME))
}

pub fn load_path(path: &Path) -> bool {
    match std::fs::read_to_string(path) {
        Ok(text) => parse_enabled(&text),
        Err(_) => true,
    }
}

/// Missing key, missing group, and anything other than an explicit off stay on.
pub fn parse_enabled(text: &str) -> bool {
    let mut in_text = false;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[') {
            let Some(name) = name.strip_suffix(']') else {
                continue;
            };
            in_text = name.trim().eq_ignore_ascii_case("Text");
            continue;
        }
        if !in_text {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim().eq_ignore_ascii_case("AddPeriodWithDoubleSpace") {
            return !is_off(value.trim());
        }
    }
    true
}

fn is_off(value: &str) -> bool {
    matches!(
        value.to_ascii_lowercase().as_str(),
        "false" | "0" | "no" | "off"
    )
}

/// Watches the config directory. The caller reloads when `changed` is true.
pub struct Watch {
    fd: OwnedFd,
    dir: PathBuf,
    dir_wd: Option<i32>,
    parent_wd: Option<i32>,
}

impl Watch {
    pub fn open(path: &Path) -> io::Result<Self> {
        let raw = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        let dir = path
            .parent()
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "config path has no directory")
            })?
            .to_path_buf();
        let mut watch = Self {
            fd,
            dir,
            dir_wd: None,
            parent_wd: None,
        };
        watch.arm()?;
        Ok(watch)
    }

    pub fn fd(&self) -> i32 {
        self.fd.as_raw_fd()
    }

    /// Drain every pending event. True when `keyboard.conf` may have changed.
    pub fn changed(&mut self) -> bool {
        let mut buf = [0u8; 4096];
        let mut reload = false;
        loop {
            let n = unsafe { libc::read(self.fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
            if n < 0 {
                let err = io::Error::last_os_error();
                if err.kind() == io::ErrorKind::WouldBlock {
                    break;
                }
                return true;
            }
            if n == 0 {
                break;
            }
            let mut offset = 0;
            while offset + 16 <= n as usize {
                let mask = u32::from_ne_bytes(buf[offset + 4..offset + 8].try_into().unwrap());
                let len =
                    u32::from_ne_bytes(buf[offset + 12..offset + 16].try_into().unwrap()) as usize;
                let name_at = offset + 16;
                let name_end = (name_at + len).min(n as usize);
                let name = &buf[name_at..name_end];
                let name = name.split(|&b| b == 0).next().unwrap_or(&[]);
                if mask & libc::IN_Q_OVERFLOW != 0 {
                    reload = true;
                }
                if mask & libc::IN_IGNORED != 0 {
                    self.dir_wd = None;
                    self.parent_wd = None;
                    reload = true;
                }
                if name.is_empty() || name == FILE_NAME.as_bytes() || name == b"elavo" {
                    reload = true;
                }
                offset = name_end;
            }
        }
        if reload {
            let _ = self.arm();
        }
        reload
    }

    fn arm(&mut self) -> io::Result<()> {
        if self.dir.is_dir() {
            if self.dir_wd.is_none() {
                self.dir_wd = Some(add_watch(
                    self.fd.as_raw_fd(),
                    &self.dir,
                    libc::IN_CLOSE_WRITE
                        | libc::IN_MOVED_TO
                        | libc::IN_DELETE
                        | libc::IN_CREATE
                        | libc::IN_ATTRIB
                        | libc::IN_MOVE_SELF
                        | libc::IN_DELETE_SELF,
                )?);
            }
        } else if let Some(parent) = self.dir.parent() {
            if parent.is_dir() && self.parent_wd.is_none() {
                self.parent_wd = Some(add_watch(
                    self.fd.as_raw_fd(),
                    parent,
                    libc::IN_CREATE | libc::IN_MOVED_TO,
                )?);
            }
        }
        Ok(())
    }
}

fn add_watch(fd: i32, path: &Path, mask: u32) -> io::Result<i32> {
    let c = CString::new(path.as_os_str().as_bytes())
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    let wd = unsafe { libc::inotify_add_watch(fd, c.as_ptr(), mask) };
    if wd < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(wd)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_means_on() {
        assert!(parse_enabled(""));
        assert!(parse_enabled("[Text]\n"));
        assert!(parse_enabled("# AddPeriodWithDoubleSpace=false\n"));
        assert!(parse_enabled("[Other]\nAddPeriodWithDoubleSpace=false\n"));
    }

    #[test]
    fn the_key_glib_writes() {
        assert!(parse_enabled("[Text]\nAddPeriodWithDoubleSpace=true\n"));
        assert!(!parse_enabled("[Text]\nAddPeriodWithDoubleSpace=false\n"));
        assert!(!parse_enabled(
            "[ Text ]\nAddPeriodWithDoubleSpace = FALSE\n"
        ));
        assert!(!parse_enabled("[Text]\nAddPeriodWithDoubleSpace=0\n"));
        assert!(parse_enabled("[Text]\nAddPeriodWithDoubleSpace=yes\n"));
    }

    #[test]
    fn a_missing_file_is_on() {
        let path = std::env::temp_dir().join("elavo-input-missing-keyboard.conf");
        let _ = std::fs::remove_file(&path);
        assert!(load_path(&path));
    }
}
