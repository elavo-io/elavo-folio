//! A rounded plate of the wallpaper, blurred and slightly darkened.
//!
//! The greeter paints its own picture, so this is the same trick as the
//! glass clock: draw the wallpaper, blur it, and tint it. It is not
//! `ext-background-effect-v1`. The login and lock screens use it behind a
//! person's picture, behind a list row while the pointer is over it, and
//! behind the name and password fields.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, glib, graphene, gsk};

/// Blur radius, in logical pixels. The same as the clock's.
const BLUR: f32 = 24.0;
/// Black over the blurred wallpaper, so the plate is a little darker than
/// the picture behind it.
const DARKEN: f32 = 0.16;
/// White over the darken. Keeps the plate reading as frosted glass.
const FROST: f32 = 0.18;
/// How much glass shows around a person's picture, on each side.
pub const PHOTO_RIM: i32 = 4;
/// How long a list row's glass takes to fade in or out.
const HOVER_MS: f64 = 200.0;

mod imp {
    use super::*;

    pub struct Plate {
        pub wallpaper: RefCell<Rc<RefCell<Option<gdk::Texture>>>>,
        pub rim: Cell<i32>,
        pub child: RefCell<Option<gtk::Widget>>,
        /// A list row. The glass fades with the pointer; the child stays put.
        pub fades: Cell<bool>,
        pub hovered: Cell<bool>,
        pub level: Cell<f64>,
        pub ticking: Cell<bool>,
        pub last_frame: Cell<i64>,
    }

