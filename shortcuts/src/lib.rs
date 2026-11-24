//! Nekoland's shortcuts, one table the compositor acts on and Settings shows and edits.
//!
//! An action is something Nekoland does: a system one it does itself, as switching windows,
//! or an application one it asks the window in front to do, as copying, by pressing the keys
//! that application does it with. A scheme gives the actions the keys macOS, Windows or a
//! Linux desktop has for them; the configuration can give any action other keys, scheme by
//! scheme, and an application other keys for its actions.

use std::collections::{BTreeMap, HashSet};
use std::fmt;

/// Whose keys the actions take.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Deserialize, serde::Serialize,
)]
#[serde(rename_all = "lowercase")]
pub enum Scheme {
    #[default]
    Linux,
    Macos,
    Windows,
}

impl Scheme {
    /// As Settings lists them.
    pub const ALL: [Scheme; 3] = [Scheme::Macos, Scheme::Windows, Scheme::Linux];

    /// As the configuration writes it.
    pub fn name(self) -> &'static str {
        match self {
            Scheme::Linux => "linux",
            Scheme::Macos => "macos",
            Scheme::Windows => "windows",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Scheme::Linux => "Linux",
            Scheme::Macos => "macOS",
            Scheme::Windows => "Windows",
        }
    }

    fn index(self) -> usize {
        match self {
            Scheme::Linux => 0,
            Scheme::Macos => 1,
            Scheme::Windows => 2,
        }
    }
}

/// The configuration's own keys for actions, by scheme and then by action, as `shortcuts`
/// has them: an empty list leaves the action without keys.
pub type Custom = BTreeMap<Scheme, BTreeMap<String, Vec<String>>>;

/// The keys applications do actions with where they are not the usual ones, by application
/// ID and then by action, as `app_shortcuts` has them: an empty string for none.
pub type AppKeys = BTreeMap<String, BTreeMap<String, String>>;

/// What a combination is when it is Super pressed and let go by itself.
pub const SUPER_ALONE: &str = "Super";

/// Modifiers and a key, the key named as XKB names its symbol: `a`, `3`, `Return`, `Left`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Combo {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub logo: bool,
    pub key: String,
}

/// Other names keys are written with, lowercase, and XKB's.
const KEY_NAMES: [(&str, &str); 44] = [
    ("enter", "Return"),
    ("return", "Return"),
    ("space", "space"),
    ("esc", "Escape"),
    ("escape", "Escape"),
    ("tab", "Tab"),
    ("backspace", "BackSpace"),
    ("delete", "Delete"),
    ("del", "Delete"),
    ("insert", "Insert"),
    ("home", "Home"),
    ("end", "End"),
    ("pageup", "Page_Up"),
    ("page_up", "Page_Up"),
    ("pagedown", "Page_Down"),
    ("page_down", "Page_Down"),
    ("left", "Left"),
    ("right", "Right"),
    ("up", "Up"),
    ("down", "Down"),
    ("print", "Print"),
    ("printscreen", "Print"),
    ("=", "equal"),
    ("equal", "equal"),
    ("-", "minus"),
    ("minus", "minus"),
    ("plus", "plus"),
    ("`", "grave"),
    ("grave", "grave"),
    (",", "comma"),
    ("comma", "comma"),
    (".", "period"),
    ("period", "period"),
    ("/", "slash"),
    ("slash", "slash"),
    (";", "semicolon"),
    ("semicolon", "semicolon"),
    ("'", "apostrophe"),
    ("[", "bracketleft"),
    ("bracketleft", "bracketleft"),
    ("]", "bracketright"),
    ("bracketright", "bracketright"),
    ("\\", "backslash"),
    ("backslash", "backslash"),
];

/// How keys whose names say little are shown, everywhere and on macOS.
const KEY_LABELS: [(&str, &str, &str); 23] = [
    ("Left", "←", "←"),
    ("Right", "→", "→"),
    ("Up", "↑", "↑"),
    ("Down", "↓", "↓"),
    ("Return", "Enter", "↩"),
    ("space", "Space", "Space"),
    ("BackSpace", "Backspace", "⌫"),
    ("Delete", "Delete", "⌦"),
    ("Tab", "Tab", "⇥"),
    ("Escape", "Esc", "⎋"),
    ("Page_Up", "PageUp", "PageUp"),
    ("Page_Down", "PageDown", "PageDown"),
    ("Print", "Print Screen", "Print Screen"),
    ("equal", "=", "="),
    ("minus", "-", "-"),
    ("plus", "+", "+"),
    ("grave", "`", "`"),
    ("comma", ",", ","),
    ("period", ".", "."),
    ("semicolon", ";", ";"),
    ("XF86Eject", "Eject", "⏏"),
    ("bracketleft", "[", "["),
    ("bracketright", "]", "]"),
];

