//! The Control Center popup: Wi-Fi, Bluetooth, brightness, and volume.
//! Lock, Sleep, and Settings stay in the Elavo menu.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};

use crate::dropdown;

/// A button whose popover is the control center. One per bar.
pub fn button() -> gtk::MenuButton {
    let popover = gtk::Popover::builder()
        .child(&view())
        .position(gtk::PositionType::Bottom)
        .has_arrow(false)
        .build();
    dropdown::prepare(&popover, true);
    // The button sits near the right edge. Grow the popup toward the left.
    popover.set_halign(gtk::Align::End);

    let button = gtk::MenuButton::builder()
        .icon_name("preferences-system-symbolic")
        .css_classes(["flat", "elavo-panel-control"])
        .popover(&popover)
        .always_show_arrow(false)
        .build();
    if let Some(image) = button
        .first_child()
        .and_then(|toggle| toggle.first_child())
        .and_downcast::<gtk::Image>()
    {
        image.set_pixel_size(16);
    }
    button
}

fn view() -> gtk::Box {
    let column = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .width_request(340)
        .css_classes(["elavo-control"])
        .build();

    let tiles = gtk::Box::builder().spacing(8).homogeneous(true).build();
    tiles.append(&wifi_tile());
    tiles.append(&bluetooth_tile());
    column.append(&tiles);

    if let Some(row) = brightness_row("Brightness", "/sys/class/backlight", |name| {
        !name.starts_with('.')
    }) {
        column.append(&row);
    }
    if let Some(row) = brightness_row("Keyboard Brightness", "/sys/class/leds", |name| {
        name.contains("kbd_backlight")
    }) {
        column.append(&row);
    }
    column.append(&volume_row());
    column
}

fn tile(icon: &str, title: &str) -> (gtk::Box, gtk::Label, gtk::Switch) {
    let box_ = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .css_classes(["elavo-control-tile"])
        .build();
    let top = gtk::Box::builder().spacing(8).build();
    top.append(&gtk::Image::builder().icon_name(icon).pixel_size(18).build());
    top.append(
        &gtk::Label::builder()
            .label(title)
            .xalign(0.0)
            .hexpand(true)
            .build(),
    );
    let switch = gtk::Switch::new();
    switch.set_valign(gtk::Align::Center);
    top.append(&switch);
    box_.append(&top);
    let subtitle = gtk::Label::builder()
        .xalign(0.0)
        .css_classes(["caption"])
        .build();
    box_.append(&subtitle);
    (box_, subtitle, switch)
}

fn wifi_tile() -> gtk::Box {
    let (tile, subtitle, switch) = tile("network-wireless-symbolic", "Wi-Fi");
    tile.set_visible(false);
    let syncing = Rc::new(Cell::new(false));
    let subtitle = Rc::new(subtitle);
    let switch = Rc::new(switch);
    let tile = Rc::new(tile);

    let syncing_set = syncing.clone();
    let switch_set = switch.clone();
    switch.connect_state_set(move |_, on| {
        if syncing_set.get() {
            return glib::Propagation::Proceed;
        }
        let switch_set = switch_set.clone();
        glib::spawn_future_local(async move {
            let Some(proxy) = nm_proxy().await else {
                return;
            };
            set_bool(
                &proxy,
                "org.freedesktop.NetworkManager",
                "WirelessEnabled",
                on,
            )
            .await;
            drop(switch_set);
        });
        glib::Propagation::Proceed
    });

    let tile_w = tile.clone();
    let subtitle_w = subtitle.clone();
    let switch_w = switch.clone();
    glib::spawn_future_local(async move {
        let Some(proxy) = nm_proxy().await else {
            return;
        };
        let apply = {
            let tile_w = tile_w.clone();
            let subtitle_w = subtitle_w.clone();
            let switch_w = switch_w.clone();
            let syncing = syncing.clone();
            move |proxy: &gio::DBusProxy| {
                let hardware: bool = prop(proxy, "WirelessHardwareEnabled").unwrap_or(false);
                tile_w.set_visible(hardware);
                if !hardware {
                    return;
                }
                let enabled: bool = prop(proxy, "WirelessEnabled").unwrap_or(false);
                syncing.set(true);
                switch_w.set_active(enabled);
                syncing.set(false);
                let path: String = prop(proxy, "PrimaryConnection").unwrap_or_default();
                let subtitle_w = subtitle_w.clone();
                glib::spawn_future_local(async move {
                    let text = if !enabled {
                        "Off".to_owned()
                    } else {
                        connection_name(&path)
                            .await
                            .unwrap_or_else(|| "Not Connected".into())
                    };
                    subtitle_w.set_label(&text);
                });
            }
        };
        apply(&proxy);
        on_properties_changed(&proxy, apply);
        std::mem::forget(proxy);
    });
    Rc::try_unwrap(tile).unwrap_or_else(|tile| (*tile).clone())
}

