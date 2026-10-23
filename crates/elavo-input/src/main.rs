//! Turns two spaces into a period while a text field is active.
//!
//! labwc hands this process the keyboard for the seat. Every key is sent back
//! through a virtual keyboard, except the second space of a quick pair and the
//! Backspace that undoes it. Those become text commits. The rule itself is in
//! `decide`.

mod config;
mod decide;
mod protocol;

use std::io::{self, ErrorKind};
use std::os::fd::{AsFd, AsRawFd, OwnedFd};
use std::time::{Duration, Instant};

use wayland_client::backend::WaylandError;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::wl_keyboard::{KeyState, KeymapFormat};
use wayland_client::protocol::wl_registry;
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::{event_created_child, Connection, Dispatch, Proxy, QueueHandle, WEnum};

use crate::config::{load_path, Watch};
use crate::decide::{
    decide, substitution_still_there, Action, Echo, Field, Input, Memory, Snapshot,
};
use crate::protocol::input_method::zwp_input_method_keyboard_grab_v2::{
    Event as GrabEvent, ZwpInputMethodKeyboardGrabV2,
};
use crate::protocol::input_method::zwp_input_method_manager_v2::ZwpInputMethodManagerV2;
use crate::protocol::input_method::zwp_input_method_v2::{Event as ImEvent, ZwpInputMethodV2};
use crate::protocol::input_method::zwp_text_input_v3::ContentPurpose;
use crate::protocol::virtual_keyboard::zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1;
use crate::protocol::virtual_keyboard::zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1;

/// Evdev codes that do not repeat into text.
const MODIFIERS: &[u32] = &[29, 42, 54, 56, 58, 69, 97, 100, 125, 126];

struct Repeat {
    key: u32,
    next: Instant,
    time: u32,
    first: bool,
}

struct Pending {
    activate: bool,
    deactivate: bool,
    surrounding: Option<Snapshot>,
    purpose: Option<u32>,
}

impl Pending {
    fn clear_text(&mut self) {
        self.surrounding = None;
        self.purpose = None;
    }
}

struct State {
    im: Option<ZwpInputMethodV2>,
    grab: Option<ZwpInputMethodKeyboardGrabV2>,
    vk: Option<ZwpVirtualKeyboardV1>,
    _im_manager: Option<ZwpInputMethodManagerV2>,
    _vk_manager: Option<ZwpVirtualKeyboardManagerV1>,
    _seat: Option<WlSeat>,

    keymap_set: bool,
    serial: u32,
    active: bool,
    enabled: bool,
    purpose: u32,
    modifiers: bool,
    depressed: u32,
    latched: u32,
    locked: u32,
    group: u32,
    rate: i32,
    delay: i32,
    field: Field,
    memory: Memory,
    pending: Pending,
    swallowed: Vec<u32>,
    repeat: Option<Repeat>,
    config_path: Option<std::path::PathBuf>,
    config_mtime: Option<std::time::SystemTime>,
    watch: Option<Watch>,
    dead: bool,
    trace: bool,
}

impl State {
    fn new() -> Self {
        let config_path = config::config_path();
        let enabled = config_path.as_deref().map(load_path).unwrap_or(true);
        let config_mtime = config_path.as_deref().and_then(mtime);
        let watch = config_path
            .as_deref()
            .and_then(|path| Watch::open(path).ok());
        Self {
            im: None,
            grab: None,
            vk: None,
            _im_manager: None,
            _vk_manager: None,
            _seat: None,
            keymap_set: false,
            serial: 0,
            active: false,
            enabled,
            purpose: 0,
            modifiers: false,
            depressed: 0,
            latched: 0,
            locked: 0,
            group: 0,
            rate: 0,
            delay: 0,
            field: Field::default(),
            memory: Memory::default(),
            pending: Pending {
                activate: false,
                deactivate: false,
                surrounding: None,
                purpose: None,
            },
            swallowed: Vec::new(),
            repeat: None,
            config_path,
            config_mtime,
            watch,
            dead: false,
            trace: std::env::var_os("ELAVO_INPUT_TRACE").is_some(),
        }
    }

    fn reload_config(&mut self) {
        let Some(path) = &self.config_path else {
            return;
        };
        self.config_mtime = mtime(path);
        let enabled = load_path(path);
        if enabled != self.enabled {
            self.log(&format!(
                "add period with double-space {}",
                if enabled { "on" } else { "off" }
            ));
        }
        self.enabled = enabled;
    }

