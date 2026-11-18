//! What a window's command palette lists, as Nekominal's: every command with its shortcut, then a tab to go to, a
//! profile to open, a colour scheme, a setting, a snippet to type and what can be done to each saved layout. Saving or
//! renaming a layout asks its name in the palette; searching every tab lists their lines that hold what is typed.

use std::rc::Rc;

use adw::prelude::*;
use vte4::prelude::*;

use super::{
    super::{
        lang::{tr, trf},
        layouts, palette::{self, Item, Source},
        profiles, screen_rows, settings, theme, SHORTCUTS,
    },
    all_windows, Window,
};

const PALETTE_ACTION: &str = "win.palette";
/// No more lines are listed of a search of every tab.
const FOUND_LIMIT: usize = 200;

/// A line of a tab, as a search of every tab finds it.
struct Line {
    window: std::rc::Weak<Window>,
    tab: adw::TabPage,
    folder: String,
    text: String,
}

impl Window {
    pub(super) fn open_palette(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        let items = Rc::new(move |query: &str| {
            weak.upgrade().map_or_else(Vec::new, |this| palette::rank(this.palette_items(), query))
        });
        self.palette.open(Source { placeholder: tr("輸入命令"), empty: tr("沒有符合的命令"), counted: false, find: items });
    }

    /// Every entry, in Nekominal's order.
    fn palette_items(self: &Rc<Self>) -> Vec<Item> {
        let mut items = self.command_items();
        items.extend(self.tab_items());
        items.extend(self.profile_items());
        items.extend(scheme_items());
        items.extend(self.setting_items());
        items.extend(self.snippet_items());
        items.extend(self.layout_items());
        items
    }

