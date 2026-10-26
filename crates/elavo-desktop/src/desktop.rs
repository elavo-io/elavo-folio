//! The layer-shell surface, file and volume watchers, and the panel's D-Bus API.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use gtk::gdk::Key;
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

use crate::area::Area;
use crate::model::{
    available_name, new_folder_name, show_mount, Dir, FileRec, GroupBy, Id, Item, Kind, Layout,
    Model, MountFacts, MountRec, Slot, Times, CELL_H, CELL_W,
};
use crate::thumbs::Thumbnails;

const BUS_PATH: &str = "/io/elavo/Desktop";
const BUS_IFACE: &str = "io.elavo.Desktop";
const DRAG_SLOP: f64 = 8.0;
const ICON_SIZE: i32 = 48;
const ICON_MIME: &str = "application/x-elavo-desktop-icons";

const FILE_OPS_BUS: &str = "org.gnome.Nautilus";
const FILE_OPS_PATH: &str = "/org/gnome/Nautilus/FileOperations2";
const FILE_OPS_IFACE: &str = "org.gnome.Nautilus.FileOperations2";
const PREVIEW_BUS: &str = "org.gnome.NautilusPreviewer";
const PREVIEW_PATH: &str = "/org/gnome/NautilusPreviewer";
const PREVIEW_IFACE: &str = "org.gnome.NautilusPreviewer2";
const FILE_MANAGER_BUS: &str = "org.freedesktop.FileManager1";
const FILE_MANAGER_PATH: &str = "/org/freedesktop/FileManager1";

const IFACE_XML: &str = r#"
<node>
  <interface name="io.elavo.Desktop">
    <method name="Activate">
      <arg type="s" name="action" direction="in"/>
    </method>
    <method name="Enabled">
      <arg type="as" name="actions" direction="in"/>
      <arg type="ab" name="enabled" direction="out"/>
    </method>
    <method name="Stacks">
      <arg type="b" name="on" direction="out"/>
    </method>
    <signal name="Changed">
      <arg type="b" name="stacks"/>
    </signal>
    <signal name="Focused">
      <arg type="b" name="focused"/>
    </signal>
  </interface>
</node>
"#;

struct Placed {
    id: Id,
    root: gtk::Box,
    label: gtk::Label,
    origin_x: f64,
    origin_y: f64,
}

/// Where a dragged icon was released, relative to where it started.
#[derive(Clone, Copy, Default)]
struct Drag {
    delta: (f64, f64),
    pointer: (f64, f64),
}

pub struct Desktop {
    app: gtk::Application,
    model: RefCell<Model>,
    windows: RefCell<Vec<gtk::ApplicationWindow>>,
    icon_window: RefCell<Option<gtk::ApplicationWindow>>,
    fixed: RefCell<Option<gtk::Fixed>>,
    rubber: RefCell<Option<gtk::Box>>,
    icons: RefCell<Vec<Placed>>,
    actions: gio::SimpleActionGroup,
    stacks_action: gio::SimpleAction,
    group_action: gio::SimpleAction,
    conn: RefCell<Option<gio::DBusConnection>>,
    dir_monitor: RefCell<Option<gio::FileMonitor>>,
    volumes: gio::VolumeMonitor,
    mount_icons: RefCell<HashMap<String, gio::Icon>>,
    desktop_dir: PathBuf,
    layout_path: PathBuf,
    layout_written: RefCell<String>,
    renaming: RefCell<Option<String>>,
    rename_entry: RefCell<Option<gtk::Entry>>,
    rename_after: RefCell<Option<String>>,
    reload_queued: Cell<bool>,
    reload_gen: Cell<u64>,
    ready: Cell<bool>,
    layout_loaded: Cell<bool>,
    suppress_click: Cell<bool>,
    notify_gen: Cell<u64>,
    can_paste: Cell<bool>,
    popover: RefCell<Option<gtk::PopoverMenu>>,
    dragging: Cell<bool>,
    rubber_origin: Cell<(f64, f64)>,
    height: Cell<i32>,
    thumbs: RefCell<Option<Rc<Thumbnails>>>,
    /// Cells chosen at a drop, held until the copied files are listed.
    dropped: RefCell<HashMap<String, Slot>>,
    space_held: Cell<bool>,
    focused: Cell<bool>,
    focus_gen: Cell<u64>,
    rebuilding: Cell<bool>,
    rebuild_again: Cell<bool>,
    /// The icon a drag started on, and where in it the pointer grabbed it.
    drag_icon: RefCell<Option<Id>>,
    drag_grab: Cell<(f64, f64)>,
    /// The folder or volume a dragged icon is over.
    drop_target: RefCell<Option<Id>>,
}

