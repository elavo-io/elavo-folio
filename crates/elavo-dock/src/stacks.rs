//! Files and folders after the divider. A folder is a stack: its tile shows
//! the first items piled up (or the folder icon), and a click opens them as
//! a fan, a grid, or a list, sorted the way the menu says.

use std::rc::Rc;

use elavo_dock_settings::Position;
use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use crate::others::{Display, Other, Sort, View};
use crate::{files, Dock, DockView};

const ATTRIBUTES: &str = "standard::name,standard::display-name,standard::icon,standard::type,\
standard::content-type,standard::is-hidden,standard::is-backup,time::modified,time::created,\
time::changed,thumbnail::path";
/// The fan holds this many; Automatic switches to the grid above it.
const FAN_MAX: usize = 12;
const FAN_ICON: i32 = 52;

pub struct Folder {
    pub is_dir: bool,
    pub name: String,
    pub icon: gio::Icon,
    /// The first items in sort order, for the stack tile.
    pub preview: Vec<gio::Icon>,
    _monitor: Option<gio::FileMonitor>,
}

#[derive(Clone)]
pub struct Item {
    pub file: gio::File,
    pub name: String,
    pub icon: gio::Icon,
    pub is_dir: bool,
    kind: String,
    modified: i64,
    created: i64,
    changed: i64,
}

fn icon_of(info: &gio::FileInfo) -> gio::Icon {
    if let Some(thumb) = info.attribute_byte_string("thumbnail::path") {
        return gio::FileIcon::new(&gio::File::for_path(thumb.as_str())).upcast();
    }
    info.icon()
        .unwrap_or_else(|| gio::ThemedIcon::new("text-x-generic").upcast())
}

/// The folder's items, hidden files left out, in `sort` order.
pub fn list(folder: &gio::File, sort: Sort) -> Vec<Item> {
    let Ok(children) = folder.enumerate_children(
        ATTRIBUTES,
        gio::FileQueryInfoFlags::NONE,
        gio::Cancellable::NONE,
    ) else {
        return Vec::new();
    };
    let time = |info: &gio::FileInfo, key: &str| info.attribute_uint64(key) as i64;
    let mut items: Vec<Item> = children
        .flatten()
        .filter(|info| !info.is_hidden() && !info.is_backup())
        .map(|info| Item {
            file: folder.child(info.name()),
            name: info.display_name().to_string(),
            icon: icon_of(&info),
            is_dir: info.file_type() == gio::FileType::Directory,
            kind: info
                .content_type()
                .map(|c| gio::content_type_get_description(&c).to_string())
                .unwrap_or_default(),
            modified: time(&info, "time::modified"),
            created: match time(&info, "time::created") {
                0 => time(&info, "time::modified"),
                t => t,
            },
            changed: time(&info, "time::changed"),
        })
        .collect();
    let key = |i: &Item| glib::FilenameCollationKey::from(i.name.as_str());
    let by_name = |a: &Item, b: &Item| key(a).cmp(&key(b));
    match sort {
        Sort::Name => items.sort_by(by_name),
        // Linux keeps no "date added"; the inode change time moves when a
        // file is moved or copied in, which is the same moment.
        Sort::DateAdded => items.sort_by_key(|i| std::cmp::Reverse(i.changed)),
        Sort::DateModified => items.sort_by_key(|i| std::cmp::Reverse(i.modified)),
        Sort::DateCreated => items.sort_by_key(|i| std::cmp::Reverse(i.created)),
        Sort::Kind => items.sort_by(|a, b| a.kind.cmp(&b.kind).then_with(|| by_name(a, b))),
    }
    items
}

