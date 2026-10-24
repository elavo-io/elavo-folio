//! Preview images for desktop icons, shared with Files through the
//! freedesktop thumbnail cache (`~/.cache/thumbnails`). A thumbnail Files
//! already made is used as is. Otherwise the desktop runs the installed
//! thumbnailer for the content type, one file at a time, and writes the
//! result where Files will find it too.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::time::{Duration, Instant, UNIX_EPOCH};

use gtk::prelude::*;
use gtk::{gdk, gdk_pixbuf, gio, glib};

/// The freedesktop "large" size, which covers a 48 px icon at 2x and more.
const SIZE: i32 = 256;
const TIMEOUT: Duration = Duration::from_secs(30);

enum State {
    Ready(gdk::Texture),
    Pending,
    None,
}

struct Entry {
    mtime: u64,
    state: State,
}

struct Job {
    path: PathBuf,
    uri: String,
    mtime: u64,
    exec: Vec<String>,
}

pub struct Thumbnails {
    thumbnailers: Vec<(String, Vec<String>)>,
    cache: RefCell<HashMap<PathBuf, Entry>>,
    queue: RefCell<VecDeque<Job>>,
    running: Cell<bool>,
    on_ready: Box<dyn Fn()>,
}

impl Thumbnails {
    pub fn new(on_ready: impl Fn() + 'static) -> Rc<Self> {
        Rc::new(Self {
            thumbnailers: read_thumbnailers(),
            cache: RefCell::new(HashMap::new()),
            queue: RefCell::new(VecDeque::new()),
            running: Cell::new(false),
            on_ready: Box::new(on_ready),
        })
    }

    /// The preview for `path`, if there is one yet. A missing preview is
    /// queued, and `on_ready` runs once it exists.
    pub fn lookup(self: &Rc<Self>, path: &Path, mime: &str) -> Option<gdk::Texture> {
        let mtime = modified(path)?;
        if let Some(entry) = self.cache.borrow().get(path) {
            if entry.mtime == mtime {
                return match &entry.state {
                    State::Ready(texture) => Some(texture.clone()),
                    State::Pending | State::None => None,
                };
            }
        }

        let file = gio::File::for_path(path);
        if let Some(texture) = cached_thumbnail(&file) {
            self.store(path, mtime, State::Ready(texture.clone()));
            return Some(texture);
        }
        let Some(exec) = self.thumbnailer_for(mime) else {
            self.store(path, mtime, State::None);
            return None;
        };
        self.store(path, mtime, State::Pending);
        self.queue.borrow_mut().push_back(Job {
            path: path.to_path_buf(),
            uri: file.uri().to_string(),
            mtime,
            exec,
        });
        self.pump();
        None
    }

    fn store(&self, path: &Path, mtime: u64, state: State) {
        self.cache
            .borrow_mut()
            .insert(path.to_path_buf(), Entry { mtime, state });
    }

    fn thumbnailer_for(&self, mime: &str) -> Option<Vec<String>> {
        let exact = self
            .thumbnailers
            .iter()
            .find(|(types, _)| types.split(';').any(|kind| kind == mime));
        let found = exact.or_else(|| {
            self.thumbnailers.iter().find(|(types, _)| {
                types
                    .split(';')
                    .any(|kind| !kind.is_empty() && gio::content_type_is_a(mime, kind))
            })
        })?;
        Some(found.1.clone())
    }

    fn pump(self: &Rc<Self>) {
        if self.running.get() {
            return;
        }
        let Some(job) = self.queue.borrow_mut().pop_front() else {
            return;
        };
        self.running.set(true);
        let thumbs = Rc::clone(self);
        glib::spawn_future_local(async move {
            let Job {
                path,
                uri,
                mtime,
                exec,
            } = job;
            let source = path.clone();
            let made = gio::spawn_blocking(move || generate(&source, &uri, mtime, &exec))
                .await
                .ok()
                .flatten();
            let texture = made.and_then(|thumb| gdk::Texture::from_filename(thumb).ok());
            let current = thumbs
                .cache
                .borrow()
                .get(&path)
                .is_some_and(|entry| entry.mtime == mtime);
            if current {
                let ready = texture.is_some();
                thumbs.store(
                    &path,
                    mtime,
                    texture.map(State::Ready).unwrap_or(State::None),
                );
                if ready {
                    (thumbs.on_ready)();
                }
            }
            thumbs.running.set(false);
            thumbs.pump();
        });
    }
}

fn modified(path: &Path) -> Option<u64> {
    let time = std::fs::metadata(path).ok()?.modified().ok()?;
    Some(time.duration_since(UNIX_EPOCH).ok()?.as_secs())
}

fn cached_thumbnail(file: &gio::File) -> Option<gdk::Texture> {
    let info = file
        .query_info(
            "thumbnail::path,thumbnail::is-valid",
            gio::FileQueryInfoFlags::NONE,
            None::<&gio::Cancellable>,
        )
        .ok()?;
    if !info.boolean("thumbnail::is-valid") {
        return None;
    }
    let path = info.attribute_byte_string("thumbnail::path")?;
    gdk::Texture::from_filename(path.as_str()).ok()
}

