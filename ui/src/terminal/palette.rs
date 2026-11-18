//! Nekominal's command palette: a card over the top of the window with a line to type in and what it finds under it,
//! each entry with its shortcut or where it is on the right. What it lists comes from a [`Source`] the window hands it:
//! the commands and the other entries found by a fuzzy match, a name being typed for a layout, or the lines of every tab.

use std::{cell::RefCell, rc::Rc};

use gtk::{gdk, glib, prelude::*};

const WIDTH: i32 = 640;
const LIST_HEIGHT: i32 = 320;
const TOP_MARGIN: i32 = 24;
const PAD: i32 = 6;

pub fn css() -> &'static str {
    r#"
.term-palette { background-color: @popover_bg_color; color: @popover_fg_color; border-radius: 12px;
  box-shadow: 0 0 0 1px alpha(black, 0.1), 0 6px 18px 2px alpha(black, 0.28); }
.term-palette entry, .term-palette entry:focus-within { background: none; box-shadow: none; outline: none; border: none; }
.term-palette-entry { min-height: 52px; font-size: 15px; }
.term-palette list { background: none; }
.term-palette list > row { min-height: 0; padding: 9px 14px; margin: 0 6px 2px; border-radius: 6px; }
"#
}

/// An entry: what it says, what is on its right (a shortcut, a folder), and what choosing it does.
pub struct Item {
    pub label: String,
    pub hint: String,
    pub run: Rc<dyn Fn()>,
}

impl Item {
    pub fn new(label: impl Into<String>, hint: impl Into<String>, run: impl Fn() + 'static) -> Self {
        Self { label: label.into(), hint: hint.into(), run: Rc::new(run) }
    }
}

/// What a source finds for what is typed.
pub type Find = Rc<dyn Fn(&str) -> Vec<Item>>;

/// What the palette lists for what is typed.
pub struct Source {
    pub placeholder: &'static str,
    /// What it says when nothing is found.
    pub empty: &'static str,
    /// Whether the number found shows at the right of the line, as a search's does.
    pub counted: bool,
    pub find: Find,
}

pub struct Palette {
    pub widget: gtk::Box,
    entry: gtk::SearchEntry,
    count: gtk::Label,
    list: gtk::ListBox,
    scroller: gtk::ScrolledWindow,
    empty: gtk::Label,
    source: RefCell<Option<Source>>,
    items: RefCell<Vec<Item>>,
    /// Where the focus was before the palette took it, given back when it closes.
    return_focus: RefCell<Option<gtk::Widget>>,
}