impl Folder {
    pub fn load(other: &Other, changed: impl Fn() + 'static) -> Self {
        let file = gio::File::for_uri(&other.uri);
        let is_dir = file.query_file_type(gio::FileQueryInfoFlags::NONE, gio::Cancellable::NONE)
            == gio::FileType::Directory;
        let monitor = is_dir
            .then(|| {
                file.monitor_directory(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE)
                    .ok()
            })
            .flatten();
        if let Some(monitor) = &monitor {
            monitor.set_rate_limit(500);
            monitor.connect_changed(move |_, _, _, _| changed());
        }
        let mut folder = Self {
            is_dir,
            name: String::new(),
            icon: gio::ThemedIcon::new("folder").upcast(),
            preview: Vec::new(),
            _monitor: monitor,
        };
        folder.refresh(other);
        folder
    }

    pub fn refresh(&mut self, other: &Other) {
        let file = gio::File::for_uri(&other.uri);
        let info = file.query_info(
            "standard::display-name,standard::icon,thumbnail::path",
            gio::FileQueryInfoFlags::NONE,
            gio::Cancellable::NONE,
        );
        match info {
            Ok(info) => {
                self.name = info.display_name().to_string();
                self.icon = icon_of(&info);
            }
            Err(_) => {
                self.name = file
                    .basename()
                    .map(|b| b.to_string_lossy().into_owned())
                    .unwrap_or_else(|| other.uri.clone());
                self.icon = gio::ThemedIcon::new("dialog-question").upcast();
            }
        }
        self.preview = if self.is_dir && other.display == Display::Stack {
            list(&file, other.sort)
                .into_iter()
                .take(3)
                .map(|i| i.icon)
                .collect()
        } else {
            Vec::new()
        };
    }
}

fn file_drag_source(widget: &impl IsA<gtk::Widget>, file: &gio::File, icon: &gio::Icon) {
    let source = gtk::DragSource::builder()
        .actions(gdk::DragAction::COPY | gdk::DragAction::MOVE | gdk::DragAction::LINK)
        .build();
    let file = file.clone();
    source.connect_prepare(move |_, _, _| {
        let list = gdk::FileList::from_array(std::slice::from_ref(&file));
        Some(gdk::ContentProvider::for_value(&list.to_value()))
    });
    let icon = icon.clone();
    source.connect_drag_begin(move |source, _| {
        if let Some(widget) = source.widget() {
            let theme = gtk::IconTheme::for_display(&widget.display());
            let paintable = theme.lookup_by_gicon(
                &icon,
                48,
                widget.scale_factor(),
                gtk::TextDirection::None,
                gtk::IconLookupFlags::empty(),
            );
            source.set_icon(Some(&paintable), 24, 24);
        }
    });
    widget.add_controller(source);
}

struct Stack {
    popover: gtk::Popover,
    root: gio::File,
    sort: Sort,
    view: View,
    position: Position,
}

impl Stack {
    fn effective_view(&self, count: usize) -> View {
        match self.view {
            View::Fan if self.position != Position::Bottom => View::List,
            View::Automatic if self.position == Position::Bottom && count <= FAN_MAX => View::Fan,
            View::Automatic => View::Grid,
            view => view,
        }
    }

    fn show(self: &Rc<Self>, folder: &gio::File) {
        let items = list(folder, self.sort);
        let view = self.effective_view(items.len());
        for class in ["elavo-dock-fan", "elavo-dock-grid", "elavo-dock-list"] {
            self.popover.remove_css_class(class);
        }
        self.popover.set_offset(0, 0);
        let content = match view {
            View::Fan => {
                self.popover.add_css_class("elavo-dock-fan");
                self.fan(folder, &items)
            }
            View::List => {
                self.popover.add_css_class("elavo-dock-list");
                self.list(folder, &items)
            }
            _ => {
                self.popover.add_css_class("elavo-dock-grid");
                self.grid(folder, &items)
            }
        };
        self.popover.set_child(Some(&content));
    }

    fn open(self: &Rc<Self>, item: &Item) {
        if item.is_dir {
            self.show(&item.file);
        } else {
            self.popover.popdown();
            files::open_uri(&item.file.uri());
        }
    }

