//! The menu bar's dots while something records, as on the Mac: orange while a microphone is
//! open, green while a camera is; while the screen is shared, a purple button whose menu
//! stops it.

use std::{
    path::Path,
    time::{Duration, Instant},
};

use smithay::{
    reexports::calloop::channel::Sender,
    utils::{Logical, Rectangle},
};

use crate::{dbus::Event, state::Nekoland};

const POLL: Duration = Duration::from_secs(2);
/// How long after its last frame the screen still counts as shared: a recorder asks for
/// frames one after another.
const SHARING_LINGER: Duration = Duration::from_secs(1);
const DOT: f64 = 6.0;
/// Each recorder's colour, as the Mac's.
pub const SCREEN: (f64, f64, f64) = (0.69, 0.32, 0.87);
pub const CAMERA: (f64, f64, f64) = (0.20, 0.78, 0.35);
pub const MICROPHONE: (f64, f64, f64) = (1.0, 0.58, 0.0);
const DOT_GAP: i32 = 4;
/// The sharing button, and how much darker it is while its menu is open.
const SHARING_WIDTH: i32 = 30;
const SHARING_HEIGHT: f64 = 22.0;
const SHARING_GAP: i32 = 4;
const SHARING_OPEN_SHADE: f64 = 0.85;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct InUse {
    pub microphone: bool,
    pub camera: bool,
    pub screen: bool,
}

/// Watches on a thread which recording devices any of the user's processes, PipeWire's
/// among them, holds open, and says when that changes.
pub fn watch_devices(events: Sender<Event>) {
    let spawned = std::thread::Builder::new()
        .name("nekoland-privacy".into())
        .spawn(move || {
            let mut last = InUse::default();
            loop {
                let now = devices_in_use();
                if now != last {
                    if events.send(Event::InUse(now)).is_err() {
                        return;
                    }
                    last = now;
                }
                std::thread::sleep(POLL);
            }
        });
    if let Err(err) = spawned {
        tracing::warn!("no recording dots: {err}");
    }
}

/// shortcut: a Bluetooth microphone has no device file and is not seen; ask PipeWire for
/// its running sources if that matters.
fn devices_in_use() -> InUse {
    let mut in_use = InUse::default();
    let Ok(processes) = std::fs::read_dir("/proc") else {
        return in_use;
    };
    // Other users' processes cannot be looked into, and are left out.
    let files = processes
        .flatten()
        .filter_map(|process| std::fs::read_dir(process.path().join("fd")).ok())
        .flat_map(|files| files.flatten())
        .filter_map(|file| std::fs::read_link(file.path()).ok());
    for target in files {
        mark(&mut in_use, &target);
        if in_use.microphone && in_use.camera {
            break;
        }
    }
    in_use
}

/// Counts an open file if it is a camera, or an ALSA device that records: its name ends
/// in `c`, for capture, as `pcmC0D0c` does.
fn mark(in_use: &mut InUse, target: &Path) {
    let Some(name) = target.to_str() else {
        return;
    };
    if name.starts_with("/dev/video") {
        in_use.camera = true;
    } else if name.strip_prefix("/dev/snd/pcm").is_some_and(|pcm| pcm.ends_with('c')) {
        in_use.microphone = true;
    }
}

impl Nekoland {
    /// Notes that a recorder took a frame of the screen; a screenshot asks for no damage.
    pub fn note_screen_frame(&mut self) {
        self.screen_shared_at = Some(Instant::now());
    }

    /// Lets the menu bar know when the screen starts or stops being shared.
    pub fn refresh_sharing(&mut self) {
        let sharing = self.screen_shared_at.is_some_and(|at| at.elapsed() < SHARING_LINGER);
        if sharing != self.menu_bar.in_use.screen {
            self.menu_bar.in_use.screen = sharing;
            self.menu_bar.items_revision += 1;
        }
    }
}

/// Draws the dots of what is in use, leftwards from `right`, and returns where they end.
pub fn draw_dots(cr: &cairo::Context, in_use: InUse, mut right: i32, center: f64) -> i32 {
    let dots = [(in_use.camera, CAMERA), (in_use.microphone, MICROPHONE)];
    for (_, (r, g, b)) in dots.iter().filter(|(on, _)| *on) {
        cr.set_source_rgb(*r, *g, *b);
        cr.arc(f64::from(right) - DOT / 2.0, center, DOT / 2.0, 0.0, std::f64::consts::TAU);
        let _ = cr.fill();
        right -= DOT as i32 + DOT_GAP;
    }
    right
}

/// Where the sharing button goes, leftwards from `right`.
pub fn sharing_button(right: i32) -> Rectangle<i32, Logical> {
    Rectangle::new(
        (right - SHARING_GAP - SHARING_WIDTH, 0).into(),
        (SHARING_WIDTH + SHARING_GAP, crate::menubar::MENU_BAR_HEIGHT).into(),
    )
}

/// The sharing button: a display, white, on the screen's purple.
pub fn draw_sharing(cr: &cairo::Context, rect: Rectangle<i32, Logical>, open: bool, center: f64) {
    let shade = if open { SHARING_OPEN_SHADE } else { 1.0 };
    let x = f64::from(rect.loc.x + SHARING_GAP);
    let width = f64::from(SHARING_WIDTH);
    crate::text::rounded_rectangle(cr, x, center - SHARING_HEIGHT / 2.0, width, SHARING_HEIGHT, 6.0);
    cr.set_source_rgb(SCREEN.0 * shade, SCREEN.1 * shade, SCREEN.2 * shade);
    let _ = cr.fill();
    // A display 13 by 9 points on a stand, as the bar's other icons are drawn.
    let (left, top) = (x + (width - 13.0) / 2.0, center - 6.0);
    cr.set_source_rgb(1.0, 1.0, 1.0);
    cr.set_line_width(1.5);
    crate::text::rounded_rectangle(cr, left, top, 13.0, 9.0, 1.5);
    let _ = cr.stroke();
    cr.move_to(left + 3.5, top + 11.75);
    cr.line_to(left + 9.5, top + 11.75);
    cr.set_line_cap(cairo::LineCap::Round);
    let _ = cr.stroke();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cameras_and_recording_sound_devices_count() {
        let marked = |path: &str| {
            let mut in_use = InUse::default();
            mark(&mut in_use, Path::new(path));
            (in_use.microphone, in_use.camera)
        };
        assert_eq!(marked("/dev/video0"), (false, true));
        assert_eq!(marked("/dev/snd/pcmC0D0c"), (true, false));
        assert_eq!(marked("/dev/snd/pcmC0D0p"), (false, false));
        assert_eq!(marked("/dev/snd/controlC0"), (false, false));
        assert_eq!(marked("socket:[1234]"), (false, false));
    }
}
