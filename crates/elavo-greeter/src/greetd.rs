//! greetd's IPC: a native-endian u32 length, then a JSON object, each way,
//! on the socket in `$GREETD_SOCK`. The password goes through greetd's own
//! PAM stack, which also unlocks the login keyring.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;

use serde_json::{json, Value};

const SESSION_FILE: &str = "/usr/share/wayland-sessions/elavo.desktop";
const FALLBACK_SESSION: &str = "/usr/libexec/elavo-session";

#[derive(Debug, PartialEq, Eq)]
pub enum Failure {
    /// The password was wrong.
    Auth,
    Other(String),
}

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

fn failure(reply: &Value) -> Failure {
    if reply["error_type"] == "auth_error" {
        Failure::Auth
    } else {
        Failure::Other(reply["description"].as_str().unwrap_or("").to_owned())
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

/// Sign `user` in with `password`, then ask greetd to start the session.
/// greetd starts it once the greeter exits.
pub fn login(user: &str, password: &str, command: &[String]) -> Result<(), Failure> {
    let path = std::env::var("GREETD_SOCK")
        .map_err(|_| Failure::Other("GREETD_SOCK is not set".into()))?;
    let mut stream = UnixStream::connect(path).map_err(|e| Failure::Other(e.to_string()))?;
    let result = converse(&mut stream, user, password, command);
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
                let response = match reply["auth_message_type"].as_str() {
                    Some("secret") if !answered => {
                        answered = true;
                        Value::from(password)
                    }
                    Some("secret") | Some("visible") => {
                        let message = reply["auth_message"].as_str().unwrap_or("");
                        return Err(Failure::Other(message.to_owned()));
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
                    json!({ "type": "error", "error_type": "auth_error", "description": "nope" })
                }
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
        assert_eq!(converse(&mut ours, "ada", "secret", &cmd), Ok(()));
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
        assert_eq!(converse(&mut ours, "ada", "guess", &[]), Err(Failure::Auth));
        drop(ours);
        server.join().unwrap();
    }
}
