//! The App Switcher page. The page holds the few choices most people make.
//! Everything else is in the Advanced window, by topic. Every control writes
//! `switcher.conf` as it changes, and the switcher watches that file. Extra
//! shortcuts are the only thing that also lands in the user's labwc `rc.xml`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use crate::config::{
    AppRule, Appearance, DisplayMode, FontFace, FontScale, Layout, Scope, ScopedShortcut,
    SelectionColor, Settings, Shelf, SortOrder, Truncation,
};
use crate::probe::previews_available;
use crate::scoped;

const STYLE: &str = "
.elavo-switcher-settings { padding: 18px 24px 24px; }
.elavo-switcher-settings > label.heading { margin: 6px 4px 8px; }
.elavo-switcher-settings list { border-radius: 10px; }
.elavo-switcher-settings row { padding: 8px 12px; min-height: 32px; }
.elavo-switcher-settings .caption { opacity: 0.6; font-size: smaller; }
.elavo-switcher-advanced stacksidebar { min-width: 180px; }
.elavo-switcher-advanced .elavo-switcher-footer { padding: 10px 16px; }
";

fn row(title: &str, control: &impl IsA<gtk::Widget>) -> gtk::ListBoxRow {
    row_with(title, None, control)
}

/// A row with a line under the title that says what the option does.
fn row_note(title: &str, note: &str, control: &impl IsA<gtk::Widget>) -> gtk::ListBoxRow {
    row_with(title, Some(note), control)
}

fn row_with(title: &str, note: Option<&str>, control: &impl IsA<gtk::Widget>) -> gtk::ListBoxRow {
    let texts = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .hexpand(true)
        .valign(gtk::Align::Center)
        .build();
    texts.append(
        &gtk::Label::builder()
            .label(title)
            .xalign(0.0)
            .wrap(true)
            .build(),
    );
    if let Some(note) = note {
        texts.append(
            &gtk::Label::builder()
                .label(note)
                .xalign(0.0)
                .wrap(true)
                .css_classes(["caption"])
                .build(),
        );
    }
    let content = gtk::Box::builder().spacing(12).build();
    content.append(&texts);
    control.set_valign(gtk::Align::Center);
    content.append(control);
    let row = gtk::ListBoxRow::builder()
        .activatable(false)
        .selectable(false)
        .child(&content)
        .build();
    // A dimmed control dims its whole row, so the reason it is off is visible.
    control
        .bind_property("sensitive", &row, "sensitive")
        .sync_create()
        .build();
    row
}

fn caption(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .wrap(true)
        .css_classes(["caption"])
        .build()
}

fn slider(min: f64, max: f64) -> gtk::Scale {
    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, min, max, 1.0);
    scale.set_round_digits(0);
    scale.set_hexpand(true);
    scale.set_draw_value(true);
    scale.set_value_pos(gtk::PositionType::Right);
    scale.set_width_request(180);
    scale
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

fn heading(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .css_classes(["heading"])
        .build()
}

fn dropdown<T>(all: &'static [T], current: T, label: fn(T) -> &'static str) -> gtk::DropDown
where
    T: Copy + PartialEq + 'static,
{
    let names: Vec<&str> = all.iter().copied().map(label).collect();
    let model = gtk::StringList::new(&names);
    let dropdown = gtk::DropDown::builder().model(&model).build();
    if let Some(index) = all.iter().position(|item| *item == current) {
        dropdown.set_selected(index as u32);
    }
    dropdown
}

fn selected<T: Copy>(dropdown: &gtk::DropDown, all: &'static [T]) -> Option<T> {
    all.get(dropdown.selected() as usize).copied()
}

struct Controls {
    layout: Vec<(Layout, gtk::ToggleButton)>,
    panel_scale: gtk::Scale,
    list_width: gtk::Scale,
    grid_columns: gtk::Scale,
    grid_single_row: gtk::Switch,
    appearance: gtk::DropDown,
    opacity: gtk::Scale,
    corner_radius: gtk::Scale,
    selection: gtk::DropDown,
    selection_hex: gtk::Entry,
    font_scale: gtk::DropDown,
    font_face: gtk::DropDown,
    bold: gtk::Switch,
    names: gtk::Switch,
    status: gtk::Switch,
    truncation: gtk::DropDown,
    reveal: gtk::Scale,
    stay_release: gtk::Switch,
    stay_tap: gtk::Switch,
    sort: gtk::DropDown,
    show_minimized: gtk::Switch,
    sink_minimized: gtk::Switch,
    windowless: gtk::Switch,
    badges: gtk::Switch,
    recent: gtk::Switch,
    recent_limit: gtk::Scale,
    fuzzy: gtk::Switch,
    fuzzy_rank: gtk::Switch,
    launchable: gtk::Switch,
    letters: gtk::Switch,
    letter_ms: gtk::Scale,
    scroll: gtk::Switch,
    scroll_reverse: gtk::Switch,
    click_outside: gtk::Switch,
    hover_selection: gtk::Switch,
    hover_actions: gtk::Switch,
    hover_close: gtk::Switch,
    hover_minimize: gtk::Switch,
    hover_maximize: gtk::Switch,
    hover_quit: gtk::Switch,
    drill: gtk::Switch,
    shelf: gtk::DropDown,
    shelf_single: gtk::Switch,
    vim: gtk::Switch,
    grave: gtk::Switch,
    audio: gtk::Switch,
    sound: gtk::Switch,
    display: gtk::DropDown,
    rules: gtk::ListBox,
    app_choice: gtk::DropDown,
    app_ids: Vec<String>,
    rule_action: gtk::DropDown,
    scoped: gtk::ListBox,
    scope: gtk::DropDown,
    record: gtk::Button,
    recorded: RefCell<Option<String>>,
    preview_note: gtk::Label,
    advanced_status: gtk::Label,
    advanced: RefCell<Option<gtk::Window>>,
    syncing: Cell<bool>,
    monitor: RefCell<Option<gio::FileMonitor>>,
}

