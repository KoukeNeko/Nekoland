//! What the user keeps to use again, as Nekominal's settings list it: the snippets on the Terminal page, each a name and
//! the text it types, and the saved window layouts on the Profiles page, each opened, renamed or deleted from its row.

use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;

use super::{
    lang::{tr, trf},
    config::Snippet,
    layouts,
    profile_settings::{icon_button, line_row},
    settings::{self, change},
    trigger_settings::{shown_text, typed_text},
    window,
};

pub const SNIPPETS: &str = "文字片段";
pub const LAYOUTS: &str = "已儲存的視窗配置";
const SNIPPETS_NOTE: &str = "從命令選擇區或建議清單輸入的文字";
const NAME_WIDTH: i32 = 160;
const NEEDS_VALUE: &str = "不能空白";
const NEW_SNIPPET: &str = "片段";

type Rerender = Rc<dyn Fn()>;

fn delete_button(tooltip: &str) -> gtk::Button {
    gtk::Button::builder()
        .icon_name("user-trash-symbolic")
        .tooltip_text(tooltip)
        .css_classes(["flat"])
        .valign(gtk::Align::Center)
        .build()
}

fn change_snippet(index: usize, edit: impl FnOnce(&mut Snippet)) {
    change(|config| {
        if let Some(snippet) = config.snippets.get_mut(index) {
            edit(snippet);
        }
    });
}

/// The snippets under their heading, then the button that adds one.
pub fn snippet_widgets(rerender: &Rerender) -> Vec<gtk::Widget> {
    let snippets = super::config::current().snippets;
    let heading = settings::heading(SNIPPETS);
    let note = gtk::Label::builder().label(tr(SNIPPETS_NOTE)).xalign(0.0).css_classes(["dim-label"]).margin_bottom(8).build();
    let mut widgets: Vec<gtk::Widget> = vec![heading.upcast(), note.upcast()];
    if !snippets.is_empty() {
        let plates = settings::plates();
        for (index, snippet) in snippets.iter().enumerate() {
            plates.append(&snippet_row(index, snippet, rerender));
        }
        widgets.push(plates.upcast());
    }
    let add = icon_button("list-add-symbolic", tr("新增片段"));
    add.connect_clicked(glib::clone!(
        #[strong]
        rerender,
        move |_| {
            change(|config| {
                let name = format!("{} {}", tr(NEW_SNIPPET), config.snippets.len() + 1);
                config.snippets.push(Snippet { name, input: String::new() });
            });
            rerender();
        }
    ));
    let adding = settings::plates();
    adding.append(&line_row(&[add.upcast()]));
    widgets.push(adding.upcast());
    widgets
}

fn snippet_row(index: usize, snippet: &Snippet, rerender: &Rerender) -> gtk::ListBoxRow {
    let name = settings::field(&snippet.name, move |field, text| {
        let text = text.trim().to_string();
        if text.is_empty() {
            settings::mark_refused(field, Some(NEEDS_VALUE));
            return;
        }
        settings::mark_refused(field, None);
        change_snippet(index, |snippet| snippet.name = text);
    });
    name.set_width_request(NAME_WIDTH);
    let input = settings::field(&shown_text(&snippet.input), move |_, text| {
        change_snippet(index, |snippet| snippet.input = typed_text(&text));
    });
    input.set_hexpand(true);
    input.set_placeholder_text(Some(tr("要輸入的文字，\\r 代表 Enter")));
    let delete = delete_button(tr("刪除片段"));
    delete.connect_clicked(glib::clone!(
        #[strong]
        rerender,
        move |_| {
            change(|config| {
                if index < config.snippets.len() {
                    config.snippets.remove(index);
                }
            });
            rerender();
        }
    ));
    line_row(&[name.upcast(), input.upcast(), delete.upcast()])
}

/// The layouts under their heading, the button that saves the window in front as one at its right.
pub fn layout_widgets() -> Vec<gtk::Widget> {
    let save = icon_button("document-save-symbolic", tr("儲存視窗配置"));
    save.connect_clicked(|_| {
        if let Some(front) = window::front_window() {
            front.ask_layout_name(None);
        }
    });
    let head = gtk::Box::builder().spacing(12).build();
    let heading = settings::heading(LAYOUTS);
    heading.set_hexpand(true);
    head.append(&heading);
    head.append(&save);
    let mut widgets: Vec<gtk::Widget> = vec![head.upcast()];
    let saved = layouts::load();
    if saved.is_empty() {
        return widgets;
    }
    let plates = settings::plates();
    for (name, tabs) in &saved {
        plates.append(&layout_row(name, tabs.len()));
    }
    widgets.push(plates.upcast());
    widgets
}

fn layout_row(name: &str, tab_count: usize) -> gtk::ListBoxRow {
    let old = name.to_string();
    let field = settings::field(name, move |field, text| {
        let text = text.trim().to_string();
        if !layouts::is_valid_name(&text) {
            settings::mark_refused(field, Some(NEEDS_VALUE));
            return;
        }
        settings::mark_refused(field, None);
        if text != old {
            report(layouts::rename(&old, &text));
        }
    });
    field.set_hexpand(true);
    let count = gtk::Label::builder().label(trf("{0} 個分頁", &[&tab_count])).css_classes(["dim-label", "caption"]).build();
    let open = gtk::Button::builder().label(tr("開啟")).valign(gtk::Align::Center).build();
    let opened = name.to_string();
    open.connect_clicked(move |_| {
        if let Some(front) = window::front_window() {
            front.open_layout(&opened, false);
        }
    });
    let delete = delete_button(tr("刪除配置"));
    let gone = name.to_string();
    delete.connect_clicked(move |_| report(layouts::delete(&gone)));
    line_row(&[field.upcast(), count.upcast(), open.upcast(), delete.upcast()])
}

/// Tells the window in front when the layouts could not be written, and shows them as they now are.
fn report(written: std::io::Result<()>) {
    match window::front_window() {
        Some(front) => front.report_layout(written),
        None => {
            if let Err(error) = written {
                eprintln!("nekoland-terminal: could not save the layouts: {error}");
            }
        }
    }
}
