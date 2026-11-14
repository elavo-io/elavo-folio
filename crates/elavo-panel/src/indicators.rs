//! Menus for the bar's Wi-Fi, volume, and battery icons. Each menu is
//! rebuilt every time it opens, so it shows the state at that moment.
//!
//! Wi-Fi talks to NetworkManager and the battery to UPower and
//! power-profiles-daemon over D-Bus. Sound goes through `wpctl`, like the
//! volume icon and the Control Center slider.

use std::collections::HashMap;

use gtk::prelude::*;
use gtk::{gio, glib};

use crate::{dropdown, menu, panelcfg};

type Props = HashMap<String, glib::Variant>;

const NM: &str = "org.freedesktop.NetworkManager";
const NM_PATH: &str = "/org/freedesktop/NetworkManager";
const NM_DEVICE: &str = "org.freedesktop.NetworkManager.Device";
const NM_WIRELESS: &str = "org.freedesktop.NetworkManager.Device.Wireless";
const NM_AP: &str = "org.freedesktop.NetworkManager.AccessPoint";
const NM_SETTINGS_PATH: &str = "/org/freedesktop/NetworkManager/Settings";
const NM_SETTINGS: &str = "org.freedesktop.NetworkManager.Settings";
const NM_CONNECTION: &str = "org.freedesktop.NetworkManager.Settings.Connection";
const NM_TYPE_WIFI: u32 = 2;
const UPOWER: &str = "org.freedesktop.UPower";
const UPOWER_DEVICE: &str = "org.freedesktop.UPower.Device";
const DISPLAY_DEVICE: &str = "/org/freedesktop/UPower/devices/DisplayDevice";
const PROFILES: [(&str, &str); 2] = [
    (
        "org.freedesktop.UPower.PowerProfiles",
        "/org/freedesktop/UPower/PowerProfiles",
    ),
    ("net.hadess.PowerProfiles", "/net/hadess/PowerProfiles"),
];
/// The most networks the menu lists. System Settings has the full list.
const MAX_NETWORKS: usize = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Wifi,
    Volume,
    Battery,
}

/// A bar button around an indicator. It is shown while `image` is.
pub fn button(child: &impl IsA<gtk::Widget>, image: &gtk::Image, kind: Kind) -> gtk::MenuButton {
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .width_request(280)
        .css_classes(["elavo-indicator-menu"])
        .build();
    let popover = gtk::Popover::builder()
        .child(&content)
        .position(gtk::PositionType::Bottom)
        .has_arrow(false)
        .build();
    // Added, not set: setting the classes would drop `background`.
    popover.add_css_class("menu");
    dropdown::prepare(&popover, true);
    // The icons sit near the right edge. Grow the menu toward the left.
    popover.set_halign(gtk::Align::End);

    let button = gtk::MenuButton::builder()
        .child(child)
        .css_classes(["flat", "elavo-panel-indicator"])
        .popover(&popover)
        .always_show_arrow(false)
        .build();
    // Fill the menu before it opens. A popover sized while still empty
    // has no height and never appears.
    button.set_create_popup_func(move |button| {
        if let Some(popover) = button.popover() {
            build(kind, &content, &popover);
        }
    });
    image
        .bind_property("visible", &button, "visible")
        .sync_create()
        .build();
    button
}

fn build(kind: Kind, content: &gtk::Box, popover: &gtk::Popover) {
    while let Some(child) = content.first_child() {
        content.remove(&child);
    }
    match kind {
        Kind::Wifi => wifi(content, popover),
        Kind::Volume => volume(content, popover),
        Kind::Battery => battery(content, popover),
    }
}

/// Build the menu again after `ms`, if it is still open.
fn rebuild_after(kind: Kind, content: &gtk::Box, popover: &gtk::Popover, ms: u64) {
    let content = content.clone();
    let popover = popover.clone();
    glib::timeout_add_local_once(std::time::Duration::from_millis(ms), move || {
        if popover.is_visible() {
            build(kind, &content, &popover);
        }
    });
}

// Rows.

