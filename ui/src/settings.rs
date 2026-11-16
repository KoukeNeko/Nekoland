//! `nekoland-settings`: Nekoland's settings. What the compositor keeps it asks for and
//! changes through `nekoland msg`, so the compositor alone reads and writes its file and
//! puts each change in effect; the rest is the system's, through its own tools.

use std::{
    path::{Path, PathBuf},
    process::Command,
    rc::Rc,
};

use slint::{
    winit_030::WinitWindowAccessor, ComponentHandle, Image, Model, ModelRc, Rgba8Pixel, SharedPixelBuffer,
    SharedString, VecModel,
};

slint::include_modules!();

const APPS_VIEWS: [&str; 4] = ["overview", "launchpad", "shelf", "sidebar"];
const WALLPAPER_MODES: [&str; 3] = ["fill", "fit", "center"];
const WEEKDAYS: [&str; 7] = ["一", "二", "三", "四", "五", "六", "日"];
/// Where pictures for the desktop are looked for, and how many are offered.
const WALLPAPER_DIRS: [&str; 3] = [
    "~/.local/share/backgrounds",
    "~/Pictures/Wallpapers",
    "/usr/share/backgrounds",
];
const WALLPAPERS: usize = 8;
const SINK: &str = "@DEFAULT_AUDIO_SINK@";

/// What a program printed, or None when it could not be run or failed.
fn output(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .inspect_err(|err| eprintln!("{program}: {err}"))
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

fn run(program: &str, args: &[&str]) {
    if output(program, args).is_none() {
        eprintln!("{program} {} failed", args.join(" "));
    }
}

/// Changes one of the compositor's settings; it answers only when the change is refused.
fn set(line: &str) {
    if let Some(reply) = output("nekoland", &["msg", &format!("set {line}")]) {
        if !reply.trim().is_empty() {
            eprintln!("{}", reply.trim());
        }
    }
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}

fn expand(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => home().join(rest),
        None => PathBuf::from(path),
    }
}

/// The pictures offered for the desktop: the one in use first, then those found.
fn wallpapers(current: Option<&Path>) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = current.into_iter().map(Path::to_path_buf).collect();
    for dir in WALLPAPER_DIRS {
        let Ok(entries) = std::fs::read_dir(expand(dir)) else {
            continue;
        };
        let mut pictures: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| {
                        matches!(extension.to_lowercase().as_str(), "jpg" | "jpeg" | "png" | "webp")
                    })
            })
            .collect();
        pictures.sort();
        for picture in pictures {
            if !found.contains(&picture) {
                found.push(picture);
            }
        }
    }
    found.truncate(WALLPAPERS);
    found
}

/// The value of `key` in a file of `KEY=VALUE` or `KEY: VALUE` lines.
fn field(text: &str, key: &str, separator: char) -> Option<String> {
    text.lines().find_map(|line| {
        let (name, value) = line.split_once(separator)?;
        (name.trim() == key).then(|| value.trim().trim_matches('"').to_string())
    })
}

fn facts() -> Vec<Fact> {
    let read = |path: &str| std::fs::read_to_string(path).unwrap_or_default();
    let mut facts = Vec::new();
    let mut add = |title: &str, value: Option<String>| {
        if let Some(value) = value.filter(|value| !value.is_empty()) {
            facts.push(Fact {
                title: title.into(),
                value: value.into(),
            });
        }
    };
    add("電腦名稱", Some(read("/etc/hostname").trim().to_string()));
    add("系統", field(&read("/etc/os-release"), "PRETTY_NAME", '='));
    add(
        "核心",
        Some(read("/proc/sys/kernel/osrelease").trim().to_string()),
    );
    add("處理器", field(&read("/proc/cpuinfo"), "model name", ':'));
    let memory = field(&read("/proc/meminfo"), "MemTotal", ':')
        .and_then(|value| value.trim_end_matches("kB").trim().parse::<f64>().ok());
    add(
        "記憶體",
        memory.map(|kib| format!("{:.1} GB", kib / 1024.0 / 1024.0)),
    );
    // The first line is the mode in use, as WIDTHxHEIGHT@HZ.
    let mode = output("nekoland", &["msg", "modes"]).and_then(|modes| {
        let line = modes.lines().next()?.to_string();
        let (size, rate) = line.split_once('@')?;
        let (width, height) = size.split_once('x')?;
        Some(format!("{width} × {height}，{rate} Hz"))
    });
    add("顯示器", mode);
    facts
}

