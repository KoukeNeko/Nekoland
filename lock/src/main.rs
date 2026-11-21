//! Nekoland's screen lock. It locks the session through `ext-session-lock`, covers each
//! display with the wallpaper under ink, the time and a field for the password, checks the
//! password with PAM, and, once it is right, lifts the ink off and unlocks.
//!
//! It reads `~/.config/nekoland/config.toml` for `wallpaper` (or swaybg's in `autostart`),
//! `lock_wallpaper` (whether the lock shows it; on unless set), `lock_blur` (on unless set),
//! `font`, `clock_24h`, and how the lock is made one's own:
//! `lock_style` (`quiet`, `poster` or `dial`), `lock_clock_color` (`#rrggbb`, else the style's),
//! `lock_clock_weight` (`light`, `regular` or `bold`), `lock_widgets` (of `battery` and
//! `playing`), `lock_notices` (`hidden`, `count` or `full`), `lock_message`,
//! `lock_show_name`, `lock_power_buttons`, `lock_shortcuts` (desktop entry IDs, whose icons
//! the compositor draws into the runtime directory), `lock_shortcuts_unlocked` (whether
//! they open without the password, through `nekoland msg lock-open`) and
//! `lock_shortcuts_layout` (`corners`, two, or `row`, six). Further, after One UI and Good
//! Lock: `lock_clock_font`, `lock_clock_size` (`small`, `medium`, `large`),
//! `lock_clock_align` (`left`, `center`, `right`; not the poster's), `lock_clock_seconds`,
//! `lock_show_date`, `calendar` and `world_clock` in `lock_widgets` with `lock_world_clock`
//! (a zone, as `Asia/Tokyo`) shown as `lock_world_clock_name`, `lock_wallpaper_file` (the lock's own), `lock_dim` (0 to
//! 80), `lock_show_lock_icon`, `lock_unlock_effect` (`lift`, `fade`, `slide`, `expand`),
//! `lock_unlock_duration` (milliseconds), `lock_sticker` (a picture) placed at
//! `lock_sticker_at` (`[x, y]` as shares of the screen) `lock_sticker_size` points large,
//! `lock_wallpaper_subject` (the wallpaper's subject cut out, drawn over the time),
//! and the rest of Good Lock's LockStar and ClockFace, which the README lists:
//! `lock_stickers`, `lock_wallpaper_folder`, `lock_lock_icon`, the unlock effect's colour,
//! direction and easing, the clock's rainbow, text, picture and dial, where the clock, the
//! notices and the message stand, the message's size, colour and opacity, what the
//! always-on display shows, and what a finger on the reader shows,
//! `weather` in `lock_widgets` with `lock_weather_at` (`[latitude, longitude]`, asked of
//! MET Norway) shown as `lock_weather_name`, and `lock_always_on`: left as long as
//! `lock_screen_off_after` says (30 seconds unless set), the time dim on black. With
//! `lock_fingerprint` (on unless set) and a finger enrolled with fprintd, a finger the
//! reader knows unlocks as the password does.

mod draw;
mod pam;
mod pin;

use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

use cairo::{Context, Format, ImageSurface};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_keyboard, delegate_output, delegate_pointer, delegate_registry, delegate_seat,
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
        pointer::{PointerEvent, PointerEventKind, PointerHandler},
        Capability, SeatHandler, SeatState,
    },
    session_lock::{
        SessionLock, SessionLockHandler, SessionLockState, SessionLockSurface, SessionLockSurfaceConfigure,
    },
    shm::{slot::SlotPool, Shm, ShmHandler},
};
use wayland_client::{
    globals::registry_queue_init,
    protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_shm, wl_surface},
    Connection, QueueHandle,
};

use draw::{Align, Button, Effect, Facts, Hit, Look, Moment, Notices, Playing, Style};

/// Room kept for the password as it is typed, so that it is not moved, leaving copies
/// behind, however long it gets in practice.
const PASSWORD_ROOM: usize = 256;

/// How long the wrong password's shake lasts.
const SHAKE: Duration = Duration::from_millis(360);
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
    /// Where the player's buttons were last drawn.
    media: Vec<Hit>,
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
    pointer: Option<wl_pointer::WlPointer>,
    /// The shortcut to open once the password is given.
    chosen: Option<usize>,
    look: Look,
    /// What the widgets and the notices show, read again every second.
    facts: Facts,
    /// What a player plays, as a thread of its own last heard: a player that does not answer
    /// must not keep the password from being typed.
    playing: Arc<Mutex<Option<Playing>>>,
    user: String,
    password: String,
    /// The PINs given wrong during this lock, by this locker or one before it.
    pin_tries: Arc<AtomicU32>,
    caps_lock: bool,
    /// How many characters the password being checked has.
    checking: Option<usize>,
    /// The last password's failure, and when it was told.
    error: Option<(&'static str, Instant)>,
    /// When the right password was given; the ink lifts from then.
    opened: Option<Instant>,
    /// Whether a timer is drawing frames.
    moving: bool,
    /// When the person last touched the keyboard or the pointer, and whether the lock has
    /// since gone to the always-on display.
    last_touch: Instant,
    dozing: bool,
    /// Whether the fingerprint reader is waiting for a finger.
    finger: bool,
    /// When the reader last read a finger, and whether it knew it.
    finger_seen: Option<(Instant, bool)>,
    exit: bool,
}

/// How long what a finger on the reader shows goes on.
const FINGER_PULSE: Duration = Duration::from_millis(600);

/// `nekoland-lock --preview FILE WIDTH HEIGHT [STYLE]` draws the lock as it would look now into
/// a PNG, without locking, for Settings to show; with a style, as it would look picked anew.
fn preview(file: &str, width: &str, height: &str, style: Option<&str>) -> ! {
    let (Ok(width), Ok(height)) = (width.parse::<i32>(), height.parse::<i32>()) else {
        fail("--preview FILE WIDTH HEIGHT [STYLE]");
    };
    let (_, look) = read_look(style, false);
    if let Some(at) = look.weather {
        fetch_weather(at);
    }
    let facts = Facts {
        playing: look.playing.then(read_playing).flatten(),
        ..read_facts(&look)
    };
    let tm = now();
    let (time, date) = (clock(&look), date());
    let moment = Moment {
        time: &time,
        date: &date,
        hour: tm.tm_hour as u32,
        minute: tm.tm_min as u32,
        second: tm.tm_sec as u32,
        facts: &facts,
        typed: 0,
        checking: false,
        caps_lock: false,
        error: None,
        chosen: None,
        finger: false,
        finger_pulse: None,
    };
    let Some(surface) = draw::backdrop(&look, width, height, true, look.blur) else {
        fail("cannot draw");
    };
    let written = Context::new(&surface).map_err(|err| err.to_string()).and_then(|cr| {
        draw::veil(&cr, &look, f64::from(width), f64::from(height));
        draw::words(&cr, &look, &moment, f64::from(width), f64::from(height));
        drop(cr);
        let mut out = std::fs::File::create(file).map_err(|err| err.to_string())?;
        surface.write_to_png(&mut out).map_err(|err| err.to_string())
    });
    match written {
        Ok(()) => std::process::exit(0),
        Err(err) => fail(&format!("{file}: {err}")),
    }
}