impl Desktop {
    pub fn start(app: &gtk::Application) -> Rc<Self> {
        if let Some(display) = gdk::Display::default() {
            let css = gtk::CssProvider::new();
            css.load_from_resource("/io/elavo/Desktop/style.css");
            gtk::style_context_add_provider_for_display(
                &display,
                &css,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }

        let desktop_dir = desktop_directory();
        let layout_path = glib::user_data_dir().join("elavo").join("desktop-layout");
        if let Some(parent) = layout_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let stacks_action =
            gio::SimpleAction::new_stateful("use-stacks", None, &false.to_variant());
        let group_action = gio::SimpleAction::new_stateful(
            "group-by",
            Some(glib::VariantTy::STRING),
            &GroupBy::Kind.id().to_variant(),
        );
        let desktop = Rc::new(Self {
            app: app.clone(),
            model: RefCell::new(Model::new(4)),
            windows: RefCell::new(Vec::new()),
            icon_window: RefCell::new(None),
            fixed: RefCell::new(None),
            rubber: RefCell::new(None),
            icons: RefCell::new(Vec::new()),
            actions: gio::SimpleActionGroup::new(),
            stacks_action,
            group_action,
            conn: RefCell::new(None),
            dir_monitor: RefCell::new(None),
            volumes: gio::VolumeMonitor::get(),
            mount_icons: RefCell::new(HashMap::new()),
            desktop_dir,
            layout_path,
            layout_written: RefCell::new(String::new()),
            renaming: RefCell::new(None),
            rename_entry: RefCell::new(None),
            rename_after: RefCell::new(None),
            reload_queued: Cell::new(false),
            reload_gen: Cell::new(0),
            ready: Cell::new(false),
            layout_loaded: Cell::new(false),
            suppress_click: Cell::new(false),
            notify_gen: Cell::new(0),
            can_paste: Cell::new(false),
            popover: RefCell::new(None),
            dragging: Cell::new(false),
            rubber_origin: Cell::new((0.0, 0.0)),
            height: Cell::new(800),
            thumbs: RefCell::new(None),
            dropped: RefCell::new(HashMap::new()),
            space_held: Cell::new(false),
            focused: Cell::new(false),
            focus_gen: Cell::new(0),
            rebuilding: Cell::new(false),
            rebuild_again: Cell::new(false),
            drag_icon: RefCell::new(None),
            drag_grab: Cell::new((0.0, 0.0)),
            drop_target: RefCell::new(None),
        });

        let weak = Rc::downgrade(&desktop);
        *desktop.thumbs.borrow_mut() = Some(Thumbnails::new(move || {
            if let Some(desktop) = weak.upgrade() {
                desktop.schedule_reload();
            }
        }));
        desktop.install_actions();
        desktop.export_bus(app);
        desktop.watch_volumes();
        desktop.watch_clipboard();
        desktop.rebuild_windows();

        let weak = Rc::downgrade(&desktop);
        if let Some(display) = gdk::Display::default() {
            display.monitors().connect_items_changed(move |_, _, _, _| {
                if let Some(desktop) = weak.upgrade() {
                    desktop.rebuild_windows();
                }
            });
        }
        desktop
    }

    fn install_actions(self: &Rc<Self>) {
        for name in [
            "new-folder",
            "open",
            "get-info",
            "rename",
            "quick-look",
            "move-to-trash",
            "cut",
            "copy",
            "paste",
            "select-all",
            "unmount",
            "change-wallpaper",
        ] {
            let action = gio::SimpleAction::new(name, None);
            let desktop = Rc::clone(self);
            action.connect_activate(move |_, _| desktop.activate(name));
            self.actions.add_action(&action);
        }
        let desktop = Rc::clone(self);
        self.stacks_action
            .connect_activate(move |_, _| desktop.activate("use-stacks"));
        self.actions.add_action(&self.stacks_action);

        let desktop = Rc::clone(self);
        self.group_action.connect_activate(move |_, target| {
            let Some(group) = target
                .and_then(|value| value.get::<String>())
                .and_then(|id| GroupBy::parse(&id))
            else {
                return;
            };
            desktop.set_group_by(group);
        });
        self.actions.add_action(&self.group_action);

        let open_with = gio::SimpleAction::new("open-with", Some(glib::VariantTy::STRING));
        let desktop = Rc::clone(self);
        open_with.connect_activate(move |_, target| {
            if let Some(app_id) = target.and_then(|value| value.get::<String>()) {
                desktop.open_with(&app_id);
            }
        });
        self.actions.add_action(&open_with);
    }

    fn export_bus(self: &Rc<Self>, app: &gtk::Application) {
        let Some(conn) = app.dbus_connection() else {
            glib::g_warning!("elavo-desktop", "no session bus connection");
            return;
        };
        let node = Box::leak(Box::new(
            gio::DBusNodeInfo::for_xml(IFACE_XML).expect("desktop bus interface"),
        ));
        let Some(interface) = node.lookup_interface(BUS_IFACE) else {
            return;
        };
        let interface = Box::leak(Box::new(interface));
        let desktop = Rc::clone(self);
        if let Err(err) = conn
            .register_object(BUS_PATH, interface)
            .method_call(move |_c, _s, _p, _i, method, params, invocation| {
                desktop.handle_bus(method, &params, invocation);
            })
            .build()
        {
            glib::g_warning!("elavo-desktop", "bus export: {err}");
            return;
        }
        *self.conn.borrow_mut() = Some(conn);
    }

    fn handle_bus(
        self: &Rc<Self>,
        method: &str,
        params: &glib::Variant,
        invocation: gio::DBusMethodInvocation,
    ) {
        match method {
            "Activate" => {
                let action = params.child_value(0).get::<String>().unwrap_or_default();
                self.activate(&action);
                invocation.return_value(None);
            }
            "Enabled" => {
                let list = params.child_value(0);
                let can_paste = self.can_paste.get();
                let mut flags = Vec::with_capacity(list.n_children());
                for index in 0..list.n_children() {
                    let name = list.child_value(index).get::<String>().unwrap_or_default();
                    flags.push(self.model.borrow().command_enabled(&name, can_paste));
                }
                invocation.return_value(Some(&(flags,).to_variant()));
            }
            "Stacks" => {
                let on = self.model.borrow().stacks();
                invocation.return_value(Some(&(on,).to_variant()));
            }
            _ => invocation
                .return_dbus_error("org.freedesktop.DBus.Error.UnknownMethod", "unknown method"),
        }
    }

    fn watch_volumes(self: &Rc<Self>) {
        let added = Rc::downgrade(self);
        self.volumes.connect_mount_added(move |_, _| {
            if let Some(desktop) = added.upgrade() {
                desktop.schedule_reload();
            }
        });
        let removed = Rc::downgrade(self);
        self.volumes.connect_mount_removed(move |_, _| {
            if let Some(desktop) = removed.upgrade() {
                desktop.schedule_reload();
            }
        });
        let changed = Rc::downgrade(self);
        self.volumes.connect_mount_changed(move |_, _| {
            if let Some(desktop) = changed.upgrade() {
                desktop.schedule_reload();
            }
        });

        // Nothing else in the session mounts drives; on GNOME the shell does.
        if automount_enabled() {
            self.volumes
                .connect_volume_added(|_, volume| automount(volume));
            for volume in self.volumes.volumes() {
                automount(&volume);
            }
        }
    }

    fn watch_clipboard(self: &Rc<Self>) {
        let Some(display) = gdk::Display::default() else {
            return;
        };
        let clipboard = display.clipboard();
        let desktop = Rc::clone(self);
        clipboard.connect_changed(move |clipboard| {
            desktop.can_paste.set(clipboard_has_files(clipboard));
            desktop.queue_changed();
        });
        self.can_paste.set(clipboard_has_files(&clipboard));
    }

    fn rebuild_windows(self: &Rc<Self>) {
        let old = std::mem::take(&mut *self.windows.borrow_mut());
        for window in old {
            window.destroy();
        }
        self.fixed.borrow_mut().take();
        self.rubber.borrow_mut().take();
        self.icons.borrow_mut().clear();
        self.icon_window.borrow_mut().take();

        let Some(display) = gdk::Display::default() else {
            return;
        };
        let monitors = display.monitors();
        let mut icon_index: Option<u32> = None;
        let mut best: Option<(i32, i32, String)> = None;
        for index in 0..monitors.n_items() {
            let Some(monitor) = monitors.item(index).and_downcast::<gdk::Monitor>() else {
                continue;
            };
            let geo = monitor.geometry();
            let connector = monitor
                .connector()
                .map(|value| value.to_string())
                .unwrap_or_default();
            let rank = (geo.x(), geo.y(), connector);
            let take = best.as_ref().is_none_or(|found| {
                (&rank.0, &rank.1, rank.2.as_str()) < (&found.0, &found.1, found.2.as_str())
            });
            if take {
                best = Some(rank);
                icon_index = Some(index);
            }
        }

        let app = self.app.clone();
        let mut windows = Vec::new();
        for index in 0..monitors.n_items() {
            let Some(monitor) = monitors.item(index).and_downcast::<gdk::Monitor>() else {
                continue;
            };
            windows.push(self.build_window(&app, &monitor, icon_index == Some(index)));
        }
        *self.windows.borrow_mut() = windows;
        if self.ready.get() {
            self.rebuild();
            self.set_keyboard(self.renaming.borrow().is_some());
        }
    }

    fn build_window(
        self: &Rc<Self>,
        app: &gtk::Application,
        monitor: &gdk::Monitor,
        icon_monitor: bool,
    ) -> gtk::ApplicationWindow {
        let window = gtk::ApplicationWindow::builder()
            .application(app)
            .title(if icon_monitor {
                "Elavo Desktop"
            } else {
                "Elavo Desktop Extra"
            })
            .decorated(false)
            .css_classes(["elavo-desktop"])
            .build();
        window.init_layer_shell();
        window.set_namespace(Some("elavo-desktop"));
        // Bottom sits above swaybg's background layer and below windows.
        window.set_layer(Layer::Bottom);
        window.set_monitor(Some(monitor));
        for edge in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
            window.set_anchor(edge, true);
        }
        window.set_exclusive_zone(0);
        window.set_keyboard_mode(KeyboardMode::OnDemand);
        window.insert_action_group("desk", Some(&self.actions));

        let fixed = gtk::Fixed::new();
        fixed.set_hexpand(true);
        fixed.set_vexpand(true);
        let backdrop = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        backdrop.add_css_class("elavo-desktop-hit");
        backdrop.set_can_focus(true);
        fixed.put(&backdrop, 0.0, 0.0);
        let rubber = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        rubber.add_css_class("elavo-desktop-rubber");
        rubber.set_can_target(false);
        rubber.set_visible(false);
        fixed.put(&rubber, 0.0, 0.0);
        window.set_child(Some(&Area::new(&fixed)));

        let keys = gtk::EventControllerKey::new();
        let desktop = Rc::clone(self);
        keys.connect_key_pressed(move |_, key, _, state| desktop.on_key(key, state));
        let desktop = Rc::clone(self);
        keys.connect_key_released(move |_, key, _, _| {
            if key == Key::space {
                desktop.space_held.set(false);
            }
        });
        window.add_controller(keys);

        let desktop = Rc::downgrade(self);
        window.connect_is_active_notify(move |window| {
            if let Some(desktop) = desktop.upgrade() {
                // Clicking a window leaves the name field without a focus
                // change inside this surface.
                if icon_monitor && !window.is_active() {
                    desktop.after_rename(|_| {});
                }
                desktop.sync_focus();
            }
        });

        let formats = gdk::ContentFormatsBuilder::new()
            .add_mime_type(ICON_MIME)
            .add_type(gdk::FileList::static_type())
            .build();
        // Files offers an Applications entry only as a link.
        let drop = gtk::DropTargetAsync::new(
            Some(formats),
            gdk::DragAction::COPY | gdk::DragAction::MOVE | gdk::DragAction::LINK,
        );
        let desktop = Rc::clone(self);
        drop.connect_drag_enter(move |_, drop, x, y| {
            desktop.on_drag_motion(drop, x, y, icon_monitor)
        });
        let desktop = Rc::clone(self);
        drop.connect_drag_motion(move |_, drop, x, y| {
            desktop.on_drag_motion(drop, x, y, icon_monitor)
        });
        let desktop = Rc::clone(self);
        drop.connect_drag_leave(move |_, _| desktop.set_drop_target(None));
        let desktop = Rc::clone(self);
        drop.connect_drop(move |_, drop, x, y| desktop.on_drop(drop, x, y, icon_monitor));
        window.add_controller(drop);

        self.bind_empty(&backdrop, &fixed, &rubber, icon_monitor);

        // Every monitor's backdrop covers its whole surface, so a click anywhere
        // lands on the desktop and not on labwc's root menu.
        let desktop = Rc::clone(self);
        let backdrop_size = backdrop.clone();
        let fixed_size = fixed.clone();
        window.connect_realize(move |window| {
            let Some(surface) = window.surface() else {
                return;
            };
            let fit = {
                let backdrop = backdrop_size.clone();
                let fixed = fixed_size.clone();
                move |width: i32, height: i32| {
                    if width >= 2 && height >= 2 {
                        backdrop.set_size_request(width, height);
                        fixed.set_size_request(width, height);
                    }
                }
            };
            fit(surface.width(), surface.height());
            let desktop_layout = Rc::clone(&desktop);
            surface.connect_layout(move |_, width, height| {
                fit(width, height);
                if icon_monitor {
                    desktop_layout.on_size(width, height);
                }
            });
            if icon_monitor {
                desktop.on_size(surface.width(), surface.height());
            }
        });
        if icon_monitor {
            *self.fixed.borrow_mut() = Some(fixed);
            *self.rubber.borrow_mut() = Some(rubber);
            *self.icon_window.borrow_mut() = Some(window.clone());
        }

        window.present();
        window
    }

    fn on_size(self: &Rc<Self>, width: i32, height: i32) {
        if width < 2 || height < 2 {
            return;
        }
        self.height.set(height);
        let first = !self.ready.get();
        let cols_changed = self.model.borrow_mut().set_area(width, height);
        if first {
            self.ready.set(true);
            self.reload();
        } else if cols_changed {
            self.rebuild();
        }
    }

