//! The administrator password prompt: Elavo's polkit authentication agent.
//! It registers with the polkit authority for the person's session, and
//! when an application asks for an administrator, it shows one card over
//! everything and checks the password with polkit's own setuid helper.
//! The helper answers polkitd itself, so the password never leaves this
//! process and the helper's pipe.

mod agent;
mod caller;
mod dialog;
mod helper;
mod identity;
mod wording;

use std::cell::{OnceCell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};

const APP_ID: &str = "io.elavo.PolkitAgent";
const PREVIEW_APP_ID: &str = "io.elavo.PolkitAgentPreview";
const STYLE: &str = include_str!("style.css");

/// `--preview` shows the card with sample text and does not touch polkit,
/// so a build can be looked at while the session's agent keeps running.
/// Unlock and Cancel just close it.
fn preview_mode() -> bool {
    std::env::args().any(|arg| arg == "--preview")
}

fn load_style() {
    let Some(display) = gdk::Display::default() else {
        return;
    };
    let css = gtk::CssProvider::new();
    css.load_from_string(STYLE);
    gtk::style_context_add_provider_for_display(
        &display,
        &css,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

/// `org.gnome.desktop.interface` `color-scheme`. Only `prefer-dark` is dark.
fn scheme_is_dark(value: &str) -> bool {
    value == "prefer-dark"
}

fn interface_settings() -> Option<gio::Settings> {
    let source = gio::SettingsSchemaSource::default()?;
    let schema = source.lookup("org.gnome.desktop.interface", true)?;
    schema
        .has_key("color-scheme")
        .then(|| gio::Settings::new("org.gnome.desktop.interface"))
}

fn apply_scheme(desktop: &gio::Settings) {
    if let Some(settings) = gtk::Settings::default() {
        settings.set_gtk_application_prefer_dark_theme(scheme_is_dark(
            desktop.string("color-scheme").as_str(),
        ));
    }
}

/// The Elavo theme's dark variant follows the color scheme, as the
/// panel's menus do.
fn follow_color_scheme() {
    let Some(desktop) = interface_settings() else {
        return;
    };
    apply_scheme(&desktop);
    desktop.connect_changed(Some("color-scheme"), |desktop, _| apply_scheme(desktop));
    thread_local! {
        static HELD: RefCell<Option<gio::Settings>> = const { RefCell::new(None) };
    }
    HELD.with(|slot| slot.replace(Some(desktop)));
}

fn main() -> glib::ExitCode {
    // A helper that exits early must not take the agent down with it when
    // the agent writes the next answer.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }
    let preview = preview_mode();
    let app = gtk::Application::builder()
        .application_id(if preview { PREVIEW_APP_ID } else { APP_ID })
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();

    let started: Rc<OnceCell<()>> = Rc::default();
    app.connect_command_line(move |app, _| {
        started.get_or_init(|| {
            load_style();
            follow_color_scheme();
            if preview {
                dialog::preview(app);
            } else {
                // The agent waits with no window between requests.
                std::mem::forget(app.hold());
                agent::start(app);
            }
        });
        glib::ExitCode::SUCCESS
    });
    app.run()
}

#[cfg(test)]
mod tests {
    #[test]
    fn only_prefer_dark_is_dark() {
        assert!(super::scheme_is_dark("prefer-dark"));
        assert!(!super::scheme_is_dark("default"));
        assert!(!super::scheme_is_dark("prefer-light"));
    }
}
