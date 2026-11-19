//! The menu bar, as the Mac's Preview has it, with its keys: ⌘ is Ctrl here, ⌥ is Alt, and
//! ⌃⌘ is Ctrl+Super. The same table names the keys the window's actions are bound to and
//! the list 鍵盤快速鍵 shows, so a key is only ever written here.

use gtk::gio;
use gtk::prelude::*;

/// A menu entry: its label, its action and target, and its keys, the first shown.
pub struct Entry {
    pub label: &'static str,
    pub action: &'static str,
    pub keys: &'static [&'static str],
}

const fn entry(label: &'static str, action: &'static str, keys: &'static [&'static str]) -> Entry {
    Entry { label, action, keys }
}

/// The menus, each a title and sections of entries.
pub const MENUS: &[(&str, &[&[Entry]])] = &[
    (
        "檔案",
        &[
            &[
                entry("從剪貼板新增", "win.new-from-clipboard", &["<Control>n"]),
                entry("打開⋯", "win.open", &["<Control>o"]),
            ],
            &[
                entry("關閉視窗", "win.close", &["<Control>w"]),
                entry("關閉所選項目", "win.close-selected", &["<Control><Shift>w"]),
                entry("儲存", "win.save", &["<Control>s"]),
                entry("複製", "win.duplicate", &["<Control><Shift>s"]),
                entry("另存新檔⋯", "win.save-as", &["<Control><Shift><Alt>s"]),
                entry("重新命名⋯", "win.rename", &[]),
                entry("搬移至⋯", "win.move-to", &[]),
                entry("回復到上次儲存的版本", "win.revert-saved", &[]),
                entry("瀏覽所有版本⋯", "win.browse-versions", &[]),
            ],
            &[
                entry("輸出⋯", "win.export", &[]),
                entry("輸出為 PDF⋯", "win.export-pdf", &[]),
            ],
            &[
                entry("輸入密碼⋯", "win.enter-password", &[]),
                entry("編輯權限⋯", "win.edit-permissions", &[]),
            ],
            &[
                entry("從掃描器輸入⋯", "win.import-scanner", &[]),
                entry("從選取範圍拍攝螢幕快照⋯", "win.screenshot::area", &[]),
                entry("從整個螢幕拍攝螢幕快照", "win.screenshot::screen", &[]),
            ],
            &[entry("列印⋯", "win.print", &["<Control>p"])],
        ],
    ),
    (
        "編輯",
        &[
            &[
                entry("還原", "win.undo", &["<Control>z"]),
                entry("重做", "win.redo", &["<Control><Shift>z"]),
            ],
            &[
                entry("剪下", "win.cut", &["<Control>x"]),
                entry("拷貝", "win.copy", &["<Control>c"]),
                entry("貼上", "win.paste", &["<Control>v"]),
                entry("刪除", "win.delete", &["Delete", "BackSpace"]),
                entry("全選", "win.select-all", &["<Control>a"]),
                entry("取消全選", "win.select-none", &["<Control><Alt>a"]),
                entry("反向選取", "win.invert-selection", &["<Control><Shift>i"]),
            ],
            &[
                entry("插入檔案中的頁面⋯", "win.insert-pdf", &[]),
                entry("插入空白頁面", "win.insert-blank", &[]),
                entry("插入掃描器中的頁面⋯", "win.insert-scanner", &[]),
                entry("刪除此頁", "win.delete-page", &[]),
                entry("上移此頁", "win.page-up", &[]),
                entry("下移此頁", "win.page-down", &[]),
                entry("丟到垃圾桶", "win.trash", &["<Control>Delete"]),
            ],
            &[
                entry("尋找⋯", "win.find", &["<Control>f"]),
                entry("尋找下一個", "win.find-next", &["<Control>g"]),
                entry("尋找上一個", "win.find-previous", &["<Control><Shift>g"]),
                entry("使用所選範圍尋找", "win.find-selection", &["<Control>e"]),
                entry("跳到所選範圍", "win.jump-selection", &["<Control>j"]),
            ],
        ],
    ),
    (
        "顯示方式",
        &[
            &[
                entry("隱藏側邊欄", "win.sidebar::hidden", &["<Control><Alt>1"]),
                entry("縮覽圖", "win.sidebar::thumbnails", &["<Control><Alt>2"]),
                entry("目錄", "win.sidebar::contents", &["<Control><Alt>3"]),
                entry("標示重點和附註", "win.sidebar::notes", &["<Control><Alt>4"]),
                entry("書籤", "win.sidebar::bookmarks", &["<Control><Alt>5"]),
                entry("聯絡表", "win.sidebar::sheet", &["<Control><Alt>6"]),
            ],
            &[
                entry("連續捲動", "win.layout::continuous", &["<Control>1"]),
                entry("單頁", "win.layout::single", &["<Control>2"]),
                entry("雙頁", "win.layout::two", &["<Control>3"]),
            ],
            &[
                entry("實際大小", "win.zoom-actual", &["<Control>0"]),
                entry("符合視窗大小", "win.zoom-fit", &["<Control>9"]),
                entry("放大", "win.zoom-in", &["<Control>plus", "<Control>equal"]),
                entry("縮小", "win.zoom-out", &["<Control>minus"]),
                entry("縮放至選取範圍", "win.zoom-selection", &["<Control>asterisk", "<Control><Shift>8"]),
            ],
            &[
                entry("顯示影像背景", "win.image-background", &["<Control><Alt>b"]),
                entry("PDF 使用深色外觀", "win.dark-pdf", &[]),
            ],
            &[
                entry("顯示標記工具列", "win.markup", &["<Control><Shift>a"]),
                entry("隱藏工具列", "win.hide-toolbar", &["<Control><Alt>t"]),
            ],
            &[
                entry("幻燈片秀", "win.slideshow", &["<Control><Shift>f"]),
                entry("進入全螢幕", "win.fullscreen", &["F11"]),
            ],
        ],
    ),
    (
        "前往",
        &[
            &[
                entry("上一項", "win.previous", &["<Alt>Up", "Page_Up", "Left"]),
                entry("下一項", "win.next", &["<Alt>Down", "Page_Down", "Right"]),
                entry("上一份文件", "win.previous-document", &["<Alt>Page_Up"]),
                entry("下一份文件", "win.next-document", &["<Alt>Page_Down"]),
                entry("第一頁", "win.first-page", &["Home"]),
                entry("最後一頁", "win.last-page", &["End"]),
            ],
            &[entry("前往頁面⋯", "win.go-to-page", &["<Control><Alt>g"])],
            &[
                entry("上一步", "win.back", &["<Control>bracketleft"]),
                entry("下一步", "win.forward", &["<Control>bracketright"]),
            ],
        ],
    ),
    (
        "工具",
        &[
            &[
                entry("顯示檢閱器", "win.info", &["<Control>i"]),
                entry("顯示放大鏡", "win.loupe", &["grave"]),
            ],
            &[
                entry("調整顏色⋯", "win.adjust-color", &["<Control><Alt>c"]),
                entry("調整大小⋯", "win.resize", &[]),
                entry("指定描述檔⋯", "win.assign-profile", &[]),
                entry("影像描述⋯", "win.describe", &[]),
                entry("加入書籤", "win.bookmark", &["<Control>d"]),
            ],
            &[
                entry("向左旋轉", "win.rotate-left", &["<Control>l"]),
                entry("向右旋轉", "win.rotate-right", &["<Control>r"]),
                entry("水平翻轉", "win.flip", &[]),
                entry("垂直翻轉", "win.flip-vertical", &[]),
            ],
            &[
                entry("文字選取", "win.tool::text", &[]),
                entry("矩形選取", "win.tool::rectangle", &[]),
                entry("橢圓形選取", "win.tool::ellipse", &[]),
                entry("套索選取", "win.tool::lasso", &[]),
                entry("智慧型套索", "win.tool::smart-lasso", &[]),
                entry("即時 Alpha", "win.tool::instant-alpha", &[]),
                entry("塗黑", "win.tool::redact", &[]),
                entry("草圖", "win.tool::sketch", &[]),
                entry("表單填寫", "win.tool::form", &[]),
            ],
            &[
                entry("裁切", "win.crop", &["<Control>k"]),
                entry("移除背景", "win.remove-background", &["<Control><Shift>k"]),
            ],
            &[
                entry("標示重點", "win.mark::highlight", &["<Control><Super>h"]),
                entry("底線", "win.mark::underline", &["<Control><Super>u"]),
                entry("刪除線", "win.mark::strikeout", &["<Control><Super>s"]),
                entry("矩形", "win.annotate::rectangle", &["<Control><Super>r"]),
                entry("橢圓形", "win.annotate::ellipse", &["<Control><Super>o"]),
                entry("線條", "win.annotate::line", &["<Control><Super>i"]),
                entry("箭頭", "win.annotate::arrow", &["<Control><Super>a"]),
                entry("多邊形", "win.annotate::polygon", &[]),
                entry("星形", "win.annotate::star", &[]),
                entry("對話框", "win.annotate::bubble", &[]),
                entry("遮罩", "win.annotate::mask", &[]),
                entry("放大鏡", "win.annotate::loupe", &["<Control><Super>l"]),
                entry("文字", "win.annotate::text", &["<Control><Super>t"]),
                entry("附註", "win.annotate::note", &["<Control><Super>n"]),
                entry("簽名⋯", "win.manage-signatures", &[]),
            ],
        ],
    ),
];

