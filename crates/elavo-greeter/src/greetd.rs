//! greetd's IPC: a native-endian u32 length, then a JSON object, each way,
//! on the socket in `$GREETD_SOCK`. The password goes through greetd's own
//! PAM stack, which also unlocks the login keyring. Prompts after the
//! first (a second factor) and PAM's messages go to the screen.
//!
//! greetd stops at an expired password instead of asking for a new one.
//! [`sign_in`] then changes it through PAM itself and signs in again.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;

use serde_json::{json, Value};

pub use crate::conv::Failure;
use crate::conv::Talk;
use crate::pam;

const SESSION_FILE: &str = "/usr/share/wayland-sessions/elavo.desktop";
const FALLBACK_SESSION: &str = "/usr/libexec/elavo-session";

fn send(stream: &mut UnixStream, message: &Value) -> Result<(), Failure> {
    let body = message.to_string();
    let len = u32::try_from(body.len()).map_err(|_| Failure::Other("message too long".into()))?;
    stream
        .write_all(&len.to_ne_bytes())
        .and_then(|()| stream.write_all(body.as_bytes()))
        .map_err(|e| Failure::Other(e.to_string()))
}

fn recv(stream: &mut UnixStream) -> Result<Value, Failure> {
    let mut len = [0u8; 4];
    stream
        .read_exact(&mut len)
        .map_err(|e| Failure::Other(e.to_string()))?;
    let mut body = vec![0u8; u32::from_ne_bytes(len) as usize];
    stream
        .read_exact(&mut body)
        .map_err(|e| Failure::Other(e.to_string()))?;
    serde_json::from_slice(&body).map_err(|e| Failure::Other(e.to_string()))
}

/// greetd's error, sorted by the PAM result its description names, such
/// as "pam_acct_mgmt: NEW_AUTHTOK_REQD".
fn failure(reply: &Value) -> Failure {
    let description = reply["description"].as_str().unwrap_or("");
    if description.contains("NEW_AUTHTOK_REQD") {
        Failure::Expired
    } else if description.contains("PERM_DENIED") || description.contains("ACCT_EXPIRED") {
        Failure::Denied
    } else if description.contains("AUTHINFO_UNAVAIL") {
        Failure::Other(crate::conv::UNREACHABLE.to_owned())
    } else if reply["error_type"] == "auth_error" {
        Failure::Auth
    } else {
        Failure::Other(description.to_owned())
    }
}

/// What greetd should run for the session, from the Elavo session file.
pub fn session_command() -> Vec<String> {
    std::fs::read_to_string(SESSION_FILE)
        .ok()
        .and_then(|text| exec_line(&text))
        .unwrap_or_else(|| vec![FALLBACK_SESSION.to_owned()])
}

fn exec_line(desktop: &str) -> Option<Vec<String>> {
    let exec = desktop
        .lines()
        .find_map(|line| line.trim().strip_prefix("Exec="))?;
    let argv: Vec<String> = exec
        .split_whitespace()
        .filter(|arg| !arg.starts_with('%'))
        .map(str::to_owned)
        .collect();
    (!argv.is_empty()).then_some(argv)
}

/// Sign `user` in, changing an expired password on the way, then ask
/// greetd to start the session. greetd starts it once the greeter exits.
pub fn sign_in(
    user: &str,
    password: &str,
    command: &[String],
    talk: &mut dyn Talk,
) -> Result<(), Failure> {
    match login(user, password, command, talk) {
        Err(Failure::Expired) => {
            let new = pam::change_expired(user, password, talk)?;
            login(user, &new, command, talk)
        }
        result => result,
    }
}

/// One try at greetd with `password`.
pub fn login(
    user: &str,
    password: &str,
    command: &[String],
    talk: &mut dyn Talk,
) -> Result<(), Failure> {
    let path = std::env::var("GREETD_SOCK")
        .map_err(|_| Failure::Other("GREETD_SOCK is not set".into()))?;
    let mut stream = UnixStream::connect(path).map_err(|e| Failure::Other(e.to_string()))?;
    let result = converse(&mut stream, user, password, command, talk);
    if result.is_err() {
        let _ = send(&mut stream, &json!({ "type": "cancel_session" }));
        let _ = recv(&mut stream);
    }
    result
}

