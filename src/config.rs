//! Settings read once at startup from `$XDG_CONFIG_HOME/nekoland/config.toml`.

use std::{collections::HashMap, path::PathBuf, process::Command};

use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// The display mode to ask for, as `WIDTHxHEIGHT`, at its fastest rate, or as
    /// `WIDTHxHEIGHT@HZ`; the display's preferred one otherwise.
    pub mode: Option<String>,
    /// How many pixels a point is drawn with; worked out from the display's density otherwise.
    pub scale: Option<f64>,
    /// Whether Night Shift is on, off, or on from `night_shift_from` to `night_shift_to`, and
    /// how warm it is, from 0 to 1.
    pub night_shift: NightShift,
    pub night_shift_from: TimeOfDay,
    pub night_shift_to: TimeOfDay,
    pub night_shift_warmth: f64,
    /// Shell commands started once the compositor is up: the panel, the wallpaper, and so on.
    pub autostart: Vec<String>,
    pub terminal: String,
    pub launcher: String,
    /// What the launcher takes before the text to start searching with.
    pub launcher_search: String,
    pub volume_up: String,
    pub volume_down: String,
    pub volume_mute: String,
    pub brightness_up: String,
    pub brightness_down: String,
    /// The locker the screen is locked with.
    pub lock: String,
    /// The family of Nekoland's own text; Noto Sans CJK TC sets what it lacks.
    pub font: String,
    /// Seconds without input after which the screen locks; 0 for never.
    pub lock_after: u64,
    /// Seconds without input after which the displays go dark, and after which the
    /// computer sleeps; 0 for never.
    pub screen_off_after: u64,
    pub sleep_after: u64,
    /// How many seconds a notification's banner stays when its program leaves it to Nekoland.
    pub notification_seconds: u64,
    /// Whether the keyboard goes to the window the pointer moves into, as GNOME's sloppy
    /// focus, instead of the one clicked.
    pub focus_follows_pointer: bool,
    /// Whether a window dragged to a side of the display fills that half, and to its top,
    /// all of it.
    pub edge_tiling: bool,
    /// What pointing into each corner of a display does, as the Mac's hot corners.
    pub hot_corner_top_left: HotCorner,
    pub hot_corner_top_right: HotCorner,
    pub hot_corner_bottom_left: HotCorner,
    pub hot_corner_bottom_right: HotCorner,
    /// What the power button does, and what closing the lid does with no other display.
    pub power_button: PowerAction,
    pub lid_close: PowerAction,
    /// Layer surfaces and windows drawn on glass, by the namespace or application ID they
    /// give, with the corner radius they draw themselves with.
    pub glass: HashMap<String, i32>,
    /// The applications kept in the Dock, by desktop entry name or application ID.
    pub dock: Vec<String>,
    /// The folders kept at the Dock's right end, before the Trash.
    pub stacks: Vec<Stack>,
    /// How the applications are shown: Super+A and the Dock's first button.
    pub apps_view: AppsView,
    /// The picture on the desktop, drawn by swaybg, and how it fills the output.
    pub wallpaper: Option<String>,
    pub wallpaper_mode: WallpaperMode,
    pub clock_24h: bool,
    pub clock_weekday: bool,
    pub clock_seconds: bool,
    pub clock_date: bool,
    /// Whether the menu bar shows the network's and the sound's icons.
    pub menu_bar_network: bool,
    pub menu_bar_sound: bool,
    /// Whether the menu bar has its glass and tint, or shows the desktop through it, its text
    /// as dark or light as the wallpaper there needs.
    pub menu_bar_background: bool,
    /// Whether the Dock is drawn on glass, or on its tint alone.
    pub dock_glass: bool,
    /// How big the Dock's icons are, in logical pixels; the rest of the Dock grows with them.
    pub dock_size: i32,
    /// Whether the Dock slides off its edge until the pointer reaches it.
    pub dock_autohide: bool,
    /// The edge of the output the Dock is at.
    pub dock_position: DockPosition,
    /// Whether the Dock's icons grow as the pointer passes over them, as the Mac's do, and
    /// how big the one under it grows, as a multiple of their size.
    pub dock_magnify: bool,
    pub dock_magnify_scale: f64,
    /// Whether a dot under an icon says its application runs, and whether an icon bounces
    /// when clicked.
    pub dock_indicators: bool,
    pub dock_bounce: bool,
    /// How solid the glass's tint is, from 0 to 1, and how strong its blur, from 0, none, to
    /// 1: of the menu bar and the Dock themselves, and of all other glass, menus,
    /// notifications and what shows behind windows, as through GTK's sidebars, whose tint
    /// and that of GTK's menus Settings gives GTK's look.
    pub bar_opacity: f64,
    pub bar_blur: f64,
    pub glass_opacity: f64,
    pub glass_blur: f64,
    pub natural_scroll: bool,
    pub tap_to_click: bool,
    /// From -1, slowest, to 1, fastest.
    pub pointer_speed: f64,
    /// How far scrolling goes, times what the mouse or touchpad sends.
    pub scroll_speed: f64,
    /// How many points across the cursor is drawn.
    pub cursor_size: i32,
    /// The cursor theme and icon theme Nekoland draws with, by folder name; its own choice
    /// of common ones when empty.
    pub cursor_theme: String,
    pub icon_theme: String,
    /// Whether the pointer goes further the faster it is moved, as libinput's adaptive
    /// profile does, instead of as far however fast.
    pub pointer_acceleration: bool,
    /// Whether the buttons are swapped, for the left hand.
    pub left_handed: bool,
    /// Whether a touchpad ignores touches while keys are typed, and for a moment after.
    pub disable_while_typing: bool,
    /// The XKB layout keys are read with, and its variant and options, as setxkbmap takes
    /// them: `us`, `dvorak`, `ctrl:nocaps`.
    pub keyboard_layout: String,
    pub keyboard_variant: String,
    pub keyboard_options: String,
    /// Whether a held key repeats; milliseconds it is held before it does, and how many
    /// times a second it then does.
    pub key_repeat: bool,
    pub repeat_delay: i32,
    pub repeat_rate: i32,
}

