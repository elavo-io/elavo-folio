//! PAM for the lock screen and for changing an expired password.
//!
//! The lock checks the signed-in person with service `elavo-lock`, which
//! uses the system's password-auth stack, so directory accounts unlock the
//! same way local ones do. It runs the account check too, and when the
//! password has expired it changes it on the spot. At login, greetd's own
//! stack stops at an expired password; the greeter changes it here with
//! service `elavo-passwd` and signs in again.
//!
//! Each prompt is answered by its kind: the first hidden prompt gets the
//! password typed, the "current password" prompt of a change gets it too,
//! and every other prompt goes to the screen. Messages go to the screen.
//! libpam is opened at run time, so building Folio does not need its
//! development files.

use std::ffi::{c_char, c_int, c_void, CStr, CString};

use crate::conv::{asks_current, Failure, Talk, UNREACHABLE};

pub const LOCK: &CStr = c"elavo-lock";
pub const PASSWD: &CStr = c"elavo-passwd";
const LIBPAM: &CStr = c"libpam.so.0";

const PAM_SUCCESS: c_int = 0;
const PAM_BUF_ERR: c_int = 5;
const PAM_PERM_DENIED: c_int = 6;
const PAM_AUTH_ERR: c_int = 7;
const PAM_CRED_INSUFFICIENT: c_int = 8;
const PAM_AUTHINFO_UNAVAIL: c_int = 9;
const PAM_USER_UNKNOWN: c_int = 10;
const PAM_MAXTRIES: c_int = 11;
const PAM_NEW_AUTHTOK_REQD: c_int = 12;
const PAM_ACCT_EXPIRED: c_int = 13;
const PAM_CONV_ERR: c_int = 19;
const PAM_AUTHTOK_ERR: c_int = 20;

const PAM_PROMPT_ECHO_OFF: c_int = 1;
const PAM_PROMPT_ECHO_ON: c_int = 2;
const PAM_ERROR_MSG: c_int = 3;
const PAM_TEXT_INFO: c_int = 4;

const PAM_REFRESH_CRED: c_int = 0x0010;
const PAM_CHANGE_EXPIRED_AUTHTOK: c_int = 0x0020;

/// How many times a new password may be refused (too weak, or the two
/// entries differ) before the change gives up.
const CHANGE_TRIES: usize = 3;

#[repr(C)]
struct Message {
    style: c_int,
    msg: *const c_char,
}

#[repr(C)]
struct Response {
    resp: *mut c_char,
    retcode: c_int,
}

type ConvFn =
    unsafe extern "C" fn(c_int, *mut *const Message, *mut *mut Response, *mut c_void) -> c_int;

#[repr(C)]
struct Conv {
    conv: ConvFn,
    appdata: *mut c_void,
}

type StartFn =
    unsafe extern "C" fn(*const c_char, *const c_char, *const Conv, *mut *mut c_void) -> c_int;
type HandleFn = unsafe extern "C" fn(*mut c_void, c_int) -> c_int;
type StrErrorFn = unsafe extern "C" fn(*mut c_void, c_int) -> *const c_char;

struct Lib {
    start: StartFn,
    authenticate: HandleFn,
    acct_mgmt: HandleFn,
    chauthtok: HandleFn,
    setcred: HandleFn,
    end: HandleFn,
    strerror: StrErrorFn,
}

fn open() -> Result<Lib, String> {
    unsafe {
        let handle = libc::dlopen(LIBPAM.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL);
        if handle.is_null() {
            return Err("libpam is not installed".into());
        }
        let sym = |name: &CStr| {
            let p = libc::dlsym(handle, name.as_ptr());
            if p.is_null() {
                Err(format!("libpam has no {}", name.to_string_lossy()))
            } else {
                Ok(p)
            }
        };
        Ok(Lib {
            start: std::mem::transmute::<*mut c_void, StartFn>(sym(c"pam_start")?),
            authenticate: std::mem::transmute::<*mut c_void, HandleFn>(sym(c"pam_authenticate")?),
            acct_mgmt: std::mem::transmute::<*mut c_void, HandleFn>(sym(c"pam_acct_mgmt")?),
            chauthtok: std::mem::transmute::<*mut c_void, HandleFn>(sym(c"pam_chauthtok")?),
            setcred: std::mem::transmute::<*mut c_void, HandleFn>(sym(c"pam_setcred")?),
            end: std::mem::transmute::<*mut c_void, HandleFn>(sym(c"pam_end")?),
            strerror: std::mem::transmute::<*mut c_void, StrErrorFn>(sym(c"pam_strerror")?),
        })
    }
}

