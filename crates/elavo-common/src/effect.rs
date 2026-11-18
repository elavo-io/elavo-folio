//! Which shapes on a surface want an effect drawn behind them, and which
//! effect.
//!
//! The shape and the effect kind are kept apart. Blur is the only kind
//! today. Glass can be added as another kind later, and one surface can
//! blur while another uses glass at the same time.
//!
//! From GTK 4.24, GTK binds `ext_background_effect_v1` itself and creates
//! the one effect object each of its surfaces may have. It sends as the
//! blur region the rounded border box of every widget that draws a CSS
//! `backdrop-filter: blur()` of at least 10px. A second effect object on
//! the same surface is a protocol error, and the compositor disconnects
//! the client. So from 4.24 a part is a CSS class on its widget, and GTK
//! sends the region. Older GTK does not know the protocol, and
//! [`crate::blur`] sends the rounded rectangles itself.

use std::cell::{Cell, RefCell};

use gtk::prelude::*;

use crate::blur::BackgroundBlur;

/// What is drawn behind a part.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EffectKind {
    Blur,
}

impl EffectKind {
    fn css_class(self) -> &'static str {
        match self {
            EffectKind::Blur => "elavo-effect-blur",
        }
    }
}

/// One shape that wants an effect behind it.
#[derive(Clone, Debug, PartialEq)]
pub struct EffectPart {
    /// The widget whose rounded border box is the shape. GTK 4.24 reads
    /// the shape from the widget, so its CSS `border-radius` must match
    /// the radius in `rect`.
    pub widget: gtk::Widget,
    /// x, y, width, height and corner radius, in surface-local pixels.
    /// Used when Folio sends the region itself, before GTK 4.24.
    pub rect: (i32, i32, i32, i32, i32),
    pub kind: EffectKind,
}

impl EffectPart {
    pub fn blur(widget: &impl IsA<gtk::Widget>, rect: (i32, i32, i32, i32, i32)) -> Self {
        Self {
            widget: widget.clone().upcast(),
            rect,
            kind: EffectKind::Blur,
        }
    }
}

/// True when GTK owns the surface's background effect object (GTK 4.24
/// and later). Folio must then never create one of its own.
pub fn gtk_owns_background_effect() -> bool {
    let (major, minor) = (gtk::major_version(), gtk::minor_version());
    major > 4 || (major == 4 && minor >= 24)
}

/// Whether the compositor offers background effects on `widget`'s display.
/// Reads the registry once per process, without binding the effect global
/// when GTK owns it.
pub fn available(widget: &gtk::Widget) -> bool {
    if gtk_owns_background_effect() {
        BackgroundBlur::offered(widget)
    } else {
        BackgroundBlur::prepare_display(widget)
    }
}

/// The parts one surface has asked for.
#[derive(Default)]
pub struct SurfaceEffects {
    /// Before GTK 4.24, Folio's own effect object on the surface.
    blur: RefCell<Option<BackgroundBlur>>,
    parts: RefCell<Vec<EffectPart>>,
}

impl SurfaceEffects {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace every part on the surface `owner` is on. An empty list
    /// removes the effect. True when the request went out; it takes effect
    /// with the surface's next commit. False when the compositor does not
    /// offer the protocol, or the surface is not there yet; call again later.
    pub fn set(&self, owner: &impl IsA<gtk::Widget>, parts: Vec<EffectPart>) -> bool {
        if gtk_owns_background_effect() {
            self.set_css(owner.upcast_ref(), parts);
            return true;
        }
        if self.blur.borrow().is_none() {
            let blur = BackgroundBlur::for_widget(owner.upcast_ref());
            *self.blur.borrow_mut() = blur;
        }
        let blur = self.blur.borrow();
        let Some(blur) = blur.as_ref() else {
            return false;
        };
        let rounds: Vec<_> = parts
            .iter()
            .filter(|part| part.kind == EffectKind::Blur)
            .map(|part| part.rect)
            .collect();
        blur.set_rounds(&rounds);
        *self.parts.borrow_mut() = parts;
        true
    }

    /// Drop every part now. Before GTK 4.24 this also destroys Folio's
    /// effect object, so the compositor drops the blur while the surface's
    /// scene node still exists, and the surface can ask again later.
    pub fn release(&self) {
        for part in self.parts.take() {
            part.widget.remove_css_class(part.kind.css_class());
        }
        self.blur.take();
    }

    fn set_css(&self, owner: &gtk::Widget, parts: Vec<EffectPart>) {
        install_css(&owner.display());
        let old = self.parts.replace(Vec::new());
        for part in &old {
            let kept = parts
                .iter()
                .any(|new| new.widget == part.widget && new.kind == part.kind);
            if !kept {
                part.widget.remove_css_class(part.kind.css_class());
            }
        }
        for part in &parts {
            part.widget.add_css_class(part.kind.css_class());
        }
        *self.parts.borrow_mut() = parts;
    }
}

impl Drop for SurfaceEffects {
    fn drop(&mut self) {
        self.release();
    }
}

/// GTK 4.24 sends a region only for a blur radius of 20 or more in GSK
/// units, which is CSS `blur(10px)`. GTK also blurs what the surface has
/// drawn under the part; that is transparent here, so only the
/// compositor's blur shows.
fn install_css(display: &gtk::gdk::Display) {
    thread_local! {
        static DONE: Cell<bool> = const { Cell::new(false) };
    }
    if DONE.with(|done| done.replace(true)) {
        return;
    }
    let css = gtk::CssProvider::new();
    css.load_from_string(".elavo-effect-blur { backdrop-filter: blur(10px); }");
    gtk::style_context_add_provider_for_display(
        display,
        &css,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
    );
}
