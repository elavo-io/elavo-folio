//! The Elavo login screen and lock screen. Spec:
//! elavo-architecture-and-planning-internal, planning/desktop/greeter.md.
//!
//! ```text
//! elavo-greeter                 login, run by greetd inside cage
//! elavo-greeter --lock          lock this session; returns once locked
//! elavo-greeter --demo [--lock] [--show list|name] [--demo-people N]
//!                               the same screen in a window; the password is "elavo".
//!                               --demo-people adds up to 8 made-up people to the list
//! elavo-greeter --demo --lock --session-lock
//!                               a real lock of the compositor it runs in, for testing
//!                               in a nested session; the password is "elavo"
//! ```
//!
//! `--lock` starts a keeper that starts the lock screen, and starts it again
//! if it stops while the screen is still locked. Both write to
//! `~/.local/state/elavo/lock.log`.

mod accounts;
mod avatar;
mod config;
mod frost;
mod glass;
mod greetd;
mod locklog;
mod logind;
mod nav;
mod pam;
mod scale;
mod screen;
mod status;
mod trace;
mod wallpaper;

use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::process::{Command, ExitCode, Stdio};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use config::Show;
use screen::{Backend, Kind, Screen};

const LOCK_APP_ID: &str = "io.elavo.Greeter.Lock";
/// The lock screen's exit status when the compositor will not lock. The
/// keeper does not start it again after this one.
const LOCK_FAILED: u8 = 3;
/// How often the lock checks that its password field is on screen, and how
/// many checks in a row may miss it before the lock windows are rebuilt.
const LOCK_CHECK: std::time::Duration = std::time::Duration::from_secs(2);
const LOCK_CHECK_MISSES: u32 = 2;
/// How long the login window waits for the wallpaper before it appears.
/// The lock does not wait: it covers the session first, and the picture
/// arrives on the window that is already up.
const WALLPAPER_WAIT: std::time::Duration = std::time::Duration::from_millis(1200);

fn first_monitor_aspect() -> Option<f64> {
    let display = gdk::Display::default()?;
    let monitor = display.monitors().item(0).and_downcast::<gdk::Monitor>()?;
    let geometry = monitor.geometry();
    (geometry.height() > 0).then(|| f64::from(geometry.width()) / f64::from(geometry.height()))
}

/// Set up GL while the wallpaper decodes on another thread, instead of when
/// the first window is realized.
fn warm_up_renderer() {
    if let Some(display) = gdk::Display::default() {
        if let Err(e) = display.prepare_gl() {
            glib::g_debug!("elavo-greeter", "no GL: {e}");
        }
    }
    trace::mark("GL ready");
}

fn trace_first_frame(window: &impl IsA<gtk::Widget>) {
    window.connect_realize(|w| {
        trace::mark("window realized");
        if let Some(clock) = w.frame_clock() {
            let id = Rc::new(std::cell::Cell::new(None));
            let id2 = id.clone();
            let handler = clock.connect_after_paint(move |clock| {
                trace::mark("first frame painted");
                if let Some(id) = id2.take() {
                    clock.disconnect(id);
                }
            });
            id.set(Some(handler));
        }
    });
}

/// Runs `f` once, when the wallpaper is ready or after `WALLPAPER_WAIT`,
/// whichever comes first.
fn after_wallpaper(screen: &Rc<Screen>, f: impl FnOnce() + 'static) {
    let f = Rc::new(std::cell::RefCell::new(Some(f)));
    let run = {
        let f = f.clone();
        move || {
            if let Some(f) = f.borrow_mut().take() {
                f();
            }
        }
    };
    let pending = f.clone();
    let timeout = run.clone();
    glib::timeout_add_local_once(WALLPAPER_WAIT, move || {
        if pending.borrow().is_some() {
            trace::mark("wallpaper wait timed out");
        }
        timeout();
    });
    screen.when_ready(run);
}