/// What the conversation function needs. It lives in a box for the whole
/// PAM transaction, since libpam keeps a pointer to it.
struct Data<'a> {
    talk: &'a mut dyn Talk,
    /// Answers the first hidden prompt, then is gone.
    password: Option<CString>,
    /// Answers a "current password" prompt during a change.
    current: Option<CString>,
    /// The last hidden answer the person typed: the new password, after a
    /// change.
    typed: Option<String>,
    cancelled: bool,
}

impl Data<'_> {
    /// The answer to one prompt, or `None` to stop the conversation.
    fn answer(&mut self, style: c_int, text: &str) -> Option<Option<CString>> {
        match style {
            PAM_PROMPT_ECHO_OFF => {
                if let Some(password) = self.password.take() {
                    return Some(Some(password));
                }
                if asks_current(text) {
                    if let Some(current) = &self.current {
                        return Some(Some(current.clone()));
                    }
                }
                let typed = self.talk.secret(text)?;
                let answer = CString::new(typed.clone()).ok()?;
                self.typed = Some(typed);
                Some(Some(answer))
            }
            PAM_PROMPT_ECHO_ON => {
                let typed = self.talk.visible(text)?;
                Some(Some(CString::new(typed).ok()?))
            }
            PAM_ERROR_MSG => {
                self.talk.error(text);
                Some(None)
            }
            PAM_TEXT_INFO => {
                self.talk.info(text);
                Some(None)
            }
            _ => Some(None),
        }
    }
}

unsafe fn free_responses(list: *mut Response, count: usize) {
    for j in 0..count {
        let resp = (*list.add(j)).resp;
        if !resp.is_null() {
            // Clear answers before giving the memory back.
            let len = libc::strlen(resp);
            std::ptr::write_bytes(resp, 0, len);
            libc::free(resp as *mut c_void);
        }
    }
    libc::free(list as *mut c_void);
}

/// PAM frees the responses, so they come from the C allocator.
unsafe extern "C" fn converse(
    count: c_int,
    messages: *mut *const Message,
    responses: *mut *mut Response,
    appdata: *mut c_void,
) -> c_int {
    if count <= 0 || messages.is_null() || responses.is_null() || appdata.is_null() {
        return PAM_CONV_ERR;
    }
    let data = &mut *(appdata as *mut Data<'static>);
    let count = count as usize;
    let list = libc::calloc(count, std::mem::size_of::<Response>()) as *mut Response;
    if list.is_null() {
        return PAM_BUF_ERR;
    }
    for i in 0..count {
        let message = *messages.add(i);
        if message.is_null() {
            continue;
        }
        let text = if (*message).msg.is_null() {
            String::new()
        } else {
            CStr::from_ptr((*message).msg)
                .to_string_lossy()
                .into_owned()
        };
        match data.answer((*message).style, &text) {
            Some(Some(answer)) => {
                let copy = libc::strdup(answer.as_ptr());
                if copy.is_null() {
                    free_responses(list, i);
                    return PAM_BUF_ERR;
                }
                (*list.add(i)).resp = copy;
            }
            Some(None) => {}
            None => {
                data.cancelled = true;
                free_responses(list, i);
                return PAM_CONV_ERR;
            }
        }
    }
    *responses = list;
    PAM_SUCCESS
}

/// One PAM transaction.
struct Session<'a> {
    lib: Lib,
    handle: *mut c_void,
    data: Box<Data<'a>>,
    _conv: Box<Conv>,
    status: c_int,
}

impl<'a> Session<'a> {
    fn start(
        service: &CStr,
        user: &str,
        password: &str,
        talk: &'a mut dyn Talk,
    ) -> Result<Self, Failure> {
        let lib = open().map_err(Failure::Other)?;
        let user = CString::new(user).map_err(|_| Failure::Auth)?;
        let password = CString::new(password).map_err(|_| Failure::Auth)?;
        let mut data = Box::new(Data {
            talk,
            password: Some(password.clone()),
            current: Some(password),
            typed: None,
            cancelled: false,
        });
        let conv = Box::new(Conv {
            conv: converse,
            appdata: &mut *data as *mut Data<'a> as *mut c_void,
        });
        let mut handle: *mut c_void = std::ptr::null_mut();
        let started = unsafe { (lib.start)(service.as_ptr(), user.as_ptr(), &*conv, &mut handle) };
        if started != PAM_SUCCESS || handle.is_null() {
            return Err(Failure::Other(format!("pam_start failed ({started})")));
        }
        Ok(Self {
            lib,
            handle,
            data,
            _conv: conv,
            status: PAM_SUCCESS,
        })
    }

