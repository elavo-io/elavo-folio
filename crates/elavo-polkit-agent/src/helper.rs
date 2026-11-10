//! A conversation with polkit's setuid helper, `polkit-agent-helper-1`.
//! It takes the user name as its argument and the cookie on its first
//! line of input, runs PAM, and writes one line per PAM message:
//! `PAM_PROMPT_ECHO_OFF`, `PAM_PROMPT_ECHO_ON`, `PAM_ERROR_MSG`, or
//! `PAM_TEXT_INFO`, then `SUCCESS` or `FAILURE`. Each answer is one line
//! back. On success the helper tells polkitd itself.

use std::cell::Cell;
use std::ffi::OsStr;
use std::path::Path;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};

use crate::wording::unescape;

const PATHS: &[&str] = &[
    "/usr/lib/polkit-1/polkit-agent-helper-1",
    "/usr/libexec/polkit-agent-helper-1",
];

#[derive(Debug, PartialEq, Eq)]
pub enum Event {
    Prompt { text: String, echo: bool },
    Info(String),
    Error(String),
    Done(bool),
}

pub struct Helper {
    process: gio::Subprocess,
    stdin: gio::OutputStream,
    stopped: Rc<Cell<bool>>,
}

impl Helper {
    /// `event` runs from the main loop, never from inside `start`.
    pub fn start(
        user: &str,
        cookie: &str,
        event: impl Fn(Event) + 'static,
    ) -> Result<Rc<Self>, glib::Error> {
        let path = PATHS
            .iter()
            .copied()
            .find(|p| Path::new(p).exists())
            .unwrap_or(PATHS[0]);
        let process = gio::Subprocess::newv(
            &[OsStr::new(path), OsStr::new(user)],
            gio::SubprocessFlags::STDIN_PIPE | gio::SubprocessFlags::STDOUT_PIPE,
        )?;
        let missing = || glib::Error::new(gio::IOErrorEnum::Failed, "helper has no pipes");
        let stdin = process.stdin_pipe().ok_or_else(missing)?;
        let stdout = process.stdout_pipe().ok_or_else(missing)?;
        let helper = Rc::new(Self {
            process,
            stdin,
            stopped: Rc::default(),
        });
        helper.write(cookie.as_bytes().to_vec())?;

        let reader = gio::DataInputStream::new(&stdout);
        let stopped = helper.stopped.clone();
        glib::spawn_future_local(async move {
            loop {
                let line = reader.read_line_utf8_future(glib::Priority::DEFAULT).await;
                if stopped.get() {
                    return;
                }
                let Ok(Some(line)) = line else {
                    stopped.set(true);
                    event(Event::Done(false));
                    return;
                };
                let Some(parsed) = parse(&line) else {
                    continue;
                };
                let done = matches!(parsed, Event::Done(_));
                if done {
                    stopped.set(true);
                }
                event(parsed);
                if done {
                    return;
                }
            }
        });
        Ok(helper)
    }

    /// One answer to the last prompt. The copy written to the pipe is wiped.
    pub fn respond(&self, answer: String) -> Result<(), glib::Error> {
        self.write(answer.into_bytes())
    }

    fn write(&self, mut line: Vec<u8>) -> Result<(), glib::Error> {
        line.push(b'\n');
        let written = self
            .stdin
            .write_all(&line, gio::Cancellable::NONE)
            .map(|_| ())
            .and_then(|()| self.stdin.flush(gio::Cancellable::NONE));
        wipe(&mut line);
        written
    }

    pub fn stop(&self) {
        if !self.stopped.replace(true) {
            self.process.force_exit();
        }
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        self.stop();
    }
}

fn wipe(bytes: &mut [u8]) {
    for b in bytes.iter_mut() {
        unsafe { std::ptr::write_volatile(b, 0) };
    }
}

fn parse(line: &str) -> Option<Event> {
    let line = line.trim_end_matches(['\n', '\r']);
    let rest = |prefix: &str| {
        line.strip_prefix(prefix)
            .map(|r| unescape(r.strip_prefix(' ').unwrap_or(r)))
    };
    if line == "SUCCESS" {
        Some(Event::Done(true))
    } else if line == "FAILURE" {
        Some(Event::Done(false))
    } else if let Some(text) = rest("PAM_PROMPT_ECHO_OFF") {
        Some(Event::Prompt { text, echo: false })
    } else if let Some(text) = rest("PAM_PROMPT_ECHO_ON") {
        Some(Event::Prompt { text, echo: true })
    } else if let Some(text) = rest("PAM_ERROR_MSG") {
        Some(Event::Error(text))
    } else {
        rest("PAM_TEXT_INFO").map(Event::Info)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_helper_protocol() {
        assert_eq!(
            parse("PAM_PROMPT_ECHO_OFF Password: \n"),
            Some(Event::Prompt {
                text: "Password: ".into(),
                echo: false
            })
        );
        assert_eq!(
            parse("PAM_TEXT_INFO Place your finger on the reader\n"),
            Some(Event::Info("Place your finger on the reader".into()))
        );
        assert_eq!(parse("SUCCESS\n"), Some(Event::Done(true)));
        assert_eq!(parse("FAILURE"), Some(Event::Done(false)));
        assert_eq!(parse("noise"), None);
    }
}