struct Args {
    lock: bool,
    foreground: bool,
    /// The keeper `--lock` starts. Not for people to use.
    keep: bool,
    session_lock: bool,
    demo: bool,
    demo_people: usize,
    show: Option<Show>,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        lock: false,
        foreground: false,
        keep: false,
        session_lock: false,
        demo: false,
        demo_people: 0,
        show: None,
    };
    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--lock" => args.lock = true,
            "--foreground" => args.foreground = true,
            "--keep" => args.keep = true,
            "--session-lock" => args.session_lock = true,
            "--demo" => args.demo = true,
            "--show" => {
                args.show = Some(match iter.next().as_deref() {
                    Some("list") => Show::List,
                    Some("name") => Show::Name,
                    _ => return Err("--show takes list or name".into()),
                })
            }
            "--demo-people" => {
                args.demo_people = iter
                    .next()
                    .and_then(|n| n.parse().ok())
                    .ok_or("--demo-people takes a number")?;
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    if args.demo_people > 0 && !args.demo {
        return Err("--demo-people needs --demo".into());
    }
    if args.session_lock && !(args.demo && args.lock) {
        return Err("--session-lock needs --demo --lock".into());
    }
    Ok(args)
}

fn load_style() {
    let display = gdk::Display::default().expect("no display");
    // The password field's caret blinks only when this is on. A session or
    // the greetd user can have it off; the greeter wants it either way.
    if let Some(settings) = gtk::Settings::default() {
        settings.set_gtk_cursor_blink(true);
    }
    let css = gtk::CssProvider::new();
    css.load_from_resource("/io/elavo/Greeter/style.css");
    gtk::style_context_add_provider_for_display(
        &display,
        &css,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
    gtk::IconTheme::for_display(&display).add_resource_path("/io/elavo/Greeter/icons");
}

impl Args {
    /// A lock of the compositor, real or for testing, rather than a window.
    fn locks_session(&self) -> bool {
        self.lock && (!self.demo || self.session_lock)
    }

    /// What the keeper and the lock screen are started with besides
    /// `--lock`.
    fn lock_flags(&self) -> &'static [&'static str] {
        if self.demo {
            &["--demo", "--session-lock"]
        } else {
            &[]
        }
    }
}

/// Reads the lock screen's one byte that says the screen is locked.
fn wait_for_locked(child: &mut std::process::Child) -> bool {
    let mut ready = [0u8; 1];
    child
        .stdout
        .take()
        .is_some_and(|mut out| matches!(out.read(&mut ready), Ok(1)))
}

/// Start the keeper and return once the compositor has locked the screen,
/// so `swayidle`'s `before-sleep` waits for the lock.
fn lock_and_detach(args: &Args) -> ExitCode {
    let Ok(exe) = std::env::current_exe() else {
        return ExitCode::FAILURE;
    };
    let mut child = match Command::new(exe)
        .args(["--lock", "--keep"])
        .args(args.lock_flags())
        .stdout(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            eprintln!("elavo-greeter: {e}");
            return ExitCode::FAILURE;
        }
    };
    if wait_for_locked(&mut child) {
        return ExitCode::SUCCESS;
    }
    match child.wait() {
        Ok(status) if status.success() => ExitCode::SUCCESS,
        _ => ExitCode::FAILURE,
    }
}