    fn bind_empty(
        self: &Rc<Self>,
        backdrop: &gtk::Box,
        fixed: &gtk::Fixed,
        rubber: &gtk::Box,
        icon_monitor: bool,
    ) {
        let click = gtk::GestureClick::new();
        click.set_button(gdk::BUTTON_PRIMARY);
        let desktop = Rc::clone(self);
        click.connect_pressed(move |_, _, _, _| {
            desktop.suppress_click.set(false);
            desktop.dragging.set(false);
        });
        let desktop = Rc::clone(self);
        let backdrop_focus = backdrop.clone();
        click.connect_released(move |_, _, _, _| {
            if desktop.suppress_click.replace(false) || desktop.dragging.get() {
                return;
            }
            let desktop = Rc::clone(&desktop);
            let backdrop = backdrop_focus.clone();
            desktop.after_rename(move |desktop| {
                let spread = desktop.model.borrow().spread().is_some();
                desktop.model.borrow_mut().click_background();
                if spread {
                    desktop.rebuild();
                } else {
                    desktop.paint_selection();
                }
                let _ = backdrop.grab_focus();
                desktop.queue_changed();
            });
        });
        backdrop.add_controller(click);

        let right = gtk::GestureClick::new();
        right.set_button(gdk::BUTTON_SECONDARY);
        let desktop = Rc::clone(self);
        let fixed_menu = fixed.clone();
        right.connect_pressed(move |_, _, x, y| {
            let desktop = Rc::clone(&desktop);
            let fixed_menu = fixed_menu.clone();
            desktop.after_rename(move |desktop| {
                let spread = desktop.model.borrow().spread().is_some();
                desktop.model.borrow_mut().click_background();
                if spread {
                    desktop.rebuild();
                } else {
                    desktop.paint_selection();
                }
                desktop.popup_at(&fixed_menu, x as i32, y as i32);
                desktop.queue_changed();
            });
        });
        backdrop.add_controller(right);

        if !icon_monitor {
            return;
        }
        let drag = gtk::GestureDrag::new();
        drag.set_button(gdk::BUTTON_PRIMARY);
        let desktop = Rc::clone(self);
        drag.connect_drag_begin(move |_, x, y| {
            desktop.rubber_origin.set((x, y));
            desktop.dragging.set(false);
        });
        let desktop = Rc::clone(self);
        let rubber_drag = rubber.clone();
        drag.connect_drag_update(move |_, dx, dy| {
            if dx * dx + dy * dy < DRAG_SLOP * DRAG_SLOP && !desktop.dragging.get() {
                return;
            }
            desktop.dragging.set(true);
            desktop.suppress_click.set(true);
            let (x, y) = desktop.rubber_origin.get();
            place_rubber(&rubber_drag, x, y, dx, dy);
            let items = desktop.model.borrow().display_items();
            let grid = desktop.model.borrow().grid();
            let hits = grid.icons_in_rect(&items, x, y, dx, dy);
            desktop.model.borrow_mut().select_ids(hits);
            desktop.paint_selection();
        });
        let desktop = Rc::clone(self);
        let rubber_end = rubber.clone();
        drag.connect_drag_end(move |_, _, _| {
            if desktop.dragging.replace(false) {
                rubber_end.set_visible(false);
                desktop.queue_changed();
            }
        });
        backdrop.add_controller(drag);
    }

    fn schedule_reload(self: &Rc<Self>) {
        if !self.ready.get() {
            return;
        }
        if self.renaming.borrow().is_some() {
            self.reload_queued.set(true);
            return;
        }
        let generation = self.reload_gen.get().wrapping_add(1);
        self.reload_gen.set(generation);
        let desktop = Rc::downgrade(self);
        glib::timeout_add_local_once(Duration::from_millis(80), move || {
            let Some(desktop) = desktop.upgrade() else {
                return;
            };
            if desktop.reload_gen.get() != generation {
                return;
            }
            desktop.reload();
        });
    }

    fn reload(self: &Rc<Self>) {
        if !self.ready.get() || self.renaming.borrow().is_some() {
            if self.renaming.borrow().is_some() {
                self.reload_queued.set(true);
            }
            return;
        }
        self.ensure_monitor();
        if !self.layout_loaded.replace(true) {
            self.load_layout();
        }
        let files = read_dir(&self.desktop_dir);
        let (mounts, icons) = read_mounts(&self.volumes);
        *self.mount_icons.borrow_mut() = icons;
        {
            let mut dropped = self.dropped.borrow_mut();
            let mut model = self.model.borrow_mut();
            model.set_now(glib::real_time() / 1_000_000);
            for file in &files {
                if let Some(slot) = dropped.remove(&file.name) {
                    model.reserve_file_slot(&file.name, slot);
                }
            }
            model.set_files(files);
            model.set_mounts(mounts);
        }
        if let Some(name) = self.rename_after.borrow_mut().take() {
            if self.model.borrow().file(&name).is_some() {
                self.model.borrow_mut().plain_click(&Id::File(name.clone()));
                *self.renaming.borrow_mut() = Some(name);
                self.set_keyboard(true);
            }
        }
        self.rebuild();
        self.save_layout();
        self.queue_changed();
    }

    fn ensure_monitor(self: &Rc<Self>) {
        if self.dir_monitor.borrow().is_some() {
            return;
        }
        let file = gio::File::for_path(&self.desktop_dir);
        let Ok(monitor) = file.monitor_directory(
            gio::FileMonitorFlags::WATCH_MOVES,
            None::<&gio::Cancellable>,
        ) else {
            return;
        };
        let desktop = Rc::downgrade(self);
        monitor.connect_changed(move |_, _, _, _| {
            if let Some(desktop) = desktop.upgrade() {
                desktop.schedule_reload();
            }
        });
        *self.dir_monitor.borrow_mut() = Some(monitor);
    }

    fn load_layout(&self) {
        let Ok(text) = std::fs::read_to_string(&self.layout_path) else {
            return;
        };
        self.model.borrow_mut().set_layout(Layout::parse(&text));
        *self.layout_written.borrow_mut() = self.model.borrow().layout_text();
    }

    fn save_layout(&self) {
        let text = self.model.borrow().layout_text();
        if *self.layout_written.borrow() == text {
            return;
        }
        let tmp = self.layout_path.with_extension("tmp");
        if std::fs::write(&tmp, &text).is_ok() && std::fs::rename(&tmp, &self.layout_path).is_ok() {
            *self.layout_written.borrow_mut() = text;
        }
    }

    /// Removing a rename field ends the rename, which asks for another
    /// rebuild. That request runs after this one instead of inside it.
    fn rebuild(self: &Rc<Self>) {
        if self.rebuilding.replace(true) {
            self.rebuild_again.set(true);
            return;
        }
        loop {
            self.rebuild_again.set(false);
            self.rebuild_once();
            if !self.rebuild_again.get() {
                break;
            }
        }
        self.rebuilding.set(false);
    }

    fn rebuild_once(self: &Rc<Self>) {
        let Some(fixed) = self.fixed.borrow().clone() else {
            return;
        };
        let old = std::mem::take(&mut *self.icons.borrow_mut());
        for placed in old {
            fixed.remove(&placed.root);
        }
        let items = self.model.borrow().display_items();
        let renaming = self.renaming.borrow().clone();
        let mut placed = Vec::with_capacity(items.len());
        for item in &items {
            placed.push(self.add_icon(&fixed, item, renaming.as_deref()));
        }
        *self.icons.borrow_mut() = placed;
        if let Some(rubber) = self.rubber.borrow().clone() {
            fixed.remove(&rubber);
            rubber.set_visible(false);
            fixed.put(&rubber, 0.0, 0.0);
        }
        self.paint_selection();
        self.sync_stacks_action();
    }

    fn add_icon(
        self: &Rc<Self>,
        fixed: &gtk::Fixed,
        item: &Item,
        renaming: Option<&str>,
    ) -> Placed {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 2);
        root.add_css_class("elavo-desktop-icon");
        root.set_size_request(CELL_W, CELL_H);
        let image = match self.thumbnail_for(item) {
            Some(texture) => gtk::Image::from_paintable(Some(&texture)),
            None => gtk::Image::from_gicon(&self.icon_for(item)),
        };
        image.set_pixel_size(ICON_SIZE);
        let visual: gtk::Widget = if let Some(count) = item.count {
            let overlay = gtk::Overlay::new();
            overlay.set_child(Some(&image));
            let badge = gtk::Label::new(Some(&count.to_string()));
            badge.add_css_class("elavo-desktop-count");
            badge.set_halign(gtk::Align::End);
            badge.set_valign(gtk::Align::End);
            overlay.add_overlay(&badge);
            overlay.upcast()
        } else {
            image.upcast()
        };
        root.append(&visual);

        let label = gtk::Label::new(Some(&item.label));
        label.add_css_class("elavo-desktop-label");
        label.set_wrap(true);
        label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        label.set_max_width_chars(12);
        label.set_lines(2);
        label.set_justify(gtk::Justification::Center);
        label.set_halign(gtk::Align::Center);
        root.append(&label);

        if let Id::File(name) = &item.id {
            if renaming == Some(name.as_str()) {
                label.set_visible(false);
                let entry = gtk::Entry::new();
                entry.add_css_class("elavo-desktop-rename");
                entry.set_text(&item.label);
                entry.set_halign(gtk::Align::Fill);
                root.append(&entry);
                self.bind_rename(&entry);
            }
        }