impl Controls {
    fn write(&self, f: impl FnOnce(&mut Settings)) {
        self.dependents();
        if self.syncing.get() {
            return;
        }
        Settings::update(f);
    }

    /// Options that only mean something while another one is on are dimmed
    /// until it is.
    fn dependents(&self) {
        let layout = self
            .layout
            .iter()
            .find(|(_, button)| button.is_active())
            .map(|(layout, _)| *layout);
        self.list_width.set_sensitive(layout == Some(Layout::List));
        let grid = layout == Some(Layout::Grid);
        self.grid_columns.set_sensitive(grid);
        self.grid_single_row.set_sensitive(grid);
        self.selection_hex.set_sensitive(
            selected(&self.selection, SelectionColor::ALL) == Some(SelectionColor::Custom),
        );
        self.sink_minimized
            .set_sensitive(self.show_minimized.is_active());
        self.recent_limit.set_sensitive(self.recent.is_active());
        let fuzzy = self.fuzzy.is_active();
        self.fuzzy_rank.set_sensitive(fuzzy);
        self.launchable.set_sensitive(fuzzy);
        self.letter_ms
            .set_sensitive(fuzzy || self.letters.is_active());
        self.shelf_single
            .set_sensitive(selected(&self.shelf, Shelf::ALL) != Some(Shelf::Off));
        self.scroll_reverse.set_sensitive(self.scroll.is_active());
        let hover = self.hover_actions.is_active();
        for switch in [
            &self.hover_close,
            &self.hover_minimize,
            &self.hover_maximize,
            &self.hover_quit,
        ] {
            switch.set_sensitive(hover);
        }
    }

    fn sync(&self, s: &Settings) {
        self.syncing.set(true);
        for (layout, button) in &self.layout {
            button.set_active(*layout == s.layout);
        }
        self.panel_scale.set_value(f64::from(s.panel_scale));
        self.list_width.set_value(f64::from(s.list_width));
        self.grid_columns.set_value(f64::from(s.grid_max_columns));
        self.grid_single_row.set_active(s.grid_single_row);
        select(&self.appearance, Appearance::ALL, s.appearance);
        self.opacity.set_value(f64::from(s.opacity));
        self.corner_radius.set_value(f64::from(s.corner_radius));
        select(&self.selection, SelectionColor::ALL, s.selection_color);
        self.selection_hex.set_text(&s.selection_color_hex);
        select(&self.font_scale, FontScale::ALL, s.font_scale);
        select(&self.font_face, FontFace::ALL, s.font_face);
        self.bold.set_active(s.bold_selected_label);
        self.names.set_active(s.show_application_names);
        self.status.set_active(s.show_window_status);
        select(&self.truncation, Truncation::ALL, s.title_truncation);
        self.reveal.set_value(f64::from(s.reveal_delay_ms));
        self.stay_release.set_active(s.stay_open_on_release);
        self.stay_tap.set_active(s.stay_open_on_quick_tap);
        select(&self.sort, SortOrder::ALL, s.sort_order);
        self.show_minimized.set_active(s.show_minimized);
        self.sink_minimized.set_active(s.sink_minimized);
        self.windowless.set_active(s.show_windowless);
        self.badges.set_active(s.show_unread_badges);
        self.recent.set_active(s.show_recently_closed);
        self.recent_limit
            .set_value(f64::from(s.recently_closed_limit));
        self.fuzzy.set_active(s.fuzzy_search);
        self.fuzzy_rank.set_active(s.fuzzy_rank_best);
        self.launchable.set_active(s.search_includes_launchable);
        self.letters.set_active(s.letter_hints);
        self.letter_ms.set_value(f64::from(s.letter_chain_ms));
        self.scroll.set_active(s.scroll_to_switch);
        self.scroll_reverse.set_active(s.scroll_reverse);
        self.click_outside.set_active(s.click_outside_to_dismiss);
        self.hover_selection.set_active(s.hover_selection);
        self.hover_actions.set_active(s.hover_actions);
        self.hover_close.set_active(s.hover_close);
        self.hover_minimize.set_active(s.hover_minimize);
        self.hover_maximize.set_active(s.hover_maximize);
        self.hover_quit.set_active(s.hover_quit);
        self.drill.set_active(s.window_drill);
        select(&self.shelf, Shelf::ALL, s.window_shelf);
        self.shelf_single.set_active(s.window_shelf_single);
        self.vim.set_active(s.vim_navigation);
        self.grave.set_active(s.grave_reverses);
        self.audio.set_active(s.show_audio);
        self.sound.set_active(s.sound_on_commit);
        select(&self.display, DisplayMode::ALL, s.display_mode);
        self.fill_rules(&s.rules);
        self.fill_scoped(&s.scoped);
        self.advanced_status.set_label(if customized(s) {
            "Some advanced options are changed."
        } else {
            ""
        });
        self.dependents();
        self.syncing.set(false);
    }

