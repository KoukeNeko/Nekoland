//! The PIN: digits that unlock as the password does, kept hashed in the user's data folder.
//! It is a weaker secret than a password, so it is only for the lock screen, and a lock takes
//! `TRIES` wrong ones before it wants the password.

use std::{
    ffi::{c_char, CStr, CString},
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::PathBuf,
};

pub const LEAST_DIGITS: usize = 6;
/// How many wrong PINs one lock takes before only the password unlocks.
pub const TRIES: u32 = 5;
/// SHA-512 crypt's rounds, to make each guess of an attacker who has the file cost time.
const ROUNDS: u32 = 1_000_000;
const SALT_ALPHABET: &[u8] = b"./0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

fn path() -> Option<PathBuf> {
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))?;
    Some(data.join("nekoland/pin"))
}

pub fn is_valid(pin: &str) -> bool {
    pin.len() >= LEAST_DIGITS && pin.bytes().all(|byte| byte.is_ascii_digit())
}

/// `crypt(3)`, from libcrypt, opened by its name: the link to it for building is in a
/// development package that is not always installed.
fn crypt(key: &str, setting: &str) -> Option<String> {
    type Crypt = unsafe extern "C" fn(*const c_char, *const c_char) -> *mut c_char;
    let (setting, key) = (CString::new(setting).ok()?, CString::new(key).ok()?);
    // Left by `break`, not `return`, so that the key's copy is wiped whatever happens.
    // SAFETY: the symbol is crypt(3), whose signature `Crypt` is, called with C strings that
    // live through the call; its answer is read before anything else is called.
    let hashed = unsafe { 'call: {
        let library = libc::dlopen(c"libcrypt.so.1".as_ptr(), libc::RTLD_NOW);
        if library.is_null() {
            break 'call None;
        }
        let symbol = libc::dlsym(library, c"crypt".as_ptr());
        if symbol.is_null() {
            break 'call None;
        }
        let crypt: Crypt = std::mem::transmute(symbol);
        let hashed = crypt(key.as_ptr(), setting.as_ptr());
        if hashed.is_null() {
            break 'call None;
        }
        // A failure is a string that begins with `*`.
        let hashed = CStr::from_ptr(hashed).to_string_lossy().into_owned();
        (!hashed.starts_with('*')).then_some(hashed)
    } };
    crate::pam::wipe(key);
    hashed
}

fn hash(pin: &str) -> Result<String, String> {
    let mut random = [0u8; 16];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut urandom| std::io::Read::read_exact(&mut urandom, &mut random))
        .map_err(|err| format!("/dev/urandom: {err}"))?;
    let salt: String = random.iter().map(|byte| char::from(SALT_ALPHABET[usize::from(*byte) % SALT_ALPHABET.len()])).collect();
    crypt(pin, &format!("$6$rounds={ROUNDS}${salt}")).ok_or_else(|| "libcrypt could not hash the PIN".to_string())
}

/// Whether `pin` is the one `stored` was made from, in the time a wrong one takes.
fn matches(pin: &str, stored: &str) -> bool {
    let Some(again) = crypt(pin, stored) else { return false };
    again.len() == stored.len() && again.bytes().zip(stored.bytes()).fold(0, |different, (a, b)| different | (a ^ b)) == 0
}

pub fn is_set() -> bool {
    path().is_some_and(|path| path.exists())
}

pub fn check(pin: &str) -> bool {
    let stored = path().and_then(|path| std::fs::read_to_string(path).ok());
    stored.is_some_and(|stored| matches(pin, stored.trim()))
}

pub fn set(pin: &str) -> Result<(), String> {
    if !is_valid(pin) {
        return Err(format!("a PIN is at least {LEAST_DIGITS} digits"));
    }
    let path = path().ok_or("no home folder")?;
    let hashed = hash(pin)?;
    let failed = |err: std::io::Error| format!("{}: {err}", path.display());
    std::fs::create_dir_all(path.parent().unwrap_or(&path)).map_err(failed)?;
    // Written whole or not at all, and for the user alone.
    let partial = path.with_extension("partial");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&partial)
        .map_err(failed)?;
    file.write_all(hashed.as_bytes()).and_then(|()| file.sync_all()).map_err(failed)?;
    std::fs::rename(&partial, &path).map_err(failed)
}

pub fn clear() -> Result<(), String> {
    let path = path().ok_or("no home folder")?;
    match std::fs::remove_file(&path) {
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(format!("{}: {err}", path.display())),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pin_is_six_digits_or_more() {
        assert!(is_valid("123456"));
        assert!(is_valid("12345678"));
        assert!(!is_valid("12345"));
        assert!(!is_valid("12345a"));
        assert!(!is_valid(""));
    }

    #[test]
    fn a_hashed_pin_matches_only_itself() {
        let stored = hash("246810").unwrap();
        assert!(stored.starts_with("$6$rounds="));
        assert!(!stored.contains("246810"));
        assert!(matches("246810", &stored));
        assert!(!matches("246811", &stored));
        assert!(!matches("246810", "not a hash"));
        // Salted: the same PIN is not the same line.
        assert_ne!(stored, hash("246810").unwrap());
    }
}