/// The ways the applications can be shown.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum AppsView {
    /// In the overview, under the desktops' bar, as GNOME does.
    #[default]
    Overview,
    /// Full screen pages of icons, as the Mac's Launchpad.
    Launchpad,
    /// Grouped by kind on one long page.
    Shelf,
    /// A list of kinds beside the icons of the one chosen.
    Sidebar,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum NightShift {
    Off,
    On,
    Scheduled,
}

/// A time of day, in minutes since midnight, written as `HH:MM`.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq)]
#[serde(try_from = "String", into = "String")]
pub struct TimeOfDay(pub u16);

impl TryFrom<String> for TimeOfDay {
    type Error = String;

    fn try_from(text: String) -> Result<Self, String> {
        let parsed = text.split_once(':').and_then(|(hours, minutes)| {
            let (hours, minutes) = (hours.parse::<u16>().ok()?, minutes.parse::<u16>().ok()?);
            (hours < 24 && minutes < 60).then_some(hours * 60 + minutes)
        });
        parsed.map(TimeOfDay).ok_or(format!("{text:?} is not a time as HH:MM"))
    }
}

impl From<TimeOfDay> for String {
    fn from(time: TimeOfDay) -> String {
        format!("{:02}:{:02}", time.0 / 60, time.0 % 60)
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum HotCorner {
    None,
    /// The windows spread apart, as Super does.
    Overview,
    Apps,
    Lock,
}

/// What the power button or closing the lid does.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum PowerAction {
    Sleep,
    /// Asks first, as the menu's 關機⋯ does.
    ShutDown,
    Lock,
    Ignore,
}

impl AppsView {
    /// Whether the applications are on one long page, not pages of a grid.
    pub fn is_list(self) -> bool {
        matches!(self, AppsView::Shelf | AppsView::Sidebar)
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum WallpaperMode {
    #[default]
    Fill,
    Fit,
    Center,
}

impl WallpaperMode {
    pub fn name(self) -> &'static str {
        match self {
            WallpaperMode::Fill => "fill",
            WallpaperMode::Fit => "fit",
            WallpaperMode::Center => "center",
        }
    }
}

/// A folder in the Dock, and how it shows what is in it, as the Mac's stacks do.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Stack {
    /// Where the folder is; `~` stands for the home folder.
    pub path: String,
    #[serde(default)]
    pub sort: Sort,
    #[serde(default)]
    pub show: Show,
    #[serde(default)]
    pub view: View,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Sort {
    Name,
    #[default]
    Added,
    Modified,
    Created,
    Kind,
}

/// What the Dock shows for the folder: the folder's own icon, or its newest items piled up.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Show {
    Folder,
    #[default]
    Stack,
}

/// The edge of the output the Dock is at.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum DockPosition {
    Left,
    #[default]
    Bottom,
    Right,
}

