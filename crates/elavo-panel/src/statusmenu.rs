//! Status menu: apps that register a `StatusNotifierItem`.
//!
//! The panel owns `org.kde.StatusNotifierWatcher`. libappindicator and
//! Ayatana talk to that name. There is no XEmbed tray.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use crate::dbusmenu::DbusMenu;
use crate::dropdown;

const WATCHER: &str = "org.kde.StatusNotifierWatcher";
const ITEM_IFACE: &str = "org.kde.StatusNotifierItem";
const PATH: &str = "/StatusNotifierWatcher";
const DEFAULT_ITEM_PATH: &str = "/StatusNotifierItem";

const XML: &str = r#"
<node>
  <interface name="org.kde.StatusNotifierWatcher">
    <method name="RegisterStatusNotifierItem">
      <arg name="service" type="s" direction="in"/>
    </method>
    <method name="RegisterStatusNotifierHost">
      <arg name="service" type="s" direction="in"/>
    </method>
    <property name="RegisteredStatusNotifierItems" type="as" access="read"/>
    <property name="IsStatusNotifierHostRegistered" type="b" access="read"/>
    <property name="ProtocolVersion" type="i" access="read"/>
    <signal name="StatusNotifierItemRegistered">
      <arg name="service" type="s"/>
    </signal>
    <signal name="StatusNotifierItemUnregistered">
      <arg name="service" type="s"/>
    </signal>
    <signal name="StatusNotifierHostRegistered"/>
  </interface>
</node>
"#;

thread_local! {
    static TRAY: RefCell<Option<Rc<Tray>>> = const { RefCell::new(None) };
}

struct Item {
    owner: String,
    path: String,
    title: String,
    icon_name: String,
    /// Extra directory the application asked us to search for `icon_name`.
    icon_theme_path: String,
    menu: String,
    item_is_menu: bool,
    has_activate: bool,
    has_context_menu: bool,
    pixmap: Option<gdk::MemoryTexture>,
}

/// What a button on a status icon does.
#[derive(Clone, Copy)]
enum Click {
    Activate,
    Menu,
    Context,
}

/// Left click activates the app. An item that is only a menu, or that has
/// no `Activate` method, opens the menu instead.
fn primary_click(item_is_menu: bool, has_activate: bool, has_menu: bool) -> Click {
    if has_menu && (item_is_menu || !has_activate) {
        Click::Menu
    } else {
        Click::Activate
    }
}

/// Right click opens the exported menu. Without one, it asks the app for a
/// context menu.
fn secondary_click(has_menu: bool, has_context_menu: bool) -> Option<Click> {
    if has_menu {
        Some(Click::Menu)
    } else if has_context_menu {
        Some(Click::Context)
    } else {
        None
    }
}

struct Tray {
    rows: RefCell<Vec<glib::WeakRef<gtk::Box>>>,
    items: RefCell<Vec<Item>>,
    proxies: RefCell<Vec<gio::DBusProxy>>,
    _owner: RefCell<Option<gio::OwnerId>>,
    _signals: RefCell<Vec<gio::SignalSubscription>>,
}

pub fn start() {
    if TRAY.with(|slot| slot.borrow().is_some()) {
        return;
    }
    let tray = Rc::new(Tray {
        rows: RefCell::default(),
        items: RefCell::default(),
        proxies: RefCell::default(),
        _owner: RefCell::default(),
        _signals: RefCell::default(),
    });
    TRAY.with(|slot| slot.replace(Some(tray.clone())));
    let tray_for_name = tray.clone();
    let id = gio::bus_own_name(
        gio::BusType::Session,
        WATCHER,
        gio::BusNameOwnerFlags::NONE,
        move |bus, _| {
            if let Err(err) = export(&bus) {
                glib::g_warning!("elavo-panel", "status menu: {err}");
            }
            watch_names(&bus, &tray_for_name);
        },
        |_, _| {},
        |_, _| glib::g_warning!("elavo-panel", "status menu name was not acquired"),
    );
    tray._owner.replace(Some(id));
}

/// One box of icons for one bar. Empty, and hidden, until an app registers.
pub fn row() -> gtk::Box {
    let row = gtk::Box::builder()
        .spacing(12)
        .css_classes(["elavo-panel-status-menu"])
        .build();
    row.set_visible(false);
    TRAY.with(|slot| {
        if let Some(tray) = slot.borrow().clone() {
            tray.fill(&row);
            tray.rows.borrow_mut().push(row.downgrade());
        }
    });
    row
}

