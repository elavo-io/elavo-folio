//! `org.freedesktop.PolicyKit1.AuthenticationAgent` on the system bus,
//! registered with the authority for the person's graphical session.
//! polkitd calls BeginAuthentication, which is answered only once the
//! helper has succeeded (polkitd then has its answer from the helper) or
//! the person cancels (`Error.Cancelled`). One card shows at a time; later
//! requests wait their turn.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::rc::{Rc, Weak};

use gtk::prelude::*;
use gtk::{gio, glib};

use crate::dialog::{self, Dialog};
use crate::helper::{Event, Helper};
use crate::identity::{self, Person};
use crate::{caller, wording};

const AUTHORITY_NAME: &str = "org.freedesktop.PolicyKit1";
const AUTHORITY_PATH: &str = "/org/freedesktop/PolicyKit1/Authority";
const AUTHORITY: &str = "org.freedesktop.PolicyKit1.Authority";
const AGENT_PATH: &str = "/org/freedesktop/PolicyKit1/AuthenticationAgent";
const AGENT: &str = "org.freedesktop.PolicyKit1.AuthenticationAgent";
const CANCELLED: &str = "org.freedesktop.PolicyKit1.Error.Cancelled";
const FAILED: &str = "org.freedesktop.PolicyKit1.Error.Failed";
const ACCESS_DENIED: &str = "org.freedesktop.DBus.Error.AccessDenied";
const LOGIN1: &str = "org.freedesktop.login1";
const WRONG_PASSWORD: &str = "Incorrect password. Try again.";
const NOT_ALLOWED: &str = "That account can't approve this.";
const LOG: &str = "elavo-polkit-agent";
const XML: &str = r#"<node>
  <interface name="org.freedesktop.PolicyKit1.AuthenticationAgent">
    <method name="BeginAuthentication">
      <arg type="s" name="action_id" direction="in"/>
      <arg type="s" name="message" direction="in"/>
      <arg type="s" name="icon_name" direction="in"/>
      <arg type="a{ss}" name="details" direction="in"/>
      <arg type="s" name="cookie" direction="in"/>
      <arg type="a(sa{sv})" name="identities" direction="in"/>
    </method>
    <method name="CancelAuthentication">
      <arg type="s" name="cookie" direction="in"/>
    </method>
  </interface>
</node>"#;

struct Request {
    cookie: String,
    spec: dialog::Spec,
    start: usize,
    people: Vec<Person>,
    invocation: gio::DBusMethodInvocation,
}

struct Agent {
    app: gtk::Application,
    /// polkitd's unique name. Only it may call the agent.
    authority: RefCell<String>,
    queue: RefCell<VecDeque<Request>>,
    active: RefCell<Option<Rc<Auth>>>,
}

pub fn start(app: &gtk::Application) {
    let app = app.clone();
    glib::spawn_future_local(async move {
        if let Err(err) = serve(app).await {
            glib::g_critical!(LOG, "cannot serve as the polkit agent: {err}");
            std::process::exit(1);
        }
    });
}

