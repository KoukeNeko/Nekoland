//! The language of the terminal, as Nekominal's: Traditional Chinese or English, or the system's. The interface is
//! written in Chinese; [`tr`] gives the English of a text when English is chosen, and [`trf`] of a text with values
//! in it, `{0}`, `{1}` standing for them so that each language puts them where it says them.

use std::{cell::Cell, fmt::Display};

use serde::{Deserialize, Serialize};

use super::config::Choice;

/// The language setting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Language {
    /// Chinese for a Chinese locale, English otherwise.
    #[default]
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "zh-TW")]
    ZhTw,
    #[serde(rename = "en-US")]
    EnUs,
}

impl Choice for Language {
    const ALL: &'static [Self] = &[Self::Auto, Self::ZhTw, Self::EnUs];

    fn label(self) -> &'static str {
        match self {
            Self::Auto => "自動",
            Self::ZhTw => "繁體中文",
            Self::EnUs => "English",
        }
    }
}

/// The environment variables of the locale, the one that wins first.
const LOCALE_VARIABLES: [&str; 4] = ["LANGUAGE", "LC_ALL", "LC_MESSAGES", "LANG"];

thread_local! {
    static ENGLISH: Cell<bool> = const { Cell::new(false) };
}

/// Whether `language` is English, the system's locale deciding for [`Language::Auto`].
fn is_english(language: Language) -> bool {
    match language {
        Language::ZhTw => false,
        Language::EnUs => true,
        Language::Auto => !system_tag().is_some_and(|tag| tag.to_ascii_lowercase().starts_with("zh")),
    }
}

fn system_tag() -> Option<String> {
    LOCALE_VARIABLES.iter().find_map(|name| std::env::var(name).ok().filter(|value| !value.is_empty()))
}

/// Takes the language the settings chose, for every text shown from now on; true when that changed it.
pub fn apply(language: Language) -> bool {
    let english = is_english(language);
    ENGLISH.replace(english) != english
}

/// `text` in the language chosen; a text without an English one stays as it is.
pub fn tr(text: &'static str) -> &'static str {
    if !ENGLISH.get() {
        return text;
    }
    english(text).unwrap_or(text)
}

/// `template` in the language chosen, `{0}`, `{1}` … replaced by `values`.
pub fn trf(template: &'static str, values: &[&dyn Display]) -> String {
    fill(tr(template), values)
}

fn fill(template: &str, values: &[&dyn Display]) -> String {
    values
        .iter()
        .enumerate()
        .fold(template.to_owned(), |text, (index, value)| text.replace(&format!("{{{index}}}"), &value.to_string()))
}

fn english(text: &str) -> Option<&'static str> {
    ENGLISH_TEXTS.iter().find(|(chinese, _)| *chinese == text).map(|(_, english)| *english)
}