fn header(title: &str, trailing: Option<&gtk::Widget>) -> gtk::Box {
    let row = gtk::Box::builder()
        .spacing(8)
        .css_classes(["elavo-indicator-head"])
        .build();
    row.append(
        &gtk::Label::builder()
            .label(title)
            .xalign(0.0)
            .hexpand(true)
            .build(),
    );
    if let Some(widget) = trailing {
        widget.set_valign(gtk::Align::Center);
        row.append(widget);
    }
    row
}

fn caption(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .wrap(true)
        .css_classes(["elavo-indicator-caption"])
        .build()
}

fn separator() -> gtk::Separator {
    gtk::Separator::new(gtk::Orientation::Horizontal)
}

/// A menu item: an optional leading icon, the text, trailing icons, and a
/// checkmark when it is the current choice.
fn item(icon: Option<&str>, text: &str, trailing: &[&str], checked: bool) -> gtk::Button {
    let row = gtk::Box::builder().spacing(8).build();
    let check = gtk::Image::builder()
        .icon_name("object-select-symbolic")
        .pixel_size(16)
        .opacity(if checked { 1.0 } else { 0.0 })
        .build();
    row.append(&check);
    if let Some(name) = icon {
        row.append(&gtk::Image::builder().icon_name(name).pixel_size(16).build());
    }
    row.append(
        &gtk::Label::builder()
            .label(text)
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .max_width_chars(28)
            .build(),
    );
    for name in trailing {
        row.append(
            &gtk::Image::builder()
                .icon_name(*name)
                .pixel_size(16)
                .build(),
        );
    }
    gtk::Button::builder()
        .child(&row)
        .css_classes(["flat", "elavo-indicator-item"])
        .build()
}

fn settings_item(popover: &gtk::Popover, text: &str, pane: &'static str) -> gtk::Button {
    let button = gtk::Button::builder()
        .child(&gtk::Label::builder().label(text).xalign(0.0).build())
        .css_classes(["flat", "elavo-indicator-item", "elavo-indicator-plain"])
        .build();
    let popover = popover.clone();
    button.connect_clicked(move |_| {
        popover.popdown();
        menu::open_settings(pane);
    });
    button
}

// Wi-Fi.

#[derive(Clone, Debug)]
struct Network {
    path: String,
    ssid: String,
    strength: u8,
    secured: bool,
    enterprise: bool,
}

fn wifi(content: &gtk::Box, popover: &gtk::Popover) {
    let switch = gtk::Switch::new();
    content.append(&header("Wi-Fi", Some(switch.upcast_ref())));
    let list = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .build();
    content.append(&list);
    content.append(&separator());
    content.append(&settings_item(popover, "Wi-Fi Settings…", "wifi"));

    let content = content.clone();
    let popover = popover.clone();
    glib::spawn_future_local(async move {
        let Some(conn) = system().await else {
            list.append(&caption("NetworkManager is not running."));
            return;
        };
        let root = get_all(&conn, NM, NM_PATH, NM).await;
        let hardware: bool = prop(&root, "WirelessHardwareEnabled").unwrap_or(false);
        let enabled: bool = prop(&root, "WirelessEnabled").unwrap_or(false);
        switch.set_active(enabled);
        switch.set_sensitive(hardware);
        {
            let conn = conn.clone();
            let content = content.clone();
            let popover = popover.clone();
            switch.connect_state_set(move |_, on| {
                let conn = conn.clone();
                let content = content.clone();
                let popover = popover.clone();
                glib::spawn_future_local(async move {
                    set_property(&conn, NM, NM_PATH, NM, "WirelessEnabled", on.to_variant()).await;
                    // A radio that just came on needs a moment to see networks.
                    rebuild_after(Kind::Wifi, &content, &popover, if on { 2500 } else { 300 });
                });
                glib::Propagation::Proceed
            });
        }
        if !hardware {
            list.append(&caption("Wi-Fi is turned off by a hardware switch."));
            return;
        }
        if !enabled {
            return;
        }
        let Some(device) = wifi_device(&conn).await else {
            list.append(&caption("No Wi-Fi hardware found."));
            return;
        };
        let options: HashMap<String, glib::Variant> = HashMap::new();
        let scan = (options,).to_variant();
        let _ = call(&conn, NM, &device, NM_WIRELESS, "RequestScan", Some(scan)).await;

        let active = get(&conn, NM, &device, NM_WIRELESS, "ActiveAccessPoint")
            .await
            .and_then(|v| v.str().map(str::to_owned))
            .unwrap_or_default();
        let networks = access_points(&conn, &device, &active).await;
        let saved = saved_networks(&conn).await;
        if networks.is_empty() {
            list.append(&caption("No networks found."));
            return;
        }
        let (current, others): (Vec<_>, Vec<_>) =
            networks.into_iter().partition(|n| n.path == active);
        for network in &current {
            list.append(&network_item(&conn, &device, network, true, None, &popover));
        }
        if !others.is_empty() {
            list.append(&caption(if current.is_empty() {
                "Networks"
            } else {
                "Other Networks"
            }));
        }
        for network in others.iter().take(MAX_NETWORKS) {
            let saved = saved.get(&network.ssid).cloned();
            list.append(&network_item(
                &conn, &device, network, false, saved, &popover,
            ));
        }
    });
}