impl Palette {
    pub fn new() -> Rc<Self> {
        let entry = gtk::SearchEntry::builder().hexpand(true).css_classes(["term-palette-entry"]).build();
        let count = gtk::Label::builder().css_classes(["dim-label", "numeric"]).margin_end(PAD * 2).build();
        let line = gtk::Box::builder().margin_start(PAD).margin_end(PAD).build();
        line.append(&entry);
        line.append(&count);
        let list = gtk::ListBox::builder().css_classes(["navigation-sidebar"]).build();
        let empty = gtk::Label::builder()
            .css_classes(["dim-label"])
            .valign(gtk::Align::Start)
            .margin_top(PAD * 3)
            .margin_bottom(PAD * 3)
            .visible(false)
            .build();
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .min_content_height(LIST_HEIGHT)
            .max_content_height(LIST_HEIGHT)
            .child(&list)
            .build();
        let widget = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .width_request(WIDTH)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Start)
            .margin_top(TOP_MARGIN)
            .css_classes(["term-palette"])
            .visible(false)
            .build();
        widget.append(&line);
        widget.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        // Nekominal's palette keeps its height as what it finds changes; saying none is found sits over the list.
        let found = gtk::Overlay::builder().child(&scroller).build();
        found.add_overlay(&empty);
        widget.append(&found);
        let this = Rc::new(Self {
            widget,
            entry,
            count,
            list,
            scroller: scroller.clone(),
            empty,
            source: RefCell::default(),
            items: RefCell::default(),
            return_focus: RefCell::default(),
        });
        this.connect();
        this
    }

    fn connect(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.entry.connect_search_changed(glib::clone!(
            #[strong]
            weak,
            move |_| {
                if let Some(this) = weak.upgrade() {
                    this.show_found();
                }
            }
        ));
        self.entry.connect_activate(glib::clone!(
            #[strong]
            weak,
            move |_| {
                if let Some(this) = weak.upgrade() {
                    this.choose(this.list.selected_row().map(|row| row.index()));
                }
            }
        ));
        self.entry.connect_stop_search(glib::clone!(
            #[strong]
            weak,
            move |_| {
                if let Some(this) = weak.upgrade() {
                    this.close();
                }
            }
        ));
        self.list.connect_row_activated(glib::clone!(
            #[strong]
            weak,
            move |_, row| {
                if let Some(this) = weak.upgrade() {
                    this.choose(Some(row.index()));
                }
            }
        ));
        // The arrows move through the entries while the focus stays in the line.
        let keys = gtk::EventControllerKey::builder().propagation_phase(gtk::PropagationPhase::Capture).build();
        keys.connect_key_pressed(glib::clone!(
            #[strong]
            weak,
            move |_, key, _, _| {
                let Some(this) = weak.upgrade() else {
                    return glib::Propagation::Proceed;
                };
                let step = match key {
                    gdk::Key::Down => 1,
                    gdk::Key::Up => -1,
                    _ => return glib::Propagation::Proceed,
                };
                this.move_selection(step);
                glib::Propagation::Stop
            }
        ));
        self.entry.add_controller(keys);
        // A click elsewhere takes the focus away, and closes it as Nekominal's does.
        let focus = gtk::EventControllerFocus::new();
        focus.connect_leave(glib::clone!(
            #[strong]
            weak,
            move |_| {
                if let Some(this) = weak.upgrade() {
                    // After the focus has moved, so that a click on an entry is still taken.
                    glib::idle_add_local_once(move || {
                        if !this.widget.has_focus() && this.widget.focus_child().is_none() {
                            this.hide();
                        }
                    });
                }
            }
        ));
        self.widget.add_controller(focus);
    }

    /// Opens with what `source` lists; open already, it lists that instead.
    pub fn open(&self, source: Source) {
        self.entry.set_placeholder_text(Some(source.placeholder));
        self.empty.set_text(source.empty);
        self.count.set_visible(source.counted);
        self.source.replace(Some(source));
        if !self.widget.is_visible() {
            let focus = self.widget.root().and_then(|root| root.focus());
            self.return_focus.replace(focus);
        }
        self.widget.set_visible(true);
        self.entry.set_text("");
        self.show_found();
        self.entry.grab_focus();
    }

    pub fn is_open(&self) -> bool {
        self.widget.is_visible()
    }

    /// Closes and gives the focus back to where it was.
    pub fn close(&self) {
        self.hide();
        if let Some(focus) = self.return_focus.take() {
            focus.grab_focus();
        }
    }

    fn hide(&self) {
        self.widget.set_visible(false);
        self.source.replace(None);
        self.items.replace(Vec::new());
    }

    /// Lists what the source finds for what is typed, the first entry selected.
    fn show_found(&self) {
        let text = self.entry.text();
        let items = match self.source.borrow().as_ref() {
            Some(source) => (source.find)(text.as_str()),
            None => Vec::new(),
        };
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        for item in &items {
            self.list.append(&row(item));
        }
        self.count.set_text(&items.len().to_string());
        self.empty.set_visible(items.is_empty());
        self.list.select_row(self.list.row_at_index(0).as_ref());
        self.items.replace(items);
    }

    fn move_selection(&self, step: i32) {
        let count = self.items.borrow().len() as i32;
        if count == 0 {
            return;
        }
        let current = self.list.selected_row().map_or(0, |row| row.index());
        let next = (current + step).rem_euclid(count);
        if let Some(row) = self.list.row_at_index(next) {
            self.list.select_row(Some(&row));
            self.scroll_to(&row);
        }
    }

    /// Scrolls the list so that `row` shows, the focus left in the line.
    fn scroll_to(&self, row: &gtk::ListBoxRow) {
        let Some(bounds) = row.compute_bounds(&self.list) else {
            return;
        };
        let top = f64::from(bounds.y());
        self.scroller.vadjustment().clamp_page(top, top + f64::from(bounds.height()));
    }

    /// Does what the entry at `index` does, after the palette has closed: an entry may open it again.
    fn choose(&self, index: Option<i32>) {
        let run = index
            .and_then(|index| self.items.borrow().get(usize::try_from(index).ok()?).map(|item| item.run.clone()));
        let Some(run) = run else {
            return;
        };
        self.close();
        run();
    }
}