fn bluetooth_tile() -> gtk::Box {
    let (tile, subtitle, switch) = tile("bluetooth-symbolic", "Bluetooth");
    tile.set_visible(false);
    let syncing = Rc::new(Cell::new(false));
    let tile = Rc::new(tile);
    let subtitle = Rc::new(subtitle);
    let switch = Rc::new(switch);

    let syncing_set = syncing.clone();
    let switch_set = switch.clone();
    switch.connect_state_set(move |_, on| {
        if syncing_set.get() {
            return glib::Propagation::Proceed;
        }
        let switch_set = switch_set.clone();
        glib::spawn_future_local(async move {
            let Some(proxy) = bluetooth_proxy().await else {
                return;
            };
            set_bool(&proxy, "org.bluez.Adapter1", "Powered", on).await;
            drop(switch_set);
        });
        glib::Propagation::Proceed
    });

    let tile_w = tile.clone();
    let subtitle_w = subtitle.clone();
    let switch_w = switch.clone();
    glib::spawn_future_local(async move {
        let Some(proxy) = bluetooth_proxy().await else {
            return;
        };
        let apply = {
            let tile_w = tile_w.clone();
            let subtitle_w = subtitle_w.clone();
            let switch_w = switch_w.clone();
            let syncing = syncing.clone();
            move |proxy: &gio::DBusProxy| {
                tile_w.set_visible(true);
                let on: bool = prop(proxy, "Powered").unwrap_or(false);
                syncing.set(true);
                switch_w.set_active(on);
                syncing.set(false);
                subtitle_w.set_label(if on { "On" } else { "Off" });
            }
        };
        apply(&proxy);
        on_properties_changed(&proxy, apply);
        std::mem::forget(proxy);
    });
    Rc::try_unwrap(tile).unwrap_or_else(|tile| (*tile).clone())
}

/// A slider for one backlight directory. `None` when no device matches.
fn brightness_row(title: &str, class_dir: &str, keep: fn(&str) -> bool) -> Option<gtk::Box> {
    let device = first_device(class_dir, keep)?;
    let max = read_u32(&device.join("max_brightness"))?.max(1);
    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .build();
    row.append(
        &gtk::Label::builder()
            .label(title)
            .xalign(0.0)
            .css_classes(["caption"])
            .build(),
    );
    let scale = gtk::Scale::with_range(
        gtk::Orientation::Horizontal,
        0.0,
        max as f64,
        if max <= 16 {
            1.0
        } else {
            (max as f64 / 100.0).max(1.0)
        },
    );
    scale.set_draw_value(max <= 16);
    scale.set_digits(0);
    scale.set_hexpand(true);
    if let Some(now) = read_u32(&device.join("brightness")) {
        scale.set_value(now.min(max) as f64);
    }
    let device_name = device
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let device_path = device.clone();
    scale.connect_value_changed(move |scale| {
        let value = snap(scale.value(), max);
        if !write_brightness(&device_path, value) {
            let name = device_name.clone();
            glib::spawn_future_local(async move {
                let _ = run(&["brightnessctl", "-d", &name, "set", &value.to_string()]).await;
            });
        }
    });
    row.append(&scale);
    Some(row)
}

