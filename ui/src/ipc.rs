//! Talking to the compositor over its socket, as `nekoland msg` does.

use std::{
    io::{Read, Write},
    path::Path,
};

/// Sends the compositor a message and returns its answer.
pub fn msg(message: &str) -> Option<String> {
    let path = std::env::var_os("NEKOLAND_SOCKET")?;
    let mut stream = std::os::unix::net::UnixStream::connect(&path)
        .inspect_err(|err| eprintln!("{}: {err}", Path::new(&path).display()))
        .ok()?;
    stream.write_all(message.as_bytes()).ok()?;
    stream.shutdown(std::net::Shutdown::Write).ok()?;
    let mut reply = String::new();
    stream.read_to_string(&mut reply).ok()?;
    Some(reply)
}
