//! Nekoland's screen lock. It locks the session through `ext-session-lock`, covers each
//! display with the wallpaper under ink, the time and a field for the password, checks the
//! password with PAM, and, once it is right, lifts the ink off and unlocks.
//!
//! It reads `~/.config/nekoland/config.toml` for `wallpaper` (or swaybg's in `autostart`),
//! `lock_wallpaper` (whether the
//! lock shows it; on unless set), `lock_blur` (on unless set), `font` and `clock_24h`.

mod draw;
mod pam;

use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use cairo::{Context, Format, ImageSurface};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_keyboard, delegate_output, delegate_registry, delegate_seat,
    delegate_session_lock, delegate_shm,
    output::{OutputHandler, OutputState},
    reexports::{
        calloop::{
            channel,
            timer::{TimeoutAction, Timer},
            EventLoop, LoopHandle,
        },
        calloop_wayland_source::WaylandSource,
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers},
        Capability, SeatHandler, SeatState,
    },
    session_lock::{
        SessionLock, SessionLockHandler, SessionLockState, SessionLockSurface, SessionLockSurfaceConfigure,
    },
    shm::{slot::SlotPool, Shm, ShmHandler},
};
use wayland_client::{
    globals::registry_queue_init,
    protocol::{wl_keyboard, wl_output, wl_seat, wl_shm, wl_surface},
    Connection, QueueHandle,
};

use draw::{Look, Moment};

/// How long the wrong password's shake lasts, and the ink takes to lift.
const SHAKE: Duration = Duration::from_millis(360);
const LIFT: Duration = Duration::from_millis(560);
/// How often the screens are drawn again while something moves.
const FRAME: Duration = Duration::from_millis(16);

/// A display's cover.
struct Screen {
    output: wl_output::WlOutput,
    surface: SessionLockSurface,
    /// Its size in points, and pixels to a point.
    size: Option<(u32, u32)>,
    scale: i32,
    /// The backdrop, ink and all, made once for the size: the blurred wallpaper while
    /// locked, the sharp one for lifting the ink off.
    locked: Option<ImageSurface>,
    sharp: Option<ImageSurface>,
}

