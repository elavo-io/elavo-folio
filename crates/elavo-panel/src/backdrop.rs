//! Keeps each bar's ink and gradient in step with the wallpaper. The ink is
//! a class on the bar, the gradient a rule for the bar's shade.
//!
//! `elavo-wallpaper` writes the path of the picture on screen to
//! `$XDG_RUNTIME_DIR/elavo/wallpaper`, or an empty file when none is shown.
//! The bar watches that file and measures the picture again for each output
//! when it changes, off the main thread.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use crate::contrast::{self, Ink, Look};

const STATE: &str = "wallpaper";

pub struct Bar {
    /// Takes the ink.
    pub widget: gtk::CenterBox,
    /// Takes the gradient. It lives on its own surface below windows.
    pub shade: gtk::Box,
    pub monitor: gdk::Monitor,
}

pub struct Backdrop {
    provider: gtk::CssProvider,
    rules: RefCell<Vec<String>>,
    /// Bumped on every refresh, so a slow measurement for an older picture
    /// or an older set of bars is dropped.
    generation: Cell<u64>,
    pending: Cell<bool>,
    monitor: RefCell<Option<gio::FileMonitor>>,
    reported: RefCell<HashSet<String>>,
}

fn state_dir() -> PathBuf {
    glib::user_runtime_dir().join("elavo")
}

fn current_picture() -> Option<PathBuf> {
    let text = std::fs::read_to_string(state_dir().join(STATE)).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| PathBuf::from(text))
}

impl Backdrop {
    /// `changed` runs when the wallpaper changes. The panel answers by
    /// calling [`Backdrop::refresh`] with its current bars.
    pub fn new(display: &gdk::Display, changed: impl Fn() + 'static) -> Rc<Self> {
        let provider = gtk::CssProvider::new();
        gtk::style_context_add_provider_for_display(
            display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
        );
        let this = Rc::new(Self {
            provider,
            rules: RefCell::default(),
            generation: Cell::new(0),
            pending: Cell::new(false),
            monitor: RefCell::default(),
            reported: RefCell::default(),
        });
        this.watch(Rc::new(changed));
        this
    }

    fn watch(self: &Rc<Self>, changed: Rc<dyn Fn()>) {
        let dir = state_dir();
        let _ = std::fs::create_dir_all(&dir);
        let monitor = match gio::File::for_path(&dir)
            .monitor_directory(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE)
        {
            Ok(m) => m,
            Err(err) => {
                glib::g_warning!("elavo-panel", "cannot watch {}: {err}", dir.display());
                return;
            }
        };
        let weak: Weak<Self> = Rc::downgrade(self);
        monitor.connect_changed(move |_, file, other, _| {
            let named = |f: Option<&gio::File>| {
                f.and_then(|f| f.basename())
                    .is_some_and(|name| name.as_os_str() == STATE)
            };
            if !(named(Some(file)) || named(other)) {
                return;
            }
            let Some(this) = weak.upgrade() else {
                return;
            };
            // A rename arrives as several events. Measure once.
            if this.pending.replace(true) {
                return;
            }
            let weak = weak.clone();
            let changed = changed.clone();
            glib::timeout_add_local_once(Duration::from_millis(150), move || {
                if let Some(this) = weak.upgrade() {
                    this.pending.set(false);
                    changed();
                }
            });
        });
        self.monitor.replace(Some(monitor));
    }

    /// Measure the wallpaper again for every bar.
    pub fn refresh(self: &Rc<Self>, bars: &[Bar]) {
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        self.rules.replace(vec![String::new(); bars.len()]);
        let picture = current_picture();
        for (index, bar) in bars.iter().enumerate() {
            bar.shade
                .add_css_class(&format!("elavo-panel-shade-{index}"));
            let Some(path) = picture.clone() else {
                self.apply(index, &bar.widget, Look::DARK);
                continue;
            };
            let geometry = bar.monitor.geometry();
            let (width, height) = (geometry.width(), geometry.height());
            let weak = Rc::downgrade(self);
            let widget = bar.widget.clone();
            glib::spawn_future_local(async move {
                let sample_path = path.clone();
                let result = gio::spawn_blocking(move || {
                    contrast::sample(&sample_path, width, height, crate::HEIGHT)
                })
                .await;
                let Some(this) = weak.upgrade() else {
                    return;
                };
                if this.generation.get() != generation {
                    return;
                }
                let look = match result {
                    Ok(Ok(strip)) => contrast::choose(strip),
                    Ok(Err(err)) => {
                        this.report(&path, &format!("cannot read {}: {err}", path.display()));
                        Look::DARK
                    }
                    Err(_) => Look::DARK,
                };
                if !look.passes() {
                    this.report(
                        &path,
                        &format!(
                            "{} reaches only {:.2}:1 under the panel, below WCAG AA",
                            path.display(),
                            look.contrast
                        ),
                    );
                }
                this.apply(index, &widget, look);
            });
        }
    }

    fn report(&self, path: &std::path::Path, message: &str) {
        let key = path.display().to_string();
        if self.reported.borrow_mut().insert(key) {
            glib::g_warning!("elavo-panel", "{message}");
        }
    }

    fn apply(&self, index: usize, widget: &gtk::CenterBox, look: Look) {
        let (on, off, rgb) = match look.ink {
            Ink::White => ("on-dark", "on-light", "0, 0, 0"),
            Ink::Black => ("on-light", "on-dark", "255, 255, 255"),
        };
        widget.remove_css_class(off);
        widget.add_css_class(on);
        let stops = contrast::STOPS
            .iter()
            .map(|(pos, share)| {
                format!("rgba({rgb}, {:.3}) {:.0}%", look.tint * share, pos * 100.0)
            })
            .collect::<Vec<_>>()
            .join(", ");
        let rule = format!(
            ".elavo-panel-shade-{index} {{ background-image: linear-gradient(to bottom, {stops}); }}\n"
        );
        let css = {
            let mut rules = self.rules.borrow_mut();
            if let Some(slot) = rules.get_mut(index) {
                *slot = rule;
            }
            rules.concat()
        };
        self.provider.load_from_string(&css);
    }
}