/// The exit code for the result of a command given to the lock in its arguments, with the
/// reason on standard error.
fn finish(result: Result<(), String>) -> i32 {
    match result {
        Ok(()) => 0,
        Err(why) => {
            eprintln!("nekoland-lock: {why}");
            1
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Settings sets and takes away the PIN, given on standard input, not in a command line.
    match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["--set-pin"] => {
            let mut pin = String::new();
            let result = std::io::stdin().read_line(&mut pin).map_err(|err| err.to_string()).and_then(|_| pin::set(pin.trim()));
            std::process::exit(finish(result));
        }
        ["--clear-pin"] => std::process::exit(finish(pin::clear())),
        _ => {}
    }
    if let [flag, file, width, height, style @ ..] = &args[..] {
        if flag == "--preview" && style.len() < 2 {
            preview(file, width, height, style.first().map(String::as_str));
        }
    }
    // A core dump of the locker would have the password typed into it.
    // SAFETY: only sets a flag of this process.
    if unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) } != 0 {
        eprintln!("nekoland-lock: could not keep from dumping core: {}", std::io::Error::last_os_error());
    }
    let conn = Connection::connect_to_env().unwrap_or_else(|err| fail(&format!("no Wayland display: {err}")));
    let (globals, queue) = registry_queue_init(&conn).unwrap_or_else(|err| fail(&err.to_string()));
    let qh: QueueHandle<Locker> = queue.handle();
    let mut event_loop: EventLoop<Locker> = EventLoop::try_new().unwrap_or_else(|err| fail(&err.to_string()));
    let shm = Shm::bind(&globals, &qh).unwrap_or_else(|err| fail(&err.to_string()));
    let pool = SlotPool::new(4 * 1920 * 1080, &shm).unwrap_or_else(|err| fail(&err.to_string()));
    let (user, look) = read_look(None, true);
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
        pointer: None,
        chosen: None,
        facts: Facts::default(),
        playing: Arc::default(),
        look,
        user,
        password: String::with_capacity(PASSWORD_ROOM),
        pin_tries: Arc::new(AtomicU32::new(read_pin_tries())),
        caps_lock: false,
        checking: None,
        error: None,
        opened: None,
        moving: false,
        last_touch: Instant::now(),
        dozing: false,
        finger: false,
        finger_seen: None,
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
    if locker.look.playing {
        let playing = locker.playing.clone();
        std::thread::spawn(move || loop {
            let now = read_playing();
            *playing.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = now;
            std::thread::sleep(Duration::from_secs(5));
        });
    }
    if let Some(at) = locker.look.weather {
        // Asked again as what was kept grows old; never on the thread that draws.
        std::thread::spawn(move || loop {
            fetch_weather(at);
            std::thread::sleep(Duration::from_secs(600));
        });
    }
    if locker.look.fingerprint {
        let (sender, receiver) = channel::channel();
        let user = locker.user.clone();
        std::thread::spawn(move || {
            if let Err(err) = verify_finger(&user, &sender) {
                eprintln!("nekoland-lock: no fingerprint: {err}");
            }
            let _ = sender.send(Finger::Gone);
        });
        let watched = event_loop.handle().insert_source(receiver, |event, _, locker| {
            if let channel::Event::Msg(finger) = event {
                locker.fingered(finger);
            }
        });
        if let Err(err) = watched {
            eprintln!("nekoland-lock: {err}");
        }
    }
    locker.facts = locker.read_facts();
    // The time, drawn again as its minute changes, and the widgets and notices as they do;
    // the player as its thread last heard.
    let mut shown = clock(&locker.look);
    event_loop
        .handle()
        .insert_source(
            Timer::from_duration(Duration::from_secs(1)),
            move |_, _, locker| {
                let now = clock(&locker.look);
                let facts = locker.read_facts();
                let look = &locker.look;
                if look.always_on && !locker.dozing && locker.opened.is_none() && locker.last_touch.elapsed() >= look.doze_after {
                    locker.dozing = true;
                    locker.redraw();
                }
                if now != shown || facts != locker.facts {
                    shown = now;
                    locker.facts = facts;
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
/// `picked` is a style as Settings offers it: with the clock and the blur it comes with, rather
/// than those set for the one in use. `locking`, the folder's next picture is shown at the next
/// lock.
fn read_look(picked: Option<&str>, locking: bool) -> (String, Look) {
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
    let word = |key: &str| config.get(key).and_then(toml::Value::as_str).unwrap_or_default();
    let own = Some(word("lock_wallpaper_file")).filter(|path| !path.is_empty());
    let turning = own.is_none().then(|| folder_wallpaper(&word("lock_wallpaper_folder").replacen('~', &home, 1), locking)).flatten();
    let wallpaper = own
        .map(str::to_string)
        .or(turning.clone())
        .or_else(|| config.get("wallpaper").and_then(toml::Value::as_str).map(str::to_string))
        .or(started.map(str::to_string))
        .filter(|_| flag("lock_wallpaper"))
        .and_then(|path| {
            let path = path.replacen('~', &home, 1);
            gdk_pixbuf::Pixbuf::from_file(&path)
                .map_err(|err| eprintln!("nekoland-lock: {path}: {err}"))
                .ok()
        });
    let (user, name) = account();
    let number = |key: &str, default: f64| config.get(key).and_then(toml::Value::as_integer).map_or(default, |n| n as f64);
    // A picture to be shown `size` points large: twice that in pixels, for a display at twice
    // the pixels, and no larger.
    let load = |path: &str, size: f64| {
        let path = path.replacen('~', &home, 1);
        let picture = gdk_pixbuf::Pixbuf::from_file(&path).map_err(|err| eprintln!("nekoland-lock: {path}: {err}")).ok()?;
        let shrink = (2.0 * size / f64::from(picture.width().max(picture.height()))).min(1.0);
        let picture = picture.scale_simple(
            ((f64::from(picture.width()) * shrink) as i32).max(1),
            ((f64::from(picture.height()) * shrink) as i32).max(1),
            gdk_pixbuf::InterpType::Hyper,
        )?;
        draw::pixbuf_surface(&picture)
    };
    let sticker = |file: &str, at: Option<&toml::Value>, size: Option<&toml::Value>| {
        let size = size.and_then(toml::Value::as_integer).map_or(160.0, |n| n as f64).clamp(32.0, 600.0);
        let at = at.and_then(shares).unwrap_or((0.78, 0.3));
        Some((load(file, size)?, at, size))
    };
    // The one sticker Settings first had, and those after it.
    let mut stickers: Vec<draw::Sticker> = Some(word("lock_sticker"))
        .filter(|path| !path.is_empty())
        .and_then(|path| sticker(path, config.get("lock_sticker_at"), config.get("lock_sticker_size")))
        .into_iter()
        .collect();
    if let Some(listed) = config.get("lock_stickers").and_then(toml::Value::as_array) {
        for entry in listed.iter().filter_map(toml::Value::as_table) {
            if let Some(file) = entry.get("file").and_then(toml::Value::as_str) {
                stickers.extend(sticker(file, entry.get("at"), entry.get("size")));
            }
        }
    }
    let at = |key: &str| config.get(key).and_then(shares);
    let style = match picked.unwrap_or(word("lock_style")) {
        "poster" => Style::Poster,
        "dial" => Style::Dial,
        _ => Style::Quiet,
    };
    let widgets: Vec<&str> = config
        .get("lock_widgets")
        .and_then(toml::Value::as_array)
        .map_or_else(|| vec!["battery", "playing"], |listed| listed.iter().filter_map(toml::Value::as_str).collect());
    let accent = wallpaper.as_ref().and_then(draw::accent_of).unwrap_or(draw::SAND);
    let effect = match word("lock_unlock_effect") {
        "fade" => Effect::Fade,
        "slide" => Effect::Slide,
        "expand" => Effect::Expand,
        "spread" => Effect::Spread,
        "wave" => Effect::Wave,
        "warp" => Effect::Warp,
        "ripple" => Effect::Ripple,
        "mosaic" => Effect::Mosaic,
        "wiper" => Effect::Wiper,
        "paint" => Effect::Paint,
        "glitch" => Effect::Glitch,
        "swirl" => Effect::Swirl,
        _ => Effect::Lift,
    };
    let look = Look {
        blur: if picked.is_some() { style != Style::Poster } else { flag("lock_blur") },
        accent,
        wallpaper,
        style,
        clock_colour: hex(word("lock_clock_color")),
        clock_rainbow: word("lock_clock_color") == "rainbow",
        clock_label: word("lock_clock_text").to_string(),
        clock_badge: Some(word("lock_clock_image")).filter(|path| !path.is_empty()).and_then(|path| load(path, 28.0)),
        numerals: match word("lock_dial_numerals") {
            "arabic" => draw::Numerals::Arabic,
            "roman" => draw::Numerals::Roman,
            _ => draw::Numerals::None,
        },
        hands: match word("lock_dial_hands") {
            "thin" => draw::Hands::Thin,
            "bold" => draw::Hands::Bold,
            _ => draw::Hands::Classic,
        },
        clock_at: at("lock_clock_at"),
        notices_at: at("lock_notices_at"),
        message_at: at("lock_message_at"),
        clock_weight: match if picked.is_some() { "" } else { word("lock_clock_weight") } {
            // As thin as the quiet style's own, which Settings shows as 細.
            "light" => pango::Weight::Ultralight,
            "regular" => pango::Weight::Normal,
            "bold" => pango::Weight::Heavy,
            _ if style == Style::Poster => pango::Weight::Heavy,
            _ => pango::Weight::Ultralight,
        },
        clock_font: word("lock_clock_font").to_string(),
        clock_scale: match word("lock_clock_size") {
            "small" => 0.8,
            "large" => 1.2,
            _ => 1.0,
        },
        clock_align: match word("lock_clock_align") {
            "left" => Align::Left,
            "right" => Align::Right,
            _ => Align::Center,
        },
        clock_seconds: config.get("lock_clock_seconds").and_then(toml::Value::as_bool).unwrap_or(false),
        show_date: flag("lock_show_date"),
        battery: widgets.contains(&"battery"),
        playing: widgets.contains(&"playing"),
        calendar: widgets.contains(&"calendar"),
        world_clock: Some(word("lock_world_clock"))
            .filter(|zone| !zone.is_empty() && widgets.contains(&"world_clock"))
            .map(str::to_string),
        world_clock_name: word("lock_world_clock_name").to_string(),
        weather: config
            .get("lock_weather_at")
            .and_then(toml::Value::as_array)
            .filter(|_| widgets.contains(&"weather"))
            .and_then(|at| {
                let degrees = |i: usize| at.get(i).and_then(|n| n.as_float().or_else(|| n.as_integer().map(|n| n as f64)));
                Some((degrees(0)?.clamp(-90.0, 90.0), degrees(1)?.clamp(-180.0, 180.0)))
            }),
        weather_name: word("lock_weather_name").to_string(),
        always_on: config.get("lock_always_on").and_then(toml::Value::as_bool).unwrap_or(false),
        always_on_widgets: config.get("lock_always_on_widgets").and_then(toml::Value::as_bool).unwrap_or(false),
        always_on_stickers: config.get("lock_always_on_stickers").and_then(toml::Value::as_bool).unwrap_or(false),
        always_on_brightness: number("lock_always_on_brightness", 100.0).clamp(20.0, 100.0) / 100.0,
        fingerprint: config.get("lock_fingerprint").and_then(toml::Value::as_bool).unwrap_or(true),
        finger_effect: match word("lock_fingerprint_effect") {
            "none" => draw::FingerEffect::None,
            "glow" => draw::FingerEffect::Glow,
            _ => draw::FingerEffect::Ripple,
        },
        doze_after: Duration::from_secs(
            config.get("lock_screen_off_after").and_then(toml::Value::as_integer).filter(|seconds| *seconds > 0).unwrap_or(30) as u64,
        ),
        dim: number("lock_dim", 0.0).clamp(0.0, 80.0) / 100.0,
        // Settings' first switch for it showed the padlock.
        lock_icon: match word("lock_lock_icon") {
            "padlock" => Some(draw::Icon::Padlock),
            "shield" => Some(draw::Icon::Shield),
            "key" => Some(draw::Icon::Key),
            "heart" => Some(draw::Icon::Heart),
            "paw" => Some(draw::Icon::Paw),
            "picture" => load(word("lock_lock_icon_file"), 22.0).map(draw::Icon::Picture),
            "" if config.get("lock_show_lock_icon").and_then(toml::Value::as_bool).unwrap_or(false) => Some(draw::Icon::Padlock),
            _ => None,
        },
        lock_icon_colour: hex(word("lock_lock_icon_color")),
        unlock: draw::Unlock {
            effect,
            colour: hex(word("lock_unlock_color")),
            direction: match word("lock_unlock_direction") {
                "left" => draw::Direction::Left,
                "right" => draw::Direction::Right,
                "up" => draw::Direction::Up,
                "down" => draw::Direction::Down,
                // The slide went to the left before it could be told which way.
                _ if effect == Effect::Slide => draw::Direction::Left,
                _ => draw::Direction::Right,
            },
            easing: match word("lock_unlock_easing") {
                "inout" => draw::Easing::InOut,
                "linear" => draw::Easing::Linear,
                _ => draw::Easing::Out,
            },
        },
        unlock_duration: Duration::from_millis(number("lock_unlock_duration", 560.0).clamp(200.0, 2000.0) as u64),
        stickers,
        // Cut from one picture: not over a folder's others.
        subject: Some(word("lock_wallpaper_subject")).filter(|path| !path.is_empty() && turning.is_none()).and_then(|path| {
            let path = path.replacen('~', &home, 1);
            let picture = gdk_pixbuf::Pixbuf::from_file(&path).map_err(|err| eprintln!("nekoland-lock: {path}: {err}")).ok()?;
            draw::pixbuf_surface(&picture)
        }),
        shortcuts_layout: match word("lock_shortcuts_layout") {
            "row" => draw::Shortcuts::Row,
            "column" => draw::Shortcuts::Column,
            _ => draw::Shortcuts::Corners,
        },
        notices: match word("lock_notices") {
            "hidden" => Notices::Hidden,
            "full" => Notices::Full,
            _ => Notices::Count,
        },
        message: word("lock_message").to_string(),
        message_size: number("lock_message_size", 13.0).clamp(10.0, 40.0),
        message_colour: hex(word("lock_message_color")),
        message_opacity: number("lock_message_opacity", 100.0).clamp(10.0, 100.0) / 100.0,
        show_name: flag("lock_show_name"),
        power_buttons: flag("lock_power_buttons"),
        shortcuts_unlocked: config.get("lock_shortcuts_unlocked").and_then(toml::Value::as_bool).unwrap_or(false),
        shortcuts: config
            .get("lock_shortcuts")
            .and_then(toml::Value::as_array)
            .map(|apps| {
                apps.iter()
                    .filter_map(toml::Value::as_str)
                    .take(draw::ROW_SHORTCUTS)
                    .enumerate()
                    .map(|(at, app)| (app.to_string(), shortcut_icon(at)))
                    .collect()
            })
            .unwrap_or_default(),
        // The account picture Settings keeps, as GNOME does.
        picture: gdk_pixbuf::Pixbuf::from_file(PathBuf::from(&home).join(".face")).ok(),
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

/// `[x, y]` as shares of the screen.
fn shares(value: &toml::Value) -> Option<(f64, f64)> {
    let at = value.as_array()?;
    let share = |i: usize| at.get(i).and_then(|n| n.as_float().or_else(|| n.as_integer().map(|n| n as f64)));
    Some((share(0)?.clamp(0.0, 1.0), share(1)?.clamp(0.0, 1.0)))
}

/// Where the lock keeps which of the folder's pictures it shows.
fn turn_file() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map_or_else(|| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".local/state"), PathBuf::from)
        .join("nekoland/lock-wallpaper")
}

/// The pictures in `folder`, by name.
fn folder_pictures(folder: &str) -> Vec<PathBuf> {
    if folder.is_empty() {
        return Vec::new();
    }
    let mut pictures: Vec<PathBuf> = std::fs::read_dir(folder)
        .map_err(|err| eprintln!("nekoland-lock: {folder}: {err}"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            let extension = path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase();
            matches!(extension.as_str(), "png" | "jpg" | "jpeg" | "webp")
        })
        .collect();
    pictures.sort();
    pictures
}

/// The folder's picture whose turn it is, as Dynamic Lock Screen changes it each lock;
/// `turn`, the next one's turn after it.
fn folder_wallpaper(folder: &str, turn: bool) -> Option<String> {
    let pictures = folder_pictures(folder);
    let file = turn_file();
    let at: usize = std::fs::read_to_string(&file).ok().and_then(|text| text.trim().parse().ok()).unwrap_or(0);
    let picture = pictures.get(at % pictures.len().max(1))?;
    if turn {
        let written = file.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::write(&file, ((at + 1) % pictures.len()).to_string()));
        if let Err(err) = written {
            eprintln!("nekoland-lock: {}: {err}", file.display());
        }
    }
    Some(picture.to_string_lossy().into_owned())
}

/// The icon the compositor drew for the shortcut at `at`, if it found one.
fn shortcut_icon(at: usize) -> Option<ImageSurface> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")?;
    let file = PathBuf::from(runtime).join(format!("nekoland-lock-shortcut-{at}.png"));
    let mut read = std::fs::File::open(file).ok()?;
    ImageSurface::create_from_png(&mut read).ok()
}

/// Starts a program and leaves it, saying so if it cannot.
fn start(program: &str, args: &[&str]) {
    if let Err(err) = std::process::Command::new(program).args(args).spawn() {
        eprintln!("nekoland-lock: {program}: {err}");
    }
}

/// `#rrggbb` as a colour.
fn hex(words: &str) -> Option<draw::Rgb> {
    let digits = words.strip_prefix('#').filter(|digits| digits.len() == 6)?;
    let value = u32::from_str_radix(digits, 16).ok()?;
    let channel = |shift: u32| f64::from((value >> shift) & 0xff) / 255.0;
    Some((channel(16), channel(8), channel(0)))
}

/// The first battery's charge, and whether it is charging.
fn read_battery() -> Option<(u8, bool)> {
    let supplies = std::fs::read_dir("/sys/class/power_supply").ok()?;
    supplies.flatten().map(|entry| entry.path()).find_map(|path| {
        let read = |name: &str| std::fs::read_to_string(path.join(name)).unwrap_or_default();
        if read("type").trim() != "Battery" {
            return None;
        }
        let percent = read("capacity").trim().parse::<u8>().ok()?;
        Some((percent.min(100), read("status").trim() == "Charging"))
    })
}

const MPRIS: &str = "org.mpris.MediaPlayer2";

/// The session bus, with little time for each answer: any program can be a player.
fn player_bus() -> Option<zbus::blocking::Connection> {
    zbus::blocking::connection::Builder::session().ok()?.method_timeout(Duration::from_millis(300)).build().ok()
}

/// Has the player `bus` go back, play or pause, or go on, as `method` says.
fn command_player(bus: &str, method: &str) {
    let Some(connection) = player_bus() else { return };
    let sent = connection.call_method(Some(bus), "/org/mpris/MediaPlayer2", Some(format!("{MPRIS}.Player").as_str()), method, &());
    if let Err(err) = sent {
        eprintln!("nekoland-lock: {bus} {method}: {err}");
    }
}

/// What an MPRIS player plays, one that plays before one paused; only a line of words.
fn read_playing() -> Option<Playing> {
    use std::collections::HashMap;
    use zbus::zvariant::OwnedValue;
    let connection = player_bus()?;
    let line = |words: String| words.chars().take(80).collect::<String>();
    let reply = connection
        .call_method(Some("org.freedesktop.DBus"), "/org/freedesktop/DBus", Some("org.freedesktop.DBus"), "ListNames", &())
        .ok()?;
    let (names,) = reply.body().deserialize::<(Vec<String>,)>().ok()?;
    let mut found: Vec<Playing> = names
        .iter()
        .filter(|name| name.starts_with(&format!("{MPRIS}.")))
        .filter_map(|bus| {
            let reply = connection
                .call_method(
                    Some(bus.as_str()),
                    "/org/mpris/MediaPlayer2",
                    Some("org.freedesktop.DBus.Properties"),
                    "GetAll",
                    &(format!("{MPRIS}.Player"),),
                )
                .ok()?;
            let (player,) = reply.body().deserialize::<(HashMap<String, OwnedValue>,)>().ok()?;
            let text = |value: &OwnedValue| String::try_from(value.clone()).ok();
            let metadata: HashMap<String, OwnedValue> =
                player.get("Metadata").and_then(|value| HashMap::try_from(value.clone()).ok())?;
            let title = metadata.get("xesam:title").and_then(text).filter(|title| !title.is_empty())?;
            let artist = metadata
                .get("xesam:artist")
                .and_then(|value| Vec::<String>::try_from(value.clone()).ok())
                .map(|artists| artists.join("、"))
                .unwrap_or_default();
            let paused = player.get("PlaybackStatus").and_then(text).as_deref() != Some("Playing");
            Some(Playing { title: line(title), artist: line(artist), paused, bus: bus.clone() })
        })
        .collect();
    found.sort_by_key(|playing| playing.paused);
    found.into_iter().next()
}

/// Where the wrong PINs of this lock are counted, so that a locker started again while the
/// screen stays locked does not get fresh tries. The compositor takes it away on unlocking.
fn pin_tries_file() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?).join("nekoland-lock-pin-tries"))
}

