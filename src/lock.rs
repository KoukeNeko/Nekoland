//! The screen lock. A locker, swaylock unless the configuration names another, asks for it
//! through ext-session-lock; while it holds it, only its surfaces are drawn and take input,
//! and only it lets go. Nekoland starts the locker from the menu, Super+L,
//! `nekoland msg lock`, before sleeping, and after `lock_after` seconds without input,
//! unless a program such as a video player keeps the screen awake.

use std::time::{Duration, Instant};

use smithay::{
    delegate_idle_inhibit, delegate_session_lock,
    input::keyboard::{FilterResult, Keysym, ModifiersState},
    output::Output,
    reexports::{
        calloop::{
            timer::{TimeoutAction, Timer},
            LoopHandle,
        },
        wayland_protocols::ext::session_lock::v1::server::ext_session_lock_v1::ExtSessionLockV1,
        wayland_server::{
            protocol::{wl_output::WlOutput, wl_surface::WlSurface},
            Resource,
        },
    },
    utils::{Logical, Point, SERIAL_COUNTER},
    wayland::{
        idle_inhibit::IdleInhibitHandler,
        session_lock::{LockSurface, SessionLockHandler, SessionLockManagerState, SessionLocker},
    },
};

use crate::{config, focus::KeyFocus, CalloopData, Nekoland};

/// How often the time without input is looked at.
const IDLE_CHECK: Duration = Duration::from_secs(5);
/// How long a locker is given to lock the screen before another may be started.
const LOCKER_START: Duration = Duration::from_secs(3);

pub struct Lock {
    /// The locker's hold on the session.
    lock: ExtSessionLockV1,
    /// The locker's surfaces, each with the output it covers.
    pub surfaces: Vec<(Output, LockSurface)>,
}

impl Lock {
    /// False once the locker has gone without unlocking: the screen stays locked, and
    /// another locker may take over.
    fn held(&self) -> bool {
        self.lock.is_alive()
    }
}

impl Nekoland {
    /// Starts the locker, unless one holds the screen already. swaylock shows the
    /// wallpaper, as the desktop has it.
    pub fn lock_screen(&mut self) {
        // One started a moment ago is still coming up.
        let starting = self.locker_started.is_some_and(|at| at.elapsed() < LOCKER_START);
        if starting || self.lock.as_ref().is_some_and(Lock::held) {
            return;
        }
        self.locker_started = Some(Instant::now());
        let mut command = self.config.lock.clone();
        if let Some(wallpaper) = self.config.wallpaper.as_ref().filter(|_| command.starts_with("swaylock")) {
            let home = std::env::var("HOME").unwrap_or_default();
            let path = wallpaper.replacen('~', &home, 1);
            let mode = self.config.wallpaper_mode.name();
            command += &format!(" -i {} -s {mode}", config::quote(&path));
        }
        config::spawn(&command);
    }

    /// The locker's surface on an output.
    pub fn lock_surface(&self, output: &Output) -> Option<&LockSurface> {
        let lock = self.lock.as_ref()?;
        lock.surfaces
            .iter()
            .find(|(on, surface)| on == output && surface.alive())
            .map(|(_, surface)| surface)
    }

    /// While locked, what is under the pointer: the locker's surface on that output, if any.
    pub fn lock_surface_under(&self, pos: Point<f64, Logical>) -> Option<(WlSurface, Point<f64, Logical>)> {
        let output = self.space.output_under(pos).next()?;
        let origin = self.space.output_geometry(output)?.loc.to_f64();
        Some((self.lock_surface(output)?.wl_surface().clone(), origin))
    }