    fn fill_rules(&self, rules: &[AppRule]) {
        while let Some(child) = self.rules.first_child() {
            self.rules.remove(&child);
        }
        for rule in rules {
            let name = app_name(&rule.id);
            let mut bits = Vec::new();
            if rule.pinned {
                bits.push("pinned");
            }
            if rule.hidden {
                bits.push("hidden");
            }
            match rule.skip_fullscreen_only {
                Some(true) => bits.push("skips Alt+Tab when fullscreen"),
                Some(false) => bits.push("skips Alt+Tab"),
                None => {}
            }
            let title = if bits.is_empty() {
                name
            } else {
                format!("{name} ({})", bits.join(", "))
            };
            let remove = gtk::Button::with_label("Remove");
            let id = rule.id.clone();
            remove.connect_clicked(move |_| {
                Settings::update(|s| s.rules.retain(|rule| rule.id != id));
            });
            self.rules.append(&row(&title, &remove));
        }
    }

    fn fill_scoped(&self, scoped: &[ScopedShortcut]) {
        while let Some(child) = self.scoped.first_child() {
            self.scoped.remove(&child);
        }
        for shortcut in scoped {
            let remove = gtk::Button::with_label("Remove");
            let key = shortcut.key.clone();
            remove.connect_clicked(move |_| {
                Settings::update(|s| s.scoped.retain(|shortcut| shortcut.key != key));
            });
            self.scoped.append(&row(
                &format!("{} — {}", shortcut.key, shortcut.scope.label()),
                &remove,
            ));
        }
    }
}

/// The values the App Switcher page itself shows. The Advanced window and
/// Restore Defaults leave these alone.
fn keep_basic(from: &Settings, to: &mut Settings) {
    to.layout = from.layout;
    to.panel_scale = from.panel_scale;
    to.appearance = from.appearance;
    to.show_application_names = from.show_application_names;
    to.show_minimized = from.show_minimized;
    to.display_mode = from.display_mode;
}

/// Whether anything in the Advanced window differs from the defaults.
fn customized(s: &Settings) -> bool {
    let mut defaults = Settings::default();
    keep_basic(s, &mut defaults);
    *s != defaults
}

/// Advanced options back to their defaults. The page's own choices, the
/// application list, and extra shortcuts stay.
fn restore_advanced(s: &mut Settings) {
    let mut restored = Settings::default();
    keep_basic(s, &mut restored);
    restored.rules = std::mem::take(&mut s.rules);
    restored.scoped = std::mem::take(&mut s.scoped);
    *s = restored;
}

fn select<T: Copy + PartialEq>(dropdown: &gtk::DropDown, all: &'static [T], current: T) {
    if let Some(index) = all.iter().position(|item| *item == current) {
        dropdown.set_selected(index as u32);
    }
}

fn app_name(id: &str) -> String {
    gio::AppInfo::all()
        .into_iter()
        .find(|info| info.id().as_deref() == Some(id))
        .map(|info| info.name().to_string())
        .unwrap_or_else(|| id.trim_end_matches(".desktop").to_owned())
}

fn applications() -> Vec<(String, String)> {
    let mut apps: Vec<(String, String)> = gio::AppInfo::all()
        .into_iter()
        .filter_map(|info| {
            let id = info.id()?.to_string();
            if !id.ends_with(".desktop") {
                return None;
            }
            Some((info.name().to_string(), id))
        })
        .collect();
    apps.sort_by_key(|a| a.0.to_lowercase());
    apps.dedup_by(|a, b| a.1 == b.1);
    apps
}

fn layout_buttons(previews: bool) -> Vec<(Layout, gtk::ToggleButton)> {
    let first = gtk::ToggleButton::with_label(Layout::Strip.label());
    Layout::ALL
        .iter()
        .copied()
        .map(|layout| {
            let button = if layout == Layout::Strip {
                first.clone()
            } else {
                let button = gtk::ToggleButton::with_label(layout.label());
                button.set_group(Some(&first));
                button
            };
            if layout == Layout::Previews {
                button.set_sensitive(previews);
                button.set_tooltip_text(Some(if previews {
                    "Pictures of each window"
                } else {
                    "This compositor cannot hand over a window's picture"
                }));
            }
            (layout, button)
        })
        .collect()
}

