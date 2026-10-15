//! About Elavo. A normal window, not a GTK About dialog. The title bar is a
//! bare GTK header bar, so the window buttons follow the GTK theme. The
//! plate sizes to its content: the sun, wordmark, and version are centered,
//! and the spec rows are one centered group.

use std::cell::RefCell;

use elavo_common::ABOUT_TITLE;
use gtk::prelude::*;
use gtk::glib;

// The panel process owns this window, so Wayland reports `io.elavo.Panel`.
// `ABOUT_TITLE` is a word joiner: nothing is drawn in the title bar, and
// the dock uses it to tell this window from a dialog.

thread_local! {
    static OPEN: RefCell<Option<glib::WeakRef<gtk::Window>>> = const { RefCell::new(None) };
}

struct Row {
    label: &'static str,
    value: String,
    href: Option<String>,
}

pub fn show() {
    if let Some(window) = existing() {
        window.present();
        return;
    }
    let rows = rows();
    let window = gtk::Window::builder()
        .title(ABOUT_TITLE)
        .resizable(false)
        .css_classes(["elavo-about"])
        .build();
    let header = gtk::HeaderBar::builder()
        .title_widget(&gtk::Label::new(None))
        .build();
    window.set_titlebar(Some(&header));
    let plate = plate(&rows);
    let (_, width, _, _) = plate.measure(gtk::Orientation::Horizontal, -1);
    let (_, height, _, _) = plate.measure(gtk::Orientation::Vertical, width);
    window.set_default_size(width, height);
    window.set_child(Some(&plate));
    if let Some(app) = gtk::gio::Application::default().and_then(|app| app.downcast::<gtk::Application>().ok()) {
        app.add_window(&window);
    }
    OPEN.with(|open| open.replace(Some(window.downgrade())));
    window.connect_close_request(|_| {
        OPEN.with(|open| open.replace(None));
        glib::Propagation::Proceed
    });
    window.present();
}

fn existing() -> Option<gtk::Window> {
    OPEN.with(|open| open.borrow().as_ref().and_then(|w| w.upgrade()))
}

fn plate(rows: &[Row]) -> gtk::Box {
    let root = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .margin_start(24)
        .margin_end(24)
        .margin_bottom(8)
        .build();
    let sun = gtk::Image::builder()
        .resource("/io/elavo/Panel/about-sun.svg")
        .pixel_size(102)
        .halign(gtk::Align::Center)
        .margin_top(16)
        .build();
    let name = gtk::Label::builder()
        .label("elavo")
        .css_classes(["elavo-about-name"])
        .halign(gtk::Align::Center)
        .margin_top(12)
        .build();
    let version = gtk::Label::builder()
        .label(format!("version {}", release_value("VERSION_ID")))
        .css_classes(["elavo-about-version"])
        .halign(gtk::Align::Center)
        .margin_top(2)
        .build();
    root.append(&sun);
    root.append(&name);
    root.append(&version);

    // One group: labels share a column and sit on its right edge, values
    // sit on the left of theirs, and the pair is centered in the window.
    let grid = gtk::Grid::builder()
        .halign(gtk::Align::Center)
        .margin_top(20)
        .margin_bottom(24)
        .row_spacing(0)
        .column_spacing(8)
        .build();
    for (i, row) in rows.iter().enumerate() {
        let label = gtk::Label::builder()
            .label(row.label)
            .css_classes(["elavo-about-row"])
            .halign(gtk::Align::End)
            .xalign(1.0)
            .build();
        let value = gtk::Label::builder()
            .css_classes(["elavo-about-row", "elavo-about-muted"])
            .halign(gtk::Align::Start)
            .xalign(0.0)
            .use_markup(row.href.is_some())
            .build();
        if let Some(href) = &row.href {
            value.set_markup(&format!(
                "<a href=\"{}\">{}</a>",
                escape(href),
                escape(&row.value)
            ));
        } else {
            value.set_label(&row.value);
        }
        grid.attach(&label, 0, i as i32, 1, 1);
        grid.attach(&value, 1, i as i32, 1, 1);
    }
    root.append(&grid);
    root
}

fn rows() -> Vec<Row> {
    let mut rows = vec![Row {
        label: "CPU",
        value: cpu(),
        href: None,
    }];
    for name in gpus() {
        rows.push(Row {
            label: "GPU",
            value: name,
            href: None,
        });
    }
    rows.push(Row {
        label: "Memory",
        value: memory(),
        href: None,
    });
    let image = image_id();
    rows.push(Row {
        label: "image ID",
        value: image.0,
        href: image.1,
    });
    rows.push(Row {
        label: "Kernel",
        value: kernel(),
        href: None,
    });
    rows
}

fn cpu() -> String {
    let text = std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
    text.lines()
        .find_map(|line| {
            let rest = line.strip_prefix("model name")?;
            let name = rest.trim_start_matches([' ', ':', '\t']);
            if name.is_empty() {
                None
            } else {
                Some(name.to_owned())
            }
        })
        .unwrap_or_else(|| "unknown".into())
}