    fn on_key(&mut self, time: u32, key: u32, pressed: bool) {
        self.log(&format!(
            "key {key} down={pressed} active={} known={} cursor={}",
            self.active, self.field.known, self.field.current.cursor
        ));
        if !pressed {
            let swallowed = take(&mut self.swallowed, key);
            if self.repeat.as_ref().is_some_and(|repeat| repeat.key == key) {
                self.repeat = None;
            }
            if !swallowed {
                self.send_key(time, key, false);
            }
            return;
        }

        self.repeat = None;
        let outcome = decide(
            &Input {
                enabled: self.enabled,
                active: self.active,
                purpose: self.purpose,
                modifiers: self.modifiers,
                text: &self.field.current.text,
                cursor: self.field.current.cursor,
                anchor: self.field.current.anchor,
                known: self.field.known,
                memory: &self.memory,
                now: time,
            },
            key,
            true,
        );
        self.memory = outcome.memory;
        match outcome.action {
            Action::Forward => {
                self.send_key(time, key, true);
                if outcome.note_space {
                    self.field.note_space();
                }
                self.arm_repeat(key, time);
            }
            Action::Replace { before, text } => {
                if self.commit_replace(before, text) {
                    self.swallow(key);
                    self.log(&format!("commit {text:?} deleting {before}"));
                } else {
                    self.send_key(time, key, true);
                    self.arm_repeat(key, time);
                }
            }
        }
    }

    fn commit_replace(&mut self, before: u32, text: &str) -> bool {
        if self.serial == 0 {
            return false;
        }
        let Some(im) = &self.im else {
            return false;
        };
        if before > 0 {
            im.delete_surrounding_text(before, 0);
        }
        im.commit_string(text.to_owned());
        im.commit(self.serial);
        self.field.replace(before, text);
        true
    }

    fn send_key(&self, time: u32, key: u32, pressed: bool) {
        if !self.keymap_set {
            return;
        }
        let Some(vk) = &self.vk else {
            return;
        };
        vk.key(time, key, u32::from(pressed));
    }

    fn send_modifiers(&self) {
        if !self.keymap_set {
            return;
        }
        let Some(vk) = &self.vk else {
            return;
        };
        vk.modifiers(self.depressed, self.latched, self.locked, self.group);
    }

    fn arm_repeat(&mut self, key: u32, time: u32) {
        if MODIFIERS.contains(&key) || self.rate <= 0 || self.delay < 0 {
            return;
        }
        self.repeat = Some(Repeat {
            key,
            next: Instant::now() + Duration::from_millis(self.delay as u64),
            time,
            first: true,
        });
    }

    fn fire_repeat(&mut self) {
        let (delay_ms, interval_ms) = repeat_steps(self.rate, self.delay);
        let Some(interval_ms) = interval_ms else {
            return;
        };
        let Some(repeat) = &self.repeat else {
            return;
        };
        if Instant::now() < repeat.next {
            return;
        }
        let key = repeat.key;
        let step = if repeat.first { delay_ms } else { interval_ms };
        let time = repeat.time.wrapping_add(step);
        if let Some(repeat) = &mut self.repeat {
            repeat.time = time;
            repeat.first = false;
            repeat.next = Instant::now() + Duration::from_millis(interval_ms as u64);
        }
        self.send_key(time, key, true);
    }

    fn repeat_timeout_ms(&self) -> i32 {
        let Some(repeat) = &self.repeat else {
            return if self.watch.is_some() { -1 } else { 1_000 };
        };
        let left = repeat.next.saturating_duration_since(Instant::now());
        left.as_millis().min(i32::MAX as u128) as i32
    }

    fn swallow(&mut self, key: u32) {
        if !self.swallowed.contains(&key) {
            self.swallowed.push(key);
        }
    }

    fn apply_done(&mut self) {
        self.serial = self.serial.saturating_add(1);
        if self.pending.activate {
            self.active = true;
            self.field.clear();
            self.memory = Memory::default();
        }
        if self.pending.deactivate {
            self.active = false;
            self.field.clear();
            self.memory = Memory::default();
            self.repeat = None;
        }
        if let Some(purpose) = self.pending.purpose.take() {
            self.purpose = purpose;
        } else if self.pending.activate {
            self.purpose = 0;
        }
        if let Some(snap) = self.pending.surrounding.take() {
            let echo = self.field.apply_remote(snap, true);
            self.note_echo(echo);
        } else if self.pending.activate {
            self.field.known = false;
        }
        self.pending.activate = false;
        self.pending.deactivate = false;
    }

    fn note_echo(&mut self, echo: Echo) {
        if echo == Echo::Stale {
            return;
        }
        let snap = &self.field.current;
        if self.memory.last_was_substitution
            && !substitution_still_there(&snap.text, snap.cursor, snap.anchor)
        {
            self.memory.last_was_substitution = false;
        }
    }

