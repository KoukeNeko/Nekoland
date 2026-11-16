//! What can hold the keyboard: a Wayland surface, or an X11 window, which also has
//! to be told through the X server.

use std::borrow::Cow;

use smithay::{
    backend::input::KeyState,
    input::{
        keyboard::{KeyboardTarget, KeysymHandle, ModifiersState},
        Seat,
    },
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{IsAlive, Serial},
    wayland::seat::WaylandFocus,
    xwayland::X11Surface,
};

use crate::Nekoland;

#[derive(Debug, Clone, PartialEq)]
pub enum KeyFocus {
    Wayland(WlSurface),
    X11(X11Surface),
}

impl From<WlSurface> for KeyFocus {
    fn from(surface: WlSurface) -> Self {
        Self::Wayland(surface)
    }
}

impl IsAlive for KeyFocus {
    fn alive(&self) -> bool {
        match self {
            Self::Wayland(surface) => surface.alive(),
            Self::X11(surface) => surface.alive(),
        }
    }
}

impl WaylandFocus for KeyFocus {
    fn wl_surface(&self) -> Option<Cow<'_, WlSurface>> {
        match self {
            Self::Wayland(surface) => Some(Cow::Borrowed(surface)),
            Self::X11(surface) => surface.wl_surface().map(Cow::Owned),
        }
    }
}

impl KeyboardTarget<Nekoland> for KeyFocus {
    fn enter(&self, seat: &Seat<Nekoland>, data: &mut Nekoland, keys: Vec<KeysymHandle<'_>>, serial: Serial) {
        match self {
            Self::Wayland(surface) => KeyboardTarget::enter(surface, seat, data, keys, serial),
            Self::X11(surface) => KeyboardTarget::enter(surface, seat, data, keys, serial),
        }
    }

    fn leave(&self, seat: &Seat<Nekoland>, data: &mut Nekoland, serial: Serial) {
        match self {
            Self::Wayland(surface) => KeyboardTarget::leave(surface, seat, data, serial),
            Self::X11(surface) => KeyboardTarget::leave(surface, seat, data, serial),
        }
    }

    fn key(
        &self,
        seat: &Seat<Nekoland>,
        data: &mut Nekoland,
        key: KeysymHandle<'_>,
        state: KeyState,
        serial: Serial,
        time: u32,
    ) {
        match self {
            Self::Wayland(surface) => KeyboardTarget::key(surface, seat, data, key, state, serial, time),
            Self::X11(surface) => KeyboardTarget::key(surface, seat, data, key, state, serial, time),
        }
    }

    fn modifiers(
        &self,
        seat: &Seat<Nekoland>,
        data: &mut Nekoland,
        modifiers: ModifiersState,
        serial: Serial,
    ) {
        match self {
            Self::Wayland(surface) => KeyboardTarget::modifiers(surface, seat, data, modifiers, serial),
            Self::X11(surface) => KeyboardTarget::modifiers(surface, seat, data, modifiers, serial),
        }
    }
}
