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
const AVATAR: i32 = 64;
/// Room inside the list's scroller above and below the rows. The scroller
/// clips to its own edges, and the pictures' and the field's shadows reach
/// this far.
const SHADOW_ROOM: i32 = 24;
/// How tall the user list may grow before it scrolls, not counting
/// [`SHADOW_ROOM`].
const LIST_HEIGHT: i32 = 320;
/// Every list row is this wide, so the photos share one left edge.
const LIST_WIDTH: i32 = 300;
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
    /// Whose wallpaper and, once chosen, whose password is on screen.
    selected: Cell<usize>,
    /// List mode shows the password only after a person is chosen. Login
    /// starts with nobody chosen when there is more than one account. One
    /// account, and the lock, open already chosen.
    chosen: Cell<bool>,
    status: Rc<Status>,
    views: RefCell<Vec<View>>,
    pictures: RefCell<HashMap<PathBuf, Picture>>,
    /// The picture on screen. Frosted plates read it when they draw.
    wallpaper: Rc<RefCell<Option<gdk::Texture>>>,
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
    /// Sleep, Restart, and Shut Down, behind the sun at login.
    power_actions: gio::SimpleActionGroup,
    confirm_title: gtk::Label,
    confirm_button: gtk::Button,
    pending: Cell<Option<Power>>,
    on_unlock: RefCell<Option<Box<dyn Fn()>>>,
    /// Set once the first wallpaper has loaded or failed to.
    wallpaper_ready: Cell<bool>,
    on_ready: RefCell<Vec<Box<dyn FnOnce()>>>,
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

/// The name and password fields. The plate is the blurred wallpaper; the
/// entry draws the border, the text, and the arrow.
fn field_plate(entry: &gtk::Entry, wallpaper: &Rc<RefCell<Option<gdk::Texture>>>) -> gtk::Widget {
    let plate = crate::frost::Plate::new(wallpaper.clone());
    plate.add_css_class("elavo-greeter-field-plate");
    plate.set_child(entry);
    plate.upcast()
}

fn avatar(
    account: &Account,
    size: i32,
    wallpaper: &Rc<RefCell<Option<gdk::Texture>>>,
) -> gtk::Widget {
    let inner = if let Some(icon) = account.icon.as_ref() {
        if let Some(image) = crate::avatar::image(icon, size) {
            image.add_css_class("elavo-greeter-avatar");
            image
        } else {
            initial_avatar(account, size)
        }
    } else {
        initial_avatar(account, size)
    };
    // A rim of the blurred wallpaper shows around the picture.
    let plate = crate::frost::Plate::new(wallpaper.clone());
    plate.set_rim(crate::frost::PHOTO_RIM);
    plate.add_css_class("elavo-greeter-photo");
    plate.set_child(&inner);
    plate.upcast()
}

fn initial_avatar(account: &Account, size: i32) -> gtk::Widget {
    let frame = gtk::Box::builder()
        .width_request(size)
        .height_request(size)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .overflow(gtk::Overflow::Hidden)
        .css_classes(["elavo-greeter-avatar"])
        .build();
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
    frame.upcast()
}

/// The chosen person's picture when other accounts exist. Hovering it fades
/// in the ring from the style sheet. Clicking it returns to the list.
fn framed_avatar(
    account: &Account,
    size: i32,
    wallpaper: &Rc<RefCell<Option<gdk::Texture>>>,
) -> gtk::Widget {
    let ring = gtk::Box::builder()
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .css_classes(["elavo-greeter-avatar-ring"])
        .build();
    ring.append(&avatar(account, size, wallpaper));
    ring.upcast()
}