fn export(bus: &gio::DBusConnection) -> Result<(), glib::Error> {
    let node = gio::DBusNodeInfo::for_xml(XML)?;
    let node = Box::leak(Box::new(node));
    let iface = &node.interfaces()[0];
    bus.register_object(PATH, iface)
        .method_call(
            |conn, sender, _path, _iface, method, params, invocation| match method {
                "RegisterStatusNotifierItem" => {
                    let service = params.child_value(0).str().unwrap_or("").to_owned();
                    register(&conn, sender.unwrap_or(""), &service);
                    invocation.return_value(None);
                }
                "RegisterStatusNotifierHost" => {
                    let _ =
                        conn.emit_signal(None, PATH, WATCHER, "StatusNotifierHostRegistered", None);
                    invocation.return_value(None);
                }
                _ => {
                    invocation.return_dbus_error("org.freedesktop.DBus.Error.UnknownMethod", method)
                }
            },
        )
        .property(|_conn, _sender, _path, _iface, name| match name {
            "RegisteredStatusNotifierItems" => registered_ids().to_variant(),
            "IsStatusNotifierHostRegistered" => true.to_variant(),
            "ProtocolVersion" => 0i32.to_variant(),
            _ => glib::Variant::from_none(glib::VariantTy::ANY),
        })
        .build()?;
    Ok(())
}

fn registered_ids() -> Vec<String> {
    TRAY.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|tray| {
                tray.items
                    .borrow()
                    .iter()
                    .map(|item| format!("{}{}", item.owner, item.path))
                    .collect()
            })
            .unwrap_or_default()
    })
}

fn register(conn: &gio::DBusConnection, sender: &str, service: &str) {
    let (owner, path) = if let Some(rest) = service.strip_prefix('/') {
        let _ = rest;
        (sender.to_owned(), service.to_owned())
    } else if service.is_empty() {
        return;
    } else {
        (service.to_owned(), DEFAULT_ITEM_PATH.to_owned())
    };
    if owner.is_empty() {
        return;
    }
    let _ = conn.emit_signal(
        None,
        PATH,
        WATCHER,
        "StatusNotifierItemRegistered",
        Some(&one_string(service)),
    );
    let owner2 = owner.clone();
    let path2 = path.clone();
    glib::spawn_future_local(async move {
        let Ok(proxy) = gio::DBusProxy::for_bus_future(
            gio::BusType::Session,
            gio::DBusProxyFlags::NONE,
            None,
            &owner2,
            &path2,
            ITEM_IFACE,
        )
        .await
        else {
            return;
        };
        // A unique-name proxy keeps reporting that name after the app has
        // quit. Ask the bus. The quit signal may already have been delivered
        // while this proxy was being created, and again in the same read as
        // the answer, before the item exists to be removed.
        let bus = proxy.connection();
        let watched = live_owner(&proxy).unwrap_or_else(|| owner2.clone());
        let methods = item_methods(&bus, &owner2, &path2).await;
        if name_has_owner(&bus, &owner2).await {
            adopt(proxy, methods);
            if !name_has_owner(&bus, &watched).await {
                remove_owner(&watched);
            }
        }
    });
}

