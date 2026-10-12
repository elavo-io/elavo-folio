use std::cell::RefCell;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;

/// Date and time for every bar. It checks once a second instead of sleeping
/// to the next minute, because GLib timeouts use the monotonic clock and
/// would show a stale time after suspend or a timezone change.
pub struct Clock {
    labels: RefCell<Vec<glib::WeakRef<gtk::Label>>>,
    format: &'static str,
    text: RefCell<String>,
}

fn locale_uses_24h() -> bool {
    glib::DateTime::from_local(2000, 1, 1, 13, 0, 0.0)
        .and_then(|t| t.format("%X"))
        .map(|s| s.contains("13"))
        .unwrap_or(true)
}

impl Clock {
    pub fn new() -> Rc<Self> {
        let clock = Rc::new(Self {
            labels: RefCell::default(),
            format: if locale_uses_24h() {
                "%a %b %-e  %H:%M"
            } else {
                "%a %b %-e  %-l:%M %p"
            },
            text: RefCell::default(),
        });
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
            .and_then(|now| now.format(self.format).ok())
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
