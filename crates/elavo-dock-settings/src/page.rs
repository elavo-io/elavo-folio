//! The Dock section of Desktop and Dock. Every control writes `dock.conf`
//! the moment it changes, and the page follows the file when something else
//! (the divider menu, a second settings window) changes it.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use crate::config::{Position, Settings, MAX_ICON_SIZE, MIN_ICON_SIZE};

const STYLE: &str = "
.elavo-dock-settings { padding: 18px 24px 24px; }
.elavo-dock-settings > label.heading { margin: 6px 4px 8px; }
.elavo-dock-settings list { border-radius: 10px; }
.elavo-dock-settings row { padding: 8px 12px; min-height: 32px; }
.elavo-dock-settings .caption { opacity: 0.6; font-size: smaller; }
";

struct Controls {
    size: gtk::Scale,
    magnification: gtk::Switch,
    magnified_size: gtk::Scale,
    positions: Vec<(Position, gtk::ToggleButton)>,
    display: gtk::DropDown,
    displays: gtk::StringList,
    minimize_to_application: gtk::Switch,
    autohide: gtk::Switch,
    animate_launch: gtk::Switch,
    show_indicators: gtk::Switch,
    show_recents: gtk::Switch,
    /// Set while controls are being moved to match the file, so their change
    /// handlers do not write it back.
    syncing: Cell<bool>,
    connectors: RefCell<Vec<Option<String>>>,
    monitor: RefCell<Option<gio::FileMonitor>>,
}

impl Controls {
    fn sync(&self, s: &Settings) {
        self.syncing.set(true);
        self.size.set_value(f64::from(s.icon_size));
        self.magnification.set_active(s.magnification);
        self.magnified_size.set_value(f64::from(s.magnified_size));
        self.magnified_size.set_sensitive(s.magnification);
        for (position, button) in &self.positions {
            button.set_active(*position == s.position);
        }
        self.minimize_to_application
            .set_active(s.minimize_to_application);
        self.autohide.set_active(s.autohide);
        self.animate_launch.set_active(s.animate_launch);
        self.show_indicators.set_active(s.show_indicators);
        self.show_recents.set_active(s.show_recents);
        self.sync_displays(s);
        self.syncing.set(false);
    }

    fn sync_displays(&self, s: &Settings) {
        let mut connectors: Vec<Option<String>> = vec![None];
        if let Some(display) = gdk::Display::default() {
            let monitors = display.monitors();
            for i in 0..monitors.n_items() {
                if let Some(monitor) = monitors.item(i).and_downcast::<gdk::Monitor>() {
                    if let Some(connector) = monitor.connector() {
                        connectors.push(Some(connector.to_string()));
                    }
                }
            }
        }
        if s.display.is_some() && !connectors.contains(&s.display) {
            connectors.push(s.display.clone());
        }
        let labels: Vec<String> = connectors
            .iter()
            .map(|c| match c {
                None => "All Displays".to_owned(),
                Some(c) => display_label(c),
            })
            .collect();
        let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        self.displays.splice(0, self.displays.n_items(), &refs);
        let selected = connectors.iter().position(|c| *c == s.display).unwrap_or(0);
        self.display.set_selected(selected as u32);
        self.connectors.replace(connectors);
    }

    fn write(&self, f: impl FnOnce(&mut Settings)) {
        if !self.syncing.get() {
            Settings::update(f);
        }
    }
}

fn display_label(connector: &str) -> String {
    let display = gdk::Display::default();
    let model = display.and_then(|d| {
        let monitors = d.monitors();
        (0..monitors.n_items())
            .filter_map(|i| monitors.item(i).and_downcast::<gdk::Monitor>())
            .find(|m| m.connector().as_deref() == Some(connector))
            .and_then(|m| m.description().or_else(|| m.model()))
    });
    match model {
        Some(model) if model != connector => format!("{model} ({connector})"),
        _ => connector.to_owned(),
    }
}

