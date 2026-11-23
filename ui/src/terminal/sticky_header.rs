//! The prompt line of the command whose output is in view, pinned over the top row while the view is scrolled up, as
//! Nekominal's sticky scroll; a click on it scrolls to that prompt. VTE takes what it is fed in its own time, so where a
//! prompt landed is not known when its OSC 133 mark passes: the prompt's text is kept instead, and the prompt is the
//! nearest row above the top that starts with one of them. Without shell integration nothing is pinned.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use gtk::{gdk, glib, prelude::*};
use vte4::prelude::*;

use super::{
    config,
    hint_mode::{screen_font, screen_offset},
    markup::html_to_markup,
    theme::{self, Scheme},
};

/// How much of the foreground the header's background takes, so that it stands a step off the pane's own.
const TINT: f64 = 0.10;
/// The line under the header.
const RULE_ALPHA: f64 = 0.25;
const RULE_WIDTH: f64 = 1.0;
/// How far from the bottom the view may be and still count as at it: VTE's adjustment is in rows.
const BOTTOM_SLACK: f64 = 0.5;
/// How many of the latest different prompts are looked for: one changes with the folder, the branch, a new shell.
const PROMPTS_KEPT: usize = 8;
/// How far above the top a prompt is looked for, so that scrolling through a long output stays light.
const SEARCH_ROWS: i64 = 5000;

pub struct StickyHeader {
    area: gtk::DrawingArea,
    screen: vte4::Terminal,
    scheme: &'static Scheme,
    /// The text of the latest different prompts, the latest last.
    prompts: RefCell<Vec<String>>,
    /// The prompt pinned now.
    pinned: RefCell<Option<Pinned>>,
    /// The view the pin was looked for in, as `view` has it.
    looked_in: Cell<Option<View>>,
}

/// The settings' generation, the top row and whether the view is at the bottom.
type View = (u64, i64, bool);

struct Pinned {
    row: i64,
    text: String,
    /// The row in its own colours, as Pango markup; None when VTE's HTML of it does not read as markup.
    markup: Option<String>,
}

impl StickyHeader {
    /// The header of `screen`, to be put over it in an overlay; drawn in the colours of the pane's profile.
    pub fn new(screen: &impl IsA<vte4::Terminal>, profile: &config::Profile) -> Rc<Self> {
        let area = gtk::DrawingArea::builder().valign(gtk::Align::Start).visible(false).build();
        area.set_cursor_from_name(Some("pointer"));
        let header = Rc::new(Self {
            area,
            screen: screen.as_ref().clone(),
            scheme: theme::scheme_of(profile),
            prompts: RefCell::default(),
            pinned: RefCell::default(),
            looked_in: Cell::default(),
        });
        header.connect();
        header
    }

    pub fn widget(&self) -> &gtk::DrawingArea {
        &self.area
    }

    fn connect(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.area.set_draw_func(glib::clone!(
            #[strong]
            weak,
            move |area, cairo, width, height| {
                if let Some(header) = weak.upgrade() {
                    header.draw(area, cairo, f64::from(width), f64::from(height));
                }
            }
        ));
        let click = gtk::GestureClick::new();
        click.connect_released(glib::clone!(
            #[strong]
            weak,
            move |_, _, _, _| {
                if let Some(header) = weak.upgrade() {
                    header.scroll_to_prompt();
                }
            }
        ));
        self.area.add_controller(click);
        if let Some(adjustment) = self.screen.vadjustment() {
            self.follow(&adjustment);
        }
        self.screen.connect_vadjustment_notify(glib::clone!(
            #[strong]
            weak,
            move |screen| {
                if let (Some(header), Some(adjustment)) = (weak.upgrade(), screen.vadjustment()) {
                    header.follow(&adjustment);
                }
            }
        ));
    }

    fn follow(self: &Rc<Self>, adjustment: &gtk::Adjustment) {
        let weak = Rc::downgrade(self);
        adjustment.connect_value_changed(move |_| {
            if let Some(header) = weak.upgrade() {
                header.refresh();
            }
        });
    }

    /// The text a prompt was drawn with, its last line: the one the command is typed on.
    pub fn remember_prompt(&self, text: String) {
        let mut prompts = self.prompts.borrow_mut();
        prompts.retain(|prompt| *prompt != text);
        prompts.push(text);
        if prompts.len() > PROMPTS_KEPT {
            prompts.remove(0);
        }
    }

    /// Refreshes after output, when it can change the pin: the rows above the top stay as they were, so only a new
    /// prompt, a move of the view or a change of the settings can.
    pub fn refresh_after_output(&self, new_prompt: bool) {
        if new_prompt || self.view() != self.looked_in.get() {
            self.refresh();
        }
    }

    fn view(&self) -> Option<View> {
        let adjustment = self.screen.vadjustment()?;
        let at_bottom = adjustment.value() + adjustment.page_size() + BOTTOM_SLACK >= adjustment.upper();
        Some((config::generation(), adjustment.value() as i64, at_bottom))
    }

    /// Pins the prompt of the output at the top of the view, or none, as the view and the settings are now.
    pub fn refresh(&self) {
        self.looked_in.set(self.view());
        let pinned = self.find_pinned();
        self.area.set_visible(pinned.is_some());
        if let Some(pinned) = &pinned {
            self.area.set_content_height(self.header_height());
            self.area.set_tooltip_text(Some(&pinned.text));
        }
        *self.pinned.borrow_mut() = pinned;
        self.area.queue_draw();
    }

