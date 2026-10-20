//! Network, volume, and battery on the greeter's bar, from the same sources
//! as the panel. Volume needs a PipeWire session, so it only appears on the
//! lock screen.

use std::cell::RefCell;
use std::rc::{Rc, Weak};
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
        self.images.borrow_mut().push(image.downgrade());
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
        image.set_icon_name(icon);
        image.set_visible(icon.is_some());
    }
}

#[derive(Default)]
pub struct Status {
    network: Indicator,
    volume: Indicator,
    battery: Indicator,
}

impl Status {
    pub fn new(volume: bool) -> Rc<Self> {
        let status = Rc::new(Self::default());
        glib::spawn_future_local(watch_network(Rc::downgrade(&status)));
        glib::spawn_future_local(watch_battery(Rc::downgrade(&status)));
        if volume {
            poll_volume(Rc::downgrade(&status));
        }
        status
    }

    pub fn append_to(&self, row: &gtk::Box) {
        row.append(&self.network.image());
        row.append(&self.volume.image());
        row.append(&self.battery.image());
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

fn watch(proxy: gio::DBusProxy, update: impl Fn(&gio::DBusProxy) + 'static) {
    update(&proxy);
    proxy.connect_local("g-properties-changed", false, move |args| {
        if let Some(proxy) = args.first().and_then(|v| v.get::<gio::DBusProxy>().ok()) {
            update(&proxy);
        }
        None
    });
    std::mem::forget(proxy);
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

async fn watch_network(status: Weak<Status>) {
    let Some(proxy) = system_proxy(
        "org.freedesktop.NetworkManager",
        "/org/freedesktop/NetworkManager",
        "org.freedesktop.NetworkManager",
    )
    .await
    else {
        return;
    };
    watch(proxy, move |proxy| {
        if let Some(status) = status.upgrade() {
            status.network.set(network_icon(proxy));
        }
    });
}

async fn watch_battery(status: Weak<Status>) {
    let Some(proxy) = system_proxy(
        "org.freedesktop.UPower",
        "/org/freedesktop/UPower/devices/DisplayDevice",
        "org.freedesktop.UPower.Device",
    )
    .await
    else {
        return;
    };
    watch(proxy, move |proxy| {
        let Some(status) = status.upgrade() else {
            return;
        };
        let present: bool = property(proxy, "IsPresent").unwrap_or(false);
        let icon: Option<String> = property(proxy, "IconName").filter(|s: &String| !s.is_empty());
        status.battery.set(if present { icon } else { None });
    });
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

fn poll_volume(status: Weak<Status>) {
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
            volume_icon("Volume: 0.80 [MUTED]").as_deref(),
            Some("audio-volume-muted-symbolic")
        );
        assert_eq!(volume_icon("garbage"), None);
    }
}