fn row(item: &Item) -> gtk::ListBoxRow {
    let line = gtk::Box::builder().spacing(12).build();
    line.append(
        &gtk::Label::builder()
            .label(&item.label)
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build(),
    );
    if !item.hint.is_empty() {
        line.append(&gtk::Label::builder().label(&item.hint).css_classes(["dim-label", "caption"]).build());
    }
    gtk::ListBoxRow::builder().child(&line).build()
}

/// How well `query` matches `text`, as Nekominal's palette scores it: its letters in order, more for letters that
/// follow one another and that start a word, and a little for a shorter text. None when it does not match.
pub fn fuzzy_score(query: &str, text: &str) -> Option<f64> {
    const LETTER: f64 = 1.0;
    const FOLLOWING: f64 = 2.0;
    const WORD_START: f64 = 3.0;
    const LENGTH_COST: f64 = 0.01;
    let needle: Vec<char> = query.trim().to_lowercase().chars().filter(|letter| *letter != ' ').collect();
    if needle.is_empty() {
        return Some(0.0);
    }
    let haystack: Vec<char> = text.to_lowercase().chars().collect();
    let mut score = 0.0;
    let mut position = 0;
    let mut previous: Option<usize> = None;
    for letter in needle {
        let found = haystack[position..].iter().position(|candidate| *candidate == letter)? + position;
        score += LETTER;
        if previous.is_some_and(|previous| found == previous + 1) {
            score += FOLLOWING;
        }
        let starts_word = found == 0
            || haystack
                .get(found - 1)
                .is_some_and(|before| before.is_whitespace() || matches!(before, ':' | '：' | '-' | '_' | '/'));
        if starts_word {
            score += WORD_START;
        }
        previous = Some(found);
        position = found + 1;
    }
    Some(score - haystack.len() as f64 * LENGTH_COST)
}

/// The items `query` matches, the best first; an empty query keeps their order.
pub fn rank(items: Vec<Item>, query: &str) -> Vec<Item> {
    if query.trim().is_empty() {
        return items;
    }
    let mut scored: Vec<(f64, Item)> =
        items.into_iter().filter_map(|item| fuzzy_score(query, &item.label).map(|score| (score, item))).collect();
    // A stable sort: equal scores keep their order.
    scored.sort_by(|left, right| right.0.total_cmp(&left.0));
    scored.into_iter().map(|(_, item)| item).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_in_order_match_and_others_do_not() {
        assert!(fuzzy_score("nt", "新增分頁 New Tab").is_some());
        assert!(fuzzy_score("xyz", "New Tab").is_none());
        assert_eq!(fuzzy_score("  ", "anything"), Some(0.0));
    }

    #[test]
    fn a_word_start_ranks_before_letters_inside_words() {
        let labels = ["片段：git status", "設定：Shell 整合", "切換到分頁：build"];
        let items = labels.iter().map(|label| Item::new(*label, "", || {})).collect();
        let ranked: Vec<String> = rank(items, "gst").into_iter().map(|item| item.label).collect();
        assert_eq!(ranked.first().map(String::as_str), Some("片段：git status"));
    }
}