        self.bind_icon(&root, &visual, item.id.clone());
        let (x, y) = self.model.borrow().grid().origin(item.slot);
        fixed.put(&root, x, y);
        Placed {
            id: item.id.clone(),
            root,
            label,
            origin_x: x,
            origin_y: y,
        }
    }

    fn thumbnail_for(&self, item: &Item) -> Option<gdk::Texture> {
        let Id::File(name) = &item.id else {
            return None;
        };
        let (mime, kind) = {
            let model = self.model.borrow();
            let file = model.file(name)?;
            (file.mime.clone(), file.kind)
        };
        if matches!(kind, Kind::Folder | Kind::Application) {
            return None;
        }
        let thumbs = self.thumbs.borrow().clone()?;
        thumbs.lookup(&self.desktop_dir.join(name), &mime)
    }

    fn icon_for(&self, item: &Item) -> gio::Icon {
        match &item.id {
            Id::Mount(uri) => self
                .mount_icons
                .borrow()
                .get(uri)
                .cloned()
                .unwrap_or_else(|| gio::ThemedIcon::new("drive-removable-media").upcast()),
            Id::Stack(group) => gio::content_type_get_icon(&self.model.borrow().stack_mime(group)),
            Id::File(name) => {
                let path = self.desktop_dir.join(name);
                if let Some(icon) = desktop_file_icon(&path) {
                    return icon;
                }
                let mime = self
                    .model
                    .borrow()
                    .file(name)
                    .map(|file| file.mime.clone())
                    .unwrap_or_else(|| "application/octet-stream".to_string());
                gio::content_type_get_icon(&mime)
            }
        }
    }

    fn bind_icon(self: &Rc<Self>, root: &gtk::Box, image: &gtk::Widget, id: Id) {
        let click = gtk::GestureClick::new();
        click.set_button(gdk::BUTTON_PRIMARY);
        let desktop = Rc::clone(self);
        click.connect_pressed(move |gesture, _, x, y| {
            if desktop.pointer_on_rename_entry(gesture.widget(), x, y) {
                gesture.set_state(gtk::EventSequenceState::Denied);
                return;
            }
            desktop.suppress_click.set(false);
            desktop.dragging.set(false);
        });
        let desktop = Rc::clone(self);
        let id_click = id.clone();
        click.connect_released(move |gesture, n_press, x, y| {
            if desktop.suppress_click.replace(false) || desktop.dragging.get() {
                return;
            }
            if desktop.pointer_on_rename_entry(gesture.widget(), x, y) {
                return;
            }
            let (ctrl, shift) = click_mods(gesture);
            let desktop = Rc::clone(&desktop);
            let id_click = id_click.clone();
            desktop.after_rename(move |desktop| {
                desktop.on_icon_click(&id_click, n_press, ctrl, shift);
            });
        });
        root.add_controller(click);

        let right = gtk::GestureClick::new();
        right.set_button(gdk::BUTTON_SECONDARY);
        let desktop = Rc::clone(self);
        let id_right = id.clone();
        let root_right = root.clone();
        right.connect_pressed(move |gesture, _, x, y| {
            if desktop.pointer_on_rename_entry(gesture.widget(), x, y) {
                gesture.set_state(gtk::EventSequenceState::Denied);
                return;
            }
            let desktop = Rc::clone(&desktop);
            let root_right = root_right.clone();
            let id_right = id_right.clone();
            desktop.after_rename(move |desktop| {
                desktop.popup_icon(&root_right, &id_right, x, y);
            });
        });
        root.add_controller(right);

        // A real drag, so icons can be dropped on the dock's Trash or into a
        // Files window. A drop back on the desktop moves the icons instead.
        let source = gtk::DragSource::new();
        source.set_actions(gdk::DragAction::MOVE | gdk::DragAction::COPY);
        let desktop = Rc::clone(self);
        let id_prepare = id.clone();
        let image_prepare = image.clone();
        source.connect_prepare(move |source, x, y| {
            if desktop.renaming.borrow().is_some() {
                return None;
            }
            desktop.begin_move(&id_prepare);
            desktop.drag_grab.set((x, y));
            *desktop.drag_icon.borrow_mut() = Some(id_prepare.clone());
            let hot = source
                .widget()
                .and_then(|root| {
                    root.compute_point(
                        &image_prepare,
                        &gtk::graphene::Point::new(x as f32, y as f32),
                    )
                })
                .map(|at| (at.x() as i32, at.y() as i32))
                .unwrap_or((ICON_SIZE / 2, ICON_SIZE / 2));
            source.set_icon(
                Some(&gtk::WidgetPaintable::new(Some(&image_prepare))),
                hot.0,
                hot.1,
            );
            Some(desktop.drag_content())
        });
        let desktop = Rc::clone(self);
        source.connect_drag_begin(move |_, _| {
            desktop.dragging.set(true);
            desktop.suppress_click.set(true);
            desktop.fade_selection(true);
        });
        let desktop = Rc::clone(self);
        source.connect_drag_end(move |_, _, _| {
            desktop.dragging.set(false);
            desktop.drag_icon.replace(None);
            desktop.set_drop_target(None);
            desktop.fade_selection(false);
        });
        root.add_controller(source);
    }

    /// What a dragged selection carries: its files for other programs, and a
    /// marker that tells the desktop the drag is its own.
    fn drag_content(&self) -> gdk::ContentProvider {
        let files: Vec<gio::File> = self
            .model
            .borrow()
            .file_targets()
            .iter()
            .map(|name| gio::File::for_path(self.desktop_dir.join(name)))
            .collect();
        let marker = gdk::ContentProvider::for_bytes(ICON_MIME, &glib::Bytes::from_static(b""));
        if files.is_empty() {
            return marker;
        }
        let list = gdk::ContentProvider::for_value(&gdk::FileList::from_array(&files).to_value());
        gdk::ContentProvider::new_union(&[marker, list])
    }

    fn fade_selection(&self, faded: bool) {
        let selected = self.model.borrow().selection().to_vec();
        for placed in self.icons.borrow().iter() {
            let opacity = if faded && selected.contains(&placed.id) {
                0.5
            } else {
                1.0
            };
            placed.root.set_opacity(opacity);
        }
    }

    fn set_drop_target(&self, target: Option<Id>) {
        if *self.drop_target.borrow() == target {
            return;
        }
        for placed in self.icons.borrow().iter() {
            if target.as_ref() == Some(&placed.id) {
                placed.root.add_css_class("drop-target");
            } else {
                placed.root.remove_css_class("drop-target");
            }
        }
        self.drop_target.replace(target);
    }

    fn on_icon_click(self: &Rc<Self>, id: &Id, n_press: i32, ctrl: bool, shift: bool) {
        let mut spread = false;
        {
            let mut model = self.model.borrow_mut();
            if shift {
                model.shift_click(id);
            } else if ctrl {
                model.ctrl_click(id);
            } else {
                model.plain_click(id);
            }
            if let Id::Stack(group) = id {
                if !ctrl && !shift && n_press == 1 {
                    model.toggle_spread(group);
                    spread = true;
                }
            }
        }
        if spread {
            self.rebuild();
        } else {
            self.paint_selection();
        }
        if n_press >= 2 && !ctrl && !shift && !matches!(id, Id::Stack(_)) {
            self.open_id(id);
        }
        self.queue_changed();
    }

    fn begin_move(&self, id: &Id) {
        if !self
            .model
            .borrow()
            .selection()
            .iter()
            .any(|item| item == id)
        {
            self.model.borrow_mut().plain_click(id);
            self.paint_selection();
        }
    }

    fn finish_move(self: &Rc<Self>, id: &Id, drag: Drag) {
        let origin = self
            .icons
            .borrow()
            .iter()
            .find(|placed| &placed.id == id)
            .map(|placed| (placed.origin_x, placed.origin_y));
        let Some((x, y)) = origin else {
            return;
        };
        let (dx, dy) = drag.delta;

        // Released over a folder, the files move into it, as in Files.
        // Over a volume they are copied and the icons stay.
        let target = {
            let model = self.model.borrow();
            match model.drop_target(drag.pointer.0, drag.pointer.1) {
                Some(Id::File(name)) => Some((self.desktop_dir.join(name), true)),
                Some(Id::Mount(uri)) => model
                    .mount(&uri)
                    .map(|mount| (PathBuf::from(&mount.path), false)),
                _ => None,
            }
        };
        if let Some((dest, cut)) = target {
            let uris: Vec<String> = self
                .model
                .borrow()
                .file_targets()
                .iter()
                .map(|name| {
                    gio::File::for_path(self.desktop_dir.join(name))
                        .uri()
                        .to_string()
                })
                .collect();
            self.rebuild();
            if !uris.is_empty() {
                self.import(uris, cut, &dest, None);
            }
            return;
        }

        let slot = self.model.borrow().grid().slot_near(x + dx, y + dy);
        self.model.borrow_mut().move_selection(id, slot);
        self.rebuild();
        self.save_layout();
        self.queue_changed();
    }

    fn paint_selection(&self) {
        let selected = self.model.borrow().selection().to_vec();
        for placed in self.icons.borrow().iter() {
            if selected.iter().any(|id| id == &placed.id) {
                placed.label.add_css_class("selected");
            } else {
                placed.label.remove_css_class("selected");
            }
        }
    }

    fn popup_icon(self: &Rc<Self>, root: &impl IsA<gtk::Widget>, id: &Id, x: f64, y: f64) {
        if !self
            .model
            .borrow()
            .selection()
            .iter()
            .any(|item| item == id)
        {
            self.model.borrow_mut().plain_click(id);
            self.paint_selection();
        }
        let Some(fixed) = self.fixed.borrow().clone() else {
            return;
        };
        let point = gtk::graphene::Point::new(x as f32, y as f32);
        let (px, py) = root
            .compute_point(&fixed, &point)
            .map(|at| (at.x() as i32, at.y() as i32))
            .unwrap_or((x as i32, y as i32));
        self.popup_at(&fixed, px, py);
        self.queue_changed();
    }

    fn popup_at(&self, fixed: &gtk::Fixed, x: i32, y: i32) {
        if let Some(old) = self.popover.borrow_mut().take() {
            old.unparent();
        }
        let menu = self.context_menu();
        let popover = gtk::PopoverMenu::from_model(Some(&menu));
        popover.set_has_arrow(false);
        popover.set_parent(fixed);
        popover.set_pointing_to(Some(&gdk::Rectangle::new(x, y, 1, 1)));
        popover.popup();
        *self.popover.borrow_mut() = Some(popover);
    }

    fn context_menu(&self) -> gio::Menu {
        let model = self.model.borrow();
        let menu = gio::Menu::new();
        let section = |items: &[(&str, &str)]| {
            let section = gio::Menu::new();
            for (label, action) in items {
                section.append(Some(label), Some(&format!("desk.{action}")));
            }
            section
        };
        let enabled = |command: &str| model.command_enabled(command, self.can_paste.get());

        if model.selection().is_empty() {
            let first = section(&[("New Folder", "new-folder")]);
            if enabled("paste") {
                first.append(Some("Paste"), Some("desk.paste"));
            }
            menu.append_section(None, &first);
            menu.append_section(None, &section(&[("Change Wallpaper…", "change-wallpaper")]));
            let stacks = section(&[("Use Stacks", "use-stacks")]);
            let groups = gio::Menu::new();
            for group in GroupBy::ALL {
                let item = gio::MenuItem::new(Some(group.label()), None);
                item.set_action_and_target_value(
                    Some("desk.group-by"),
                    Some(&group.id().to_variant()),
                );
                groups.append_item(&item);
            }
            stacks.append_submenu(Some("Group Stacks By"), &groups);
            menu.append_section(None, &stacks);
            menu.append_section(None, &section(&[("Properties", "get-info")]));
            return menu;
        }

        let open = section(&[("Open", "open")]);
        if let Some((_, apps)) = self.open_with_apps() {
            let with = gio::Menu::new();
            for app in apps {
                let Some(id) = app.id() else {
                    continue;
                };
                let item = gio::MenuItem::new(Some(&app.display_name()), None);
                item.set_action_and_target_value(Some("desk.open-with"), Some(&id.to_variant()));
                with.append_item(&item);
            }
            if with.n_items() > 0 {
                open.append_submenu(Some("Open With"), &with);
            }
        }
        menu.append_section(None, &open);

        if let Some(word) = model.volume_menu_word() {
            menu.append_section(None, &section(&[(word, "unmount")]));
        }
        if enabled("move-to-trash") {
            menu.append_section(None, &section(&[("Move to Trash", "move-to-trash")]));
        }
        let info = section(&[("Properties", "get-info")]);
        if enabled("rename") {
            info.append(Some("Rename…"), Some("desk.rename"));
        }
        if enabled("quick-look") {
            info.append(Some("Quick Look"), Some("desk.quick-look"));
        }
        menu.append_section(None, &info);
        if enabled("copy") {
            menu.append_section(None, &section(&[("Copy", "copy")]));
        }
        menu
    }

    fn on_key(self: &Rc<Self>, key: Key, state: gdk::ModifierType) -> glib::Propagation {
        if self.renaming.borrow().is_some() {
            return glib::Propagation::Proceed;
        }
        let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
        let handled = match key {
            Key::Left if !ctrl => {
                self.model.borrow_mut().arrow(Dir::Left);
                true
            }
            Key::Right if !ctrl => {
                self.model.borrow_mut().arrow(Dir::Right);
                true
            }
            Key::Up if !ctrl => {
                self.model.borrow_mut().arrow(Dir::Up);
                true
            }
            Key::Down if !ctrl => {
                self.model.borrow_mut().arrow(Dir::Down);
                true
            }
            Key::a | Key::A if ctrl => {
                self.model.borrow_mut().select_all();
                true
            }
            Key::Delete | Key::KP_Delete => {
                self.trash_selection();
                return glib::Propagation::Stop;
            }
            Key::Return | Key::KP_Enter => {
                self.open_selection();
                return glib::Propagation::Stop;
            }
            Key::c | Key::C if ctrl => {
                self.copy_selection(false);
                return glib::Propagation::Stop;
            }
            Key::x | Key::X if ctrl => {
                self.copy_selection(true);
                return glib::Propagation::Stop;
            }
            Key::v | Key::V if ctrl => {
                self.paste(None);
                return glib::Propagation::Stop;
            }
            Key::Escape => {
                let spread = self.model.borrow().spread().is_some();
                self.model.borrow_mut().click_background();
                if spread {
                    self.rebuild();
                }
                true
            }
            Key::space if !ctrl => {
                if !self.space_held.replace(true) {
                    self.quick_look();
                }
                return glib::Propagation::Stop;
            }
            _ => false,
        };
        if handled {
            self.paint_selection();
            self.queue_changed();
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    }

    fn activate(self: &Rc<Self>, action: &str) {
        match action {
            "new-folder" => self.new_folder(),
            "open" => self.open_selection(),
            "get-info" => self.show_properties(),
            "rename" => self.start_rename(),
            "quick-look" => self.quick_look(),
            "move-to-trash" => self.trash_selection(),
            "cut" => self.copy_selection(true),
            "copy" => self.copy_selection(false),
            "paste" => self.paste(None),
            "select-all" => {
                self.model.borrow_mut().select_all();
                self.paint_selection();
                self.queue_changed();
            }
            "use-stacks" => self.toggle_stacks(),
            "change-wallpaper" => self.change_wallpaper(),
            "unmount" => self.unmount_selection(),
            _ => {}
        }
    }

    fn new_folder(self: &Rc<Self>) {
        let name = new_folder_name(&self.model.borrow().file_names());
        let path = self.desktop_dir.join(&name);
        if let Err(err) = gio::File::for_path(&path).make_directory(None::<&gio::Cancellable>) {
            glib::g_warning!("elavo-desktop", "new folder: {err}");
            return;
        }
        *self.rename_after.borrow_mut() = Some(name);
        self.reload();
    }

    fn start_rename(self: &Rc<Self>) {
        let name = match self.model.borrow().selection() {
            [Id::File(name)] => name.clone(),
            _ => return,
        };
        *self.renaming.borrow_mut() = Some(name);
        self.set_keyboard(true);
        self.rebuild();
    }

    fn bind_rename(self: &Rc<Self>, entry: &gtk::Entry) {
        *self.rename_entry.borrow_mut() = Some(entry.clone());

        // The entry's text child holds the focus, so this controller's enter
        // signal does not fire. `ready` turns on only after we focus it, so
        // the leave that GTK emits while handing over focus is ignored.
        let focus = gtk::EventControllerFocus::new();
        let ready = Rc::new(Cell::new(false));
        let desktop = Rc::clone(self);
        let entry_leave = entry.clone();
        let ready_leave = Rc::clone(&ready);
        focus.connect_leave(move |_| {
            if !ready_leave.get() {
                return;
            }
            let typed = entry_leave.text().to_string();
            let desktop = Rc::clone(&desktop);
            glib::idle_add_local_once(move || desktop.finish_rename(true, typed));
        });
        entry.add_controller(focus);

        // The entry's text child takes Return as `activate` before a key
        // controller on the entry sees it. Finishing rebuilds the icons, so it
        // waits until the entry's own handler has returned.
        let desktop = Rc::clone(self);
        entry.connect_activate(move |entry| {
            let desktop = Rc::clone(&desktop);
            let typed = entry.text().to_string();
            glib::idle_add_local_once(move || desktop.finish_rename(true, typed));
        });
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let desktop = Rc::clone(self);
        keys.connect_key_pressed(move |_, key, _, _| {
            if key != Key::Escape {
                return glib::Propagation::Proceed;
            }
            let desktop = Rc::clone(&desktop);
            glib::idle_add_local_once(move || desktop.finish_rename(false, String::new()));
            glib::Propagation::Stop
        });
        entry.add_controller(keys);

        let entry = entry.clone();
        glib::idle_add_local_once(move || {
            entry.grab_focus();
            ready.set(true);
            let text = entry.text();
            let end = i32::from(entry.text_length());
            if let Some(dot) = text.rfind('.') {
                if dot > 0 {
                    entry.select_region(0, dot as i32);
                    return;
                }
            }
            entry.select_region(0, end);
        });
    }

    /// Confirms a rename, then runs `then`. An invalid name stays in editing
    /// and `then` does not run. With no rename in progress, `then` runs now.
    fn after_rename(self: &Rc<Self>, then: impl FnOnce(&Rc<Self>) + 'static) {
        if self.renaming.borrow().is_none() {
            then(self);
            return;
        }
        let typed = self
            .rename_entry
            .borrow()
            .as_ref()
            .map(|entry| entry.text().to_string())
            .unwrap_or_default();
        let desktop = Rc::clone(self);
        glib::idle_add_local_once(move || {
            desktop.finish_rename(true, typed);
            if desktop.renaming.borrow().is_some() {
                return;
            }
            then(&desktop);
        });
    }

    /// A click on the name field itself, which keeps editing.
    fn pointer_on_rename_entry(&self, widget: Option<gtk::Widget>, x: f64, y: f64) -> bool {
        let Some(entry) = self.rename_entry.borrow().clone() else {
            return false;
        };
        widget
            .and_then(|widget| widget.pick(x, y, gtk::PickFlags::DEFAULT))
            .is_some_and(|picked| {
                picked == entry.clone().upcast::<gtk::Widget>() || picked.is_ancestor(&entry)
            })
    }

    fn finish_rename(self: &Rc<Self>, commit: bool, typed: String) {
        let Some(name) = self.renaming.borrow_mut().take() else {
            return;
        };
        if commit && !self.apply_rename(&name, &typed) {
            *self.renaming.borrow_mut() = Some(name);
            self.set_keyboard(true);
            let active = self
                .icon_window
                .borrow()
                .as_ref()
                .is_some_and(|window| window.is_active());
            if active {
                if let Some(entry) = self.rename_entry.borrow().clone() {
                    glib::idle_add_local_once(move || {
                        entry.grab_focus();
                    });
                }
            }
            return;
        }
        self.rename_entry.borrow_mut().take();
        self.set_keyboard(false);
        self.rebuild();
        if self.reload_queued.replace(false) {
            self.reload();
        }
    }

    fn apply_rename(&self, from: &str, typed: &str) -> bool {
        let typed = typed.trim();
        if typed.is_empty()
            || typed == "."
            || typed == ".."
            || typed.contains('/')
            || typed.contains('\\')
        {
            return false;
        }
        if typed == from {
            return true;
        }
        if self
            .model
            .borrow()
            .file_names()
            .iter()
            .any(|name| name == typed)
        {
            return false;
        }
        let src = gio::File::for_path(self.desktop_dir.join(from));
        match src.set_display_name(typed, None::<&gio::Cancellable>) {
            Ok(_) => {
                self.model.borrow_mut().rename_file(from, typed);
                true
            }
            Err(err) => {
                glib::g_warning!("elavo-desktop", "rename: {err}");
                false
            }
        }
    }

    fn set_keyboard(&self, exclusive: bool) {
        let mode = if exclusive {
            KeyboardMode::Exclusive
        } else {
            KeyboardMode::OnDemand
        };
        if let Some(window) = self.icon_window.borrow().as_ref() {
            window.set_keyboard_mode(mode);
        }
    }

    fn open_selection(self: &Rc<Self>) {
        let ids = self.model.borrow().selection().to_vec();
        let mut spread = false;
        for id in &ids {
            match id {
                Id::Stack(kind) => {
                    self.model.borrow_mut().toggle_spread(kind);
                    spread = true;
                }
                Id::File(name) => self.open_file(name),
                Id::Mount(uri) => open_uri(uri),
            }
        }
        if spread {
            self.rebuild();
            self.queue_changed();
        }
    }

    fn open_id(&self, id: &Id) {
        match id {
            Id::File(name) => self.open_file(name),
            Id::Mount(uri) => open_uri(uri),
            Id::Stack(_) => {}
        }
    }

    fn open_file(&self, name: &str) {
        let path = self.desktop_dir.join(name);
        let file = gio::File::for_path(&path);
        if path.extension().and_then(|ext| ext.to_str()) == Some("desktop") {
            if launcher_is_trusted(&path) {
                if let Some(info) = gio_unix::DesktopAppInfo::from_filename(&path) {
                    if info.launch(&[], None::<&gio::AppLaunchContext>).is_ok() {
                        return;
                    }
                }
            }
            // A launcher that arrived from a download or a copy is shown as
            // text, not run. Running it would execute whatever `Exec=` says.
            if let Some(editor) = gio::AppInfo::default_for_type("text/plain", false) {
                if let Err(err) = editor.launch(&[file], None::<&gio::AppLaunchContext>) {
                    glib::g_warning!("elavo-desktop", "open: {err}");
                }
            }
            return;
        }
        open_uri(file.uri().as_str());
    }

    /// Sushi, the previewer Files uses for Quick Look. A second press closes it.
    fn quick_look(&self) {
        let uri = match self.model.borrow().preview_target() {
            Some(Id::File(name)) => gio::File::for_path(self.desktop_dir.join(name))
                .uri()
                .to_string(),
            Some(Id::Mount(uri)) => uri.clone(),
            _ => return,
        };
        let Some(conn) = self.conn.borrow().clone() else {
            return;
        };
        conn.call(
            Some(PREVIEW_BUS),
            PREVIEW_PATH,
            PREVIEW_IFACE,
            "ShowFile",
            Some(&(uri, "", true, "").to_variant()),
            None,
            gio::DBusCallFlags::NONE,
            -1,
            None::<&gio::Cancellable>,
            |result| {
                if let Err(err) = result {
                    glib::g_warning!("elavo-desktop", "quick look: {err}");
                }
            },
        );
    }

    /// Copy, move, and trash go through Files' own file operations, so they
    /// handle folders and name clashes the way Files does, show progress, and
    /// can be undone from the panel's Edit menu. `fallback` runs if Files
    /// cannot be reached.
    fn file_operation(
        &self,
        method: &'static str,
        args: glib::Variant,
        fallback: impl FnOnce() + 'static,
    ) {
        let Some(conn) = self.conn.borrow().clone() else {
            fallback();
            return;
        };
        conn.call(
            Some(FILE_OPS_BUS),
            FILE_OPS_PATH,
            FILE_OPS_IFACE,
            method,
            Some(&args),
            None,
            gio::DBusCallFlags::NONE,
            -1,
            None::<&gio::Cancellable>,
            move |result| {
                if let Err(err) = result {
                    glib::g_warning!("elavo-desktop", "{method}: {err}");
                    fallback();
                }
            },
        );
    }

    fn trash_selection(&self) {
        let paths: Vec<PathBuf> = self
            .model
            .borrow()
            .file_targets()
            .into_iter()
            .map(|name| self.desktop_dir.join(name))
            .collect();
        if paths.is_empty() {
            return;
        }
        let uris: Vec<String> = paths
            .iter()
            .map(|path| gio::File::for_path(path).uri().to_string())
            .collect();
        let args = (uris, platform_data()).to_variant();
        self.file_operation("TrashURIs", args, move || {
            for path in paths {
                if let Err(err) = gio::File::for_path(path).trash(None::<&gio::Cancellable>) {
                    glib::g_warning!("elavo-desktop", "trash: {err}");
                }
            }
        });
    }

    fn toggle_stacks(self: &Rc<Self>) {
        self.model.borrow_mut().toggle_stacks();
        self.sync_stacks_action();
        self.rebuild();
        self.save_layout();
        self.queue_changed();
    }

    fn sync_stacks_action(&self) {
        let (on, group) = {
            let model = self.model.borrow();
            (model.stacks(), model.group_by())
        };
        self.stacks_action.set_state(&on.to_variant());
        self.group_action.set_state(&group.id().to_variant());
        self.group_action.set_enabled(on);
    }

    fn unmount_selection(self: &Rc<Self>) {
        let (uri, eject) = {
            let model = self.model.borrow();
            let uri = match model.selection() {
                [Id::Mount(uri)] => uri.clone(),
                _ => return,
            };
            (uri, model.volume_menu_word() == Some("Eject"))
        };
        let Some(mount) = self
            .volumes
            .mounts()
            .into_iter()
            .find(|mount| mount.root().uri().as_str() == uri)
        else {
            return;
        };
        glib::spawn_future_local(async move {
            let result = if eject {
                mount
                    .eject_with_operation_future(
                        gio::MountUnmountFlags::NONE,
                        None::<&gio::MountOperation>,
                    )
                    .await
            } else {
                mount
                    .unmount_with_operation_future(
                        gio::MountUnmountFlags::NONE,
                        None::<&gio::MountOperation>,
                    )
                    .await
            };
            if let Err(err) = result {
                glib::g_warning!("elavo-desktop", "unmount: {err}");
            }
        });
    }

    fn show_properties(&self) {
        let mut uris: Vec<String> = self
            .model
            .borrow()
            .file_targets()
            .iter()
            .map(|name| {
                gio::File::for_path(self.desktop_dir.join(name))
                    .uri()
                    .to_string()
            })
            .collect();
        uris.extend(
            self.model
                .borrow()
                .selection()
                .iter()
                .filter_map(|id| match id {
                    Id::Mount(uri) => Some(uri.clone()),
                    _ => None,
                }),
        );
        if uris.is_empty() {
            uris.push(gio::File::for_path(&self.desktop_dir).uri().to_string());
        }
        self.call_bus(
            FILE_MANAGER_BUS,
            FILE_MANAGER_PATH,
            FILE_MANAGER_BUS,
            "ShowItemProperties",
            (uris, "").to_variant(),
        );
    }

    fn call_bus(
        &self,
        bus: &'static str,
        path: &str,
        interface: &str,
        method: &'static str,
        args: glib::Variant,
    ) {
        let Some(conn) = self.conn.borrow().clone() else {
            return;
        };
        conn.call(
            Some(bus),
            path,
            interface,
            method,
            Some(&args),
            None,
            gio::DBusCallFlags::NONE,
            -1,
            None::<&gio::Cancellable>,
            move |result| {
                if let Err(err) = result {
                    glib::g_warning!("elavo-desktop", "{bus} {method}: {err}");
                }
            },
        );
    }

    fn change_wallpaper(&self) {
        let argv = ["elavo-system-settings", "--pane", "wallpaper"].map(std::ffi::OsStr::new);
        if let Err(err) = gio::Subprocess::newv(&argv, gio::SubprocessFlags::NONE) {
            glib::g_warning!("elavo-desktop", "opening System Settings: {err}");
        }
    }

    fn set_group_by(self: &Rc<Self>, group: GroupBy) {
        if !self.model.borrow_mut().set_group_by(group) {
            return;
        }
        self.sync_stacks_action();
        self.rebuild();
        self.save_layout();
        self.queue_changed();
    }

    /// The selected file's content type and the applications that open it,
    /// the default first.
    fn open_with_apps(&self) -> Option<(String, Vec<gio::AppInfo>)> {
        let model = self.model.borrow();
        let [Id::File(name)] = model.selection() else {
            return None;
        };
        let file = model.file(name)?;
        if file.kind == Kind::Application {
            return None;
        }
        let default = gio::AppInfo::default_for_type(&file.mime, false);
        let mut apps: Vec<gio::AppInfo> = default.iter().cloned().collect();
        for app in gio::AppInfo::all_for_type(&file.mime) {
            if !app.should_show() || apps.iter().any(|known| known.equal(&app)) {
                continue;
            }
            apps.push(app);
        }
        Some((name.clone(), apps))
    }

    fn open_with(&self, app_id: &str) {
        let Some((name, apps)) = self.open_with_apps() else {
            return;
        };
        let Some(app) = apps
            .into_iter()
            .find(|app| app.id().is_some_and(|id| id == app_id))
        else {
            return;
        };
        let file = gio::File::for_path(self.desktop_dir.join(name));
        if let Err(err) = app.launch(&[file], None::<&gio::AppLaunchContext>) {
            glib::g_warning!("elavo-desktop", "open with: {err}");
        }
    }

    fn copy_selection(self: &Rc<Self>, cut: bool) {
        let uris: Vec<String> = self
            .model
            .borrow()
            .file_targets()
            .iter()
            .map(|name| {
                gio::File::for_path(self.desktop_dir.join(name))
                    .uri()
                    .to_string()
            })
            .collect();
        if uris.is_empty() {
            return;
        }
        let op = if cut { "cut" } else { "copy" };
        let gnome = format!("{op}\n{}\n", uris.join("\n"));
        let list = format!("{}\r\n", uris.join("\r\n"));
        let provider = gdk::ContentProvider::new_union(&[
            gdk::ContentProvider::for_bytes(
                "x-special/gnome-copied-files",
                &glib::Bytes::from(gnome.as_bytes()),
            ),
            gdk::ContentProvider::for_bytes("text/uri-list", &glib::Bytes::from(list.as_bytes())),
        ]);
        let Some(display) = gdk::Display::default() else {
            return;
        };
        if let Err(err) = display.clipboard().set_content(Some(&provider)) {
            glib::g_warning!("elavo-desktop", "clipboard: {err}");
        }
        self.can_paste.set(true);
        self.queue_changed();
    }

    fn paste(self: &Rc<Self>, dest: Option<PathBuf>) {
        let desktop = Rc::clone(self);
        glib::spawn_future_local(async move {
            let Some(display) = gdk::Display::default() else {
                return;
            };
            let clipboard = display.clipboard();
            let Ok((stream, mime)) = clipboard
                .read_future(
                    &["x-special/gnome-copied-files", "text/uri-list"],
                    glib::Priority::DEFAULT,
                )
                .await
            else {
                return;
            };
            let Ok(bytes) = stream
                .read_bytes_future(1024 * 1024, glib::Priority::DEFAULT)
                .await
            else {
                return;
            };
            let text = String::from_utf8_lossy(&bytes).into_owned();
            let (cut, uris) = parse_clipboard(mime.as_str(), &text);
            let dest = dest.unwrap_or_else(|| desktop.desktop_dir.clone());
            desktop.import(uris, cut, &dest, None);
            // Cut files have moved. Pasting them again would find nothing.
            if cut {
                let _ = clipboard.set_content(None::<&gdk::ContentProvider>);
            }
        });
    }

    fn is_own_drag(&self, drop: &gdk::Drop) -> bool {
        self.drag_icon.borrow().is_some() && drop.formats().contain_mime_type(ICON_MIME)
    }

    fn on_drag_motion(
        &self,
        drop: &gdk::Drop,
        x: f64,
        y: f64,
        icon_monitor: bool,
    ) -> gdk::DragAction {
        if icon_monitor {
            let target = self.model.borrow().drop_target(x, y);
            self.set_drop_target(target);
        }
        if self.is_own_drag(drop) {
            if icon_monitor {
                gdk::DragAction::MOVE
            } else {
                gdk::DragAction::empty()
            }
        } else if drop.actions().contains(gdk::DragAction::MOVE) {
            gdk::DragAction::MOVE
        } else if drop.actions().contains(gdk::DragAction::COPY) {
            gdk::DragAction::COPY
        } else {
            drop.actions() & gdk::DragAction::LINK
        }
    }

    fn on_drop(self: &Rc<Self>, drop: &gdk::Drop, x: f64, y: f64, icon_monitor: bool) -> bool {
        self.set_drop_target(None);
        if self.is_own_drag(drop) {
            let Some(id) = self.drag_icon.borrow().clone().filter(|_| icon_monitor) else {
                return false;
            };
            let origin = self
                .icons
                .borrow()
                .iter()
                .find(|placed| placed.id == id)
                .map(|placed| (placed.origin_x, placed.origin_y));
            let Some((ox, oy)) = origin else {
                return false;
            };
            let grab = self.drag_grab.get();
            let drag = Drag {
                delta: (x - grab.0 - ox, y - grab.1 - oy),
                pointer: (x, y),
            };
            drop.finish(gdk::DragAction::MOVE);
            // The icons are rebuilt, and the drag's source is one of them.
            self.dragging.set(false);
            self.drag_icon.replace(None);
            let desktop = Rc::clone(self);
            glib::idle_add_local_once(move || desktop.finish_move(&id, drag));
            return true;
        }

        let desktop = Rc::clone(self);
        let drop = drop.clone();
        glib::spawn_future_local(async move {
            let files = drop
                .read_value_future(gdk::FileList::static_type(), glib::Priority::DEFAULT)
                .await
                .ok()
                .and_then(|value| value.get::<gdk::FileList>().ok())
                .map(|list| list.files())
                .unwrap_or_default();
            let action = if files.is_empty() {
                gdk::DragAction::empty()
            } else {
                desktop.import_drop(files, x, y, icon_monitor, drop.actions())
            };
            drop.finish(action);
        });
        true
    }

    /// Files dropped from another program. They move when they come from the
    /// same disk and the source allows it, and are copied otherwise, as in Files.
    fn import_drop(
        &self,
        files: Vec<gio::File>,
        x: f64,
        y: f64,
        icon_monitor: bool,
        offered: gdk::DragAction,
    ) -> gdk::DragAction {
        let dest = if icon_monitor {
            let model = self.model.borrow();
            match model.drop_target(x, y) {
                Some(Id::File(name)) => self.desktop_dir.join(name),
                Some(Id::Mount(uri)) => model
                    .mount(&uri)
                    .map(|mount| PathBuf::from(&mount.path))
                    .unwrap_or_else(|| self.desktop_dir.clone()),
                _ => self.desktop_dir.clone(),
            }
        } else {
            self.desktop_dir.clone()
        };
        // An Applications entry is the install itself. It stays where it is,
        // and the desktop gets a shortcut that opens it.
        let (entries, files): (Vec<gio::File>, Vec<gio::File>) =
            files.into_iter().partition(is_applications_entry);
        let linked = !entries.is_empty();
        for entry in entries {
            make_shortcut(&entry, &dest);
        }
        if files.is_empty() || !offered.intersects(gdk::DragAction::MOVE | gdk::DragAction::COPY) {
            // Files offers an entry as a copy, since Wayland has no link.
            // Either way the source keeps it.
            return match linked {
                true if offered.contains(gdk::DragAction::COPY) => gdk::DragAction::COPY,
                true => gdk::DragAction::LINK & offered,
                false => gdk::DragAction::empty(),
            };
        }
        let disk = filesystem_id(&gio::File::for_path(&dest));
        let cut = offered.contains(gdk::DragAction::MOVE)
            && disk.is_some()
            && files.iter().all(|file| filesystem_id(file) == disk);
        let uris = files
            .into_iter()
            .map(|file| file.uri().to_string())
            .collect();
        let at = (icon_monitor && dest == self.desktop_dir).then(|| {
            self.model
                .borrow()
                .grid()
                .slot_near(x - f64::from(CELL_W) / 2.0, y - f64::from(CELL_H) / 2.0)
        });
        self.import(uris, cut, &dest, at);
        if cut {
            gdk::DragAction::MOVE
        } else {
            gdk::DragAction::COPY
        }
    }

    fn import(&self, uris: Vec<String>, cut: bool, dest_dir: &Path, at: Option<Slot>) {
        let mut sources = Vec::new();
        let mut names = Vec::new();
        for uri in uris {
            let src = gio::File::for_uri(&uri);
            let Some(base) = src.basename() else {
                continue;
            };
            let base = base.to_string_lossy().into_owned();
            if base.is_empty() || base.contains('/') || base.contains('\0') {
                continue;
            }
            if cut
                && src
                    .path()
                    .as_ref()
                    .is_some_and(|path| path.parent() == Some(dest_dir))
            {
                continue;
            }
            sources.push(uri);
            names.push(base);
        }
        if sources.is_empty() {
            return;
        }
        if let Some(start) = at {
            let slots = self.model.borrow().free_slots_from(start, names.len());
            let mut dropped = self.dropped.borrow_mut();
            dropped.clear();
            dropped.extend(names.iter().cloned().zip(slots));
        }

        let dest_uri = gio::File::for_path(dest_dir).uri().to_string();
        let method = if cut { "MoveURIs" } else { "CopyURIs" };
        let args = (sources.clone(), dest_uri, platform_data()).to_variant();
        let dest_dir = dest_dir.to_path_buf();
        self.file_operation(method, args, move || {
            let mut existing = dir_names(&dest_dir);
            let mut jobs = Vec::new();
            for (uri, base) in sources.into_iter().zip(names) {
                let name = available_name(&existing, &base);
                existing.push(name.clone());
                jobs.push((uri, dest_dir.join(name).to_string_lossy().into_owned(), cut));
            }
            glib::spawn_future_local(async move {
                match gio::spawn_blocking(move || transfer(jobs)).await {
                    Ok(Ok(())) => {}
                    Ok(Err(err)) => glib::g_warning!("elavo-desktop", "copy: {err}"),
                    Err(err) => glib::g_warning!("elavo-desktop", "copy: {err:?}"),
                }
            });
        });
    }

    /// Tell the panel when the desktop holds the keyboard. Losing it waits a
    /// moment, so focus passing to the desktop's own menu or rename field does
    /// not flip the panel's menus back and forth.
    fn sync_focus(self: &Rc<Self>) {
        let Ok(windows) = self.windows.try_borrow() else {
            return;
        };
        let active = windows.iter().any(|window| window.is_active());
        drop(windows);
        let generation = self.focus_gen.get().wrapping_add(1);
        self.focus_gen.set(generation);
        if active {
            self.set_focused(true);
            return;
        }
        let desktop = Rc::downgrade(self);
        glib::timeout_add_local_once(Duration::from_millis(150), move || {
            let Some(desktop) = desktop.upgrade() else {
                return;
            };
            if desktop.focus_gen.get() == generation {
                desktop.set_focused(false);
            }
        });
    }

    fn set_focused(&self, on: bool) {
        if self.focused.replace(on) == on {
            return;
        }
        let Some(conn) = self.conn.borrow().clone() else {
            return;
        };
        if let Err(err) = conn.emit_signal(
            None,
            BUS_PATH,
            BUS_IFACE,
            "Focused",
            Some(&(on,).to_variant()),
        ) {
            glib::g_warning!("elavo-desktop", "focused: {err}");
        }
    }

    fn queue_changed(self: &Rc<Self>) {
        let generation = self.notify_gen.get().wrapping_add(1);
        self.notify_gen.set(generation);
        let desktop = Rc::downgrade(self);
        glib::timeout_add_local_once(Duration::from_millis(40), move || {
            let Some(desktop) = desktop.upgrade() else {
                return;
            };
            if desktop.notify_gen.get() != generation {
                return;
            }
            desktop.emit_changed();
        });
    }

    fn emit_changed(&self) {
        let Some(conn) = self.conn.borrow().clone() else {
            return;
        };
        let on = self.model.borrow().stacks();
        if let Err(err) = conn.emit_signal(
            None,
            BUS_PATH,
            BUS_IFACE,
            "Changed",
            Some(&(on,).to_variant()),
        ) {
            glib::g_warning!("elavo-desktop", "changed: {err}");
        }
    }
}

