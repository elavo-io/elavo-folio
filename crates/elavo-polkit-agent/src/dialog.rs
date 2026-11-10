//! The card. It floats over every window on an overlay layer surface,
//! centered on the output the compositor picks (labwc uses the focused
//! one), and holds the keyboard while it is up.
//!
//! Fingerprint: a later variant reads "Touch ID or enter your password"
//! with a "Use Password…" pill. It would be a second `Mode` here that hides
//! the password field and swaps the pills, switched on when the helper
//! relays pam_fprintd's prompt (see `agent::Auth::event`). Only the
//! password mode exists today.

use std::cell::{Cell, RefCell};
use std::f64::consts::TAU;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use gtk4_layer_shell::{KeyboardMode, Layer, LayerShell};

use crate::{identity, wording};

/// Room around the card for its shadow.
const MARGIN: i32 = 28;
const WIDTH: i32 = 300;
const SHAKE_SECONDS: f64 = 0.45;
const SHAKE_PIXELS: f64 = 12.0;
const SHAKE_TIMES: f64 = 4.0;
const GENERIC_ICON: &str = "preferences-system";
const PREVIEW_APP: &str = "System Settings";
const PREVIEW_DESKTOP_ID: &str = "io.elavo.SystemSettings.desktop";

pub struct Spec {
    pub title: String,
    pub body: String,
    /// The requesting action or application, drawn on the lock.
    pub badge: gio::Icon,
    /// The name the field starts with: the person at the keyboard when
    /// they can answer, else empty. Anyone who can answer may type theirs.
    pub name: String,
    pub action_id: String,
}

type Callback<T> = RefCell<Option<Box<dyn Fn(T)>>>;

pub struct Dialog {
    window: gtk::Window,
    card: gtk::Box,
    note: gtk::Label,
    name: gtk::Entry,
    password: gtk::PasswordEntry,
    unlock: gtk::Button,
    on_submit: Callback<(String, String)>,
    on_cancel: Callback<()>,
    shake: Rc<Cell<u32>>,
    closed: Cell<bool>,
}

