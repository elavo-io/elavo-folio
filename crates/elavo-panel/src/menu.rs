//! The Elavo menu behind the sun: About, Settings, power, lock, and log out.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};
use gtk4_layer_shell::{KeyboardMode, LayerShell};

/// Register the menu's `app.*` actions once per process.
pub fn install_actions(app: &gtk::Application) {
    let add = |name: &str, f: fn()| {
        let action = gio::SimpleAction::new(name, None);
        action.connect_activate(move |_, _| f());
        app.add_action(&action);
    };
    add("sleep", || login1("Suspend"));
    add("restart", || {
        confirm("Restart now?", "Restart", || login1("Reboot"));
    });
    add("shut-down", || {
        confirm("Shut down now?", "Shut Down", || login1("PowerOff"));
    });
    add("lock", || spawn(&["loginctl", "lock-session"]));
    add("log-out", || {
        confirm("Log out now?", "Log Out", || spawn(&["labwc", "--exit"]));
    });

    let installed = system_settings().is_some();
    for (name, pane) in [("about", "about"), ("update", "updates"), ("settings", "")] {
        let action = gio::SimpleAction::new(name, None);
        action.set_enabled(installed);
        action.connect_activate(move |_, _| open_settings(pane));
        app.add_action(&action);
    }
}

/// Open a System Settings pane. An empty pane opens Settings where it last was.
pub(crate) fn open_settings(pane: &str) {
    let Some(info) = system_settings() else {
        return;
    };
    let program = gtk::gio::prelude::AppInfoExt::commandline(&info)
        .unwrap_or_else(|| std::path::PathBuf::from("elavo-system-settings"));
    let mut argv = vec![program.into_os_string()];
    if !pane.is_empty() {
        argv.push(std::ffi::OsString::from(pane));
    }
    let refs: Vec<&std::ffi::OsStr> = argv.iter().map(std::ffi::OsString::as_os_str).collect();
    if let Err(err) = gio::Subprocess::newv(&refs, gio::SubprocessFlags::NONE) {
        glib::g_warning!("elavo-panel", "settings: {err}");
    }
}

/// System Settings, when it is installed.
fn system_settings() -> Option<elavo_common::DesktopAppInfo> {
    elavo_common::DesktopAppInfo::new("io.elavo.SystemSettings.desktop")
}

fn model() -> gio::Menu {
    let menu = gio::Menu::new();

    let about = gio::Menu::new();
    about.append(Some("About Elavo"), Some("app.about"));
    about.append(Some("Software Update"), Some("app.update"));
    menu.append_section(None, &about);

    let settings = gio::Menu::new();
    settings.append(Some("System Settings…"), Some("app.settings"));
    menu.append_section(None, &settings);

    let power = gio::Menu::new();
    power.append(Some("Sleep"), Some("app.sleep"));
    power.append(Some("Restart…"), Some("app.restart"));
    power.append(Some("Shut Down…"), Some("app.shut-down"));
    menu.append_section(None, &power);

    let session = gio::Menu::new();
    session.append(Some("Lock Screen"), Some("app.lock"));
    session.append(Some("Log Out…"), Some("app.log-out"));
    menu.append_section(None, &session);

    menu
}

/// The sun button for one bar. While the menu is open the bar takes keyboard
/// focus so the arrow keys work; a layer surface has none otherwise.
pub fn button(window: &gtk::ApplicationWindow) -> gtk::MenuButton {
    let button = gtk::MenuButton::builder()
        .icon_name("elavo-sun-symbolic")
        .tooltip_text("Elavo")
        .css_classes(["flat", "elavo-panel-menu"])
        .menu_model(&model())
        .always_show_arrow(false)
        .build();
    if let Some(image) = button
        .first_child()
        .and_then(|toggle| toggle.first_child())
        .and_downcast::<gtk::Image>()
    {
        image.set_pixel_size(16);
    }
    if let Some(popover) = button.popover() {
        popover.set_has_arrow(false);
        popover.add_css_class("elavo-panel-menu-popover");
        let window_ref = window.downgrade();
        popover.connect_show(move |_| {
            if let Some(window) = window_ref.upgrade() {
                window.set_keyboard_mode(KeyboardMode::OnDemand);
            }
        });
        let window_ref = window.downgrade();
        popover.connect_closed(move |_| {
            if let Some(window) = window_ref.upgrade() {
                window.set_keyboard_mode(KeyboardMode::None);
            }
        });
    }
    button
}