fn row(title: &str, control: &impl IsA<gtk::Widget>) -> gtk::ListBoxRow {
    let label = gtk::Label::builder()
        .label(title)
        .xalign(0.0)
        .hexpand(true)
        .wrap(true)
        .build();
    let content = gtk::Box::builder().spacing(12).build();
    content.append(&label);
    control.set_valign(gtk::Align::Center);
    content.append(control);
    gtk::ListBoxRow::builder()
        .activatable(false)
        .selectable(false)
        .child(&content)
        .build()
}

fn slider_row(title: &str, scale: &gtk::Scale, low: &str, high: &str) -> gtk::ListBoxRow {
    scale.set_hexpand(true);
    scale.set_draw_value(false);
    scale.set_width_request(220);
    let caption = |text: &str| {
        gtk::Label::builder()
            .label(text)
            .css_classes(["caption"])
            .build()
    };
    let slider = gtk::Box::builder().spacing(8).build();
    slider.append(&caption(low));
    slider.append(scale);
    slider.append(&caption(high));
    row(title, &slider)
}

fn group(rows: &[gtk::ListBoxRow]) -> gtk::Frame {
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["rich-list"])
        .build();
    for row in rows {
        list.append(row);
    }
    gtk::Frame::builder().child(&list).build()
}

fn size_scale() -> gtk::Scale {
    let scale = gtk::Scale::with_range(
        gtk::Orientation::Horizontal,
        f64::from(MIN_ICON_SIZE),
        f64::from(MAX_ICON_SIZE),
        1.0,
    );
    scale.set_round_digits(0);
    scale
}