    /// Keeps the keyboard on the locker.
    pub fn focus_lock(&mut self) {
        let focus = self.lock.as_ref().and_then(|lock| {
            lock.surfaces
                .iter()
                .find(|(_, surface)| surface.alive())
                .map(|(_, surface)| KeyFocus::Wayland(surface.wl_surface().clone()))
        });
        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
        };
        // An input method holding the keyboard would take the password, or compose it into
        // something else: it is let go, and the locker gets the keys as typed.
        if keyboard.is_grabbed() {
            keyboard.unset_grab(self);
        }
        if keyboard.current_focus() != focus {
            keyboard.set_focus(self, focus, SERIAL_COUNTER.next_serial());
        }
    }

    /// A key while locked goes to the locker, but for switching the virtual terminal and
    /// quitting, which leave the session behind the lock. Any key brings back a locker that
    /// went away.
    pub fn locked_key(&mut self, modifiers: &ModifiersState, sym: Keysym, pressed: bool) -> FilterResult<()> {
        if pressed {
            if self.vt_key(sym) {
                return FilterResult::Intercept(());
            }
            if sym == Keysym::BackSpace && modifiers.ctrl && modifiers.alt {
                self.loop_signal.stop();
                return FilterResult::Intercept(());
            }
            self.lock_screen();
        }
        self.focus_lock();
        FilterResult::Forward
    }
}

impl SessionLockHandler for Nekoland {
    fn lock_state(&mut self) -> &mut SessionLockManagerState {
        &mut self.protocols.session_lock
    }

    fn lock(&mut self, confirmation: SessionLocker) {
        // A second locker is refused: dropping its confirmation tells it so.
        if self.lock.as_ref().is_some_and(Lock::held) {
            return;
        }
        let lock = confirmation.ext_session_lock().clone();
        let surfaces = self.lock.take().map(|lock| lock.surfaces).unwrap_or_default();
        self.lock = Some(Lock { lock, surfaces });
        self.focus_lock();
        // shortcut: confirmed before the next frame, the first without windows, is shown;
        // wait for that frame's presentation if a locker relies on it.
        confirmation.lock();
    }

    fn unlock(&mut self) {
        self.lock = None;
        match self.space.elements().last().cloned() {
            Some(window) => self.focus_window(&window),
            None => self.set_keyboard_focus(None),
        }
    }

    fn new_surface(&mut self, surface: LockSurface, output: WlOutput) {
        let Some(output) = Output::from_resource(&output) else {
            return;
        };
        let Some(geometry) = self.space.output_geometry(&output) else {
            return;
        };
        let Some(lock) = self.lock.as_mut() else {
            return;
        };
        surface.with_pending_state(|state| {
            state.size = Some((geometry.size.w as u32, geometry.size.h as u32).into());
        });
        surface.send_configure();
        lock.surfaces.retain(|(on, old)| on != &output && old.alive());
        lock.surfaces.push((output, surface));
        self.focus_lock();
    }
}
delegate_session_lock!(Nekoland);

impl IdleInhibitHandler for Nekoland {
    fn inhibit(&mut self, surface: WlSurface) {
        self.idle_inhibitors.push(surface);
    }

    fn uninhibit(&mut self, surface: WlSurface) {
        self.idle_inhibitors.retain(|inhibitor| inhibitor != &surface);
    }
}
delegate_idle_inhibit!(Nekoland);

/// Once there has been no input for as long as the configuration says, locks the screen,
/// turns the displays off and sleeps, unless a program keeps the screen awake.
pub fn watch_idle(handle: &LoopHandle<'static, CalloopData>) {
    // The input each was last done after: each is done once until there is more, so that a
    // locker that does not start is not started every few seconds.
    let (mut locked_after, mut slept_after) = (None, None);
    let inserted = handle.insert_source(Timer::from_duration(IDLE_CHECK), move |_, _, data| {
        let state = &mut data.state;
        state.idle_inhibitors.retain(Resource::is_alive);
        // shortcut: a program keeps the screen awake while it lives, seen or not.
        if !state.idle_inhibitors.is_empty() {
            return TimeoutAction::ToDuration(IDLE_CHECK);
        }
        let idle = state.last_input.elapsed();
        let past = |seconds: u64| seconds != 0 && idle >= Duration::from_secs(seconds);
        if past(state.config.lock_after) && state.lock.is_none() && locked_after != Some(state.last_input) {
            locked_after = Some(state.last_input);
            state.lock_screen();
        }
        if past(state.config.screen_off_after) {
            state.screens_off = true;
        }
        if past(state.config.sleep_after) && slept_after != Some(state.last_input) {
            slept_after = Some(state.last_input);
            state.sleep();
        }
        TimeoutAction::ToDuration(IDLE_CHECK)
    });
    if let Err(err) = inserted {
        tracing::warn!("idle: {err}");
    }
}