/// Make the sun part of the menu bar beside it: while one of the two has a
/// menu open, moving the pointer onto the other opens that one instead.
pub fn link(sun: &gtk::MenuButton, region: &gtk::Box) {
    let to_sun = gtk::EventControllerMotion::new();
    let region_ref = region.downgrade();
    to_sun.connect_enter(move |motion, _, _| {
        let open = region_ref
            .upgrade()
            .and_then(|region| open_popover(region.upcast_ref()));
        if let (Some(open), Some(sun)) = (open, motion.widget().and_downcast::<gtk::MenuButton>()) {
            open.popdown();
            sun.popup();
        }
    });
    sun.add_controller(to_sun);

    // While the sun's menu is open the bar gets crossing events but no motion,
    // so each menu bar item listens for the pointer until the menu closes.
    let Some(popover) = sun.popover() else {
        return;
    };
    let watching: Rc<RefCell<Vec<(gtk::Widget, gtk::EventControllerMotion)>>> = Rc::default();
    let (region_ref, sun_ref, list) = (region.downgrade(), sun.downgrade(), Rc::clone(&watching));
    popover.connect_show(move |_| {
        let Some(region) = region_ref.upgrade() else {
            return;
        };
        for item in menubar_items(region.upcast_ref()) {
            let motion = gtk::EventControllerMotion::new();
            let (sun_ref, item_ref) = (sun_ref.clone(), item.downgrade());
            motion.connect_enter(move |_, _, _| {
                // Closing the menu removes this controller, which GTK must not
                // see while it is still delivering the crossing.
                let (sun_ref, item_ref) = (sun_ref.clone(), item_ref.clone());
                glib::idle_add_local_once(move || {
                    if let (Some(sun), Some(item)) = (sun_ref.upgrade(), item_ref.upgrade()) {
                        sun.popdown();
                        item.activate();
                    }
                });
            });
            item.add_controller(motion.clone());
            list.borrow_mut().push((item, motion));
        }
    });
    popover.connect_closed(move |_| {
        let stale = watching.take();
        glib::idle_add_local_once(move || {
            for (item, motion) in stale {
                item.remove_controller(&motion);
            }
        });
    });
}

/// The top-level items of the menu bar under `widget`.
fn menubar_items(widget: &gtk::Widget) -> Vec<gtk::Widget> {
    let mut items = Vec::new();
    let mut child = widget.first_child();
    while let Some(current) = child {
        if current.is::<gtk::PopoverMenuBar>() {
            let mut item = current.first_child();
            while let Some(it) = item {
                item = it.next_sibling();
                items.push(it);
            }
        } else {
            items.extend(menubar_items(&current));
        }
        child = current.next_sibling();
    }
    items
}

/// A visible popover under `widget`, not looking inside popovers.
fn open_popover(widget: &gtk::Widget) -> Option<gtk::Popover> {
    let mut child = widget.first_child();
    while let Some(current) = child {
        match current.downcast_ref::<gtk::Popover>() {
            Some(popover) if popover.is_visible() => return Some(popover.clone()),
            Some(_) => {}
            None => {
                if let Some(popover) = open_popover(&current) {
                    return Some(popover);
                }
            }
        }
        child = current.next_sibling();
    }
    None
}

fn spawn(argv: &[&str]) {
    let argv: Vec<&std::ffi::OsStr> = argv.iter().map(std::ffi::OsStr::new).collect();
    if let Err(err) = gio::Subprocess::newv(&argv, gio::SubprocessFlags::NONE) {
        glib::g_warning!("elavo-panel", "{:?}: {err}", argv);
    }
}

/// Call a logind manager method with interactive=true, so polkit can ask
/// when the session is not allowed outright.
fn login1(method: &'static str) {
    glib::spawn_future_local(async move {
        let result = async {
            let bus = gio::bus_get_future(gio::BusType::System).await?;
            bus.call_future(
                Some("org.freedesktop.login1"),
                "/org/freedesktop/login1",
                "org.freedesktop.login1.Manager",
                method,
                Some(&(true,).to_variant()),
                None,
                gio::DBusCallFlags::ALLOW_INTERACTIVE_AUTHORIZATION,
                -1,
            )
            .await
        }
        .await;
        if let Err(err) = result {
            glib::g_warning!("elavo-panel", "login1 {method}: {err}");
        }
    });
}

fn confirm(message: &'static str, accept: &'static str, then: fn()) {
    glib::spawn_future_local(async move {
        let dialog = gtk::AlertDialog::builder()
            .message(message)
            .buttons(["Cancel", accept])
            .cancel_button(0)
            .default_button(1)
            .modal(true)
            .build();
        if let Ok(1) = dialog.choose_future(None::<&gtk::Window>).await {
            then();
        }
    });
}
