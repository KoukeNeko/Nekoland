//! The power button and the lid, which Nekoland handles instead of logind, as the
//! configuration says, and sleeping, from them, the menu and a time without input.

use smithay::backend::input::{Switch, SwitchState, SwitchToggleEvent};
use zbus::zvariant::OwnedFd;

use crate::{
    config::{self, PowerAction},
    state::Nekoland,
};

impl Nekoland {
    /// Locked as it wakes: the locker is given a moment to take the screen first.
    pub fn sleep(&mut self) {
        self.lock_screen();
        config::spawn("sleep 1; systemctl suspend");
    }

    pub fn power_action(&mut self, action: PowerAction) {
        match action {
            PowerAction::Sleep => self.sleep(),
            PowerAction::ShutDown => config::spawn("nekoland-confirm shutdown"),
            PowerAction::Lock => self.lock_screen(),
            PowerAction::Ignore => {}
        }
    }

    /// Closing the lid does nothing while another display is connected, as logind's
    /// docked laptop does, and the Mac's.
    pub fn lid_switched<B: smithay::backend::input::InputBackend>(&mut self, event: &impl SwitchToggleEvent<B>) {
        if event.switch() != Some(Switch::Lid) || event.state() != SwitchState::On {
            return;
        }
        // shortcut: the laptop's own panel stays lit with the lid closed; turn it off once
        // displays can be turned off one by one.
        let docked = self.space.outputs().any(|output| !is_built_in(&output.name()));
        if !docked {
            self.power_action(self.config.lid_close);
        }
    }
}

/// Whether a display is a laptop's own panel, by its connector.
fn is_built_in(name: &str) -> bool {
    ["eDP", "LVDS", "DSI"].iter().any(|kind| name.starts_with(kind))
}

/// Keeps logind from acting on the power button and the lid for as long as the returned
/// file is open.
pub fn take_power_keys() -> zbus::Result<OwnedFd> {
    let bus = zbus::blocking::Connection::system()?;
    let reply = bus.call_method(
        Some("org.freedesktop.login1"),
        "/org/freedesktop/login1",
        Some("org.freedesktop.login1.Manager"),
        "Inhibit",
        &(
            "handle-power-key:handle-lid-switch",
            "Nekoland",
            "Nekoland handles them as its settings say",
            "block",
        ),
    )?;
    reply.body().deserialize()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_laptops_own_panel_is_built_in() {
        assert!(is_built_in("eDP-1"));
        assert!(is_built_in("LVDS-1"));
        assert!(!is_built_in("HDMI-A-1"));
        assert!(!is_built_in("DP-2"));
        assert!(!is_built_in("Virtual-1"));
    }
}