/// The keys that are not in these menus: the markup's own, and those that end what is under way.
pub const OTHER_KEYS: &[Entry] = &[
    entry("設定⋯", "app.preferences", &["<Control>comma"]),
    entry("鍵盤快速鍵", "app.shortcuts", &["<Control>question"]),
    entry("結束", "app.quit", &["<Control>q"]),
    // Nekoland's own 視窗 menu holds it; the key is the window's.
    entry("縮到最小", "win.minimize", &["<Control>m"]),
    entry("取消", "win.cancel", &["Escape"]),
];

fn section_of(entries: &[Entry]) -> gio::Menu {
    let section = gio::Menu::new();
    for entry in entries {
        let item = gio::MenuItem::new(Some(entry.label), Some(entry.action));
        if let Some(key) = entry.keys.first() {
            item.set_attribute_value("accel", Some(&key.to_variant()));
        }
        section.append_item(&item);
    }
    section
}

/// The menu bar; 檔案 holds `recent`, the files opened lately, under 打開最近使用的檔案.
pub fn menubar(recent: &gio::Menu) -> gio::Menu {
    let bar = gio::Menu::new();
    for (index, (title, sections)) in MENUS.iter().enumerate() {
        let menu = gio::Menu::new();
        for (at, entries) in sections.iter().enumerate() {
            let section = section_of(entries);
            if index == 0 && at == 0 {
                section.append_submenu(Some("打開最近使用的檔案"), recent);
            }
            menu.append_section(None, &section);
        }
        bar.append_submenu(Some(title), &menu);
    }
    bar
}

