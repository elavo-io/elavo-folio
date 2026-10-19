//! One picture per window, when labwc offers the capture protocols.
//! A missing protocol, or a capture that fails, leaves the tile on the icon.

use std::collections::HashMap;
use std::os::fd::{AsFd, FromRawFd, OwnedFd};
use std::time::{Duration, Instant};

use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{wl_buffer, wl_registry, wl_shm, wl_shm_pool};
use wayland_client::{event_created_child, Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::ext::foreign_toplevel_list::v1::client::{
    ext_foreign_toplevel_handle_v1::{self, ExtForeignToplevelHandleV1},
    ext_foreign_toplevel_list_v1::{self, ExtForeignToplevelListV1},
};
use wayland_protocols::ext::image_capture_source::v1::client::{
    ext_foreign_toplevel_image_capture_source_manager_v1::ExtForeignToplevelImageCaptureSourceManagerV1,
    ext_image_capture_source_v1::ExtImageCaptureSourceV1,
};
use wayland_protocols::ext::image_copy_capture::v1::client::{
    ext_image_copy_capture_frame_v1::{self, ExtImageCopyCaptureFrameV1},
    ext_image_copy_capture_manager_v1::{ExtImageCopyCaptureManagerV1, Options},
    ext_image_copy_capture_session_v1::{self, ExtImageCopyCaptureSessionV1},
};

use elavo_switcher_settings::previews_available;

#[derive(Clone, Debug)]
pub struct Shot {
    pub app_id: String,
    pub title: String,
    pub width: i32,
    pub height: i32,
    pub rgba: Vec<u8>,
}

pub struct Previews {
    tx: async_channel::Sender<(String, String)>,
}

pub fn start() -> (Previews, async_channel::Receiver<Shot>) {
    let (req_tx, req_rx) = async_channel::unbounded();
    let (shot_tx, shot_rx) = async_channel::unbounded();
    if previews_available() {
        std::thread::Builder::new()
            .name("switcher-preview".into())
            .spawn(move || run(req_rx, shot_tx))
            .ok();
    }
    (Previews { tx: req_tx }, shot_rx)
}

impl Previews {
    pub fn request(&self, app_id: &str, title: &str) {
        let _ = self.tx.send_blocking((app_id.to_owned(), title.to_owned()));
    }
}

struct Known {
    app_id: String,
    title: String,
    handle: ExtForeignToplevelHandleV1,
}

#[allow(dead_code)]
struct Capture {
    app_id: String,
    title: String,
    source: Option<ExtImageCaptureSourceV1>,
    session: Option<ExtImageCopyCaptureSessionV1>,
    frame: Option<ExtImageCopyCaptureFrameV1>,
    pool: Option<wl_shm_pool::WlShmPool>,
    buffer: Option<wl_buffer::WlBuffer>,
    width: i32,
    height: i32,
    format: Option<u32>,
    ready: bool,
    map: Option<(*mut libc::c_void, usize)>,
}

struct State {
    known: Vec<Known>,
    pending_app: HashMap<wayland_client::backend::ObjectId, (String, String)>,
    captures: Vec<Capture>,
    shm: Option<wl_shm::WlShm>,
    source_manager: Option<ExtForeignToplevelImageCaptureSourceManagerV1>,
    copy_manager: Option<ExtImageCopyCaptureManagerV1>,
    shots: async_channel::Sender<Shot>,
}

fn run(requests: async_channel::Receiver<(String, String)>, shots: async_channel::Sender<Shot>) {
    let Ok(conn) = Connection::connect_to_env() else {
        return;
    };
    let Ok((globals, mut queue)) = registry_queue_init::<State>(&conn) else {
        return;
    };
    let qh = queue.handle();
    let Ok(list) = globals.bind::<ExtForeignToplevelListV1, _, _>(&qh, 1..=1, ()) else {
        return;
    };
    let Ok(source_manager) =
        globals.bind::<ExtForeignToplevelImageCaptureSourceManagerV1, _, _>(&qh, 1..=1, ())
    else {
        return;
    };
    let Ok(copy_manager) = globals.bind::<ExtImageCopyCaptureManagerV1, _, _>(&qh, 1..=1, ())
    else {
        return;
    };
    let Ok(shm) = globals.bind::<wl_shm::WlShm, _, _>(&qh, 1..=1, ()) else {
        return;
    };
    let mut state = State {
        known: Vec::new(),
        pending_app: HashMap::new(),
        captures: Vec::new(),
        shm: Some(shm),
        source_manager: Some(source_manager),
        copy_manager: Some(copy_manager),
        shots,
    };
    let _list = list;
    let started = Instant::now();
    while started.elapsed() < Duration::from_millis(400) {
        if queue.blocking_dispatch(&mut state).is_err() {
            return;
        }
        if !state.known.is_empty() {
            break;
        }
    }
    while let Ok((app_id, title)) = requests.recv_blocking() {
        begin(&mut state, &qh, &app_id, &title);
        let deadline = Instant::now() + Duration::from_millis(700);
        while Instant::now() < deadline && state.captures.iter().any(|c| !c.ready) {
            if queue.blocking_dispatch(&mut state).is_err() {
                return;
            }
        }
        state.captures.clear();
    }
}

fn begin(state: &mut State, qh: &QueueHandle<State>, app_id: &str, title: &str) {
    let Some(known) = state
        .known
        .iter()
        .find(|known| known.app_id == app_id && (title.is_empty() || known.title == title))
    else {
        return;
    };
    let (Some(sources), Some(copies)) =
        (state.source_manager.as_ref(), state.copy_manager.as_ref())
    else {
        return;
    };
    let source = sources.create_source(&known.handle, qh, ());
    let session = copies.create_session(&source, Options::empty(), qh, state.captures.len() as u32);
    state.captures.push(Capture {
        app_id: app_id.to_owned(),
        title: title.to_owned(),
        source: Some(source),
        session: Some(session),
        frame: None,
        pool: None,
        buffer: None,
        width: 0,
        height: 0,
        format: None,
        ready: false,
        map: None,
    });
}

fn xrgb_to_rgba(src: &[u8], width: i32, height: i32) -> Vec<u8> {
    let mut out = Vec::with_capacity((width * height * 4) as usize);
    let (pixels, _) = src.as_chunks::<4>();
    for px in pixels.iter().take((width * height) as usize) {
        out.extend_from_slice(&[px[2], px[1], px[0], 255]);
    }
    out
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

impl Dispatch<wl_shm::WlShm, ()> for State {
    fn event(
        _: &mut Self,
        _: &wl_shm::WlShm,
        _: wl_shm::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_shm_pool::WlShmPool, ()> for State {
    fn event(
        _: &mut Self,
        _: &wl_shm_pool::WlShmPool,
        _: wl_shm_pool::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_buffer::WlBuffer, ()> for State {
    fn event(
        _: &mut Self,
        _: &wl_buffer::WlBuffer,
        _: wl_buffer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ExtForeignToplevelImageCaptureSourceManagerV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ExtForeignToplevelImageCaptureSourceManagerV1,
        _: wayland_protocols::ext::image_capture_source::v1::client::ext_foreign_toplevel_image_capture_source_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ExtImageCaptureSourceV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ExtImageCaptureSourceV1,
        _: wayland_protocols::ext::image_capture_source::v1::client::ext_image_capture_source_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ExtImageCopyCaptureManagerV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ExtImageCopyCaptureManagerV1,
        _: wayland_protocols::ext::image_copy_capture::v1::client::ext_image_copy_capture_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ExtForeignToplevelListV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ExtForeignToplevelListV1,
        event: ext_foreign_toplevel_list_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let ext_foreign_toplevel_list_v1::Event::Toplevel { toplevel } = event {
            state
                .pending_app
                .insert(toplevel.id(), (String::new(), String::new()));
            state.known.push(Known {
                app_id: String::new(),
                title: String::new(),
                handle: toplevel,
            });
        }
    }

    event_created_child!(State, ExtForeignToplevelListV1, [
        ext_foreign_toplevel_list_v1::EVT_TOPLEVEL_OPCODE => (ExtForeignToplevelHandleV1, ()),
    ]);
}

impl Dispatch<ExtForeignToplevelHandleV1, ()> for State {
    fn event(
        state: &mut Self,
        handle: &ExtForeignToplevelHandleV1,
        event: ext_foreign_toplevel_handle_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use ext_foreign_toplevel_handle_v1::Event as E;
        let Some(known) = state.known.iter_mut().find(|known| known.handle == *handle) else {
            return;
        };
        match event {
            E::AppId { app_id } => known.app_id = app_id,
            E::Title { title } => known.title = title,
            E::Closed => {
                state.known.retain(|known| known.handle != *handle);
                handle.destroy();
            }
            _ => {}
        }
    }
}

impl Dispatch<ExtImageCopyCaptureSessionV1, u32> for State {
    fn event(
        state: &mut Self,
        session: &ExtImageCopyCaptureSessionV1,
        event: ext_image_copy_capture_session_v1::Event,
        index: &u32,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        use ext_image_copy_capture_session_v1::Event as E;
        let Some(capture) = state.captures.get_mut(*index as usize) else {
            return;
        };
        match event {
            E::BufferSize { width, height } => {
                capture.width = width as i32;
                capture.height = height as i32;
            }
            E::ShmFormat { format } => {
                let value = u32::from(format);
                if value == u32::from(wl_shm::Format::Xrgb8888)
                    || value == u32::from(wl_shm::Format::Argb8888)
                {
                    capture.format = Some(value);
                }
            }
            E::Done => {
                if capture.frame.is_some() || capture.width <= 0 || capture.format.is_none() {
                    return;
                }
                let Some(shm) = state.shm.as_ref() else {
                    return;
                };
                let stride = capture.width * 4;
                let size = (stride * capture.height) as usize;
                let Some(fd) = memfd(size) else {
                    return;
                };
                let raw = fd.as_raw_fd();
                let mapped = unsafe {
                    libc::mmap(
                        std::ptr::null_mut(),
                        size,
                        libc::PROT_READ,
                        libc::MAP_SHARED,
                        raw,
                        0,
                    )
                };
                if mapped == libc::MAP_FAILED {
                    return;
                }
                capture.map = Some((mapped, size));
                let pool = shm.create_pool(fd.as_fd(), size as i32, qh, ());
                let buffer = pool.create_buffer(
                    0,
                    capture.width,
                    capture.height,
                    stride,
                    wl_shm::Format::Xrgb8888,
                    qh,
                    (),
                );
                let frame = session.create_frame(qh, *index);
                frame.attach_buffer(&buffer);
                frame.damage_buffer(0, 0, capture.width, capture.height);
                frame.capture();
                capture.pool = Some(pool);
                capture.buffer = Some(buffer);
                capture.frame = Some(frame);
            }
            E::Stopped => {
                capture.ready = true;
            }
            _ => {}
        }
    }
}

impl Dispatch<ExtImageCopyCaptureFrameV1, u32> for State {
    fn event(
        state: &mut Self,
        frame: &ExtImageCopyCaptureFrameV1,
        event: ext_image_copy_capture_frame_v1::Event,
        index: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use ext_image_copy_capture_frame_v1::Event as E;
        if !matches!(event, E::Ready | E::Failed { .. }) {
            return;
        }
        let Some(capture) = state.captures.get_mut(*index as usize) else {
            return;
        };
        if matches!(event, E::Ready) {
            if let Some((mapped, size)) = capture.map.take() {
                let src = unsafe { std::slice::from_raw_parts(mapped as *const u8, size) };
                let rgba = xrgb_to_rgba(src, capture.width, capture.height);
                unsafe { libc::munmap(mapped, size) };
                let _ = state.shots.send_blocking(Shot {
                    app_id: capture.app_id.clone(),
                    title: capture.title.clone(),
                    width: capture.width,
                    height: capture.height,
                    rgba,
                });
            }
        } else if let Some((mapped, size)) = capture.map.take() {
            unsafe { libc::munmap(mapped, size) };
        }
        capture.ready = true;
        frame.destroy();
    }
}

fn memfd(size: usize) -> Option<OwnedFd> {
    let fd = unsafe { libc::memfd_create(c"elavo-switcher".as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        return None;
    }
    let owned = unsafe { OwnedFd::from_raw_fd(fd) };
    if unsafe { libc::ftruncate(owned.as_raw_fd(), size as i64) } != 0 {
        return None;
    }
    Some(owned)
}

use std::os::fd::AsRawFd;
