//! The kernel's own device events, for a system without udev, which would otherwise pass them
//! on: unkernel's guest. Displays changing size and input devices coming and going are heard
//! here instead.

use std::{
    io,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
};

use smithay::reexports::calloop::{generic::Generic, Interest, LoopHandle, Mode, PostAction};

use crate::Nekoland;

/// The multicast group the kernel sends its uevents to; udevd's own are group 2.
const KERNEL_UEVENT_GROUP: u32 = 1;
/// Uevents are a few hundred bytes; the kernel sends none longer than a page.
const UEVENT_BUFFER_SIZE: usize = 8192;

/// One device event: what happened, to which subsystem's device, and the rest it said.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Uevent {
    pub action: String,
    pub subsystem: String,
    /// Under `/dev`, as `input/event3` or `dri/card0`; empty for a device without a node.
    pub devname: String,
    pub hotplug: bool,
}

impl Uevent {
    /// Reads one message: `ACTION@DEVPATH`, then `KEY=VALUE` fields, each ending in a NUL.
    pub fn parse(message: &[u8]) -> Option<Self> {
        let mut fields = message.split(|&byte| byte == 0).map(String::from_utf8_lossy);
        // A header without '@' is libudev's, which only udevd sends.
        fields.next().filter(|header| header.contains('@'))?;
        let mut event = Self::default();
        for field in fields {
            let Some((key, value)) = field.split_once('=') else {
                continue;
            };
            match key {
                "ACTION" => event.action = value.into(),
                "SUBSYSTEM" => event.subsystem = value.into(),
                "DEVNAME" => event.devname = value.into(),
                "HOTPLUG" => event.hotplug = value == "1",
                _ => {}
            }
        }
        Some(event)
    }
}

/// Calls `on_event` with each uevent the kernel sends from now on.
pub fn watch(
    handle: &LoopHandle<'static, Nekoland>,
    mut on_event: impl FnMut(Uevent, &mut Nekoland) + 'static,
) -> io::Result<()> {
    let socket = open_uevent_socket()?;
    handle
        .insert_source(
            Generic::new(socket, Interest::READ, Mode::Level),
            move |_, socket, data| {
                let mut buffer = vec![0u8; UEVENT_BUFFER_SIZE];
                loop {
                    // Safety: the buffer is writable for its whole length.
                    let received = unsafe {
                        libc::recv(
                            socket.as_raw_fd(),
                            buffer.as_mut_ptr().cast(),
                            buffer.len(),
                            libc::MSG_DONTWAIT,
                        )
                    };
                    if received < 0 {
                        let err = io::Error::last_os_error();
                        // ENOBUFS: events were lost while the queue was full; the next ones still come.
                        match err.raw_os_error() {
                            Some(libc::EAGAIN) => return Ok(PostAction::Continue),
                            Some(libc::ENOBUFS) => continue,
                            _ => return Err(err),
                        }
                    }
                    let length = usize::try_from(received).unwrap_or_default();
                    if let Some(event) = Uevent::parse(&buffer[..length]) {
                        on_event(event, data);
                    }
                }
            },
        )
        .map_err(|err| err.error)?;
    Ok(())
}

fn open_uevent_socket() -> io::Result<OwnedFd> {
    // Safety: a plain socket call; its result is checked before use.
    let raw = unsafe {
        libc::socket(
            libc::AF_NETLINK,
            libc::SOCK_DGRAM | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
            libc::NETLINK_KOBJECT_UEVENT,
        )
    };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    // Safety: the descriptor was just made and belongs to nothing else.
    let socket = unsafe { OwnedFd::from_raw_fd(raw) };
    // Safety: sockaddr_nl is plain integers, for which zero is a value.
    let mut address: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
    address.nl_family = libc::AF_NETLINK as libc::sa_family_t;
    address.nl_groups = KERNEL_UEVENT_GROUP;
    // Safety: the address is a sockaddr_nl of the size given.
    let bound = unsafe {
        libc::bind(
            socket.as_raw_fd(),
            std::ptr::from_ref(&address).cast(),
            std::mem::size_of::<libc::sockaddr_nl>() as libc::socklen_t,
        )
    };
    if bound < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(socket)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_display_hotplug() {
        let message =
            b"change@/devices/virtio0/drm/card0\0ACTION=change\0DEVPATH=/devices/virtio0/drm/card0\0\
            SUBSYSTEM=drm\0HOTPLUG=1\0DEVNAME=dri/card0\0SEQNUM=812\0";
        let event = Uevent::parse(message).unwrap();
        assert_eq!(
            event,
            Uevent {
                action: "change".into(),
                subsystem: "drm".into(),
                devname: "dri/card0".into(),
                hotplug: true
            }
        );
    }

    #[test]
    fn skips_libudev_messages() {
        assert_eq!(Uevent::parse(b"libudev\0\xfe\xed\xca\xfe"), None);
    }
}