fn adopt(proxy: gio::DBusProxy, methods: (bool, bool)) {
    // A proxy can finish coming up after the app has already quit. Drawing
    // it then leaves a gear that nothing owns.
    let Some(owner) = live_owner(&proxy) else {
        return;
    };
    let path = proxy.object_path().to_string();
    let owner_gone = owner.clone();
    proxy.connect_notify_local(Some("g-name-owner"), move |proxy, _| {
        if !still_owned(live_owner(proxy).as_deref(), &owner_gone) {
            remove_owner(&owner_gone);
        }
    });
    if !still_owned(live_owner(&proxy).as_deref(), &owner) {
        return;
    }
    refresh_item(&proxy, &owner, &path, Some(methods));
    let owner_c = owner.clone();
    let path_c = path.clone();
    proxy.connect_local("g-properties-changed", false, move |args| {
        if let Some(proxy) = args.first().and_then(|v| v.get::<gio::DBusProxy>().ok()) {
            if still_owned(live_owner(&proxy).as_deref(), &owner_c) {
                refresh_item(&proxy, &owner_c, &path_c, None);
            } else {
                remove_owner(&owner_c);
            }
        }
        None
    });
    let proxy_s = proxy.clone();
    let owner_icon = owner.clone();
    let sub = proxy.connection().subscribe_to_signal(
        proxy.name().as_deref(),
        Some(ITEM_IFACE),
        Some("NewIcon"),
        Some(proxy.object_path().as_str()),
        None,
        gio::DBusSignalFlags::NONE,
        move |_| {
            let proxy_s = proxy_s.clone();
            let owner_icon = owner_icon.clone();
            glib::spawn_future_local(async move {
                if !still_owned(live_owner(&proxy_s).as_deref(), &owner_icon) {
                    remove_owner(&owner_icon);
                    return;
                }
                let _ = proxy_s
                    .call_future(
                        "org.freedesktop.DBus.Properties.GetAll",
                        Some(&one_string(ITEM_IFACE)),
                        gio::DBusCallFlags::NONE,
                        1000,
                    )
                    .await;
                if still_owned(live_owner(&proxy_s).as_deref(), &owner_icon) {
                    refresh_item(&proxy_s, &owner_icon, proxy_s.object_path().as_str(), None);
                } else {
                    remove_owner(&owner_icon);
                }
            });
        },
    );
    TRAY.with(|slot| {
        if let Some(tray) = slot.borrow().clone() {
            tray.proxies.borrow_mut().push(proxy);
            tray._signals.borrow_mut().push(sub);
        }
    });
}

fn refresh_item(proxy: &gio::DBusProxy, owner: &str, path: &str, methods: Option<(bool, bool)>) {
    let item = Item {
        owner: owner.to_owned(),
        path: path.to_owned(),
        title: string_prop(proxy, "Title")
            .unwrap_or_else(|| string_prop(proxy, "Id").unwrap_or_default()),
        icon_name: string_prop(proxy, "IconName").unwrap_or_default(),
        icon_theme_path: string_prop(proxy, "IconThemePath").unwrap_or_default(),
        menu: string_prop(proxy, "Menu").unwrap_or_default(),
        item_is_menu: proxy
            .cached_property("ItemIsMenu")
            .and_then(|v| v.get::<bool>())
            .unwrap_or(false),
        has_activate: methods.map(|(activate, _)| activate).unwrap_or(false),
        has_context_menu: methods.map(|(_, context)| context).unwrap_or(false),
        pixmap: proxy.cached_property("IconPixmap").and_then(|v| pixmap(&v)),
    };
    TRAY.with(|slot| {
        let Some(tray) = slot.borrow().clone() else {
            return;
        };
        {
            let mut items = tray.items.borrow_mut();
            if let Some(existing) = items
                .iter_mut()
                .find(|i| i.owner == item.owner && i.path == item.path)
            {
                let mut item = item;
                if methods.is_none() {
                    item.has_activate = existing.has_activate;
                    item.has_context_menu = existing.has_context_menu;
                }
                *existing = item;
            } else {
                items.push(item);
            }
        }
        tray.rebuild();
    });
}

fn watch_names(bus: &gio::DBusConnection, tray: &Rc<Tray>) {
    let sub = bus.subscribe_to_signal(
        Some("org.freedesktop.DBus"),
        Some("org.freedesktop.DBus"),
        Some("NameOwnerChanged"),
        Some("/org/freedesktop/DBus"),
        None,
        gio::DBusSignalFlags::NONE,
        |signal| {
            let name_v = signal.parameters.child_value(0);
            let new_v = signal.parameters.child_value(2);
            let name = name_v.str().unwrap_or("");
            let new = new_v.str().unwrap_or("");
            if new.is_empty() && !name.is_empty() {
                remove_owner(name);
            }
        },
    );
    tray._signals.borrow_mut().push(sub);
}