/// `*.thumbnailer` files, the user's first, then each system data dir.
/// The first entry that names a content type wins it.
fn read_thumbnailers() -> Vec<(String, Vec<String>)> {
    let mut dirs = vec![glib::user_data_dir()];
    dirs.extend(glib::system_data_dirs());
    let mut found = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir.join("thumbnailers")) else {
            continue;
        };
        let mut paths: Vec<PathBuf> = entries
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.extension().is_some_and(|ext| ext == "thumbnailer"))
            .collect();
        paths.sort();
        for path in paths {
            let Some(name) = path.file_name().map(|name| name.to_os_string()) else {
                continue;
            };
            if !seen.insert(name) {
                continue;
            }
            if let Some(entry) = parse_thumbnailer(&path) {
                found.push(entry);
            }
        }
    }
    found
}

fn parse_thumbnailer(path: &Path) -> Option<(String, Vec<String>)> {
    let keys = glib::KeyFile::new();
    keys.load_from_file(path, glib::KeyFileFlags::NONE).ok()?;
    let group = "Thumbnailer Entry";
    if let Ok(try_exec) = keys.string(group, "TryExec") {
        let try_exec = try_exec.as_str();
        let present = if try_exec.starts_with('/') {
            Path::new(try_exec).exists()
        } else {
            glib::find_program_in_path(try_exec).is_some()
        };
        if !present {
            return None;
        }
    }
    let exec = keys.string(group, "Exec").ok()?;
    let types = keys.string(group, "MimeType").ok()?;
    let argv = glib::shell_parse_argv(exec.as_str()).ok()?;
    let argv: Vec<String> = argv
        .into_iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    if argv.is_empty() {
        return None;
    }
    Some((types.to_string(), argv))
}

fn expand(arg: &str, uri: &str, input: &str, output: &str) -> String {
    let mut out = String::with_capacity(arg.len());
    let mut chars = arg.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('u') => out.push_str(uri),
            Some('i') => out.push_str(input),
            Some('o') => out.push_str(output),
            Some('s') => out.push_str(&SIZE.to_string()),
            Some('%') => out.push('%'),
            Some(other) => {
                out.push('%');
                out.push(other);
            }
            None => out.push('%'),
        }
    }
    out
}

/// Runs off the main thread. Returns the cache file it wrote.
fn generate(path: &Path, uri: &str, mtime: u64, exec: &[String]) -> Option<PathBuf> {
    let root = glib::user_cache_dir().join("thumbnails");
    let dir = root.join("large");
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)
        .ok()?;
    let md5 = glib::compute_checksum_for_string(glib::ChecksumType::Md5, uri)?;
    let target = dir.join(format!("{md5}.png"));
    let pid = std::process::id();
    let raw = dir.join(format!(".{md5}-{pid}.raw.png"));
    let tagged = dir.join(format!(".{md5}-{pid}.png"));

    let input = path.to_string_lossy();
    let output = raw.to_string_lossy();
    let args: Vec<String> = exec
        .iter()
        .map(|arg| expand(arg, uri, &input, &output))
        .collect();
    let ran = run(&args);
    let result = ran.then(|| tag(&raw, &tagged, uri, mtime)).flatten();
    let _ = std::fs::remove_file(&raw);
    if result.is_none() {
        let _ = std::fs::remove_file(&tagged);
        return None;
    }
    std::fs::rename(&tagged, &target).ok()?;
    Some(target)
}

fn run(args: &[String]) -> bool {
    let Some((program, rest)) = args.split_first() else {
        return false;
    };
    let Ok(mut child) = Command::new(program)
        .args(rest)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if start.elapsed() < TIMEOUT => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

/// Re-save the thumbnailer's output with the URI and modification time the
/// spec requires, so Files and GIO treat it as valid.
fn tag(raw: &Path, tagged: &Path, uri: &str, mtime: u64) -> Option<()> {
    let mut pixbuf = gdk_pixbuf::Pixbuf::from_file(raw).ok()?;
    let (width, height) = (pixbuf.width(), pixbuf.height());
    if width > SIZE || height > SIZE {
        let scale = f64::from(SIZE) / f64::from(width.max(height));
        let w = ((f64::from(width) * scale).round() as i32).max(1);
        let h = ((f64::from(height) * scale).round() as i32).max(1);
        pixbuf = pixbuf.scale_simple(w, h, gdk_pixbuf::InterpType::Bilinear)?;
    }
    let mtime = mtime.to_string();
    pixbuf
        .savev(
            tagged,
            "png",
            &[
                ("tEXt::Thumb::URI", uri),
                ("tEXt::Thumb::MTime", &mtime),
                ("tEXt::Software", "Elavo Desktop"),
            ],
        )
        .ok()?;
    std::fs::set_permissions(tagged, std::fs::Permissions::from_mode(0o600)).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exec_fields_expand() {
        assert_eq!(
            expand("--input", "file:///a", "/a", "/o.png"),
            "--input".to_string()
        );
        assert_eq!(expand("%u", "file:///a%20b", "/a b", "/o"), "file:///a%20b");
        assert_eq!(expand("%i", "u", "/a b", "/o"), "/a b");
        assert_eq!(expand("-o%o", "u", "i", "/o.png"), "-o/o.png");
        assert_eq!(expand("%s", "u", "i", "o"), "256");
        assert_eq!(expand("100%%", "u", "i", "o"), "100%");
    }
}