/// The page, ready to put in System Settings.
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

    let loaded = Settings::load();
    let previews = previews_available();
    let apps = applications();
    let app_names: Vec<&str> = apps.iter().map(|(name, _)| name.as_str()).collect();
    let app_model = gtk::StringList::new(&app_names);
    let app_ids = apps.into_iter().map(|(_, id)| id).collect::<Vec<_>>();

    let controls = Rc::new(Controls {
        layout: layout_buttons(previews),
        panel_scale: slider(35.0, 150.0),
        list_width: slider(30.0, 100.0),
        grid_columns: slider(0.0, 12.0),
        grid_single_row: gtk::Switch::new(),
        appearance: dropdown(Appearance::ALL, loaded.appearance, Appearance::label),
        opacity: slider(30.0, 100.0),
        corner_radius: slider(-1.0, 40.0),
        selection: dropdown(
            SelectionColor::ALL,
            loaded.selection_color,
            SelectionColor::label,
        ),
        selection_hex: gtk::Entry::builder().placeholder_text("#112233").build(),
        font_scale: dropdown(FontScale::ALL, loaded.font_scale, FontScale::label),
        font_face: dropdown(FontFace::ALL, loaded.font_face, FontFace::label),
        bold: gtk::Switch::new(),
        names: gtk::Switch::new(),
        status: gtk::Switch::new(),
        truncation: dropdown(Truncation::ALL, loaded.title_truncation, Truncation::label),
        reveal: slider(0.0, 500.0),
        stay_release: gtk::Switch::new(),
        stay_tap: gtk::Switch::new(),
        sort: dropdown(SortOrder::ALL, loaded.sort_order, SortOrder::label),
        show_minimized: gtk::Switch::new(),
        sink_minimized: gtk::Switch::new(),
        windowless: gtk::Switch::new(),
        badges: gtk::Switch::new(),
        recent: gtk::Switch::new(),
        recent_limit: slider(0.0, 50.0),
        fuzzy: gtk::Switch::new(),
        fuzzy_rank: gtk::Switch::new(),
        launchable: gtk::Switch::new(),
        letters: gtk::Switch::new(),
        letter_ms: slider(200.0, 3000.0),
        scroll: gtk::Switch::new(),
        scroll_reverse: gtk::Switch::new(),
        click_outside: gtk::Switch::new(),
        hover_selection: gtk::Switch::new(),
        hover_actions: gtk::Switch::new(),
        hover_close: gtk::Switch::new(),
        hover_minimize: gtk::Switch::new(),
        hover_maximize: gtk::Switch::new(),
        hover_quit: gtk::Switch::new(),
        drill: gtk::Switch::new(),
        shelf: dropdown(Shelf::ALL, loaded.window_shelf, Shelf::label),
        shelf_single: gtk::Switch::new(),
        vim: gtk::Switch::new(),
        grave: gtk::Switch::new(),
        audio: gtk::Switch::new(),
        sound: gtk::Switch::new(),
        display: dropdown(DisplayMode::ALL, loaded.display_mode, DisplayMode::label),
        rules: gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["rich-list"])
            .build(),
        app_choice: gtk::DropDown::builder().model(&app_model).build(),
        app_ids,
        rule_action: gtk::DropDown::from_strings(&[
            "Pin to front",
            "Hide",
            "Skip Alt+Tab",
            "Skip when fullscreen",
        ]),
        scoped: gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["rich-list"])
            .build(),
        scope: dropdown(Scope::ALL, Scope::All, Scope::label),
        record: gtk::Button::with_label("Record shortcut"),
        recorded: RefCell::new(None),
        preview_note: gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .css_classes(["caption"])
            .label(if previews {
                ""
            } else {
                "Window previews stay off: this compositor cannot hand over a window's picture."
            })
            .build(),
        advanced_status: caption(""),
        advanced: RefCell::new(None),
        syncing: Cell::new(false),
        monitor: RefCell::new(None),
    });
    controls.sync(&loaded);

    wire(&controls);

    let page = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .css_classes(["elavo-switcher-settings"])
        .build();
    page.append(&heading("App Switcher"));
    page.append(&caption(
        "Hold Alt and press Tab to switch between applications. \
         Hold Alt and press ` to switch between windows of the application you are in.",
    ));

    let layouts = gtk::Box::builder().css_classes(["linked"]).build();
    for (_, button) in &controls.layout {
        layouts.append(button);
    }
    page.append(&group(&[
        row("Style", &layouts),
        row("Size", &controls.panel_scale),
        row("Appearance", &controls.appearance),
        row("Show application names", &controls.names),
    ]));
    page.append(&controls.preview_note);
    page.append(&group(&[
        row("Include minimized windows", &controls.show_minimized),
        row("Show on", &controls.display),
    ]));

    let advanced = gtk::Button::with_label("Advanced…");
    let footer = gtk::Box::builder().spacing(12).build();
    controls.advanced_status.set_hexpand(true);
    footer.append(&controls.advanced_status);
    footer.append(&advanced);
    page.append(&footer);

    let c = Rc::downgrade(&controls);
    let anchor = page.downgrade();
    advanced.connect_clicked(move |_| {
        let (Some(c), Some(anchor)) = (c.upgrade(), anchor.upgrade()) else {
            return;
        };
        let window = c
            .advanced
            .borrow_mut()
            .get_or_insert_with(|| advanced_window(&c))
            .clone();
        window.set_transient_for(anchor.root().and_downcast::<gtk::Window>().as_ref());
        window.present();
    });

    let c = Rc::downgrade(&controls);
    controls.monitor.replace(Settings::watch(move |s| {
        if let Some(c) = c.upgrade() {
            c.sync(s);
        }
    }));
    let keep = RefCell::new(Some(controls));
    page.connect_destroy(move |_| {
        if let Some(controls) = keep.take() {
            if let Some(window) = controls.advanced.take() {
                window.destroy();
            }
        }
    });
    page.upcast()
}