/// The sidebar's button: 顯示方式's first two sections, what the sidebar shows and how the
/// pages are laid out.
pub fn sidebar_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    let view = MENUS.iter().find(|(title, _)| *title == "顯示方式").map_or(&[][..], |(_, sections)| *sections);
    for entries in view.iter().take(2) {
        menu.append_section(None, &section_of(entries));
    }
    menu
}

/// Binds every action named above to its keys. An action with a target, as `win.layout::two`
/// is, takes the keys for that target.
pub fn bind_keys(app: &impl IsA<gtk::Application>) {
    let all = MENUS
        .iter()
        .flat_map(|(_, sections)| sections.iter().flat_map(|entries| entries.iter()))
        .chain(OTHER_KEYS.iter());
    for entry in all.filter(|entry| !entry.keys.is_empty()) {
        let name = match entry.action.split_once("::") {
            Some((action, target)) => format!("{action}('{target}')"),
            None => entry.action.to_owned(),
        };
        app.as_ref().set_accels_for_action(&name, entry.keys);
    }
}

/// The entries that have keys, by menu, for 鍵盤快速鍵.
pub fn with_keys() -> Vec<(&'static str, Vec<&'static Entry>)> {
    let mut groups: Vec<(&'static str, Vec<&'static Entry>)> = MENUS
        .iter()
        .map(|(title, sections)| (*title, sections.iter().flat_map(|entries| entries.iter()).filter(|entry| !entry.keys.is_empty()).collect()))
        .collect();
    groups.push(("其他", OTHER_KEYS.iter().collect()));
    groups
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn no_two_entries_share_a_key() {
        let mut seen: HashMap<&str, &str> = HashMap::new();
        let all = MENUS
            .iter()
            .flat_map(|(_, sections)| sections.iter().flat_map(|entries| entries.iter()))
            .chain(OTHER_KEYS.iter());
        for entry in all {
            for key in entry.keys {
                if let Some(other) = seen.insert(key, entry.action) {
                    panic!("{key} is both {other} and {}", entry.action);
                }
            }
        }
    }
}