fn memory() -> String {
    let text = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let kb = text.lines().find_map(|line| {
        let rest = line.strip_prefix("MemTotal:")?;
        rest.split_whitespace().next()?.parse::<u64>().ok()
    });
    match kb {
        Some(kb) => format!("{} GB", (kb as f64 / 1_048_576.0).round() as u64),
        None => "unknown".into(),
    }
}

fn kernel() -> String {
    std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .map(|s| s.trim().to_owned())
        .unwrap_or_else(|_| "unknown".into())
}

/// `lspci -mm` quoted fields. VGA, 3D, and Display controllers, one row each.
fn gpus() -> Vec<String> {
    let Ok(output) = std::process::Command::new("lspci").arg("-mm").output() else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut names = Vec::new();
    for line in text.lines() {
        // The slot is unquoted, so the fields are class, vendor, device.
        let fields = quoted_fields(line);
        let class = fields.first().map(String::as_str).unwrap_or("");
        if !(class.contains("VGA") || class.contains("3D") || class.contains("Display")) {
            continue;
        }
        if let Some(name) = fields.get(2).filter(|n| !n.is_empty()) {
            names.push(marketing_name(name).to_owned());
        }
    }
    names
}

/// `WhiskeyLake-U GT2 [UHD Graphics 620]` to `UHD Graphics 620`. A name
/// without brackets is kept whole.
fn marketing_name(name: &str) -> &str {
    name.rsplit_once('[')
        .and_then(|(_, rest)| rest.strip_suffix(']'))
        .filter(|inner| !inner.trim().is_empty())
        .map(str::trim)
        .unwrap_or(name)
}

fn quoted_fields(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut rest = line;
    while let Some(start) = rest.find('"') {
        rest = &rest[start + 1..];
        let Some(end) = rest.find('"') else {
            break;
        };
        fields.push(rest[..end].to_owned());
        rest = &rest[end + 1..];
    }
    fields
}

/// The last six hex digits of the booted image digest, linked to that
/// image's package when the reference is a `ghcr.io` name. `bootc status`
/// needs root, so this reads the same digest from `rpm-ostree status`.
/// A machine that is not a bootc host shows "unknown".
fn image_id() -> (String, Option<String>) {
    let Ok(output) = std::process::Command::new("rpm-ostree")
        .args(["status", "--json"])
        .output()
    else {
        return ("unknown".into(), None);
    };
    if !output.status.success() {
        return ("unknown".into(), None);
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let Some(deployment) = booted_deployment(&text) else {
        return ("unknown".into(), None);
    };
    let Some(hex) = json_string(deployment, "container-image-reference-digest")
        .and_then(|digest| {
            digest
                .strip_prefix("sha256:")
                .filter(|hex| hex.chars().all(|c| c.is_ascii_hexdigit()) && hex.len() >= 6)
                .map(|hex| hex.to_owned())
        })
    else {
        return ("unknown".into(), None);
    };
    let label = hex[hex.len() - 6..].to_owned();
    let href = json_string(deployment, "container-image-reference")
        .as_deref()
        .map(image_name)
        .and_then(package_url);
    (label, href)
}

/// The pretty-printed deployment object whose `"booted"` field is true.
fn booted_deployment(text: &str) -> Option<&str> {
    text.split("\n    {").find(|chunk| {
        json_bool_true(chunk, "booted")
    })
}

fn find_key<'a>(object: &'a str, key: &str) -> Option<&'a str> {
    let marker = format!("\"{key}\"");
    let mut rest = object;
    while let Some(at) = rest.find(&marker) {
        let after = rest[at + marker.len()..].trim_start();
        if after.starts_with(':') {
            return Some(after[1..].trim_start());
        }
        rest = &rest[at + marker.len()..];
    }
    None
}

fn json_bool_true(object: &str, key: &str) -> bool {
    find_key(object, key).is_some_and(|value| value.starts_with("true"))
}

fn json_string(object: &str, key: &str) -> Option<String> {
    let rest = find_key(object, key)?.strip_prefix('"')?;
    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            out.push(chars.next()?);
            continue;
        }
        if c == '"' {
            return Some(out);
        }
        out.push(c);
    }
    None
}

/// `ostree-unverified-registry:ghcr.io/org/name:tag` to `ghcr.io/org/name:tag`.
fn image_name(reference: &str) -> &str {
    reference
        .strip_prefix("ostree-")
        .and_then(|rest| rest.split_once(':'))
        .map(|(_, name)| name)
        .unwrap_or(reference)
}

fn package_url(image: &str) -> Option<String> {
    let rest = image.strip_prefix("ghcr.io/")?;
    let path = rest.rsplit_once(':').map(|(path, _)| path).unwrap_or(rest);
    let mut parts = path.split('/');
    let owner = parts.next()?;
    let name = parts.next()?;
    if owner.is_empty() || name.is_empty() {
        return None;
    }
    Some(format!(
        "https://github.com/{owner}/{name}/pkgs/container/{name}"
    ))
}

fn release_value(key: &str) -> String {
    let text = std::fs::read_to_string("/etc/os-release")
        .or_else(|_| std::fs::read_to_string("/usr/lib/os-release"))
        .unwrap_or_default();
    text.lines()
        .find_map(|line| {
            let (k, v) = line.split_once('=')?;
            (k == key).then(|| v.trim().trim_matches('"').to_owned())
        })
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "unknown".into())
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