fn converse(
    stream: &mut UnixStream,
    user: &str,
    password: &str,
    command: &[String],
    talk: &mut dyn Talk,
) -> Result<(), Failure> {
    send(
        stream,
        &json!({ "type": "create_session", "username": user }),
    )?;
    let mut answered = false;
    loop {
        let reply = recv(stream)?;
        match reply["type"].as_str() {
            Some("success") => break,
            Some("error") => return Err(failure(&reply)),
            Some("auth_message") => {
                let message = reply["auth_message"].as_str().unwrap_or("");
                let response = match reply["auth_message_type"].as_str() {
                    Some("secret") if !answered => {
                        answered = true;
                        Value::from(password)
                    }
                    Some("secret") => Value::from(talk.secret(message).ok_or(Failure::Cancelled)?),
                    Some("visible") => {
                        Value::from(talk.visible(message).ok_or(Failure::Cancelled)?)
                    }
                    Some("info") => {
                        talk.info(message);
                        Value::Null
                    }
                    Some("error") => {
                        talk.error(message);
                        Value::Null
                    }
                    _ => Value::Null,
                };
                send(
                    stream,
                    &json!({ "type": "post_auth_message_response", "response": response }),
                )?;
            }
            _ => return Err(Failure::Other("unexpected reply from greetd".into())),
        }
    }
    send(
        stream,
        &json!({ "type": "start_session", "cmd": command, "env": [] }),
    )?;
    let reply = recv(stream)?;
    match reply["type"].as_str() {
        Some("success") => Ok(()),
        _ => Err(failure(&reply)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conv::tests::Script;
    use std::thread;

    #[test]
    fn exec_from_the_session_file() {
        let file =
            "[Desktop Entry]\nName=Elavo\nExec=/usr/libexec/elavo-session\nType=Application\n";
        assert_eq!(
            exec_line(file),
            Some(vec!["/usr/libexec/elavo-session".to_owned()])
        );
        assert_eq!(
            exec_line("[Desktop Entry]\nExec=labwc %U\n"),
            Some(vec!["labwc".to_owned()])
        );
        assert_eq!(exec_line("[Desktop Entry]\n"), None);
    }

    /// A greetd that asks for one secret and accepts `secret`.
    fn fake_greetd(mut stream: UnixStream) -> Vec<Value> {
        let mut seen = Vec::new();
        loop {
            let Ok(request) = recv(&mut stream) else {
                return seen;
            };
            seen.push(request.clone());
            let reply = match request["type"].as_str() {
                Some("create_session") => json!({
                    "type": "auth_message", "auth_message_type": "secret", "auth_message": "Password: "
                }),
                Some("post_auth_message_response") if request["response"] == "secret" => {
                    json!({ "type": "success" })
                }
                Some("post_auth_message_response") => {
                    json!({ "type": "error", "error_type": "auth_error", "description": "pam_authenticate: AUTH_ERR" })
                }
                _ => json!({ "type": "success" }),
            };
            send(&mut stream, &reply).unwrap();
        }
    }

    /// A greetd in front of FreeIPA two-factor: two secrets, then success.
    fn fake_two_factor(mut stream: UnixStream) -> Vec<Value> {
        let mut seen = Vec::new();
        loop {
            let Ok(request) = recv(&mut stream) else {
                return seen;
            };
            seen.push(request.clone());
            let reply = match (request["type"].as_str(), request["response"].as_str()) {
                (Some("create_session"), _) => json!({
                    "type": "auth_message", "auth_message_type": "secret", "auth_message": "First Factor: "
                }),
                (Some("post_auth_message_response"), Some("pw")) => json!({
                    "type": "auth_message", "auth_message_type": "info", "auth_message": "Enter the code"
                }),
                (Some("post_auth_message_response"), None) => json!({
                    "type": "auth_message", "auth_message_type": "secret", "auth_message": "Second Factor: "
                }),
                (Some("post_auth_message_response"), Some("123456")) => {
                    json!({ "type": "success" })
                }
                (Some("post_auth_message_response"), _) => json!({
                    "type": "error", "error_type": "auth_error", "description": "pam_authenticate: AUTH_ERR"
                }),
                _ => json!({ "type": "success" }),
            };
            send(&mut stream, &reply).unwrap();
        }
    }

    #[test]
    fn a_right_password_starts_the_session() {
        let (mut ours, theirs) = UnixStream::pair().unwrap();
        let server = thread::spawn(move || fake_greetd(theirs));
        let cmd = vec!["/usr/libexec/elavo-session".to_owned()];
        let mut talk = Script::new(&[]);
        assert_eq!(
            converse(&mut ours, "ada", "secret", &cmd, &mut talk),
            Ok(())
        );
        drop(ours);
        let seen = server.join().unwrap();
        assert_eq!(seen[0]["username"], "ada");
        assert_eq!(seen[2]["type"], "start_session");
        assert_eq!(seen[2]["cmd"][0], "/usr/libexec/elavo-session");
    }

    #[test]
    fn a_wrong_password_is_an_auth_failure() {
        let (mut ours, theirs) = UnixStream::pair().unwrap();
        let server = thread::spawn(move || fake_greetd(theirs));
        let mut talk = Script::new(&[]);
        assert_eq!(
            converse(&mut ours, "ada", "guess", &[], &mut talk),
            Err(Failure::Auth)
        );
        drop(ours);
        server.join().unwrap();
    }

    #[test]
    fn a_second_factor_is_asked_for() {
        let (mut ours, theirs) = UnixStream::pair().unwrap();
        let server = thread::spawn(move || fake_two_factor(theirs));
        let mut talk = Script::new(&["123456"]);
        assert_eq!(converse(&mut ours, "otto", "pw", &[], &mut talk), Ok(()));
        drop(ours);
        server.join().unwrap();
        assert_eq!(talk.seen, ["info: Enter the code", "secret: Second Factor"]);
    }

    #[test]
    fn backing_out_of_the_second_factor_cancels() {
        let (mut ours, theirs) = UnixStream::pair().unwrap();
        let server = thread::spawn(move || fake_two_factor(theirs));
        let mut talk = Script::new(&[]);
        assert_eq!(
            converse(&mut ours, "otto", "pw", &[], &mut talk),
            Err(Failure::Cancelled)
        );
        drop(ours);
        server.join().unwrap();
    }

    #[test]
    fn greetd_errors_are_sorted() {
        let error = |kind: &str, description: &str| {
            failure(&json!({ "type": "error", "error_type": kind, "description": description }))
        };
        assert_eq!(
            error("auth_error", "pam_acct_mgmt: NEW_AUTHTOK_REQD"),
            Failure::Expired
        );
        assert_eq!(
            error("auth_error", "pam_acct_mgmt: PERM_DENIED"),
            Failure::Denied
        );
        assert_eq!(
            error("auth_error", "pam_authenticate: AUTH_ERR"),
            Failure::Auth
        );
        assert_eq!(
            error("error", "session is already being created"),
            Failure::Other("session is already being created".into())
        );
    }
}