impl DockPosition {
    pub fn name(self) -> &'static str {
        match self {
            DockPosition::Left => "left",
            DockPosition::Bottom => "bottom",
            DockPosition::Right => "right",
        }
    }
}

/// How the folder's items open from the Dock.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum View {
    #[default]
    Fan,
    Grid,
    List,
    Auto,
}

impl Sort {
    fn name(self) -> &'static str {
        match self {
            Sort::Name => "name",
            Sort::Added => "added",
            Sort::Modified => "modified",
            Sort::Created => "created",
            Sort::Kind => "kind",
        }
    }
}

impl Show {
    fn name(self) -> &'static str {
        match self {
            Show::Folder => "folder",
            Show::Stack => "stack",
        }
    }
}

impl View {
    fn name(self) -> &'static str {
        match self {
            View::Fan => "fan",
            View::Grid => "grid",
            View::List => "list",
            View::Auto => "auto",
        }
    }
}

impl Stack {
    pub fn new(path: String) -> Self {
        Self {
            path,
            sort: Sort::default(),
            show: Show::default(),
            view: View::default(),
        }
    }

    /// The folder's path with `~` and `$HOME` put in.
    pub fn folder(&self) -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_default();
        let path = match self.path.strip_prefix('~') {
            Some(rest) => format!("{home}{rest}"),
            None => self.path.replace("$HOME", &home),
        };
        PathBuf::from(path)
    }
}

/// The user's downloads folder, as `xdg-user-dirs` names it, written with `~`.
fn downloads() -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let named = std::fs::read_to_string(format!("{home}/.config/user-dirs.dirs"))
        .ok()
        .and_then(|text| {
            text.lines().find_map(|line| {
                let value = line.trim().strip_prefix("XDG_DOWNLOAD_DIR=")?;
                Some(value.trim_matches('"').replace("$HOME", "~"))
            })
        });
    named.unwrap_or_else(|| "~/Downloads".into())
}

