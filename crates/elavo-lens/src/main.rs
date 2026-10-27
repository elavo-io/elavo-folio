//! Lens: Super+Space search for applications and quick calculations,
//! with an Applications section that browses every installed app.
//! Spec: elavo-architecture-and-planning-internal, planning/desktop/lens.md.

mod calc;

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::{Rc, Weak};

use elavo_common::{AppEntry, AppIndex};
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

const APP_ID: &str = "io.elavo.Lens";
const STYLE: &str = include_str!("style.css");
const WIDTH: i32 = 680;
const MAX_APPLICATIONS: usize = 8;
const GRID_ICON: i32 = 56;
const GRID_COLUMNS: u32 = 6;

#[derive(Clone)]
enum Action {
    Launch(AppEntry),
    Copy(String),
}

/// What System Settings' Lens pane chose in `elavo/lens.conf`: the
/// Applications folders (the default) or every installed app. The shared
/// folder holds the system's applications, so without it Lens lists every
/// installed app rather than only this account's own entries.
fn load_applications() -> AppIndex {
    let file = glib::KeyFile::new();
    let path = glib::user_config_dir().join("elavo/lens.conf");
    let all = file
        .load_from_file(&path, glib::KeyFileFlags::NONE)
        .ok()
        .and_then(|()| file.string("Lens", "Applications").ok())
        .is_some_and(|v| v.trim().eq_ignore_ascii_case("all"));
    if all || !elavo_common::apps::shared_applications_dir().is_dir() {
        AppIndex::load()
    } else {
        AppIndex::load_applications_folders()
    }
}

/// x, y, width, height, corner radius, in window coordinates.
type BlurPart = (i32, i32, i32, i32, i32);

struct Lens {
    window: gtk::ApplicationWindow,
    column: gtk::Box,
    bar: gtk::Box,
    blur: RefCell<Option<elavo_common::blur::BackgroundBlur>>,
    blur_parts: RefCell<Vec<BlurPart>>,
    entry: gtk::Entry,
    apps_button: gtk::ToggleButton,
    results: gtk::Box,
    list: gtk::ListBox,
    list_scroller: gtk::ScrolledWindow,
    grid: gtk::FlowBox,
    grid_scroller: gtk::ScrolledWindow,
    apps: RefCell<Vec<AppEntry>>,
    /// Action for each list row, by row index. Headings have none.
    actions: RefCell<Vec<Option<Action>>>,
    /// An application is being dragged out, to the Dock.
    dragging: Cell<bool>,
    me: Weak<Lens>,
}

impl Lens {
    fn new(app: &gtk::Application) -> Rc<Self> {
        let display = gdk::Display::default().expect("no display");
        let css = gtk::CssProvider::new();
        css.load_from_string(STYLE);
        gtk::style_context_add_provider_for_display(
            &display,
            &css,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );

        let window = gtk::ApplicationWindow::builder()
            .application(app)
            .title("Lens")
            .decorated(false)
            .css_classes(["elavo-lens"])
            .build();
        window.init_layer_shell();
        window.set_namespace(Some("elavo-lens"));
        window.set_layer(Layer::Overlay);
        for edge in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
            window.set_anchor(edge, true);
        }
        window.set_keyboard_mode(KeyboardMode::Exclusive);

