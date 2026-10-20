//! The large clock: numerals of frosted white glass. The wallpaper behind
//! them is blurred and masked by the glyphs, a translucent white goes over
//! that, and a soft shadow keeps the edges on a bright picture.

use std::cell::RefCell;

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, glib, graphene, gsk};

/// Blur radius for the wallpaper inside the numerals, in logical pixels.
const BLUR: f32 = 24.0;
/// How white the glass is over the blurred wallpaper.
const FROST: f32 = 0.55;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct GlassClock {
        pub text: RefCell<String>,
        pub texture: RefCell<Option<gdk::Texture>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for GlassClock {
        const NAME: &'static str = "ElavoGlassClock";
        type Type = super::GlassClock;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("glassclock");
        }
    }

    impl ObjectImpl for GlassClock {}

    impl WidgetImpl for GlassClock {
        fn measure(&self, orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            let layout = self.obj().create_pango_layout(Some(&self.text.borrow()));
            let (w, h) = layout.pixel_size();
            let size = match orientation {
                gtk::Orientation::Horizontal => w,
                _ => h,
            };
            (size, size, -1, -1)
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let widget = self.obj();
            let text = self.text.borrow();
            if text.is_empty() {
                return;
            }
            let layout = widget.create_pango_layout(Some(&text));
            let (lw, lh) = layout.pixel_size();
            let (w, h) = (widget.width() as f32, widget.height() as f32);
            let origin = graphene::Point::new((w - lw as f32) / 2.0, (h - lh as f32) / 2.0);
            let area = graphene::Rect::new(0.0, 0.0, w, h);

            snapshot.save();
            snapshot.translate(&graphene::Point::new(origin.x(), origin.y() + 2.0));
            snapshot.push_blur(8.0);
            snapshot.append_layout(&layout, &gdk::RGBA::new(0.0, 0.0, 0.0, 0.22));
            snapshot.pop();
            snapshot.restore();

            snapshot.push_mask(gsk::MaskMode::Alpha);
            snapshot.save();
            snapshot.translate(&origin);
            snapshot.append_layout(&layout, &gdk::RGBA::WHITE);
            snapshot.restore();
            snapshot.pop();

            if let Some(texture) = self.texture.borrow().as_ref() {
                if let Some(cover) = cover_rect(&widget, texture) {
                    let reach = BLUR * 2.0;
                    snapshot.push_blur(f64::from(BLUR));
                    snapshot.push_clip(&graphene::Rect::new(
                        -reach,
                        -reach,
                        w + reach * 2.0,
                        h + reach * 2.0,
                    ));
                    snapshot.append_texture(texture, &cover);
                    snapshot.pop();
                    snapshot.pop();
                }
            }
            snapshot.append_color(&gdk::RGBA::new(1.0, 1.0, 1.0, FROST), &area);
            snapshot.pop();
        }
    }
}

glib::wrapper! {
    pub struct GlassClock(ObjectSubclass<imp::GlassClock>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl GlassClock {
    pub fn new() -> Self {
        let clock: Self = glib::Object::builder()
            .property("accessible-role", gtk::AccessibleRole::Label)
            .build();
        clock.add_css_class("elavo-greeter-clock");
        clock
    }

    pub fn set_text(&self, text: &str) {
        if *self.imp().text.borrow() == text {
            return;
        }
        self.imp().text.replace(text.to_owned());
        self.update_property(&[gtk::accessible::Property::Label(text)]);
        self.queue_resize();
    }

    pub fn set_texture(&self, texture: Option<&gdk::Texture>) {
        self.imp().texture.replace(texture.cloned());
        self.queue_draw();
    }
}

impl Default for GlassClock {
    fn default() -> Self {
        Self::new()
    }
}

/// Where the wallpaper sits, scaled to cover the window, in this widget's
/// coordinates.
fn cover_rect(widget: &GlassClock, texture: &gdk::Texture) -> Option<graphene::Rect> {
    let root = widget.root()?;
    let root = root.upcast_ref::<gtk::Widget>();
    let offset = widget.compute_point(root, &graphene::Point::zero())?;
    let (rw, rh) = (root.width() as f32, root.height() as f32);
    let (tw, th) = (texture.width() as f32, texture.height() as f32);
    let scale = (rw / tw).max(rh / th);
    let (cw, ch) = (tw * scale, th * scale);
    Some(graphene::Rect::new(
        (rw - cw) / 2.0 - offset.x(),
        (rh - ch) / 2.0 - offset.y(),
        cw,
        ch,
    ))
}
