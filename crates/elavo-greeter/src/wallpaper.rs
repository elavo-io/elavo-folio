//! The picture behind the greeter, and the bar's ink over it.
//!
//! Each session publishes a copy of its wallpaper to `PUBLISHED/<uid>`
//! (`elavo-greeter-publish`), because the greeter cannot read home
//! directories. The lock screen runs as the person and reads the file
//! `elavo-wallpaper` writes in the runtime directory instead.

use std::path::{Path, PathBuf};

use gtk::prelude::*;
use gtk::{gdk, gdk_pixbuf::Pixbuf, gio, glib};

pub const PUBLISHED: &str = "/var/lib/elavo/greeter/wallpapers";
const DEFAULT_ROOT: &str = "/usr/share/backgrounds/elavo";
/// Wide enough to cover a 4K output without decoding a 5K original.
const DECODE_WIDTH: i32 = 3840;
/// WCAG AA for normal text.
const AA: f64 = 4.5;

/// The first light picture of the installed dynamic wallpapers, which is
/// also what a new account starts with.
pub fn default() -> Option<PathBuf> {
    let mut found = Vec::new();
    collect_light(Path::new(DEFAULT_ROOT), &mut found);
    found.sort();
    found.into_iter().next()
}

fn collect_light(dir: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.contains("source") {
            continue;
        }
        if path.is_dir() {
            collect_light(&path, found);
        } else if is_light(&name) {
            found.push(path);
        }
    }
}

fn is_light(name: &str) -> bool {
    let Some((stem, ext)) = name.rsplit_once('.') else {
        return false;
    };
    matches!(ext, "jpg" | "jpeg" | "png" | "webp") && (stem == "light" || stem.ends_with("-light"))
}

/// The published copy for `uid`, or the default picture.
pub fn for_uid(uid: u32) -> Option<PathBuf> {
    let published = Path::new(PUBLISHED).join(uid.to_string());
    if published.is_file() {
        Some(published)
    } else {
        default()
    }
}

/// The picture on screen in this session.
pub fn session() -> Option<PathBuf> {
    let runtime = glib::user_runtime_dir();
    std::fs::read_to_string(runtime.join("elavo/wallpaper"))
        .ok()
        .map(|s| PathBuf::from(s.trim()))
        .filter(|p| p.is_file())
        .or_else(default)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ink {
    White,
    Black,
}

impl Ink {
    pub fn css_class(self) -> &'static str {
        match self {
            Ink::White => "on-dark",
            Ink::Black => "on-light",
        }
    }
}

#[derive(Clone)]
pub struct Picture {
    pub texture: gdk::Texture,
    pub ink: Ink,
}

struct Decoded {
    bytes: glib::Bytes,
    width: i32,
    height: i32,
    stride: usize,
    alpha: bool,
    ink: Ink,
}

/// Decode `path` off the main thread. `aspect` is the output's width over
/// its height, used to find the strip a 28px bar covers.
pub async fn load(path: PathBuf, aspect: f64, bar_share: f64) -> Option<Picture> {
    let decoded = gio::spawn_blocking(move || decode(&path, aspect, bar_share))
        .await
        .ok()
        .flatten()?;
    let format = if decoded.alpha {
        gdk::MemoryFormat::R8g8b8a8
    } else {
        gdk::MemoryFormat::R8g8b8
    };
    let texture = gdk::MemoryTexture::new(
        decoded.width,
        decoded.height,
        format,
        &decoded.bytes,
        decoded.stride,
    );
    Some(Picture {
        texture: texture.upcast(),
        ink: decoded.ink,
    })
}

fn decode(path: &Path, aspect: f64, bar_share: f64) -> Option<Decoded> {
    let pixbuf = Pixbuf::from_file_at_scale(path, DECODE_WIDTH, -1, true)
        .map_err(|e| glib::g_warning!("elavo-greeter", "{}: {e}", path.display()))
        .ok()?;
    let width = pixbuf.width();
    let height = pixbuf.height();
    let stride = pixbuf.rowstride() as usize;
    let channels = pixbuf.n_channels() as usize;
    let bytes = pixbuf.read_pixel_bytes();
    let ink = ink_for(&bytes, width, height, stride, channels, aspect, bar_share);
    Some(Decoded {
        bytes,
        width,
        height,
        stride,
        alpha: pixbuf.has_alpha(),
        ink,
    })
}

fn linear(channel: f64) -> f64 {
    if channel <= 0.04045 {
        channel / 12.92
    } else {
        ((channel + 0.055) / 1.055).powf(2.4)
    }
}

fn luminance(r: u8, g: u8, b: u8) -> f64 {
    let c = |v: u8| linear(f64::from(v) / 255.0);
    0.2126 * c(r) + 0.7152 * c(g) + 0.0722 * c(b)
}

fn contrast(a: f64, b: f64) -> f64 {
    let (hi, lo) = if a >= b { (a, b) } else { (b, a) };
    (hi + 0.05) / (lo + 0.05)
}

/// The panel's rule, without its gradient: black when black passes AA
/// against the darkest tenth of the strip under the bar, white otherwise.
fn ink_for(
    bytes: &[u8],
    width: i32,
    height: i32,
    stride: usize,
    channels: usize,
    aspect: f64,
    bar_share: f64,
) -> Ink {
    let (w, h) = (f64::from(width), f64::from(height));
    let visible_w = (h * aspect).min(w);
    let visible_h = (w / aspect).min(h);
    let x0 = ((w - visible_w) / 2.0).max(0.0) as i32;
    let x1 = (x0 + visible_w as i32).min(width);
    let y0 = ((h - visible_h) / 2.0).max(0.0) as i32;
    let y1 = (y0 + (visible_h * bar_share).ceil() as i32).clamp(y0 + 1, height);
    let mut values = Vec::new();
    for y in (y0..y1).step_by(2) {
        for x in (x0..x1).step_by(4) {
            let i = y as usize * stride + x as usize * channels;
            if let Some(px) = bytes.get(i..i + 3) {
                values.push(luminance(px[0], px[1], px[2]));
            }
        }
    }
    if values.is_empty() {
        return Ink::White;
    }
    values.sort_by(f64::total_cmp);
    let dark = values[(values.len() - 1) / 10];
    if contrast(0.0, dark) >= AA {
        Ink::Black
    } else {
        Ink::White
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn light_pictures_by_name() {
        assert!(is_light("blue-light.jpg"));
        assert!(is_light("light.png"));
        assert!(!is_light("blue-dark.jpg"));
        assert!(!is_light("blue-thumb.jpg"));
        assert!(!is_light("lighthouse.jpg"));
    }

    fn flat(r: u8, g: u8, b: u8) -> Vec<u8> {
        [r, g, b].repeat(16 * 10)
    }

    #[test]
    fn bright_strip_takes_black_ink() {
        let px = flat(254, 188, 0);
        assert_eq!(ink_for(&px, 16, 10, 48, 3, 1.6, 0.1), Ink::Black);
    }

    #[test]
    fn deep_blue_strip_takes_white_ink() {
        let px = flat(3, 56, 225);
        assert_eq!(ink_for(&px, 16, 10, 48, 3, 1.6, 0.1), Ink::White);
    }
}