/// Run the lock screen until it unlocks. If it stops while the screen is
/// locked (a crash, or GTK giving up on the compositor), the compositor
/// keeps the session locked with nothing to type into, so start it again.
/// Never unlocks anything itself.
fn keep_lock(args: &Args) -> ExitCode {
    locklog::open();
    let Ok(exe) = std::env::current_exe() else {
        return ExitCode::FAILURE;
    };
    let mut first = true;
    let mut restarts: Vec<std::time::Instant> = Vec::new();
    loop {
        let spawned = Command::new(&exe)
            .args(["--lock", "--foreground"])
            .args(args.lock_flags())
            .stdout(Stdio::piped())
            .spawn();
        let mut child = match spawned {
            Ok(child) => child,
            Err(e) => {
                locklog::note(&format!("could not start the lock screen: {e}"));
                if first {
                    return ExitCode::FAILURE;
                }
                std::thread::sleep(std::time::Duration::from_secs(1));
                continue;
            }
        };
        let locked = wait_for_locked(&mut child);
        if first {
            first = false;
            if !locked {
                // Not locked: the compositor refused, or a lock is already up
                // and this one handed over to it.
                let status = child.wait();
                locklog::note(&format!("the lock screen ended without locking: {status:?}"));
                return match status {
                    Ok(status) if status.success() => ExitCode::SUCCESS,
                    _ => ExitCode::FAILURE,
                };
            }
            notify_locked();
        } else if locked {
            locklog::note("the lock screen is back");
        }
        let status = child.wait();
        let code = status.as_ref().ok().and_then(|s| s.code());
        match code {
            Some(0) => {
                locklog::note("unlocked");
                return ExitCode::SUCCESS;
            }
            Some(c) if c == i32::from(LOCK_FAILED) || c == 2 => {
                locklog::note(&format!(
                    "the lock screen could not lock again ({status:?}); the compositor keeps the screen locked"
                ));
                return ExitCode::FAILURE;
            }
            _ => {}
        }
        locklog::note(&format!(
            "the lock screen stopped while locked ({status:?}); starting it again"
        ));
        let now = std::time::Instant::now();
        restarts.retain(|t| now.duration_since(*t) < std::time::Duration::from_secs(60));
        restarts.push(now);
        // Keep trying, but not in a tight loop if it keeps failing.
        let pause = if restarts.len() > 5 { 5000 } else { 300 };
        std::thread::sleep(std::time::Duration::from_millis(pause));
    }
}

/// Tell `lock_and_detach` the screen is locked, then let go of the pipe.
fn notify_locked() {
    let mut out = std::io::stdout();
    let _ = out.write_all(b"locked\n");
    let _ = out.flush();
    if let Ok(null) = std::fs::OpenOptions::new().write(true).open("/dev/null") {
        unsafe {
            libc::dup2(null.as_raw_fd(), libc::STDOUT_FILENO);
        }
    }
}

fn describe(monitor: &gdk::Monitor) -> String {
    let g = monitor.geometry();
    format!(
        "{} {}x{}+{}+{}",
        monitor.connector().unwrap_or_default(),
        g.width(),
        g.height(),
        g.x(),
        g.y()
    )
}

/// The lock's windows, one per output, and the check that the password
/// field is on one of them.
///
/// gtk4-session-lock unmaps a window when its output goes away and asks
/// for a new one when an output appears. After a suspend the laptop's panel
/// can do both, and the new window used to get a view without the password
/// field, which stayed in the old, unmapped window.
struct LockWindows {
    app: gtk::Application,
    screen: Rc<Screen>,
    windows: std::cell::RefCell<Vec<(gdk::Monitor, gtk::Window)>>,
    locked: std::cell::Cell<bool>,
    /// Checks in a row that did not find the password field on screen.
    misses: std::cell::Cell<u32>,
}

impl LockWindows {
    fn prune(&self) {
        self.windows.borrow_mut().retain(|(monitor, _)| {
            let valid = monitor.is_valid();
            if !valid {
                locklog::note(&format!("output {} is gone", describe(monitor)));
            }
            valid
        });
    }

    /// Whether the account is in one of the live lock windows.
    fn account_has_window(&self) -> bool {
        let Some(root) = self.screen.account_root() else {
            return false;
        };
        let root = root.upcast::<gtk::Widget>();
        self.windows
            .borrow()
            .iter()
            .any(|(_, window)| window.upcast_ref::<gtk::Widget>() == &root)
    }

    fn add(&self, instance: &gtk4_session_lock::Instance, monitor: &gdk::Monitor) {
        locklog::note(&format!("output {} added", describe(monitor)));
        self.prune();
        let window = gtk::Window::builder()
            .application(&self.app)
            .title("Elavo Lock Screen")
            .css_classes(["elavo-greeter-window"])
            .build();
        trace_first_frame(&window);
        let rehome = !self.account_has_window();
        let others = !self.windows.borrow().is_empty();
        self.windows
            .borrow_mut()
            .push((monitor.clone(), window.clone()));
        if rehome && (others || self.locked.get()) {
            self.rebuild("the password field had no window");
        } else if rehome {
            self.rebuild("");
        } else {
            window.set_child(Some(&self.screen.view(Some(monitor))));
        }
        trace::mark("lock window built");
        instance.assign_window_to_monitor(&window, monitor);
    }

