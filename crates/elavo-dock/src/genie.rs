//! What the compositor needs for minimizing into the dock, and the window
//! pictures it leaves for minimized tiles. labwc-mods `mods/genie.c` reads
//! and writes the other side.
//!
//! Both live in `$XDG_RUNTIME_DIR/elavo/<WAYLAND_DISPLAY>/`:
//! `dock-targets` (written here) says where each icon rests, local to the
//! dock's surface on each output, and `minimized/<n>.png` with
//! `minimized/<n>.meta` (app ID, newline, title) is the window as it was
//! when it minimized.

use std::path::PathBuf;

use gtk::graphene;

pub fn dir() -> Option<PathBuf> {
    let display = std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "wayland-0".into());
    if display.is_empty() || display.contains('/') {
        return None;
    }
    Some(gtk::glib::user_runtime_dir().join("elavo").join(display))
}

/// Tabs and newlines separate fields and lines; a title keeps neither.
pub fn clean(text: &str) -> String {
    text.chars()
        .map(|c| {
            if matches!(c, '\t' | '\n' | '\r') {
                ' '
            } else {
                c
            }
        })
        .collect()
}

fn rect(r: &graphene::Rect) -> String {
    format!(
        "{}\t{}\t{}\t{}",
        r.x().round() as i32,
        r.y().round() as i32,
        r.width().round().max(1.0) as i32,
        r.height().round().max(1.0) as i32
    )
}

#[derive(Default)]
pub struct Targets {
    pub lines: Vec<String>,
}

impl Targets {
    pub fn new(animate: bool) -> Self {
        Self {
            lines: vec![format!("animate\t{}", u8::from(animate))],
        }
    }

    pub fn slot(&mut self, output: &str, r: &graphene::Rect) {
        self.lines
            .push(format!("slot\t{}\t{}", clean(output), rect(r)));
    }

    pub fn app(&mut self, output: &str, app_id: &str, r: &graphene::Rect) {
        self.lines.push(format!(
            "app\t{}\t{}\t{}",
            clean(output),
            clean(app_id),
            rect(r)
        ));
    }

    pub fn window(&mut self, output: &str, app_id: &str, title: &str, r: &graphene::Rect) {
        self.lines.push(format!(
            "window\t{}\t{}\t{}\t{}",
            clean(output),
            clean(app_id),
            clean(title),
            rect(r)
        ));
    }

    pub fn text(&self) -> String {
        let mut text = self.lines.join("\n");
        text.push('\n');
        text
    }

    /// Replace the file in one step, so the compositor never reads half.
    pub fn write(&self) {
        let Some(dir) = dir() else {
            return;
        };
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        let path = dir.join("dock-targets");
        let part = dir.join("dock-targets.part");
        if std::fs::write(&part, self.text()).is_ok() {
            let _ = std::fs::rename(&part, &path);
        }
    }
}

/// A picture the compositor left: its number and file.
#[derive(Debug, PartialEq)]
pub struct Picture {
    pub n: u64,
    pub png: PathBuf,
}

/// The newest picture of a window with this app ID, preferring one with
/// this title, that is not in `taken`.
pub fn find_picture(app_id: &str, title: &str, taken: &[u64]) -> Option<Picture> {
    let dir = dir()?.join("minimized");
    let mut best: Option<(bool, u64)> = None;
    for entry in std::fs::read_dir(&dir).ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(n) = name
            .strip_suffix(".meta")
            .and_then(|n| n.parse::<u64>().ok())
        else {
            continue;
        };
        if taken.contains(&n) {
            continue;
        }
        let Ok(meta) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        let mut lines = meta.lines();
        if lines.next() != Some(clean(app_id).as_str()) {
            continue;
        }
        let titled = lines.next() == Some(clean(title).as_str());
        if best.is_none_or(|b| (titled, n) > b) {
            best = Some((titled, n));
        }
    }
    let (_, n) = best?;
    let png = dir.join(format!("{n}.png"));
    png.exists().then_some(Picture { n, png })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_are_tab_separated_and_whole_pixels() {
        let mut t = Targets::new(true);
        t.slot("DP-1", &graphene::Rect::new(10.4, 20.6, 48.0, 48.0));
        t.app(
            "DP-1",
            "io.elavo.Files",
            &graphene::Rect::new(0.0, 0.0, 48.0, 48.0),
        );
        t.window(
            "DP-1",
            "io.elavo.Files",
            "Home\tDocs",
            &graphene::Rect::new(1.0, 2.0, 3.0, 4.0),
        );
        assert_eq!(
            t.text(),
            "animate\t1\nslot\tDP-1\t10\t21\t48\t48\napp\tDP-1\tio.elavo.Files\t0\t0\t48\t48\n\
             window\tDP-1\tio.elavo.Files\tHome Docs\t1\t2\t3\t4\n"
        );
    }
}
