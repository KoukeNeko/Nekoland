//! The launcher's own commands: the system's, the active window's, and the views of their
//! own that some open.

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum View {
    Root,
    Clipboard,
    Emoji,
    Files,
    Snippets,
    Quicklinks,
    Notes,
    Windows,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Does {
    /// A program, with its arguments.
    Run(&'static [&'static str]),
    /// A message to the compositor, as `nekoland msg` sends.
    Message(&'static str),
    /// One of the compositor's window commands, for the window under the launcher.
    Window(&'static str),
    Open(View),
    DarkMode,
    ClearClipboard,
    SearchMenus,
    NewSnippet,
    NewQuicklink,
    NewNote,
}

pub struct Command {
    pub id: &'static str,
    pub title: &'static str,
    /// What else it is found by, English among them.
    pub words: &'static str,
    /// A theme icon, or a part of the screen drawn as a window there: left, top, right, bottom.
    pub icon: &'static str,
    pub part: Option<[f32; 4]>,
    pub does: Does,
}

const fn command(
    id: &'static str,
    title: &'static str,
    words: &'static str,
    icon: &'static str,
    does: Does,
) -> Command {
    Command { id, title, words, icon, part: None, does }
}

const fn window(id: &'static str, title: &'static str, words: &'static str, part: [f32; 4]) -> Command {
    Command { id, title, words, icon: "", part: Some(part), does: Does::Window(id) }
}

pub const COMMANDS: &[Command] = &[
    command(
        "clipboard",
        "剪貼簿紀錄",
        "clipboard history 複製 貼上",
        "edit-paste",
        Does::Open(View::Clipboard),
    ),
    command(
        "emoji",
        "搜尋表情符號",
        "emoji symbols 符號 表情",
        "accessories-character-map",
        Does::Open(View::Emoji),
    ),
    command("files", "搜尋檔案", "search files find 檔案", "folder", Does::Open(View::Files)),
    command("menus", "搜尋選單項目", "search menu items 選單", "help-browser", Does::SearchMenus),
    command("windows", "切換視窗", "switch windows 視窗", "view-app-grid", Does::Open(View::Windows)),
    command("snippets", "搜尋文字片段", "search snippets 片段", "text-editor", Does::Open(View::Snippets)),
    command("new-snippet", "建立文字片段", "create snippet 片段", "text-editor", Does::NewSnippet),
    command(
        "quicklinks",
        "搜尋快速連結",
        "search quicklinks 連結",
        "user-bookmarks",
        Does::Open(View::Quicklinks),
    ),
    command("new-quicklink", "建立快速連結", "create quicklink 連結", "user-bookmarks", Does::NewQuicklink),
    command("notes", "搜尋筆記", "search notes 筆記", "x-office-document", Does::Open(View::Notes)),
    command("new-note", "新增筆記", "create note 筆記", "x-office-document", Does::NewNote),
    command(
        "clear-clipboard",
        "清除剪貼簿紀錄",
        "clear clipboard history",
        "user-trash-full",
        Does::ClearClipboard,
    ),
    command("lock", "鎖定螢幕", "lock screen", "system-lock-screen", Does::Message("lock")),
    command("sleep", "睡眠", "sleep suspend 休眠", "system-shutdown", Does::Message("sleep")),
    command(
        "restart",
        "重新開機",
        "restart reboot",
        "system-reboot",
        Does::Run(&["nekoland-confirm", "restart"]),
    ),
    command(
        "shutdown",
        "關機",
        "shut down power off",
        "system-shutdown",
        Does::Run(&["nekoland-confirm", "shutdown"]),
    ),
    command("logout", "登出", "log out", "system-log-out", Does::Run(&["nekoland-confirm", "logout"])),
    command(
        "empty-trash",
        "清空垃圾桶",
        "empty trash 垃圾桶",
        "user-trash-full",
        Does::Run(&["nekoland-confirm", "empty-trash"]),
    ),
    command(
        "dark-mode",
        "切換深色模式",
        "toggle dark mode appearance 外觀 淺色",
        "preferences-desktop-theme",
        Does::DarkMode,
    ),
    command(
        "settings",
        "開啟設定",
        "settings preferences 偏好設定",
        "org.gnome.Settings",
        Does::Run(&["nekoland-settings"]),
    ),
    command(
        "overview",
        "顯示概覽",
        "mission control overview 桌面",
        "preferences-desktop-display",
        Does::Message("overview"),
    ),
    window("left", "左半邊", "left half window 視窗", [0.0, 0.0, 0.5, 1.0]),
    window("right", "右半邊", "right half window 視窗", [0.5, 0.0, 1.0, 1.0]),
    window("top", "上半部", "top half window 視窗", [0.0, 0.0, 1.0, 0.5]),
    window("bottom", "下半部", "bottom half window 視窗", [0.0, 0.5, 1.0, 1.0]),
    window("top-left", "左上角", "top left quarter window 視窗", [0.0, 0.0, 0.5, 0.5]),
    window("top-right", "右上角", "top right quarter window 視窗", [0.5, 0.0, 1.0, 0.5]),
    window("bottom-left", "左下角", "bottom left quarter window 視窗", [0.0, 0.5, 0.5, 1.0]),
    window("bottom-right", "右下角", "bottom right quarter window 視窗", [0.5, 0.5, 1.0, 1.0]),
    window("first-third", "左邊三分之一", "first third window 視窗", [0.0, 0.0, 0.333, 1.0]),
    window("center-third", "中間三分之一", "center third window 視窗", [0.333, 0.0, 0.667, 1.0]),
    window("last-third", "右邊三分之一", "last third window 視窗", [0.667, 0.0, 1.0, 1.0]),
    window("first-two-thirds", "左邊三分之二", "first two thirds window 視窗", [0.0, 0.0, 0.667, 1.0]),
    window("last-two-thirds", "右邊三分之二", "last two thirds window 視窗", [0.333, 0.0, 1.0, 1.0]),
    window("maximize", "最大化", "maximize window 視窗 放大", [0.0, 0.0, 1.0, 1.0]),
    window("almost-maximize", "幾乎最大化", "almost maximize window 視窗", [0.06, 0.06, 0.94, 0.94]),
    window("center", "置中", "center window 視窗 中央", [0.2, 0.2, 0.8, 0.8]),
    window("restore", "還原視窗", "restore window 視窗", [0.15, 0.25, 0.7, 0.85]),
    window("fullscreen", "切換全螢幕", "toggle fullscreen window 視窗", [0.0, 0.0, 1.0, 1.0]),
    window("minimize", "縮到最小", "minimize window 視窗 隱藏", [0.3, 0.78, 0.7, 0.92]),
    command("close", "關閉視窗", "close window 視窗", "window-close", Does::Window("close")),
];

/// The command that opens a view, for its title.
pub fn opening(view: View) -> Option<&'static Command> {
    COMMANDS.iter().find(|command| command.does == Does::Open(view))
}