    /// Build every live window's view again. The first takes the account.
    /// The session stays locked throughout.
    fn rebuild(&self, reason: &str) {
        self.prune();
        let windows = self.windows.borrow().clone();
        if windows.is_empty() {
            locklog::note("no output to show the lock on; waiting for one");
            return;
        }
        if !reason.is_empty() {
            locklog::note(&format!(
                "rebuilding the lock on {} output(s): {reason}",
                windows.len()
            ));
        }
        self.screen.release_views();
        for (monitor, window) in &windows {
            window.set_child(Some(&self.screen.view(Some(monitor))));
        }
        self.misses.set(0);
    }

    fn check(&self) {
        if !self.locked.get() {
            return;
        }
        self.prune();
        if self.windows.borrow().is_empty() {
            // Nothing to show it on until an output comes back.
            return;
        }
        if self.screen.account_on_screen() {
            if self.misses.replace(0) > 0 {
                locklog::note("the password field is on screen");
            }
            if let Some(root) = self.screen.account_root() {
                if root.focus().is_none() {
                    self.screen.focus_account();
                }
            }
            return;
        }
        let misses = self.misses.get() + 1;
        self.misses.set(misses);
        locklog::note("the password field is not on screen");
        if misses >= LOCK_CHECK_MISSES {
            self.rebuild("the password field stayed off screen");
        }
    }
}

fn run_lock(app: &gtk::Application, backend: Backend) {
    if !gtk4_session_lock::is_supported() {
        locklog::note("the compositor does not support ext-session-lock-v1");
        std::process::exit(i32::from(LOCK_FAILED));
    }
    locklog::note(&format!(
        "starting the lock (GSK_RENDERER={})",
        std::env::var("GSK_RENDERER").unwrap_or_default()
    ));
    let screen = Screen::new(Kind::Lock, backend, Show::List, 0);
    trace::mark("screen built");
    screen.preload(first_monitor_aspect());
    warm_up_renderer();
    let instance = gtk4_session_lock::Instance::new();
    // Real locks tell logind. A test lock in a nested session does not
    // touch the real session's state.
    let hint = backend != Backend::Demo;

    let windows = Rc::new(LockWindows {
        app: app.clone(),
        screen: screen.clone(),
        windows: Default::default(),
        locked: Default::default(),
        misses: Default::default(),
    });
    let for_monitors = windows.clone();
    instance.connect_monitor(move |instance, monitor| {
        for_monitors.add(instance, monitor);
    });
    if let Some(display) = gdk::Display::default() {
        display
            .monitors()
            .connect_items_changed(|model, _, removed, added| {
                locklog::note(&format!(
                    "outputs changed: {removed} removed, {added} added, {} now",
                    model.n_items()
                ));
            });
    }
    let for_locked = windows.clone();
    instance.connect_locked(move |_| {
        trace::mark("locked");
        locklog::note("locked");
        for_locked.locked.set(true);
        notify_locked();
        if hint {
            glib::spawn_future_local(logind::set_locked_hint(true));
        }
    });
    instance.connect_failed(|_| {
        locklog::note("the screen could not be locked");
        std::process::exit(i32::from(LOCK_FAILED));
    });
    instance.connect_unlocked(move |_| {
        locklog::note("unlocking");
        glib::spawn_future_local(async move {
            if hint {
                logind::set_locked_hint(false).await;
            }
            // Exiting skips finalizing the lock windows; see the GTK 4.22.5
            // note in AGENTS.md.
            std::process::exit(0);
        });
    });
    let weak = instance.downgrade();
    screen.connect_unlock(move || {
        if let Some(instance) = weak.upgrade() {
            instance.unlock();
        }
    });
    let for_check = windows.clone();
    glib::timeout_add_local(LOCK_CHECK, move || {
        for_check.check();
        glib::ControlFlow::Continue
    });
    let for_sleep = windows.clone();
    logind::watch_sleep(move |going| {
        if going {
            locklog::note("going to sleep");
        } else {
            locklog::note("back from sleep");
            // Check soon rather than on the next tick.
            let windows = for_sleep.clone();
            glib::timeout_add_local_once(std::time::Duration::from_millis(1500), move || {
                windows.check();
            });
        }
    });
    // Lock before the wallpaper finishes. before-sleep waits on this, and a
    // wait here lets the machine suspend with the session still showing.
    // The picture is already decoding and is applied when it arrives.
    trace::mark("requesting lock");
    locklog::note("requesting lock");
    instance.lock();
    std::mem::forget(instance);
}