    /// Runs `act` on the window while it is open.
    fn on_this(self: &Rc<Self>, act: impl Fn(&Rc<Self>) + 'static) -> impl Fn() + 'static {
        let weak = Rc::downgrade(self);
        move || {
            if let Some(this) = weak.upgrade() {
                act(&this);
            }
        }
    }

    fn command_items(self: &Rc<Self>) -> Vec<Item> {
        SHORTCUTS
            .iter()
            .filter(|(action, _, _)| *action != PALETTE_ACTION)
            .map(|&(action, keys, label)| {
                let hint = keys.first().map(|keys| shortcut_label(keys)).unwrap_or_default();
                Item::new(tr(label), hint, self.on_this(move |this| {
                    if let Err(error) = WidgetExt::activate_action(&this.window, action, None) {
                        eprintln!("nekoland-terminal: could not run {action}: {error}");
                    }
                }))
            })
            .collect()
    }

    fn tab_items(self: &Rc<Self>) -> Vec<Item> {
        self.pages()
            .into_iter()
            .map(|(tab, page)| {
                Item::new(trf("切換到分頁：{0}", &[&page.title()]), "", self.on_this(move |this| {
                    this.tab_view.set_selected_page(&tab);
                }))
            })
            .collect()
    }

    fn profile_items(self: &Rc<Self>) -> Vec<Item> {
        profiles::all()
            .into_iter()
            .map(|profile| {
                let name = profile.name;
                Item::new(trf("新增分頁：{0}", &[&name]), "", self.on_this(move |this| {
                    let directory = this.focused_pane().and_then(|pane| pane.directory());
                    this.open_tab(directory, Some(&name), None);
                }))
            })
            .collect()
    }

    fn setting_items(self: &Rc<Self>) -> Vec<Item> {
        settings::searchable()
            .into_iter()
            .map(|(label, section)| {
                Item::new(trf("設定：{0}", &[&tr(label)]), "", self.on_this(move |this| this.open_settings(section, None)))
            })
            .collect()
    }

    fn snippet_items(self: &Rc<Self>) -> Vec<Item> {
        super::config::current()
            .snippets
            .into_iter()
            .map(|snippet| {
                Item::new(trf("片段：{0}", &[&snippet.name]), "", self.on_this(move |this| {
                    if let Some(pane) = this.focused_pane() {
                        pane.send(&snippet.input);
                        pane.screen.grab_focus();
                    }
                }))
            })
            .collect()
    }

    /// Four entries a layout: open it here, open it in a new window, rename it, delete it.
    fn layout_items(self: &Rc<Self>) -> Vec<Item> {
        let mut items = Vec::new();
        for name in layouts::load().into_keys() {
            let open = name.clone();
            items.push(Item::new(trf("開啟配置：{0}", &[&name]), "", self.on_this(move |this| this.open_layout(&open, false))));
            let open = name.clone();
            items.push(Item::new(trf("在新視窗開啟配置：{0}", &[&name]), "", self.on_this(move |this| {
                this.open_layout(&open, true);
            })));
            let old = name.clone();
            items.push(Item::new(trf("重新命名配置：{0}", &[&name]), "", self.on_this(move |this| {
                this.ask_layout_name(Some(old.clone()));
            })));
            let gone = name.clone();
            items.push(Item::new(trf("刪除配置：{0}", &[&name]), "", self.on_this(move |this| {
                this.report_layout(layouts::delete(&gone));
            })));
        }
        items
    }

    /// Opens the tabs of a saved layout here, after the ones the window has, or in a window of their own.
    pub(crate) fn open_layout(self: &Rc<Self>, name: &str, new_window: bool) {
        let Some(tabs) = layouts::load().remove(name) else {
            return;
        };
        let target = match (new_window, self.application()) {
            (true, Some(app)) => Window::new(&app),
            _ => self.clone(),
        };
        target.open_saved(&tabs);
        target.window.present();
    }

    /// Asks in the palette for a name to save the window's tabs under, or to give the layout `renaming`.
    pub(crate) fn ask_layout_name(self: &Rc<Self>, renaming: Option<String>) {
        let weak = Rc::downgrade(self);
        let find = Rc::new(move |text: &str| {
            let name = text.trim().to_string();
            if !layouts::is_valid_name(&name) {
                return Vec::new();
            }
            let label = match &renaming {
                Some(_) => trf("將配置重新命名為「{0}」", &[&name]),
                None => trf("儲存為配置「{0}」", &[&name]),
            };
            let weak = weak.clone();
            let renaming = renaming.clone();
            vec![Item::new(label, "", move || {
                let Some(this) = weak.upgrade() else {
                    return;
                };
                let saved = match &renaming {
                    Some(old) => layouts::rename(old, &name),
                    None => layouts::save(&name, this.saved()),
                };
                this.report_layout(saved);
            })]
        });
        self.palette.open(Source { placeholder: tr("輸入配置名稱"), empty: "", counted: false, find });
    }

    /// Tells the user when a layout could not be written; the settings show the layouts as they now are.
    pub(crate) fn report_layout(&self, written: std::io::Result<()>) {
        if let Err(error) = written {
            eprintln!("nekoland-terminal: could not save the layouts: {error}");
            self.toasts.add_toast(adw::Toast::new(&trf("無法儲存配置：{0}", &[&error])));
        }
        super::refresh_settings();
    }

    /// Searches the lines of every tab of every window, read once as it opens.
    pub(super) fn open_tab_search(self: &Rc<Self>) {
        let lines = Rc::new(all_lines());
        let find = Rc::new(move |text: &str| found_lines(&lines, text));
        self.palette.open(Source { placeholder: tr("搜尋所有分頁"), empty: tr("沒有結果"), counted: true, find });
    }


}

/// A shortcut as GTK writes it for the user.
fn shortcut_label(accelerator: &str) -> String {
    gtk::accelerator_parse(accelerator)
        .map(|(key, modifiers)| gtk::accelerator_get_label(key, modifiers).to_string())
        .unwrap_or_default()
}

fn scheme_items() -> Vec<Item> {
    theme::all_schemes()
        .into_iter()
        .map(|scheme| {
            let name = scheme.name;
            Item::new(trf("色彩配置：{0}", &[&name]), "", move || {
                settings::change(|config| config.scheme = name.to_string());
            })
        })
        .collect()
}

/// The lines of a screen, its scrollback with them.
pub(super) fn screen_lines(screen: &vte4::Terminal) -> Vec<String> {
    let (first, count) = screen_rows::kept(screen);
    let last = first + count;
    let (text, _) = screen.text_range_format(vte4::Format::Text, first, 0, last, screen.column_count());
    text.map(|text| text.lines().map(|line| line.trim_end().to_string()).collect()).unwrap_or_default()
}

/// Every line of every tab with text on it, each once a tab.
fn all_lines() -> Vec<Line> {
    let mut lines = Vec::new();
    for window in all_windows() {
        for (tab, page) in window.pages() {
            let folder = page.folder_label().unwrap_or_default();
            let mut seen = std::collections::HashSet::new();
            for pane in page.panes() {
                for text in screen_lines(pane.screen.upcast_ref()) {
                    if !text.trim().is_empty() && seen.insert(text.clone()) {
                        lines.push(Line { window: Rc::downgrade(&window), tab: tab.clone(), folder: folder.clone(), text });
                    }
                }
            }
        }
    }
    lines
}

/// The lines that hold `text`, whatever its case; going to one opens the find bar of its tab on it.
fn found_lines(lines: &[Line], text: &str) -> Vec<Item> {
    let wanted = text.trim().to_lowercase();
    if wanted.is_empty() {
        return Vec::new();
    }
    lines
        .iter()
        .filter(|line| line.text.to_lowercase().contains(&wanted))
        .take(FOUND_LIMIT)
        .map(|line| {
            let (window, tab, query) = (line.window.clone(), line.tab.clone(), text.trim().to_string());
            Item::new(line.text.trim(), line.folder.clone(), move || {
                if let Some(window) = window.upgrade() {
                    window.find_in_tab(&tab, &query);
                }
            })
        })
        .collect()
}
