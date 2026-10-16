use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};

const INTERFACE: &str = "org.gnome.desktop.interface";
const H24: &str = "%a %b %-e  %H:%M";
const H12: &str = "%a %b %-e  %-l:%M %p";

/// Date and time for every bar. It checks once a second instead of sleeping
/// to the next minute, because GLib timeouts use the monotonic clock and
/// would show a stale time after suspend or a timezone change.
pub struct Clock {
    labels: RefCell<Vec<glib::WeakRef<gtk::Label>>>,
    format: Cell<&'static str>,
    text: RefCell<String>,
    /// Keeps the `clock-format` change signal connected.
    _settings: Option<gio::Settings>,
}

fn locale_uses_24h() -> bool {
    glib::DateTime::from_local(2000, 1, 1, 13, 0, 0.0)
        .and_then(|t| t.format("%X"))
        .map(|s| s.contains("13"))
        .unwrap_or(true)
}

/// Where System Settings' 24-hour switch is stored, when the schema has it.
fn interface_settings() -> Option<gio::Settings> {
    let schema = gio::SettingsSchemaSource::default()?.lookup(INTERFACE, true)?;
    schema
        .has_key("clock-format")
        .then(|| gio::Settings::new(INTERFACE))
}

fn format_for(settings: Option<&gio::Settings>) -> &'static str {
    // The schema's default is 24h, so an unset key means "follow the
    // locale", not 24-hour.
    let chosen = settings
        .and_then(|s| s.user_value("clock-format"))
        .and_then(|v| v.str().map(str::to_owned));
    let h24 = match chosen {
        Some(v) => v != "12h",
        None => locale_uses_24h(),
    };
    if h24 {
        H24
    } else {
        H12
    }
}

impl Clock {
    pub fn new() -> Rc<Self> {
        let settings = interface_settings();
        let clock = Rc::new(Self {
            labels: RefCell::default(),
            format: Cell::new(format_for(settings.as_ref())),
            text: RefCell::default(),
            _settings: settings.clone(),
        });
        if let Some(settings) = &settings {
            let weak = Rc::downgrade(&clock);
            settings.connect_changed(Some("clock-format"), move |s, _| {
                if let Some(clock) = weak.upgrade() {
                    clock.format.set(format_for(Some(s)));
                    clock.tick();
                }
            });
        }
        clock.tick();
        let weak = Rc::downgrade(&clock);
        glib::timeout_add_seconds_local(1, move || match weak.upgrade() {
            Some(clock) => {
                clock.tick();
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        });
        clock
    }

    pub fn label(&self) -> gtk::Label {
        let label = gtk::Label::builder()
            .label(self.text.borrow().as_str())
            .css_classes(["elavo-panel-clock"])
            .build();
        let mut labels = self.labels.borrow_mut();
        labels.retain(|l| l.upgrade().is_some());
        labels.push(label.downgrade());
        label
    }

    fn tick(&self) {
        let Some(text) = glib::DateTime::now_local()
            .ok()
            .and_then(|now| now.format(self.format.get()).ok())
        else {
            return;
        };
        if *self.text.borrow() == text.as_str() {
            return;
        }
        self.text.replace(text.to_string());
        for label in self.labels.borrow().iter().filter_map(|l| l.upgrade()) {
            label.set_label(&text);
        }
    }
}
