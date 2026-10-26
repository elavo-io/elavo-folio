//! A container that asks for no space and gives its child all of it.
//!
//! The desktop surface takes its size from labwc. Whatever the child asks for
//! (a backdrop sized to the last surface, icons in far columns) must not hold
//! the surface larger than labwc's size, or a smaller size after a scale or
//! panel change is never applied.

use std::cell::RefCell;

use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct Area {
        pub child: RefCell<Option<gtk::Widget>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Area {
        const NAME: &'static str = "ElavoDesktopArea";
        type Type = super::Area;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for Area {
        fn constructed(&self) {
            self.parent_constructed();
            self.obj().set_overflow(gtk::Overflow::Hidden);
        }

        fn dispose(&self) {
            if let Some(child) = self.child.take() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for Area {
        fn measure(&self, _orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            (0, 0, -1, -1)
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            if let Some(child) = self.child.borrow().as_ref() {
                // Never below the child's own minimum; the overflow is clipped.
                let (min_w, _, _, _) = child.measure(gtk::Orientation::Horizontal, -1);
                let (min_h, _, _, _) = child.measure(gtk::Orientation::Vertical, -1);
                child.allocate(width.max(min_w), height.max(min_h), baseline, None);
            }
        }
    }
}

glib::wrapper! {
    pub struct Area(ObjectSubclass<imp::Area>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Area {
    pub fn new(child: &impl IsA<gtk::Widget>) -> Self {
        let area: Self = glib::Object::new();
        child.set_parent(&area);
        area.imp().child.replace(Some(child.clone().upcast()));
        area
    }
}