impl Default for Config {
    fn default() -> Self {
        Self {
            mode: None,
            scale: None,
            autostart: Vec::new(),
            terminal: "foot".into(),
            launcher: "nekoland-launcher".into(),
            launcher_search: "--search=".into(),
            volume_up: "wpctl set-volume -l 1.0 @DEFAULT_AUDIO_SINK@ 5%+".into(),
            volume_down: "wpctl set-volume @DEFAULT_AUDIO_SINK@ 5%-".into(),
            volume_mute: "wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle".into(),
            brightness_up: "brightnessctl set 5%+".into(),
            brightness_down: "brightnessctl set 5%-".into(),
            // The ring in Nekoland's paper over dark paper, shown before anything is typed,
            // red when the password is wrong.
            lock: concat!(
                "swaylock -c 1b1b1e --indicator-idle-visible --indicator-radius 64",
                " --inside-color 1b1b1ecc --inside-clear-color 1b1b1ecc --inside-ver-color 1b1b1ecc",
                " --inside-wrong-color c4372fcc --ring-color f4f3f0 --ring-clear-color f4f3f0",
                " --ring-ver-color f4f3f0 --ring-wrong-color c4372f --key-hl-color 6f6d72",
                " --bs-hl-color c4372f --line-color 00000000 --separator-color 00000000",
                " --text-color f4f3f0 --text-clear-color f4f3f0 --text-ver-color f4f3f0",
                " --text-wrong-color f4f3f0",
            )
            .into(),
            font: "Ubuntu Sans".into(),
            lock_after: 0,
            screen_off_after: 0,
            sleep_after: 0,
            // logind's own: it powers off, which Nekoland asks about first.
            power_button: PowerAction::ShutDown,
            lid_close: PowerAction::Sleep,
            hot_corner_top_left: HotCorner::None,
            hot_corner_top_right: HotCorner::None,
            hot_corner_bottom_left: HotCorner::None,
            hot_corner_bottom_right: HotCorner::None,
            notification_seconds: 5,
            focus_follows_pointer: false,
            edge_tiling: true,
            night_shift: NightShift::Off,
            // The Mac's custom schedule.
            night_shift_from: TimeOfDay(22 * 60),
            night_shift_to: TimeOfDay(7 * 60),
            night_shift_warmth: 0.7,
            glass: HashMap::from([("nekoland-launcher".into(), 16)]),
            dock: [
                "org.gnome.Nautilus",
                "org.gnome.TextEditor",
                "org.gnome.Calculator",
                "foot",
            ]
            .map(String::from)
            .to_vec(),
            stacks: vec![Stack::new(downloads())],
            apps_view: AppsView::default(),
            wallpaper: None,
            wallpaper_mode: WallpaperMode::default(),
            clock_24h: true,
            clock_weekday: true,
            clock_seconds: false,
            menu_bar_network: true,
            menu_bar_sound: true,
            clock_date: true,
            menu_bar_background: true,
            dock_glass: true,
            dock_size: 40,
            dock_autohide: false,
            dock_position: DockPosition::Bottom,
            dock_magnify: true,
            dock_magnify_scale: 1.6,
            dock_indicators: true,
            dock_bounce: true,
            bar_opacity: 0.5,
            bar_blur: 0.5,
            glass_opacity: 0.8,
            glass_blur: 0.5,
            natural_scroll: false,
            tap_to_click: true,
            pointer_speed: 0.0,
            scroll_speed: 1.0,
            cursor_size: 24,
            cursor_theme: String::new(),
            icon_theme: String::new(),
            pointer_acceleration: true,
            left_handed: false,
            disable_while_typing: true,
            keyboard_layout: "us".into(),
            keyboard_variant: String::new(),
            keyboard_options: String::new(),
            key_repeat: true,
            repeat_delay: 200,
            repeat_rate: 25,
        }
    }
}

impl Config {
    pub fn load() -> Self {
        let Some(path) = config_path() else {
            return Self::default();
        };
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(err) => {
                tracing::warn!("{}: {err}", path.display());
                return Self::default();
            }
        };
        toml::from_str(&text).unwrap_or_else(|err| {
            tracing::warn!("{}: {err}", path.display());
            Self::default()
        })
    }

    /// Gives each glass the look the configuration says, and says whether any changed.
    pub fn set_glass_look(&self) -> bool {
        use crate::glass::{set_look, Tier};
        // Not `||`: both are set, whichever changed.
        set_look(Tier::Bar, self.bar_opacity, self.bar_blur)
            | set_look(Tier::Menu, self.glass_opacity, self.glass_blur)
    }

    /// The keyboard's repeat delay and rate, as Wayland takes them: a rate of 0 repeats none.
    pub fn repeat(&self) -> (i32, i32) {
        (self.repeat_delay, if self.key_repeat { self.repeat_rate } else { 0 })
    }

    /// The keyboard's layout, variant and options, as XKB is given them.
    pub fn keyboard(&self) -> [String; 3] {
        [&self.keyboard_layout, &self.keyboard_variant, &self.keyboard_options].map(String::clone)
    }

    /// The size `mode` asks for, and the rate in hertz if it names one.
    pub fn wanted_mode(&self) -> Option<((u16, u16), Option<u32>)> {
        let mode = self.mode.as_deref()?;
        let (size, rate) = match mode.split_once('@') {
            Some((size, rate)) => (size, Some(rate.trim().parse().ok()?)),
            None => (mode, None),
        };
        let (width, height) = size.split_once('x')?;
        Some(((width.trim().parse().ok()?, height.trim().parse().ok()?), rate))
    }
}

pub fn xkb([layout, variant, options]: &[String; 3]) -> smithay::input::keyboard::XkbConfig<'_> {
    smithay::input::keyboard::XkbConfig {
        layout,
        variant,
        options: Some(options.clone()).filter(|options| !options.is_empty()),
        ..Default::default()
    }
}