fn key_name(token: &str) -> Option<String> {
    let lower = token.to_lowercase();
    if let Some((_, name)) = KEY_NAMES.iter().find(|(written, _)| *written == lower) {
        return Some(name.to_string());
    }
    let mut chars = token.chars();
    match (chars.next(), chars.next()) {
        (Some(only), None) if only.is_ascii_alphanumeric() => {
            return Some(only.to_ascii_lowercase().to_string())
        }
        _ => {}
    }
    if let Some(number) = lower
        .strip_prefix('f')
        .and_then(|number| number.parse::<u8>().ok())
    {
        return (1..=35).contains(&number).then(|| format!("F{number}"));
    }
    // Any other symbol by its XKB name, as XF86AudioMute; the compositor knows whether it is one.
    (!token.is_empty() && token.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
        .then(|| token.to_string())
}

impl Combo {
    /// `Ctrl+Shift+C`, `Super+Left`, or `Super` alone; None for what is not a combination.
    pub fn parse(text: &str) -> Option<Combo> {
        let tokens: Vec<&str> = text.split('+').map(str::trim).collect();
        let (key, modifiers) = tokens.split_last()?;
        let mut combo = Combo {
            ctrl: false,
            alt: false,
            shift: false,
            logo: false,
            key: String::new(),
        };
        for modifier in modifiers {
            match modifier.to_lowercase().as_str() {
                "ctrl" | "control" => combo.ctrl = true,
                "alt" | "option" => combo.alt = true,
                "shift" => combo.shift = true,
                "super" | "win" | "cmd" | "command" | "logo" | "meta" => combo.logo = true,
                _ => return None,
            }
        }
        combo.key = match key.to_lowercase().as_str() {
            "super" | "win" | "cmd" | "command" | "logo" | "meta" if modifiers.is_empty() => {
                SUPER_ALONE.into()
            }
            "ctrl" | "control" | "alt" | "option" | "shift" | "super" | "win" | "cmd" | "command"
            | "logo" | "meta" => return None,
            _ => key_name(key)?,
        };
        Some(combo)
    }

    /// How it is shown: `Super Shift 3`, or with macOS's symbols, `⇧⌘3`.
    pub fn label(&self, scheme: Scheme) -> String {
        let mac = scheme == Scheme::Macos;
        if self.key == SUPER_ALONE {
            return if mac { "⌘" } else { "Super" }.into();
        }
        let key = KEY_LABELS
            .iter()
            .find(|(name, ..)| *name == self.key)
            .map(|(_, usual, macos)| if mac { *macos } else { *usual }.to_string())
            .unwrap_or_else(|| {
                let mut chars = self.key.chars();
                match (chars.next(), chars.next()) {
                    (Some(only), None) => only.to_uppercase().to_string(),
                    _ => self.key.clone(),
                }
            });
        if mac {
            let mut label = String::new();
            for (on, symbol) in [
                (self.ctrl, "⌃"),
                (self.alt, "⌥"),
                (self.shift, "⇧"),
                (self.logo, "⌘"),
            ] {
                if on {
                    label.push_str(symbol);
                }
            }
            label + &key
        } else {
            let mut words: Vec<&str> = [
                (self.logo, "Super"),
                (self.ctrl, "Ctrl"),
                (self.alt, "Alt"),
                (self.shift, "Shift"),
            ]
            .into_iter()
            .filter_map(|(on, word)| on.then_some(word))
            .collect();
            words.push(&key);
            words.join(" ")
        }
    }

    fn without_shift(&self) -> Combo {
        Combo {
            shift: false,
            ..self.clone()
        }
    }

    /// The same keys with Shift too.
    pub fn with_shift(&self) -> Combo {
        Combo {
            shift: true,
            ..self.clone()
        }
    }
}

/// As the configuration writes it: `Ctrl+Alt+Shift+Super+key`.
impl fmt::Display for Combo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (on, word) in [
            (self.ctrl, "Ctrl"),
            (self.alt, "Alt"),
            (self.shift, "Shift"),
            (self.logo, "Super"),
        ] {
            if on {
                write!(f, "{word}+")?;
            }
        }
        f.write_str(&self.key)
    }
}

/// Where Settings lists an action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    General,
    Windows,
    Arrange,
    Desktops,
    Dock,
    Screenshots,
    Editing,
    Text,
}

impl Group {
    pub const ALL: [Group; 8] = [
        Group::General,
        Group::Windows,
        Group::Arrange,
        Group::Desktops,
        Group::Dock,
        Group::Screenshots,
        Group::Editing,
        Group::Text,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Group::General => "一般",
            Group::Windows => "視窗",
            Group::Arrange => "視窗排列",
            Group::Desktops => "桌面",
            Group::Dock => "Dock",
            Group::Screenshots => "截圖",
            Group::Editing => "編輯",
            Group::Text => "文字游標",
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct Action {
    pub id: &'static str,
    pub title: &'static str,
    pub group: Group,
    /// The keys each scheme gives it: Linux's, macOS's and Windows'.
    keys: [&'static [&'static str]; 3],
    /// For an application action, the keys applications do it with and the keys terminals
    /// do, where Ctrl with a letter goes to the program running in them: combinations one
    /// after another, separated by spaces, or none.
    native: Option<(&'static str, &'static str)>,
    /// Whether its keys with Shift too do it with Shift too, as moving the cursor does selecting.
    pub extends: bool,
}

impl Action {
    /// Whether the window in front does it, rather than Nekoland.
    pub fn is_app(&self) -> bool {
        self.native.is_some()
    }

    /// The keys `scheme` gives it.
    pub fn defaults(&self, scheme: Scheme) -> Vec<Combo> {
        self.keys[scheme.index()]
            .iter()
            .filter_map(|text| Combo::parse(text))
            .collect()
    }
}

const fn system(
    id: &'static str,
    title: &'static str,
    group: Group,
    keys: [&'static [&'static str]; 3],
) -> Action {
    Action {
        id,
        title,
        group,
        keys,
        native: None,
        extends: false,
    }
}

/// An application action, with keys on macOS alone: elsewhere applications take Ctrl with
/// the letter themselves.
const fn app(
    id: &'static str,
    title: &'static str,
    group: Group,
    mac: &'static [&'static str],
    usual: &'static str,
    terminal: &'static str,
) -> Action {
    Action {
        id,
        title,
        group,
        keys: [&[], mac, &[]],
        native: Some((usual, terminal)),
        extends: false,
    }
}

/// A cursor movement, which Shift turns into selecting.
const fn moving(
    id: &'static str,
    title: &'static str,
    mac: &'static [&'static str],
    usual: &'static str,
    terminal: &'static str,
) -> Action {
    let mut action = app(id, title, Group::Text, mac, usual, terminal);
    action.extends = true;
    action
}

/// A system action whose keys with Shift do it the other way round.
const fn moving_system(
    id: &'static str,
    title: &'static str,
    group: Group,
    keys: [&'static [&'static str]; 3],
) -> Action {
    let mut action = system(id, title, group, keys);
    action.extends = true;
    action
}

use Group::*;