struct Locker {
    conn: Connection,
    handle: LoopHandle<'static, Locker>,
    registry: RegistryState,
    compositor: CompositorState,
    outputs: OutputState,
    seats: SeatState,
    shm: Shm,
    pool: SlotPool,
    lock_state: SessionLockState,
    lock: Option<SessionLock>,
    screens: Vec<Screen>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    look: Look,
    user: String,
    password: String,
    caps_lock: bool,
    /// How many characters the password being checked has.
    checking: Option<usize>,
    /// The last password's failure, and when it was told.
    error: Option<(&'static str, Instant)>,
    /// When the right password was given; the ink lifts from then.
    opened: Option<Instant>,
    /// Whether a timer is drawing frames.
    moving: bool,
    exit: bool,
}

fn main() {
    let conn = Connection::connect_to_env().unwrap_or_else(|err| fail(&format!("no Wayland display: {err}")));
    let (globals, queue) = registry_queue_init(&conn).unwrap_or_else(|err| fail(&err.to_string()));
    let qh: QueueHandle<Locker> = queue.handle();
    let mut event_loop: EventLoop<Locker> = EventLoop::try_new().unwrap_or_else(|err| fail(&err.to_string()));
    let shm = Shm::bind(&globals, &qh).unwrap_or_else(|err| fail(&err.to_string()));
    let pool = SlotPool::new(4 * 1920 * 1080, &shm).unwrap_or_else(|err| fail(&err.to_string()));
    let (user, look) = read_look();
    let mut locker = Locker {
        conn: conn.clone(),
        handle: event_loop.handle(),
        registry: RegistryState::new(&globals),
        compositor: CompositorState::bind(&globals, &qh).unwrap_or_else(|err| fail(&err.to_string())),
        outputs: OutputState::new(&globals, &qh),
        seats: SeatState::new(&globals, &qh),
        shm,
        pool,
        lock_state: SessionLockState::new(&globals, &qh),
        lock: None,
        screens: Vec::new(),
        keyboard: None,
        look,
        user,
        password: String::new(),
        caps_lock: false,
        checking: None,
        error: None,
        opened: None,
        moving: false,
        exit: false,
    };
    locker.lock = Some(
        locker
            .lock_state
            .lock(&qh)
            .unwrap_or_else(|_| fail("the compositor cannot lock the screen (no ext-session-lock)")),
    );
    WaylandSource::new(conn, queue)
        .insert(event_loop.handle())
        .unwrap_or_else(|err| fail(&err.to_string()));
    // The time, drawn again as its minute changes.
    let mut shown = clock(locker.look.clock_24h);
    event_loop
        .handle()
        .insert_source(
            Timer::from_duration(Duration::from_secs(1)),
            move |_, _, locker| {
                let now = clock(locker.look.clock_24h);
                if now != shown {
                    shown = now;
                    locker.redraw();
                }
                TimeoutAction::ToDuration(Duration::from_secs(1))
            },
        )
        .unwrap_or_else(|err| fail(&err.to_string()));
    while !locker.exit {
        if let Err(err) = event_loop.dispatch(None, &mut locker) {
            fail(&err.to_string());
        }
    }
}

fn fail(why: &str) -> ! {
    eprintln!("nekoland-lock: {why}");
    std::process::exit(1)
}

/// The user's name to show, and how the configuration says the lock looks.
fn read_look() -> (String, Look) {
    let home = std::env::var("HOME").unwrap_or_default();
    let dir = std::env::var_os("XDG_CONFIG_HOME")
        .map_or_else(|| PathBuf::from(&home).join(".config"), PathBuf::from);
    let path = dir.join("nekoland/config.toml");
    let config: toml::Table = match std::fs::read_to_string(&path) {
        Ok(text) => text.parse().unwrap_or_else(|err| {
            eprintln!("nekoland-lock: {}: {err}", path.display());
            toml::Table::new()
        }),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => toml::Table::new(),
        Err(err) => {
            eprintln!("nekoland-lock: {}: {err}", path.display());
            toml::Table::new()
        }
    };
    let flag = |key: &str| config.get(key).and_then(toml::Value::as_bool).unwrap_or(true);
    // Without a wallpaper of Nekoland's own, the one swaybg is started with at login.
    let started = config
        .get("autostart")
        .and_then(toml::Value::as_array)
        .and_then(|commands| {
            commands
                .iter()
                .filter_map(toml::Value::as_str)
                .find_map(|command| {
                    let mut words = command.strip_prefix("swaybg ")?.split_whitespace();
                    words.find(|word| *word == "-i")?;
                    words.next()
                })
        });
    let wallpaper = config
        .get("wallpaper")
        .and_then(toml::Value::as_str)
        .or(started)
        .filter(|_| flag("lock_wallpaper"))
        .and_then(|path| {
            let path = path.replacen('~', &home, 1);
            gdk_pixbuf::Pixbuf::from_file(&path)
                .map_err(|err| eprintln!("nekoland-lock: {path}: {err}"))
                .ok()
        });
    let (user, name) = account();
    let look = Look {
        wallpaper,
        blur: flag("lock_blur"),
        font: config
            .get("font")
            .and_then(toml::Value::as_str)
            .unwrap_or("Ubuntu Sans")
            .to_string(),
        clock_24h: flag("clock_24h"),
        name,
    };
    (user, look)
}

/// The user's login, and the name to show: the full one, where the account has it.
fn account() -> (String, String) {
    // SAFETY: getpwuid's record is read before anything else could call it again.
    unsafe {
        let entry = libc::getpwuid(libc::getuid());
        if entry.is_null() {
            fail("this user has no account entry");
        }
        let login = std::ffi::CStr::from_ptr((*entry).pw_name)
            .to_string_lossy()
            .into_owned();
        let gecos = if (*entry).pw_gecos.is_null() {
            String::new()
        } else {
            std::ffi::CStr::from_ptr((*entry).pw_gecos)
                .to_string_lossy()
                .into_owned()
        };
        let full = gecos.split(',').next().unwrap_or_default().trim().to_string();
        let name = if full.is_empty() { login.clone() } else { full };
        (login, name)
    }
}

/// The local time, `14:23` or `2:23`, and the date, `11月17日 星期二`.
fn now() -> libc::tm {
    // SAFETY: localtime_r fills the tm it is given.
    unsafe {
        let seconds = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&seconds, &mut tm);
        tm
    }
}

