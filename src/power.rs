//! The power button and the lid, which Nekoland handles instead of logind, as the
//! configuration says, and sleeping, from them, the menu and a time without input.

use smithay::backend::input::{Switch, SwitchState, SwitchToggleEvent};
use zbus::zvariant::OwnedFd;

use crate::{
    config::{self, PowerAction},
    state::Nekoland,
};

/// How far the picture turns for the way up iio-sensor-proxy says the computer is held.
fn orientation_degrees(orientation: &str) -> u16 {
    match orientation {
        "left-up" => 270,
        "right-up" => 90,
        "bottom-up" => 180,
        _ => 0,
    }
}

impl Nekoland {
    /// Locked as it wakes: it sleeps once the locker has the screen, which the lock handler
    /// tells. From an xrdp session the computer is elsewhere, and asleep would end the
    /// session: it locks only.
    pub fn sleep(&mut self) {
        if self.remote {
            return self.lock_screen();
        }
        if self.lock.as_ref().is_some_and(|lock| lock.held()) {
            return config::spawn("systemctl suspend");
        }
        self.sleep_asked = Some(std::time::Instant::now());
        self.lock_screen();
    }

    pub fn power_action(&mut self, action: PowerAction) {
        match action {
            PowerAction::Sleep => self.sleep(),
            PowerAction::ShutDown => config::spawn("nekoland-confirm shutdown"),
            PowerAction::Lock => self.lock_screen(),
            PowerAction::Ignore => {}
        }
    }

    /// Turns the computer's own display as it is held, unless the rotation is locked, as
    /// Windows' Super+O does; an external one stays as it is set.
    pub fn orientation_changed(&mut self, orientation: String) {
        self.orientation = Some(orientation);
        self.turn_with_orientation();
    }

    /// Puts the computer's own display the way up it is held, in the configuration read
    /// but not in its file, which keeps the rotation set by hand.
    pub fn turn_with_orientation(&mut self) {
        // Locked, the display stays the way it was turned, through the configuration read again.
        if !self.config.rotation_lock {
            self.turned = self.orientation.as_deref().map(orientation_degrees);
        }
        let Some(degrees) = self.turned else {
            return;
        };
        let Ok(rotation) = crate::config::Rotation::try_from(degrees) else {
            return;
        };
        let built_in: Vec<String> =
            self.space.outputs().map(|output| output.name()).filter(|name| is_built_in(name)).collect();
        for name in built_in {
            self.config.display_rotations.insert(name, rotation);
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
pub(crate) fn is_built_in(name: &str) -> bool {
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

#[cfg(test)]
mod orientation_tests {
    use super::orientation_degrees;

    #[test]
    fn turns_against_the_way_up() {
        assert_eq!(orientation_degrees("normal"), 0);
        assert_eq!(orientation_degrees("left-up"), 270);
        assert_eq!(orientation_degrees("right-up"), 90);
        assert_eq!(orientation_degrees("bottom-up"), 180);
        assert_eq!(orientation_degrees("undefined"), 0);
    }
}
