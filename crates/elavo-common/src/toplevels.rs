//! Window list over `zwlr_foreign_toplevel_manager_v1`, on its own Wayland
//! connection and thread so its queue never waits on GDK's. Requests
//! (activate, minimize, close) are sent from the GTK thread; wayland-client
//! proxies are thread-safe.

use std::collections::HashMap;

pub use wayland_client::backend::ObjectId;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{wl_registry, wl_seat};
use wayland_client::{event_created_child, Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols_wlr::foreign_toplevel::v1::client::{
    zwlr_foreign_toplevel_handle_v1::{self, ZwlrForeignToplevelHandleV1},
    zwlr_foreign_toplevel_manager_v1::{self, ZwlrForeignToplevelManagerV1},
};

#[derive(Clone, Debug)]
pub struct Toplevel {
    pub key: ObjectId,
    pub handle: ZwlrForeignToplevelHandleV1,
    pub app_id: String,
    pub title: String,
    pub activated: bool,
    pub minimized: bool,
    pub maximized: bool,
    pub fullscreen: bool,
}

pub enum Event {
    Changed(Toplevel),
    Closed(ObjectId),
}

pub struct Controller {
    conn: Connection,
    seat: wl_seat::WlSeat,
}

impl Controller {
    pub fn activate(&self, toplevel: &Toplevel) {
        if toplevel.minimized {
            toplevel.handle.unset_minimized();
        }
        toplevel.handle.activate(&self.seat);
        let _ = self.conn.flush();
    }

    pub fn minimize(&self, toplevel: &Toplevel) {
        toplevel.handle.set_minimized();
        let _ = self.conn.flush();
    }

    pub fn close(&self, toplevel: &Toplevel) {
        toplevel.handle.close();
        let _ = self.conn.flush();
    }

    /// Unminimize without raising. Used by Show All.
    pub fn restore(&self, toplevel: &Toplevel) {
        toplevel.handle.unset_minimized();
        let _ = self.conn.flush();
    }

    pub fn toggle_maximized(&self, toplevel: &Toplevel) {
        if toplevel.maximized {
            toplevel.handle.unset_maximized();
        } else {
            toplevel.handle.set_maximized();
        }
        let _ = self.conn.flush();
    }

    pub fn toggle_fullscreen(&self, toplevel: &Toplevel) {
        if toplevel.fullscreen {
            toplevel.handle.unset_fullscreen();
        } else {
            toplevel.handle.set_fullscreen(None);
        }
        let _ = self.conn.flush();
    }
}

struct State {
    tx: async_channel::Sender<Event>,
    pending: HashMap<ObjectId, Toplevel>,
}

// zwlr_foreign_toplevel_handle_v1.state values.
const STATE_MAXIMIZED: u32 = 0;
const STATE_MINIMIZED: u32 = 1;
const STATE_ACTIVATED: u32 = 2;
const STATE_FULLSCREEN: u32 = 3;

pub fn start() -> Result<(Controller, async_channel::Receiver<Event>), String> {
    let conn = Connection::connect_to_env().map_err(|e| e.to_string())?;
    let (globals, mut queue) = registry_queue_init::<State>(&conn).map_err(|e| e.to_string())?;
    let qh = queue.handle();
    let manager: ZwlrForeignToplevelManagerV1 = globals
        .bind(&qh, 1..=3, ())
        .map_err(|e| format!("zwlr_foreign_toplevel_manager_v1: {e}"))?;
    let seat: wl_seat::WlSeat = globals
        .bind(&qh, 1..=7, ())
        .map_err(|e| format!("wl_seat: {e}"))?;

    let (tx, rx) = async_channel::unbounded();
    let mut state = State {
        tx,
        pending: HashMap::new(),
    };
    std::thread::Builder::new()
        .name("foreign-toplevel".into())
        .spawn(move || {
            let _manager = manager;
            while queue.blocking_dispatch(&mut state).is_ok() {
                if state.tx.is_closed() {
                    break;
                }
            }
        })
        .map_err(|e| e.to_string())?;
    Ok((Controller { conn, seat }, rx))
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
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

impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(
        _: &mut Self,
        _: &wl_seat::WlSeat,
        _: wl_seat::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwlrForeignToplevelManagerV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ZwlrForeignToplevelManagerV1,
        event: zwlr_foreign_toplevel_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwlr_foreign_toplevel_manager_v1::Event::Toplevel { toplevel } = event {
            state.pending.insert(
                toplevel.id(),
                Toplevel {
                    key: toplevel.id(),
                    handle: toplevel,
                    app_id: String::new(),
                    title: String::new(),
                    activated: false,
                    minimized: false,
                    maximized: false,
                    fullscreen: false,
                },
            );
        }
    }

    event_created_child!(State, ZwlrForeignToplevelManagerV1, [
        zwlr_foreign_toplevel_manager_v1::EVT_TOPLEVEL_OPCODE => (ZwlrForeignToplevelHandleV1, ()),
    ]);
}

impl Dispatch<ZwlrForeignToplevelHandleV1, ()> for State {
    fn event(
        state: &mut Self,
        handle: &ZwlrForeignToplevelHandleV1,
        event: zwlr_foreign_toplevel_handle_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use zwlr_foreign_toplevel_handle_v1::Event as E;
        let id = handle.id();
        match event {
            E::Closed => {
                state.pending.remove(&id);
                let _ = state.tx.send_blocking(Event::Closed(id));
                handle.destroy();
            }
            E::Done => {
                if let Some(toplevel) = state.pending.get(&id) {
                    let _ = state.tx.send_blocking(Event::Changed(toplevel.clone()));
                }
            }
            event => {
                let Some(toplevel) = state.pending.get_mut(&id) else {
                    return;
                };
                match event {
                    E::Title { title } => toplevel.title = title,
                    E::AppId { app_id } => toplevel.app_id = app_id,
                    E::State { state: raw } => {
                        let values: Vec<u32> = raw
                            .as_chunks::<4>()
                            .0
                            .iter()
                            .map(|c| u32::from_ne_bytes(*c))
                            .collect();
                        toplevel.activated = values.contains(&STATE_ACTIVATED);
                        toplevel.minimized = values.contains(&STATE_MINIMIZED);
                        toplevel.maximized = values.contains(&STATE_MAXIMIZED);
                        toplevel.fullscreen = values.contains(&STATE_FULLSCREEN);
                    }
                    _ => {}
                }
            }
        }
    }
}