fn clock(hours_24: bool) -> String {
    let tm = now();
    let hour = if hours_24 {
        tm.tm_hour
    } else {
        (tm.tm_hour + 11) % 12 + 1
    };
    format!("{hour}:{:02}", tm.tm_min)
}

fn date() -> String {
    const WEEKDAYS: [&str; 7] = ["日", "一", "二", "三", "四", "五", "六"];
    let tm = now();
    format!(
        "{}月{}日 星期{}",
        tm.tm_mon + 1,
        tm.tm_mday,
        WEEKDAYS[tm.tm_wday as usize % 7]
    )
}

impl Locker {
    fn cover(&mut self, qh: &QueueHandle<Self>, output: wl_output::WlOutput) {
        let Some(lock) = &self.lock else { return };
        let surface = lock.create_lock_surface(self.compositor.create_surface(qh), &output, qh);
        let scale = self
            .outputs
            .info(&output)
            .map_or(1, |info| info.scale_factor.max(1));
        surface.wl_surface().set_buffer_scale(scale);
        self.screens.push(Screen {
            output,
            surface,
            size: None,
            scale,
            locked: None,
            sharp: None,
        });
    }

    fn redraw(&mut self) {
        for i in 0..self.screens.len() {
            self.draw(i);
        }
    }

    fn draw(&mut self, i: usize) {
        let (time, date) = (clock(self.look.clock_24h), date());
        let error = self
            .error
            .map(|(why, at)| (why, at.elapsed().as_secs_f64() / SHAKE.as_secs_f64()));
        let moment = Moment {
            time: &time,
            date: &date,
            typed: self.checking.unwrap_or_else(|| self.password.chars().count()),
            checking: self.checking.is_some(),
            caps_lock: self.caps_lock,
            error,
        };
        let lifted = self
            .opened
            .map(|at| (at.elapsed().as_secs_f64() / LIFT.as_secs_f64()).min(1.0));
        let screen = &mut self.screens[i];
        let Some((width, height)) = screen.size else {
            return;
        };
        let scale = screen.scale;
        let (pw, ph) = (width as i32 * scale, height as i32 * scale);
        if screen.locked.is_none() {
            screen.locked = draw::backdrop(&self.look, pw, ph, true, self.look.blur).inspect(|surface| {
                if let Ok(cr) = Context::new(surface) {
                    cr.scale(f64::from(scale), f64::from(scale));
                    draw::veil(&cr, f64::from(width), f64::from(height));
                }
            });
        }
        if lifted.is_some() && screen.sharp.is_none() {
            screen.sharp = draw::backdrop(&self.look, pw, ph, false, false);
        }
        let stride = Format::ARgb32.stride_for_width(pw as u32).unwrap_or(pw * 4);
        let (buffer, canvas) = match self.pool.create_buffer(pw, ph, stride, wl_shm::Format::Argb8888) {
            Ok(made) => made,
            Err(err) => return eprintln!("nekoland-lock: no buffer: {err}"),
        };
        // SAFETY: the surface draws into the buffer's memory and is finished before the
        // buffer is handed to the compositor, while `canvas` is still borrowed.
        let drawn = unsafe {
            ImageSurface::create_for_data_unsafe(canvas.as_mut_ptr(), Format::ARgb32, pw, ph, stride)
        };
        let Ok(target) = drawn else {
            return eprintln!("nekoland-lock: cannot draw");
        };
        if let (Ok(cr), Some(locked)) = (Context::new(&target), &screen.locked) {
            let words = |cr: &Context| {
                cr.save().ok();
                cr.scale(f64::from(scale), f64::from(scale));
                draw::words(cr, &self.look, &moment, f64::from(width), f64::from(height));
                cr.restore().ok();
            };
            match (lifted, &screen.sharp) {
                (Some(lifted), Some(sharp)) => {
                    // The ink and the words lift together, as one sheet.
                    if let Ok(layer) = ImageSurface::create(Format::ARgb32, pw, ph) {
                        if let Ok(lc) = Context::new(&layer) {
                            let _ = lc.set_source_surface(locked, 0.0, 0.0);
                            let _ = lc.paint();
                            words(&lc);
                        }
                        draw::lift(&cr, sharp, &layer, lifted, f64::from(scale));
                    }
                }
                _ => {
                    let _ = cr.set_source_surface(locked, 0.0, 0.0);
                    let _ = cr.paint();
                    words(&cr);
                }
            }
        }
        target.finish();
        let surface = screen.surface.wl_surface();
        if let Err(err) = buffer.attach_to(surface) {
            return eprintln!("nekoland-lock: {err:?}");
        }
        surface.damage_buffer(0, 0, pw, ph);
        surface.commit();
    }