fn network_item(
    conn: &gio::DBusConnection,
    device: &str,
    network: &Network,
    joined: bool,
    saved: Option<String>,
    popover: &gtk::Popover,
) -> gtk::Button {
    let lock: &[&str] = if network.secured {
        &["system-lock-screen-symbolic"]
    } else {
        &[]
    };
    let button = item(
        Some(signal_icon(network.strength)),
        &network.ssid,
        lock,
        joined,
    );
    let conn = conn.clone();
    let device = device.to_owned();
    let network = network.clone();
    let popover = popover.clone();
    button.connect_clicked(move |_| {
        popover.popdown();
        if joined {
            return;
        }
        // A secured network that has never been joined needs a password,
        // and System Settings is where that is asked for.
        if saved.is_none() && (network.secured || network.enterprise) {
            menu::open_settings("wifi");
            return;
        }
        let conn = conn.clone();
        let device = device.clone();
        let network = network.clone();
        let saved = saved.clone();
        glib::spawn_future_local(async move {
            join(&conn, &device, &network, saved.as_deref()).await;
        });
    });
    button
}

fn signal_icon(strength: u8) -> &'static str {
    match strength {
        80.. => "network-wireless-signal-excellent-symbolic",
        55..=79 => "network-wireless-signal-good-symbolic",
        30..=54 => "network-wireless-signal-ok-symbolic",
        6..=29 => "network-wireless-signal-weak-symbolic",
        _ => "network-wireless-signal-none-symbolic",
    }
}

async fn wifi_device(conn: &gio::DBusConnection) -> Option<String> {
    let reply = call(conn, NM, NM_PATH, NM, "GetDevices", None).await?;
    for path in strings(&reply.child_value(0)) {
        let kind = get(conn, NM, &path, NM_DEVICE, "DeviceType")
            .await
            .and_then(|v| v.get::<u32>());
        if kind == Some(NM_TYPE_WIFI) {
            return Some(path);
        }
    }
    None
}