fn config_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| Some(PathBuf::from(std::env::var_os("HOME")?).join(".config")))?;
    Some(base.join("nekoland/config.toml"))
}

/// A file beside the configuration, for what Nekoland keeps between sessions.
pub fn config_file(name: &str) -> Option<PathBuf> {
    config_path().map(|path| path.with_file_name(name))
}

/// Writes the Dock's kept applications back to the configuration file.
pub fn save_dock(dock: &[String]) {
    let quoted: Vec<String> = dock.iter().map(|app_id| format!("{app_id:?}")).collect();
    if let Err(err) = save_setting("dock", &format!("dock = [{}]", quoted.join(", "))) {
        tracing::warn!("{err}");
    }
}

/// Writes the Dock's folders back to the configuration file.
pub fn save_stacks(stacks: &[Stack]) {
    if let Err(err) = save_setting("stacks", &stacks_line(stacks)) {
        tracing::warn!("{err}");
    }
}

fn stacks_line(stacks: &[Stack]) -> String {
    let tables: Vec<String> = stacks
        .iter()
        .map(|stack| {
            format!(
                "{{ path = {:?}, sort = {:?}, show = {:?}, view = {:?} }}",
                stack.path,
                stack.sort.name(),
                stack.show.name(),
                stack.view.name()
            )
        })
        .collect();
    format!("stacks = [{}]", tables.join(", "))
}

/// Changes one setting, given as a line of TOML, for the settings application.
pub fn set(line: &str) -> Result<(), String> {
    let table: toml::Table = toml::from_str(line).map_err(|err| err.to_string())?;
    match table.keys().collect::<Vec<_>>()[..] {
        [key] => save_setting(key, line),
        _ => Err(format!("not one setting: {line}")),
    }
}

/// Takes the setting `key` out of the configuration file, back to its default.
pub fn unset(key: &str) -> Result<(), String> {
    save_setting(key.trim(), "")
}

/// Puts `line` in the configuration file in place of the setting `key`, or before its
/// first table, if the file still reads as a configuration with it.
// shortcut: edits the file as text, so a setting written across several lines other than
// one item per line is left as it is; a TOML writer if that comes up.
fn save_setting(key: &str, line: &str) -> Result<(), String> {
    let path = config_path().ok_or("no home folder")?;
    let failed = |err: std::io::Error| format!("{}: {err}", path.display());
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(failed(err)),
    };
    let changed = with_setting(&text, key, line);
    toml::from_str::<Config>(&changed).map_err(|err| format!("{line}: {err}"))?;
    std::fs::create_dir_all(path.parent().unwrap_or(&path))
        .and_then(|_| std::fs::write(&path, changed))
        .map_err(failed)
}

/// The configuration text with its `key = [...]` setting, on one line or several, replaced,
/// or taken out for an empty `line`.
fn with_setting(text: &str, key: &str, line: &str) -> String {
    let new = (!line.is_empty()).then(|| line.to_string());
    let mut out = Vec::new();
    let mut lines = text.lines();
    let mut replaced = false;
    while let Some(current) = lines.next() {
        // Top-level settings end where the first table starts.
        if !replaced && current.trim_start().starts_with('[') {
            out.extend(new.clone());
            replaced = true;
        }
        let (name, value) = current.split_once('=').unwrap_or((current, ""));
        if name.trim() == key && !replaced {
            // An array that goes on past this line ends at the line closing it.
            let mut rest = current.to_string();
            while value.trim_start().starts_with('[') && !rest.trim_end().ends_with(']') {
                match lines.next() {
                    Some(next) => rest = next.to_string(),
                    None => break,
                }
            }
            out.extend(new.clone());
            replaced = true;
        } else {
            out.push(current.to_string());
        }
    }
    if !replaced {
        out.extend(new.clone());
    }
    out.join("\n") + "\n"
}

/// Text quoted for a shell, as one word.
pub fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// Draws the wallpaper with swaybg, if one is set; the program is returned, to be stopped
/// when the wallpaper changes.
pub fn start_wallpaper(config: &Config) -> Option<std::process::Child> {
    let home = std::env::var("HOME").unwrap_or_default();
    let path = config.wallpaper.as_ref()?.replacen('~', &home, 1);
    Command::new("swaybg")
        .args(["-m", config.wallpaper_mode.name(), "-i", &path])
        .spawn()
        .inspect_err(|err| tracing::warn!("swaybg: {err}"))
        .ok()
}

