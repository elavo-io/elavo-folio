//! The screen the login window and the lock screen share: the wallpaper,
//! the bar, the glass clock, the account, and at login the power buttons.
//!
//! Every output gets a view with the wallpaper, the bar, and the clock. The
//! first view also holds the account and takes the keyboard.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gdk, gio, glib, pango};

use crate::accounts::{self, Account};
use crate::config::Show;
use crate::glass::GlassClock;
use crate::greetd::{self, Failure};
use crate::logind::{self, Power};
use crate::pam;
use crate::status::Status;
use crate::wallpaper::{self, Picture};

const BAR: i32 = 28;
const AVATAR: i32 = 96;
const AVATAR_OTHER: i32 = 64;
const INTERFACE: &str = "org.gnome.desktop.interface";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Login,
    Lock,
}

/// Who checks the password.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    Greetd,
    Pam,
    /// For trying the screen in a session: the password is "elavo".
    Demo,
}

struct View {
    picture: gtk::Picture,
    bar: gtk::CenterBox,
    date: gtk::Label,
    clock: GlassClock,
}

pub struct Screen {
    kind: Kind,
    backend: Backend,
    show: Show,
    accounts: Vec<Account>,
    selected: Cell<usize>,
    status: Rc<Status>,
    views: RefCell<Vec<View>>,
    pictures: RefCell<HashMap<PathBuf, Picture>>,
    shown: RefCell<Option<PathBuf>>,
    aspect: Cell<f64>,
    h24: Cell<bool>,
    _settings: Option<gio::Settings>,
    people: gtk::Box,
    password_block: gtk::Box,
    name: gtk::Entry,
    password: gtk::Entry,
    message: gtk::Label,
    stack: gtk::Stack,
    power_row: gtk::Box,
    confirm_title: gtk::Label,
    confirm_button: gtk::Button,
    pending: Cell<Option<Power>>,
    on_unlock: RefCell<Option<Box<dyn Fn()>>>,
}

fn interface_settings() -> Option<gio::Settings> {
    let schema = gio::SettingsSchemaSource::default()?.lookup(INTERFACE, true)?;
    schema
        .has_key("clock-format")
        .then(|| gio::Settings::new(INTERFACE))
}

/// System Settings' 24-hour switch when this account has set it, otherwise
/// the locale.
fn uses_24h(settings: Option<&gio::Settings>) -> bool {
    let chosen = settings
        .and_then(|s| s.user_value("clock-format"))
        .and_then(|v| v.str().map(str::to_owned));
    match chosen {
        Some(v) => v != "12h",
        None => glib::DateTime::from_local(2000, 1, 1, 13, 0, 0.0)
            .and_then(|t| t.format("%X"))
            .map(|s| s.contains("13"))
            .unwrap_or(true),
    }
}

fn password_entry() -> gtk::Entry {
    let entry = gtk::Entry::builder()
        .visibility(false)
        .input_purpose(gtk::InputPurpose::Password)
        .placeholder_text("Enter Password")
        .secondary_icon_name("go-next-symbolic")
        .secondary_icon_activatable(true)
        .secondary_icon_tooltip_text("Sign In")
        .width_request(240)
        .halign(gtk::Align::Center)
        .css_classes(["elavo-greeter-field"])
        .build();
    entry.set_secondary_icon_sensitive(false);
    entry.connect_changed(|e| e.set_secondary_icon_sensitive(!e.text().is_empty()));
    entry
}

fn avatar(account: &Account, size: i32) -> gtk::Widget {
    let frame = gtk::Box::builder()
        .width_request(size)
        .height_request(size)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .overflow(gtk::Overflow::Hidden)
        .css_classes(["elavo-greeter-avatar"])
        .build();
    match account.icon.as_ref() {
        Some(icon) => {
            let picture = gtk::Picture::for_filename(icon);
            picture.set_content_fit(gtk::ContentFit::Cover);
            picture.set_size_request(size, size);
            picture.set_hexpand(true);
            frame.append(&picture);
        }
        None => {
            let label = gtk::Label::builder()
                .label(account.initial())
                .hexpand(true)
                .css_classes(["elavo-greeter-initial"])
                .build();
            let attrs = pango::AttrList::new();
            attrs.insert(pango::AttrSize::new_size_absolute(
                (f64::from(size) * 0.42 * f64::from(pango::SCALE)) as i32,
            ));
            label.set_attributes(Some(&attrs));
            frame.append(&label);
        }
    }
    frame.upcast()
}