/// An `.eapp` entry from an Applications folder (elavo-applications).
fn is_applications_entry(file: &gio::File) -> bool {
    file.path().is_some_and(|path| {
        path.extension().is_some_and(|e| e == "eapp") && path.join("entry.json").is_file()
    })
}

/// A shortcut to `entry` in `dir`, written by `elavo-applications-ctl`.
fn make_shortcut(entry: &gio::File, dir: &Path) {
    let Some(path) = entry.path() else {
        return;
    };
    let argv = [
        std::ffi::OsStr::new("elavo-applications-ctl"),
        std::ffi::OsStr::new("shortcut"),
        path.as_os_str(),
        dir.as_os_str(),
    ];
    if let Err(err) = gio::Subprocess::newv(&argv, gio::SubprocessFlags::NONE) {
        glib::g_warning!(
            "elavo-desktop",
            "making a shortcut to {}: {err}",
            path.display()
        );
    }
}

fn filesystem_id(file: &gio::File) -> Option<String> {
    file.query_info(
        "id::filesystem",
        gio::FileQueryInfoFlags::NONE,
        None::<&gio::Cancellable>,
    )
    .ok()
    .and_then(|info| info.attribute_string("id::filesystem"))
    .map(|id| id.to_string())
}

/// GNOME's media setting, on unless the schema says otherwise.
fn automount_enabled() -> bool {
    const SCHEMA: &str = "org.gnome.desktop.media-handling";
    let Some(source) = gio::SettingsSchemaSource::default() else {
        return true;
    };
    if source.lookup(SCHEMA, true).is_none() {
        return true;
    }
    gio::Settings::new(SCHEMA).boolean("automount")
}

