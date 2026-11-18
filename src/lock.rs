//! The screen lock. A locker, nekoland-lock unless the configuration names another, asks for it
//! through ext-session-lock; while it holds it, only its surfaces are drawn and take input,
//! and only it lets go. Nekoland starts the locker from the menu, Super+L,
//! `nekoland msg lock`, before sleeping, and after `lock_after` seconds without input,
//! unless a program such as a video player keeps the screen awake.

use std::time::{Duration, Instant};

use smithay::{
    delegate_idle_inhibit, delegate_session_lock,
    desktop::{Window, WindowSurfaceType},
    input::{
        keyboard::{FilterResult, Keysym, ModifiersState},
        pointer::MotionEvent,
    },
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

use crate::{
    config,
    focus::KeyFocus,
    text::named_icon,
    windows::{key_focus_of, window_names},
    CalloopData, Nekoland,
};

/// How often the time without input is looked at.
const IDLE_CHECK: Duration = Duration::from_secs(5);
/// How long a locker is given to lock the screen before another may be started.
const LOCKER_START: Duration = Duration::from_secs(3);

pub struct Lock {
    /// The locker's hold on the session.
    lock: ExtSessionLockV1,
    /// The locker's surfaces, each with the output it covers.
    pub surfaces: Vec<(Output, LockSurface)>,
    /// The lock's shortcut opened without the password: its windows, and only they, stand
    /// above the lock until they close.
    guest: Option<String>,
    /// Whether it has shown a window yet: once its last one closes, it is let go.
    guest_shown: bool,
    /// When it locked, which the displays' going dark while locked counts from.
    since: Instant,
}

impl Lock {
    /// False once the locker has gone without unlocking: the screen stays locked, and
    /// another locker may take over.
    fn held(&self) -> bool {
        self.lock.is_alive()
    }
}

/// Where the notifications that came in while locked are kept for nekoland-lock to show, one
/// to a line: the program, a tab, what it says. Only in the user's own runtime directory: a
/// shared one such as /tmp would let others read them or plant a link in their place.
fn locker_notices() -> Option<std::path::PathBuf> {
    Some(std::path::PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?).join("nekoland-lock-notices"))
}

pub fn tell_locker(app: &str, said: &str) {
    use std::io::Write;
    let plain = |words: &str| words.replace(['\t', '\n', '\r'], " ");
    let Some(path) = locker_notices() else { return };
    let line = format!("{}\t{}\n", plain(app), plain(said));
    let written = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut file| file.write_all(line.as_bytes()));
    if let Err(err) = written {
        tracing::warn!("{}: {err}", path.display());
    }
}

impl Nekoland {
    /// Starts the locker, unless one holds the screen already. swaylock is given the
    /// wallpaper, as the desktop has it; nekoland-lock reads it itself.
    pub fn lock_screen(&mut self) {
        // One started a moment ago is still coming up.
        let starting = self.locker_started.is_some_and(|at| at.elapsed() < LOCKER_START);
        if starting || self.lock.as_ref().is_some_and(Lock::held) {
            return;
        }
        self.locker_started = Some(Instant::now());
        self.draw_lock_shortcuts();
        if let Some(path) = locker_notices() {
            if let Err(err) = std::fs::remove_file(&path) {
                if err.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!("{}: {err}", path.display());
                }
            }
        }
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

    /// The windows of the shortcut opened over the lock, back to front.
    pub fn lock_guests(&self) -> Vec<Window> {
        let Some(guest) = self.lock.as_ref().and_then(|lock| lock.guest.as_deref()) else {
            return Vec::new();
        };
        self.space
            .elements()
            .filter(|window| window_names(window).1.eq_ignore_ascii_case(guest))
            .cloned()
            .collect()
    }

    /// `nekoland msg lock-open APP`, from the lock's shortcuts: only an app set as one opens.
    /// Locked, it opens over the lock if the shortcuts need no password; unlocked, as the
    /// lock let go with the password given, it simply opens.
    pub fn open_from_lock(&mut self, app_id: &str) {
        if !self.config.lock_shortcuts.iter().any(|shortcut| shortcut == app_id) {
            return tracing::warn!("lock-open: {app_id} is not a lock shortcut");
        }
        if let Some(lock) = self.lock.as_mut() {
            if !self.config.lock_shortcuts_unlocked {
                return;
            }
            lock.guest = Some(app_id.to_string());
            lock.guest_shown = false;
        }
        self.launch_app(app_id);
    }

