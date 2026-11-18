//! A screen's rows by the numbers VTE reads its text with. VTE 0.76 counts its scroll view from 0 at the first row its
//! buffer holds, while its text and cursor are numbered by the buffer's own rows, which start further on once the
//! scrollback is full and on the alternate screen, numbered after the normal one's. VTE does not tell the difference,
//! so it is found by matching the text shown with the rows it could be, the cursor being on the screen's last page, and
//! kept for each screen while the text still matches.

use std::{cell::RefCell, collections::HashMap};

use gtk::prelude::*;
use vte4::prelude::*;

thread_local! {
    /// The difference last found for each screen, by its address: checked before it is used, so a reused address only
    /// costs a search.
    static OFFSETS: RefCell<HashMap<usize, i64>> = RefCell::new(HashMap::new());
}

/// The scroll view's numbers on a screen: its value, the end of its range, and the rows of a page.
struct View {
    value: i64,
    upper: i64,
    page: i64,
}

fn view_of(screen: &vte4::Terminal) -> Option<View> {
    let adjustment = screen.vadjustment()?;
    Some(View { value: adjustment.value() as i64, upper: adjustment.upper() as i64, page: adjustment.page_size() as i64 })
}

/// The row at the top of what the screen shows.
pub fn first_shown(screen: &vte4::Terminal) -> i64 {
    view_of(screen).map_or(0, |view| view.value + offset(screen, &view))
}

/// The row at the top of the screen's last page, where programs write and the cursor is.
pub fn live_top(screen: &vte4::Terminal) -> i64 {
    view_of(screen).map_or(0, |view| view.upper - view.page + offset(screen, &view))
}

/// The first row the screen still holds, scrollback with it, and how many it holds.
pub fn kept(screen: &vte4::Terminal) -> (i64, i64) {
    view_of(screen).map_or((0, 0), |view| (offset(screen, &view), view.upper))
}

fn offset(screen: &vte4::Terminal, view: &View) -> i64 {
    let key = screen.as_ptr() as usize;
    let shown = shown_text(screen);
    let cached = OFFSETS.with(|offsets| offsets.borrow().get(&key).copied());
    if let Some(offset) = cached.filter(|offset| shows(screen, view.value + offset, &shown)) {
        return offset;
    }
    let found = find_offset(screen, view, &shown);
    OFFSETS.with(|offsets| offsets.borrow_mut().insert(key, found));
    found
}

/// None differs while the scrollback has room on the normal screen; otherwise the cursor, on the last page, bounds it.
fn find_offset(screen: &vte4::Terminal, view: &View, shown: &str) -> i64 {
    let (_, cursor_row) = screen.cursor_position();
    let last_page_top = view.upper - view.page;
    let candidates = (cursor_row - last_page_top - view.page + 1)..=(cursor_row - last_page_top);
    std::iter::once(0)
        .chain(candidates)
        .find(|offset| shows(screen, view.value + offset, shown))
        .unwrap_or_default()
}

fn shown_text(screen: &vte4::Terminal) -> String {
    screen.text_format(vte4::Format::Text).map(|text| text.trim_end().to_string()).unwrap_or_default()
}

/// Whether the rows from `top` read as what the screen shows.
fn shows(screen: &vte4::Terminal, top: i64, shown: &str) -> bool {
    let last = top + screen.row_count() - 1;
    let (text, _) = screen.text_range_format(vte4::Format::Text, top, 0, last, screen.column_count());
    text.is_some_and(|text| text.trim_end() == shown)
}