/// Mounts a volume the way GNOME Shell does for a drive that is plugged in.
/// No password dialog is offered, so an encrypted drive waits for Files.
fn automount(volume: &gio::Volume) {
    if volume.get_mount().is_some() || !volume.can_mount() || !volume.should_automount() {
        return;
    }
    let name = volume.name().to_string();
    volume.mount(
        gio::MountMountFlags::NONE,
        None::<&gio::MountOperation>,
        None::<&gio::Cancellable>,
        move |result| {
            if let Err(err) = result {
                glib::g_warning!("elavo-desktop", "mounting {name}: {err}");
            }
        },
    );
}

fn place_rubber(rubber: &gtk::Box, x: f64, y: f64, w: f64, h: f64) {
    let (x, w) = if w < 0.0 { (x + w, -w) } else { (x, w) };
    let (y, h) = if h < 0.0 { (y + h, -h) } else { (y, h) };
    let Some(parent) = rubber.parent().and_downcast::<gtk::Fixed>() else {
        return;
    };
    parent.move_(rubber, x, y);
    rubber.set_size_request(w.round() as i32, h.round() as i32);
    rubber.set_visible(true);
}

fn click_mods(gesture: &gtk::GestureClick) -> (bool, bool) {
    let Some(event) = gesture.current_event() else {
        return (false, false);
    };
    let state = event.modifier_state();
    (
        state.contains(gdk::ModifierType::CONTROL_MASK),
        state.contains(gdk::ModifierType::SHIFT_MASK),
    )
}