fn monitor_list(display: &gdk::Display) -> Vec<gdk::Monitor> {
    let model = display.monitors();
    (0..model.n_items())
        .filter_map(|i| model.item(i).and_downcast::<gdk::Monitor>())
        .collect()
}

fn monitor_rects(monitors: &[gdk::Monitor]) -> Vec<screen::Rect> {
    monitors
        .iter()
        .map(|monitor| {
            let geometry = monitor.geometry();
            (
                geometry.x(),
                geometry.y(),
                geometry.width(),
                geometry.height(),
            )
        })
        .collect()
}

/// Put `view` in the rectangle `(x, y, w, h)` of a window `window_w` by `window_h`.
fn place_view(
    board: &gtk::Overlay,
    view: &gtk::Widget,
    window_w: i32,
    window_h: i32,
    place: screen::Rect,
) {
    let (x, y, w, h) = place;
    view.set_hexpand(true);
    view.set_vexpand(true);
    view.set_halign(gtk::Align::Fill);
    view.set_valign(gtk::Align::Fill);
    view.set_margin_start(x);
    view.set_margin_end((window_w - x - w).max(0));
    view.set_margin_top(y);
    view.set_margin_bottom((window_h - y - h).max(0));
    board.add_overlay(view);
}

fn clear_views(board: &gtk::Overlay) {
    let mut child = board.first_child();
    let mut doomed = Vec::new();
    while let Some(widget) = child {
        child = widget.next_sibling();
        doomed.push(widget);
    }
    for widget in doomed {
        board.remove_overlay(&widget);
    }
}

/// One view per output inside the single login window. The account stays on
/// the first. A window that cannot hold every output yet keeps its layout.
fn place_views(screen: &Rc<Screen>, board: &gtk::Overlay, display: &gdk::Display) {
    let width = board.width();
    let height = board.height();
    let monitors = monitor_list(display);
    let places = screen::view_places(width, height, &monitor_rects(&monitors));
    if places.is_empty() {
        return;
    }
    screen.release_views();
    clear_views(board);
    if places.len() == monitors.len() {
        for (monitor, place) in monitors.iter().zip(places) {
            place_view(board, &screen.view(Some(monitor)), width, height, place);
        }
    } else {
        let place = places[0];
        place_view(board, &screen.view(monitors.first()), width, height, place);
    }
}

fn watch_output(monitor: &gdk::Monitor, generation: &Rc<std::cell::Cell<u64>>) {
    if unsafe { monitor.data::<bool>("elavo-greeter-output") }.is_some() {
        return;
    }
    unsafe { monitor.set_data("elavo-greeter-output", true) };
    let generation = generation.clone();
    monitor.connect_notify_local(Some("geometry"), move |_, _| {
        generation.set(generation.get().wrapping_add(1));
    });
}

fn watch_outputs(screen: &Rc<Screen>, board: &gtk::Overlay, display: &gdk::Display) {
    let generation = Rc::new(std::cell::Cell::new(0u64));
    let seen = Rc::new(std::cell::Cell::new((-1i32, -1i32, u64::MAX)));
    for monitor in monitor_list(display) {
        watch_output(&monitor, &generation);
    }
    let generation_for_items = generation.clone();
    display
        .monitors()
        .connect_items_changed(move |model, position, _removed, added| {
            generation_for_items.set(generation_for_items.get().wrapping_add(1));
            for index in position..position + added {
                if let Some(monitor) = model.item(index).and_downcast::<gdk::Monitor>() {
                    watch_output(&monitor, &generation_for_items);
                }
            }
        });
    let screen = screen.clone();
    let display = display.clone();
    board.add_tick_callback(move |board, _| {
        let stamp = (board.width(), board.height(), generation.get());
        if stamp != seen.get() {
            seen.set(stamp);
            place_views(&screen, board, &display);
        }
        glib::ControlFlow::Continue
    });
}