        let entry = gtk::Entry::builder()
            .placeholder_text("Search")
            .hexpand(true)
            .build();
        let apps_button = gtk::ToggleButton::builder()
            .icon_name("view-app-grid-symbolic")
            .tooltip_text("Applications (Ctrl+1)")
            .valign(gtk::Align::Center)
            .focusable(false)
            .build();
        let bar = gtk::Box::builder()
            .css_classes(["elavo-lens-bar"])
            .spacing(10)
            .build();
        bar.append(
            &gtk::Image::builder()
                .icon_name("system-search-symbolic")
                .pixel_size(22)
                .build(),
        );
        bar.append(&entry);
        bar.append(&apps_button);

        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::Single)
            .activate_on_single_click(true)
            .build();
        let list_scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(520)
            .child(&list)
            .build();

        let grid = gtk::FlowBox::builder()
            .css_classes(["elavo-lens-grid"])
            .homogeneous(true)
            .min_children_per_line(GRID_COLUMNS)
            .max_children_per_line(GRID_COLUMNS)
            .selection_mode(gtk::SelectionMode::Single)
            .activate_on_single_click(true)
            .valign(gtk::Align::Start)
            .build();
        let grid_scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .min_content_height(460)
            .child(&grid)
            .build();

        let results = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .css_classes(["elavo-lens-results"])
            .visible(false)
            .build();
        results.append(&list_scroller);
        results.append(&grid_scroller);

        let column = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .css_classes(["elavo-lens-column"])
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Start)
            .width_request(WIDTH)
            .build();
        column.append(&bar);
        column.append(&results);

        let root = gtk::Box::builder().hexpand(true).vexpand(true).build();
        column.set_hexpand(true);
        root.append(&column);
        window.set_child(Some(&root));

        let lens = Rc::new_cyclic(|me| Self {
            me: me.clone(),
            window,
            column,
            bar,
            blur: RefCell::new(None),
            blur_parts: RefCell::default(),
            entry,
            apps_button,
            results,
            list,
            list_scroller,
            grid,
            grid_scroller,
            apps: RefCell::default(),
            actions: RefCell::default(),
            dragging: Cell::new(false),
        });
        lens.connect(&root);
        lens.sync_blur();
        lens
    }

    /// Blur the bar, and the results card when it is open. The fullscreen
    /// layer around them stays clear. The panel does not use this.
    fn sync_blur(&self) {
        let mut parts = Vec::new();
        let bar = if self.dragging.get() {
            None
        } else {
            self.bar.compute_bounds(&self.window)
        };
        if let Some(bounds) = bar {
            parts.push((
                bounds.x().round() as i32,
                bounds.y().round() as i32,
                bounds.width().round().max(0.0) as i32,
                bounds.height().round().max(0.0) as i32,
                27,
            ));
        }
        if self.results.is_visible() && !self.dragging.get() {
            if let Some(bounds) = self.results.compute_bounds(&self.window) {
                parts.push((
                    bounds.x().round() as i32,
                    bounds.y().round() as i32,
                    bounds.width().round().max(0.0) as i32,
                    bounds.height().round().max(0.0) as i32,
                    22,
                ));
            }
        }
        if self.blur.borrow().is_none() {
            if let Some(blur) =
                elavo_common::blur::BackgroundBlur::for_widget(self.window.upcast_ref())
            {
                *self.blur.borrow_mut() = Some(blur);
            }
        }
        if let Some(blur) = self.blur.borrow().as_ref() {
            if *self.blur_parts.borrow() == parts {
                return;
            }
            blur.set_rounds(&parts);
            *self.blur_parts.borrow_mut() = parts;
            // The region applies on the next surface commit.
            self.window.queue_draw();
        }
    }

    fn connect(self: &Rc<Self>, root: &gtk::Box) {
        let weak = Rc::downgrade(self);
        self.window.connect_realize(move |_| {
            if let Some(lens) = weak.upgrade() {
                lens.sync_blur();
            }
        });
        // Bounds are only known after layout, which runs after the calls
        // above. Runs only while mapped; unchanged parts are not resent.
        let weak = Rc::downgrade(self);
        self.window
            .add_tick_callback(move |_, _| match weak.upgrade() {
                Some(lens) => {
                    lens.sync_blur();
                    glib::ControlFlow::Continue
                }
                None => glib::ControlFlow::Break,
            });
        let weak = Rc::downgrade(self);
        self.entry.connect_changed(move |_| {
            if let Some(s) = weak.upgrade() {
                s.refresh();
            }
        });

        let weak = Rc::downgrade(self);
        self.entry.connect_activate(move |_| {
            if let Some(s) = weak.upgrade() {
                s.activate_selection();
            }
        });

        let weak = Rc::downgrade(self);
        self.apps_button.connect_toggled(move |_| {
            if let Some(s) = weak.upgrade() {
                s.refresh();
                s.entry.grab_focus();
            }
        });

        let weak = Rc::downgrade(self);
        self.list.connect_row_activated(move |_, row| {
            if let Some(s) = weak.upgrade() {
                s.run_row(row);
            }
        });

        let weak = Rc::downgrade(self);
        self.grid.set_filter_func(move |child| {
            let Some(s) = weak.upgrade() else {
                return true;
            };
            let query = s.entry.text();
            s.grid_app(child).is_some_and(|app| app.matches(&query))
        });

        let weak = Rc::downgrade(self);
        self.grid.connect_child_activated(move |_, child| {
            if let Some(s) = weak.upgrade() {
                if let Some(app) = s.grid_app(child) {
                    s.run(&Action::Launch(app));
                }
            }
        });

        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, modifiers| {
            let Some(s) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            s.on_key(key, modifiers)
        });
        self.window.add_controller(keys);

        let click = gtk::GestureClick::new();
        let weak = Rc::downgrade(self);
        let root_ref = root.downgrade();
        click.connect_pressed(move |_, _, x, y| {
            let (Some(s), Some(root)) = (weak.upgrade(), root_ref.upgrade()) else {
                return;
            };
            let inside = s
                .column
                .compute_bounds(&root)
                .is_some_and(|b| b.contains_point(&gtk::graphene::Point::new(x as f32, y as f32)));
            if !inside {
                s.hide();
            }
        });
        root.add_controller(click);
    }

    fn on_key(&self, key: gdk::Key, modifiers: gdk::ModifierType) -> glib::Propagation {
        let apps_mode = self.apps_button.is_active();
        match key {
            gdk::Key::Escape => {
                if !self.entry.text().is_empty() {
                    self.entry.set_text("");
                } else if apps_mode {
                    self.apps_button.set_active(false);
                } else {
                    self.hide();
                }
                glib::Propagation::Stop
            }
            gdk::Key::_1 if modifiers.contains(gdk::ModifierType::CONTROL_MASK) => {
                self.apps_button.set_active(!apps_mode);
                glib::Propagation::Stop
            }
            gdk::Key::Down | gdk::Key::Up if !apps_mode => {
                self.move_selection(if key == gdk::Key::Down { 1 } else { -1 });
                glib::Propagation::Stop
            }
            gdk::Key::Down if apps_mode && self.entry.has_focus() => {
                if let Some(child) = self.first_visible_grid_child() {
                    self.grid.select_child(&child);
                    child.grab_focus();
                }
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        }
    }

    fn grid_app(&self, child: &gtk::FlowBoxChild) -> Option<AppEntry> {
        let id = child.child()?.widget_name();
        self.apps
            .borrow()
            .iter()
            .find(|a| a.id == id.as_str())
            .cloned()
    }

    fn first_visible_grid_child(&self) -> Option<gtk::FlowBoxChild> {
        let mut child = self.grid.first_child();
        while let Some(widget) = child {
            if let Some(fb) = widget.downcast_ref::<gtk::FlowBoxChild>() {
                if fb.is_child_visible() {
                    return Some(fb.clone());
                }
            }
            child = widget.next_sibling();
        }
        None
    }

    fn populate_grid(&self) {
        while let Some(child) = self.grid.first_child() {
            self.grid.remove(&child);
        }
        for app in self.apps.borrow().iter() {
            let tile = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(6)
                .name(app.id.as_str())
                .tooltip_text(app.name.as_str())
                .build();
            tile.append(&app_image(app, GRID_ICON));
            tile.add_controller(self.drag_source(app));
            tile.append(
                &gtk::Label::builder()
                    .label(app.name.as_str())
                    .ellipsize(gtk::pango::EllipsizeMode::End)
                    .max_width_chars(12)
                    .justify(gtk::Justification::Center)
                    .build(),
            );
            self.grid.append(&tile);
        }
    }

    fn heading(&self, text: &str) {
        let row = gtk::ListBoxRow::builder()
            .selectable(false)
            .activatable(false)
            .css_classes(["elavo-lens-heading"])
            .child(&gtk::Label::builder().label(text).xalign(0.0).build())
            .build();
        self.list.append(&row);
        self.actions.borrow_mut().push(None);
    }

    fn row(&self, icon: gtk::Image, title: &gtk::Label, subtitle: Option<&str>, action: Action) {
        let text = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .valign(gtk::Align::Center)
            .build();
        text.append(title);
        if let Some(subtitle) = subtitle {
            text.append(
                &gtk::Label::builder()
                    .label(subtitle)
                    .xalign(0.0)
                    .ellipsize(gtk::pango::EllipsizeMode::End)
                    .css_classes(["elavo-lens-subtitle"])
                    .build(),
            );
        }
        let line = gtk::Box::builder().spacing(12).build();
        line.append(&icon);
        line.append(&text);
        self.list
            .append(&gtk::ListBoxRow::builder().child(&line).build());
        self.actions.borrow_mut().push(Some(action));
    }

    fn app_row(&self, app: &AppEntry) {
        let title = gtk::Label::builder()
            .label(app.name.as_str())
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["elavo-lens-title"])
            .build();
        self.row(
            app_image(app, 32),
            &title,
            app.generic_name.as_deref(),
            Action::Launch(app.clone()),
        );
        if let Some(row) = self.list.last_child() {
            row.add_controller(self.drag_source(app));
        }
    }

    /// Dragging an application out of Lens offers its Applications entry,
    /// or its desktop file, as a link; the Dock pins it.
    fn drag_source(&self, app: &AppEntry) -> gtk::DragSource {
        let source = gtk::DragSource::builder()
            .actions(gdk::DragAction::LINK | gdk::DragAction::COPY)
            .build();
        let file = app.info.filename().map(|path| {
            match path
                .parent()
                .filter(|p| p.extension().is_some_and(|e| e == "eapp"))
            {
                Some(bundle) => gio::File::for_path(bundle),
                None => gio::File::for_path(&path),
            }
        });
        let Some(file) = file else {
            return source;
        };
        let list = gdk::FileList::from_array(&[file]);
        source.set_content(Some(&gdk::ContentProvider::for_value(&list.to_value())));
        let icon = app.icon();
        let lens = self.me.clone();
        source.connect_drag_begin(move |source, _| {
            let Some(lens) = lens.upgrade() else {
                return;
            };
            // The tile is about to be hidden, so the drag icon is drawn from
            // the theme rather than from the widget.
            if let Some(icon) = icon.as_ref() {
                let theme = gtk::IconTheme::for_display(&WidgetExt::display(&lens.window));
                let paintable = theme.lookup_by_gicon(
                    icon,
                    GRID_ICON,
                    lens.window.scale_factor(),
                    gtk::TextDirection::None,
                    gtk::IconLookupFlags::empty(),
                );
                source.set_icon(Some(&paintable), GRID_ICON / 2, GRID_ICON / 2);
            }
            lens.step_aside();
        });
        let lens = self.me.clone();
        source.connect_drag_end(move |_, _, _| {
            if let Some(lens) = lens.upgrade() {
                lens.hide();
            }
        });
        source
    }

    /// Lens covers the screen above the Dock. While an application is
    /// dragged out of it, Lens draws nothing, takes no input, and sits below
    /// the Dock's layer, so the drop reaches the Dock.
    fn step_aside(&self) {
        self.dragging.set(true);
        self.column.set_opacity(0.0);
        self.sync_blur();
        if let Some(surface) = self.window.surface() {
            surface.set_input_region(Some(&gtk::cairo::Region::create()));
        }
        self.window.set_keyboard_mode(KeyboardMode::None);
        self.window.set_layer(Layer::Bottom);
    }

    /// Undoes [`Lens::step_aside`] while Lens is hidden.
    fn come_back(&self) {
        if !self.dragging.replace(false) {
            return;
        }
        self.column.set_opacity(1.0);
        if let Some(surface) = self.window.surface() {
            surface.set_input_region(None);
        }
        self.window.set_layer(Layer::Overlay);
        self.window.set_keyboard_mode(KeyboardMode::Exclusive);
    }

    fn refresh(&self) {
        let apps_mode = self.apps_button.is_active();
        self.list_scroller.set_visible(!apps_mode);
        self.grid_scroller.set_visible(apps_mode);
        if apps_mode {
            self.grid.invalidate_filter();
            self.grid.unselect_all();
            self.results.set_visible(true);
            self.sync_blur();
            return;
        }

        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        self.actions.borrow_mut().clear();

        let query = self.entry.text();
        if query.trim().is_empty() {
            self.results.set_visible(false);
            self.sync_blur();
            return;
        }

        if let Some(answer) = calc::evaluate(&query) {
            self.heading("Calculator");
            let title = gtk::Label::builder()
                .label(format!("= {answer}"))
                .xalign(0.0)
                .selectable(false)
                .css_classes(["elavo-lens-calc"])
                .build();
            let icon = gtk::Image::builder()
                .icon_name("accessories-calculator-symbolic")
                .pixel_size(32)
                .build();
            self.row(
                icon,
                &title,
                Some("Press Return to copy"),
                Action::Copy(answer),
            );
        }

        let ranked: Vec<AppEntry> = {
            let apps = self.apps.borrow();
            let mut scored: Vec<(u32, &AppEntry)> = apps
                .iter()
                .filter_map(|a| a.score(&query).map(|s| (s, a)))
                .collect();
            scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.name.cmp(&b.1.name)));
            scored
                .into_iter()
                .take(MAX_APPLICATIONS + 1)
                .map(|(_, a)| a.clone())
                .collect()
        };
        if let Some((top, rest)) = ranked.split_first() {
            self.heading("Top Hit");
            self.app_row(top);
            if !rest.is_empty() {
                self.heading("Applications");
                for app in rest {
                    self.app_row(app);
                }
            }
        }

        if self.actions.borrow().is_empty() {
            self.heading("No Results");
        }
        self.results.set_visible(true);
        self.move_selection(0);
        self.sync_blur();
    }

    /// Select the next selectable row in `step` direction; 0 selects the first.
    fn move_selection(&self, step: i32) {
        let actions = self.actions.borrow();
        let count = actions.len() as i32;
        let current = self.list.selected_row().map(|r| r.index());
        let mut index = match (current, step) {
            (Some(i), s) if s != 0 => i + s,
            _ => 0,
        };
        let direction = if step < 0 { -1 } else { 1 };
        while (0..count).contains(&index) {
            if actions[index as usize].is_some() {
                if let Some(row) = self.list.row_at_index(index) {
                    self.list.select_row(Some(&row));
                }
                return;
            }
            index += direction;
        }
    }

    fn activate_selection(&self) {
        if self.apps_button.is_active() {
            let child = self
                .grid
                .selected_children()
                .into_iter()
                .next()
                .or_else(|| self.first_visible_grid_child());
            if let Some(app) = child.and_then(|c| self.grid_app(&c)) {
                self.run(&Action::Launch(app));
            }
        } else if let Some(row) = self.list.selected_row() {
            self.run_row(&row);
        }
    }

    fn run_row(&self, row: &gtk::ListBoxRow) {
        let action = self
            .actions
            .borrow()
            .get(row.index() as usize)
            .cloned()
            .flatten();
        if let Some(action) = action {
            self.run(&action);
        }
    }

    fn run(&self, action: &Action) {
        match action {
            Action::Launch(app) => elavo_common::launch(&app.info, None),
            Action::Copy(text) => WidgetExt::display(&self.window).clipboard().set_text(text),
        }
        self.hide();
    }

    fn show(&self, applications: bool) {
        self.apps
            .replace(load_applications().shown().cloned().collect());
        self.populate_grid();
        self.entry.set_text("");
        if self.apps_button.is_active() == applications {
            self.refresh();
        } else {
            self.apps_button.set_active(applications);
        }
        self.window.present();
        self.entry.grab_focus();
    }

    fn hide(&self) {
        self.window.set_visible(false);
        self.come_back();
    }

    fn toggle(&self, applications: bool) {
        if !self.window.is_visible() {
            self.show(applications);
        } else if applications && !self.apps_button.is_active() {
            self.apps_button.set_active(true);
        } else {
            self.hide();
        }
    }
}

fn app_image(app: &AppEntry, size: i32) -> gtk::Image {
    let image = gtk::Image::builder().pixel_size(size).build();
    match app.icon() {
        Some(icon) => image.set_from_gicon(&icon),
        None => image.set_icon_name(Some("application-x-executable")),
    }
    image
}

fn main() -> glib::ExitCode {
    let app = gtk::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();

    let lens: Rc<OnceCell<Rc<Lens>>> = Rc::default();
    app.connect_command_line(move |app, cmdline| {
        let lens = lens.get_or_init(|| {
            std::mem::forget(app.hold());
            Lens::new(app)
        });
        let args = cmdline.arguments();
        let has = |flag: &str| args.iter().any(|a| a == flag);
        if has("--toggle") {
            lens.toggle(false);
        } else if has("--applications") {
            if lens.window.is_visible() {
                lens.hide();
            } else {
                lens.show(true);
            }
        }
        glib::ExitCode::SUCCESS
    });
    app.run()
}