fn power_button(icon: &str, label: &str) -> gtk::Button {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let circle = gtk::Box::builder()
        .halign(gtk::Align::Center)
        .css_classes(["elavo-greeter-power-circle"])
        .build();
    circle.append(
        &gtk::Image::builder()
            .icon_name(icon)
            .pixel_size(18)
            .hexpand(true)
            .build(),
    );
    column.append(&circle);
    column.append(&gtk::Label::new(Some(label)));
    gtk::Button::builder()
        .child(&column)
        .css_classes(["elavo-greeter-power"])
        .build()
}

impl Screen {
    pub fn new(kind: Kind, backend: Backend, show: Show, demo_people: usize) -> Rc<Self> {
        let (accounts, selected) = match kind {
            Kind::Lock => (vec![accounts::current()], 0),
            Kind::Login => {
                let mut list = accounts::list();
                list.extend(accounts::demo_people(demo_people));
                let selected = accounts::last_user(&list).unwrap_or(0);
                (list, selected)
            }
        };
        let show = if kind == Kind::Lock { Show::List } else { show };
        let settings = interface_settings();

        let message = gtk::Label::builder()
            .visible(false)
            .wrap(true)
            .max_width_chars(36)
            .justify(gtk::Justification::Center)
            .css_classes(["elavo-greeter-message"])
            .build();
        let password = password_entry();
        let name = gtk::Entry::builder()
            .placeholder_text("Name")
            .width_request(240)
            .halign(gtk::Align::Center)
            .css_classes(["elavo-greeter-field"])
            .build();

        // Reserved for the fingerprint prompt. Nothing shows it yet.
        let fingerprint = gtk::Box::builder()
            .spacing(6)
            .halign(gtk::Align::Center)
            .visible(false)
            .css_classes(["elavo-greeter-fingerprint"])
            .build();
        fingerprint.append(&gtk::Image::from_icon_name("auth-fingerprint-symbolic"));
        fingerprint.append(&gtk::Label::new(Some("Touch the fingerprint sensor")));

        let password_block = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(10)
            .halign(gtk::Align::Center)
            .build();
        password_block.append(&fingerprint);
        if show == Show::Name {
            password_block.append(&name);
        }
        password_block.append(&password);
        password_block.append(&message);

        let people = gtk::Box::builder()
            .spacing(if accounts.len() > 5 { 28 } else { 40 })
            .halign(gtk::Align::Center)
            .margin_start(24)
            .margin_end(24)
            .build();
        // A row wider than the screen scrolls sideways instead of growing
        // the window. Focus follows the password field, so the selected
        // person stays in view.
        let people_scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::External)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_width(true)
            .propagate_natural_height(true)
            .halign(gtk::Align::Center)
            .child(&people)
            .build();