/// The page, ready to put in a window or a settings sidebar. It loads the
/// current settings and stays in step with the file while it exists.
pub fn page() -> gtk::Widget {
    if let Some(display) = gdk::Display::default() {
        let css = gtk::CssProvider::new();
        css.load_from_string(STYLE);
        gtk::style_context_add_provider_for_display(
            &display,
            &css,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }

    let first = gtk::ToggleButton::with_label(Position::Left.label());
    let positions: Vec<(Position, gtk::ToggleButton)> = Position::ALL
        .into_iter()
        .map(|p| {
            if p == Position::Left {
                (p, first.clone())
            } else {
                let button = gtk::ToggleButton::with_label(p.label());
                button.set_group(Some(&first));
                (p, button)
            }
        })
        .collect();
    let displays = gtk::StringList::new(&[]);

    let controls = Rc::new(Controls {
        size: size_scale(),
        magnification: gtk::Switch::new(),
        magnified_size: size_scale(),
        positions,
        display: gtk::DropDown::builder().model(&displays).build(),
        displays,
        minimize_to_application: gtk::Switch::new(),
        autohide: gtk::Switch::new(),
        animate_launch: gtk::Switch::new(),
        show_indicators: gtk::Switch::new(),
        show_recents: gtk::Switch::new(),
        syncing: Cell::new(false),
        connectors: RefCell::default(),
        monitor: RefCell::default(),
    });
    controls.sync(&Settings::load());

    let c = Rc::downgrade(&controls);
    controls.size.connect_value_changed(move |scale| {
        if let Some(c) = c.upgrade() {
            c.write(|s| s.icon_size = scale.value().round() as i32);
        }
    });
    let c = Rc::downgrade(&controls);
    controls.magnified_size.connect_value_changed(move |scale| {
        if let Some(c) = c.upgrade() {
            c.write(|s| s.magnified_size = scale.value().round() as i32);
        }
    });
    type Set = fn(&mut Settings, bool);
    let switches: [(&gtk::Switch, Set); 6] = [
        (&controls.magnification, |s, v| s.magnification = v),
        (&controls.minimize_to_application, |s, v| {
            s.minimize_to_application = v
        }),
        (&controls.autohide, |s, v| s.autohide = v),
        (&controls.animate_launch, |s, v| s.animate_launch = v),
        (&controls.show_indicators, |s, v| s.show_indicators = v),
        (&controls.show_recents, |s, v| s.show_recents = v),
    ];
    for (switch, set) in switches {
        let c = Rc::downgrade(&controls);
        switch.connect_active_notify(move |switch| {
            if let Some(c) = c.upgrade() {
                let active = switch.is_active();
                c.write(|s| set(s, active));
                if switch == &c.magnification {
                    c.magnified_size.set_sensitive(active);
                }
            }
        });
    }
    for (position, button) in &controls.positions {
        let c = Rc::downgrade(&controls);
        let position = *position;
        button.connect_toggled(move |button| {
            if let (Some(c), true) = (c.upgrade(), button.is_active()) {
                c.write(|s| s.position = position);
            }
        });
    }
    let c = Rc::downgrade(&controls);
    controls.display.connect_selected_notify(move |dropdown| {
        let Some(c) = c.upgrade() else {
            return;
        };
        let choice = c
            .connectors
            .borrow()
            .get(dropdown.selected() as usize)
            .cloned();
        if let Some(choice) = choice {
            c.write(|s| s.display = choice);
        }
    });
    if let Some(display) = gdk::Display::default() {
        let c = Rc::downgrade(&controls);
        display.monitors().connect_items_changed(move |_, _, _, _| {
            if let Some(c) = c.upgrade() {
                c.syncing.set(true);
                c.sync_displays(&Settings::load());
                c.syncing.set(false);
            }
        });
    }
    let c = Rc::downgrade(&controls);
    controls.monitor.replace(Settings::watch(move |s| {
        if let Some(c) = c.upgrade() {
            c.sync(s);
        }
    }));

    let position = gtk::Box::builder().css_classes(["linked"]).build();
    for (_, button) in &controls.positions {
        position.append(button);
    }

    let page = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .css_classes(["elavo-dock-settings"])
        .build();
    page.append(
        &gtk::Label::builder()
            .label("Dock")
            .xalign(0.0)
            .css_classes(["heading"])
            .build(),
    );
    page.append(&group(&[
        slider_row("Size", &controls.size, "Small", "Large"),
        row("Magnification", &controls.magnification),
        slider_row("Magnified size", &controls.magnified_size, "Small", "Large"),
        row("Position on screen", &position),
        row("Show Dock on", &controls.display),
    ]));
    page.append(&group(&[
        row(
            "Minimize windows into application icon",
            &controls.minimize_to_application,
        ),
        row("Automatically hide and show the Dock", &controls.autohide),
        row("Animate opening applications", &controls.animate_launch),
        row(
            "Show indicators for open applications",
            &controls.show_indicators,
        ),
        row(
            "Show suggested and recent apps in Dock",
            &controls.show_recents,
        ),
    ]));

    // The page owns the controls through this, so the watcher lives exactly
    // as long as the page.
    let keep = RefCell::new(Some(controls));
    page.connect_destroy(move |_| {
        keep.take();
    });
    page.upcast()
}

/// A standalone Dock Settings window around [`page`]. Until the settings
/// application exists, the dock opens this.
pub fn window(app: &gtk::Application) -> gtk::Window {
    let scrolled = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .propagate_natural_height(true)
        .child(&page())
        .build();
    let window = gtk::Window::builder()
        .application(app)
        .title("Dock Settings")
        .default_width(560)
        .resizable(true)
        .child(&scrolled)
        .build();
    let close = gtk::EventControllerKey::new();
    let weak = window.downgrade();
    close.connect_key_pressed(move |_, key, _, modifiers| {
        let escape = key == gdk::Key::Escape;
        let ctrl_w = modifiers.contains(gdk::ModifierType::CONTROL_MASK) && key == gdk::Key::w;
        if let (true, Some(window)) = (escape || ctrl_w, weak.upgrade()) {
            window.close();
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    window.add_controller(close);
    window
}