    fn open_in_files(&self, folder: &gio::File) -> gtk::Button {
        let button = gtk::Button::builder()
            .label("Open in Files")
            .css_classes(["flat", "elavo-dock-stack-open"])
            .build();
        let popover = self.popover.downgrade();
        let uri = folder.uri().to_string();
        button.connect_clicked(move |_| {
            if let Some(popover) = popover.upgrade() {
                popover.popdown();
            }
            files::open_uri(&uri);
        });
        button
    }

    fn header(self: &Rc<Self>, folder: &gio::File) -> gtk::Box {
        let header = gtk::Box::builder()
            .spacing(6)
            .css_classes(["elavo-dock-stack-header"])
            .build();
        if !folder.equal(&self.root) {
            let back = gtk::Button::builder()
                .icon_name("go-previous-symbolic")
                .css_classes(["flat", "circular"])
                .tooltip_text("Back")
                .build();
            let stack = Rc::downgrade(self);
            let parent = folder.parent();
            back.connect_clicked(move |_| {
                if let (Some(stack), Some(parent)) = (stack.upgrade(), parent.as_ref()) {
                    stack.show(parent);
                }
            });
            header.append(&back);
        }
        let name = folder
            .query_info(
                "standard::display-name",
                gio::FileQueryInfoFlags::NONE,
                gio::Cancellable::NONE,
            )
            .map(|i| i.display_name().to_string())
            .unwrap_or_default();
        header.append(
            &gtk::Label::builder()
                .label(name)
                .hexpand(true)
                .css_classes(["heading"])
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build(),
        );
        header
    }

    fn item_button(self: &Rc<Self>, item: &Item, child: &impl IsA<gtk::Widget>) -> gtk::Button {
        let button = gtk::Button::builder()
            .child(child)
            .css_classes(["flat", "elavo-dock-stack-item"])
            .tooltip_text(item.name.as_str())
            .build();
        let stack = Rc::downgrade(self);
        let clicked = item.clone();
        button.connect_clicked(move |_| {
            if let Some(stack) = stack.upgrade() {
                stack.open(&clicked);
            }
        });
        file_drag_source(&button, &item.file, &item.icon);
        button
    }

    fn grid(self: &Rc<Self>, folder: &gio::File, items: &[Item]) -> gtk::Widget {
        let columns = items.len().clamp(3, 6) as u32;
        let flow = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .homogeneous(true)
            .min_children_per_line(columns)
            .max_children_per_line(columns)
            .column_spacing(4)
            .row_spacing(4)
            .valign(gtk::Align::Start)
            .build();
        for item in items {
            let tile = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(4)
                .width_request(92)
                .build();
            tile.append(
                &gtk::Image::builder()
                    .gicon(&item.icon)
                    .pixel_size(56)
                    .build(),
            );
            tile.append(
                &gtk::Label::builder()
                    .label(item.name.as_str())
                    .wrap(true)
                    .wrap_mode(gtk::pango::WrapMode::WordChar)
                    .lines(2)
                    .ellipsize(gtk::pango::EllipsizeMode::Middle)
                    .justify(gtk::Justification::Center)
                    .max_width_chars(12)
                    .build(),
            );
            flow.append(&self.item_button(item, &tile));
        }
        let scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .propagate_natural_width(true)
            .max_content_height(520)
            .child(&flow)
            .build();
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .build();
        content.append(&self.header(folder));
        if items.is_empty() {
            content.append(&empty_label());
        } else {
            content.append(&scrolled);
        }
        content.append(&self.open_in_files(folder));
        content.upcast()
    }