/// The wrong PINs counted so far; a count that cannot be read is taken as all of them.
fn read_pin_tries() -> u32 {
    let Some(file) = pin_tries_file() else { return 0 };
    match std::fs::read_to_string(&file) {
        Ok(text) => text.trim().parse().unwrap_or(pin::TRIES),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => 0,
        Err(err) => {
            eprintln!("nekoland-lock: {}: {err}", file.display());
            pin::TRIES
        }
    }
}

/// The notifications the compositor kept for the lock since it locked.
fn read_notices() -> Vec<(String, String)> {
    let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") else { return Vec::new() };
    std::fs::read_to_string(PathBuf::from(runtime).join("nekoland-lock-notices"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .map(|(app, summary)| (app.to_string(), summary.to_string()))
        .collect()
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

fn clock(look: &Look) -> String {
    let tm = now();
    let hour = if look.clock_24h {
        tm.tm_hour
    } else {
        (tm.tm_hour + 11) % 12 + 1
    };
    if look.clock_seconds {
        format!("{hour}:{:02}:{:02}", tm.tm_min, tm.tm_sec)
    } else {
        format!("{hour}:{:02}", tm.tm_min)
    }
}

/// Today's next event in the Calendar's file, `10:00 Title`, one lasting all day if there is
/// no other; lines are `YYYY-MM-DD<TAB>HH:MM or -<TAB>calendar<TAB>title`.
fn read_event() -> Option<String> {
    let data = std::env::var_os("XDG_DATA_HOME").map_or_else(
        || PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".local/share"),
        PathBuf::from,
    );
    let events = std::fs::read_to_string(data.join("nekoland/calendar.tsv")).ok()?;
    next_event(&events, now())
}

fn next_event(events: &str, tm: libc::tm) -> Option<String> {
    let day = format!("{:04}-{:02}-{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday);
    let now = format!("{:02}:{:02}", tm.tm_hour, tm.tm_min);
    let mut today: Vec<(&str, &str)> = events
        .lines()
        .filter_map(|line| {
            let mut fields = line.splitn(4, '\t');
            let (date, time, _, title) = (fields.next()?, fields.next()?, fields.next()?, fields.next()?);
            (date == day && (time == "-" || time >= now.as_str())).then_some((time, title))
        })
        .collect();
    // Timed ones first, the soonest first; "-" sorts after every time.
    today.sort_by_key(|(time, _)| if *time == "-" { "99" } else { *time });
    let (time, title) = today.first()?;
    let title: String = title.chars().take(60).collect();
    Some(if *time == "-" { format!("整天 {title}") } else { format!("{time} {title}") })
}

/// The second clock's city, from its zone's name, and its time there.
fn world_time(zone: &str, look: &Look) -> Option<(String, String)> {
    let there = glib::DateTime::now(&glib::TimeZone::from_identifier(Some(zone))?).ok()?;
    let time = there.format(if look.clock_24h { "%-H:%M" } else { "%-I:%M" }).ok()?;
    let city = if look.world_clock_name.is_empty() {
        zone.rsplit('/').next().unwrap_or(zone).replace('_', " ")
    } else {
        look.world_clock_name.clone()
    };
    Some((city, time.to_string()))
}

/// What the widgets and the notices show now, but what plays, which a thread reads.
fn read_facts(look: &Look) -> Facts {
    Facts {
        battery: look.battery.then(read_battery).flatten(),
        playing: None,
        notices: if look.notices == Notices::Hidden { Vec::new() } else { read_notices() },
        event: look.calendar.then(read_event).flatten(),
        world: look.world_clock.as_deref().and_then(|zone| world_time(zone, look)),
        weather: read_weather(look),
    }
}

/// Where the last weather asked for is kept, for the lock and Settings' preview alike:
/// `latitude,longitude<TAB>degrees<TAB>MET Norway's symbol<TAB>when, in Unix seconds`.
fn weather_file() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map_or_else(|| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".cache"), PathBuf::from)
        .join("nekoland/lock-weather")
}

fn weather_place((latitude, longitude): (f64, f64)) -> String {
    format!("{latitude:.2},{longitude:.2}")
}

/// Asks MET Norway, which needs no account, for the weather at a place, unless what is kept
/// for it is less than half an hour old.
fn fetch_weather(at: (f64, f64)) {
    let file = weather_file();
    let place = weather_place(at);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |since| since.as_secs());
    let kept = std::fs::read_to_string(&file).unwrap_or_default();
    let fields: Vec<&str> = kept.trim().split('\t').collect();
    if let [kept_place, _, _, when] = fields[..] {
        if kept_place == place && when.parse::<u64>().is_ok_and(|when| now.saturating_sub(when) < 1800) {
            return;
        }
    }
    let url = format!("https://api.met.no/weatherapi/locationforecast/2.0/compact?lat={:.2}&lon={:.2}", at.0, at.1);
    // MET Norway turns away requests that do not say what sends them.
    let answer = match std::process::Command::new("curl").args(["-sf", "--max-time", "6", "-A", "nekoland-lock/1.0", &url]).output() {
        Ok(answer) if answer.status.success() => answer.stdout,
        Ok(answer) => return eprintln!("nekoland-lock: weather: curl {}", answer.status),
        Err(err) => return eprintln!("nekoland-lock: weather: curl: {err}"),
    };
    let Some((degrees, symbol)) = parse_weather(&String::from_utf8_lossy(&answer)) else {
        return eprintln!("nekoland-lock: weather: an answer without the current weather");
    };
    let written = file
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&file, format!("{place}\t{degrees}\t{symbol}\t{now}\n")));
    if let Err(err) = written {
        eprintln!("nekoland-lock: {}: {err}", file.display());
    }
}

