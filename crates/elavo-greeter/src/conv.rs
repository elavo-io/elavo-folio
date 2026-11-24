//! One sign-in or unlock can take more than a password: a second factor,
//! or a new password when the old one has expired. The check runs on a
//! worker thread. It reports PAM's messages and asks for each further
//! answer through [`Talk`]; the screen shows them and sends the answers
//! back.

use std::sync::mpsc;

/// Why a sign-in or an unlock did not go through.
#[derive(Debug, PartialEq, Eq)]
pub enum Failure {
    /// PAM does not say whether the password was wrong or the account is
    /// locked or disabled, so these share one message.
    Auth,
    /// The password was right, but this account may not sign in here.
    Denied,
    /// The password has expired and has to be changed first.
    Expired,
    /// The person backed out of a prompt.
    Cancelled,
    Other(String),
}

pub const WRONG: &str = "Wrong password, or the account is locked or disabled.";
pub const DENIED: &str = "This account can't sign in on this computer.";
pub const EXPIRED: &str = "Your password has expired.";
pub const UNREACHABLE: &str =
    "The account server can't be reached, and this account hasn't signed in here before.";

impl Failure {
    /// The line under the password field, if any.
    pub fn text(&self) -> Option<String> {
        match self {
            Failure::Auth => Some(WRONG.to_owned()),
            Failure::Denied => Some(DENIED.to_owned()),
            Failure::Expired => Some(EXPIRED.to_owned()),
            Failure::Cancelled => None,
            Failure::Other(text) if text.trim().is_empty() => Some("Could not sign in.".to_owned()),
            Failure::Other(text) => Some(text.trim().to_owned()),
        }
    }
}

/// The worker's side of the conversation.
pub trait Talk {
    /// Something PAM wants shown, such as "Password expired".
    fn info(&mut self, text: &str);
    /// A problem PAM reports, such as "Passwords do not match".
    fn error(&mut self, text: &str);
    /// A hidden answer. `None` when the person backed out.
    fn secret(&mut self, prompt: &str) -> Option<String>;
    /// A visible answer, such as a user name or a code.
    fn visible(&mut self, prompt: &str) -> Option<String>;
}

/// What the worker sends the screen.
#[derive(Debug, PartialEq, Eq)]
pub enum Event {
    Info(String),
    Error(String),
    /// Ask for a hidden answer with this label.
    Secret(String),
    /// Ask for a visible answer with this label.
    Visible(String),
    Done(Result<(), Failure>),
}

/// [`Talk`] for a worker thread: events go to the screen's main loop, and
/// each prompt waits for the screen's answer. A closed channel counts as
/// backing out.
pub struct Worker {
    pub events: async_channel::Sender<Event>,
    pub answers: mpsc::Receiver<Option<String>>,
}

impl Worker {
    fn ask(&mut self, event: Event) -> Option<String> {
        self.events.send_blocking(event).ok()?;
        self.answers.recv().ok().flatten()
    }
}

impl Talk for Worker {
    fn info(&mut self, text: &str) {
        let _ = self
            .events
            .send_blocking(Event::Info(text.trim().to_owned()));
    }

    fn error(&mut self, text: &str) {
        let _ = self
            .events
            .send_blocking(Event::Error(text.trim().to_owned()));
    }

    fn secret(&mut self, prompt: &str) -> Option<String> {
        self.ask(Event::Secret(label(prompt)))
    }

    fn visible(&mut self, prompt: &str) -> Option<String> {
        self.ask(Event::Visible(label(prompt)))
    }
}

/// A PAM prompt as a field's placeholder: "New password: " to
/// "New password".
pub fn label(prompt: &str) -> String {
    let text = prompt.trim().trim_end_matches(':').trim_end();
    if text.is_empty() {
        "Password".to_owned()
    } else {
        text.to_owned()
    }
}

/// Whether a prompt asks for the password in use, during a change. The
/// person typed it moments ago, so it is not asked again.
pub fn asks_current(prompt: &str) -> bool {
    let text = prompt.to_lowercase();
    text.contains("current") || text.trim_start().starts_with("old ")
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// A [`Talk`] with answers given up front. It records what it was
    /// shown and asked.
    #[derive(Default)]
    pub struct Script {
        pub answers: VecDeque<String>,
        pub seen: Vec<String>,
    }

    impl Script {
        pub fn new(answers: &[&str]) -> Self {
            Self {
                answers: answers.iter().map(|a| (*a).to_owned()).collect(),
                seen: Vec::new(),
            }
        }
    }

    impl Talk for Script {
        fn info(&mut self, text: &str) {
            self.seen.push(format!("info: {}", text.trim()));
        }
        fn error(&mut self, text: &str) {
            self.seen.push(format!("error: {}", text.trim()));
        }
        fn secret(&mut self, prompt: &str) -> Option<String> {
            self.seen.push(format!("secret: {}", label(prompt)));
            self.answers.pop_front()
        }
        fn visible(&mut self, prompt: &str) -> Option<String> {
            self.seen.push(format!("visible: {}", label(prompt)));
            self.answers.pop_front()
        }
    }

    #[test]
    fn prompts_become_labels() {
        assert_eq!(label("New password: "), "New password");
        assert_eq!(label("Second Factor: "), "Second Factor");
        assert_eq!(label(" : "), "Password");
    }

    #[test]
    fn the_current_password_prompt_is_recognized() {
        assert!(asks_current("Current Password: "));
        assert!(asks_current("(current) UNIX password: "));
        assert!(asks_current("Old password: "));
        assert!(!asks_current("New password: "));
        assert!(!asks_current("Retype new password: "));
    }

    #[test]
    fn failures_have_plain_messages() {
        assert_eq!(Failure::Auth.text().as_deref(), Some(WRONG));
        assert_eq!(Failure::Denied.text().as_deref(), Some(DENIED));
        assert_eq!(Failure::Cancelled.text(), None);
        assert_eq!(
            Failure::Other(String::new()).text().as_deref(),
            Some("Could not sign in.")
        );
    }

    #[test]
    fn a_worker_waits_for_each_answer() {
        let (events, inbox) = async_channel::unbounded();
        let (reply, answers) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            let mut worker = Worker { events, answers };
            worker.info("Password expired. ");
            let first = worker.secret("New password: ");
            let second = worker.secret("Retype new password: ");
            (first, second)
        });
        assert_eq!(
            inbox.recv_blocking(),
            Ok(Event::Info("Password expired.".into()))
        );
        assert_eq!(
            inbox.recv_blocking(),
            Ok(Event::Secret("New password".into()))
        );
        reply.send(Some("n3w".into())).unwrap();
        assert_eq!(
            inbox.recv_blocking(),
            Ok(Event::Secret("Retype new password".into()))
        );
        drop(reply);
        assert_eq!(thread.join().unwrap(), (Some("n3w".into()), None));
    }
}