    fn run(&mut self, step: HandleFn, flags: c_int) -> c_int {
        self.status = unsafe { step(self.handle, flags) };
        self.status
    }

    fn describe(&self, code: c_int) -> String {
        unsafe {
            let text = (self.lib.strerror)(self.handle, code);
            if text.is_null() {
                format!("PAM error {code}")
            } else {
                CStr::from_ptr(text).to_string_lossy().into_owned()
            }
        }
    }

    fn cancelled(&self) -> bool {
        self.data.cancelled
    }

    /// The password and any further factors.
    fn authenticate(&mut self) -> Result<(), Failure> {
        let code = self.run(self.lib.authenticate, 0);
        match code {
            PAM_SUCCESS => Ok(()),
            _ if self.cancelled() => Err(Failure::Cancelled),
            PAM_AUTH_ERR | PAM_CRED_INSUFFICIENT | PAM_USER_UNKNOWN | PAM_MAXTRIES => {
                Err(Failure::Auth)
            }
            PAM_AUTHINFO_UNAVAIL => Err(Failure::Other(UNREACHABLE.to_owned())),
            PAM_PERM_DENIED | PAM_ACCT_EXPIRED => Err(Failure::Denied),
            _ => Err(Failure::Other(self.describe(code))),
        }
    }

    /// Whether the account may be used now. `Expired` when the password
    /// must be changed first.
    fn account(&mut self) -> Result<(), Failure> {
        let code = self.run(self.lib.acct_mgmt, 0);
        match code {
            PAM_SUCCESS => Ok(()),
            PAM_NEW_AUTHTOK_REQD => Err(Failure::Expired),
            PAM_PERM_DENIED | PAM_ACCT_EXPIRED | PAM_USER_UNKNOWN => Err(Failure::Denied),
            PAM_AUTHINFO_UNAVAIL => Err(Failure::Other(UNREACHABLE.to_owned())),
            _ => Err(Failure::Other(self.describe(code))),
        }
    }

    /// Changes the expired password. Returns the new one.
    fn change_expired(&mut self) -> Result<String, Failure> {
        for _ in 0..CHANGE_TRIES {
            self.data.typed = None;
            let code = self.run(self.lib.chauthtok, PAM_CHANGE_EXPIRED_AUTHTOK);
            if self.cancelled() {
                return Err(Failure::Cancelled);
            }
            match code {
                PAM_SUCCESS => {
                    return self
                        .data
                        .typed
                        .take()
                        .ok_or_else(|| Failure::Other("No new password was given.".into()))
                }
                // Too weak, or the two entries differ. PAM has said why.
                PAM_AUTHTOK_ERR => continue,
                _ => return Err(Failure::Other(self.describe(code))),
            }
        }
        Err(Failure::Other("The password was not changed.".into()))
    }

    fn refresh(&mut self) {
        unsafe { (self.lib.setcred)(self.handle, PAM_REFRESH_CRED) };
    }
}

impl Drop for Session<'_> {
    fn drop(&mut self) {
        unsafe { (self.lib.end)(self.handle, self.status) };
    }
}

/// Unlock for `user` with `password`, asking for anything else the
/// account needs: a second factor, or a new password.
pub fn unlock(user: &str, password: &str, talk: &mut dyn Talk) -> Result<(), Failure> {
    let mut session = Session::start(LOCK, user, password, talk)?;
    session.authenticate()?;
    match session.account() {
        Ok(()) => {}
        Err(Failure::Expired) => {
            session.change_expired()?;
        }
        Err(other) => return Err(other),
    }
    session.refresh();
    Ok(())
}

