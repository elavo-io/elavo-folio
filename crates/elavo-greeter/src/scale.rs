//! The login screen's display scale. cage starts every output at 100%, which
//! leaves the greeter tiny on a 4K panel, and the scale a person picks in
//! Displays belongs to their session, not to the screen before it. So at
//! login the greeter gives each output a default for its resolution, through
//! wlr-output-management (`wlr-randr`), before its window appears.

use std::process::Command;

use gtk::gdk;
use gtk::prelude::*;

/// The smallest mode, long side by short side in pixels, each scale applies
/// from. Ordered from smallest to largest; the last rule a mode reaches wins.
const RULES: &[(i32, i32, f64)] = &[(3840, 2160, 1.75)];

/// The default scale for a mode, or `None` to keep 100%.
pub fn for_mode(width: i32, height: i32) -> Option<f64> {
    let (long, short) = (width.max(height), width.min(height));
    RULES
        .iter()
        .rev()
        .find(|(l, s, _)| long >= *l && short >= *s)
        .map(|(_, _, scale)| *scale)
}

/// Scale each output still at 100% to its default. An output something else
/// already scaled is left alone.
pub fn apply_defaults() {
    let Some(display) = gdk::Display::default() else {
        return;
    };
    let monitors = display.monitors();
    for i in 0..monitors.n_items() {
        let Some(monitor) = monitors.item(i).and_downcast::<gdk::Monitor>() else {
            continue;
        };
        if monitor.scale() != 1.0 {
            continue;
        }
        // At 100% the logical size is the mode.
        let geometry = monitor.geometry();
        let (Some(scale), Some(connector)) = (
            for_mode(geometry.width(), geometry.height()),
            monitor.connector(),
        ) else {
            continue;
        };
        let result = Command::new("wlr-randr")
            .args([
                "--output",
                connector.as_str(),
                "--scale",
                &scale.to_string(),
            ])
            .status();
        match result {
            Ok(status) if status.success() => crate::trace::mark("output scaled"),
            Ok(status) => glib_warning(&format!("wlr-randr {connector} --scale {scale}: {status}")),
            Err(e) => glib_warning(&format!("wlr-randr: {e}")),
        }
    }
}

fn glib_warning(message: &str) {
    gtk::glib::g_warning!("elavo-greeter", "{message}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_k_and_above_get_175() {
        assert_eq!(for_mode(3840, 2160), Some(1.75));
        assert_eq!(for_mode(5120, 2880), Some(1.75));
        assert_eq!(for_mode(2160, 3840), Some(1.75));
    }

    #[test]
    fn smaller_modes_stay_at_100() {
        assert_eq!(for_mode(1920, 1080), None);
        assert_eq!(for_mode(2560, 1440), None);
        assert_eq!(for_mode(3440, 1440), None);
        assert_eq!(for_mode(3840, 1600), None);
    }
}