fn remove_owner(owner: &str) {
    TRAY.with(|slot| {
        let Some(tray) = slot.borrow().clone() else {
            return;
        };
        let removed: Vec<String> = {
            let mut items = tray.items.borrow_mut();
            let before = items.len();
            let gone: Vec<String> = items
                .iter()
                .filter(|item| item.owner == owner)
                .map(|item| format!("{}{}", item.owner, item.path))
                .collect();
            items.retain(|item| item.owner != owner);
            if items.len() == before {
                return;
            }
            gone
        };
        let bus = tray
            .proxies
            .borrow()
            .first()
            .map(|proxy| proxy.connection());
        tray.proxies.borrow_mut().retain(|proxy| {
            live_owner(proxy)
                .is_some_and(|current| current != owner && proxy.name().as_deref() != Some(owner))
        });
        tray.rebuild();
        if let Some(bus) = bus {
            for id in removed {
                let _ = bus.emit_signal(
                    None,
                    PATH,
                    WATCHER,
                    "StatusNotifierItemUnregistered",
                    Some(&one_string(&id)),
                );
            }
        }
    });
}

impl Tray {
    fn fill(&self, row: &gtk::Box) {
        clear(row);
        for item in self.items.borrow().iter() {
            row.append(&item_button(item));
        }
        row.set_visible(!self.items.borrow().is_empty());
    }

    fn rebuild(&self) {
        self.rows.borrow_mut().retain(|row| row.upgrade().is_some());
        for row in self.rows.borrow().iter().filter_map(|row| row.upgrade()) {
            self.fill(&row);
        }
    }
}

fn item_button(item: &Item) -> gtk::Button {
    let child: gtk::Widget = if let Some(icon) = named_icon(&item.icon_name, &item.icon_theme_path)
    {
        icon
    } else if let Some(texture) = &item.pixmap {
        let image = gtk::Image::from_paintable(Some(texture));
        image.set_pixel_size(16);
        image.upcast()
    } else {
        gtk::Image::builder()
            .icon_name("application-x-executable-symbolic")
            .pixel_size(16)
            .build()
            .upcast()
    };
    let button = gtk::Button::builder()
        .css_classes(["flat", "elavo-panel-status-item"])
        .child(&child)
        .build();
    // The name stays available to accessibility. A tooltip is a popover,
    // and the dark theme draws that popover with no visible text.
    button.update_property(&[gtk::accessible::Property::Label(&item.title)]);
    let owner = item.owner.clone();
    let menu = item.menu.clone();
    let primary = primary_click(
        item.item_is_menu,
        item.has_activate,
        is_menu_path(&item.menu),
    );
    let secondary = secondary_click(is_menu_path(&item.menu), item.has_context_menu);
    button.connect_clicked(move |button| {
        run_click(&primary, button, &owner, &menu);
    });
    if let Some(secondary) = secondary {
        let owner = item.owner.clone();
        let menu = item.menu.clone();
        let right = gtk::GestureClick::new();
        right.set_button(gdk::BUTTON_SECONDARY);
        right.connect_pressed(move |gesture, _, _, _| {
            gesture.set_state(gtk::EventSequenceState::Claimed);
            let Some(button) = gesture.widget().and_downcast::<gtk::Button>() else {
                return;
            };
            run_click(&secondary, &button, &owner, &menu);
        });
        button.add_controller(right);
    }
    unsafe {
        button.set_data("sni-path", item.path.clone());
    }
    button
}

fn button_path(button: &gtk::Button) -> String {
    unsafe {
        button
            .data::<String>("sni-path")
            .map(|p| p.as_ref().clone())
            .unwrap_or_default()
    }
}

fn is_menu_path(path: &str) -> bool {
    !path.is_empty() && path != "/"
}

fn run_click(click: &Click, button: &gtk::Button, owner: &str, menu: &str) {
    let owner = owner.to_owned();
    let path = button_path(button);
    match click {
        Click::Activate => {
            glib::spawn_future_local(async move {
                activate(&owner, &path).await;
            });
        }
        Click::Menu => open_menu(button, owner, menu.to_owned()),
        Click::Context => {
            glib::spawn_future_local(async move {
                context_menu(&owner, &path).await;
            });
        }
    }
}