/// One entry per name: the joined access point, otherwise the strongest.
async fn access_points(conn: &gio::DBusConnection, device: &str, active: &str) -> Vec<Network> {
    let Some(reply) = call(conn, NM, device, NM_WIRELESS, "GetAllAccessPoints", None).await else {
        return Vec::new();
    };
    let mut best: HashMap<String, Network> = HashMap::new();
    for path in strings(&reply.child_value(0)) {
        let p = get_all(conn, NM, &path, NM_AP).await;
        let ssid =
            String::from_utf8_lossy(&prop::<Vec<u8>>(&p, "Ssid").unwrap_or_default()).into_owned();
        if ssid.is_empty() {
            continue;
        }
        let flags: u32 = prop(&p, "Flags").unwrap_or(0);
        let keys =
            prop::<u32>(&p, "WpaFlags").unwrap_or(0) | prop::<u32>(&p, "RsnFlags").unwrap_or(0);
        let network = Network {
            strength: prop(&p, "Strength").unwrap_or(0),
            // NM_802_11_AP_FLAGS_PRIVACY, or any key management.
            secured: flags & 0x1 != 0 || keys != 0,
            // NM_802_11_AP_SEC_KEY_MGMT_802_1X.
            enterprise: keys & 0x200 != 0,
            ssid: ssid.clone(),
            path,
        };
        let keep = best.get(&ssid).is_some_and(|old| {
            old.path == active || (network.path != active && old.strength >= network.strength)
        });
        if !keep {
            best.insert(ssid, network);
        }
    }
    let mut list: Vec<Network> = best.into_values().collect();
    list.sort_by(|a, b| b.strength.cmp(&a.strength).then(a.ssid.cmp(&b.ssid)));
    list
}

/// Saved Wi-Fi connections by network name.
async fn saved_networks(conn: &gio::DBusConnection) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let Some(reply) = call(
        conn,
        NM,
        NM_SETTINGS_PATH,
        NM_SETTINGS,
        "ListConnections",
        None,
    )
    .await
    else {
        return out;
    };
    for path in strings(&reply.child_value(0)) {
        let Some(settings) = call(conn, NM, &path, NM_CONNECTION, "GetSettings", None)
            .await
            .and_then(|r| r.child_value(0).get::<HashMap<String, Props>>())
        else {
            continue;
        };
        if let Some(ssid) = settings
            .get("802-11-wireless")
            .and_then(|w| prop::<Vec<u8>>(w, "ssid"))
        {
            out.insert(String::from_utf8_lossy(&ssid).into_owned(), path);
        }
    }
    out
}

async fn join(conn: &gio::DBusConnection, device: &str, network: &Network, saved: Option<&str>) {
    let (Ok(device), Ok(ap)) = (
        glib::variant::ObjectPath::try_from(device.to_owned()),
        glib::variant::ObjectPath::try_from(network.path.clone()),
    ) else {
        return;
    };
    if let Some(saved) = saved.and_then(|p| glib::variant::ObjectPath::try_from(p.to_owned()).ok())
    {
        let args = (saved, device, ap).to_variant();
        let _ = call(conn, NM, NM_PATH, NM, "ActivateConnection", Some(args)).await;
        return;
    }
    let mut wireless: Props = HashMap::new();
    wireless.insert(
        "ssid".to_owned(),
        glib::Variant::array_from_fixed_array(network.ssid.as_bytes()),
    );
    let mut settings: HashMap<String, Props> = HashMap::new();
    settings.insert("802-11-wireless".to_owned(), wireless);
    let args = (settings, device, ap).to_variant();
    let _ = call(
        conn,
        NM,
        NM_PATH,
        NM,
        "AddAndActivateConnection",
        Some(args),
    )
    .await;
}

// Sound.

#[derive(Clone, Debug, PartialEq, Eq)]
struct Sink {
    id: u32,
    name: String,
    default: bool,
}

/// The Audio section's sinks in `wpctl status`. The default is starred:
/// ` │  *   56. Built-in Audio Analog Stereo        [vol: 0.91]`.
fn parse_sinks(status: &str) -> Vec<Sink> {
    let mut out = Vec::new();
    let mut audio = false;
    let mut sinks = false;
    for line in status.lines() {
        if !line.starts_with(' ') && !line.trim().is_empty() {
            audio = line.trim() == "Audio";
            sinks = false;
            continue;
        }
        if !audio {
            continue;
        }
        if line.contains('├') || line.contains('└') {
            sinks = line.contains("Sinks:");
            continue;
        }
        if !sinks {
            continue;
        }
        let rest = line.trim_start_matches(|c: char| c == '│' || c.is_whitespace());
        let default = rest.starts_with('*');
        let rest = rest.trim_start_matches('*').trim_start();
        let Some((id, name)) = rest.split_once(". ") else {
            continue;
        };
        let Ok(id) = id.trim().parse() else {
            continue;
        };
        let name = name.split(" [").next().unwrap_or(name).trim().to_owned();
        out.push(Sink { id, name, default });
    }
    out
}