    /// Draws a frame every few milliseconds until nothing moves any more.
    fn animate(&mut self) {
        if self.moving {
            return;
        }
        self.moving = true;
        let started = self
            .handle
            .insert_source(Timer::from_duration(FRAME), |_, _, locker| {
                locker.redraw();
                if locker.opened.is_some_and(|at| at.elapsed() >= LIFT) {
                    locker.unlock();
                    locker.moving = false;
                    return TimeoutAction::Drop;
                }
                let shaking = locker.error.is_some_and(|(_, at)| at.elapsed() < SHAKE);
                if locker.opened.is_none() && !shaking {
                    locker.moving = false;
                    return TimeoutAction::Drop;
                }
                TimeoutAction::ToDuration(FRAME)
            });
        if let Err(err) = started {
            eprintln!("nekoland-lock: {err}");
            self.moving = false;
        }
    }

    fn unlock(&mut self) {
        if let Some(lock) = self.lock.take() {
            lock.unlock();
            // The compositor must have the unlock before this goes, or the screen stays locked.
            if let Err(err) = self.conn.roundtrip() {
                eprintln!("nekoland-lock: {err}");
            }
        }
        self.exit = true;
    }

    fn submit(&mut self) {
        if self.password.is_empty() || self.checking.is_some() {
            return;
        }
        let (sender, receiver) = channel::channel();
        let (user, password) = (self.user.clone(), std::mem::take(&mut self.password));
        // Its dots stay, dimmed, while it is checked.
        self.checking = Some(password.chars().count());
        std::thread::spawn(move || {
            let _ = sender.send(pam::check(&user, &password));
        });
        let watched = self.handle.insert_source(receiver, |event, _, locker| {
            let channel::Event::Msg(result) = event else {
                return;
            };
            locker.checking = None;
            match result {
                Ok(()) => {
                    locker.error = None;
                    locker.opened = Some(Instant::now());
                }
                Err(pam::Failure::Wrong) => locker.error = Some(("密碼不正確", Instant::now())),
                Err(pam::Failure::Unable(why)) => {
                    eprintln!("nekoland-lock: {why}");
                    locker.error = Some(("無法驗證密碼", Instant::now()));
                }
            }
            locker.redraw();
            locker.animate();
        });
        if let Err(err) = watched {
            eprintln!("nekoland-lock: {err}");
        }
        self.redraw();
    }
}