fn volume_row() -> gtk::Box {
    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .build();
    row.append(
        &gtk::Label::builder()
            .label("Volume")
            .xalign(0.0)
            .css_classes(["caption"])
            .build(),
    );
    let line = gtk::Box::builder().spacing(8).build();
    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 0.01);
    scale.set_draw_value(false);
    scale.set_hexpand(true);
    let mute = gtk::Switch::builder().valign(gtk::Align::Center).build();
    line.append(&scale);
    line.append(&mute);
    row.append(&line);

    let writing = Rc::new(Cell::new(false));
    let writing_s = writing.clone();
    let mute_s = mute.clone();
    scale.connect_value_changed(move |scale| {
        if writing_s.get() {
            return;
        }
        let frac = scale.value().clamp(0.0, 1.0);
        let text = format!("{frac:.2}");
        let muted = mute_s.is_active();
        glib::spawn_future_local(async move {
            let _ = run(&["wpctl", "set-volume", "@DEFAULT_AUDIO_SINK@", &text]).await;
            if muted && frac > 0.0 {
                let _ = run(&["wpctl", "set-mute", "@DEFAULT_AUDIO_SINK@", "0"]).await;
            }
        });
    });
    let writing_m = writing.clone();
    let scale_m = scale.clone();
    mute.connect_state_set(move |_, on| {
        if writing_m.get() {
            return glib::Propagation::Proceed;
        }
        let arg = if on { "1" } else { "0" };
        let restore = scale_m.value();
        glib::spawn_future_local(async move {
            let _ = run(&["wpctl", "set-mute", "@DEFAULT_AUDIO_SINK@", arg]).await;
            if !on && restore > 0.0 {
                let text = format!("{restore:.2}");
                let _ = run(&["wpctl", "set-volume", "@DEFAULT_AUDIO_SINK@", &text]).await;
            }
        });
        glib::Propagation::Proceed
    });

    let writing_r = writing.clone();
    let scale_r = scale.clone();
    let mute_r = mute.clone();
    glib::spawn_future_local(async move {
        if let Some(text) = run(&["wpctl", "get-volume", "@DEFAULT_AUDIO_SINK@"]).await {
            if let Some((level, muted)) = parse_volume(&text) {
                writing_r.set(true);
                scale_r.set_value(level.clamp(0.0, 1.0));
                mute_r.set_active(muted);
                writing_r.set(false);
            }
        }
    });
    row
}

pub fn snap(value: f64, max: u32) -> u32 {
    let max = max.max(1);
    value.round().clamp(0.0, max as f64) as u32
}

fn first_device(class_dir: &str, keep: fn(&str) -> bool) -> Option<PathBuf> {
    let mut found = std::fs::read_dir(class_dir).ok()?.filter_map(|entry| {
        let entry = entry.ok()?;
        let name = entry.file_name();
        let name = name.to_str()?;
        keep(name).then(|| entry.path())
    });
    found.next()
}

fn read_u32(path: &Path) -> Option<u32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

fn write_brightness(device: &Path, value: u32) -> bool {
    std::fs::write(device.join("brightness"), value.to_string()).is_ok()
}

async fn nm_proxy() -> Option<gio::DBusProxy> {
    system_proxy(
        "org.freedesktop.NetworkManager",
        "/org/freedesktop/NetworkManager",
        "org.freedesktop.NetworkManager",
    )
    .await
}

async fn bluetooth_proxy() -> Option<gio::DBusProxy> {
    system_proxy("org.bluez", "/org/bluez/hci0", "org.bluez.Adapter1").await
}