fn volume_icon(level: f64, muted: bool) -> &'static str {
    if muted || level < 0.01 {
        "audio-volume-muted-symbolic"
    } else if level < 0.34 {
        "audio-volume-low-symbolic"
    } else if level < 0.67 {
        "audio-volume-medium-symbolic"
    } else {
        "audio-volume-high-symbolic"
    }
}

fn volume(content: &gtk::Box, popover: &gtk::Popover) {
    content.append(&header("Sound", None));
    let line = gtk::Box::builder()
        .spacing(6)
        .css_classes(["elavo-indicator-volume"])
        .build();
    let mute_icon = gtk::Image::builder()
        .icon_name("audio-volume-medium-symbolic")
        .pixel_size(16)
        .build();
    let mute = gtk::ToggleButton::builder()
        .child(&mute_icon)
        .tooltip_text("Mute")
        .css_classes(["flat", "elavo-indicator-mute"])
        .valign(gtk::Align::Center)
        .build();
    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 0.01);
    scale.set_draw_value(false);
    scale.set_hexpand(true);
    line.append(&mute);
    line.append(&scale);
    content.append(&line);
    let outputs = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .build();
    content.append(&outputs);
    content.append(&separator());
    content.append(&settings_item(popover, "Sound Settings…", "sound"));

    let content = content.clone();
    let popover = popover.clone();
    glib::spawn_future_local(async move {
        if let Some((level, muted)) = run(&["wpctl", "get-volume", "@DEFAULT_AUDIO_SINK@"])
            .await
            .as_deref()
            .and_then(crate::control::parse_volume)
        {
            scale.set_value(level.clamp(0.0, 1.0));
            mute.set_active(muted);
            mute_icon.set_icon_name(Some(volume_icon(level, muted)));
        } else {
            line.set_sensitive(false);
        }
        {
            let mute = mute.clone();
            let mute_icon = mute_icon.clone();
            scale.connect_value_changed(move |scale| {
                let level = scale.value().clamp(0.0, 1.0);
                let text = format!("{level:.2}");
                let unmute = mute.is_active() && level > 0.0;
                mute_icon.set_icon_name(Some(volume_icon(level, false)));
                glib::spawn_future_local(async move {
                    let _ = run(&["wpctl", "set-volume", "@DEFAULT_AUDIO_SINK@", &text]).await;
                    if unmute {
                        let _ = run(&["wpctl", "set-mute", "@DEFAULT_AUDIO_SINK@", "0"]).await;
                    }
                });
                if unmute {
                    mute.set_active(false);
                }
            });
        }
        {
            let scale = scale.clone();
            mute.connect_toggled(move |mute| {
                let on = mute.is_active();
                mute_icon.set_icon_name(Some(volume_icon(scale.value(), on)));
                let arg = if on { "1" } else { "0" };
                glib::spawn_future_local(async move {
                    let _ = run(&["wpctl", "set-mute", "@DEFAULT_AUDIO_SINK@", arg]).await;
                });
            });
        }

        let sinks = run(&["wpctl", "status"])
            .await
            .map(|text| parse_sinks(&text))
            .unwrap_or_default();
        if sinks.is_empty() {
            return;
        }
        outputs.append(&separator());
        outputs.append(&caption("Output"));
        for sink in sinks {
            let button = item(None, &sink.name, &[], sink.default);
            let content = content.clone();
            let popover = popover.clone();
            button.connect_clicked(move |_| {
                if sink.default {
                    return;
                }
                let id = sink.id.to_string();
                let content = content.clone();
                let popover = popover.clone();
                glib::spawn_future_local(async move {
                    let _ = run(&["wpctl", "set-default", &id]).await;
                    build(Kind::Volume, &content, &popover);
                });
            });
            outputs.append(&button);
        }
    });
}

// Battery.

