//! Font ligatures, as Nekominal's `font.ligatures`: `=>`, `>=`, `->` drawn the way the font draws them. VTE draws each
//! cell on its own and shapes nothing, so the screen's picture is drawn here: VTE's own, with the cells of each run of
//! operator characters a coding font joins masked out of it, and in their place the screen's background and the run
//! shaped by Pango with the font's `calt` and `liga`. The background is VTE's own colour at its own opacity, so a
//! see-through screen shows the window behind those cells as behind any other. A run keeps the colours and weight of
//! its cells; the cell under the cursor breaks a run, and a selection is left to VTE, as in Nekominal, where those
//! cells break a ligature too.

use gtk::{gdk, graphene, gsk, pango, prelude::*};
use unicode_width::UnicodeWidthChar;
use vte4::prelude::*;

use super::{
    config,
    hint_mode::{screen_font, screen_offset},
    markup::html_to_markup,
    screen::Painter,
    screen_rows,
    theme::{self, Scheme},
};

/// The characters the coding fonts make ligatures of.
const OPERATOR_CHARS: &str = "=<>!-+*/&|:.~#?%^$_;\\@";
/// The shortest run a font joins.
const SHORTEST_RUN: usize = 2;
/// Pango's font features for a run: the coding ligatures on, the discretionary ones off, as Nekominal shapes them.
const FEATURES: &str = "calt 1, liga 1, dlig 0";
/// What masks a run's cells out of VTE's picture: any opaque colour.
const MASK: gdk::RGBA = gdk::RGBA::BLACK;

/// A run of operator characters on one row, `start..end` its columns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Run {
    start: usize,
    end: usize,
}

/// A run on the screen: its row in the buffer and on the screen, and its cells' rectangle.
struct Placed {
    row: i64,
    run: Run,
    area: graphene::Rect,
}

/// The painter of a screen whose profile is `profile`: its text takes the profile's foreground.
pub fn painter(profile: &config::Profile) -> Painter {
    let scheme = theme::scheme_of(profile);
    Box::new(move |screen, snapshot, draw_screen| paint(scheme, screen, snapshot, draw_screen))
}

fn paint(scheme: &Scheme, screen: &vte4::Terminal, snapshot: &gtk::Snapshot, draw_screen: &dyn Fn(&gtk::Snapshot)) {
    let placed = if is_on(screen) { visible_runs(screen) } else { Vec::new() };
    if placed.is_empty() {
        draw_screen(snapshot);
        return;
    }
    snapshot.push_mask(gsk::MaskMode::InvertedAlpha);
    for run in &placed {
        snapshot.append_color(&MASK, &run.area);
    }
    snapshot.pop();
    draw_screen(snapshot);
    snapshot.pop();
    let background = screen.color_background_for_draw();
    let foreground = theme::colour(scheme.foreground);
    let font = screen_font(screen);
    for run in &placed {
        snapshot.append_color(&background, &run.area);
        draw_run(screen, snapshot, font.as_ref(), &foreground, run);
    }
}

fn is_on(screen: &vte4::Terminal) -> bool {
    config::current().ligatures && !screen.has_selection()
}

/// The runs of the rows in view, but the one under the cursor.
fn visible_runs(screen: &vte4::Terminal) -> Vec<Placed> {
    let first = screen_rows::first_shown(screen);
    let (cursor_column, cursor_row) = screen.cursor_position();
    let cursor_column = usize::try_from(cursor_column).unwrap_or(usize::MAX);
    let (cell_width, cell_height) = (screen.char_width() as f32, screen.char_height() as f32);
    let (left, top) = screen_offset(screen);
    let mut placed = Vec::new();
    for (index, row) in (first..first + screen.row_count()).enumerate() {
        for run in runs(&row_text(screen, row)) {
            if row == cursor_row && (run.start..run.end).contains(&cursor_column) {
                continue;
            }
            let area = graphene::Rect::new(
                left as f32 + run.start as f32 * cell_width,
                top as f32 + index as f32 * cell_height,
                (run.end - run.start) as f32 * cell_width,
                cell_height,
            );
            placed.push(Placed { row, run, area });
        }
    }
    placed
}

fn row_text(screen: &vte4::Terminal, row: i64) -> String {
    let (text, _) = screen.text_range_format(vte4::Format::Text, row, 0, row, screen.column_count());
    text.map(|text| text.trim_end_matches('\n').to_string()).unwrap_or_default()
}

/// The run shaped, in the colours and weight VTE gave its cells, centred in its row as VTE centres its text.
fn draw_run(
    screen: &vte4::Terminal,
    snapshot: &gtk::Snapshot,
    font: Option<&pango::FontDescription>,
    foreground: &gdk::RGBA,
    placed: &Placed,
) {
    let (start, end) = (placed.run.start as i64, placed.run.end as i64);
    let (html, _) = screen.text_range_format(vte4::Format::Html, placed.row, start, placed.row, end);
    let Some(markup) = html.map(|html| html_to_markup(&html)).filter(|markup| pango::parse_markup(markup, '\0').is_ok())
    else {
        return;
    };
    let layout = screen.create_pango_layout(None);
    layout.set_font_description(font);
    layout.set_markup(&markup);
    let attributes = layout.attributes().unwrap_or_default();
    attributes.insert(pango::AttrFontFeatures::new(FEATURES));
    layout.set_attributes(Some(&attributes));
    let (_, text_height) = layout.pixel_size();
    let y = placed.area.y() + (placed.area.height() - text_height as f32) / 2.0;
    snapshot.save();
    snapshot.translate(&graphene::Point::new(placed.area.x(), y));
    snapshot.append_layout(&layout, foreground);
    snapshot.restore();
}

/// The runs of operator characters in a row's text, by column: a wide character takes two.
fn runs(text: &str) -> Vec<Run> {
    let mut found = Vec::new();
    let mut column = 0;
    let mut start = None;
    for char in text.chars() {
        let is_operator = OPERATOR_CHARS.contains(char);
        match (is_operator, start) {
            (true, None) => start = Some(column),
            (false, Some(from)) => {
                push_run(&mut found, from, column);
                start = None;
            }
            _ => {}
        }
        column += char.width().unwrap_or(0);
    }
    if let Some(from) = start {
        push_run(&mut found, from, column);
    }
    found
}

fn push_run(found: &mut Vec<Run>, start: usize, end: usize) {
    if end - start >= SHORTEST_RUN {
        found.push(Run { start, end });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_of_two_or_more_operators_are_found_by_column() {
        assert_eq!(runs("a => b != c"), [Run { start: 2, end: 4 }, Run { start: 7, end: 9 }]);
        assert_eq!(runs("x = y"), []);
        assert_eq!(runs("a->"), [Run { start: 1, end: 3 }]);
    }

    #[test]
    fn a_wide_character_takes_two_columns() {
        assert_eq!(runs("中=>"), [Run { start: 2, end: 4 }]);
    }
}