/// Change `user`'s expired password, `password`, at login. Returns the new
/// password, to sign in with.
pub fn change_expired(user: &str, password: &str, talk: &mut dyn Talk) -> Result<String, Failure> {
    let mut session = Session::start(PASSWD, user, password, talk)?;
    session.authenticate()?;
    match session.account() {
        Err(Failure::Expired) => session.change_expired(),
        // The directory no longer asks for a change: sign in as before.
        Ok(()) => Ok(password.to_owned()),
        Err(other) => Err(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conv::tests::Script;

    fn data(script: &mut Script) -> Data<'_> {
        Data {
            talk: script,
            password: Some(c"typed".to_owned()),
            current: Some(c"typed".to_owned()),
            typed: None,
            cancelled: false,
        }
    }

    #[test]
    fn the_first_hidden_prompt_gets_the_password() {
        let mut script = Script::new(&["123456"]);
        let mut data = data(&mut script);
        let first = data.answer(PAM_PROMPT_ECHO_OFF, "First Factor: ");
        assert_eq!(first, Some(Some(c"typed".to_owned())));
        let second = data.answer(PAM_PROMPT_ECHO_OFF, "Second Factor: ");
        assert_eq!(second, Some(Some(c"123456".to_owned())));
        drop(data);
        assert_eq!(script.seen, ["secret: Second Factor"]);
    }

    #[test]
    fn a_change_asks_only_for_the_new_password() {
        let mut script = Script::new(&["n3w-Pass", "n3w-Pass"]);
        let mut data = data(&mut script);
        data.password = None;
        assert_eq!(data.answer(PAM_TEXT_INFO, "Password expired."), Some(None));
        assert_eq!(
            data.answer(PAM_PROMPT_ECHO_OFF, "Current Password: "),
            Some(Some(c"typed".to_owned()))
        );
        data.answer(PAM_PROMPT_ECHO_OFF, "New password: ");
        data.answer(PAM_PROMPT_ECHO_OFF, "Retype new password: ");
        assert_eq!(data.typed.as_deref(), Some("n3w-Pass"));
        drop(data);
        assert_eq!(
            script.seen,
            [
                "info: Password expired.",
                "secret: New password",
                "secret: Retype new password"
            ]
        );
    }

    #[test]
    fn backing_out_stops_the_conversation() {
        let mut script = Script::new(&[]);
        let mut data = data(&mut script);
        data.password = None;
        assert_eq!(data.answer(PAM_PROMPT_ECHO_OFF, "Second Factor: "), None);
        assert_eq!(data.answer(PAM_PROMPT_ECHO_ON, "Code: "), None);
    }

    #[test]
    fn errors_are_shown() {
        let mut script = Script::new(&[]);
        let mut data = data(&mut script);
        assert_eq!(
            data.answer(PAM_ERROR_MSG, "Passwords do not match."),
            Some(None)
        );
        drop(data);
        assert_eq!(script.seen, ["error: Passwords do not match."]);
    }

    /// Talks to the machine's PAM, so it only runs when asked:
    /// `cargo test -p elavo-greeter -- --ignored`.
    #[test]
    #[ignore]
    fn an_unknown_account_is_refused() {
        let mut script = Script::new(&[]);
        assert_eq!(
            unlock("elavo-no-such-user", "guess", &mut script),
            Err(Failure::Auth)
        );
    }

    /// For a test machine: `ELAVO_PAM_USER`, `ELAVO_PAM_PASSWORD`, and
    /// optional `ELAVO_PAM_ANSWERS` (answers to later prompts, split by
    /// `|`). `ELAVO_PAM_CHANGE=1` runs the login password change instead of
    /// the unlock. Prints what PAM showed and asked, and the result.
    #[test]
    #[ignore]
    fn try_an_account_from_the_environment() {
        let Ok(user) = std::env::var("ELAVO_PAM_USER") else {
            return;
        };
        let password = std::env::var("ELAVO_PAM_PASSWORD").unwrap_or_default();
        let answers = std::env::var("ELAVO_PAM_ANSWERS").unwrap_or_default();
        let answers: Vec<&str> = answers.split('|').filter(|a| !a.is_empty()).collect();
        let mut script = Script::new(&answers);
        let result = if std::env::var("ELAVO_PAM_CHANGE").as_deref() == Ok("1") {
            change_expired(&user, &password, &mut script).map(|_| ())
        } else {
            unlock(&user, &password, &mut script)
        };
        for line in &script.seen {
            println!("  {line}");
        }
        println!("result: {result:?}");
    }
}
