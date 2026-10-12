//! The Elavo top panel. Spec: elavo-architecture-and-planning-internal,
//! planning/desktop/top-panel.md.

mod clock;
mod menu;
mod status;

use std::cell::{OnceCell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

const APP_ID: &str = "io.elavo.Panel";
const HEIGHT: i32 = 28;

struct Panel {
    app: gtk::Application,
    windows: RefCell<Vec<gtk::ApplicationWindow>>,
    clock: Rc<clock::Clock>,
    status: Rc<status::Status>,
}

impl Panel {
    fn new(app: &gtk::Application) -> Rc<Self> {
        let display = gdk::Display::default().expect("no display");

        let css = gtk::CssProvider::new();
        css.load_from_resource("/io/elavo/Panel/style.css");
        gtk::style_context_add_provider_for_display(
            &display,
            &css,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        menu::install_actions(app);

        let panel = Rc::new(Self {
            app: app.clone(),
            windows: RefCell::default(),
            clock: clock::Clock::new(),
            status: status::Status::new(),
        });

        let weak = Rc::downgrade(&panel);
        display.monitors().connect_items_changed(move |_, _, _, _| {
            if let Some(panel) = weak.upgrade() {
                panel.rebuild();
            }
        });
        panel.rebuild();
        panel
    }

    fn rebuild(&self) {
        for window in self.windows.take() {
            window.destroy();
        }
        let display = gdk::Display::default().expect("no display");
        let monitors = display.monitors();
        let mut windows = Vec::new();
        for i in 0..monitors.n_items() {
            if let Some(monitor) = monitors.item(i).and_downcast::<gdk::Monitor>() {
                windows.push(self.bar(&monitor));
            }
        }
        self.windows.replace(windows);
    }

    fn bar(&self, monitor: &gdk::Monitor) -> gtk::ApplicationWindow {
        let window = gtk::ApplicationWindow::builder()
            .application(&self.app)
            .title("Elavo Panel")
            .decorated(false)
            .default_height(HEIGHT)
            .css_classes(["elavo-panel"])
            .build();
        window.init_layer_shell();
        window.set_namespace(Some("elavo-panel"));
        window.set_layer(Layer::Top);
        window.set_monitor(Some(monitor));
        for edge in [Edge::Top, Edge::Left, Edge::Right] {
            window.set_anchor(edge, true);
        }
        window.auto_exclusive_zone_enable();
        window.set_keyboard_mode(KeyboardMode::None);

        let bar = gtk::CenterBox::builder()
            .css_classes(["elavo-panel-bar"])
            .start_widget(&menu::button(&window))
            .center_widget(&self.clock.label())
            .end_widget(&self.status.widget())
            .build();
        window.set_child(Some(&bar));
        window.present();
        window
    }
}

fn main() -> glib::ExitCode {
    gio::resources_register_include!("elavo-panel.gresource").expect("panel resources");

    let app = gtk::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();

    let panel: Rc<OnceCell<Rc<Panel>>> = Rc::default();
    app.connect_command_line(move |app, _| {
        panel.get_or_init(|| {
            // The panel stays up even while every bar is being rebuilt.
            std::mem::forget(app.hold());
            Panel::new(app)
        });
        glib::ExitCode::SUCCESS
    });
    app.run()
}