    impl Default for Plate {
        fn default() -> Self {
            Self {
                wallpaper: RefCell::new(Rc::new(RefCell::new(None))),
                rim: Cell::new(0),
                child: RefCell::new(None),
                fades: Cell::new(false),
                hovered: Cell::new(false),
                level: Cell::new(1.0),
                ticking: Cell::new(false),
                last_frame: Cell::new(0),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Plate {
        const NAME: &'static str = "ElavoGreeterPlate";
        type Type = super::Plate;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for Plate {
        fn dispose(&self) {
            // glib chains to the parent's dispose after this returns.
            if let Some(child) = self.child.borrow_mut().take() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for Plate {
        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let rim = self.rim.get().saturating_mul(2);
            let Some(child) = self.child.borrow().clone() else {
                return (rim, rim, -1, -1);
            };
            let for_child = if for_size < 0 {
                -1
            } else {
                (for_size - rim).max(0)
            };
            let (min, nat, min_b, nat_b) = child.measure(orientation, for_child);
            let inset = self.rim.get();
            let baseline = |value: i32| if value < 0 { -1 } else { value + inset };
            (min + rim, nat + rim, baseline(min_b), baseline(nat_b))
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            let Some(child) = self.child.borrow().clone() else {
                return;
            };
            let (child_w, child_h, x, y) = if self.rim.get() > 0 {
                let (_, nat_w, _, _) = child.measure(gtk::Orientation::Horizontal, -1);
                let (_, nat_h, _, _) = child.measure(gtk::Orientation::Vertical, -1);
                let child_w = nat_w.min(width).max(0);
                let child_h = nat_h.min(height).max(0);
                let x = (width - child_w) / 2;
                let y = (height - child_h) / 2;
                (child_w, child_h, x, y)
            } else {
                (width, height, 0, 0)
            };
            let transform =
                gsk::Transform::new().translate(&graphene::Point::new(x as f32, y as f32));
            child.allocate(child_w, child_h, baseline, Some(transform));
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let widget = self.obj();
            let (w, h) = (widget.width() as f32, widget.height() as f32);
            let level = if self.fades.get() {
                self.level.get()
            } else {
                1.0
            };
            if w > 0.0 && h > 0.0 && level > 0.0 {
                let fade = level < 1.0;
                if fade {
                    snapshot.push_opacity(level);
                }
                let bounds = graphene::Rect::new(0.0, 0.0, w, h);
                let radius = w.min(h) / 2.0;
                snapshot.push_rounded_clip(&gsk::RoundedRect::from_rect(bounds, radius));
                if let Some(texture) = self.wallpaper.borrow().borrow().clone() {
                    if let Some(cover) = widget_wallpaper(widget.upcast_ref(), &texture) {
                        let reach = BLUR * 2.0;
                        snapshot.push_blur(f64::from(BLUR));
                        snapshot.push_clip(&graphene::Rect::new(
                            -reach,
                            -reach,
                            w + reach * 2.0,
                            h + reach * 2.0,
                        ));
                        snapshot.append_texture(&texture, &cover);
                        snapshot.pop();
                        snapshot.pop();
                    }
                }
                snapshot.append_color(&gdk::RGBA::new(0.0, 0.0, 0.0, DARKEN), &bounds);
                snapshot.append_color(&gdk::RGBA::new(1.0, 1.0, 1.0, FROST), &bounds);
                snapshot.pop();
                if fade {
                    snapshot.pop();
                }
            }
            if let Some(child) = self.child.borrow().clone() {
                widget.snapshot_child(&child, snapshot);
            }
        }
    }
}

glib::wrapper! {
    pub struct Plate(ObjectSubclass<imp::Plate>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Plate {
    pub fn new(wallpaper: Rc<RefCell<Option<gdk::Texture>>>) -> Self {
        let plate: Self = glib::Object::builder()
            .property("accessible-role", gtk::AccessibleRole::Presentation)
            .build();
        plate.add_css_class("elavo-greeter-frost");
        plate.imp().wallpaper.replace(wallpaper);
        plate.set_halign(gtk::Align::Center);
        plate.set_valign(gtk::Align::Center);
        plate
    }

    /// Extra glass around the child, in logical pixels on each side.
    /// Zero fills the plate with the child, which is what a row and a
    /// field want.
    pub fn set_rim(&self, rim: i32) {
        self.imp().rim.set(rim.max(0));
    }
    /// The glass fades in while the pointer is over the row. The child does
    /// not. [`Self::set_hovered`] drives it.
    pub fn fade(&self) {
        self.imp().fades.set(true);
        self.imp().level.set(0.0);
    }

    pub fn set_hovered(&self, hovered: bool) {
        if self.imp().hovered.replace(hovered) == hovered {
            return;
        }
        if self.imp().ticking.replace(true) {
            return;
        }
        let plate = self.clone();
        plate.add_tick_callback(move |plate, clock| {
            let imp = plate.imp();
            let now = clock.frame_time();
            let prev = imp.last_frame.replace(now);
            let dt = if prev == 0 {
                0.0
            } else {
                (now.saturating_sub(prev)) as f64 / 1000.0
            };
            let target = if imp.hovered.get() { 1.0 } else { 0.0 };
            let current = imp.level.get();
            let next = if target > current {
                (current + dt / HOVER_MS).min(target)
            } else {
                (current - dt / HOVER_MS).max(target)
            };
            imp.level.set(next);
            plate.queue_draw();
            if next == target {
                imp.ticking.set(false);
                imp.last_frame.set(0);
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
    }

    pub fn set_child(&self, child: &impl IsA<gtk::Widget>) {
        let child = child.upcast_ref::<gtk::Widget>();
        if let Some(old) = self.imp().child.borrow_mut().take() {
            old.unparent();
        }
        child.set_parent(self);
        self.imp().child.replace(Some(child.clone()));
    }
}

/// Where the wallpaper sits, scaled to cover the window, in `widget`'s
/// coordinates.
pub(crate) fn widget_wallpaper(
    widget: &gtk::Widget,
    texture: &gdk::Texture,
) -> Option<graphene::Rect> {
    let root = widget.root()?;
    let root = root.upcast_ref::<gtk::Widget>();
    let offset = widget.compute_point(root, &graphene::Point::zero())?;
    let (x, y, w, h) = wallpaper_rect(
        root.width() as f32,
        root.height() as f32,
        texture.width() as f32,
        texture.height() as f32,
        offset.x(),
        offset.y(),
    );
    (w > 0.0 && h > 0.0).then(|| graphene::Rect::new(x, y, w, h))
}

/// Cover placement. `offset` is this widget's origin inside the window.
fn wallpaper_rect(
    root_w: f32,
    root_h: f32,
    tex_w: f32,
    tex_h: f32,
    offset_x: f32,
    offset_y: f32,
) -> (f32, f32, f32, f32) {
    let scale = (root_w / tex_w).max(root_h / tex_h);
    let (cw, ch) = (tex_w * scale, tex_h * scale);
    (
        (root_w - cw) / 2.0 - offset_x,
        (root_h - ch) / 2.0 - offset_y,
        cw,
        ch,
    )
}

#[cfg(test)]
mod tests {
    use super::wallpaper_rect;

    #[test]
    fn wallpaper_stays_centered_in_the_window() {
        // A 200×100 picture covering a 100×100 window is cropped on the
        // sides. The widget sits 10px right and 20px down from the window.
        let (x, y, w, h) = wallpaper_rect(100.0, 100.0, 200.0, 100.0, 10.0, 20.0);
        assert!((h - 100.0).abs() < 0.05);
        assert!((y + 20.0).abs() < 0.05);
        assert!(w > 100.0);
        assert!((x + w / 2.0 - 40.0).abs() < 0.05);
    }
}