    /// The shortcuts' icons, for nekoland-lock, which has no icon themes of its own to find
    /// them in: `nekoland-lock-shortcut-0.png` and `-1.png` in the runtime directory.
    pub fn draw_lock_shortcuts(&mut self) {
        let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from) else {
            return;
        };
        // As many as nekoland-lock shows in a row.
        for at in 0..6 {
            let file = runtime.join(format!("nekoland-lock-shortcut-{at}.png"));
            let icon = self
                .config
                .lock_shortcuts
                .get(at)
                .cloned()
                .and_then(|app_id| self.apps.info(&app_id).icon_name.clone())
                .and_then(|name| named_icon(&name, "", 64, (1.0, 1.0, 1.0)));
            let written = match icon {
                Some(icon) => std::fs::File::create(&file)
                    .map_err(|err| err.to_string())
                    .and_then(|mut out| icon.write_to_png(&mut out).map_err(|err| err.to_string())),
                None => match std::fs::remove_file(&file) {
                    Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err.to_string()),
                    _ => Ok(()),
                },
            };
            if let Err(err) = written {
                tracing::warn!("{}: {err}", file.display());
            }
        }
    }

    /// While locked, what is under the pointer: the shortcut opened over the lock, else the
    /// locker's surface on that output, if any.
    pub fn lock_surface_under(&self, pos: Point<f64, Logical>) -> Option<(WlSurface, Point<f64, Logical>)> {
        for window in self.lock_guests().iter().rev() {
            // Where its surfaces are, as Space::element_under has it: its shadow is outside it.
            let location = self.space.element_location(window)? - window.geometry().loc;
            if let Some((surface, at)) = window.surface_under(pos - location.to_f64(), WindowSurfaceType::ALL) {
                return Some((surface, (at + location).to_f64()));
            }
        }
        let output = self.space.output_under(pos).next()?;
        let origin = self.space.output_geometry(output)?.loc.to_f64();
        Some((self.lock_surface(output)?.wl_surface().clone(), origin))
    }

    /// Keeps the keyboard on the locker, or on the shortcut opened over it.
    pub fn focus_lock(&mut self) {
        let guests = self.lock_guests();
        if let Some(lock) = self.lock.as_mut() {
            if !guests.is_empty() {
                lock.guest_shown = true;
            } else if lock.guest_shown {
                lock.guest = None;
            }
        }
        let guest = guests.last().map(key_focus_of);
        let focus = guest.or_else(|| self.lock.as_ref().and_then(|lock| {
            lock.surfaces
                .iter()
                .find(|(_, surface)| surface.alive())
                .map(|(_, surface)| KeyFocus::Wayland(surface.wl_surface().clone()))
        }));
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
            // The Super key puts a shortcut opened over the lock back behind it, open, for
            // after the lock.
            if matches!(sym, Keysym::Super_L | Keysym::Super_R)
                && self.lock.as_mut().and_then(|lock| lock.guest.take()).is_some()
            {
                self.focus_lock();
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
        self.lock = Some(Lock { lock, surfaces, guest: None, guest_shown: false, since: Instant::now() });
        self.focus_lock();
        // The pointer leaves what it was over as well, so that a click or scroll without
        // moving it first reaches nothing behind the lock.
        if let Some(pointer) = self.seat.get_pointer() {
            if pointer.is_grabbed() {
                pointer.unset_grab(self, SERIAL_COUNTER.next_serial(), 0);
            }
            let location = pointer.current_location();
            let under = self.surface_under(location);
            let motion = MotionEvent {
                location,
                serial: SERIAL_COUNTER.next_serial(),
                time: 0,
            };
            pointer.motion(self, under, &motion);
            pointer.frame(self);
        }
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
        // While locked, after as long again since it locked or was last touched, as a phone's
        // lock screen goes dark sooner than the phone does.
        let locked_idle = state.lock.as_ref().map(|lock| idle.min(lock.since.elapsed()));
        let off_while_locked = state.config.lock_screen_off_after != 0
            && locked_idle.is_some_and(|idle| idle >= Duration::from_secs(state.config.lock_screen_off_after));
        // The locker shows the time dimly instead, as an always-on display.
        let always_on = state.lock.is_some() && state.config.lock_always_on;
        if (past(state.config.screen_off_after) || off_while_locked) && !always_on {
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
