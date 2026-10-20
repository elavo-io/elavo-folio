//! Copy a person's wallpaper where the greeter can read it. Run as root
//! through pkexec by `elavo-wallpaper`; polkit allows the active session
//! without a password (`io.elavo.greeter.publish-wallpaper`).
//!
//! ```text
//! pkexec /usr/libexec/elavo-greeter-publish PICTURE
//! ```
//!
//! It writes `/var/lib/elavo/greeter/wallpapers/<uid>`, mode 0644, for the
//! uid pkexec reports. The picture must be a regular file the caller owns,
//! or one of the system's backgrounds, at most 32 MiB, and a JPEG, PNG, or
//! WebP.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use std::process::ExitCode;

const DIR: &str = "/var/lib/elavo/greeter/wallpapers";
const SYSTEM: &str = "/usr/share/backgrounds/";
const LIMIT: u64 = 32 * 1024 * 1024;

fn open(path: &Path) -> Result<File, String> {
    fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// The picture's bytes, after the checks in the module comment. Ownership
/// is checked on the opened file, so swapping a path component after
/// `canonicalize` can only reach a file the caller owns.
fn read_allowed(path: &Path, uid: u32) -> Result<Vec<u8>, String> {
    let real = fs::canonicalize(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let file = open(&real)?;
    let meta = file.metadata().map_err(|e| e.to_string())?;
    if meta.uid() != uid && !real.starts_with(SYSTEM) {
        return Err(format!("{} is not yours", path.display()));
    }
    if !meta.file_type().is_file() {
        return Err(format!("{} is not a regular file", path.display()));
    }
    if meta.len() > LIMIT {
        return Err(format!("{} is larger than 32 MiB", path.display()));
    }
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    file.take(LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > LIMIT {
        return Err(format!("{} is larger than 32 MiB", path.display()));
    }
    if !is_picture(&bytes) {
        return Err(format!("{} is not a JPEG, PNG, or WebP", path.display()));
    }
    Ok(bytes)
}

fn is_picture(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0xFF, 0xD8, 0xFF])
        || bytes.starts_with(b"\x89PNG\r\n\x1a\n")
        || (bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP")
}

fn write(uid: u32, bytes: &[u8]) -> Result<(), String> {
    let dir = Path::new(DIR);
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let target = dir.join(uid.to_string());
    let temp: PathBuf = dir.join(format!(".{uid}.tmp"));
    let _ = fs::remove_file(&temp);
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o644)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&temp)
        .map_err(|e| e.to_string())?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|e| e.to_string())?;
    fs::set_permissions(&temp, fs::Permissions::from_mode(0o644)).map_err(|e| e.to_string())?;
    fs::rename(&temp, &target).map_err(|e| e.to_string())
}

fn run() -> Result<(), String> {
    if unsafe { libc::geteuid() } != 0 {
        return Err("run through pkexec".into());
    }
    let uid: u32 = std::env::var("PKEXEC_UID")
        .ok()
        .and_then(|v| v.parse().ok())
        .ok_or("PKEXEC_UID is not set")?;
    let mut args = std::env::args_os().skip(1);
    let (Some(path), None) = (args.next(), args.next()) else {
        return Err("usage: elavo-greeter-publish PICTURE".into());
    };
    let bytes = read_allowed(Path::new(&path), uid)?;
    write(uid, &bytes)
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("elavo-greeter-publish: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picture_headers() {
        assert!(is_picture(&[0xFF, 0xD8, 0xFF, 0xE0]));
        assert!(is_picture(b"\x89PNG\r\n\x1a\nrest"));
        assert!(is_picture(b"RIFF\0\0\0\0WEBPVP8 "));
        assert!(!is_picture(b"root:x:0:0"));
        assert!(!is_picture(b""));
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("elavo-publish-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn only_the_callers_own_pictures() {
        let dir = scratch("own");
        let picture = dir.join("mine.jpg");
        fs::write(&picture, [0xFF, 0xD8, 0xFF, 0xE0, 1, 2, 3]).unwrap();
        let me = unsafe { libc::getuid() };
        assert!(read_allowed(&picture, me).is_ok());
        assert!(read_allowed(&picture, me + 1).is_err());

        let text = dir.join("notes.jpg");
        fs::write(&text, b"not a picture").unwrap();
        assert!(read_allowed(&text, me).is_err());

        // A link to a file the caller does not own is refused, even when
        // the link itself is theirs.
        let link = dir.join("shadow.jpg");
        std::os::unix::fs::symlink("/etc/passwd", &link).unwrap();
        assert!(read_allowed(&link, me).is_err());
        let _ = fs::remove_dir_all(&dir);
    }
}