static ACTIONS: &[Action] = &[
    system(
        "launcher",
        "啟動器",
        General,
        [
            &["Alt+Space", "Alt+F2"],
            &["Super+Space", "Alt+Space"],
            &["Super+S", "Super+R", "Super+Q"],
        ],
    ),
    system(
        "apps",
        "所有應用程式",
        General,
        [&["Super+A"], &["F4"], &["Super", "Ctrl+Escape"]],
    ),
    system(
        "terminal",
        "終端機",
        General,
        [&["Super+Return", "Ctrl+Alt+T"], &[], &["Ctrl+Alt+T"]],
    ),
    system(
        "drop-down-terminal",
        "下拉式終端機",
        General,
        [&["Super+grave"], &["Ctrl+grave"], &["Super+grave"]],
    ),
    system("settings", "設定", General, [&[], &[], &["Super+I"]]),
    system(
        "lock",
        "鎖定螢幕",
        General,
        [&["Super+L"], &["Ctrl+Super+Q"], &["Super+L"]],
    ),
    system("projection", "投影", General, [&["Super+P"], &[], &["Super+P"]]),
    system(
        "menu-bar",
        "選單列",
        General,
        [&["Ctrl+F2", "F10"], &["Ctrl+F2"], &["Ctrl+F2"]],
    ),
    system(
        "dock",
        "Dock",
        General,
        [&["Ctrl+F3"], &["Ctrl+F3"], &["Ctrl+F3", "Super+T"]],
    ),
    system(
        "control-center",
        "控制中心",
        General,
        [&["Super+S"], &["Ctrl+F8"], &["Super+A"]],
    ),
    system(
        "notification-center",
        "通知中心",
        General,
        [&["Super+V"], &[], &["Super+N"]],
    ),
    system(
        "files",
        "檔案",
        General,
        [&["Super+E"], &["Alt+Super+Space"], &["Super+E"]],
    ),
    system(
        "task-manager",
        "工作管理員",
        General,
        [
            &["Ctrl+Shift+Escape"],
            &["Alt+Super+Escape"],
            &["Ctrl+Shift+Escape"],
        ],
    ),
    system(
        "input-method",
        "切換輸入法",
        General,
        [&["Super+Space"], &["Ctrl+Space"], &["Super+Space"]],
    ),
    system(
        "log-out",
        "登出",
        General,
        [&["Ctrl+Alt+Delete"], &["Shift+Super+Q"], &[]],
    ),
    system(
        "screen-reader",
        "螢幕閱讀器",
        General,
        [&["Alt+Super+S"], &["Super+F5"], &["Ctrl+Super+Return"]],
    ),
    system(
        "zoom-toggle",
        "螢幕放大開關",
        General,
        [&["Alt+Super+8"], &["Alt+Super+8"], &["Super+Escape"]],
    ),
    system(
        "zoom-in",
        "放大螢幕",
        General,
        [
            &["Alt+Super+equal", "Alt+Super+plus"],
            &["Alt+Super+equal"],
            &["Super+equal"],
        ],
    ),
    system(
        "zoom-out",
        "縮小螢幕",
        General,
        [&["Alt+Super+minus"], &["Alt+Super+minus"], &["Super+minus"]],
    ),
    system(
        "emoji",
        "表情符號",
        General,
        [
            &["Super+period"],
            &["Ctrl+Super+space"],
            &["Super+period", "Super+semicolon"],
        ],
    ),
    system("clipboard-history", "剪貼簿歷史", General, [&[], &[], &["Super+V"]]),
    system("quick-link", "快速連結", General, [&[], &[], &["Super+X"]]),
    system("widgets", "小工具", General, [&[], &[], &["Super+W"]]),
    system("quick-note", "快速備忘錄", General, [&[], &[], &[]]),
    system("date-time", "日期與時間", General, [&[], &[], &["Alt+Super+D"]]),
    system("do-not-disturb", "勿擾", General, [&[], &[], &[]]),
    system("dock-autohide", "自動隱藏 Dock", General, [&[], &["Alt+Super+D"], &[]]),
    system(
        "record-screen",
        "錄製螢幕",
        General,
        [&["Ctrl+Alt+Shift+R"], &[], &["Shift+Super+R"]],
    ),
    system("dictation", "語音輸入", General, [&[], &[], &["Super+H"]]),
    system("cast", "投放到無線顯示器", General, [&[], &[], &["Super+K"]]),
    system("rotation-lock", "旋轉鎖定", General, [&[], &[], &["Super+O"]]),
    system(
        "on-screen-keyboard",
        "螢幕鍵盤",
        General,
        [&[], &[], &["Ctrl+Super+O"]],
    ),
    system(
        "invert-colors",
        "反轉顏色",
        General,
        [&[], &["Ctrl+Alt+Super+8"], &["Ctrl+Super+C"]],
    ),
    system(
        "accessibility",
        "協助工具",
        General,
        [&[], &["Alt+Super+F5"], &["Super+U"]],
    ),
    // GNOME's: takes the keys back from a window that asked for all of them.
    system(
        "restore-shortcuts",
        "取回系統快速鍵",
        General,
        [&["Super+Escape"], &[], &[]],
    ),
    system("about", "關於這部電腦", General, [&[], &[], &["Super+Pause"]]),
    system("security", "安全選項", General, [&[], &[], &["Ctrl+Alt+Delete"]]),
    system("sleep", "睡眠", General, [&[], &["Alt+Super+XF86Eject"], &[]]),
    system("restart", "重新開機", General, [&[], &["Ctrl+Super+XF86Eject"], &[]]),
    system(
        "shut-down",
        "關機",
        General,
        [&[], &["Ctrl+Alt+Super+XF86Eject"], &[]],
    ),
    system("screens-off", "關閉螢幕", General, [&[], &["Ctrl+Shift+XF86Eject"], &[]]),
    // Shift goes the other way, as it does on each of them.
    moving_system(
        "switch-window",
        "切換視窗",
        Windows,
        [&["Alt+Tab", "Super+Tab"], &["Super+Tab"], &["Alt+Tab"]],
    ),
    system(
        "switch-window-held",
        "切換視窗（不需按住）",
        Windows,
        [&[], &[], &["Ctrl+Alt+Tab"]],
    ),
    moving_system(
        "cycle-windows",
        "依序切換視窗",
        Windows,
        [&["Alt+Escape"], &["Ctrl+F4"], &["Alt+Escape"]],
    ),
    // KDE's Alt+F3, as Alt+Space is the launcher's there.
    system(
        "window-menu",
        "視窗選單",
        Windows,
        [&["Alt+F3"], &[], &["Alt+space"]],
    ),
    system("move-window", "移動視窗", Windows, [&["Alt+F7"], &[], &[]]),
    system("resize-window", "調整視窗大小", Windows, [&["Alt+F8"], &[], &[]]),
    system("peek-desktop", "預覽桌面", Windows, [&[], &[], &["Super+comma"]]),
    system("snap-layouts", "貼齊版面", Windows, [&[], &[], &["Super+Z"]]),
    system(
        "overview",
        "所有視窗",
        Windows,
        [&["Ctrl+Alt+Up", "Super"], &["Ctrl+Up", "F3"], &["Super+Tab"]],
    ),
    system(
        "close-window",
        "關閉視窗",
        Windows,
        [&["Alt+F4", "Super+Q"], &[], &["Alt+F4"]],
    ),
    system("quit-app", "結束應用程式", Windows, [&[], &["Super+Q"], &[]]),
    system("app-windows", "應用程式視窗", Windows, [&[], &["Ctrl+Down"], &[]]),
    system(
        "next-app-window",
        "同一應用程式的下一個視窗",
        Windows,
        [&["Alt+grave"], &["Super+grave"], &[]],
    ),
    system("minimize", "最小化", Windows, [&["Super+H"], &["Super+M"], &[]]),
    system(
        "hide-app",
        "隱藏應用程式",
        Windows,
        [&[], &["Super+H", "Alt+Super+M"], &[]],
    ),
    system(
        "hide-others",
        "隱藏其他",
        Windows,
        [&[], &["Alt+Super+H"], &["Super+Home"]],
    ),
    system("show-all", "顯示全部", Windows, [&[], &[], &["Shift+Super+M"]]),
    system(
        "show-desktop",
        "顯示桌面",
        Windows,
        [&["Super+D"], &["F11", "Super+F3"], &["Super+D", "Super+M"]],
    ),
    system("toggle-maximize", "縮放", Windows, [&["Alt+F10"], &[], &[]]),
    system("sticky", "在所有桌面顯示", Windows, [&[], &[], &[]]),
    system(
        "fullscreen",
        "全螢幕",
        Windows,
        [&["F11"], &["Ctrl+Super+F"], &["F11"]],
    ),
    // Windows' keys for the halves and the rest as Linux's; macOS's are Rectangle's.
    system(
        "arrange-left",
        "左半邊",
        Arrange,
        [&["Super+Left"], &["Ctrl+Alt+Left"], &["Super+Left"]],
    ),
    system(
        "arrange-right",
        "右半邊",
        Arrange,
        [&["Super+Right"], &["Ctrl+Alt+Right"], &["Super+Right"]],
    ),
    system(
        "arrange-top",
        "上半部",
        Arrange,
        [&["Alt+Super+Up"], &["Ctrl+Alt+Up"], &["Alt+Super+Up"]],
    ),
    system(
        "arrange-bottom",
        "下半部",
        Arrange,
        [&["Alt+Super+Down"], &["Ctrl+Alt+Down"], &["Alt+Super+Down"]],
    ),
    system(
        "arrange-maximize",
        "最大化",
        Arrange,
        [
            &["Super+Up", "Alt+Super+Return"],
            &["Ctrl+Alt+Return"],
            &["Super+Up", "Alt+Super+Return"],
        ],
    ),
    system(
        "arrange-maximize-height",
        "最大化高度",
        Arrange,
        [
            &["Alt+Shift+Super+Up"],
            &["Ctrl+Alt+Shift+Up"],
            &["Alt+Shift+Super+Up", "Shift+Super+Up"],
        ],
    ),
    // Rectangle's Ctrl+Option+Delete is the way out of Nekoland, Ctrl+Alt+Backspace, here.
    system(
        "arrange-restore",
        "回到先前的大小",
        Arrange,
        [
            &["Super+Down", "Alt+Super+BackSpace"],
            &["Ctrl+Alt+Shift+Down"],
            &["Alt+Super+BackSpace"],
        ],
    ),
    // Windows' Super+Down: a window put somewhere goes back, any other to the Dock.
    system(
        "arrange-restore-or-minimize",
        "還原或最小化",
        Arrange,
        [&[], &[], &["Super+Down"]],
    ),
    system(
        "arrange-display-left",
        "移到左邊的顯示器",
        Arrange,
        [
            &["Shift+Super+Left"],
            &["Ctrl+Alt+Super+Left"],
            &["Shift+Super+Left"],
        ],
    ),
    system(
        "arrange-display-right",
        "移到右邊的顯示器",
        Arrange,
        [
            &["Shift+Super+Right"],
            &["Ctrl+Alt+Super+Right"],
            &["Shift+Super+Right"],
        ],
    ),
    system(
        "arrange-display-up",
        "移到上方的顯示器",
        Arrange,
        [&["Shift+Super+Up"], &["Ctrl+Alt+Super+Up"], &[]],
    ),
    system(
        "arrange-display-down",
        "移到下方的顯示器",
        Arrange,
        [&["Shift+Super+Down"], &["Ctrl+Alt+Super+Down"], &[]],
    ),
    system(
        "arrange-top-left",
        "左上角",
        Arrange,
        [&["Alt+Super+U"], &["Ctrl+Alt+U"], &["Alt+Super+U"]],
    ),
    system(
        "arrange-top-right",
        "右上角",
        Arrange,
        [&["Alt+Super+I"], &["Ctrl+Alt+I"], &["Alt+Super+I"]],
    ),
    system(
        "arrange-bottom-left",
        "左下角",
        Arrange,
        [&["Alt+Super+J"], &["Ctrl+Alt+J"], &["Alt+Super+J"]],
    ),
    system(
        "arrange-bottom-right",
        "右下角",
        Arrange,
        [&["Alt+Super+K"], &["Ctrl+Alt+K"], &["Alt+Super+K"]],
    ),
    system(
        "arrange-first-third",
        "左三分之一",
        Arrange,
        // Windows' Alt+Super+D is the clock's, so its thirds start a key along.
        [&["Alt+Super+D"], &["Ctrl+Alt+D"], &["Alt+Super+A"]],
    ),
    system(
        "arrange-center-third",
        "中間三分之一",
        Arrange,
        [&["Alt+Super+F"], &["Ctrl+Alt+F"], &["Alt+Super+F"]],
    ),
    system(
        "arrange-last-third",
        "右三分之一",
        Arrange,
        [&["Alt+Super+G"], &["Ctrl+Alt+G"], &["Alt+Super+G"]],
    ),
    system(
        "arrange-first-two-thirds",
        "左三分之二",
        Arrange,
        [&["Alt+Super+E"], &["Ctrl+Alt+E"], &["Alt+Super+E"]],
    ),
    system(
        "arrange-last-two-thirds",
        "右三分之二",
        Arrange,
        [&["Alt+Super+T"], &["Ctrl+Alt+T"], &["Alt+Super+T"]],
    ),
    system(
        "arrange-center",
        "置中",
        Arrange,
        [&["Alt+Super+C"], &["Ctrl+Alt+C"], &["Alt+Super+C"]],
    ),
    system(
        "arrange-larger",
        "加大",
        Arrange,
        [
            &["Alt+Shift+Super+equal"],
            &["Ctrl+Alt+equal"],
            &["Alt+Super+equal"],
        ],
    ),
    system(
        "arrange-smaller",
        "縮小",
        Arrange,
        [
            &["Alt+Shift+Super+minus"],
            &["Ctrl+Alt+minus"],
            &["Alt+Super+minus"],
        ],
    ),
    system(
        "workspace-left",
        "上一個桌面",
        Desktops,
        [
            &["Ctrl+Super+Left", "Ctrl+Alt+Left", "Super+Page_Up"],
            &["Ctrl+Left"],
            &["Ctrl+Super+Left"],
        ],
    ),
    system(
        "workspace-right",
        "下一個桌面",
        Desktops,
        [
            &["Ctrl+Super+Right", "Ctrl+Alt+Right", "Super+Page_Down"],
            &["Ctrl+Right"],
            &["Ctrl+Super+Right"],
        ],
    ),
    system(
        "workspace-first",
        "第一個桌面",
        Desktops,
        [&["Super+Home"], &[], &[]],
    ),
    system(
        "workspace-last",
        "最後一個桌面",
        Desktops,
        [&["Super+End"], &[], &[]],
    ),
    system("workspace-1", "桌面 1", Desktops, [&[], &["Ctrl+1"], &[]]),
    system("workspace-2", "桌面 2", Desktops, [&[], &["Ctrl+2"], &[]]),
    system("workspace-3", "桌面 3", Desktops, [&[], &["Ctrl+3"], &[]]),
    system("workspace-4", "桌面 4", Desktops, [&[], &["Ctrl+4"], &[]]),
    system("workspace-5", "桌面 5", Desktops, [&[], &["Ctrl+5"], &[]]),
    system("workspace-6", "桌面 6", Desktops, [&[], &["Ctrl+6"], &[]]),
    system("workspace-7", "桌面 7", Desktops, [&[], &["Ctrl+7"], &[]]),
    system("workspace-8", "桌面 8", Desktops, [&[], &["Ctrl+8"], &[]]),
    system("workspace-9", "桌面 9", Desktops, [&[], &["Ctrl+9"], &[]]),
    system(
        "new-workspace",
        "新增桌面",
        Desktops,
        [&["Ctrl+Super+D"], &[], &["Ctrl+Super+D"]],
    ),
    system(
        "close-workspace",
        "關閉桌面",
        Desktops,
        [&["Ctrl+Super+F4"], &[], &["Ctrl+Super+F4"]],
    ),
    system(
        "carry-left",
        "視窗移到上一個桌面",
        Desktops,
        [
            &[
                "Ctrl+Shift+Super+Left",
                "Shift+Super+Page_Up",
                "Ctrl+Alt+Shift+Left",
            ],
            &["Ctrl+Shift+Left"],
            &["Ctrl+Shift+Super+Left"],
        ],
    ),
    system(
        "carry-right",
        "視窗移到下一個桌面",
        Desktops,
        [
            &[
                "Ctrl+Shift+Super+Right",
                "Shift+Super+Page_Down",
                "Ctrl+Alt+Shift+Right",
            ],
            &["Ctrl+Shift+Right"],
            &["Ctrl+Shift+Super+Right"],
        ],
    ),
    system(
        "carry-first",
        "視窗移到第一個桌面",
        Desktops,
        [&["Shift+Super+Home"], &[], &[]],
    ),
    system(
        "carry-last",
        "視窗移到最後一個桌面",
        Desktops,
        [&["Shift+Super+End"], &[], &[]],
    ),
    system(
        "dock-1",
        "第 1 個應用程式",
        Dock,
        [&["Super+1"], &[], &["Super+1"]],
    ),
    system(
        "dock-2",
        "第 2 個應用程式",
        Dock,
        [&["Super+2"], &[], &["Super+2"]],
    ),
    system(
        "dock-3",
        "第 3 個應用程式",
        Dock,
        [&["Super+3"], &[], &["Super+3"]],
    ),
    system(
        "dock-4",
        "第 4 個應用程式",
        Dock,
        [&["Super+4"], &[], &["Super+4"]],
    ),
    system(
        "dock-5",
        "第 5 個應用程式",
        Dock,
        [&["Super+5"], &[], &["Super+5"]],
    ),
    system(
        "dock-6",
        "第 6 個應用程式",
        Dock,
        [&["Super+6"], &[], &["Super+6"]],
    ),
    system(
        "dock-7",
        "第 7 個應用程式",
        Dock,
        [&["Super+7"], &[], &["Super+7"]],
    ),
    system(
        "dock-8",
        "第 8 個應用程式",
        Dock,
        [&["Super+8"], &[], &["Super+8"]],
    ),
    system(
        "dock-9",
        "第 9 個應用程式",
        Dock,
        [&["Super+9"], &[], &["Super+9"]],
    ),
    system(
        "dock-new-1",
        "第 1 個應用程式的新視窗",
        Dock,
        [&[], &[], &["Shift+Super+1"]],
    ),
    system(
        "dock-new-2",
        "第 2 個應用程式的新視窗",
        Dock,
        [&[], &[], &["Shift+Super+2"]],
    ),
    system(
        "dock-new-3",
        "第 3 個應用程式的新視窗",
        Dock,
        [&[], &[], &["Shift+Super+3"]],
    ),
    system(
        "dock-new-4",
        "第 4 個應用程式的新視窗",
        Dock,
        [&[], &[], &["Shift+Super+4"]],
    ),
    system(
        "dock-new-5",
        "第 5 個應用程式的新視窗",
        Dock,
        [&[], &[], &["Shift+Super+5"]],
    ),
    system(
        "dock-new-6",
        "第 6 個應用程式的新視窗",
        Dock,
        [&[], &[], &["Shift+Super+6"]],
    ),
    system(
        "dock-new-7",
        "第 7 個應用程式的新視窗",
        Dock,
        [&[], &[], &["Shift+Super+7"]],
    ),
    system(
        "dock-new-8",
        "第 8 個應用程式的新視窗",
        Dock,
        [&[], &[], &["Shift+Super+8"]],
    ),
    system(
        "dock-new-9",
        "第 9 個應用程式的新視窗",
        Dock,
        [&[], &[], &["Shift+Super+9"]],
    ),
    system(
        "dock-menu-1",
        "第 1 個應用程式的選單",
        Dock,
        [&[], &[], &["Alt+Super+1"]],
    ),
    system(
        "dock-menu-2",
        "第 2 個應用程式的選單",
        Dock,
        [&[], &[], &["Alt+Super+2"]],
    ),
    system(
        "dock-menu-3",
        "第 3 個應用程式的選單",
        Dock,
        [&[], &[], &["Alt+Super+3"]],
    ),
    system(
        "dock-menu-4",
        "第 4 個應用程式的選單",
        Dock,
        [&[], &[], &["Alt+Super+4"]],
    ),
    system(
        "dock-menu-5",
        "第 5 個應用程式的選單",
        Dock,
        [&[], &[], &["Alt+Super+5"]],
    ),
    system(
        "dock-menu-6",
        "第 6 個應用程式的選單",
        Dock,
        [&[], &[], &["Alt+Super+6"]],
    ),
    system(
        "dock-menu-7",
        "第 7 個應用程式的選單",
        Dock,
        [&[], &[], &["Alt+Super+7"]],
    ),
    system(
        "dock-menu-8",
        "第 8 個應用程式的選單",
        Dock,
        [&[], &[], &["Alt+Super+8"]],
    ),
    system(
        "dock-menu-9",
        "第 9 個應用程式的選單",
        Dock,
        [&[], &[], &["Alt+Super+9"]],
    ),
    system(
        "shot-full",
        "全螢幕截圖",
        Screenshots,
        [
            &["Shift+Super+3", "Shift+Print"],
            &["Shift+Super+3"],
            &["Super+Print"],
        ],
    ),
    system(
        "shot-full-copy",
        "全螢幕截圖到剪貼簿",
        Screenshots,
        [&["Ctrl+Shift+Super+3"], &["Ctrl+Shift+Super+3"], &[]],
    ),
    // Taken as the button comes up, as the Mac's ⇧⌘4.
    system(
        "shot-area",
        "截取範圍",
        Screenshots,
        [&["Shift+Super+4"], &["Shift+Super+4"], &[]],
    ),
    system(
        "shot-area-copy",
        "截取範圍到剪貼簿",
        Screenshots,
        [&["Ctrl+Shift+Super+4"], &["Ctrl+Shift+Super+4"], &[]],
    ),
    // The strip of tools, as the Mac's ⇧⌘5, GNOME's Print and Windows' Snipping Tool.
    system(
        "shot-tools",
        "截圖與錄影選項",
        Screenshots,
        [
            &["Print"],
            &["Shift+Super+5", "Print"],
            &["Shift+Super+S", "Print"],
        ],
    ),
    system(
        "shot-window",
        "截取視窗",
        Screenshots,
        [
            &["Shift+Super+5", "Alt+Print"],
            &[],
            &["Alt+Print"],
        ],
    ),
    system(
        "shot-scroll",
        "捲動截圖",
        Screenshots,
        [&["Shift+Super+6"], &["Shift+Super+6"], &[]],
    ),
    system(
        "shot-repeat",
        "重複上次截圖",
        Screenshots,
        [&["Shift+Super+R"], &["Shift+Super+R"], &[]],
    ),
    system(
        "shot-ocr",
        "擷取文字",
        Screenshots,
        [&["Shift+Super+O"], &["Shift+Super+O"], &["Shift+Super+T"]],
    ),
    system(
        "shot-clipboard",
        "從剪貼簿載入",
        Screenshots,
        [&["Shift+Super+V"], &["Shift+Super+V"], &[]],
    ),
    system(
        "shot-delay",
        "延遲截圖",
        Screenshots,
        [&["Shift+Super+D"], &["Shift+Super+D"], &[]],
    ),
    app("copy", "拷貝", Editing, &["Super+C"], "Ctrl+C", "Ctrl+Shift+C"),
    app("cut", "剪下", Editing, &["Super+X"], "Ctrl+X", ""),
    app("paste", "貼上", Editing, &["Super+V"], "Ctrl+V", "Ctrl+Shift+V"),
    app("undo", "還原", Editing, &["Super+Z"], "Ctrl+Z", ""),
    app("redo", "重做", Editing, &["Shift+Super+Z"], "Ctrl+Shift+Z", ""),
    app(
        "select-all",
        "全選",
        Editing,
        &["Super+A"],
        "Ctrl+A",
        "Ctrl+Shift+A",
    ),
    app("find", "尋找", Editing, &["Super+F"], "Ctrl+F", "Ctrl+Shift+F"),
    app("find-next", "尋找下一個", Editing, &["Super+G"], "Ctrl+G", ""),
    app(
        "find-previous",
        "尋找上一個",
        Editing,
        &["Shift+Super+G"],
        "Ctrl+Shift+G",
        "",
    ),
    app("replace", "取代", Editing, &["Alt+Super+F"], "Ctrl+H", ""),
    app("save", "儲存", Editing, &["Super+S"], "Ctrl+S", ""),
    app(
        "save-as",
        "另存新檔",
        Editing,
        &["Shift+Super+S"],
        "Ctrl+Shift+S",
        "",
    ),
    app("open", "打開", Editing, &["Super+O"], "Ctrl+O", ""),
    app("new", "新增", Editing, &["Super+N"], "Ctrl+N", "Ctrl+Shift+N"),
    app(
        "new-tab",
        "新增分頁",
        Editing,
        &["Super+T"],
        "Ctrl+T",
        "Ctrl+Shift+T",
    ),
    app(
        "reopen-tab",
        "重新打開分頁",
        Editing,
        &["Shift+Super+T"],
        "Ctrl+Shift+T",
        "",
    ),
    app(
        "next-tab",
        "下一個分頁",
        Editing,
        &["Shift+Super+bracketright", "Alt+Super+Right"],
        "Ctrl+Page_Down",
        "Ctrl+Page_Down",
    ),
    app(
        "previous-tab",
        "上一個分頁",
        Editing,
        &["Shift+Super+bracketleft", "Alt+Super+Left"],
        "Ctrl+Page_Up",
        "Ctrl+Page_Up",
    ),
    app(
        "close",
        "關閉分頁或視窗",
        Editing,
        &["Super+W"],
        "Ctrl+W",
        "Ctrl+Shift+W",
    ),
    app("print", "列印", Editing, &["Super+P"], "Ctrl+P", ""),
    app("reload", "重新載入", Editing, &["Super+R"], "Ctrl+R", ""),
    app("address", "網址列", Editing, &["Super+L"], "Ctrl+L", ""),
    app("back", "上一頁", Editing, &["Super+bracketleft"], "Alt+Left", ""),
    app(
        "forward",
        "下一頁",
        Editing,
        &["Super+bracketright"],
        "Alt+Right",
        "",
    ),
    app(
        "text-larger",
        "放大",
        Editing,
        &["Super+equal", "Super+plus"],
        "Ctrl+equal",
        "Ctrl+equal",
    ),
    app(
        "text-smaller",
        "縮小",
        Editing,
        &["Super+minus"],
        "Ctrl+minus",
        "Ctrl+minus",
    ),
    app(
        "text-actual",
        "實際大小",
        Editing,
        &["Super+0"],
        "Ctrl+0",
        "Ctrl+0",
    ),
    app(
        "developer-tools",
        "開發者工具",
        Editing,
        &["Alt+Super+I"],
        "Ctrl+Shift+I",
        "",
    ),
    app(
        "preferences",
        "偏好設定",
        Editing,
        &["Super+comma"],
        "Ctrl+comma",
        "Ctrl+comma",
    ),
    moving("line-start", "行首", &["Super+Left", "Ctrl+A"], "Home", "Home"),
    moving("line-end", "行尾", &["Super+Right", "Ctrl+E"], "End", "End"),
    moving("document-start", "文件開頭", &["Super+Up"], "Ctrl+Home", ""),
    moving("document-end", "文件結尾", &["Super+Down"], "Ctrl+End", ""),
    moving("word-left", "上一個字", &["Alt+Left"], "Ctrl+Left", "Ctrl+Left"),
    moving(
        "word-right",
        "下一個字",
        &["Alt+Right"],
        "Ctrl+Right",
        "Ctrl+Right",
    ),
    moving("char-left", "前一個字元", &["Ctrl+B"], "Left", "Left"),
    moving("char-right", "後一個字元", &["Ctrl+F"], "Right", "Right"),
    moving("line-up", "上一行", &["Ctrl+P"], "Up", "Up"),
    moving("line-down", "下一行", &["Ctrl+N"], "Down", "Down"),
    moving("paragraph-start", "段落開頭", &["Alt+Up"], "Ctrl+Up", ""),
    moving("paragraph-end", "段落結尾", &["Alt+Down"], "Ctrl+Down", ""),
    app(
        "delete-word",
        "刪除前一個字",
        Text,
        &["Alt+BackSpace"],
        "Ctrl+BackSpace",
        "Ctrl+W",
    ),
    app(
        "delete-line",
        "刪除到行首",
        Text,
        &["Super+BackSpace"],
        "Shift+Home BackSpace",
        "Ctrl+U",
    ),
    app(
        "delete-forward",
        "刪除後一個字元",
        Text,
        &["Ctrl+D"],
        "Delete",
        "Delete",
    ),
    app(
        "delete-word-forward",
        "刪除後一個字",
        Text,
        &["Alt+Delete"],
        "Ctrl+Delete",
        "Alt+D",
    ),
    app(
        "delete-to-line-end",
        "刪除到行尾",
        Text,
        &["Ctrl+K"],
        "Shift+End Delete",
        "Ctrl+K",
    ),
    app(
        "backspace",
        "刪除前一個字元",
        Text,
        &["Ctrl+H"],
        "BackSpace",
        "BackSpace",
    ),
];

