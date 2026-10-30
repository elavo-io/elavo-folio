//! Ask the compositor to blur a shape on GTK's own Wayland connection.
//!
//! `elavo-common`'s foreign-toplevel connection is a different `wl_display`.
//! A surface from GTK's connection cannot be named from that one, so this
//! binds `ext_background_effect_v1` on the display GTK already uses.
//!
//! The registry roundtrip happens once per process. Doing it again from a
//! menu's frame callback reenters GTK's display while the popup is mapping.

use std::cell::RefCell;
use std::os::raw::c_void;

use gdk4_wayland::prelude::WaylandSurfaceExtManual;
use gtk::prelude::*;
use wayland_client::backend::{Backend, ObjectId};
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_region::WlRegion;
use wayland_client::protocol::wl_registry;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::ext::background_effect::v1::client::ext_background_effect_manager_v1::ExtBackgroundEffectManagerV1;
use wayland_protocols::ext::background_effect::v1::client::ext_background_effect_surface_v1::ExtBackgroundEffectSurfaceV1;

struct BlurState;

struct WaylandBlur {
    conn: Connection,
    /// Kept so the proxies bound on this queue stay alive. GTK reads the
    /// shared display; events for this queue stay queued and are not required.
    #[allow(dead_code)]
    queue: wayland_client::EventQueue<BlurState>,
    qh: QueueHandle<BlurState>,
    compositor: WlCompositor,
    manager: ExtBackgroundEffectManagerV1,
}

thread_local! {
    static WAYLAND: RefCell<Option<WaylandBlur>> = const { RefCell::new(None) };
}

pub struct BackgroundBlur {
    conn: Connection,
    effect: ExtBackgroundEffectSurfaceV1,
    compositor: WlCompositor,
    qh: QueueHandle<BlurState>,
}

impl BackgroundBlur {
    /// Read the blur global once, on the display `widget` is on. Menus can
    /// then attach without a roundtrip inside their frame callback.
    pub fn prepare_display(widget: &gtk::Widget) -> bool {
        let Ok(display) = widget.display().downcast::<gdk4_wayland::WaylandDisplay>() else {
            return false;
        };
        let Some(wl_display) = display.wl_display_raw() else {
            return false;
        };
        ensure(wl_display.as_ptr().cast())
    }

    /// Bind blur for the toplevel that owns `widget`. None when the
    /// compositor does not offer `ext_background_effect_v1`.
    pub fn for_widget(widget: &gtk::Widget) -> Option<Self> {
        let surface = widget.native()?.surface()?;
        let wayland_surface = surface.downcast::<gdk4_wayland::WaylandSurface>().ok()?;
        let display = widget
            .display()
            .downcast::<gdk4_wayland::WaylandDisplay>()
            .ok()?;
        let wl_display = display.wl_display_raw()?.as_ptr();
        let wl_surface = wayland_surface.wl_surface_raw()?.as_ptr();
        Self::attach(wl_display.cast(), wl_surface.cast())
    }

    /// `wl_display` and `wl_surface` are GTK's. Returns none when this
    /// compositor does not offer the protocol, or the pointers are null.
    pub fn attach(wl_display: *mut c_void, wl_surface: *mut c_void) -> Option<Self> {
        if wl_display.is_null() || wl_surface.is_null() || !ensure(wl_display) {
            return None;
        }
        let surface_id =
            unsafe { ObjectId::from_ptr(WlSurface::interface(), wl_surface.cast()) }.ok()?;
        WAYLAND.with(|slot| {
            let slot = slot.borrow();
            let wayland = slot.as_ref()?;
            let surface = WlSurface::from_id(&wayland.conn, surface_id).ok()?;
            let effect = wayland
                .manager
                .get_background_effect(&surface, &wayland.qh, ());
            let blur = Self {
                conn: wayland.conn.clone(),
                effect,
                compositor: wayland.compositor.clone(),
                qh: wayland.qh.clone(),
            };
            let _ = blur.conn.flush();
            Some(blur)
        })
    }