/// The temperature and the weather's symbol for the hour now in MET Norway's answer, the
/// first of its `timeseries`; the symbol without `_day` or `_night`.
fn parse_weather(answer: &str) -> Option<(f64, String)> {
    let now = &answer[answer.find("\"timeseries\":")?..];
    let at = now.find("\"air_temperature\":")? + "\"air_temperature\":".len();
    let rest = &now[at..];
    let end = rest.find(|c: char| !(c.is_ascii_digit() || c == '-' || c == '.')).unwrap_or(rest.len());
    let degrees: f64 = rest[..end].parse().ok().filter(|n: &f64| n.is_finite())?;
    let at = now.find("\"symbol_code\":\"")? + "\"symbol_code\":\"".len();
    let symbol = now[at..].split('"').next()?.split('_').next()?;
    if symbol.is_empty() || !symbol.bytes().all(|b| b.is_ascii_lowercase()) {
        return None;
    }
    Some((degrees.clamp(-100.0, 100.0), symbol.to_string()))
}

/// MET Norway's weather symbol in words.
fn weather_words(symbol: &str) -> &'static str {
    match symbol {
        "clearsky" => "晴",
        "fair" => "晴時多雲",
        "partlycloudy" => "多雲",
        "cloudy" => "陰",
        "fog" => "霧",
        _ if symbol.contains("thunder") => "雷雨",
        _ if symbol.contains("sleet") => "雨夾雪",
        _ if symbol.contains("snowshowers") => "陣雪",
        _ if symbol.contains("snow") => "雪",
        _ if symbol.contains("rainshowers") => "陣雨",
        "lightrain" => "小雨",
        "heavyrain" => "大雨",
        "rain" => "雨",
        _ => "",
    }
}

