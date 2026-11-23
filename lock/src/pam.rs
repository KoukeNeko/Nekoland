//! Checking a password with PAM, as the `nekoland-lock` service says to.

use std::{
    ffi::{c_char, c_int, c_void, CString},
    ptr,
};

const SERVICE: &str = "nekoland-lock";

const PAM_SUCCESS: c_int = 0;
const PAM_BUF_ERR: c_int = 5;
const PAM_AUTH_ERR: c_int = 7;
/// The password has aged out: it is still the right one, and changing it is for later.
const PAM_NEW_AUTHTOK_REQD: c_int = 12;
const PAM_PROMPT_ECHO_OFF: c_int = 1;
const PAM_PROMPT_ECHO_ON: c_int = 2;
const PAM_REFRESH_CRED: c_int = 0x0010;

#[repr(C)]
struct Message {
    style: c_int,
    text: *const c_char,
}

#[repr(C)]
struct Response {
    text: *mut c_char,
    code: c_int,
}

#[repr(C)]
struct Conversation {
    converse: extern "C" fn(c_int, *mut *const Message, *mut *mut Response, *mut c_void) -> c_int,
    data: *mut c_void,
}

#[link(name = "pam")]
extern "C" {
    fn pam_start(
        service: *const c_char,
        user: *const c_char,
        conversation: *const Conversation,
        handle: *mut *mut c_void,
    ) -> c_int;
    fn pam_authenticate(handle: *mut c_void, flags: c_int) -> c_int;
    fn pam_acct_mgmt(handle: *mut c_void, flags: c_int) -> c_int;
    fn pam_setcred(handle: *mut c_void, flags: c_int) -> c_int;
    fn pam_end(handle: *mut c_void, status: c_int) -> c_int;
    fn pam_strerror(handle: *mut c_void, code: c_int) -> *const c_char;
}

pub enum Failure {
    /// The password is not the user's.
    Wrong,
    /// PAM could not tell, for a reason it gives.
    Unable(String),
}

/// Answers each prompt for a secret with the password; PAM frees what is given.
extern "C" fn converse(
    count: c_int,
    messages: *mut *const Message,
    responses: *mut *mut Response,
    data: *mut c_void,
) -> c_int {
    let Ok(count) = usize::try_from(count) else {
        return PAM_BUF_ERR;
    };
    // SAFETY: PAM hands an array of `count` messages and a place for the answers, and `data`
    // is the password given to `pam_start` below, alive until `pam_end`.
    unsafe {
        let password = &*(data as *const CString);
        let answers = libc::calloc(count, std::mem::size_of::<Response>()) as *mut Response;
        if answers.is_null() {
            return PAM_BUF_ERR;
        }
        for i in 0..count {
            let message = &**messages.add(i);
            if matches!(message.style, PAM_PROMPT_ECHO_OFF | PAM_PROMPT_ECHO_ON) {
                (*answers.add(i)).text = libc::strdup(password.as_ptr());
            }
        }
        *responses = answers;
    }
    PAM_SUCCESS
}

/// Whether `password` is `user`'s. Blocks for as long as PAM takes, seconds when wrong.
pub fn check(user: &str, password: &str) -> Result<(), Failure> {
    let password = CString::new(password).map_err(|_| Failure::Wrong)?;
    let result = run(user, &password, true);
    wipe(password.into_bytes());
    result
}

/// Whether `user`'s account may unlock, for what unlocks without the password, a PIN or a
/// finger: one that has expired or been locked may not.
pub fn account(user: &str) -> Result<(), Failure> {
    run(user, &CString::default(), false)
}

/// Overwrites a password's bytes before they are freed, so that a copy of the memory, as a
/// core dump is, does not have it.
pub fn wipe(bytes: impl Into<Vec<u8>>) {
    let mut bytes = bytes.into();
    // Out to the capacity: past the length may be what was typed before it was shortened.
    bytes.resize(bytes.capacity(), 0);
    for byte in bytes.iter_mut() {
        // SAFETY: a byte of the vector, written in place; volatile, so it is not left out.
        unsafe { ptr::write_volatile(byte, 0) };
    }
}

fn run(user: &str, password: &CString, authenticate: bool) -> Result<(), Failure> {
    let unable = |why: &str| Failure::Unable(why.to_string());
    let service = CString::new(SERVICE).expect("no NUL in the name");
    let user = CString::new(user).map_err(|_| unable("user name has a NUL"))?;
    let conversation = Conversation {
        converse,
        data: password as *const CString as *mut c_void,
    };
    let mut handle = ptr::null_mut();
    // SAFETY: every pointer lives until `pam_end`, which is called whatever happens.
    unsafe {
        let started = pam_start(service.as_ptr(), user.as_ptr(), &conversation, &mut handle);
        if started != PAM_SUCCESS {
            return Err(unable(&format!("pam_start failed ({started})")));
        }
        let said = |status: c_int| {
            let text = pam_strerror(handle, status);
            Failure::Unable(if text.is_null() {
                format!("PAM error {status}")
            } else {
                std::ffi::CStr::from_ptr(text).to_string_lossy().into_owned()
            })
        };
        let mut status = if authenticate { pam_authenticate(handle, 0) } else { PAM_SUCCESS };
        let result = match status {
            PAM_SUCCESS => {
                // The right password of an account that has expired or been locked does not
                // unlock, as it would not log in; one only due to be changed does.
                status = pam_acct_mgmt(handle, 0);
                if matches!(status, PAM_SUCCESS | PAM_NEW_AUTHTOK_REQD) {
                    // Renews Kerberos tickets and the like; failing to is no reason to stay locked.
                    pam_setcred(handle, PAM_REFRESH_CRED);
                    Ok(())
                } else {
                    Err(said(status))
                }
            }
            PAM_AUTH_ERR => Err(Failure::Wrong),
            _ => Err(said(status)),
        };
        pam_end(handle, status);
        result
    }
}
