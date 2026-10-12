//! The Elavo menu behind the sun: About, Settings, power, lock, and log out.

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
    add("about", show_about);
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

    let settings = gio::SimpleAction::new("settings", None);
    // Enabled when elavo-settings exists (phase 3).
    settings.set_enabled(false);
    app.add_action(&settings);
}

fn model() -> gio::Menu {
    let menu = gio::Menu::new();

    let about = gio::Menu::new();
    about.append(Some("About Elavo"), Some("app.about"));
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

fn os_release() -> Vec<(String, String)> {
    let text = std::fs::read_to_string("/etc/os-release")
        .or_else(|_| std::fs::read_to_string("/usr/lib/os-release"))
        .unwrap_or_default();
    text.lines()
        .filter_map(|line| line.split_once('='))
        .map(|(k, v)| (k.trim().to_owned(), v.trim().trim_matches('"').to_owned()))
        .collect()
}

fn show_about() {
    let release = os_release();
    let get = |key: &str| {
        release
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    };
    let dialog = gtk::AboutDialog::builder()
        .program_name("Elavo")
        .logo_icon_name("elavo-sun-symbolic")
        .version(get("VERSION"))
        .comments(get("PRETTY_NAME"))
        .modal(false)
        .build();
    dialog.present();
}
