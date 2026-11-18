//! The terminal's settings, kept in `~/.config/nekoland/terminal.json` under Nekominal's names. A key missing from the
//! file takes its default, so a file from an older version still loads.

use super::lang::tr;
use std::{cell::RefCell, collections::BTreeMap, fs, io, path::PathBuf};

use gtk::glib;
use serde::{Deserialize, Serialize};

const FILE: &str = "nekoland/terminal.json";

pub const FONT_SIZE_RANGE: (f64, f64) = (6.0, 48.0);
pub const LINE_HEIGHT_RANGE: (f64, f64) = (1.0, 2.0);
pub const SCROLLBACK_RANGE: (f64, f64) = (0.0, 100_000.0);
pub const BLINK_RATE_RANGE: (f64, f64) = (200.0, 1_500.0);
pub const LARGE_PASTE_RANGE: (f64, f64) = (0.0, 97_656.0);
pub const CLOSE_THRESHOLD_RANGE: (f64, f64) = (1.0, 1_000.0);
pub const LONG_COMMAND_RANGE: (f64, f64) = (0.0, 86_400.0);
pub const PREVIEW_SIZE_RANGE: (f64, f64) = (160.0, 960.0);
/// Lower than this and the text is hard to read, as Nekominal holds it.
pub const OPACITY_RANGE: (f64, f64) = (0.2, 1.0);

/// A choice the settings show as one of a few named options.
pub trait Choice: Copy + PartialEq + 'static {
    const ALL: &'static [Self];
    fn label(self) -> &'static str;
}

/// Where the tabs are: on the bar, or in a sidebar as a list or as icons.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TabLayout {
    #[default]
    Top,
    List,
    Icons,
}

impl TabLayout {
    pub fn is_side(self) -> bool {
        self != Self::Top
    }
}

impl Choice for TabLayout {
    const ALL: &'static [Self] = &[Self::Top, Self::List, Self::Icons];

    fn label(self) -> &'static str {
        match self {
            Self::Top => tr("上方分頁"),
            Self::List => tr("側邊欄 · 清單"),
            Self::Icons => tr("側邊欄 · 圖示"),
        }
    }
}

/// How the list in the sidebar shows a tab.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TabStyle {
    #[default]
    Activity,
    Cards,
    Repo,
}

impl Choice for TabStyle {
    const ALL: &'static [Self] = &[Self::Activity, Self::Cards, Self::Repo];

    fn label(self) -> &'static str {
        match self {
            Self::Activity => tr("活動清單"),
            Self::Cards => tr("Agent 卡片"),
            Self::Repo => tr("依專案分組"),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CursorShape {
    #[default]
    Block,
    Bar,
    Underline,
}

impl Choice for CursorShape {
    const ALL: &'static [Self] = &[Self::Block, Self::Bar, Self::Underline];

    fn label(self) -> &'static str {
        match self {
            Self::Block => tr("區塊"),
            Self::Bar => tr("直線"),
            Self::Underline => tr("底線"),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RightClick {
    #[default]
    Smart,
    Menu,
}

impl Choice for RightClick {
    const ALL: &'static [Self] = &[Self::Smart, Self::Menu];

    fn label(self) -> &'static str {
        match self {
            Self::Smart => tr("複製或貼上"),
            Self::Menu => tr("顯示功能表"),
        }
    }
}

/// What the right of the bar shows while no agent runs in the pane in front.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NoAgent {
    #[default]
    Place,
    Hide,
}

impl Choice for NoAgent {
    const ALL: &'static [Self] = &[Self::Place, Self::Hide];

    fn label(self) -> &'static str {
        match self {
            Self::Place => tr("只留資料夾和分支"),
            Self::Hide => tr("整條隱藏"),
        }
    }
}

/// When the bell makes a sound.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BellSound {
    #[default]
    None,
    Background,
    Always,
}

impl Choice for BellSound {
    const ALL: &'static [Self] = &[Self::None, Self::Background, Self::Always];

    fn label(self) -> &'static str {
        match self {
            Self::None => tr("無"),
            Self::Background => tr("僅背景"),
            Self::Always => tr("一律"),
        }
    }
}

/// Text the command palette types into the pane, by its name.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Snippet {
    pub name: String,
    /// What it types, an Enter as `\r`.
    pub input: String,
}

/// What a program may do with the clipboard through OSC 52.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Osc52Policy {
    Off,
    #[default]
    Write,
    ReadWrite,
}

impl Choice for Osc52Policy {
    const ALL: &'static [Self] = &[Self::Off, Self::Write, Self::ReadWrite];

    fn label(self) -> &'static str {
        match self {
            Self::Off => tr("關閉"),
            Self::Write => tr("僅寫入"),
            Self::ReadWrite => tr("讀取與寫入"),
        }
    }
}