async fn connection_name(path: &str) -> Option<String> {
    if path.is_empty() || path == "/" {
        return None;
    }
    let proxy = system_proxy(
        "org.freedesktop.NetworkManager",
        path,
        "org.freedesktop.NetworkManager.Connection.Active",
    )
    .await?;
    let id: String = prop(&proxy, "Id")?;
    if id.is_empty() {
        None
    } else {
        Some(id)
    }
}

async fn system_proxy(name: &str, path: &str, interface: &str) -> Option<gio::DBusProxy> {
    gio::DBusProxy::for_bus_future(
        gio::BusType::System,
        gio::DBusProxyFlags::NONE,
        None,
        name,
        path,
        interface,
    )
    .await
    .ok()
}

fn prop<T: glib::variant::FromVariant>(proxy: &gio::DBusProxy, name: &str) -> Option<T> {
    proxy.cached_property(name).and_then(|v| v.get::<T>())
}

fn on_properties_changed(proxy: &gio::DBusProxy, update: impl Fn(&gio::DBusProxy) + 'static) {
    proxy.connect_local("g-properties-changed", false, move |args| {
        if let Some(proxy) = args.first().and_then(|v| v.get::<gio::DBusProxy>().ok()) {
            update(&proxy);
        }
        None
    });
}

/// `org.freedesktop.DBus.Properties.Set` takes `(ssv)`. A bare bool would be `(ssb)`.
fn property_set(iface: &str, name: &str, value: bool) -> glib::Variant {
    (
        iface,
        name,
        glib::Variant::from_variant(&value.to_variant()),
    )
        .to_variant()
}

async fn set_bool(proxy: &gio::DBusProxy, iface: &str, name: &str, value: bool) {
    let params = property_set(iface, name, value);
    let Some(destination) = proxy.name() else {
        return;
    };
    let path = proxy.object_path();
    if let Err(err) = proxy
        .connection()
        .call_future(
            Some(destination.as_str()),
            path.as_str(),
            "org.freedesktop.DBus.Properties",
            "Set",
            Some(&params),
            None,
            gio::DBusCallFlags::NONE,
            -1,
        )
        .await
    {
        glib::g_warning!("elavo-panel", "{iface}.{name}: {err}");
    }
}

async fn run(argv: &[&str]) -> Option<String> {
    let args: Vec<&std::ffi::OsStr> = argv.iter().map(std::ffi::OsStr::new).collect();
    let proc = gio::Subprocess::newv(
        &args,
        gio::SubprocessFlags::STDOUT_PIPE | gio::SubprocessFlags::STDERR_SILENCE,
    )
    .ok()?;
    proc.communicate_utf8_future(None)
        .await
        .ok()
        .and_then(|(out, _)| out)
        .map(|text| text.to_string())
}

/// `wpctl get-volume` prints `Volume: 0.40` or `Volume: 0.40 [MUTED]`.
pub fn parse_volume(text: &str) -> Option<(f64, bool)> {
    let level = text.split_whitespace().nth(1)?.parse().ok()?;
    Some((level, text.contains("[MUTED]") || level < 0.01))
}

#[cfg(test)]
mod tests {
    use super::{parse_volume, property_set, snap};

    #[test]
    fn keyboard_steps_snap_to_whole_levels() {
        assert_eq!(snap(0.4, 2), 0);
        assert_eq!(snap(0.6, 2), 1);
        assert_eq!(snap(9.0, 2), 2);
    }

    #[test]
    fn a_property_write_is_a_variant() {
        assert_eq!(
            property_set("org.freedesktop.NetworkManager", "WirelessEnabled", true)
                .type_()
                .as_str(),
            "(ssv)"
        );
    }

    #[test]
    fn wpctl_volume_line() {
        assert_eq!(parse_volume("Volume: 0.40\n"), Some((0.40, false)));
        assert_eq!(parse_volume("Volume: 0.00 [MUTED]"), Some((0.0, true)));
        assert_eq!(parse_volume("nope"), None);
    }
}
