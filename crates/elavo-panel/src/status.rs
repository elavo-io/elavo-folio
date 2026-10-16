//! Network, volume, and battery icons. Display only: in phase 3 a click on
//! this area opens the control center.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gio, glib};

#[derive(Default)]
struct Indicator {
    icon: RefCell<Option<String>>,
    images: RefCell<Vec<glib::WeakRef<gtk::Image>>>,
}

impl Indicator {
    fn image(&self) -> gtk::Image {
        let image = gtk::Image::builder().pixel_size(16).build();
        Self::apply(&image, self.icon.borrow().as_deref());
        let mut images = self.images.borrow_mut();
        images.retain(|i| i.upgrade().is_some());
        images.push(image.downgrade());
        image
    }

    fn set(&self, icon: Option<String>) {
        if *self.icon.borrow() == icon {
            return;
        }
        for image in self.images.borrow().iter().filter_map(|i| i.upgrade()) {
            Self::apply(&image, icon.as_deref());
        }
        self.icon.replace(icon);
    }

    fn apply(image: &gtk::Image, icon: Option<&str>) {
        match icon {
            Some(name) => {
                image.set_icon_name(Some(name));
                image.set_visible(true);
            }
            None => image.set_visible(false),
        }
    }
}

#[derive(Default)]
pub struct Status {
    network: Indicator,
    volume: Indicator,
    battery: Indicator,
}

impl Status {
    pub fn new() -> Rc<Self> {
        let status = Rc::new(Self::default());
        glib::spawn_future_local(watch_network(Rc::downgrade(&status)));
        glib::spawn_future_local(watch_battery(Rc::downgrade(&status)));
        poll_volume(Rc::downgrade(&status));
        status
    }

    pub fn widget(&self) -> gtk::Box {
        let row = gtk::Box::builder()
            .spacing(12)
            .css_classes(["elavo-panel-status"])
            .build();
        row.append(&self.network.image());
        row.append(&self.volume.image());
        row.append(&self.battery.image());
        row.append(&search_button());
        row
    }
}

/// Toggles Lens, the same action as Super+Space.
fn search_button() -> gtk::Button {
    let icon = gtk::Image::builder()
        .icon_name("elavo-edit-find-symbolic")
        .pixel_size(16)
        .build();
    let button = gtk::Button::builder()
        .css_classes(["elavo-panel-lens"])
        .child(&icon)
        .tooltip_text("Search")
        .build();
    button.connect_clicked(|_| {
        let argv: Vec<&std::ffi::OsStr> = ["elavo-lens", "--toggle"]
            .iter()
            .map(std::ffi::OsStr::new)
            .collect();
        if let Err(err) = gio::Subprocess::newv(&argv, gio::SubprocessFlags::NONE) {
            glib::g_warning!("elavo-panel", "lens: {err}");
        }
    });
    button
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
    .map_err(|err| glib::g_warning!("elavo-panel", "{interface}: {err}"))
    .ok()
}

fn on_properties_changed(proxy: &gio::DBusProxy, update: impl Fn(&gio::DBusProxy) + 'static) {
    proxy.connect_local("g-properties-changed", false, move |args| {
        if let Some(proxy) = args.first().and_then(|v| v.get::<gio::DBusProxy>().ok()) {
            update(&proxy);
        }
        None
    });
}

fn property<T: glib::variant::FromVariant>(proxy: &gio::DBusProxy, name: &str) -> Option<T> {
    proxy.cached_property(name).and_then(|v| v.get::<T>())
}

fn network_icon(proxy: &gio::DBusProxy) -> Option<String> {
    // NM_STATE_CONNECTED_LOCAL is 50; below that there is no usable link.
    let state: u32 = property(proxy, "State")?;
    if state < 50 {
        return Some("network-offline-symbolic".into());
    }
    let kind: String = property(proxy, "PrimaryConnectionType").unwrap_or_default();
    let name = match kind.as_str() {
        k if k.contains("wireless") => "network-wireless-symbolic",
        "802-3-ethernet" => "network-wired-symbolic",
        "vpn" | "wireguard" => "network-vpn-symbolic",
        _ => "network-transmit-receive-symbolic",
    };
    Some(name.into())
}

async fn watch_network(status: std::rc::Weak<Status>) {
    let Some(proxy) = system_proxy(
        "org.freedesktop.NetworkManager",
        "/org/freedesktop/NetworkManager",
        "org.freedesktop.NetworkManager",
    )
    .await
    else {
        return;
    };
    let update = move |proxy: &gio::DBusProxy| {
        if let Some(status) = status.upgrade() {
            status.network.set(network_icon(proxy));
        }
    };
    update(&proxy);
    on_properties_changed(&proxy, update);
    std::mem::forget(proxy);
}

async fn watch_battery(status: std::rc::Weak<Status>) {
    let Some(proxy) = system_proxy(
        "org.freedesktop.UPower",
        "/org/freedesktop/UPower/devices/DisplayDevice",
        "org.freedesktop.UPower.Device",
    )
    .await
    else {
        return;
    };
    let update = move |proxy: &gio::DBusProxy| {
        let Some(status) = status.upgrade() else {
            return;
        };
        let present: bool = property(proxy, "IsPresent").unwrap_or(false);
        let icon: Option<String> = property(proxy, "IconName").filter(|s: &String| !s.is_empty());
        status.battery.set(if present { icon } else { None });
    };
    update(&proxy);
    on_properties_changed(&proxy, update);
    std::mem::forget(proxy);
}

fn volume_icon(wpctl: &str) -> Option<String> {
    // "Volume: 0.40" or "Volume: 0.40 [MUTED]"
    let level: f64 = wpctl.split_whitespace().nth(1)?.parse().ok()?;
    let name = if wpctl.contains("[MUTED]") || level < 0.01 {
        "audio-volume-muted-symbolic"
    } else if level < 0.34 {
        "audio-volume-low-symbolic"
    } else if level < 0.67 {
        "audio-volume-medium-symbolic"
    } else {
        "audio-volume-high-symbolic"
    };
    Some(name.into())
}

/// Polls `wpctl` until the control center brings a PipeWire client in phase 3.
fn poll_volume(status: std::rc::Weak<Status>) {
    glib::spawn_future_local(async move {
        loop {
            let output = match gio::Subprocess::newv(
                &[
                    "wpctl".as_ref(),
                    "get-volume".as_ref(),
                    "@DEFAULT_AUDIO_SINK@".as_ref(),
                ],
                gio::SubprocessFlags::STDOUT_PIPE | gio::SubprocessFlags::STDERR_SILENCE,
            ) {
                Ok(proc) => proc
                    .communicate_utf8_future(None)
                    .await
                    .ok()
                    .and_then(|(out, _)| out),
                Err(_) => None,
            };
            let Some(status) = status.upgrade() else {
                return;
            };
            status.volume.set(output.as_deref().and_then(volume_icon));
            drop(status);
            glib::timeout_future(Duration::from_secs(3)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::volume_icon;

    #[test]
    fn wpctl_output() {
        assert_eq!(
            volume_icon("Volume: 0.40\n").as_deref(),
            Some("audio-volume-medium-symbolic")
        );
        assert_eq!(
            volume_icon("Volume: 0.80 [MUTED]\n").as_deref(),
            Some("audio-volume-muted-symbolic")
        );
        assert_eq!(
            volume_icon("Volume: 1.00").as_deref(),
            Some("audio-volume-high-symbolic")
        );
        assert_eq!(volume_icon("garbage"), None);
    }
}