fn run_window(
    app: &gtk::Application,
    kind: Kind,
    backend: Backend,
    show: Show,
    demo_people: usize,
) {
    if backend == Backend::Greetd {
        scale::apply_defaults();
    }
    let screen = Screen::new(kind, backend, show, demo_people);
    trace::mark("screen built");
    screen.preload(first_monitor_aspect());
    warm_up_renderer();
    let display = gdk::Display::default().expect("no display");
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title(match (kind, backend, demo_people) {
            (Kind::Lock, Backend::Demo, _) => "Elavo Lock",
            (Kind::Login, Backend::Demo, 0) => "Elavo Login",
            (Kind::Login, Backend::Demo, _) => "Elavo Login, several people",
            _ => "Elavo Login",
        })
        .decorated(backend == Backend::Demo)
        .default_width(1280)
        .default_height(800)
        .css_classes(["elavo-greeter-window"])
        .build();
    if backend == Backend::Demo {
        let monitor = display.monitors().item(0).and_downcast::<gdk::Monitor>();
        window.set_child(Some(&screen.view(monitor.as_ref())));
    } else {
        // Cage gives this one window the bounding box of every output.
        // Each output gets a view inside it. A second toplevel would be
        // stretched the same way, and destroying one crashes GTK 4.22.5.
        let board = gtk::Overlay::new();
        board.set_hexpand(true);
        board.set_vexpand(true);
        window.set_child(Some(&board));
        place_views(&screen, &board, &display);
        watch_outputs(&screen, &board, &display);
    }
    if backend != Backend::Demo {
        window.fullscreen();
    }
    // The window and the screen's state stay for the life of the process.
    after_wallpaper(&screen, move || {
        trace::mark("presenting window");
        window.present();
        std::mem::forget(window);
    });
    std::mem::forget::<Rc<Screen>>(screen);
}

fn main() -> ExitCode {
    trace::init();
    trace::mark("process started");
    // GTK's Vulkan renderer takes 0.6 to 1.4 s to set up on the first
    // window here, and GL about 0.1 s. Both the lock and the login screen
    // are one window that has to appear at once. Set before GTK starts.
    if std::env::var_os("GSK_RENDERER").is_none() {
        std::env::set_var("GSK_RENDERER", "gl");
    }
    let args = match parse_args() {
        Ok(args) => args,
        Err(e) => {
            eprintln!("elavo-greeter: {e}");
            return ExitCode::from(2);
        }
    };
    if args.locks_session() {
        if args.keep {
            return keep_lock(&args);
        }
        if !args.foreground {
            return lock_and_detach(&args);
        }
        locklog::open();
    }

    gio::resources_register_include!("elavo-greeter.gresource").expect("greeter resources");

    // A test lock does not take the real lock's name on the session bus.
    let app = if args.lock && !args.demo {
        gtk::Application::builder()
            .application_id(LOCK_APP_ID)
            .build()
    } else {
        // greetd's user has no session bus, so the login window does not
        // register a name on one.
        gtk::Application::builder()
            .flags(gio::ApplicationFlags::NON_UNIQUE)
            .build()
    };

    let started = std::cell::Cell::new(false);
    app.connect_activate(move |app| {
        // A second `--lock` while locked lands here and does nothing.
        if started.replace(true) {
            return;
        }
        std::mem::forget(app.hold());
        trace::mark("activated");
        load_style();
        let show = args.show.unwrap_or_else(config::load);
        match (args.lock, args.demo) {
            (true, false) => run_lock(app, Backend::Pam),
            (true, true) if args.session_lock => run_lock(app, Backend::Demo),
            (true, true) => run_window(app, Kind::Lock, Backend::Demo, show, 0),
            (false, true) => run_window(app, Kind::Login, Backend::Demo, show, args.demo_people),
            (false, false) => run_window(app, Kind::Login, Backend::Greetd, show, 0),
        }
    });
    let empty: [&str; 0] = [];
    match app.run_with_args(&empty) {
        code if code == glib::ExitCode::SUCCESS => ExitCode::SUCCESS,
        _ => ExitCode::FAILURE,
    }
}