    /// Replace the blur region with these rounded rectangles, in surface-local
    /// pixels. An empty list removes the effect.
    pub fn set_rounds(&self, parts: &[(i32, i32, i32, i32, i32)]) {
        if parts.is_empty() {
            self.effect.set_blur_region(None);
            let _ = self.conn.flush();
            return;
        }
        let region = self.compositor.create_region(&self.qh, ());
        for &(x, y, w, h, radius) in parts {
            add_round_rect(&region, x, y, w, h, radius);
        }
        self.effect.set_blur_region(Some(&region));
        region.destroy();
        let _ = self.conn.flush();
    }

    /// Destroy the effect object now. A menu does this when it closes, so the
    /// compositor drops the blur while the surface's scene node still exists,
    /// and the surface can ask for a new one when it opens again.
    pub fn release(self) {
        drop(self);
    }
}

impl Drop for BackgroundBlur {
    /// Dropping a proxy does not send its destructor. Without this the
    /// compositor keeps the effect, and the next `get_background_effect` on
    /// the same surface is a protocol error that disconnects the client.
    fn drop(&mut self) {
        self.effect.destroy();
        let _ = self.conn.flush();
    }
}

fn ensure(wl_display: *mut c_void) -> bool {
    if WAYLAND.with(|slot| slot.borrow().is_some()) {
        return true;
    }
    let Some(wayland) = connect(wl_display) else {
        return false;
    };
    WAYLAND.with(|slot| {
        if slot.borrow().is_none() {
            *slot.borrow_mut() = Some(wayland);
        }
    });
    WAYLAND.with(|slot| slot.borrow().is_some())
}

fn connect(wl_display: *mut c_void) -> Option<WaylandBlur> {
    let backend = unsafe { Backend::from_foreign_display(wl_display.cast()) };
    let conn = Connection::from_backend(backend);
    let (globals, queue) = registry_queue_init::<BlurState>(&conn).ok()?;
    let qh = queue.handle();
    let manager = globals.bind(&qh, 1..=1, ()).ok()?;
    let compositor = globals.bind(&qh, 1..=6, ()).ok()?;
    let _ = conn.flush();
    Some(WaylandBlur {
        conn,
        queue,
        qh,
        compositor,
        manager,
    })
}

fn add_round_rect(region: &WlRegion, x: i32, y: i32, w: i32, h: i32, radius: i32) {
    if w <= 0 || h <= 0 {
        return;
    }
    let radius = radius.max(0).min(w / 2).min(h / 2);
    if radius <= 1 {
        region.add(x, y, w, h);
        return;
    }
    for row in 0..radius {
        let dy = radius - row;
        let inset = radius - ((radius * radius - dy * dy) as f64).sqrt().round() as i32;
        let inset = inset.clamp(0, w / 2);
        region.add(x + inset, y + row, w - 2 * inset, 1);
    }
    region.add(x, y + radius, w, (h - 2 * radius).max(0));
    for row in 0..radius {
        let dy = row + 1;
        let inset = radius - ((radius * radius - dy * dy) as f64).sqrt().round() as i32;
        let inset = inset.clamp(0, w / 2);
        region.add(x + inset, y + h - radius + row, w - 2 * inset, 1);
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for BlurState {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

macro_rules! empty_dispatch {
    ($($ty:ty),* $(,)?) => {$(
        impl Dispatch<$ty, ()> for BlurState {
            fn event(
                _: &mut Self,
                _: &$ty,
                _: <$ty as Proxy>::Event,
                _: &(),
                _: &Connection,
                _: &QueueHandle<Self>,
            ) {
            }
        }
    )*};
}

empty_dispatch!(
    WlCompositor,
    WlRegion,
    ExtBackgroundEffectManagerV1,
    ExtBackgroundEffectSurfaceV1,
);