/// UPower's `State` and times, as a line under the percentage.
fn battery_state(state: u32, to_empty: i64, to_full: i64) -> String {
    match state {
        1 if to_full > 0 => format!("Charging, {} until full", hours(to_full)),
        1 => "Charging".into(),
        2 if to_empty > 0 => format!("{} remaining", hours(to_empty)),
        2 | 6 => "On battery".into(),
        3 => "Empty".into(),
        4 => "Fully charged".into(),
        5 => "Plugged in, not charging".into(),
        _ => String::new(),
    }
}

fn hours(seconds: i64) -> String {
    let minutes = (seconds + 30) / 60;
    format!("{}:{:02}", minutes / 60, minutes % 60)
}

fn profile_label(profile: &str) -> String {
    match profile {
        "power-saver" => "Low Power".into(),
        "balanced" => "Balanced".into(),
        "performance" => "High Performance".into(),
        other => other.to_owned(),
    }
}

fn battery(content: &gtk::Box, popover: &gtk::Popover) {
    let percent = gtk::Label::builder()
        .css_classes(["elavo-indicator-percent"])
        .build();
    content.append(&header("Battery", Some(percent.upcast_ref())));
    let state = caption("");
    state.set_visible(false);
    content.append(&state);
    let modes = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .build();
    content.append(&modes);
    content.append(&separator());
    let shown = panelcfg::show_battery_percentage();
    let toggle = item(None, "Show Percentage in Panel", &[], shown);
    {
        let popover = popover.clone();
        toggle.connect_clicked(move |_| {
            popover.popdown();
            panelcfg::set_show_battery_percentage(!shown);
        });
    }
    content.append(&toggle);
    content.append(&settings_item(popover, "Battery Settings…", "power"));

    let content = content.clone();
    let popover = popover.clone();
    glib::spawn_future_local(async move {
        let Some(conn) = system().await else {
            return;
        };
        let p = get_all(&conn, UPOWER, DISPLAY_DEVICE, UPOWER_DEVICE).await;
        if let Some(level) = prop::<f64>(&p, "Percentage") {
            percent.set_label(&format!("{}%", level.round() as i64));
        }
        let text = battery_state(
            prop(&p, "State").unwrap_or(0),
            prop(&p, "TimeToEmpty").unwrap_or(0),
            prop(&p, "TimeToFull").unwrap_or(0),
        );
        state.set_label(&text);
        state.set_visible(!text.is_empty());

        for (service, path) in PROFILES {
            let p = get_all(&conn, service, path, service).await;
            if p.is_empty() {
                continue;
            }
            let active: String = prop(&p, "ActiveProfile").unwrap_or_default();
            let available: Vec<String> = p
                .get("Profiles")
                .map(|list| {
                    list.iter()
                        .filter_map(|v| v.get::<Props>())
                        .filter_map(|d| prop::<String>(&d, "Profile"))
                        .collect()
                })
                .unwrap_or_default();
            if available.is_empty() {
                break;
            }
            modes.append(&separator());
            modes.append(&caption("Power Mode"));
            // Lowest power first, as System Settings lists them.
            for profile in ["power-saver", "balanced", "performance"]
                .into_iter()
                .filter(|name| available.iter().any(|a| a == name))
            {
                let button = item(None, &profile_label(profile), &[], profile == active);
                let conn = conn.clone();
                let content = content.clone();
                let popover = popover.clone();
                button.connect_clicked(move |_| {
                    let conn = conn.clone();
                    let content = content.clone();
                    let popover = popover.clone();
                    glib::spawn_future_local(async move {
                        set_property(
                            &conn,
                            service,
                            path,
                            service,
                            "ActiveProfile",
                            profile.to_variant(),
                        )
                        .await;
                        build(Kind::Battery, &content, &popover);
                    });
                });
                modes.append(&button);
            }
            break;
        }
    });
}

// D-Bus and processes.

async fn system() -> Option<gio::DBusConnection> {
    gio::bus_get_future(gio::BusType::System)
        .await
        .map_err(|err| glib::g_warning!("elavo-panel", "system bus: {err}"))
        .ok()
}