    fn log(&self, msg: &str) {
        if self.trace {
            eprintln!("elavo-input: {msg}");
        }
    }
}

fn repeat_steps(rate: i32, delay: i32) -> (u32, Option<u32>) {
    if rate <= 0 || delay < 0 {
        return (0, None);
    }
    let interval = (1000 / rate as u32).max(1);
    (delay as u32, Some(interval))
}

fn take(keys: &mut Vec<u32>, key: u32) -> bool {
    if let Some(index) = keys.iter().position(|held| *held == key) {
        keys.swap_remove(index);
        true
    } else {
        false
    }
}

fn mtime(path: &std::path::Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
}

fn keymap_format(format: WEnum<KeymapFormat>) -> u32 {
    match format {
        WEnum::Value(KeymapFormat::XkbV1) => 1,
        WEnum::Unknown(value) => value,
        WEnum::Value(_) => 0,
    }
}

fn is_pressed(state: WEnum<KeyState>) -> bool {
    match state {
        WEnum::Value(KeyState::Pressed) | WEnum::Unknown(1) => true,
        WEnum::Value(_) | WEnum::Unknown(_) => false,
    }
}

fn purpose_code(purpose: WEnum<ContentPurpose>) -> u32 {
    match purpose {
        WEnum::Value(ContentPurpose::Password) => decide::PURPOSE_PASSWORD,
        WEnum::Value(ContentPurpose::Pin) => decide::PURPOSE_PIN,
        WEnum::Value(ContentPurpose::Terminal) => decide::PURPOSE_TERMINAL,
        WEnum::Unknown(value) => value,
        WEnum::Value(_) => 0,
    }
}

fn main() {
    if let Err(err) = run() {
        eprintln!("elavo-input: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let conn = Connection::connect_to_env().map_err(|err| err.to_string())?;
    let (globals, mut queue) =
        registry_queue_init::<State>(&conn).map_err(|err| err.to_string())?;
    let qh = queue.handle();

    let seat: WlSeat = globals
        .bind(&qh, 1..=9, ())
        .map_err(|_| "no seat".to_owned())?;
    let im_manager: ZwpInputMethodManagerV2 = match globals.bind(&qh, 1..=1, ()) {
        Ok(manager) => manager,
        Err(_) => {
            eprintln!("elavo-input: input method is not offered");
            return Ok(());
        }
    };
    let vk_manager: ZwpVirtualKeyboardManagerV1 = match globals.bind(&qh, 1..=1, ()) {
        Ok(manager) => manager,
        Err(_) => {
            eprintln!("elavo-input: virtual keyboard is not offered");
            return Ok(());
        }
    };

    let mut state = State::new();
    state.im = Some(im_manager.get_input_method(&seat, &qh, ()));
    state.vk = Some(vk_manager.create_virtual_keyboard(&seat, &qh, ()));
    if let Some(im) = &state.im {
        state.grab = Some(im.grab_keyboard(&qh, ()));
    }
    state._im_manager = Some(im_manager);
    state._vk_manager = Some(vk_manager);
    state._seat = Some(seat);
    state.log(&format!(
        "ready, period {}",
        if state.enabled { "on" } else { "off" }
    ));

    let wl = conn.backend().poll_fd().as_raw_fd();
    loop {
        queue
            .dispatch_pending(&mut state)
            .map_err(|err| err.to_string())?;
        if state.dead {
            eprintln!("elavo-input: input method unavailable");
            return Ok(());
        }
        state.fire_repeat();

        let need_out = match conn.flush() {
            Ok(()) => false,
            Err(WaylandError::Io(err)) if err.kind() == ErrorKind::WouldBlock => true,
            Err(err) => return Err(err.to_string()),
        };

        if let Some(guard) = queue.prepare_read() {
            let watch_fd = state.watch.as_ref().map(Watch::fd);
            let (wl_ready, cfg_ready) = wake(wl, watch_fd, state.repeat_timeout_ms(), need_out)
                .map_err(|err| err.to_string())?;
            if wl_ready {
                if let Err(err) = guard.read() {
                    return Err(err.to_string());
                }
            } else {
                drop(guard);
            }
            if cfg_ready {
                let reload = state.watch.as_mut().is_some_and(Watch::changed);
                if reload {
                    state.reload_config();
                }
            } else if state.watch.is_none() {
                let path = state.config_path.clone();
                let stamped = path.as_deref().and_then(mtime);
                if stamped != state.config_mtime {
                    state.reload_config();
                }
            }
        }
    }
}

fn wake(wl: i32, watch: Option<i32>, timeout_ms: i32, need_out: bool) -> io::Result<(bool, bool)> {
    let mut fds = [
        libc::pollfd {
            fd: wl,
            events: libc::POLLIN | if need_out { libc::POLLOUT } else { 0 },
            revents: 0,
        },
        libc::pollfd {
            fd: watch.unwrap_or(-1),
            events: libc::POLLIN,
            revents: 0,
        },
    ];
    let count = if watch.is_some() { 2 } else { 1 };
    loop {
        let rc = unsafe { libc::poll(fds.as_mut_ptr(), count, timeout_ms) };
        if rc < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }
        let ready = fds[0].revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0;
        let cfg = count == 2 && fds[1].revents & libc::POLLIN != 0;
        return Ok((ready, cfg));
    }
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

impl Dispatch<WlSeat, ()> for State {
    fn event(
        _: &mut Self,
        _: &WlSeat,
        _: <WlSeat as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwpInputMethodManagerV2, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZwpInputMethodManagerV2,
        _: <ZwpInputMethodManagerV2 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }

    event_created_child!(State, ZwpInputMethodManagerV2, [
        crate::protocol::input_method::zwp_input_method_manager_v2::REQ_GET_INPUT_METHOD_OPCODE => (ZwpInputMethodV2, ()),
    ]);
}

impl Dispatch<ZwpVirtualKeyboardManagerV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZwpVirtualKeyboardManagerV1,
        _: <ZwpVirtualKeyboardManagerV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }

    event_created_child!(State, ZwpVirtualKeyboardManagerV1, [
        crate::protocol::virtual_keyboard::zwp_virtual_keyboard_manager_v1::REQ_CREATE_VIRTUAL_KEYBOARD_OPCODE => (ZwpVirtualKeyboardV1, ()),
    ]);
}

