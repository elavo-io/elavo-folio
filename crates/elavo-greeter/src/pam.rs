//! The lock screen's password check: PAM service `elavo-lock`, which holds
//! `pam_unix` only. libpam is opened at run time, so building Folio does
//! not need its development files.

use std::ffi::{c_char, c_int, c_void, CStr, CString};

const SERVICE: &CStr = c"elavo-lock";
const LIBPAM: &CStr = c"libpam.so.0";

const PAM_SUCCESS: c_int = 0;
const PAM_BUF_ERR: c_int = 5;
const PAM_CONV_ERR: c_int = 19;
const PAM_PROMPT_ECHO_OFF: c_int = 1;
const PAM_PROMPT_ECHO_ON: c_int = 2;
const PAM_REFRESH_CRED: c_int = 0x0010;

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

struct Lib {
    start: StartFn,
    authenticate: HandleFn,
    setcred: HandleFn,
    end: HandleFn,
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
            setcred: std::mem::transmute::<*mut c_void, HandleFn>(sym(c"pam_setcred")?),
            end: std::mem::transmute::<*mut c_void, HandleFn>(sym(c"pam_end")?),
        })
    }
}

/// Answers every hidden or visible prompt with the password. PAM frees the
/// responses, so they come from the C allocator.
unsafe extern "C" fn converse(
    count: c_int,
    messages: *mut *const Message,
    responses: *mut *mut Response,
    appdata: *mut c_void,
) -> c_int {
    if count <= 0 || messages.is_null() || responses.is_null() {
        return PAM_CONV_ERR;
    }
    let password = appdata as *const c_char;
    let list = libc::calloc(count as usize, std::mem::size_of::<Response>()) as *mut Response;
    if list.is_null() {
        return PAM_BUF_ERR;
    }
    for i in 0..count as usize {
        let message = *messages.add(i);
        if message.is_null() {
            continue;
        }
        if matches!((*message).style, PAM_PROMPT_ECHO_OFF | PAM_PROMPT_ECHO_ON) {
            let copy = libc::strdup(password);
            if copy.is_null() {
                for j in 0..i {
                    libc::free((*list.add(j)).resp as *mut c_void);
                }
                libc::free(list as *mut c_void);
                return PAM_BUF_ERR;
            }
            (*list.add(i)).resp = copy;
        }
    }
    *responses = list;
    PAM_SUCCESS
}

/// Whether `password` is `user`'s.
pub fn check(user: &str, password: &str) -> Result<bool, String> {
    let lib = open()?;
    let user = CString::new(user).map_err(|e| e.to_string())?;
    let password = CString::new(password).map_err(|_| "the password holds a NUL".to_owned())?;
    let conv = Conv {
        conv: converse,
        appdata: password.as_ptr() as *mut c_void,
    };
    unsafe {
        let mut handle: *mut c_void = std::ptr::null_mut();
        let started = (lib.start)(SERVICE.as_ptr(), user.as_ptr(), &conv, &mut handle);
        if started != PAM_SUCCESS || handle.is_null() {
            return Err(format!("pam_start failed ({started})"));
        }
        let status = (lib.authenticate)(handle, 0);
        if status == PAM_SUCCESS {
            (lib.setcred)(handle, PAM_REFRESH_CRED);
        }
        (lib.end)(handle, status);
        Ok(status == PAM_SUCCESS)
    }
}

#[cfg(test)]
mod tests {
    /// Talks to the machine's PAM, so it only runs when asked:
    /// `cargo test -p elavo-greeter -- --ignored`.
    #[test]
    #[ignore]
    fn an_unknown_account_is_refused() {
        assert_eq!(super::check("elavo-no-such-user", "guess"), Ok(false));
    }
}