async fn call(
    conn: &gio::DBusConnection,
    dest: &str,
    path: &str,
    iface: &str,
    method: &str,
    args: Option<glib::Variant>,
) -> Option<glib::Variant> {
    conn.call_future(
        Some(dest),
        path,
        iface,
        method,
        args.as_ref(),
        None,
        gio::DBusCallFlags::NONE,
        10_000,
    )
    .await
    .map_err(|err| glib::g_warning!("elavo-panel", "{iface}.{method}: {err}"))
    .ok()
}

async fn get_all(conn: &gio::DBusConnection, dest: &str, path: &str, iface: &str) -> Props {
    let args = (iface,).to_variant();
    conn.call_future(
        Some(dest),
        path,
        "org.freedesktop.DBus.Properties",
        "GetAll",
        Some(&args),
        None,
        gio::DBusCallFlags::NONE,
        10_000,
    )
    .await
    .ok()
    .and_then(|reply| reply.child_value(0).get::<Props>())
    .unwrap_or_default()
}

async fn get(
    conn: &gio::DBusConnection,
    dest: &str,
    path: &str,
    iface: &str,
    name: &str,
) -> Option<glib::Variant> {
    let args = (iface, name).to_variant();
    call(
        conn,
        dest,
        path,
        "org.freedesktop.DBus.Properties",
        "Get",
        Some(args),
    )
    .await
    .and_then(|reply| reply.child_value(0).as_variant())
}

async fn set_property(
    conn: &gio::DBusConnection,
    dest: &str,
    path: &str,
    iface: &str,
    name: &str,
    value: glib::Variant,
) {
    let args = (iface, name, glib::Variant::from_variant(&value)).to_variant();
    let _ = call(
        conn,
        dest,
        path,
        "org.freedesktop.DBus.Properties",
        "Set",
        Some(args),
    )
    .await;
}

fn prop<T: glib::variant::FromVariant>(props: &Props, name: &str) -> Option<T> {
    props.get(name).and_then(|v| v.get::<T>())
}

fn strings(list: &glib::Variant) -> Vec<String> {
    list.iter()
        .filter_map(|v| v.str().map(str::to_owned))
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    const STATUS: &str = "PipeWire 'pipewire-0' [1.6.11]
 └─ Clients:
        33. uresourced                          [1.6.11]

Audio
 ├─ Devices:
 │      47. Built-in Audio                      [alsa]
 │  
 ├─ Sinks:
 │  *   56. Built-in Audio Analog Stereo        [vol: 0.91]
 │      68. Living Room TV                      [vol: 1.00]
 │  
 ├─ Sources:
 │  *   57. Built-in Audio Analog Stereo        [vol: 1.00 MUTED]
 │  
 └─ Streams:

Video
 ├─ Sinks:
 │      90. Not audio                           [x]
";

    #[test]
    fn reads_the_audio_sinks_and_the_default() {
        assert_eq!(
            parse_sinks(STATUS),
            vec![
                Sink {
                    id: 56,
                    name: "Built-in Audio Analog Stereo".into(),
                    default: true
                },
                Sink {
                    id: 68,
                    name: "Living Room TV".into(),
                    default: false
                },
            ]
        );
        assert!(parse_sinks("garbage").is_empty());
    }

    #[test]
    fn battery_lines() {
        assert_eq!(battery_state(1, 0, 3900), "Charging, 1:05 until full");
        assert_eq!(battery_state(2, 12_000, 0), "3:20 remaining");
        assert_eq!(battery_state(4, 0, 0), "Fully charged");
        assert_eq!(battery_state(0, 0, 0), "");
    }

    #[test]
    fn signal_bars() {
        assert_eq!(
            signal_icon(90),
            "network-wireless-signal-excellent-symbolic"
        );
        assert_eq!(signal_icon(40), "network-wireless-signal-ok-symbolic");
        assert_eq!(signal_icon(0), "network-wireless-signal-none-symbolic");
    }
}