/// A page in the Advanced window: groups under headings, scrolled.
fn section(parts: &[gtk::Widget]) -> gtk::ScrolledWindow {
    let body = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .css_classes(["elavo-switcher-settings"])
        .build();
    for part in parts {
        body.append(part);
    }
    gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .hexpand(true)
        .child(&body)
        .build()
}

fn advanced_window(controls: &Rc<Controls>) -> gtk::Window {
    let c = controls;
    let stack = gtk::Stack::builder()
        .transition_type(gtk::StackTransitionType::Crossfade)
        .build();

    let appearance = section(&[
        heading("Panel").upcast(),
        group(&[
            row("Opacity", &c.opacity),
            row_note(
                "Corner radius",
                "0 follows the size. −1 makes square corners.",
                &c.corner_radius,
            ),
            row("Selection color", &c.selection),
            row("Custom color", &c.selection_hex),
        ])
        .upcast(),
        heading("Text").upcast(),
        group(&[
            row("Font size", &c.font_scale),
            row("Font", &c.font_face),
            row("Bold name for the selection", &c.bold),
            row("Shorten long titles at the", &c.truncation),
        ])
        .upcast(),
        heading("List and grid").upcast(),
        group(&[
            row_note("List width", "Used by the List style.", &c.list_width),
            row_note(
                "Grid columns",
                "Used by the Grid style. 0 fits the screen.",
                &c.grid_columns,
            ),
            row("Keep the grid on one row", &c.grid_single_row),
        ])
        .upcast(),
        heading("Marks").upcast(),
        group(&[
            row("Minimized and full screen marks", &c.status),
            row("Unread badges", &c.badges),
            row("Mark applications playing sound", &c.audio),
        ])
        .upcast(),
    ]);
    stack.add_titled(&appearance, Some("appearance"), "Appearance");

    let behavior = section(&[
        heading("Timing").upcast(),
        group(&[
            row_note(
                "Delay before showing (ms)",
                "A quicker press switches without showing the switcher.",
                &c.reveal,
            ),
            row("Stay open after releasing Alt", &c.stay_release),
            row("Stay open after a quick press", &c.stay_tap),
        ])
        .upcast(),
        heading("What is listed").upcast(),
        group(&[
            row("Order", &c.sort),
            row("Put minimized windows last", &c.sink_minimized),
            row_note(
                "Include applications with no window",
                "Adds Dock applications that are not running.",
                &c.windowless,
            ),
            row_note(
                "Recently quit applications",
                "Lets search open them again.",
                &c.recent,
            ),
            row("How many recently quit", &c.recent_limit),
        ])
        .upcast(),
        heading("Sound").upcast(),
        group(&[row("Play a sound when switching", &c.sound)]).upcast(),
    ]);
    stack.add_titled(&behavior, Some("behavior"), "Behavior");

    let keyboard = section(&[
        heading("Search").upcast(),
        group(&[
            row("Type to filter", &c.fuzzy),
            row("Put the best match first", &c.fuzzy_rank),
            row("Include applications that are not running", &c.launchable),
            row_note(
                "Letter hints",
                "Press a letter to jump to the application it marks.",
                &c.letters,
            ),
            row("Typed letters last (ms)", &c.letter_ms),
        ])
        .upcast(),
        heading("Windows").upcast(),
        group(&[
            row("Down arrow shows the application's windows", &c.drill),
            row("Window shelf", &c.shelf),
            row("Shelf for an application with one window", &c.shelf_single),
        ])
        .upcast(),
        heading("Keys").upcast(),
        group(&[
            row_note(
                "Vim keys",
                "H, J, K, and L move the selection. Shift+H minimizes.",
                &c.vim,
            ),
            row_note(
                "Alt+` steps back through applications",
                "Instead of switching windows of the application you are in.",
                &c.grave,
            ),
        ])
        .upcast(),
    ]);
    stack.add_titled(&keyboard, Some("keyboard"), "Keyboard & Search");

    let pointer = section(&[
        group(&[
            row("Scroll to move the selection", &c.scroll),
            row("Reverse scrolling", &c.scroll_reverse),
            row("Click outside to cancel", &c.click_outside),
            row("Select the icon under the pointer", &c.hover_selection),
        ])
        .upcast(),
        heading("Buttons on the icon under the pointer").upcast(),
        group(&[
            row("Show buttons", &c.hover_actions),
            row("Close", &c.hover_close),
            row("Minimize", &c.hover_minimize),
            row("Maximize", &c.hover_maximize),
            row("Quit", &c.hover_quit),
        ])
        .upcast(),
    ]);
    stack.add_titled(&pointer, Some("pointer"), "Pointer");

    let add_rule = gtk::Button::with_label("Add");
    let rule_bar = gtk::Box::builder().spacing(8).build();
    c.app_choice.set_hexpand(true);
    rule_bar.append(&c.app_choice);
    rule_bar.append(&c.rule_action);
    rule_bar.append(&add_rule);
    let applications = section(&[
        caption(
            "Put an application first, leave it out, or have Alt+Tab do nothing while it is in front.",
        )
        .upcast(),
        gtk::Frame::builder().child(&c.rules).build().upcast(),
        rule_bar.upcast(),
    ]);
    stack.add_titled(&applications, Some("applications"), "Applications");

    let add_scoped = gtk::Button::with_label("Add");
    let scoped_bar = gtk::Box::builder().spacing(8).build();
    c.record.set_hexpand(true);
    scoped_bar.append(&c.record);
    scoped_bar.append(&c.scope);
    scoped_bar.append(&add_scoped);
    let shortcuts = section(&[
        caption("Each shortcut opens the switcher with only some windows. Alt+Tab and Alt+` stay as they are.")
            .upcast(),
        gtk::Frame::builder().child(&c.scoped).build().upcast(),
        scoped_bar.upcast(),
    ]);
    stack.add_titled(&shortcuts, Some("shortcuts"), "Shortcuts");

    let sidebar = gtk::StackSidebar::builder().stack(&stack).build();
    let body = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    body.append(&sidebar);
    body.append(&gtk::Separator::new(gtk::Orientation::Vertical));
    body.append(&stack);

    let restore = gtk::Button::with_label("Restore Defaults");
    let done = gtk::Button::builder()
        .label("Done")
        .css_classes(["suggested-action"])
        .build();
    let footer = gtk::Box::builder()
        .spacing(8)
        .css_classes(["elavo-switcher-footer"])
        .build();
    restore.set_hexpand(true);
    restore.set_halign(gtk::Align::Start);
    footer.append(&restore);
    footer.append(&done);

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.append(&body);
    root.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    root.append(&footer);

    let window = gtk::Window::builder()
        .title("App Switcher Options")
        .titlebar(
            &gtk::HeaderBar::builder()
                .css_classes(["elavo-title-only"])
                .build(),
        )
        .modal(true)
        .hide_on_close(true)
        .default_width(720)
        .default_height(560)
        .child(&root)
        .build();
    // Added, not set: `css_classes` on the builder replaces GTK's own
    // `background` class, and the window then draws with no fill.
    window.add_css_class("elavo-switcher-advanced");
    window.set_default_widget(Some(&done));

    let win = window.downgrade();
    done.connect_clicked(move |_| {
        if let Some(window) = win.upgrade() {
            window.close();
        }
    });
    let win = window.downgrade();
    restore.connect_clicked(move |_| {
        let Some(window) = win.upgrade() else {
            return;
        };
        let dialog = gtk::AlertDialog::builder()
            .message("Restore the default options?")
            .detail("Your style, size, and the other choices on the App Switcher page stay. So do the applications and shortcuts you added.")
            .buttons(["Cancel", "Restore"])
            .cancel_button(0)
            .default_button(1)
            .modal(true)
            .build();
        dialog.choose(Some(&window), gio::Cancellable::NONE, |answer| {
            if matches!(answer, Ok(1)) {
                Settings::update(restore_advanced);
            }
        });
    });

    let c = Rc::downgrade(controls);
    add_rule.connect_clicked(move |_| {
        let Some(c) = c.upgrade() else {
            return;
        };
        let Some(id) = c.app_ids.get(c.app_choice.selected() as usize).cloned() else {
            return;
        };
        let action = c.rule_action.selected();
        c.write(|s| {
            let rule = s.rules.iter_mut().find(|rule| rule.id == id);
            let rule = if let Some(rule) = rule {
                rule
            } else {
                s.rules.push(AppRule {
                    id: id.clone(),
                    pinned: false,
                    hidden: false,
                    skip_fullscreen_only: None,
                });
                s.rules.last_mut().unwrap()
            };
            match action {
                0 => rule.pinned = true,
                1 => rule.hidden = true,
                2 => rule.skip_fullscreen_only = Some(false),
                3 => rule.skip_fullscreen_only = Some(true),
                _ => {}
            }
        });
    });
    let c = Rc::downgrade(controls);
    add_scoped.connect_clicked(move |_| {
        let Some(c) = c.upgrade() else {
            return;
        };
        let Some(key) = c.recorded.borrow().clone() else {
            return;
        };
        let Some(scope) = selected(&c.scope, Scope::ALL) else {
            return;
        };
        c.write(|s| {
            s.scoped.retain(|shortcut| shortcut.key != key);
            s.scoped.push(ScopedShortcut { key, scope });
        });
        c.recorded.replace(None);
        c.record.set_label("Record shortcut");
    });
    let c = Rc::downgrade(controls);
    controls.record.connect_clicked(move |button| {
        if let Some(c) = c.upgrade() {
            c.recorded.replace(None);
            button.set_label("Press a shortcut…");
        }
    });
    let keys = gtk::EventControllerKey::new();
    let c = Rc::downgrade(controls);
    let win = window.downgrade();
    keys.connect_key_pressed(move |_, keyval, _, mods| {
        let Some(c) = c.upgrade() else {
            return glib::Propagation::Proceed;
        };
        let recording = c.record.label().as_deref() == Some("Press a shortcut…");
        if !recording {
            let close = keyval == gdk::Key::Escape
                || (mods.contains(gdk::ModifierType::CONTROL_MASK) && keyval == gdk::Key::w);
            if let (true, Some(window)) = (close, win.upgrade()) {
                window.close();
                return glib::Propagation::Stop;
            }
            return glib::Propagation::Proceed;
        }
        if matches!(keyval, gdk::Key::Escape) {
            c.record.set_label("Record shortcut");
            return glib::Propagation::Stop;
        }
        let Some(name) = labwc_key(keyval, mods) else {
            return glib::Propagation::Stop;
        };
        if !scoped::key_allowed(&name) {
            c.record.set_label("Choose another shortcut");
            return glib::Propagation::Stop;
        }
        c.recorded.replace(Some(name.clone()));
        c.record.set_label(&name);
        glib::Propagation::Stop
    });
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    window.add_controller(keys);

    let c = Rc::downgrade(controls);
    window.connect_hide(move |_| {
        if let Some(c) = c.upgrade() {
            if c.record.label().as_deref() == Some("Press a shortcut…") {
                c.record.set_label("Record shortcut");
            }
        }
    });

    window
}