async fn serve(app: gtk::Application) -> Result<(), glib::Error> {
    let bus = gio::bus_get_future(gio::BusType::System).await?;
    let info = gio::DBusNodeInfo::for_xml(XML)?
        .lookup_interface(AGENT)
        .ok_or_else(|| glib::Error::new(gio::IOErrorEnum::Failed, "agent interface"))?;
    let agent = Rc::new(Agent {
        app,
        authority: RefCell::default(),
        queue: RefCell::default(),
        active: RefCell::default(),
    });
    let weak = Rc::downgrade(&agent);
    bus.register_object(AGENT_PATH, &info)
        .method_call(move |_, sender, _, _, method, params, invocation| {
            if let Some(agent) = weak.upgrade() {
                agent.call(sender, method, &params, invocation);
            }
        })
        .build()?;

    let session = session_id(&bus).await?;
    // Register whenever polkitd appears, so a restarted polkitd still has
    // an agent for this session.
    let weak = Rc::downgrade(&agent);
    // The watch lasts as long as the connection.
    let _ = gio::bus_watch_name_on_connection(
        &bus,
        AUTHORITY_NAME,
        gio::BusNameWatcherFlags::AUTO_START,
        move |bus, _, owner| {
            let Some(agent) = weak.upgrade() else {
                return;
            };
            agent.authority.replace(owner.to_owned());
            let session = session.clone();
            glib::spawn_future_local(async move {
                match register(&bus, &session).await {
                    Ok(()) => glib::g_message!(LOG, "registered for session {session}"),
                    Err(err) => {
                        glib::g_critical!(LOG, "cannot register for session {session}: {err}");
                        std::process::exit(1);
                    }
                }
            });
        },
        |_, _| {},
    );
    // The agent lives as long as the process.
    std::mem::forget(agent);
    Ok(())
}

async fn register(bus: &gio::DBusConnection, session: &str) -> Result<(), glib::Error> {
    let subject = (
        "unix-session".to_owned(),
        HashMap::from([("session-id".to_owned(), session.to_variant())]),
    );
    // The authority takes the agent's object path as a string.
    bus.call_future(
        Some(AUTHORITY_NAME),
        AUTHORITY_PATH,
        AUTHORITY,
        "RegisterAuthenticationAgent",
        Some(&(subject, locale(), AGENT_PATH).to_variant()),
        None,
        gio::DBusCallFlags::NONE,
        -1,
    )
    .await
    .map(|_| ())
}

fn locale() -> String {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|key| std::env::var(key).ok())
        .find(|value| !value.is_empty())
        .unwrap_or_else(|| "C".to_owned())
}

async fn property(
    bus: &gio::DBusConnection,
    path: &str,
    interface: &str,
    name: &str,
) -> Option<glib::Variant> {
    let reply = bus
        .call_future(
            Some(LOGIN1),
            path,
            "org.freedesktop.DBus.Properties",
            "Get",
            Some(&(interface, name).to_variant()),
            glib::VariantTy::new("(v)").ok(),
            gio::DBusCallFlags::NONE,
            -1,
        )
        .await
        .ok()?;
    reply.child_value(0).as_variant()
}

/// The logind session to register for: this process's own when it runs
/// inside one, else the person's graphical session (a user service runs
/// outside every session, which is how polkit itself resolves it), else
/// `XDG_SESSION_ID`.
async fn session_id(bus: &gio::DBusConnection) -> Result<String, glib::Error> {
    let own = bus
        .call_future(
            Some(LOGIN1),
            "/org/freedesktop/login1",
            "org.freedesktop.login1.Manager",
            "GetSessionByPID",
            Some(&(std::process::id(),).to_variant()),
            glib::VariantTy::new("(o)").ok(),
            gio::DBusCallFlags::NONE,
            -1,
        )
        .await
        .ok()
        .and_then(|reply| reply.child_value(0).str().map(str::to_owned));
    if let Some(path) = own {
        let id = property(bus, &path, "org.freedesktop.login1.Session", "Id").await;
        if let Some(id) = id.and_then(|v| v.str().map(str::to_owned)) {
            return Ok(id);
        }
    }
    let display = property(
        bus,
        "/org/freedesktop/login1/user/self",
        "org.freedesktop.login1.User",
        "Display",
    )
    .await
    .and_then(|v| v.child_value(0).str().map(str::to_owned))
    .filter(|id| !id.is_empty());
    if let Some(id) = display {
        return Ok(id);
    }
    std::env::var("XDG_SESSION_ID")
        .ok()
        .filter(|id| !id.is_empty())
        .ok_or_else(|| glib::Error::new(gio::IOErrorEnum::NotFound, "no logind session"))
}