impl Dialog {
    pub fn new(app: &gtk::Application, spec: &Spec) -> Rc<Self> {
        let window = gtk::Window::builder()
            .application(app)
            .title("Authenticate")
            .decorated(false)
            .resizable(false)
            .css_classes(["elavo-polkit-agent"])
            .build();
        if gtk4_layer_shell::is_supported() {
            window.init_layer_shell();
            window.set_namespace(Some("elavo-polkit-agent"));
            window.set_layer(Layer::Overlay);
            // No anchors: the compositor centers the card.
            window.set_keyboard_mode(KeyboardMode::Exclusive);
        }

        let card = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .css_classes(["elavo-auth-card"])
            .width_request(WIDTH)
            .margin_top(MARGIN)
            .margin_bottom(MARGIN)
            .margin_start(MARGIN)
            .margin_end(MARGIN)
            .build();

        card.append(&icon(&spec.badge, &spec.action_id));

        let title = gtk::Label::builder()
            .label(&spec.title)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .max_width_chars(30)
            .xalign(0.0)
            .halign(gtk::Align::Fill)
            .css_classes(["elavo-auth-title"])
            .margin_top(14)
            .build();
        let body = gtk::Label::builder()
            .label(&spec.body)
            .wrap(true)
            .max_width_chars(30)
            .xalign(0.0)
            .halign(gtk::Align::Fill)
            .css_classes(["elavo-auth-body"])
            .margin_top(6)
            .build();
        card.append(&title);
        card.append(&body);

        let fields = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .margin_top(16)
            .build();
        let name = gtk::Entry::builder()
            .text(&spec.name)
            .placeholder_text("Name")
            .hexpand(true)
            .build();
        fields.append(&name);
        let password = gtk::PasswordEntry::builder()
            .placeholder_text("Password")
            .show_peek_icon(false)
            .hexpand(true)
            .build();
        fields.append(&password);
        card.append(&fields);

        let note = gtk::Label::builder()
            .wrap(true)
            .max_width_chars(30)
            .xalign(0.0)
            .css_classes(["elavo-auth-note"])
            .margin_top(8)
            .visible(false)
            .build();
        card.append(&note);

        let buttons = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .margin_top(16)
            .build();
        let unlock = gtk::Button::builder()
            .label("Unlock")
            .css_classes(["elavo-auth-pill", "suggested-action"])
            .build();
        let cancel = gtk::Button::builder()
            .label("Cancel")
            .css_classes(["elavo-auth-pill"])
            .build();
        buttons.append(&unlock);
        buttons.append(&cancel);
        card.append(&buttons);

        window.set_child(Some(&card));

        let dialog = Rc::new(Self {
            window,
            card,
            note,
            name,
            password,
            unlock,
            on_submit: RefCell::default(),
            on_cancel: RefCell::default(),
            shake: Rc::default(),
            closed: Cell::new(false),
        });

        let weak = Rc::downgrade(&dialog);
        let submit = move || {
            if let Some(dialog) = weak.upgrade() {
                dialog.submit();
            }
        };
        let submit_click = submit.clone();
        dialog.unlock.connect_clicked(move |_| submit_click());
        dialog.password.connect_activate(move |_| submit());

        let weak = Rc::downgrade(&dialog);
        let cancel_now = move || {
            if let Some(dialog) = weak.upgrade() {
                dialog.cancel();
            }
        };
        let cancel_click = cancel_now.clone();
        cancel.connect_clicked(move |_| cancel_click());
        let keys = gtk::EventControllerKey::new();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Escape {
                cancel_now();
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        dialog.window.add_controller(keys);

        let weak = Rc::downgrade(&dialog);
        dialog.name.connect_activate(move |_| {
            if let Some(dialog) = weak.upgrade() {
                dialog.password.grab_focus();
            }
        });
        let weak = Rc::downgrade(&dialog);
        dialog.name.connect_changed(move |_| {
            if let Some(dialog) = weak.upgrade() {
                dialog.set_note(None, false);
            }
        });

        dialog.window.present();
        if spec.name.is_empty() {
            dialog.name.grab_focus();
        } else {
            dialog.password.grab_focus();
        }
        dialog
    }

    pub fn connect_submit(&self, f: impl Fn((String, String)) + 'static) {
        self.on_submit.replace(Some(Box::new(f)));
    }

    pub fn connect_cancel(&self, f: impl Fn(()) + 'static) {
        self.on_cancel.replace(Some(Box::new(f)));
    }

    fn submit(&self) {
        if !self.unlock.is_sensitive() || self.closed.get() {
            return;
        }
        let name = self.name.text().trim().to_owned();
        let password = self.password.text().to_string();
        if let Some(f) = self.on_submit.borrow().as_ref() {
            f((name, password));
        }
    }

    fn cancel(&self) {
        if self.closed.get() {
            return;
        }
        if let Some(f) = self.on_cancel.borrow().as_ref() {
            f(());
        }
    }

    /// While the password is being checked.
    pub fn set_busy(&self, busy: bool) {
        self.unlock.set_sensitive(!busy);
        self.name.set_sensitive(!busy);
        self.password.set_sensitive(!busy);
        if !busy {
            if self.name.text().trim().is_empty() {
                self.name.grab_focus();
            } else {
                self.password.grab_focus();
            }
        }
    }

    /// PAM's own question, when it is not the usual password prompt.
    pub fn prompt(&self, text: &str) {
        let text = text.trim().trim_end_matches(':').trim();
        let usual = text.is_empty() || text.to_lowercase().starts_with("password");
        self.password
            .set_placeholder_text(Some(if usual { "Password" } else { text }));
    }

    pub fn set_note(&self, text: Option<&str>, error: bool) {
        match text.map(str::trim).filter(|t| !t.is_empty()) {
            Some(text) => {
                self.note.set_label(text);
                if error {
                    self.note.add_css_class("error");
                } else {
                    self.note.remove_css_class("error");
                }
                self.note.set_visible(true);
            }
            None => self.note.set_visible(false),
        }
    }

    /// A wrong password: shake, clear the field, and say so.
    pub fn fail(&self, text: &str) {
        self.password.set_text("");
        self.set_note(Some(text), true);
        self.set_busy(false);
        self.shake();
    }

    fn shake(&self) {
        let generation = self.shake.get().wrapping_add(1);
        self.shake.set(generation);
        let current = self.shake.clone();
        let start: Cell<Option<i64>> = Cell::new(None);
        self.card.add_tick_callback(move |card, clock| {
            // A newer shake takes over the margins.
            if current.get() != generation {
                return glib::ControlFlow::Break;
            }
            let now = clock.frame_time();
            let t0 = match start.get() {
                Some(t0) => t0,
                None => {
                    start.set(Some(now));
                    now
                }
            };
            let t = (now - t0) as f64 / 1_000_000.0;
            if t >= SHAKE_SECONDS {
                card.set_margin_start(MARGIN);
                card.set_margin_end(MARGIN);
                return glib::ControlFlow::Break;
            }
            let progress = t / SHAKE_SECONDS;
            let dx = SHAKE_PIXELS * (1.0 - progress) * (progress * SHAKE_TIMES * TAU).sin();
            let dx = dx.round() as i32;
            card.set_margin_start(MARGIN + dx);
            card.set_margin_end(MARGIN - dx);
            glib::ControlFlow::Continue
        });
    }

    pub fn close(&self) {
        if self.closed.replace(true) {
            return;
        }
        self.password.set_text("");
        self.window.destroy();
    }
}

/// The lock in its circle, with the request's icon on its lower right.
fn icon(badge: &gio::Icon, action_id: &str) -> gtk::Widget {
    let lock = gtk::Image::from_gicon(&gio::ThemedIcon::from_names(&[
        "changes-prevent-symbolic",
        "system-lock-screen-symbolic",
        "dialog-password-symbolic",
    ]));
    lock.set_pixel_size(30);
    let circle = gtk::Box::builder()
        .css_classes(["elavo-auth-lock"])
        .halign(gtk::Align::Start)
        .valign(gtk::Align::Start)
        .build();
    lock.set_hexpand(true);
    lock.set_vexpand(true);
    circle.append(&lock);

    let small = gtk::Image::from_gicon(badge);
    small.set_pixel_size(30);
    small.add_css_class("elavo-auth-badge");
    small.set_halign(gtk::Align::End);
    small.set_valign(gtk::Align::End);

    let overlay = gtk::Overlay::builder()
        .child(&circle)
        .halign(gtk::Align::Start)
        .width_request(76)
        .height_request(72)
        .build();
    circle.set_halign(gtk::Align::Start);
    overlay.add_overlay(&small);
    if !action_id.is_empty() {
        overlay.set_tooltip_text(Some(action_id));
    }
    overlay.upcast()
}

/// The request's icon when the theme has it, then the application's, then
/// a generic one.
pub fn badge(icon_name: &str, app_icon: Option<gio::Icon>) -> gio::Icon {
    let theme = gdk::Display::default().map(|d| gtk::IconTheme::for_display(&d));
    if !icon_name.is_empty() && theme.as_ref().is_some_and(|t| t.has_icon(icon_name)) {
        return gio::ThemedIcon::new(icon_name).upcast();
    }
    app_icon.unwrap_or_else(|| gio::ThemedIcon::new(GENERIC_ICON).upcast())
}

/// `--preview`: sample text, no polkit. Unlock and Cancel close it.
pub fn preview(app: &gtk::Application) {
    let me = identity::Person::from_uid(unsafe { libc::getuid() });
    let app_icon = gio_unix::DesktopAppInfo::new(PREVIEW_DESKTOP_ID).and_then(|a| a.icon());
    let spec = Spec {
        title: wording::title("", Some(PREVIEW_APP)),
        body: wording::BODY.to_owned(),
        badge: badge("", app_icon),
        name: me.map(|p| p.display_name().to_owned()).unwrap_or_default(),
        action_id: "io.elavo.preview".to_owned(),
    };
    let dialog = Dialog::new(app, &spec);
    let weak = Rc::downgrade(&dialog);
    dialog.connect_submit(move |_| {
        if let Some(dialog) = weak.upgrade() {
            dialog.close();
        }
    });
    let weak = Rc::downgrade(&dialog);
    dialog.connect_cancel(move |()| {
        if let Some(dialog) = weak.upgrade() {
            dialog.close();
        }
    });
    // The window holds the application; the dialog lives as long as it.
    let keep = RefCell::new(Some(dialog.clone()));
    dialog.window.connect_destroy(move |_| {
        keep.take();
    });
}