impl Dispatch<ZwpInputMethodV2, ()> for State {
    fn event(
        state: &mut Self,
        _: &ZwpInputMethodV2,
        event: <ZwpInputMethodV2 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ImEvent::Activate => {
                state.log("activate");
                state.pending.activate = true;
                state.pending.deactivate = false;
                state.pending.clear_text();
            }
            ImEvent::Deactivate => {
                state.log("deactivate");
                state.pending.deactivate = true;
                state.pending.activate = false;
            }
            ImEvent::SurroundingText {
                text,
                cursor,
                anchor,
            } => {
                state.pending.surrounding = Some(Snapshot {
                    text,
                    cursor,
                    anchor,
                });
            }
            ImEvent::TextChangeCause { .. } => {}
            ImEvent::ContentType { purpose, .. } => {
                state.pending.purpose = Some(purpose_code(purpose));
            }
            ImEvent::Done => {
                state.apply_done();
                state.log(&format!(
                    "done active={} purpose={} known={}",
                    state.active, state.purpose, state.field.known
                ));
            }
            ImEvent::Unavailable => state.dead = true,
        }
    }

    event_created_child!(State, ZwpInputMethodV2, [
        crate::protocol::input_method::zwp_input_method_v2::REQ_GRAB_KEYBOARD_OPCODE => (ZwpInputMethodKeyboardGrabV2, ()),
    ]);
}

impl Dispatch<ZwpInputMethodKeyboardGrabV2, ()> for State {
    fn event(
        state: &mut Self,
        _: &ZwpInputMethodKeyboardGrabV2,
        event: <ZwpInputMethodKeyboardGrabV2 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            GrabEvent::Keymap { format, fd, size } => {
                state.log("keymap");
                state.on_keymap(format, fd, size);
            }
            GrabEvent::Key {
                time,
                key,
                state: key_state,
                ..
            } => {
                state.on_key(time, key, is_pressed(key_state));
            }
            GrabEvent::Modifiers {
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
                ..
            } => {
                state.depressed = mods_depressed;
                state.latched = mods_latched;
                state.locked = mods_locked;
                state.group = group;
                state.modifiers = mods_depressed != 0 || mods_latched != 0;
                state.send_modifiers();
            }
            GrabEvent::RepeatInfo { rate, delay } => {
                state.rate = rate;
                state.delay = delay;
            }
        }
    }
}

impl Dispatch<ZwpVirtualKeyboardV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZwpVirtualKeyboardV1,
        _: <ZwpVirtualKeyboardV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl State {
    fn on_keymap(&mut self, format: WEnum<KeymapFormat>, fd: OwnedFd, size: u32) {
        {
            if let Some(vk) = &self.vk {
                vk.keymap(keymap_format(format), fd.as_fd(), size);
            }
        }
        self.keymap_set = true;
        self.send_modifiers();
    }
}