fn labwc_key(key: gdk::Key, mods: gdk::ModifierType) -> Option<String> {
    if matches!(
        key,
        gdk::Key::Alt_L
            | gdk::Key::Alt_R
            | gdk::Key::Control_L
            | gdk::Key::Control_R
            | gdk::Key::Shift_L
            | gdk::Key::Shift_R
            | gdk::Key::Super_L
            | gdk::Key::Super_R
    ) {
        return None;
    }
    let name = key.name()?.to_string();
    let name = if name.len() == 1 {
        name.to_ascii_lowercase()
    } else {
        name
    };
    let mut parts = Vec::new();
    if mods.contains(gdk::ModifierType::ALT_MASK) {
        parts.push("A");
    }
    if mods.contains(gdk::ModifierType::SHIFT_MASK) {
        parts.push("S");
    }
    if mods.contains(gdk::ModifierType::CONTROL_MASK) {
        parts.push("C");
    }
    if mods.contains(gdk::ModifierType::SUPER_MASK) {
        parts.push("W");
    }
    if parts.is_empty() {
        return None;
    }
    parts.push(&name);
    Some(parts.join("-"))
}

fn wire(controls: &Rc<Controls>) {
    for (layout, button) in &controls.layout {
        let c = Rc::downgrade(controls);
        let layout = *layout;
        button.connect_toggled(move |button| {
            if let (Some(c), true) = (c.upgrade(), button.is_active()) {
                c.write(|s| s.layout = layout);
            }
        });
    }
    type SetScale = fn(&mut Settings, i32);
    let scales: [(&gtk::Scale, SetScale); 7] = [
        (&controls.panel_scale, |s, v| s.panel_scale = v),
        (&controls.list_width, |s, v| s.list_width = v),
        (&controls.grid_columns, |s, v| s.grid_max_columns = v),
        (&controls.opacity, |s, v| s.opacity = v),
        (&controls.corner_radius, |s, v| s.corner_radius = v),
        (&controls.reveal, |s, v| s.reveal_delay_ms = v),
        (&controls.recent_limit, |s, v| s.recently_closed_limit = v),
    ];
    for (scale, set) in scales {
        let c = Rc::downgrade(controls);
        scale.connect_value_changed(move |scale| {
            if let Some(c) = c.upgrade() {
                let value = scale.value().round() as i32;
                c.write(|s| set(s, value));
            }
        });
    }
    let c = Rc::downgrade(controls);
    controls.letter_ms.connect_value_changed(move |scale| {
        if let Some(c) = c.upgrade() {
            let value = scale.value().round() as i32;
            c.write(|s| s.letter_chain_ms = value);
        }
    });
    type Set = fn(&mut Settings, bool);
    let switches: [(&gtk::Switch, Set); 24] = [
        (&controls.grid_single_row, |s, v| s.grid_single_row = v),
        (&controls.bold, |s, v| s.bold_selected_label = v),
        (&controls.names, |s, v| s.show_application_names = v),
        (&controls.status, |s, v| s.show_window_status = v),
        (&controls.stay_release, |s, v| s.stay_open_on_release = v),
        (&controls.stay_tap, |s, v| s.stay_open_on_quick_tap = v),
        (&controls.show_minimized, |s, v| s.show_minimized = v),
        (&controls.sink_minimized, |s, v| s.sink_minimized = v),
        (&controls.windowless, |s, v| s.show_windowless = v),
        (&controls.badges, |s, v| s.show_unread_badges = v),
        (&controls.recent, |s, v| s.show_recently_closed = v),
        (&controls.fuzzy, |s, v| s.fuzzy_search = v),
        (&controls.fuzzy_rank, |s, v| s.fuzzy_rank_best = v),
        (&controls.launchable, |s, v| {
            s.search_includes_launchable = v
        }),
        (&controls.letters, |s, v| s.letter_hints = v),
        (&controls.scroll, |s, v| s.scroll_to_switch = v),
        (&controls.scroll_reverse, |s, v| s.scroll_reverse = v),
        (&controls.click_outside, |s, v| {
            s.click_outside_to_dismiss = v
        }),
        (&controls.hover_selection, |s, v| s.hover_selection = v),
        (&controls.hover_actions, |s, v| s.hover_actions = v),
        (&controls.hover_close, |s, v| s.hover_close = v),
        (&controls.hover_minimize, |s, v| s.hover_minimize = v),
        (&controls.hover_maximize, |s, v| s.hover_maximize = v),
        (&controls.hover_quit, |s, v| s.hover_quit = v),
    ];
    for (switch, set) in switches {
        let c = Rc::downgrade(controls);
        switch.connect_active_notify(move |switch| {
            if let Some(c) = c.upgrade() {
                let active = switch.is_active();
                c.write(|s| set(s, active));
            }
        });
    }
    let more: [(&gtk::Switch, Set); 4] = [
        (&controls.drill, |s, v| s.window_drill = v),
        (&controls.shelf_single, |s, v| s.window_shelf_single = v),
        (&controls.vim, |s, v| s.vim_navigation = v),
        (&controls.grave, |s, v| s.grave_reverses = v),
    ];
    for (switch, set) in more {
        let c = Rc::downgrade(controls);
        switch.connect_active_notify(move |switch| {
            if let Some(c) = c.upgrade() {
                let active = switch.is_active();
                c.write(|s| set(s, active));
            }
        });
    }
    let c = Rc::downgrade(controls);
    controls.audio.connect_active_notify(move |switch| {
        if let Some(c) = c.upgrade() {
            let active = switch.is_active();
            c.write(|s| s.show_audio = active);
        }
    });
    let c = Rc::downgrade(controls);
    controls.sound.connect_active_notify(move |switch| {
        if let Some(c) = c.upgrade() {
            let active = switch.is_active();
            c.write(|s| s.sound_on_commit = active);
        }
    });

    let appearance = controls.appearance.clone();
    let selection = controls.selection.clone();
    let font_scale = controls.font_scale.clone();
    let font_face = controls.font_face.clone();
    let truncation = controls.truncation.clone();
    let sort = controls.sort.clone();
    let shelf = controls.shelf.clone();
    let display = controls.display.clone();
    bind_enum(controls, &appearance, Appearance::ALL, |s, v| {
        s.appearance = v
    });
    bind_enum(controls, &selection, SelectionColor::ALL, |s, v| {
        s.selection_color = v
    });
    bind_enum(controls, &font_scale, FontScale::ALL, |s, v| {
        s.font_scale = v
    });
    bind_enum(controls, &font_face, FontFace::ALL, |s, v| s.font_face = v);
    bind_enum(controls, &truncation, Truncation::ALL, |s, v| {
        s.title_truncation = v
    });
    bind_enum(controls, &sort, SortOrder::ALL, |s, v| s.sort_order = v);
    bind_enum(controls, &shelf, Shelf::ALL, |s, v| s.window_shelf = v);
    bind_enum(controls, &display, DisplayMode::ALL, |s, v| {
        s.display_mode = v
    });

    let c = Rc::downgrade(controls);
    controls.selection_hex.connect_changed(move |entry| {
        if let Some(c) = c.upgrade() {
            let text = entry.text().to_string();
            c.write(|s| s.selection_color_hex = text);
        }
    });
}