impl Agent {
    fn call(
        self: &Rc<Self>,
        sender: Option<&str>,
        method: &str,
        params: &glib::Variant,
        invocation: gio::DBusMethodInvocation,
    ) {
        if sender != Some(self.authority.borrow().as_str()) {
            invocation.return_dbus_error(ACCESS_DENIED, "Only polkitd may call the agent");
            return;
        }
        match method {
            "BeginAuthentication" => self.begin(params, invocation),
            "CancelAuthentication" => {
                let cookie = params.child_value(0);
                self.cancel(cookie.str().unwrap_or(""));
                invocation.return_value(None);
            }
            _ => invocation.return_dbus_error(
                "org.freedesktop.DBus.Error.UnknownMethod",
                &format!("No method {method}"),
            ),
        }
    }

    fn begin(self: &Rc<Self>, params: &glib::Variant, invocation: gio::DBusMethodInvocation) {
        let text = |i| params.child_value(i).str().unwrap_or("").to_owned();
        let action_id = text(0);
        let message = text(1);
        let icon_name = text(2);
        let details: HashMap<String, String> = params.child_value(3).get().unwrap_or_default();
        let cookie = text(4);
        let (people, start) = identity::people(&params.child_value(5), unsafe { libc::getuid() });
        if people.is_empty() {
            invocation.return_dbus_error(FAILED, "No one in the request can be asked");
            return;
        }
        let caller = caller::find(&details);
        let spec = dialog::Spec {
            title: wording::title(&message, caller.as_ref().map(|c| c.name.as_str())),
            body: wording::BODY.to_owned(),
            badge: dialog::badge(&icon_name, caller.and_then(|c| c.icon)),
            name: start
                .map(|i| people[i].display_name().to_owned())
                .unwrap_or_default(),
            action_id,
        };
        self.queue.borrow_mut().push_back(Request {
            cookie,
            spec,
            start: start.unwrap_or(0),
            people,
            invocation,
        });
        self.next();
    }

    fn cancel(self: &Rc<Self>, cookie: &str) {
        let active = self.active.borrow().clone();
        if let Some(auth) = active.filter(|a| a.cookie == cookie) {
            auth.finish(false);
            return;
        }
        let mut queue = self.queue.borrow_mut();
        if let Some(i) = queue.iter().position(|r| r.cookie == cookie) {
            if let Some(request) = queue.remove(i) {
                request
                    .invocation
                    .return_dbus_error(CANCELLED, "The authentication was cancelled");
            }
        }
    }

    fn next(self: &Rc<Self>) {
        if self.active.borrow().is_some() {
            return;
        }
        let Some(request) = self.queue.borrow_mut().pop_front() else {
            return;
        };
        let auth = Auth::new(self, request);
        self.active.replace(Some(auth.clone()));
        auth.start_helper();
    }

    fn done(self: &Rc<Self>) {
        self.active.take();
        self.next();
    }
}

/// One request on screen and its helper.
struct Auth {
    agent: Weak<Agent>,
    cookie: String,
    people: Vec<Person>,
    person: Cell<usize>,
    dialog: Rc<Dialog>,
    helper: RefCell<Option<Rc<Helper>>>,
    /// An answer typed before the helper asked for it.
    pending: RefCell<Option<String>>,
    /// The helper is waiting on an answer.
    awaiting: Cell<bool>,
    /// The person answered since this helper started.
    submitted: Cell<bool>,
    invocation: RefCell<Option<gio::DBusMethodInvocation>>,
}

impl Auth {
    fn new(agent: &Rc<Agent>, request: Request) -> Rc<Self> {
        let dialog = Dialog::new(&agent.app, &request.spec);
        let auth = Rc::new(Self {
            agent: Rc::downgrade(agent),
            cookie: request.cookie,
            people: request.people,
            person: Cell::new(request.start),
            dialog,
            helper: RefCell::default(),
            pending: RefCell::default(),
            awaiting: Cell::new(false),
            submitted: Cell::new(false),
            invocation: RefCell::new(Some(request.invocation)),
        });
        let weak = Rc::downgrade(&auth);
        auth.dialog.connect_submit(move |(name, password)| {
            if let Some(auth) = weak.upgrade() {
                auth.submit(&name, password);
            }
        });
        let weak = Rc::downgrade(&auth);
        auth.dialog.connect_cancel(move |()| {
            if let Some(auth) = weak.upgrade() {
                auth.finish(false);
            }
        });
        auth
    }