fn open_menu(button: &gtk::Button, owner: String, path: String) {
    let button = button.clone();
    glib::spawn_future_local(async move {
        let Some(menu) = DbusMenu::load(&owner, &path, gio::Menu::new().upcast()).await else {
            activate(&owner, &button_path(&button)).await;
            return;
        };
        let popover = gtk::PopoverMenu::from_model(Some(&menu.model()));
        // Items are `app.<id>`. Without this group GTK disables every row,
        // because the bar's `app` group belongs to the focused window.
        popover.insert_action_group("app", Some(&menu.actions()));
        // A new popover is visible. Parenting it now would show an empty
        // one on hover, before it is popped up on purpose.
        popover.set_visible(false);
        popover.set_parent(&button);
        dropdown::prepare(popover.upcast_ref(), true);
        // The icon sits at the right. Grow the menu toward the left.
        popover.set_halign(gtk::Align::End);
        // Unparenting inside `closed` runs before the row's action, and
        // takes the action group with it, so the click does nothing. It
        // also leaves the popover showing on the next hover.
        popover.connect_closed(|popover| {
            let popover = popover.clone();
            glib::idle_add_local_once(move || {
                if popover.parent().is_some() && !popover.is_visible() {
                    popover.unparent();
                }
            });
        });
        unsafe {
            popover.set_data("sni-menu", menu);
        }
        popover.popup();
    });
}

async fn context_menu(owner: &str, path: &str) {
    call_item(owner, path, "ContextMenu").await;
}

async fn activate(owner: &str, path: &str) {
    call_item(owner, path, "Activate").await;
}

async fn call_item(owner: &str, path: &str, method: &str) {
    if owner.is_empty() || path.is_empty() {
        return;
    }
    let Ok(proxy) = gio::DBusProxy::for_bus_future(
        gio::BusType::Session,
        gio::DBusProxyFlags::DO_NOT_LOAD_PROPERTIES,
        None,
        owner,
        path,
        ITEM_IFACE,
    )
    .await
    else {
        return;
    };
    if let Err(err) = proxy
        .call_future(
            method,
            Some(&(0i32, 0i32).to_variant()),
            gio::DBusCallFlags::NONE,
            1000,
        )
        .await
    {
        glib::g_warning!("elavo-panel", "{method}: {err}");
    }
}

/// Whether the item exports `Activate` and `ContextMenu`. A unique-name proxy
/// does not say, so this reads the introspection XML.
async fn item_methods(bus: &gio::DBusConnection, owner: &str, path: &str) -> (bool, bool) {
    let Ok(reply) = bus
        .call_future(
            Some(owner),
            path,
            "org.freedesktop.DBus.Introspectable",
            "Introspect",
            Some(&().to_variant()),
            None,
            gio::DBusCallFlags::NONE,
            1000,
        )
        .await
    else {
        return (false, false);
    };
    let xml = reply.child_value(0).get::<String>().unwrap_or_default();
    (
        xml.contains("<method name=\"Activate\""),
        xml.contains("<method name=\"ContextMenu\""),
    )
}

async fn name_has_owner(bus: &gio::DBusConnection, name: &str) -> bool {
    let Ok(reply) = bus
        .call_future(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "NameHasOwner",
            Some(&(name,).to_variant()),
            None,
            gio::DBusCallFlags::NONE,
            1000,
        )
        .await
    else {
        return false;
    };
    reply.child_value(0).get::<bool>().unwrap_or(false)
}

/// The unique name that currently owns this item, if the app is still running.
fn live_owner(proxy: &gio::DBusProxy) -> Option<String> {
    let owner = proxy.name_owner()?.to_string();
    if owner.is_empty() {
        None
    } else {
        Some(owner)
    }
}

/// `current` is the name owner now. An item stays only while that is the name
/// it was adopted under.
fn still_owned(current: Option<&str>, adopted: &str) -> bool {
    current == Some(adopted) && !adopted.is_empty()
}

/// A one-argument D-Bus signal is a tuple. A bare string is rejected.
fn one_string(value: &str) -> glib::Variant {
    (value,).to_variant()
}

fn string_prop(proxy: &gio::DBusProxy, name: &str) -> Option<String> {
    let value = proxy.cached_property(name)?;
    let text = value
        .str()
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
        .or_else(|| value.get::<String>().filter(|text| !text.is_empty()));
    text
}