fn bind_enum<T>(
    controls: &Rc<Controls>,
    dropdown: &gtk::DropDown,
    all: &'static [T],
    set: fn(&mut Settings, T),
) where
    T: Copy + 'static,
{
    let c = Rc::downgrade(controls);
    dropdown.connect_selected_notify(move |dropdown| {
        let Some(c) = c.upgrade() else {
            return;
        };
        let Some(value) = all.get(dropdown.selected() as usize).copied() else {
            return;
        };
        c.write(|s| set(s, value));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_choices_do_not_count_as_advanced() {
        let mut s = Settings {
            layout: Layout::Grid,
            panel_scale: 80,
            show_application_names: true,
            ..Default::default()
        };
        assert!(!customized(&s));
        s.fuzzy_search = true;
        assert!(customized(&s));
    }

    #[test]
    fn restore_keeps_the_page_applications_and_shortcuts() {
        let mut s = Settings {
            layout: Layout::List,
            fuzzy_search: true,
            reveal_delay_ms: 200,
            ..Default::default()
        };
        s.rules.push(AppRule {
            id: "foot.desktop".into(),
            pinned: true,
            hidden: false,
            skip_fullscreen_only: None,
        });
        s.scoped.push(ScopedShortcut {
            key: "W-Tab".into(),
            scope: Scope::All,
        });
        restore_advanced(&mut s);
        assert_eq!(s.layout, Layout::List);
        assert!(!s.fuzzy_search);
        assert_eq!(s.reveal_delay_ms, Settings::default().reveal_delay_ms);
        assert_eq!(s.rules.len(), 1);
        assert_eq!(s.scoped.len(), 1);
    }
}