/// Every action, as Settings lists them within their groups.
pub fn actions() -> &'static [Action] {
    ACTIONS
}

pub fn action(id: &str) -> Option<&'static Action> {
    ACTIONS.iter().find(|action| action.id == id)
}

/// The keys the configuration gives `action` in `scheme`, if it gives it any.
fn own_keys(scheme: Scheme, custom: &Custom, action: &Action) -> Option<Vec<Combo>> {
    custom
        .get(&scheme)?
        .get(action.id)
        .map(|keys| keys.iter().filter_map(|text| Combo::parse(text)).collect())
}

/// The keys `action` has in `scheme`: the configuration's, or else its defaults without
/// those in `taken`, which the configuration gives to other actions.
fn keys_besides(scheme: Scheme, custom: &Custom, action: &Action, taken: &HashSet<Combo>) -> Vec<Combo> {
    own_keys(scheme, custom, action).unwrap_or_else(|| {
        action
            .defaults(scheme)
            .into_iter()
            .filter(|combo| !taken.contains(combo))
            .collect()
    })
}

/// Every key the configuration gives some action in `scheme`.
fn taken(scheme: Scheme, custom: &Custom) -> HashSet<Combo> {
    ACTIONS
        .iter()
        .filter_map(|action| own_keys(scheme, custom, action))
        .flatten()
        .collect()
}