/// The default output's volume, 0 to 100, and whether it is muted; None without sound.
fn volume() -> Option<(f32, bool)> {
    let text = output("wpctl", &["get-volume", SINK])?;
    let level: f32 = text.split_whitespace().nth(1)?.parse().ok()?;
    Some((level * 100.0, text.contains("[MUTED]")))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let window = Settings::new()?;
    slint::set_xdg_app_id("nekoland-settings")?;

    // The settings in effect, with what the file leaves out at the compositor's defaults.
    let config: toml::Table = output("nekoland", &["msg", "config"])
        .and_then(|text| {
            toml::from_str(&text)
                .inspect_err(|err| eprintln!("nekoland msg config: {err}"))
                .ok()
        })
        .unwrap_or_default();
    let flag = |key: &str| config.get(key).and_then(toml::Value::as_bool).unwrap_or_default();
    let word = |key: &str| {
        config
            .get(key)
            .and_then(toml::Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let index =
        |names: &[&str], key: &str| names.iter().position(|name| *name == word(key)).unwrap_or(0) as i32;

    window.set_apps_view(index(&APPS_VIEWS, "apps_view"));
    window.set_dock_glass(flag("dock_glass"));
    window.set_dock_magnify(flag("dock_magnify"));
    window.set_dock_magnify_scale(
        config
            .get("dock_magnify_scale")
            .and_then(toml::Value::as_float)
            .unwrap_or(1.0) as f32,
    );
    window.set_clock_24h(flag("clock_24h"));
    window.set_clock_weekday(flag("clock_weekday"));
    window.set_clock_seconds(flag("clock_seconds"));
    window.set_wallpaper_mode(index(&WALLPAPER_MODES, "wallpaper_mode"));
    window.set_natural_scroll(flag("natural_scroll"));
    window.set_tap_to_click(flag("tap_to_click"));
    window.set_pointer_speed(
        config
            .get("pointer_speed")
            .and_then(toml::Value::as_float)
            .unwrap_or_default() as f32,
    );

    let autostart: Vec<String> = config
        .get("autostart")
        .and_then(toml::Value::as_array)
        .map(|commands| {
            commands
                .iter()
                .filter_map(|command| command.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    // Without a wallpaper of Nekoland's own, the one swaybg is started with at login.
    let current = config
        .get("wallpaper")
        .and_then(toml::Value::as_str)
        .map(expand)
        .or_else(|| {
            autostart.iter().find_map(|command| {
                let mut words = command.strip_prefix("swaybg ")?.split_whitespace();
                words.find(|word| *word == "-i")?;
                words.next().map(expand)
            })
        });
    let pictures = wallpapers(current.as_deref());
    window.set_wallpapers(ModelRc::new(VecModel::from(vec![
        Image::default();
        pictures.len()
    ])));
    window.set_wallpaper(if current.is_some() { 0 } else { -1 });
    show_wallpapers(&window, pictures.clone());

    if let Some(scheme) = output(
        "gsettings",
        &["get", "org.gnome.desktop.interface", "color-scheme"],
    ) {
        window.set_dark(scheme.contains("dark"));
    }
    if let Some((level, muted)) = volume() {
        window.set_has_sound(true);
        window.set_volume(level.round());
        window.set_muted(muted);
    }

    let autostart = Rc::new(VecModel::from(
        autostart.into_iter().map(SharedString::from).collect::<Vec<_>>(),
    ));
    window.set_autostart(ModelRc::from(autostart.clone()));
    window.set_facts(ModelRc::new(VecModel::from(facts())));

    if let Some(now) = output("date", &["+%-m %-d %u %-H %-M %-S"]) {
        let parts: Vec<i32> = now
            .split_whitespace()
            .filter_map(|part| part.parse().ok())
            .collect();
        if let [month, day, weekday, hour, minute, second] = parts[..] {
            window.set_month(month);
            window.set_day(day);
            window.set_weekday(WEEKDAYS[(weekday as usize).clamp(1, 7) - 1].into());
            window.set_hour(hour);
            window.set_minute(minute);
            window.set_second(second);
        }
    }

    window.on_apps_view_changed(|kind| set(&format!("apps_view = {:?}", APPS_VIEWS[kind as usize])));
    window.on_setting_changed(|key, on| set(&format!("{key} = {on}")));
    window.on_wallpaper_mode_changed(|mode| {
        set(&format!("wallpaper_mode = {:?}", WALLPAPER_MODES[mode as usize]))
    });
    window.on_pointer_speed_changed(|speed| set(&format!("pointer_speed = {speed:.2}")));
    window.on_magnify_scale_changed(|scale| set(&format!("dock_magnify_scale = {scale:.2}")));
    {
        let autostart = autostart.clone();
        window.on_wallpaper_chosen(move |chosen| {
            let Some(path) = pictures.get(chosen as usize) else {
                return;
            };
            set(&format!("wallpaper = {:?}", path.to_string_lossy()));
            // A wallpaper started at login would stay under this one; it is not started again.
            // shortcut: the one running now stays until the session ends; stopping it needs its process.
            let kept: Vec<SharedString> = autostart
                .iter()
                .filter(|command| !command.starts_with("swaybg"))
                .collect();
            if kept.len() != autostart.row_count() {
                autostart.set_vec(kept);
                save_autostart(&autostart);
            }
        });
    }
    window.on_dark_changed(|dark| {
        run(
            "gsettings",
            &[
                "set",
                "org.gnome.desktop.interface",
                "color-scheme",
                if dark { "prefer-dark" } else { "default" },
            ],
        )
    });
    window.on_volume_changed(|level| run("wpctl", &["set-volume", SINK, &format!("{:.2}", level / 100.0)]));
    window.on_muted_changed(|muted| run("wpctl", &["set-mute", SINK, if muted { "1" } else { "0" }]));
    {
        let autostart = autostart.clone();
        window.on_autostart_removed(move |row| {
            autostart.remove(row as usize);
            save_autostart(&autostart);
        });
    }
    window.on_autostart_added(move |command| {
        autostart.push(command.trim().into());
        save_autostart(&autostart);
    });

    let weak = window.as_weak();
    window.on_dragged(move || {
        if let Some(window) = weak.upgrade() {
            window
                .window()
                .with_winit_window(|window| window.drag_window().inspect_err(|err| eprintln!("{err}")));
        }
    });
    let weak = window.as_weak();
    window.on_minimize(move || {
        if let Some(window) = weak.upgrade() {
            window.window().set_minimized(true);
        }
    });
    let weak = window.as_weak();
    window.on_maximize(move || {
        if let Some(window) = weak.upgrade() {
            let maximized = window.window().is_maximized();
            window.window().set_maximized(!maximized);
        }
    });
    window.on_close_window(|| {
        let _ = slint::quit_event_loop();
    });

    window.run()?;
    Ok(())
}

/// Puts each picture in, small, once it is read: reading them all would hold the window up.
fn show_wallpapers(window: &Settings, pictures: Vec<PathBuf>) {
    let weak = window.as_weak();
    let spawned = std::thread::Builder::new()
        .name("wallpapers".into())
        .spawn(move || {
            for (row, path) in pictures.into_iter().enumerate() {
                let picture = match image::open(&path) {
                    Ok(picture) => picture.thumbnail(640, 640).to_rgba8(),
                    Err(err) => {
                        eprintln!("{}: {err}", path.display());
                        continue;
                    }
                };
                let pixels = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
                    picture.as_raw(),
                    picture.width(),
                    picture.height(),
                );
                let _ = weak.upgrade_in_event_loop(move |window| {
                    window
                        .get_wallpapers()
                        .set_row_data(row, Image::from_rgba8(pixels));
                });
            }
        });
    if let Err(err) = spawned {
        eprintln!("{err}");
    }
}

fn save_autostart(autostart: &VecModel<SharedString>) {
    let quoted: Vec<String> = autostart
        .iter()
        .map(|command| format!("{:?}", command.as_str()))
        .collect();
    set(&format!("autostart = [{}]", quoted.join(", ")));
}