/// Every text of the interface with its English.
const ENGLISH_TEXTS: &[(&str, &str)] = &[
    // States and groups.
    ("等待回應", "Waiting"),
    ("等待輸入", "Waiting for input"),
    ("執行中", "Running"),
    ("已完成", "Done"),
    ("錯誤", "Error"),
    ("其他", "Other"),
    ("結束碼 {0}", "Exit code {0}"),
    // Choices of the settings.
    ("上方分頁", "Tabs on top"),
    ("側邊欄 · 清單", "Sidebar · list"),
    ("側邊欄 · 圖示", "Sidebar · icons"),
    ("活動清單", "Activity list"),
    ("Agent 卡片", "Agent cards"),
    ("依專案分組", "By project"),
    ("區塊", "Block"),
    ("直線", "Bar"),
    ("底線", "Underline"),
    ("複製或貼上", "Copy or paste"),
    ("顯示功能表", "Show menu"),
    ("只留資料夾和分支", "Folder and branch only"),
    ("整條隱藏", "Hide the bar"),
    ("無", "None"),
    ("僅背景", "In background"),
    ("一律", "Always"),
    ("關閉", "Close"),
    ("僅寫入", "Write only"),
    ("讀取與寫入", "Read and write"),
    ("標示", "Highlight"),
    ("通知", "Notify"),
    ("分頁徽章", "Tab badge"),
    ("送出文字", "Send text"),
    ("響鈴", "Bell"),
    ("自動", "Auto"),
    // The context card.
    ("從快取讀取", "Cache read"),
    ("寫入快取", "Cache write"),
    ("新的輸入", "New input"),
    ("剩餘", "Free"),
    ("脈絡", "Context"),
    ("{0} 視窗", "{0} window"),
    ("已使用", "Used"),
    ("費用", "Cost"),
    ("時間", "Time"),
    ("{0} 輸入 · {1} 輸出", "{0} in · {1} out"),
    ("最後一次回覆", "Last reply"),
    ("{0} 輸出", "{0} out"),
    ("行數", "Lines"),
    ("這個工作階段", "This session"),
    // Tab colours.
    ("紅", "Red"),
    ("橘", "Orange"),
    ("黃", "Yellow"),
    ("綠", "Green"),
    ("藍", "Blue"),
    ("紫", "Purple"),
    ("灰", "Gray"),
    // Panes.
    ("終端機", "Terminal"),
    ("指令失敗，結束代碼 {0}，耗時 {1} 秒", "Command failed with exit code {0} after {1} s"),
    ("指令完成，耗時 {0} 秒", "Command finished after {0} s"),
    ("取消", "Cancel"),
    ("貼上", "Paste"),
    ("貼上 {0} 行？", "Paste {0} lines?"),
    ("貼上 {0}？", "Paste {0}?"),
    ("複製", "Copy"),
    ("尋找", "Find"),
    ("向右分割", "Split right"),
    ("向下分割", "Split down"),
    // Profiles.
    ("與視窗相同", "Same as window"),
    ("不能空白", "Cannot be empty"),
    ("已經有這個設定檔", "This profile exists"),
    ("名稱不能有空格或 =", "No spaces or = in a name"),
    ("已經有這個變數", "This variable exists"),
    ("編輯", "Edit"),
    ("新增設定檔", "New profile"),
    ("預設設定檔", "Default profile"),
    ("所有設定檔", "All profiles"),
    ("還原為偵測到的設定", "Restore detected settings"),
    ("刪除設定檔", "Delete profile"),
    ("名稱", "Name"),
    ("瀏覽…", "Browse…"),
    ("程式", "Program"),
    ("引數", "Arguments"),
    ("起始資料夾", "Starting folder"),
    ("色彩配置", "Color scheme"),
    ("字型大小", "Font size"),
    ("新增變數", "Add variable"),
    ("環境變數", "Environment variables"),
    ("值", "Value"),
    ("刪除", "Delete"),
    // Phone notifications.
    ("手機通知", "Phone notifications"),
    ("傳送到", "Send to"),
    ("格式", "Format"),
    ("何時傳送", "When"),
    ("存取權杖", "Access token"),
    ("傳送哪些事件", "Events"),
    ("測試", "Test"),
    ("ntfy 主題，或接收 POST 的 https 網址", "An ntfy topic, or an https address that takes a POST"),
    ("選填，以 Bearer 傳送，不寫進記錄", "Optional, sent as Bearer, never logged"),
    ("傳送測試通知", "Send a test"),
    ("傳送中", "Sending"),
    ("剛才", "just now"),
    ("● 已送達", "● Delivered"),
    ("上次傳送：{0}，手機已收到", "Last sent {0}, delivered"),
    ("上次傳送：{0}", "Last sent {0}"),
    ("{0} 分鐘前", "{0} min ago"),
    ("網址要是 https://（http:// 只能連到這台電腦）", "The address must be https:// (http:// only to this computer)"),
    ("終端機的測試通知", "A test notification from the terminal"),
    ("不在電腦前", "When away"),
    ("失敗", "Failed"),
    ("完成", "Done"),
    ("長指令結束", "Long command ended"),
    ("程式的通知", "Program notifications"),
    ("測試沒有完成", "The test did not finish"),
    // Snippets and layouts.
    ("文字片段", "Snippets"),
    ("已儲存的視窗配置", "Saved layouts"),
    ("從命令選擇區或建議清單輸入的文字", "Text typed from the command palette or suggestions"),
    ("片段", "Snippet"),
    ("新增片段", "Add snippet"),
    ("要輸入的文字，\\r 代表 Enter", "Text to type, \\r for Enter"),
    ("刪除片段", "Delete snippet"),
    ("儲存視窗配置", "Save window layout"),
    ("{0} 個分頁", "{0} tabs"),
    ("開啟", "Open"),
    ("刪除配置", "Delete layout"),
    // Settings.
    ("外觀", "Appearance"),
    ("設定檔", "Profiles"),
    ("預覽", "Previews"),
    ("觸發規則", "Triggers"),
    ("快速鍵", "Shortcuts"),
    ("進階", "Advanced"),
    ("關於", "About"),
    ("分頁版面", "Tab layout"),
    ("側邊欄的分頁", "Sidebar tabs"),
    ("背景不透明度（%）", "Background opacity (%)"),
    ("字型", "Font"),
    ("行高", "Line height"),
    ("游標形狀", "Cursor shape"),
    ("游標閃爍", "Blinking cursor"),
    ("標題格式", "Title format"),
    ("捲動緩衝區行數", "Scrollback lines"),
    ("選取時複製", "Copy on select"),
    ("工作列提示", "Taskbar alert"),
    ("字型連字", "Font ligatures"),
    ("側邊欄 · 清單時，每個分頁顯示多少", "How much each tab shows in the sidebar list"),
    ("=>、>= 等組合顯示為一個字形", "Show => or >= as one glyph"),
    ("kitty 鍵盤協定", "kitty keyboard protocol"),
    ("捲動時將指令列固定在頂端", "Pin the command line while scrolling"),
    ("右鍵", "Right click"),
    ("貼上多行前確認", "Confirm multi-line paste"),
    ("貼上大量文字前確認（KiB）", "Confirm large paste (KiB)"),
    ("鈴聲視覺提示", "Visual bell"),
    ("鈴聲音效", "Bell sound"),
    ("關閉前確認", "Confirm before closing"),
    ("觸發確認的工作階段數", "Sessions that need confirming"),
    ("啟動時還原工作階段", "Restore sessions on start"),
    ("桌面通知", "Desktop notifications"),
    ("顯示桌面通知", "Show desktop notifications"),
    ("長指令結束時通知（秒，0 為關閉）", "Notify when a long command ends (s, 0 off)"),
    ("代理程式環境變數", "Agent environment variables"),
    ("顯示狀態列", "Show status bar"),
    ("沒有 agent 時", "Without an agent"),
    ("狀態列：工作狀態", "Status bar: state"),
    ("狀態列：模型", "Status bar: model"),
    ("狀態列：脈絡用量", "Status bar: context"),
    ("狀態列：費用", "Status bar: cost"),
    ("狀態列：工作階段時間", "Status bar: session time"),
    ("狀態列：Git 分支", "Status bar: Git branch"),
    ("狀態列：資料夾", "Status bar: folder"),
    ("Shell 整合", "Shell integration"),
    ("OSC 52 剪貼簿", "OSC 52 clipboard"),
    ("游標指到圖片時顯示預覽", "Preview images under the pointer"),
    ("新分頁沿用終端機啟動時的環境變數", "New tabs inherit the terminal's environment"),
    ("開啟「檔案:行」的編輯器", "Editor for file:line"),
    ("原始碼", "Source code"),
    ("開啟時顯示側邊欄", "Show sidebar on open"),
    ("依狀態分組", "Group by state"),
    ("游標閃爍間隔（毫秒）", "Cursor blink interval (ms)"),
    ("預覽大小（最長邊，像素）", "Preview size (longest side, px)"),
    ("語言", "Language"),
    ("搜尋設定", "Search settings"),
    ("搜尋結果", "Search results"),
    ("沒有符合的設定", "No matching settings"),
    ("設定 JSON", "Settings JSON"),
    ("無法使用這個值", "This value cannot be used"),
    ("版本", "Version"),
    // Sidebar.
    ("設定", "Settings"),
    ("關閉分頁", "Close tab"),
    ("{0} 窗格", "{0} panes"),
    // Subagents.
    ("子代理", "Subagents"),
    ("Esc 返回", "Esc to go back"),
    ("回覆", "Reply"),
    ("結果", "Result"),
    ("任務", "Task"),
    ("開啟紀錄", "Open transcript"),
    ("開啟日誌", "Open journal"),
    ("執行中 {0}", "Running {0}"),
    ("已完成 · {0}", "Done · {0}"),
    ("{0} 次工具呼叫", "{0} tool calls"),
    ("更早的 {0} 次", "{0} earlier"),
    ("{0} 個執行中", "{0} running"),
    ("{0} 個失敗", "{0} failed"),
    ("{0} 個已完成", "{0} done"),
    ("還有 {0} 個", "{0} more"),
    ("複製回覆", "Copy reply"),
    // Title bar and menus.
    ("切換側邊欄", "Toggle sidebar"),
    ("新增分頁", "New tab"),
    ("{0} 個等待回應", "{0} waiting"),
    ("下一個需要回應的分頁 (Ctrl+Shift+U)", "Next tab that needs you (Ctrl+Shift+U)"),
    ("新視窗", "New window"),
    ("重新命名分頁", "Rename tab"),
    ("放大字型", "Zoom in"),
    ("縮小字型", "Zoom out"),
    ("重設字型大小", "Reset zoom"),
    ("關於終端機", "About Terminal"),
    ("主選單", "Main menu"),
    ("命令選擇區", "Command palette"),
    ("搜尋所有分頁", "Search all tabs"),
    ("提示選取（網址、路徑、雜湊）", "Hints (URLs, paths, hashes)"),
    ("下一個需要回應的分頁", "Next tab that needs you"),
    ("結束", "Quit"),
    // Triggers.
    ("輸出出現符合的文字時做一件事", "Do something when the output matches"),
    ("新增規則", "Add rule"),
    ("不是有效的正規表示式", "Not a valid regular expression"),
    ("符合的文字", "Pattern"),
    ("只在執行", "Only while running"),
    ("動作", "Action"),
    ("還沒換行也算", "Match before the line ends"),
    ("時；空白就是任何程式", "; empty for any program"),
    ("通知內容", "Notification text"),
    ("要送出的文字，\\r 是 Enter", "Text to send, \\r for Enter"),
    ("送出的文字", "Text to send"),
    ("標示顏色", "Highlight color"),
    ("錯誤標紅", "Errors in red"),
    ("測試失敗加徽章", "Badge on failed tests"),
    ("部署完成通知", "Notify when deployed"),
    ("密碼提示時響鈴", "Bell at a password prompt"),
    // The command palette.
    ("輸入命令", "Type a command"),
    ("沒有符合的命令", "No matching commands"),
    ("切換到分頁：{0}", "Go to tab: {0}"),
    ("新增分頁：{0}", "New tab: {0}"),
    ("設定：{0}", "Settings: {0}"),
    ("片段：{0}", "Snippet: {0}"),
    ("開啟配置：{0}", "Open layout: {0}"),
    ("在新視窗開啟配置：{0}", "Open layout in a new window: {0}"),
    ("重新命名配置：{0}", "Rename layout: {0}"),
    ("刪除配置：{0}", "Delete layout: {0}"),
    ("將配置重新命名為「{0}」", "Rename layout to “{0}”"),
    ("儲存為配置「{0}」", "Save as layout “{0}”"),
    ("輸入配置名稱", "Layout name"),
    ("無法儲存配置：{0}", "Could not save the layout: {0}"),
    ("從 Windows Terminal 匯入色彩配置", "Import color schemes from Windows Terminal"),
    ("匯入 iTerm2 色彩配置（.itermcolors）", "Import iTerm2 color presets (.itermcolors)"),
    ("下拉式視窗（Super+`）", "Drop-down window (Super+`)"),
    ("更新並重新啟動", "Update and restart"),
    ("已安裝新版本", "A newer version is installed"),
    ("無法重新啟動：{0}", "Could not restart: {0}"),
    ("已匯入 {0} 個色彩配置：{1}", "Imported {0} color schemes: {1}"),
    ("沒有新的色彩配置可以匯入", "No new color schemes to import"),
    ("無法匯入：{0}", "Could not import: {0}"),
    ("刪除色彩配置", "Delete color scheme"),
    ("在新的 git 工作樹開啟分頁", "New tab in a git worktree"),
    ("移除這個 git 工作樹", "Remove this worktree"),
    ("輸入新分支的名稱", "Type a name for the new branch"),
    ("建立分支「{0}」與它的工作樹", "Create branch “{0}” and its worktree"),
    ("「{0}」不能當分支名稱", "“{0}” cannot be a branch name"),
    ("工作樹已建立，但 .worktreeinclude 列出的檔案沒有複製過去：{0}", "The worktree is made, but the files .worktreeinclude names were not copied: {0}"),
    ("這個窗格的資料夾不在 git 儲存庫裡", "The folder of this pane is not in a git repository"),
    ("無法建立工作樹：{0}", "The worktree could not be made: {0}"),
    ("這是儲存庫的主要簽出，不會被移除", "This is the main checkout of the repository: it is never removed"),
    ("工作樹有 {0} 個未提交的變更與 {1} 個未追蹤的檔案：請先提交或捨棄它們", "The worktree has {0} uncommitted changes and {1} untracked files: commit or discard them first"),
    ("工作樹不在任何分支上：移除它會遺失它的提交", "The worktree is on no branch: removing it would lose its commits"),
    ("移除工作樹 {0}？", "Remove the worktree {0}?"),
    ("沒有未提交的變更", "No uncommitted changes"),
    ("{0} 個檔案有未提交的變更", "{0} files have uncommitted changes"),
    ("所有提交都已推送到 {0}", "Every commit is pushed to {0}"),
    ("{0} 個提交還沒推送", "{0} commits are not pushed"),
    ("這個儲存庫沒有遠端", "The repository has no remote"),
    ("已合併到 {0}", "Merged into {0}"),
    ("已合併到 {0}（PR #{1}）", "Merged into {0} (PR #{1})"),
    ("還沒合併到 {0}", "Not merged into {0}"),
    ("{0} 個分頁在這個資料夾，會一起關閉", "{0} tabs in this folder close with it"),
    ("{0} 正在這個資料夾執行", "{0} is running in this folder"),
    ("{0} 個被 git 忽略的檔案或資料夾（例如 .env、建置結果）會隨資料夾刪除", "{0} files or folders git ignores (.env, build output) are deleted with the folder"),
    ("分支已合併，但被忽略的檔案會刪除", "The branch is merged, but the ignored files are deleted"),
    ("移除資料夾並刪除分支", "Remove the folder and delete the branch"),
    ("分支已合併，不會遺失任何東西", "The branch is merged: nothing is lost"),
    ("git 只刪除已合併的分支", "git deletes the branch only once it is merged"),
    ("只移除資料夾", "Only remove the folder"),
    ("保留分支 {0}，之後可以再建立工作樹", "Keeps the branch {0} to make a worktree of again later"),
    ("要先提交並推送", "Commit and push first"),
    ("要先提交", "Commit first"),
    ("要先推送", "Push first"),
    ("未提交的變更會遺失", "The uncommitted changes would be lost"),
    ("移除", "Remove"),
    ("開啟分頁查看", "Show the tab"),
    ("工作樹已移除；分支「{0}」保留：{1}", "The worktree is removed; its branch “{0}” is kept: {1}"),
    ("無法移除工作樹：{0}", "The worktree could not be removed: {0}"),
    ("沒有結果", "No results"),
    ("色彩配置：{0}", "Color scheme: {0}"),
    // Windows.
    ("關閉這個分頁？", "Close this tab?"),
    ("關閉視窗？", "Close this window?"),
    ("重新命名", "Rename"),
    ("重新命名…", "Rename…"),
    ("顏色", "Color"),
    ("複製分頁", "Duplicate tab"),
    ("移到新視窗", "Move to new window"),
    ("關閉右側分頁", "Close tabs to the right"),
    ("{0}還在執行，關閉後會結束。", "{0} still running and will end."),
    ("「{0}」", "“{0}”"),
    ("、", ", "),
    ("{0} 等你", "{0} is waiting"),
    ("{0} 完成", "{0} is done"),
    ("{0} 失敗", "{0} failed"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chinese_is_shown_as_written_and_english_from_the_table() {
        apply(Language::ZhTw);
        assert_eq!(tr("關閉分頁"), "關閉分頁");
        assert_eq!(trf("{0} 個執行中", &[&3]), "3 個執行中");
        apply(Language::EnUs);
        assert_eq!(tr("關閉分頁"), "Close tab");
        assert_eq!(trf("指令失敗，結束代碼 {0}，耗時 {1} 秒", &[&2, &30]), "Command failed with exit code 2 after 30 s");
        assert_eq!(tr("沒有的字"), "沒有的字", "a text without English stays");
        apply(Language::ZhTw);
    }

    #[test]
    fn every_english_text_has_no_chinese_and_the_same_values() {
        let is_chinese = |letter: char| ('\u{2E80}'..='\u{9FFF}').contains(&letter);
        for (chinese, english) in ENGLISH_TEXTS {
            assert!(!english.chars().any(is_chinese), "{english}");
            for index in 0..3 {
                let value = format!("{{{index}}}");
                assert_eq!(chinese.contains(&value), english.contains(&value), "{chinese}");
            }
        }
    }
}