/// What a trigger does with a line of output that matches it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TriggerAction {
    #[default]
    Highlight,
    Notify,
    Badge,
    Send,
    Bell,
}

impl Choice for TriggerAction {
    const ALL: &'static [Self] = &[Self::Highlight, Self::Notify, Self::Badge, Self::Send, Self::Bell];

    fn label(self) -> &'static str {
        match self {
            Self::Highlight => tr("標示"),
            Self::Notify => tr("通知"),
            Self::Badge => tr("分頁徽章"),
            Self::Send => tr("送出文字"),
            Self::Bell => tr("響鈴"),
        }
    }
}

/// A regular expression matched against each line of output, and what it does when one matches, as Nekominal's
/// triggers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Trigger {
    pub name: String,
    pub pattern: String,
    pub enabled: bool,
    pub action: TriggerAction,
    /// The notification's text or the text sent: `$0` is the match, `$1` to `$9` its groups.
    pub text: String,
    /// The background of a highlight, `#rrggbb`.
    pub color: String,
    /// Text is sent only once the user allowed it in the file.
    pub allow_send: bool,
    /// Only while the pane runs this program; empty for any.
    pub program: String,
    /// Matches the line being written before its line break, for a prompt.
    pub instant: bool,
}

pub const DEFAULT_HIGHLIGHT: &str = "#7a5c00";
/// The colours a highlight can take, Nekominal's: amber, red, blue, green and purple.
pub const HIGHLIGHTS: [&str; 5] = [DEFAULT_HIGHLIGHT, "#7a1f2b", "#1f4f7a", "#2b6b3a", "#5b2b7a"];
/// As many rules as the settings keep.
pub const MAX_TRIGGERS: usize = 64;

impl Default for Trigger {
    fn default() -> Self {
        Self {
            name: String::new(),
            pattern: String::new(),
            enabled: true,
            action: TriggerAction::Highlight,
            text: "$0".to_string(),
            color: DEFAULT_HIGHLIGHT.to_string(),
            allow_send: false,
            program: String::new(),
            instant: false,
        }
    }
}

/// A shell a new tab can start, beside the ones found on the system.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Profile {
    pub name: String,
    pub program: String,
    /// The arguments, separated by spaces.
    pub args: String,
    /// Where it starts; empty for where the tab in front is.
    pub cwd: String,
    /// Its own colour scheme; None for the window's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color_scheme: Option<String>,
    /// Its own font size; None for the window's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub font_size: Option<f64>,
    /// Variables its shell gets besides the others.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Config {
    pub language: super::lang::Language,
    pub tab_layout: TabLayout,
    pub tab_style: TabStyle,
    pub sidebar_open: bool,
    pub group_by_state: bool,
    pub scheme: String,
    /// Colour schemes of the user's own, imported from Windows Terminal or iTerm2; listed after the built-in ones.
    pub schemes: Vec<super::scheme_import::OwnScheme>,
    /// How much of the window's background covers what is behind it, in [`OPACITY_RANGE`].
    pub opacity: f64,
    /// Families in order of preference, separated by commas.
    pub font_family: String,
    pub font_size: f64,
    pub line_height: f64,
    /// Operator runs such as `=>` drawn as the font's ligatures.
    pub ligatures: bool,
    pub cursor_shape: CursorShape,
    pub cursor_blink: bool,
    /// Milliseconds the cursor is shown, and as many hidden.
    pub cursor_blink_rate: u32,
    pub scrollback: u32,
    pub copy_on_select: bool,
    /// While the view is scrolled up into a command's output, its prompt line is pinned over the top row.
    pub sticky_header: bool,
    pub right_click: RightClick,
    pub warn_multiline: bool,
    /// KiB from which a paste is confirmed; 0 never.
    pub warn_large_kib: u32,
    pub bell_visual: bool,
    /// The Dock icon asks for attention when the window is not active.
    pub bell_taskbar: bool,
    pub bell_sound: BellSound,
    pub confirm_close: bool,
    /// How many running programs a close must end before it asks.
    pub confirm_close_threshold: u32,
    pub restore_session: bool,
    pub snippets: Vec<Snippet>,
    pub notifications: bool,
    /// Seconds a command must run for its end to be notified; 0 never.
    pub long_command: u32,
    /// The same notifications sent on to a phone.
    pub push: super::push::Push,
    pub profiles: Vec<Profile>,
    /// The name of the profile a new tab starts; empty for the login shell.
    pub default_profile: String,
    pub agent_environment: bool,
    pub shell_integration: bool,
    /// Programs may ask for the kitty keyboard protocol.
    pub kitty_keyboard: bool,
    pub osc52: Osc52Policy,
    /// The pieces on the right of the bar about the pane in front.
    pub status_bar: bool,
    pub status_no_agent: NoAgent,
    pub status_state: bool,
    pub status_model: bool,
    pub status_context: bool,
    pub status_cost: bool,
    pub status_time: bool,
    pub status_branch: bool,
    pub status_folder: bool,
    /// How a tab is named when the user has not named it (`super::title`).
    pub tab_title: String,
    pub preview_images: bool,
    /// The longest side of a preview, in pixels.
    pub preview_size: u32,
    pub triggers: Vec<Trigger>,
    /// The command that opens `file:line`, `{file}` and `{line}` standing for them; empty for the desktop's default.
    pub editor: String,
    /// New tabs get the environment the terminal started with, rather than a clean login one.
    pub inherit_environment: bool,
}