/// The theme's icon when it has this name. Otherwise the file the
/// application shipped under `IconThemePath`, either `{name}.png` in that
/// directory or an icon theme rooted there.
fn named_icon(name: &str, theme_path: &str) -> Option<gtk::Widget> {
    if name.is_empty() {
        return None;
    }
    if gtk::IconTheme::default().has_icon(name) {
        return Some(
            gtk::Image::builder()
                .icon_name(name)
                .pixel_size(16)
                .build()
                .upcast(),
        );
    }
    let paintable = loose_texture(theme_path, name).or_else(|| app_theme_icon(theme_path, name))?;
    let image = gtk::Image::from_paintable(Some(&paintable));
    image.set_pixel_size(16);
    Some(image.upcast())
}

fn loose_texture(theme_path: &str, name: &str) -> Option<gdk::Paintable> {
    let path = loose_icon(std::path::Path::new(theme_path), name)?;
    let texture = gdk::Texture::from_filename(path).ok()?;
    Some(trimmed(&texture).upcast())
}

/// Drop the clear margin around an application icon so the mark fills the
/// same 16px box as a theme icon. Steam's tray file is a small mark in a
/// larger canvas.
fn trimmed(texture: &gdk::Texture) -> gdk::Paintable {
    let width = texture.width();
    let height = texture.height();
    if !(1..=256).contains(&width) || !(1..=256).contains(&height) {
        return texture.clone().upcast();
    }
    let stride = width as usize * 4;
    let mut data = vec![0u8; stride * height as usize];
    texture.download(&mut data, stride);
    let Some((x, y, w, h)) = content_bounds(&data, width, height, stride) else {
        return texture.clone().upcast();
    };
    if w >= width - 1 && h >= height - 1 {
        return texture.clone().upcast();
    }
    let mut cropped = Vec::with_capacity((w * h * 4) as usize);
    for row in 0..h {
        let start = ((y + row) as usize * stride) + x as usize * 4;
        cropped.extend_from_slice(&data[start..start + w as usize * 4]);
    }
    gdk::MemoryTexture::new(
        w,
        h,
        gdk::MemoryFormat::R8g8b8a8,
        &glib::Bytes::from_owned(cropped),
        (w * 4) as usize,
    )
    .upcast()
}

