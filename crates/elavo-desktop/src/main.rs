//! Icons for `~/Desktop` and mounted volumes, drawn on the wallpaper.

mod area;
mod desktop;
mod model;
mod thumbs;

use std::cell::Cell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};

fn main() -> glib::ExitCode {
    gio::resources_register_include!("elavo-desktop.gresource").expect("desktop resources");

    let app = gtk::Application::builder()
        .application_id("io.elavo.Desktop")
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();

    let started = Rc::new(Cell::new(false));
    app.connect_command_line(move |app, _| {
        if !started.replace(true) {
            // The process has no window that should keep it alive on its own
            // while every monitor surface is being rebuilt.
            std::mem::forget(app.hold());
            desktop::Desktop::start(app);
        }
        glib::ExitCode::SUCCESS
    });
    app.run()
}