/// The login screen's sun: the desktop's Elavo menu, cut down to the power
/// section. Restart and Shut Down ask first, inside the screen.
fn sun_menu(actions: &gio::SimpleActionGroup) -> gtk::MenuButton {
    let power = gio::Menu::new();
    power.append(Some("Sleep"), Some("greeter.sleep"));
    power.append(Some("Restart…"), Some("greeter.restart"));
    power.append(Some("Shut Down…"), Some("greeter.shut-down"));
    let menu = gio::Menu::new();
    menu.append_section(None, &power);

    let button = gtk::MenuButton::builder()
        .icon_name("elavo-sun-symbolic")
        .tooltip_text("Elavo")
        .css_classes(["flat", "elavo-greeter-sun"])
        .menu_model(&menu)
        .always_show_arrow(false)
        .valign(gtk::Align::Center)
        .build();
    button.insert_action_group("greeter", Some(actions));
    if let Some(image) = button
        .first_child()
        .and_then(|toggle| toggle.first_child())
        .and_downcast::<gtk::Image>()
    {
        image.set_pixel_size(16);
    }
    if let Some(popover) = button.popover() {
        popover.set_has_arrow(false);
        // Open from the sun's left edge rather than centered on it, so the
        // menu stays on screen without the compositor moving it.
        popover.set_halign(gtk::Align::Start);
        popover.add_css_class("elavo-greeter-menu-popover");
    }
    button
}

impl Screen {
    pub fn new(kind: Kind, backend: Backend, show: Show, demo_people: usize) -> Rc<Self> {
        let (accounts, selected, chosen) = match kind {
            Kind::Lock => (vec![accounts::current()], 0, true),
            Kind::Login => {
                let mut list = accounts::list();
                list.extend(accounts::demo_people(demo_people));
                let selected = accounts::last_user(&list).unwrap_or(0);
                // One account has nothing to choose. The password is showing
                // when the screen appears.
                let alone = list.len() == 1;
                (list, selected, alone)
            }
        };
        let show = if kind == Kind::Lock { Show::List } else { show };
        let settings = interface_settings();
        let wallpaper = Rc::new(RefCell::new(None));

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
            password_block.append(&field_plate(&name, &wallpaper));
        }
        password_block.append(&field_plate(&password, &wallpaper));
        password_block.append(&message);