    fn stop_helper(&self) {
        if let Some(helper) = self.helper.take() {
            helper.stop();
        }
        self.awaiting.set(false);
    }

    fn start_helper(self: &Rc<Self>) {
        self.stop_helper();
        self.submitted.set(false);
        let Some(person) = self.people.get(self.person.get()) else {
            return;
        };
        let weak = Rc::downgrade(self);
        let started = Helper::start(&person.user, &self.cookie, move |event| {
            if let Some(auth) = weak.upgrade() {
                auth.event(event);
            }
        });
        match started {
            Ok(helper) => {
                self.helper.replace(Some(helper));
            }
            Err(err) => {
                glib::g_warning!(LOG, "cannot start the polkit helper: {err}");
                self.dialog
                    .set_note(Some("The password can't be checked right now."), true);
            }
        }
    }

    fn submit(self: &Rc<Self>, name: &str, password: String) {
        // Only someone polkitd listed can answer; anyone else is turned
        // away here, before PAM is asked.
        let Some(i) = identity::find(&self.people, name) else {
            self.dialog.fail(NOT_ALLOWED);
            return;
        };
        if i != self.person.get() || self.helper.borrow().is_none() {
            self.person.set(i);
            self.pending.take();
            self.start_helper();
        }
        self.dialog.set_busy(true);
        self.dialog.set_note(None, false);
        self.submitted.set(true);
        if self.awaiting.get() {
            self.respond(password);
        } else {
            self.pending.replace(Some(password));
        }
    }

    fn respond(&self, answer: String) {
        self.awaiting.set(false);
        let helper = self.helper.borrow().clone();
        if let Some(helper) = helper {
            if let Err(err) = helper.respond(answer) {
                glib::g_warning!(LOG, "cannot answer the polkit helper: {err}");
            }
        }
    }

    fn event(self: &Rc<Self>, event: Event) {
        match event {
            Event::Prompt { text, .. } => {
                self.awaiting.set(true);
                let pending = self.pending.take();
                match pending {
                    Some(answer) => self.respond(answer),
                    None => {
                        self.dialog.prompt(&text);
                        self.dialog.set_busy(false);
                    }
                }
            }
            // Fingerprint: pam_fprintd speaks through PAM_TEXT_INFO ("Place
            // your finger on the reader") and its result through
            // PAM_ERROR_MSG. A fingerprint mode would switch the card to its
            // "Touch ID or enter your password" variant here.
            Event::Info(text) => self.dialog.set_note(Some(&text), false),
            Event::Error(text) => self.dialog.set_note(Some(&text), true),
            Event::Done(true) => self.finish(true),
            Event::Done(false) => {
                self.helper.take();
                self.awaiting.set(false);
                if self.submitted.get() {
                    self.pending.take();
                    self.dialog.fail(WRONG_PASSWORD);
                    self.start_helper();
                } else {
                    // The helper gave up before anyone answered; the next
                    // Unlock starts a new one.
                    self.dialog.set_busy(false);
                }
            }
        }
    }

    /// Success, or Cancel from the person or from polkitd.
    fn finish(self: &Rc<Self>, success: bool) {
        let Some(invocation) = self.invocation.take() else {
            return;
        };
        self.stop_helper();
        self.pending.take();
        self.dialog.close();
        if success {
            invocation.return_value(None);
        } else {
            invocation.return_dbus_error(CANCELLED, "The authentication was cancelled");
        }
        if let Some(agent) = self.agent.upgrade() {
            agent.done();
        }
    }
}