impl SessionLockHandler for Locker {
    fn locked(&mut self, _: &Connection, qh: &QueueHandle<Self>, _: SessionLock) {
        for output in self.outputs.outputs() {
            self.cover(qh, output);
        }
    }

    fn finished(&mut self, _: &Connection, _: &QueueHandle<Self>, _: SessionLock) {
        eprintln!("nekoland-lock: the compositor would not lock the screen; another locker holds it");
        self.lock = None;
        self.exit = true;
    }

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        surface: SessionLockSurface,
        configure: SessionLockSurfaceConfigure,
        _: u32,
    ) {
        let Some(i) = self
            .screens
            .iter()
            .position(|screen| screen.surface.wl_surface() == surface.wl_surface())
        else {
            return;
        };
        let screen = &mut self.screens[i];
        if screen.size != Some(configure.new_size) {
            screen.size = Some(configure.new_size);
            screen.locked = None;
            screen.sharp = None;
        }
        self.draw(i);
    }
}

impl KeyboardHandler for Locker {
    fn enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: &wl_surface::WlSurface,
        _: u32,
        _: &[u32],
        _: &[Keysym],
    ) {
    }

    fn leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: &wl_surface::WlSurface,
        _: u32,
    ) {
    }

    fn press_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        if self.checking.is_some() || self.opened.is_some() {
            return;
        }
        match event.keysym {
            Keysym::Return | Keysym::KP_Enter => return self.submit(),
            Keysym::BackSpace => {
                self.password.pop();
            }
            Keysym::Escape => self.password.clear(),
            _ => match event.utf8 {
                Some(text) if !text.chars().any(char::is_control) => self.password.push_str(&text),
                _ => return,
            },
        }
        self.error = None;
        self.redraw();
    }

    fn release_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        _: KeyEvent,
    ) {
    }

    fn update_modifiers(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        modifiers: Modifiers,
        _: u32,
    ) {
        if self.caps_lock != modifiers.caps_lock {
            self.caps_lock = modifiers.caps_lock;
            self.redraw();
        }
    }
}

impl SeatHandler for Locker {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seats
    }

    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}

    fn new_capability(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard && self.keyboard.is_none() {
            match self.seats.get_keyboard(qh, &seat, None) {
                Ok(keyboard) => self.keyboard = Some(keyboard),
                Err(err) => eprintln!("nekoland-lock: no keyboard: {err}"),
            }
        }
    }

    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard {
            if let Some(keyboard) = self.keyboard.take() {
                keyboard.release();
            }
        }
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl OutputHandler for Locker {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.outputs
    }

    fn new_output(&mut self, _: &Connection, qh: &QueueHandle<Self>, output: wl_output::WlOutput) {
        if self.lock.as_ref().is_some_and(SessionLock::is_locked) {
            self.cover(qh, output);
        }
    }

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, output: wl_output::WlOutput) {
        self.screens.retain(|screen| screen.output != output);
    }
}

impl CompositorHandler for Locker {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        factor: i32,
    ) {
        if let Some(i) = self
            .screens
            .iter()
            .position(|screen| screen.surface.wl_surface() == surface)
        {
            let screen = &mut self.screens[i];
            screen.scale = factor.max(1);
            screen.surface.wl_surface().set_buffer_scale(screen.scale);
            screen.locked = None;
            screen.sharp = None;
            self.draw(i);
        }
    }

    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }

    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {}

    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl ShmHandler for Locker {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for Locker {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }
    registry_handlers![OutputState, SeatState];
}

delegate_compositor!(Locker);
delegate_output!(Locker);
delegate_seat!(Locker);
delegate_keyboard!(Locker);
delegate_session_lock!(Locker);
delegate_shm!(Locker);
delegate_registry!(Locker);