        let confirm_title = gtk::Label::builder()
            .css_classes(["elavo-greeter-confirm-title"])
            .build();
        let confirm_button = gtk::Button::builder()
            .css_classes(["elavo-greeter-pill", "suggested"])
            .build();
        let cancel = gtk::Button::builder()
            .label("Cancel")
            .css_classes(["elavo-greeter-pill"])
            .build();
        let confirm_buttons = gtk::Box::builder()
            .spacing(12)
            .halign(gtk::Align::Center)
            .build();
        confirm_buttons.append(&cancel);
        confirm_buttons.append(&confirm_button);
        let confirm = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(16)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::End)
            .css_classes(["elavo-greeter-card"])
            .build();
        confirm.append(&confirm_title);
        confirm.append(&confirm_buttons);

        let stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .vhomogeneous(false)
            .build();
        let accounts_page = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .valign(gtk::Align::End)
            .build();
        if show == Show::Name {
            accounts_page.append(&password_block);
        } else {
            accounts_page.append(&people_scroller);
        }
        stack.add_named(&accounts_page, Some("accounts"));
        stack.add_named(&confirm, Some("confirm"));

        let power_row = gtk::Box::builder()
            .spacing(36)
            .halign(gtk::Align::Center)
            .visible(kind == Kind::Login)
            .css_classes(["elavo-greeter-power-row"])
            .build();

        let screen = Rc::new(Self {
            kind,
            backend,
            show,
            accounts,
            selected: Cell::new(selected),
            status: Status::new(kind == Kind::Lock),
            views: RefCell::default(),
            pictures: RefCell::default(),
            shown: RefCell::default(),
            aspect: Cell::new(16.0 / 10.0),
            h24: Cell::new(uses_24h(settings.as_ref())),
            _settings: settings.clone(),
            people,
            password_block,
            name,
            password,
            message,
            stack,
            power_row,
            confirm_title,
            confirm_button,
            pending: Cell::new(None),
            on_unlock: RefCell::default(),
        });

        for (icon, label, action) in [
            ("weather-clear-night-symbolic", "Sleep", Power::Sleep),
            ("system-reboot-symbolic", "Restart", Power::Restart),
            ("system-shutdown-symbolic", "Shut Down", Power::ShutDown),
        ] {
            let button = power_button(icon, label);
            let weak = Rc::downgrade(&screen);
            button.connect_clicked(move |_| {
                if let Some(screen) = weak.upgrade() {
                    screen.ask(action);
                }
            });
            screen.power_row.append(&button);
        }

        let weak = Rc::downgrade(&screen);
        cancel.connect_clicked(move |_| {
            if let Some(screen) = weak.upgrade() {
                screen.dismiss();
            }
        });
        let weak = Rc::downgrade(&screen);
        screen.confirm_button.connect_clicked(move |_| {
            if let Some(screen) = weak.upgrade() {
                if let Some(action) = screen.pending.take() {
                    logind::power(action);
                }
                screen.dismiss();
            }
        });

        let weak = Rc::downgrade(&screen);
        screen.password.connect_activate(move |_| {
            if let Some(screen) = weak.upgrade() {
                screen.submit();
            }
        });
        let weak = Rc::downgrade(&screen);
        screen.password.connect_icon_press(move |_, position| {
            if position == gtk::EntryIconPosition::Secondary {
                if let Some(screen) = weak.upgrade() {
                    screen.submit();
                }
            }
        });
        let message = screen.message.clone();
        screen
            .password
            .connect_changed(move |_| message.set_visible(false));
        let password = screen.password.clone();
        screen.name.connect_activate(move |_| {
            password.grab_focus();
        });

        let keys = gtk::EventControllerKey::new();
        let entry = screen.password.clone();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Escape {
                entry.set_text("");
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        screen.password.add_controller(keys);

        if let Some(settings) = &settings {
            let weak = Rc::downgrade(&screen);
            settings.connect_changed(Some("clock-format"), move |s, _| {
                if let Some(screen) = weak.upgrade() {
                    screen.h24.set(uses_24h(Some(s)));
                    screen.tick();
                }
            });
        }

        screen.rebuild_people();
        Self::start_clock(Rc::downgrade(&screen));
        screen
    }

    /// Runs after a successful unlock.
    pub fn connect_unlock(&self, f: impl Fn() + 'static) {
        self.on_unlock.replace(Some(Box::new(f)));
    }

    fn start_clock(weak: Weak<Self>) {
        glib::timeout_add_seconds_local(1, move || match weak.upgrade() {
            Some(screen) => {
                screen.tick();
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        });
    }

    fn tick(&self) {
        let Ok(now) = glib::DateTime::now_local() else {
            return;
        };
        let big = if self.h24.get() { "%H:%M" } else { "%-l:%M" };
        let big = now.format(big).map(|s| s.to_string()).unwrap_or_default();
        let date = now
            .format("%A, %B %-e")
            .map(|s| s.to_string())
            .unwrap_or_default();
        for view in self.views.borrow().iter() {
            view.clock.set_text(&big);
            if view.date.label() != date {
                view.date.set_label(&date);
            }
        }
    }

    /// The content for one output. The first one asked for holds the
    /// account.
    pub fn view(self: &Rc<Self>, monitor: Option<&gdk::Monitor>) -> gtk::Widget {
        if let Some(monitor) = monitor {
            if self.views.borrow().is_empty() {
                let geometry = monitor.geometry();
                if geometry.height() > 0 {
                    self.aspect
                        .set(f64::from(geometry.width()) / f64::from(geometry.height()));
                }
            }
        }
        let primary = self.views.borrow().is_empty();

        let picture = gtk::Picture::builder()
            .content_fit(gtk::ContentFit::Cover)
            .can_shrink(true)
            .hexpand(true)
            .vexpand(true)
            .build();

        let sun = gtk::Image::builder()
            .icon_name("elavo-sun-symbolic")
            .pixel_size(16)
            .css_classes(["elavo-greeter-sun"])
            .build();
        let end = gtk::Box::builder().spacing(12).build();
        self.status.append_to(&end);
        let bar = gtk::CenterBox::builder()
            .css_classes(["elavo-greeter-bar", "on-dark"])
            .height_request(BAR)
            .start_widget(&sun)
            .end_widget(&end)
            .build();

        let date = gtk::Label::builder()
            .css_classes(["elavo-greeter-date"])
            .build();
        let clock = GlassClock::new();
        clock.set_halign(gtk::Align::Center);

        let column = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        column.append(&bar);
        column.append(&date);
        column.append(&clock);
        let spacer = gtk::Box::builder().vexpand(true).build();
        column.append(&spacer);
        if primary {
            column.append(&self.stack);
            column.append(&self.power_row);
            let focus = if self.show == Show::Name {
                self.name.clone()
            } else {
                self.password.clone()
            };
            column.connect_map(move |_| {
                focus.grab_focus();
            });
        }
        let bottom = gtk::Box::builder()
            .height_request(if self.kind == Kind::Login { 32 } else { 72 })
            .build();
        column.append(&bottom);

        let root = gtk::Overlay::builder()
            .css_classes(["elavo-greeter"])
            .child(&picture)
            .build();
        root.add_overlay(&column);

        self.views.borrow_mut().push(View {
            picture,
            bar,
            date,
            clock,
        });
        self.tick();
        let shown = self.shown.borrow().clone();
        match shown.and_then(|p| self.pictures.borrow().get(&p).cloned()) {
            Some(picture) => self.apply(&picture),
            None => self.show_wallpaper(),
        }
        root.upcast()
    }

    fn selected_account(&self) -> Option<&Account> {
        self.accounts.get(self.selected.get())
    }

    fn wallpaper_path(&self) -> Option<PathBuf> {
        match (self.kind, self.backend, self.show) {
            (Kind::Lock, _, _) => wallpaper::session(),
            (_, _, Show::Name) => wallpaper::default(),
            (_, Backend::Demo, _) => {
                let account = self.selected_account()?;
                if account.uid == unsafe { libc::getuid() } {
                    wallpaper::session()
                } else {
                    wallpaper::for_uid(account.uid)
                }
            }
            _ => wallpaper::for_uid(self.selected_account()?.uid),
        }
    }

    fn show_wallpaper(self: &Rc<Self>) {
        let Some(path) = self.wallpaper_path() else {
            return;
        };
        self.shown.replace(Some(path.clone()));
        if let Some(picture) = self.pictures.borrow().get(&path).cloned() {
            self.apply(&picture);
            return;
        }
        let weak = Rc::downgrade(self);
        let aspect = self.aspect.get();
        let share = f64::from(BAR) / 900.0;
        glib::spawn_future_local(async move {
            let Some(picture) = wallpaper::load(path.clone(), aspect, share).await else {
                return;
            };
            let Some(screen) = weak.upgrade() else {
                return;
            };
            screen
                .pictures
                .borrow_mut()
                .insert(path.clone(), picture.clone());
            if screen.shown.borrow().as_ref() == Some(&path) {
                screen.apply(&picture);
            }
        });
    }

    fn apply(&self, picture: &Picture) {
        for view in self.views.borrow().iter() {
            view.picture.set_paintable(Some(&picture.texture));
            view.clock.set_texture(Some(&picture.texture));
            for ink in [wallpaper::Ink::White, wallpaper::Ink::Black] {
                view.bar.remove_css_class(ink.css_class());
            }
            view.bar.add_css_class(picture.ink.css_class());
        }
    }

    /// The row of people: the selected one large, with the password field
    /// under the name, the others smaller beside it.
    fn rebuild_people(self: &Rc<Self>) {
        if self.show == Show::Name {
            return;
        }
        while let Some(child) = self.people.first_child() {
            if let Some(column) = child.downcast_ref::<gtk::Box>() {
                if self.password_block.parent().as_ref() == Some(column.upcast_ref()) {
                    column.remove(&self.password_block);
                }
            }
            self.people.remove(&child);
        }
        for (i, account) in self.accounts.iter().enumerate() {
            let selected = i == self.selected.get();
            let size = if selected { AVATAR } else { AVATAR_OTHER };
            let column = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(14)
                .valign(gtk::Align::Start)
                .margin_top(if selected {
                    0
                } else {
                    (AVATAR - AVATAR_OTHER) / 2
                })
                .build();
            let person = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(10)
                .build();
            person.append(&avatar(account, size));
            person.append(
                &gtk::Label::builder()
                    .label(account.display_name())
                    .ellipsize(pango::EllipsizeMode::End)
                    .max_width_chars(if selected { 24 } else { 17 })
                    .css_classes([if selected {
                        "elavo-greeter-name"
                    } else {
                        "elavo-greeter-name-other"
                    }])
                    .build(),
            );
            if selected {
                column.append(&person);
                column.append(&self.password_block);
            } else {
                let button = gtk::Button::builder()
                    .child(&person)
                    .css_classes(["elavo-greeter-person"])
                    .tooltip_text(account.display_name())
                    .build();
                let weak = Rc::downgrade(self);
                button.connect_clicked(move |_| {
                    if let Some(screen) = weak.upgrade() {
                        screen.select(i);
                    }
                });
                column.append(&button);
            }
            self.people.append(&column);
        }
    }

    fn select(self: &Rc<Self>, index: usize) {
        if index == self.selected.get() || index >= self.accounts.len() {
            return;
        }
        self.selected.set(index);
        self.password.set_text("");
        self.message.set_visible(false);
        self.rebuild_people();
        self.show_wallpaper();
        self.password.grab_focus();
    }

    fn ask(&self, action: Power) {
        if action == Power::Sleep {
            logind::power(action);
            return;
        }
        let (title, button) = match action {
            Power::Restart => ("Restart the computer now?", "Restart"),
            _ => ("Shut down the computer now?", "Shut Down"),
        };
        self.confirm_title.set_label(title);
        self.confirm_button.set_label(button);
        self.pending.set(Some(action));
        self.power_row.set_visible(false);
        self.stack.set_visible_child_name("confirm");
        self.confirm_button.grab_focus();
    }

    fn dismiss(&self) {
        self.pending.set(None);
        self.stack.set_visible_child_name("accounts");
        self.power_row.set_visible(self.kind == Kind::Login);
        if self.show == Show::Name && self.name.text().is_empty() {
            self.name.grab_focus();
        } else {
            self.password.grab_focus();
        }
    }

    fn set_busy(&self, busy: bool) {
        self.password.set_sensitive(!busy);
        self.name.set_sensitive(!busy);
    }

    fn submit(self: &Rc<Self>) {
        if !self.password.is_sensitive() {
            return;
        }
        let user = match self.show {
            Show::Name => self.name.text().trim().to_owned(),
            Show::List => match self.selected_account() {
                Some(account) => account.name.clone(),
                None => return,
            },
        };
        if user.is_empty() {
            self.name.grab_focus();
            return;
        }
        let password = self.password.text().to_string();
        self.set_busy(true);
        let backend = self.backend;
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let who = user.clone();
            let result = gio::spawn_blocking(move || match backend {
                Backend::Greetd => greetd::login(&who, &password, &greetd::session_command()),
                Backend::Pam => match pam::check(&who, &password) {
                    Ok(true) => Ok(()),
                    Ok(false) => Err(Failure::Auth),
                    Err(e) => Err(Failure::Other(e)),
                },
                Backend::Demo => {
                    std::thread::sleep(Duration::from_millis(300));
                    if password == "elavo" {
                        Ok(())
                    } else {
                        Err(Failure::Auth)
                    }
                }
            })
            .await
            .unwrap_or_else(|_| Err(Failure::Other("the check stopped".into())));
            if let Some(screen) = weak.upgrade() {
                screen.finish(result, &user);
            }
        });
    }

    fn finish(&self, result: Result<(), Failure>, user: &str) {
        match result {
            Ok(()) => match (self.kind, self.backend) {
                (Kind::Lock, _) => {
                    if let Some(f) = self.on_unlock.borrow().as_ref() {
                        f();
                    }
                }
                (Kind::Login, Backend::Greetd) => {
                    accounts::remember(user);
                    // greetd starts the session once the greeter is gone.
                    // Exiting here also skips finalizing the window.
                    std::process::exit(0);
                }
                (Kind::Login, _) => std::process::exit(0),
            },
            Err(failure) => {
                self.set_busy(false);
                self.password.set_text("");
                self.password.grab_focus();
                if let Failure::Other(text) = failure {
                    let text = if text.is_empty() {
                        "Could not sign in.".to_owned()
                    } else {
                        text
                    };
                    self.message.set_label(&text);
                    self.message.set_visible(true);
                }
                self.password.add_css_class("shake");
                let entry = self.password.clone();
                glib::timeout_add_local_once(Duration::from_millis(450), move || {
                    entry.remove_css_class("shake");
                });
            }
        }
    }
}