        let people = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(4)
            .width_request(LIST_WIDTH)
            .halign(gtk::Align::Center)
            .margin_start(24)
            .margin_end(24)
            .margin_top(SHADOW_ROOM)
            .margin_bottom(SHADOW_ROOM)
            .build();
        // A long list scrolls instead of growing the window. The password
        // stays out of the list until someone is chosen.
        let people_scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::External)
            .propagate_natural_width(true)
            .propagate_natural_height(true)
            .max_content_height(LIST_HEIGHT + SHADOW_ROOM * 2)
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

        let screen = Rc::new(Self {
            kind,
            backend,
            show,
            accounts,
            selected: Cell::new(selected),
            chosen: Cell::new(chosen),
            status: Status::new(kind == Kind::Lock),
            views: RefCell::default(),
            pictures: RefCell::default(),
            wallpaper,
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
            power_actions: gio::SimpleActionGroup::new(),
            confirm_title,
            confirm_button,
            pending: Cell::new(None),
            on_unlock: RefCell::default(),
            wallpaper_ready: Cell::new(false),
            on_ready: RefCell::default(),
        });

        for (name, action) in [
            ("sleep", Power::Sleep),
            ("restart", Power::Restart),
            ("shut-down", Power::ShutDown),
        ] {
            let item = gio::SimpleAction::new(name, None);
            let weak = Rc::downgrade(&screen);
            item.connect_activate(move |_, _| {
                if let Some(screen) = weak.upgrade() {
                    screen.ask(action);
                }
            });
            screen.power_actions.add_action(&item);
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

    /// Start loading the wallpaper before any window exists, so the first
    /// frame can show it. `aspect` is the first output's width over height.
    pub fn preload(self: &Rc<Self>, aspect: Option<f64>) {
        if let Some(aspect) = aspect.filter(|a| a.is_finite() && *a > 0.0) {
            self.aspect.set(aspect);
        }
        self.show_wallpaper();
    }

    /// Runs `f` once the first wallpaper is ready, or at once if it already
    /// is. A picture that fails to load counts as ready.
    pub fn when_ready(&self, f: impl FnOnce() + 'static) {
        if self.wallpaper_ready.get() {
            f();
        } else {
            self.on_ready.borrow_mut().push(Box::new(f));
        }
    }

    fn mark_ready(&self) {
        if self.wallpaper_ready.replace(true) {
            return;
        }
        crate::trace::mark("wallpaper ready");
        let waiting = std::mem::take(&mut *self.on_ready.borrow_mut());
        for f in waiting {
            f();
        }
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

        let sun: gtk::Widget = if self.kind == Kind::Login && primary {
            sun_menu(&self.power_actions).upcast()
        } else {
            gtk::Image::builder()
                .icon_name("elavo-sun-symbolic")
                .pixel_size(16)
                .css_classes(["elavo-greeter-sun-mark"])
                .build()
                .upcast()
        };
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
            let weak = Rc::downgrade(self);
            column.connect_map(move |_| {
                let Some(screen) = weak.upgrade() else {
                    return;
                };
                if screen.show == Show::Name {
                    screen.name.grab_focus();
                } else if screen.chosen.get() {
                    screen.password.grab_focus();
                }
            });
        }
        let base = if self.kind == Kind::Login { 32 } else { 72 };
        // The list's scroller already ends with SHADOW_ROOM of empty space.
        let room = if self.show == Show::Name {
            0
        } else {
            SHADOW_ROOM
        };
        let bottom = gtk::Box::builder()
            .height_request((base - room).max(0))
            .build();
        column.append(&bottom);

        let shade = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        shade.set_can_target(false);
        shade.set_hexpand(true);
        shade.set_vexpand(true);
        shade.set_halign(gtk::Align::Fill);
        shade.set_valign(gtk::Align::Fill);
        shade.add_css_class("elavo-greeter-shade");

        let root = gtk::Overlay::builder()
            .css_classes(["elavo-greeter"])
            .child(&picture)
            .build();
        root.add_overlay(&shade);
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
            self.mark_ready();
            return;
        };
        if self.shown.borrow().as_ref() == Some(&path) && !self.wallpaper_ready.get() {
            // Already loading.
            return;
        }
        self.shown.replace(Some(path.clone()));
        let cached = self.pictures.borrow().get(&path).cloned();
        if let Some(picture) = cached {
            self.apply(&picture);
            self.mark_ready();
            return;
        }
        // Leave the picture that is already on screen. The new one replaces
        // it when the decode finishes, so choosing another person is not a
        // black frame, and a picture decoded before the first present is
        // what that present shows.
        let weak = Rc::downgrade(self);
        let aspect = self.aspect.get();
        let share = f64::from(BAR) / 900.0;
        glib::spawn_future_local(async move {
            let picture = wallpaper::load(path.clone(), aspect, share).await;
            let Some(screen) = weak.upgrade() else {
                return;
            };
            if let Some(picture) = picture {
                screen
                    .pictures
                    .borrow_mut()
                    .insert(path.clone(), picture.clone());
                if screen.shown.borrow().as_ref() == Some(&path) {
                    screen.apply(&picture);
                }
            }
            screen.mark_ready();
        });
    }

    fn apply(&self, picture: &Picture) {
        *self.wallpaper.borrow_mut() = Some(picture.texture.clone());
        for view in self.views.borrow().iter() {
            view.picture.set_paintable(Some(&picture.texture));
            view.clock.set_texture(Some(&picture.texture));
            if let Some(root) = view.picture.root() {
                root.queue_draw();
            }
            for ink in [wallpaper::Ink::White, wallpaper::Ink::Black] {
                view.bar.remove_css_class(ink.css_class());
            }
            view.bar.add_css_class(picture.ink.css_class());
        }
    }

    /// List mode has two states. With nobody chosen, every account is a row.
    /// Once someone is chosen, that person and the password replace the list.
    fn rebuild_people(self: &Rc<Self>) {
        if self.show == Show::Name {
            return;
        }
        if let Some(parent) = self.password_block.parent() {
            if let Ok(parent) = parent.downcast::<gtk::Box>() {
                parent.remove(&self.password_block);
            }
        }
        while let Some(child) = self.people.first_child() {
            self.people.remove(&child);
        }
        if self.chosen.get() && self.selected.get() < self.accounts.len() {
            let column = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(14)
                .halign(gtk::Align::Center)
                .build();
            column.append(&self.person_button(self.selected.get(), true));
            column.append(&self.password_block);
            self.people.append(&column);
        } else {
            for index in 0..self.accounts.len() {
                self.people.append(&self.person_button(index, false));
            }
        }
    }

    /// A list row has the photo on the left and the name on the right.
    /// `back` is the chosen person: the photo sits above the name. With more
    /// than one account, clicking it returns to the list.
    fn person_button(self: &Rc<Self>, index: usize, back: bool) -> gtk::Widget {
        let account = &self.accounts[index];
        let name = gtk::Label::builder()
            .label(account.display_name())
            .ellipsize(pango::EllipsizeMode::End)
            .max_width_chars(22)
            .css_classes(["elavo-greeter-name"])
            .build();
        let picture = if back && self.accounts.len() > 1 {
            framed_avatar(account, AVATAR, &self.wallpaper)
        } else {
            avatar(account, AVATAR, &self.wallpaper)
        };
        let body = if back {
            name.set_justify(gtk::Justification::Center);
            name.set_xalign(0.5);
            let column = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(10)
                .halign(gtk::Align::Center)
                .build();
            column.append(&picture);
            column.append(&name);
            column.upcast::<gtk::Widget>()
        } else {
            name.set_xalign(0.0);
            name.set_halign(gtk::Align::Start);
            name.set_hexpand(true);
            let row = gtk::Box::builder()
                .orientation(gtk::Orientation::Horizontal)
                .spacing(12)
                .halign(gtk::Align::Start)
                .hexpand(true)
                .build();
            row.append(&picture);
            row.append(&name);
            row.upcast::<gtk::Widget>()
        };
        // One account never returns to a list, so the picture is not a button.
        if back && self.accounts.len() < 2 {
            return body;
        }
        // A list row fades in a blurred plate behind the picture and the name.
        // The chosen person keeps the white ring on the picture instead.
        let mut hover = None;
        let child = if back {
            body
        } else {
            body.add_css_class("elavo-greeter-person-row");
            let plate = crate::frost::Plate::new(self.wallpaper.clone());
            plate.add_css_class("elavo-greeter-hover");
            plate.fade();
            plate.set_hexpand(true);
            plate.set_child(&body);
            hover = Some(plate.clone());
            plate.upcast()
        };
        let button = gtk::Button::builder()
            .child(&child)
            .halign(if back {
                gtk::Align::Center
            } else {
                gtk::Align::Fill
            })
            .hexpand(!back)
            .css_classes(["elavo-greeter-person"])
            .tooltip_text(account.display_name())
            .build();
        if back {
            button.add_css_class("elavo-greeter-chosen");
        }
        if let Some(plate) = hover {
            let motion = gtk::EventControllerMotion::new();
            let entered = plate.clone();
            motion.connect_enter(move |_, _, _| entered.set_hovered(true));
            motion.connect_leave(move |_| plate.set_hovered(false));
            button.add_controller(motion);
        }
        let weak = Rc::downgrade(self);
        button.connect_clicked(move |_| {
            if let Some(screen) = weak.upgrade() {
                if back {
                    screen.clear_choice();
                } else {
                    screen.select(index);
                }
            }
        });
        button.upcast()
    }

    fn select(self: &Rc<Self>, index: usize) {
        if index >= self.accounts.len() {
            return;
        }
        self.selected.set(index);
        self.chosen.set(true);
        self.password.set_text("");
        self.message.set_visible(false);
        self.rebuild_people();
        self.show_wallpaper();
        self.password.grab_focus();
    }

    fn clear_choice(self: &Rc<Self>) {
        self.chosen.set(false);
        self.password.set_text("");
        self.message.set_visible(false);
        self.rebuild_people();
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
        self.stack.set_visible_child_name("confirm");
        self.confirm_button.grab_focus();
    }

    fn dismiss(&self) {
        self.pending.set(None);
        self.stack.set_visible_child_name("accounts");
        if self.show == Show::Name && self.name.text().is_empty() {
            self.name.grab_focus();
        } else if self.show == Show::Name || self.chosen.get() {
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
            Show::List => {
                if !self.chosen.get() {
                    return;
                }
                match self.selected_account() {
                    Some(account) => account.name.clone(),
                    None => return,
                }
            }
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
                let field = self
                    .password
                    .parent()
                    .unwrap_or_else(|| self.password.clone().upcast());
                field.add_css_class("shake");
                glib::timeout_add_local_once(Duration::from_millis(450), move || {
                    field.remove_css_class("shake");
                });
            }
        }
    }
}