fn clipboard_has_files(clipboard: &gdk::Clipboard) -> bool {
    let formats = clipboard.formats();
    formats.contain_mime_type("text/uri-list")
        || formats.contain_mime_type("x-special/gnome-copied-files")
}

fn desktop_directory() -> PathBuf {
    let dir = glib::user_special_dir(glib::UserDirectory::Desktop)
        .unwrap_or_else(|| glib::home_dir().join("Desktop"));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn read_dir(dir: &Path) -> Vec<FileRec> {
    let file = gio::File::for_path(dir);
    let Ok(enumerator) = file.enumerate_children(
        "standard::name,standard::type,standard::content-type,standard::is-hidden,time::access,time::modified,time::created",
        gio::FileQueryInfoFlags::NONE,
        None::<&gio::Cancellable>,
    ) else {
        return Vec::new();
    };
    let mut files = Vec::new();
    loop {
        match enumerator.next_file(None::<&gio::Cancellable>) {
            Ok(Some(info)) => {
                let name = info.name().to_string_lossy().into_owned();
                if name.is_empty() || name.starts_with('.') || info.is_hidden() {
                    continue;
                }
                let mime = info
                    .content_type()
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "application/octet-stream".to_string());
                let is_dir =
                    info.file_type() == gio::FileType::Directory || mime == "inode/directory";
                let time = |key: &str| info.attribute_uint64(key) as i64;
                files.push(FileRec {
                    kind: Kind::from_content_type(&mime, is_dir),
                    name,
                    mime,
                    times: Times {
                        opened: time("time::access"),
                        modified: time("time::modified"),
                        created: time("time::created"),
                    },
                });
            }
            Ok(None) => break,
            Err(err) => {
                glib::g_warning!("elavo-desktop", "list: {err}");
                break;
            }
        }
    }
    files
}