    fn list(self: &Rc<Self>, folder: &gio::File, items: &[Item]) -> gtk::Widget {
        let rows = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        for item in items {
            let row = gtk::Box::builder().spacing(8).build();
            row.append(
                &gtk::Image::builder()
                    .gicon(&item.icon)
                    .pixel_size(20)
                    .build(),
            );
            row.append(
                &gtk::Label::builder()
                    .label(item.name.as_str())
                    .xalign(0.0)
                    .hexpand(true)
                    .ellipsize(gtk::pango::EllipsizeMode::Middle)
                    .max_width_chars(36)
                    .build(),
            );
            if item.is_dir {
                row.append(&gtk::Image::from_icon_name("go-next-symbolic"));
            }
            rows.append(&self.item_button(item, &row));
        }
        let scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(560)
            .min_content_width(280)
            .child(&rows)
            .build();
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(4)
            .build();
        content.append(&self.header(folder));
        if items.is_empty() {
            content.append(&empty_label());
        } else {
            content.append(&scrolled);
        }
        content.append(&self.open_in_files(folder));
        content.upcast()
    }

    /// Items rise from the tile in a curve, nearest first, each with its
    /// name beside it, and "Open in Files" on top.
    fn fan(self: &Rc<Self>, folder: &gio::File, items: &[Item]) -> gtk::Widget {
        let column = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(2)
            .build();
        let open = self.open_in_files(folder);
        open.set_halign(gtk::Align::End);
        column.append(&open);
        let shown: Vec<&Item> = items.iter().take(FAN_MAX).collect();
        let widest = fan_offset(shown.len().saturating_sub(1));
        let mut nearest = None;
        for (i, item) in shown.iter().enumerate().rev() {
            let row = gtk::Box::builder()
                .spacing(10)
                .halign(gtk::Align::End)
                .build();
            row.append(
                &gtk::Label::builder()
                    .label(item.name.as_str())
                    .css_classes(["elavo-dock-label"])
                    .ellipsize(gtk::pango::EllipsizeMode::Middle)
                    .max_width_chars(28)
                    .build(),
            );
            row.append(
                &gtk::Image::builder()
                    .gicon(&item.icon)
                    .pixel_size(FAN_ICON)
                    .build(),
            );
            let button = self.item_button(item, &row);
            button.set_halign(gtk::Align::End);
            button.set_margin_end(widest - fan_offset(i));
            column.append(&button);
            if i == 0 {
                nearest = Some((button, row));
            }
        }
        // The popover centres on the tile; move it so the nearest icon sits
        // straight above the tile instead.
        if let Some((button, row)) = nearest {
            let width = |w: &gtk::Widget| w.preferred_size().1.width();
            let pad = (width(button.upcast_ref()) - width(row.upcast_ref())) / 2;
            let from_right = widest + pad + FAN_ICON / 2;
            self.popover
                .set_offset(from_right - width(column.upcast_ref()) / 2, 0);
        }
        column.upcast()
    }
}

/// How far right the fan has curved by item `i`, counting up from the tile.
fn fan_offset(i: usize) -> i32 {
    let x = i as f64 / FAN_MAX as f64;
    (x * x * 90.0).round() as i32
}

fn empty_label() -> gtk::Label {
    gtk::Label::builder()
        .label("No Items")
        .css_classes(["dim-label"])
        .margin_top(24)
        .margin_bottom(24)
        .build()
}

pub fn show(dock: &Rc<Dock>, view: &DockView, key: &str, other: &Other) {
    let position = dock.settings.borrow().position;
    let popover = gtk::Popover::builder().has_arrow(true).build();
    popover.add_css_class("elavo-dock-stack");
    let root = gio::File::for_uri(&other.uri);
    let stack = Rc::new(Stack {
        popover: popover.clone(),
        root: root.clone(),
        sort: other.sort,
        view: other.view,
        position,
    });
    stack.show(&root);
    if stack.effective_view(list(&root, other.sort).len()) == View::Fan {
        popover.set_has_arrow(false);
    }
    // The popover holds the stack for as long as it is open.
    let keep = std::cell::RefCell::new(Some(stack));
    popover.connect_closed(move |_| {
        keep.take();
    });
    view.popup(&popover, key);
}