/// The weather kept for the place asked of, as the widget shows it.
fn read_weather(look: &Look) -> Option<(String, String)> {
    let place = weather_place(look.weather?);
    let kept = std::fs::read_to_string(weather_file()).ok()?;
    let fields: Vec<&str> = kept.trim().split('\t').collect();
    let [kept_place, degrees, code, _] = fields[..] else { return None };
    if kept_place != place {
        return None;
    }
    let degrees: f64 = degrees.parse().ok()?;
    let words = weather_words(code);
    let city = if look.weather_name.is_empty() { "天氣".to_string() } else { look.weather_name.clone() };
    Some((city, format!("{degrees:.0}° {words}").trim_end().to_string()))
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
    /// What the widgets and the notices show now; what plays as its thread last heard.
    fn read_facts(&self) -> Facts {
        Facts {
            playing: self.playing.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone(),
            ..read_facts(&self.look)
        }
    }

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
            media: Vec::new(),
        });
    }

    fn redraw(&mut self) {
        for i in 0..self.screens.len() {
            self.draw(i);
        }
    }

    fn draw(&mut self, i: usize) {
        let (time, date) = (clock(&self.look), date());
        let error = self
            .error
            .map(|(why, at)| (why, at.elapsed().as_secs_f64() / SHAKE.as_secs_f64()));
        let tm = now();
        let moment = Moment {
            time: &time,
            date: &date,
            hour: tm.tm_hour as u32,
            minute: tm.tm_min as u32,
            second: tm.tm_sec as u32,
            facts: &self.facts,
            typed: self.checking.unwrap_or_else(|| self.password.chars().count()),
            checking: self.checking.is_some(),
            caps_lock: self.caps_lock,
            error,
            chosen: self.chosen,
            finger: self.finger,
            finger_pulse: self.finger_seen.and_then(|(at, known)| {
                let t = at.elapsed().as_secs_f64() / FINGER_PULSE.as_secs_f64();
                (t < 1.0).then_some((t, known))
            }),
        };
        let lifted = self
            .opened
            .map(|at| (at.elapsed().as_secs_f64() / self.look.unlock_duration.as_secs_f64()).min(1.0));
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
                    draw::veil(&cr, &self.look, f64::from(width), f64::from(height));
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
        if let (true, None, Ok(cr)) = (self.dozing, lifted, Context::new(&target)) {
            cr.scale(f64::from(scale), f64::from(scale));
            draw::doze(&cr, &self.look, &moment, f64::from(width), f64::from(height));
            screen.media.clear();
        } else if let (Ok(cr), Some(locked)) = (Context::new(&target), &screen.locked) {
            let words = |cr: &Context| {
                cr.save().ok();
                cr.scale(f64::from(scale), f64::from(scale));
                let media = draw::words(cr, &self.look, &moment, f64::from(width), f64::from(height));
                cr.restore().ok();
                media
            };
            match (lifted, &screen.sharp) {
                (Some(lifted), Some(sharp)) => {
                    // The ink and the words go together, as one sheet.
                    if let Ok(layer) = ImageSurface::create(Format::ARgb32, pw, ph) {
                        if let Ok(lc) = Context::new(&layer) {
                            let _ = lc.set_source_surface(locked, 0.0, 0.0);
                            let _ = lc.paint();
                            words(&lc);
                        }
                        draw::reveal(&cr, &self.look.unlock, self.look.accent, sharp, &layer, lifted, f64::from(scale));
                    }
                }
                _ => {
                    let _ = cr.set_source_surface(locked, 0.0, 0.0);
                    let _ = cr.paint();
                    screen.media = words(&cr);
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
                if locker.opened.is_some_and(|at| at.elapsed() >= locker.look.unlock_duration) {
                    locker.unlock();
                    locker.moving = false;
                    return TimeoutAction::Drop;
                }
                let shaking = locker.error.is_some_and(|(_, at)| at.elapsed() < SHAKE);
                let pulsing = locker.finger_seen.is_some_and(|(at, _)| at.elapsed() < FINGER_PULSE);
                if locker.opened.is_none() && !shaking && !pulsing {
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
            if let Some((app, _)) = self.chosen.and_then(|at| self.look.shortcuts.get(at)) {
                start("nekoland", &["msg", &format!("lock-open {app}")]);
            }
        }
        self.exit = true;
    }

    /// The person is there: the lock wakes from the always-on display, if it dozed.
    fn touched(&mut self) {
        self.last_touch = Instant::now();
        if self.dozing {
            self.dozing = false;
            self.redraw();
        }
    }

    /// What the fingerprint reader said.
    fn fingered(&mut self, finger: Finger) {
        if self.opened.is_some() {
            return;
        }
        match finger {
            Finger::Ready => self.finger = true,
            Finger::Gone => self.finger = false,
            Finger::Matched => {
                self.error = None;
                self.password.clear();
                self.finger_seen = Some((Instant::now(), true));
                // Unlocked once the account may be, as for the password.
                let (sender, receiver) = channel::channel();
                let user = self.user.clone();
                std::thread::spawn(move || drop(sender.send(pam::account(&user))));
                self.when_checked(receiver);
            }
            Finger::Wrong => {
                self.error = Some(("指紋不符", Instant::now()));
                self.finger_seen = Some((Instant::now(), false));
            }
            Finger::Again => self.error = Some(("請再掃描一次", Instant::now())),
        }
        // A finger on the reader is the person there.
        self.last_touch = Instant::now();
        self.dozing = false;
        self.redraw();
        self.animate();
    }

    /// What a press on a button does.
    fn press(&mut self, button: Button) {
        match button {
            Button::Shortcut(at) => {
                let Some((app, _)) = self.look.shortcuts.get(at) else { return };
                if self.look.shortcuts_unlocked {
                    start("nekoland", &["msg", &format!("lock-open {app}")]);
                } else {
                    // Opened once the password is given; pressed again, it is not.
                    self.chosen = if self.chosen == Some(at) { None } else { Some(at) };
                    self.redraw();
                }
            }
            Button::Previous | Button::PlayPause | Button::Next => {
                let Some(bus) = self.facts.playing.as_ref().map(|playing| playing.bus.clone()) else { return };
                let method = match button {
                    Button::Previous => "Previous",
                    Button::Next => "Next",
                    _ => "PlayPause",
                };
                // Off this thread, as the player may be slow to answer; what it plays then
                // is read again at once, for the button to show it.
                let playing = self.playing.clone();
                std::thread::spawn(move || {
                    command_player(&bus, method);
                    *playing.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = read_playing();
                });
            }
            Button::Sleep => start("nekoland", &["msg", "sleep"]),
            Button::Restart => start("systemctl", &["reboot"]),
            Button::PowerOff => start("systemctl", &["poweroff"]),
        }
    }

    fn submit(&mut self) {
        if self.password.is_empty() || self.checking.is_some() {
            return;
        }
        let (sender, receiver) = channel::channel();
        let password = std::mem::replace(&mut self.password, String::with_capacity(PASSWORD_ROOM));
        let user = self.user.clone();
        // Its dots stay, dimmed, while it is checked.
        self.checking = Some(password.chars().count());
        let tries = self.pin_tries.clone();
        std::thread::spawn(move || {
            // Digits, with a PIN set and tries left, are tried as the PIN first; then, being
            // possibly a password of digits, as the password.
            if pin::is_valid(&password) && tries.load(Ordering::Relaxed) < pin::TRIES && pin::is_set() {
                if pin::check(&password) {
                    pam::wipe(password);
                    let _ = sender.send(pam::account(&user));
                    return;
                }
                let wrong = tries.fetch_add(1, Ordering::Relaxed) + 1;
                if let Some(file) = pin_tries_file() {
                    if let Err(err) = std::fs::write(&file, wrong.to_string()) {
                        eprintln!("nekoland-lock: {}: {err}", file.display());
                    }
                }
            }
            let _ = sender.send(pam::check(&user, &password));
            pam::wipe(password);
        });
        self.when_checked(receiver);
        self.redraw();
    }

    /// Unlocks once what was given is found right, or tells why not.
    fn when_checked(&mut self, receiver: channel::Channel<Result<(), pam::Failure>>) {
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
        self.touched();
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
        if capability == Capability::Pointer && self.pointer.is_none() {
            match self.seats.get_pointer(qh, &seat) {
                Ok(pointer) => self.pointer = Some(pointer),
                Err(err) => eprintln!("nekoland-lock: no pointer: {err}"),
            }
        }
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
        if capability == Capability::Pointer {
            if let Some(pointer) = self.pointer.take() {
                pointer.release();
            }
        }
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl PointerHandler for Locker {
    fn pointer_frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_pointer::WlPointer, events: &[PointerEvent]) {
        const BTN_LEFT: u32 = 0x110;
        // The first touch only wakes it: what was pressed could not be seen.
        let dozed = self.dozing;
        self.touched();
        if dozed {
            return;
        }
        for event in events {
            let PointerEventKind::Press { button: BTN_LEFT, .. } = event.kind else { continue };
            if self.checking.is_some() || self.opened.is_some() {
                continue;
            }
            let Some(screen) = self.screens.iter().find(|screen| screen.surface.wl_surface() == &event.surface) else {
                continue;
            };
            let Some((width, height)) = screen.size else { continue };
            let (x, y) = event.position;
            let media = screen.media.iter().find(|(_, cx, cy, r)| (x - cx).hypot(y - cy) <= *r).map(|(button, ..)| *button);
            if let Some(button) = media.or_else(|| draw::button_at(&self.look, f64::from(width), f64::from(height), x, y)) {
                self.press(button);
            }
        }
    }
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
delegate_pointer!(Locker);
delegate_session_lock!(Locker);
delegate_shm!(Locker);
delegate_registry!(Locker);

#[cfg(test)]
mod tests {
    use super::{next_event, parse_weather};

    #[test]
    fn the_weather_is_the_first_hours_not_the_units() {
        let answer = r#"{"properties":{"meta":{"units":{"air_temperature":"celsius"}},"timeseries":[{"time":"2026-11-18T09:00:00Z","data":{"instant":{"details":{"air_pressure_at_sea_level":1021.3,"air_temperature":-3.4}},"next_12_hours":{"summary":{"symbol_code":"lightrain"}},"next_1_hours":{"summary":{"symbol_code":"lightsnowshowers_night"}}}}]}}"#;
        assert_eq!(parse_weather(answer), Some((-3.4, "lightrain".to_string())));
        assert_eq!(parse_weather(r#"{"error":true}"#), None);
        assert_eq!(parse_weather(r#"{"timeseries":[{"air_temperature":"x","symbol_code":"rain"}]}"#), None);
    }

    #[test]
    fn the_next_event_is_todays_soonest_still_to_come_then_one_all_day() {
        // SAFETY: an all-zero tm is a valid value to fill in.
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        (tm.tm_year, tm.tm_mon, tm.tm_mday, tm.tm_hour, tm.tm_min) = (126, 10, 18, 12, 0);
        let events = "2026-11-18\t09:00\tw\t早會\n2026-11-18\t-\tp\t生日\n2026-11-18\t18:30\tp\t晚餐\n2026-11-18\t13:00\tw\t開會\n2026-11-19\t08:00\tw\t明天\n";
        assert_eq!(next_event(events, tm).as_deref(), Some("13:00 開會"));
        tm.tm_hour = 20;
        assert_eq!(next_event(events, tm).as_deref(), Some("整天 生日"));
        tm.tm_mday = 20;
        assert_eq!(next_event(events, tm), None);
    }
}

/// How many unknown fingers the lock takes before it wants the password.
const FINGER_TRIES: u32 = 5;

/// What the fingerprint reader says, from its thread.
enum Finger {
    /// It waits for a finger.
    Ready,
    Matched,
    Wrong,
    /// The scan could not be read: too short, off centre and the like.
    Again,
    /// There is no reader, no finger enrolled, or it stopped.
    Gone,
}

/// Asks fprintd to verify `user`'s fingers until one matches, telling `sender` how each
/// scan went. fprintd's own policy decides whether this session may: an active local one.
fn verify_finger(user: &str, sender: &channel::Sender<Finger>) -> zbus::Result<()> {
    const FPRINT: &str = "net.reactivated.Fprint";
    let bus = zbus::blocking::Connection::system()?;
    let manager = zbus::blocking::Proxy::new(&bus, FPRINT, "/net/reactivated/Fprint/Manager", "net.reactivated.Fprint.Manager")?;
    // No fprintd, no reader or no finger enrolled are not failures, only no fingerprint.
    fn quiet<T>(err: zbus::Error) -> zbus::Result<Option<T>> {
        match &err {
            zbus::Error::MethodError(name, ..)
                if ["NoSuchDevice", "NoEnrolledPrints", "ServiceUnknown"].iter().any(|end| name.as_str().ends_with(end)) =>
            {
                Ok(None)
            }
            _ => Err(err),
        }
    }
    let Some(path) = manager.call::<_, _, zbus::zvariant::OwnedObjectPath>("GetDefaultDevice", &()).map(Some).or_else(quiet)? else {
        return Ok(());
    };
    let device = zbus::blocking::Proxy::new(&bus, FPRINT, path, "net.reactivated.Fprint.Device")?;
    let enrolled = device.call::<_, _, Vec<String>>("ListEnrolledFingers", &(user,)).map(Some).or_else(quiet)?;
    if enrolled.is_none_or(|fingers| fingers.is_empty()) {
        return Ok(());
    }
    // Listened for before verifying starts, so that no answer comes before it is heard.
    let statuses = device.receive_signal("VerifyStatus")?;
    device.call::<_, _, ()>("Claim", &(user,))?;
    device.call::<_, _, ()>("VerifyStart", &("any",))?;
    let _ = sender.send(Finger::Ready);
    let mut misses = 0;
    for status in statuses {
        let (result, done): (String, bool) = status.body().deserialize()?;
        let said = match result.as_str() {
            "verify-match" => {
                let _ = sender.send(Finger::Matched);
                return Ok(());
            }
            "verify-no-match" => Finger::Wrong,
            "verify-disconnected" | "verify-unknown-error" => return Err(zbus::Error::Failure(result)),
            _ => Finger::Again,
        };
        let wrong = matches!(said, Finger::Wrong);
        let _ = sender.send(said);
        if wrong {
            misses += 1;
            // Fingers can be tried without end, which a password through PAM cannot: after
            // a few, only the password unlocks.
            if misses == FINGER_TRIES {
                device.call::<_, _, ()>("VerifyStop", &())?;
                device.call::<_, _, ()>("Release", &())?;
                return Ok(());
            }
        }
        if done {
            // fprintd ends verifying after a finger it does not know; it is asked again.
            device.call::<_, _, ()>("VerifyStop", &())?;
            device.call::<_, _, ()>("VerifyStart", &("any",))?;
        }
    }
    Ok(())
}