impl Default for Config {
    /// Nekominal's defaults.
    fn default() -> Self {
        Self {
            language: super::lang::Language::default(),
            tab_layout: TabLayout::Top,
            tab_style: TabStyle::Activity,
            sidebar_open: true,
            group_by_state: false,
            scheme: "Tokyo Night".to_string(),
            schemes: Vec::new(),
            // Nekominal starts at 0.72 over Windows' acrylic; GTK has no backdrop material to blur what shows through.
            opacity: 1.0,
            font_family: "JetBrains Mono, DejaVu Sans Mono, monospace".to_string(),
            font_size: 14.0,
            line_height: 1.1,
            ligatures: true,
            cursor_shape: CursorShape::Block,
            cursor_blink: true,
            cursor_blink_rate: 530,
            scrollback: 10_000,
            copy_on_select: false,
            sticky_header: true,
            right_click: RightClick::Smart,
            warn_multiline: true,
            warn_large_kib: 5,
            bell_visual: true,
            bell_taskbar: true,
            bell_sound: BellSound::None,
            confirm_close: true,
            confirm_close_threshold: 1,
            restore_session: false,
            snippets: Vec::new(),
            notifications: true,
            long_command: 10,
            push: super::push::Push::default(),
            profiles: Vec::new(),
            default_profile: String::new(),
            agent_environment: true,
            shell_integration: true,
            kitty_keyboard: true,
            osc52: Osc52Policy::Write,
            status_bar: true,
            status_no_agent: NoAgent::Place,
            tab_title: super::title::DEFAULT.to_string(),
            status_state: true,
            status_model: true,
            status_context: true,
            status_cost: true,
            status_time: true,
            status_branch: true,
            status_folder: true,
            preview_images: true,
            preview_size: 480,
            triggers: Vec::new(),
            editor: String::new(),
            inherit_environment: true,
        }
    }
}

thread_local! {
    static CURRENT: RefCell<Config> = RefCell::new(load());
    /// Counts the changes, so that what is built from the settings knows when to build again.
    static GENERATION: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Grows by one with every change of the settings.
pub fn generation() -> u64 {
    GENERATION.get()
}

pub fn path() -> PathBuf {
    glib::user_config_dir().join(FILE)
}

fn load() -> Config {
    match fs::read_to_string(path()) {
        Ok(text) => parse(&text),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Config::default(),
        Err(error) => {
            eprintln!("nekoland-terminal: could not read {}: {error}", path().display());
            Config::default()
        }
    }
}

fn parse(text: &str) -> Config {
    serde_json::from_str(text).unwrap_or_else(|error| {
        eprintln!("nekoland-terminal: {} is not valid, using the defaults: {error}", path().display());
        Config::default()
    })
}

pub fn save(config: &Config) -> io::Result<()> {
    let path = path();
    if let Some(folder) = path.parent() {
        fs::create_dir_all(folder)?;
    }
    let text = serde_json::to_string_pretty(config).map_err(io::Error::other)?;
    fs::write(path, text)
}

pub fn current() -> Config {
    CURRENT.with_borrow(Clone::clone)
}

/// Changes the settings and writes them out.
pub fn update(change: impl FnOnce(&mut Config)) {
    let config = CURRENT.with_borrow_mut(|config| {
        change(config);
        config.clone()
    });
    GENERATION.set(GENERATION.get() + 1);
    if let Err(error) = save(&config) {
        eprintln!("nekoland-terminal: could not save {}: {error}", path().display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_key_takes_its_default() {
        let config = parse(r#"{ "scheme": "Nord", "fontSize": 12 }"#);
        assert_eq!(config.scheme, "Nord");
        assert_eq!(config.font_size, 12.0);
        assert_eq!(config.line_height, Config::default().line_height);
        assert_eq!(config.tab_layout, TabLayout::Top);
    }

    #[test]
    fn settings_written_read_back_the_same() {
        let config = Config {
            tab_layout: TabLayout::List,
            tab_style: TabStyle::Cards,
            cursor_shape: CursorShape::Underline,
            right_click: RightClick::Menu,
            confirm_close: false,
            ..Config::default()
        };
        let text = serde_json::to_string(&config).unwrap();
        assert_eq!(parse(&text), config);
    }
}