/// The rectangle of pixels that are actually drawn. `None` when the image
/// is empty.
fn content_bounds(
    rgba: &[u8],
    width: i32,
    height: i32,
    stride: usize,
) -> Option<(i32, i32, i32, i32)> {
    let mut min_x = width;
    let mut min_y = height;
    let mut max_x = -1;
    let mut max_y = -1;
    for y in 0..height {
        let row = y as usize * stride;
        for x in 0..width {
            let alpha = rgba.get(row + x as usize * 4 + 3).copied().unwrap_or(0);
            if alpha > 32 {
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
    }
    (max_x >= min_x).then_some((min_x, min_y, max_x - min_x + 1, max_y - min_y + 1))
}

/// `IconThemePath` may be a directory of themes (`hicolor/...`) rather than
/// a single loose file. A fresh theme leaves the session theme alone.
fn app_theme_icon(theme_path: &str, name: &str) -> Option<gdk::Paintable> {
    let dir = std::path::Path::new(theme_path);
    if !dir.is_dir() {
        return None;
    }
    let theme = gtk::IconTheme::new();
    theme.add_search_path(dir);
    if let Some(parent) = dir.parent() {
        theme.add_search_path(parent);
    }
    if !theme.has_icon(name) {
        return None;
    }
    Some(
        theme
            .lookup_icon(
                name,
                &[],
                16,
                1,
                gtk::TextDirection::None,
                gtk::IconLookupFlags::empty(),
            )
            .upcast(),
    )
}

/// `{dir}/{name}.png`, or `.svg` / `.svgz`. A name with a slash is not a file
/// in that directory.
fn loose_icon(dir: &std::path::Path, name: &str) -> Option<std::path::PathBuf> {
    if !icon_name_ok(name) || !dir.is_dir() {
        return None;
    }
    for ext in ["png", "svg", "svgz"] {
        let file = dir.join(format!("{name}.{ext}"));
        if file.is_file() {
            return Some(file);
        }
    }
    None
}

fn icon_name_ok(name: &str) -> bool {
    !name.is_empty()
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains('\0')
        && !name.contains("..")
}

/// The largest icon in an `a(iiay)` pixmap. Bytes are ARGB, network order.
fn pixmap(value: &glib::Variant) -> Option<gdk::MemoryTexture> {
    let mut best: Option<(i32, i32, Vec<u8>)> = None;
    for i in 0..value.n_children() {
        let child = value.child_value(i);
        let Some(width) = child.child_value(0).get::<i32>() else {
            continue;
        };
        let Some(height) = child.child_value(1).get::<i32>() else {
            continue;
        };
        let Some(bytes) = child.child_value(2).get::<Vec<u8>>() else {
            continue;
        };
        if width <= 0 || height <= 0 || bytes.len() != (width * height * 4) as usize {
            continue;
        }
        let replace = best.as_ref().is_none_or(|(w, _, _)| width > *w);
        if replace {
            best = Some((width, height, bytes));
        }
    }
    let (width, height, argb) = best?;
    let mut rgba = Vec::with_capacity(argb.len());
    for pixel in argb.as_chunks::<4>().0 {
        rgba.extend_from_slice(&[pixel[1], pixel[2], pixel[3], pixel[0]]);
    }
    Some(gdk::MemoryTexture::new(
        width,
        height,
        gdk::MemoryFormat::R8g8b8a8,
        &glib::Bytes::from_owned(rgba),
        (width * 4) as usize,
    ))
}

fn clear(row: &gtk::Box) {
    while let Some(child) = row.first_child() {
        row.remove(&child);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        content_bounds, icon_name_ok, loose_icon, one_string, pixmap, primary_click,
        secondary_click, still_owned, Click,
    };
    use gtk::gdk::prelude::TextureExt;
    use gtk::glib;

    #[test]
    fn a_left_click_activates_and_a_right_click_opens_the_menu() {
        assert!(matches!(primary_click(false, true, true), Click::Activate));
        assert!(matches!(secondary_click(true, true), Some(Click::Menu)));
        assert!(matches!(primary_click(true, true, true), Click::Menu));
        assert!(matches!(primary_click(false, false, true), Click::Menu));
        assert!(matches!(secondary_click(false, true), Some(Click::Context)));
        assert!(secondary_click(false, false).is_none());
    }

    #[test]
    fn a_vanished_owner_is_not_kept() {
        assert!(!still_owned(None, ":1.12519"));
        assert!(!still_owned(Some(":1.12525"), ":1.12519"));
        assert!(!still_owned(Some(""), ":1.12519"));
        assert!(still_owned(Some(":1.12525"), ":1.12525"));
    }

    #[test]
    fn a_status_signal_is_a_tuple() {
        assert_eq!(
            one_string("org.kde.StatusNotifierItem-1").type_().as_str(),
            "(s)"
        );
    }

    #[test]
    fn an_app_directory_provides_its_own_icon() {
        let dir = std::env::temp_dir().join(format!("elavo-sni-icon-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("steam_tray_mono.png"), b"png").unwrap();
        assert_eq!(
            loose_icon(&dir, "steam_tray_mono").as_deref(),
            Some(dir.join("steam_tray_mono.png").as_path())
        );
        assert!(loose_icon(&dir, "missing").is_none());
        assert!(loose_icon(&dir, "../steam_tray_mono").is_none());
        assert!(!icon_name_ok("steam/tray"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn clear_margin_is_not_part_of_the_icon() {
        let mut rgba = vec![0u8; 4 * 4 * 4];
        // A 2×2 mark in the bottom right of a 4×4 canvas.
        for (x, y) in [(2, 2), (3, 2), (2, 3), (3, 3)] {
            rgba[(y * 4 + x) * 4 + 3] = 255;
        }
        assert_eq!(content_bounds(&rgba, 4, 4, 16), Some((2, 2, 2, 2)));
        rgba.fill(0);
        rgba[3] = 255;
        assert_eq!(content_bounds(&rgba, 1, 1, 4), Some((0, 0, 1, 1)));
    }

    #[test]
    fn an_argb_pixmap_becomes_a_square_texture() {
        let texture = pixmap(&sample_pixmap()).expect("pixmap");
        assert_eq!(texture.width(), 1);
        assert_eq!(texture.height(), 1);
    }

    fn sample_pixmap() -> glib::Variant {
        // (iiay) then wrap in an array by parsing a literal.
        glib::Variant::parse(
            Some(glib::VariantTy::new("a(iiay)").unwrap()),
            "[(1, 1, [255, 10, 20, 30])]",
        )
        .unwrap()
    }
}