/// Every action with its keys in `scheme`, the configuration's first: a key the
/// configuration gives one action is taken from the action the scheme gave it to.
pub fn bindings(scheme: Scheme, custom: &Custom) -> Vec<(&'static Action, Vec<Combo>)> {
    let taken = taken(scheme, custom);
    ACTIONS
        .iter()
        .map(|action| (action, keys_besides(scheme, custom, action, &taken)))
        .collect()
}

/// The keys `id` has in `scheme`, as `bindings` gives them, worked out for it alone.
pub fn keys(scheme: Scheme, custom: &Custom, id: &str) -> Vec<Combo> {
    let Some(action) = action(id) else {
        return Vec::new();
    };
    keys_besides(scheme, custom, action, &taken(scheme, custom))
}

/// Whether the configuration gives `id` keys of its own in `scheme`.
pub fn is_custom(scheme: Scheme, custom: &Custom, id: &str) -> bool {
    custom.get(&scheme).is_some_and(|own| own.contains_key(id))
}

/// The action `combo` does among `bindings`, as `bindings()` makes them once for a
/// configuration, and whether it was found as one that Shift extends, with Shift added to
/// its keys.
pub fn find(bindings: &[(&'static Action, Vec<Combo>)], combo: &Combo) -> Option<(&'static Action, bool)> {
    let bound = |combo: &Combo| {
        bindings
            .iter()
            .find(|(_, keys)| keys.contains(combo))
            .map(|(action, _)| *action)
    };
    if let Some(action) = bound(combo) {
        return Some((action, false));
    }
    combo
        .shift
        .then(|| bound(&combo.without_shift()))
        .flatten()
        .filter(|action| action.extends)
        .map(|action| (action, true))
}

/// Terminals, by what their application IDs have in them, where Ctrl with a letter goes
/// to the program running in them.
// shortcut: known by name; read their desktop entry's TerminalEmulator category once one
// is missed.
const TERMINALS: [&str; 12] = [
    "terminal",
    "foot",
    "kitty",
    "alacritty",
    "ptyxis",
    "konsole",
    "wezterm",
    "xterm",
    "ghostty",
    "tilix",
    "terminator",
    "org.gnome.console",
];

pub fn is_terminal(app_id: &str) -> bool {
    let app_id = app_id.to_lowercase();
    TERMINALS.iter().any(|name| app_id.contains(name))
}

/// The keys the application `app_id` does an application action with, one combination
/// after another: none for a system action, or one the application has no keys for.
pub fn native(action: &Action, app_id: &str, apps: &AppKeys) -> Vec<Combo> {
    let Some((usual, terminal)) = action.native else {
        return Vec::new();
    };
    let keys = match apps.get(app_id).and_then(|keys| keys.get(action.id)) {
        Some(own) => own.as_str(),
        None if is_terminal(app_id) => terminal,
        None => usual,
    };
    keys.split_whitespace().filter_map(Combo::parse).collect()
}

/// What macOS' ⌘ with a key that no action has stands for in other applications, which
/// take Ctrl where the Mac takes ⌘: Ctrl, or Ctrl+Shift in a terminal, where Ctrl with a
/// letter goes to the program running in it.
pub fn command_key(combo: &Combo, app_id: &str) -> Option<Combo> {
    if !combo.logo || combo.ctrl || combo.key == SUPER_ALONE {
        return None;
    }
    Some(Combo {
        ctrl: true,
        logo: false,
        shift: combo.shift || is_terminal(app_id),
        ..combo.clone()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn combo(text: &str) -> Combo {
        Combo::parse(text).unwrap()
    }

    #[test]
    fn combinations_are_read_in_any_spelling_and_written_one_way() {
        assert_eq!(combo("cmd+shift+3"), combo("Shift+Super+3"));
        assert_eq!(combo("Ctrl+Alt+Enter").key, "Return");
        assert_eq!(combo("Super+=").key, "equal");
        assert_eq!(combo("Win+PageUp").to_string(), "Super+Page_Up");
        assert_eq!(
            combo("Super+Ctrl+Shift+Left").to_string(),
            "Ctrl+Shift+Super+Left"
        );
        assert_eq!(combo("Super").key, SUPER_ALONE);
        assert_eq!(Combo::parse("Ctrl+Shift"), None);
        assert_eq!(Combo::parse("Hyper+A"), None);
        assert_eq!(Combo::parse(""), None);
    }

    #[test]
    fn labels_follow_the_scheme() {
        assert_eq!(combo("Shift+Super+3").label(Scheme::Macos), "⇧⌘3");
        assert_eq!(combo("Ctrl+Alt+Return").label(Scheme::Macos), "⌃⌥↩");
        assert_eq!(combo("Ctrl+Super+Left").label(Scheme::Linux), "Super Ctrl ←");
        assert_eq!(combo("Super").label(Scheme::Windows), "Super");
    }

    #[test]
    fn every_scheme_gives_each_combination_to_one_action() {
        for scheme in Scheme::ALL {
            let mut seen = HashSet::new();
            for action in ACTIONS {
                assert_eq!(
                    action.keys[scheme.index()].len(),
                    action.defaults(scheme).len(),
                    "{}",
                    action.id
                );
                for combo in action.defaults(scheme) {
                    assert!(seen.insert(combo.clone()), "{} twice in {scheme:?}", combo);
                }
            }
        }
        let ids: HashSet<&str> = ACTIONS.iter().map(|action| action.id).collect();
        assert_eq!(ids.len(), ACTIONS.len());
    }

    #[test]
    fn natives_parse() {
        for action in ACTIONS {
            if let Some((usual, terminal)) = action.native {
                for keys in [usual, terminal] {
                    assert_eq!(
                        keys.split_whitespace().count(),
                        native_of(keys).len(),
                        "{}",
                        action.id
                    );
                }
            }
        }
    }

    fn native_of(keys: &str) -> Vec<Combo> {
        keys.split_whitespace().filter_map(Combo::parse).collect()
    }

    #[test]
    fn linux_keeps_ctrl_for_applications_and_macos_takes_super() {
        let none = Custom::new();
        assert_eq!(
            find(&bindings(Scheme::Linux, &none), &combo("Ctrl+C")).map(|(action, _)| action.id),
            None
        );
        assert_eq!(
            find(&bindings(Scheme::Macos, &none), &combo("Super+C")).map(|(action, _)| action.id),
            Some("copy")
        );
        assert_eq!(
            find(&bindings(Scheme::Linux, &none), &combo("Super+Left")).map(|(action, _)| action.id),
            Some("arrange-left")
        );
        assert_eq!(
            find(&bindings(Scheme::Macos, &none), &combo("Super+Left")).map(|(action, _)| action.id),
            Some("line-start")
        );
        assert_eq!(
            find(&bindings(Scheme::Windows, &none), &combo("Super")).map(|(action, _)| action.id),
            Some("apps")
        );
    }

    #[test]
    fn shift_extends_a_movement_only() {
        let none = Custom::new();
        let found = find(&bindings(Scheme::Macos, &none), &combo("Shift+Super+Left"))
            .map(|(action, shift)| (action.id, shift));
        assert_eq!(found, Some(("line-start", true)));
        assert_eq!(
            find(&bindings(Scheme::Macos, &none), &combo("Shift+Super+C")),
            None
        );
    }

    #[test]
    fn a_key_given_in_the_configuration_is_taken_from_the_action_that_had_it() {
        let custom: Custom = [(
            Scheme::Linux,
            [("lock".to_string(), vec!["Super+Q".to_string()])].into(),
        )]
        .into();
        assert_eq!(
            find(&bindings(Scheme::Linux, &custom), &combo("Super+Q")).map(|(action, _)| action.id),
            Some("lock")
        );
        assert_eq!(
            keys(Scheme::Linux, &custom, "close-window"),
            vec![combo("Alt+F4")]
        );
        assert_eq!(find(&bindings(Scheme::Linux, &custom), &combo("Super+L")), None);
        // Only in the scheme it was given in.
        assert_eq!(
            find(&bindings(Scheme::Windows, &custom), &combo("Super+L")).map(|(action, _)| action.id),
            Some("lock")
        );
        let off: Custom = [(Scheme::Linux, [("lock".to_string(), vec![])].into())].into();
        assert!(keys(Scheme::Linux, &off, "lock").is_empty());
        assert!(is_custom(Scheme::Linux, &off, "lock"));
        // One action's keys, worked out alone, are those of the whole table.
        for (action, bound) in bindings(Scheme::Linux, &custom) {
            assert_eq!(keys(Scheme::Linux, &custom, action.id), bound, "{}", action.id);
        }
    }

    #[test]
    fn terminals_and_applications_named_in_the_configuration_get_their_own_keys() {
        let copy = action("copy").unwrap();
        let none = AppKeys::new();
        assert_eq!(native(copy, "org.gnome.TextEditor", &none), vec![combo("Ctrl+C")]);
        assert_eq!(
            native(copy, "io.github.koukeneko.Nekoland.Terminal", &none),
            vec![combo("Ctrl+Shift+C")]
        );
        assert!(native(action("undo").unwrap(), "kitty", &none).is_empty());
        let line = native(action("delete-line").unwrap(), "firefox", &none);
        assert_eq!(line, vec![combo("Shift+Home"), combo("BackSpace")]);
        let apps: AppKeys = [(
            "emacs".to_string(),
            [("copy".to_string(), "Alt+W".to_string())].into(),
        )]
        .into();
        assert_eq!(native(copy, "emacs", &apps), vec![combo("Alt+W")]);
        assert!(native(action("lock").unwrap(), "firefox", &none).is_empty());
    }

    #[test]
    fn command_with_any_other_key_is_ctrl_in_applications() {
        assert_eq!(
            command_key(&combo("Super+B"), "org.gnome.TextEditor"),
            Some(combo("Ctrl+B"))
        );
        assert_eq!(
            command_key(&combo("Shift+Super+K"), "firefox"),
            Some(combo("Ctrl+Shift+K"))
        );
        assert_eq!(
            command_key(&combo("Super+K"), "foot"),
            Some(combo("Ctrl+Shift+K"))
        );
        assert_eq!(command_key(&combo("Ctrl+Super+K"), "firefox"), None);
        assert_eq!(command_key(&combo("Ctrl+K"), "firefox"), None);
        assert_eq!(command_key(&combo("Super"), "firefox"), None);
    }
}