    fn find_pinned(&self) -> Option<Pinned> {
        if !config::current().sticky_header {
            return None;
        }
        let adjustment = self.screen.vadjustment()?;
        if adjustment.value() + adjustment.page_size() + BOTTOM_SLACK >= adjustment.upper() {
            return None;
        }
        let prompts = self.prompts.borrow();
        if prompts.is_empty() {
            return None;
        }
        let top = adjustment.value() as i64;
        let first = (adjustment.lower() as i64).max(top - SEARCH_ROWS);
        let is_prompt = |text: &str| prompts.iter().any(|prompt| text.starts_with(prompt));
        // The rows above the top are read at once. A line wrapped over several rows has no newline between them, so
        // the nearest prompt's line starts at most as many rows above the top as lines follow it; rows below that
        // are passed over, and the row it starts on is looked for one by one from there.
        let columns = self.screen.column_count();
        let (above, _) = self.screen.text_range_format(vte4::Format::Text, first, 0, top - 1, columns);
        let lines_after = above?.lines().rev().position(|line| is_prompt(line.trim_end()))?;
        let latest = top - 1 - lines_after as i64;
        let mut found = None;
        let row = pinned_prompt(top, first, |row| {
            if row != top && row > latest {
                return false;
            }
            let text = self.row_text(row);
            let matched = is_prompt(&text);
            if matched {
                found = Some(text);
            }
            matched
        })?;
        Some(Pinned { row, text: found.unwrap_or_default(), markup: self.row_markup(row) })
    }

    fn row_markup(&self, row: i64) -> Option<String> {
        let (html, _) = self.screen.text_range_format(vte4::Format::Html, row, 0, row, self.screen.column_count());
        let markup = html_to_markup(&html?);
        gtk::pango::parse_markup(&markup, '\0').is_ok().then_some(markup)
    }

    fn row_text(&self, row: i64) -> String {
        let (text, _) = self.screen.text_range_format(vte4::Format::Text, row, 0, row, self.screen.column_count());
        text.map(|text| text.trim_end().to_string()).unwrap_or_default()
    }

    fn header_height(&self) -> i32 {
        let (_, top) = screen_offset(&self.screen);
        top as i32 + self.screen.char_height() as i32
    }

    fn scroll_to_prompt(&self) {
        let Some(row) = self.pinned.borrow().as_ref().map(|pinned| pinned.row) else {
            return;
        };
        if let Some(adjustment) = self.screen.vadjustment() {
            adjustment.set_value(row as f64);
        }
    }

    fn draw(&self, area: &gtk::DrawingArea, cairo: &gtk::cairo::Context, width: f64, height: f64) {
        let pinned = self.pinned.borrow();
        let Some(pinned) = pinned.as_ref() else {
            return;
        };
        let background = theme::colour(self.scheme.background);
        let foreground = theme::colour(self.scheme.foreground);
        set_colour(cairo, &background, 1.0);
        cairo.rectangle(0.0, 0.0, width, height);
        let mut drawn = cairo.fill();
        set_colour(cairo, &foreground, TINT);
        cairo.rectangle(0.0, 0.0, width, height);
        drawn = drawn.and_then(|()| cairo.fill());
        set_colour(cairo, &foreground, RULE_ALPHA);
        cairo.rectangle(0.0, height - RULE_WIDTH, width, RULE_WIDTH);
        drawn = drawn.and_then(|()| cairo.fill());
        if let Err(error) = drawn {
            eprintln!("nekoland-terminal: could not draw the pinned prompt: {error}");
            return;
        }
        let layout = area.create_pango_layout(None);
        match &pinned.markup {
            Some(markup) => layout.set_markup(markup),
            None => layout.set_text(&pinned.text),
        }
        layout.set_font_description(screen_font(&self.screen).as_ref());
        let (left, top) = screen_offset(&self.screen);
        set_colour(cairo, &foreground, 1.0);
        cairo.move_to(left, top);
        pangocairo::functions::show_layout(cairo, &layout);
    }
}

fn set_colour(cairo: &gtk::cairo::Context, colour: &gdk::RGBA, alpha: f64) {
    cairo.set_source_rgba(f64::from(colour.red()), f64::from(colour.green()), f64::from(colour.blue()), alpha);
}

/// The prompt pinned over a view whose top row is `top`: the nearest prompt above it, down to row `first`, whose
/// command's output then reaches the top; none when the top is a prompt, which is in sight.
fn pinned_prompt(top: i64, first: i64, mut is_prompt: impl FnMut(i64) -> bool) -> Option<i64> {
    if is_prompt(top) {
        return None;
    }
    (first..top).rev().find(|row| is_prompt(*row))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROMPTS: [i64; 3] = [0, 10, 30];

    fn is_prompt(row: i64) -> bool {
        PROMPTS.contains(&row)
    }

    #[test]
    fn the_prompt_above_the_top_is_pinned() {
        assert_eq!(pinned_prompt(15, 0, is_prompt), Some(10));
        assert_eq!(pinned_prompt(31, 0, is_prompt), Some(30));
    }

    #[test]
    fn none_is_pinned_on_a_prompt_or_past_the_search() {
        assert_eq!(pinned_prompt(10, 0, is_prompt), None);
        assert_eq!(pinned_prompt(9, 0, is_prompt), Some(0));
        assert_eq!(pinned_prompt(9, 1, is_prompt), None);
    }
}