/// Starts a shell command as a client of this compositor.
pub fn spawn(command: &str) {
    if let Err(err) = Command::new("sh").arg("-c").arg(command).spawn() {
        tracing::warn!("{command}: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dock_line_is_replaced_where_it_is() {
        let text = "terminal = \"foot\"\ndock = [\n    \"a\",\n    \"b\",\n]\nmode = \"1920x1080\"\n";
        assert_eq!(
            with_setting(text, "dock", "dock = [\"b\"]"),
            "terminal = \"foot\"\ndock = [\"b\"]\nmode = \"1920x1080\"\n"
        );
        assert_eq!(
            with_setting("terminal = \"foot\"\n", "dock", "dock = []"),
            "terminal = \"foot\"\ndock = []\n"
        );
    }

    #[test]
    fn a_plain_setting_is_replaced_alone_and_added_before_tables() {
        let text = "clock_24h = true\ndock = [\n    \"a\",\n]\n[glass]\nwaybar = 12\n";
        assert_eq!(
            with_setting(text, "clock_24h", "clock_24h = false"),
            "clock_24h = false\ndock = [\n    \"a\",\n]\n[glass]\nwaybar = 12\n"
        );
        assert_eq!(
            with_setting(text, "dock_glass", "dock_glass = false"),
            "clock_24h = true\ndock = [\n    \"a\",\n]\ndock_glass = false\n[glass]\nwaybar = 12\n"
        );
    }

    #[test]
    fn times_of_day_are_read_and_written_as_hours_and_minutes() {
        assert_eq!(TimeOfDay::try_from("07:30".to_string()), Ok(TimeOfDay(450)));
        assert_eq!(String::from(TimeOfDay(22 * 60)), "22:00");
        for bad in ["24:00", "7:60", "7", "noon"] {
            assert!(TimeOfDay::try_from(bad.to_string()).is_err(), "{bad}");
        }
    }

    #[test]
    fn an_unset_setting_is_taken_out() {
        let text = "scale = 1.5\ndock = [\n    \"a\",\n]\n[glass]\nwaybar = 12\n";
        assert_eq!(
            with_setting(text, "dock", ""),
            "scale = 1.5\n[glass]\nwaybar = 12\n"
        );
        assert_eq!(with_setting(text, "mode", ""), text);
    }

    #[test]
    fn stacks_are_written_back_as_they_are_read() {
        let stack = Stack {
            path: "~/下載".into(),
            sort: Sort::Kind,
            show: Show::Folder,
            view: View::Grid,
        };
        let stacks = vec![stack.clone(), Stack::new("~/Downloads".into())];
        let config: Config = toml::from_str(&stacks_line(&stacks)).unwrap();
        assert_eq!(config.stacks, stacks);
    }

    #[test]
    fn wanted_mode_parses_width_height_and_rate() {
        let wanted = |mode: &str| {
            Config {
                mode: Some(mode.into()),
                ..Config::default()
            }
            .wanted_mode()
        };
        assert_eq!(wanted("1920x1080"), Some(((1920, 1080), None)));
        assert_eq!(wanted("2560x1440@144"), Some(((2560, 1440), Some(144))));
    }

    #[test]
    fn wanted_mode_rejects_malformed_values() {
        for mode in ["1920", "x1080", "widexhigh", "1920x1080@", "1920x1080@fast"] {
            let config = Config {
                mode: Some(mode.into()),
                ..Config::default()
            };
            assert_eq!(config.wanted_mode(), None, "{mode}");
        }
    }

    #[test]
    fn glass_names_layers_with_their_corner_radius() {
        let config: Config = toml::from_str("[glass]\npanel = 0\ndock = 18").unwrap();
        assert_eq!(
            config.glass,
            HashMap::from([("panel".into(), 0), ("dock".into(), 18)])
        );
    }

    #[test]
    fn unknown_keys_are_rejected() {
        assert!(toml::from_str::<Config>("moed = \"1x1\"").is_err());
    }
}
