//! The trash at the end of the dock: full or empty from `trash:///` (GVfs
//! counts every mounted volume's trash), and Empty Trash with the Mac's
//! confirmation.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};

#[derive(Default)]
pub struct Trash {
    full: Rc<Cell<bool>>,
    monitor: RefCell<Option<gio::FileMonitor>>,
}

fn root() -> gio::File {
    gio::File::for_uri("trash:///")
}

async fn count() -> u32 {
    root()
        .query_info_future(
            "trash::item-count",
            gio::FileQueryInfoFlags::NONE,
            glib::Priority::DEFAULT,
        )
        .await
        .map(|info| info.attribute_uint32("trash::item-count"))
        .unwrap_or(0)
}

impl Trash {
    pub fn full(&self) -> bool {
        self.full.get()
    }

    pub fn start(&self, changed: impl Fn() + 'static) {
        let changed = Rc::new(changed);
        let full = self.full.clone();
        let update = Rc::new(move || {
            let full = full.clone();
            let changed = changed.clone();
            glib::spawn_future_local(async move {
                let now = count().await > 0;
                if full.replace(now) != now {
                    changed();
                }
            });
        });
        update();
        match root().monitor_directory(gio::FileMonitorFlags::NONE, gio::Cancellable::NONE) {
            Ok(monitor) => {
                let update = update.clone();
                monitor.connect_changed(move |_, _, _, _| update());
                self.monitor.replace(Some(monitor));
            }
            Err(err) => glib::g_warning!("elavo-dock", "watching the trash: {err}"),
        }
    }
}

/// Ask, then permanently delete everything in the trash.
pub fn empty() {
    let dialog = gtk::AlertDialog::builder()
        .modal(true)
        .message("Are you sure you want to permanently erase the items in the Trash?")
        .detail("You can't undo this action.")
        .buttons(["Cancel", "Empty Trash"])
        .cancel_button(0)
        .default_button(1)
        .build();
    dialog.choose(None::<&gtk::Window>, gio::Cancellable::NONE, |choice| {
        if choice != Ok(1) {
            return;
        }
        std::thread::spawn(|| {
            let root = root();
            let Ok(children) = root.enumerate_children(
                "standard::name",
                gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
                gio::Cancellable::NONE,
            ) else {
                return;
            };
            for info in children.flatten() {
                let child = root.child(info.name());
                if let Err(err) = delete(&child) {
                    glib::g_warning!("elavo-dock", "emptying the trash: {err}");
                }
            }
        });
    });
}

fn delete(file: &gio::File) -> Result<(), glib::Error> {
    match file.delete(gio::Cancellable::NONE) {
        Err(err) if err.matches(gio::IOErrorEnum::NotEmpty) => {
            let children = file.enumerate_children(
                "standard::name",
                gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
                gio::Cancellable::NONE,
            )?;
            for info in children.flatten() {
                delete(&file.child(info.name()))?;
            }
            file.delete(gio::Cancellable::NONE)
        }
        other => other,
    }
}