fn dir_names(dir: &Path) -> Vec<String> {
    read_dir(dir).into_iter().map(|file| file.name).collect()
}

fn read_mounts(monitor: &gio::VolumeMonitor) -> (Vec<MountRec>, HashMap<String, gio::Icon>) {
    let mut mounts = Vec::new();
    let mut icons = HashMap::new();
    for mount in monitor.mounts() {
        let facts = mount_facts(&mount);
        if !show_mount(&facts) {
            continue;
        }
        let root = mount.root();
        let uri = root.uri().to_string();
        let path = root
            .path()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_default();
        icons.insert(uri.clone(), mount.icon());
        mounts.push(MountRec {
            uri,
            name: mount.name().to_string(),
            path,
            eject: mount.can_eject(),
            unmount: mount.can_unmount(),
        });
    }
    mounts.sort_by(|a, b| a.uri.cmp(&b.uri));
    (mounts, icons)
}

fn mount_facts(mount: &gio::Mount) -> MountFacts {
    let root = mount.root();
    let scheme = root
        .uri_scheme()
        .map(|value| value.to_string())
        .unwrap_or_default();
    let network = scheme != "file";
    let path = root
        .path()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_default();
    let drive = mount.drive();
    let removable = drive
        .as_ref()
        .is_some_and(|drive| drive.is_removable() || drive.is_media_removable());
    let optical = drive
        .as_ref()
        .is_some_and(|drive| icon_is_optical(&drive.icon()))
        || icon_is_optical(&mount.icon());
    MountFacts {
        shadowed: mount.is_shadowed(),
        root: if network {
            root.uri().to_string()
        } else {
            path
        },
        removable,
        optical,
        network,
    }
}

fn icon_is_optical(icon: &gio::Icon) -> bool {
    let Some(themed) = icon.downcast_ref::<gio::ThemedIcon>() else {
        return false;
    };
    themed.names().iter().any(|name| {
        let name = name.to_ascii_lowercase();
        name.contains("optical") || name.contains("cdrom") || name.contains("dvd")
    })
}

fn desktop_file_icon(path: &Path) -> Option<gio::Icon> {
    if path.extension().and_then(|ext| ext.to_str()) != Some("desktop") {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    let mut in_entry = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_entry {
            continue;
        }
        let Some(icon) = line.strip_prefix("Icon=") else {
            continue;
        };
        let icon = icon.trim();
        if icon.is_empty() {
            return None;
        }
        if let Some(path) = icon.strip_prefix('/') {
            return Some(gio::FileIcon::new(&gio::File::for_path(format!("/{path}"))).upcast());
        }
        return Some(gio::ThemedIcon::new(icon).upcast());
    }
    None
}

/// The freedesktop convention for launchers outside the application
/// directories: run one only if its owner made it executable, or Files marked
/// it trusted.
fn launcher_is_trusted(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let executable = std::fs::metadata(path)
        .map(|meta| meta.permissions().mode() & 0o100 != 0)
        .unwrap_or(false);
    if executable {
        return true;
    }
    gio::File::for_path(path)
        .query_info(
            "metadata::trusted",
            gio::FileQueryInfoFlags::NONE,
            None::<&gio::Cancellable>,
        )
        .ok()
        .and_then(|info| info.attribute_string("metadata::trusted"))
        .is_some_and(|value| value == "true" || value == "yes")
}

fn platform_data() -> HashMap<String, glib::Variant> {
    HashMap::new()
}

fn open_uri(uri: &str) {
    if let Err(err) = gio::AppInfo::launch_default_for_uri(uri, None::<&gio::AppLaunchContext>) {
        glib::g_warning!("elavo-desktop", "open: {err}");
    }
}

fn parse_clipboard(mime: &str, text: &str) -> (bool, Vec<String>) {
    if mime == "x-special/gnome-copied-files" {
        let mut lines = text.lines();
        let cut = lines.next().unwrap_or("").trim() == "cut";
        let uris = lines
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect();
        (cut, uris)
    } else {
        let uris = text
            .lines()
            .map(str::trim)
            .filter(|line| line.contains("://"))
            .map(str::to_string)
            .collect();
        (false, uris)
    }
}

fn transfer(jobs: Vec<(String, String, bool)>) -> Result<(), String> {
    for (src, dest, cut) in jobs {
        let src = gio::File::for_uri(&src);
        let dest = gio::File::for_path(&dest);
        let result = if cut {
            src.move_(
                &dest,
                gio::FileCopyFlags::NONE,
                None::<&gio::Cancellable>,
                None,
            )
        } else {
            src.copy(
                &dest,
                gio::FileCopyFlags::NONE,
                None::<&gio::Cancellable>,
                None,
            )
        };
        if let Err(err) = result {
            return Err(err.to_string());
        }
    }
    Ok(())
}
