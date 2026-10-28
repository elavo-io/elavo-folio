//! The Elavo login screen and lock screen. Spec:
//! elavo-architecture-and-planning-internal, planning/desktop/greeter.md.
//!
//! ```text
//! elavo-greeter                 login, run by greetd inside cage
//! elavo-greeter --lock          lock this session; returns once locked
//! elavo-greeter --demo [--lock] [--show list|name] [--demo-people N]
//!                               the same screen in a window; the password is "elavo".
//!                               --demo-people adds up to 8 made-up people to the list
//! ```

mod accounts;
mod avatar;
mod config;
mod frost;
mod glass;
mod greetd;
mod logind;
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
    demo: bool,
    demo_people: usize,
    show: Option<Show>,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        lock: false,
        foreground: false,
        demo: false,
        demo_people: 0,
        show: None,
    };
    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--lock" => args.lock = true,
            "--foreground" => args.foreground = true,
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

/// Start the lock in a child and return once the compositor has locked the
/// screen, so `swayidle`'s `before-sleep` waits for the lock.
fn lock_and_detach() -> ExitCode {
    let Ok(exe) = std::env::current_exe() else {
        return ExitCode::FAILURE;
    };
    let mut child = match Command::new(exe)
        .args(["--lock", "--foreground"])
        .stdout(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            eprintln!("elavo-greeter: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut ready = [0u8; 1];
    let locked = child
        .stdout
        .take()
        .is_some_and(|mut out| matches!(out.read(&mut ready), Ok(1)));
    if locked {
        return ExitCode::SUCCESS;
    }
    match child.wait() {
        Ok(status) if status.success() => ExitCode::SUCCESS,
        _ => ExitCode::FAILURE,
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

fn run_lock(app: &gtk::Application, backend: Backend) {
    if !gtk4_session_lock::is_supported() {
        eprintln!("elavo-greeter: the compositor does not support ext-session-lock-v1");
        std::process::exit(1);
    }
    let screen = Screen::new(Kind::Lock, backend, Show::List, 0);
    trace::mark("screen built");
    screen.preload(first_monitor_aspect());
    warm_up_renderer();
    let instance = gtk4_session_lock::Instance::new();

    let app_for_windows = app.clone();
    let screen_for_windows = screen.clone();
    instance.connect_monitor(move |instance, monitor| {
        let window = gtk::Window::builder()
            .application(&app_for_windows)
            .title("Elavo Lock Screen")
            .css_classes(["elavo-greeter-window"])
            .build();
        window.set_child(Some(&screen_for_windows.view(Some(monitor))));
        trace::mark("lock window built");
        trace_first_frame(&window);
        instance.assign_window_to_monitor(&window, monitor);
    });
    instance.connect_locked(|_| {
        trace::mark("locked");
        notify_locked();
        glib::spawn_future_local(logind::set_locked_hint(true));
    });
    instance.connect_failed(|_| {
        eprintln!("elavo-greeter: the screen could not be locked");
        std::process::exit(1);
    });
    instance.connect_unlocked(|_| {
        glib::spawn_future_local(async {
            logind::set_locked_hint(false).await;
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
    // Lock before the wallpaper finishes. before-sleep waits on this, and a
    // wait here lets the machine suspend with the session still showing.
    // The picture is already decoding and is applied when it arrives.
    trace::mark("requesting lock");
    instance.lock();
    std::mem::forget(instance);
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
    let monitor = display.monitors().item(0).and_downcast::<gdk::Monitor>();
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
    window.set_child(Some(&screen.view(monitor.as_ref())));
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
    if args.lock && !args.demo && !args.foreground {
        return lock_and_detach();
    }

    gio::resources_register_include!("elavo-greeter.gresource").expect("greeter resources");

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
