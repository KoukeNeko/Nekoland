//! How Claude Code's use of its context shows: a meter coloured by how full it is, the line of the context under a row
//! of the sidebar, and the card the bar's meter opens, as Nekominal's: the window, what fills it, and the session's cost,
//! time, tokens and lines.

use gtk::prelude::*;

use super::lang::{tr, trf};
use super::usage::{self, Usage};

/// Past these shares of the window the meter warns, then alarms.
const WARN_SHARE: f64 = 70.0;
const ALARM_SHARE: f64 = 90.0;
const FULL: f64 = 100.0;
const CARD_WIDTH: i32 = 380;
const CARD_PAD: i32 = 18;
const BAR_HEIGHT: i32 = 8;
const SWATCH: i32 = 10;
/// The colour of each part of the window, as the card's swatches and bar show them.
const READ_CLASS: &str = "term-part-read";
const WRITE_CLASS: &str = "term-part-write";
const NEW_CLASS: &str = "term-part-new";
const FREE_CLASS: &str = "term-part-free";

pub fn css() -> &'static str {
    r#"
.term-meter trough, .term-meter progress { min-height: 4px; border-radius: 2px; }
.term-meter.warning progress { background-color: @warning_color; }
.term-meter.error progress { background-color: @error_color; }
.term-context-button > button { padding: 2px 8px; font-weight: normal; }
.term-context-card { padding: 18px; }
.term-share { font-size: 22px; font-weight: 700; }
.term-parts { border-radius: 4px; background: alpha(currentColor, 0.12); }
.term-part-read { background: @blue_3; }
.term-part-write { background: @orange_3; }
.term-part-new { background: @green_3; }
.term-part-free { background: alpha(currentColor, 0.12); }
.term-swatch { border-radius: 2px; }
.term-added { color: @success_color; }
.term-removed { color: @error_color; }
"#
}

/// A meter of `share` of the window, in the accent colour, warning past [`WARN_SHARE`] and alarming past [`ALARM_SHARE`].
pub fn meter(share: f64, width: i32) -> gtk::ProgressBar {
    let bar = gtk::ProgressBar::builder()
        .fraction(share.clamp(0.0, FULL) / FULL)
        .width_request(width)
        .valign(gtk::Align::Center)
        .css_classes(["term-meter"])
        .build();
    if share > ALARM_SHARE {
        bar.add_css_class("error");
    } else if share > WARN_SHARE {
        bar.add_css_class("warning");
    }
    bar
}

/// The line of the context under a row: the meter, then the share and the cost.
pub fn context_line(share: f64, cost: Option<f64>) -> gtk::Box {
    let line = gtk::Box::builder().spacing(8).build();
    let bar = meter(share, -1);
    bar.set_hexpand(true);
    line.append(&bar);
    line.append(&caption(&usage::context_text(share, cost)));
    line
}

fn caption(text: &str) -> gtk::Label {
    gtk::Label::builder().label(text).css_classes(["caption", "dim-label"]).build()
}

fn label(text: &str, classes: &[&str]) -> gtk::Label {
    gtk::Label::builder().label(text).xalign(0.0).css_classes(classes).build()
}

/// A share of `total` as a percentage with a decimal, as the card's rows say it.
fn percent(part: u64, total: u64) -> String {
    if total == 0 {
        return String::new();
    }
    format!("{:.1}%", part as f64 * FULL / total as f64)
}

/// A whole number with its thousands grouped: `1,204`.
fn grouped(number: u64) -> String {
    const GROUP: usize = 3;
    let digits = number.to_string();
    let mut text = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(GROUP) {
            text.push(',');
        }
        text.push(digit);
    }
    text
}

/// The parts of the window in use and free: read from the cache, written to it, new, and what is left.
fn parts(usage: &Usage, window: u64) -> [(&'static str, &'static str, u64); 4] {
    let read = usage.cache_read_tokens.unwrap_or(0);
    let write = usage.cache_creation_tokens.unwrap_or(0);
    let new = usage.input_tokens.unwrap_or(0);
    let free = window.saturating_sub(read + write + new);
    [
        (tr("從快取讀取"), READ_CLASS, read),
        (tr("寫入快取"), WRITE_CLASS, write),
        (tr("新的輸入"), NEW_CLASS, new),
        (tr("剩餘"), FREE_CLASS, free),
    ]
}

/// The window as one bar, each part as wide as its share.
fn parts_bar(parts: &[(&str, &str, u64)], window: u64) -> gtk::Box {
    let inner = CARD_WIDTH - 2 * CARD_PAD;
    let bar = gtk::Box::builder()
        .height_request(BAR_HEIGHT)
        .overflow(gtk::Overflow::Hidden)
        .css_classes(["term-parts"])
        .build();
    for (_, class, tokens) in parts.iter().filter(|(_, class, _)| *class != FREE_CLASS) {
        let width = (*tokens as f64 / window as f64 * f64::from(inner)).round() as i32;
        if width > 0 {
            bar.append(&gtk::Box::builder().width_request(width).css_classes([*class]).build());
        }
    }
    bar
}

fn swatch(class: &str) -> gtk::Box {
    gtk::Box::builder()
        .width_request(SWATCH)
        .height_request(SWATCH)
        .valign(gtk::Align::Center)
        .css_classes(["term-swatch", class])
        .build()
}

/// The head of the card: the model and the window, then how much of it is used.
fn head(usage: &Usage, window: Option<u64>) -> gtk::Box {
    let title = gtk::Box::builder().spacing(8).build();
    title.append(&label(tr("脈絡"), &["heading"]));
    title.append(&label(usage.model.as_deref().unwrap_or_default(), &["dim-label"]));
    let size = window.map(|window| trf("{0} 視窗", &[&usage::window_text(window)])).unwrap_or_default();
    let size = label(&size, &["dim-label"]);
    size.set_hexpand(true);
    size.set_xalign(1.0);
    title.append(&size);
    let share = gtk::Box::builder().spacing(6).build();
    share.append(&label(&format!("{:.0}%", usage.context.unwrap_or(0.0)), &["term-share"]));
    let used = label(tr("已使用"), &["dim-label"]);
    used.set_valign(gtk::Align::Baseline);
    share.append(&used);
    if let (Some(used), Some(window)) = (usage.used_tokens(), window) {
        let amount = format!("{} / {}", usage::tokens_text(used), usage::window_text(window));
        let amount = label(&amount, &["dim-label"]);
        amount.set_hexpand(true);
        amount.set_xalign(1.0);
        share.append(&amount);
    }
    let head = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(10).build();
    head.append(&title);
    head.append(&share);
    head
}

/// Rows of a name and its value, the names in a column.
fn table(rows: &[(gtk::Widget, gtk::Widget, Option<String>)]) -> gtk::Grid {
    let grid = gtk::Grid::builder().row_spacing(8).column_spacing(12).build();
    for (line, (name, value, share)) in rows.iter().enumerate() {
        let line = line as i32;
        name.set_hexpand(true);
        grid.attach(name, 0, line, 1, 1);
        grid.attach(value, 1, line, 1, 1);
        if let Some(share) = share {
            let share = label(share, &["dim-label", "numeric"]);
            share.set_xalign(1.0);
            share.set_width_chars(6);
            grid.attach(&share, 2, line, 1, 1);
        }
    }
    grid
}

fn named_swatch(name: &str, class: &str) -> gtk::Widget {
    let line = gtk::Box::builder().spacing(10).build();
    line.append(&swatch(class));
    line.append(&label(name, &[]));
    line.upcast()
}

fn value(text: &str) -> gtk::Widget {
    let value = label(text, &["numeric"]);
    value.set_xalign(1.0);
    value.upcast()
}

/// What the session cost, how long it took, the tokens it moved, its last reply and the lines it changed: the rows
/// the status line said.
fn session_rows(usage: &Usage) -> Vec<(gtk::Widget, gtk::Widget, Option<String>)> {
    let name = |text: &str| label(text, &["dim-label"]).upcast::<gtk::Widget>();
    let mut rows = Vec::new();
    if let Some(cost) = usage.cost {
        rows.push((name(tr("費用")), value(&usage::cost_text(cost)), None));
    }
    if let Some(duration) = usage.duration_ms {
        let time = match usage.api_duration_ms {
            Some(api) => format!("{} · API {}", usage::clock(duration), usage::clock(api)),
            None => usage::clock(duration),
        };
        rows.push((name(tr("時間")), value(&time), None));
    }
    if let (Some(input), Some(output)) = (usage.total_input_tokens, usage.total_output_tokens) {
        let tokens = trf("{0} 輸入 · {1} 輸出", &[&usage::tokens_text(input), &usage::tokens_text(output)]);
        rows.push((name("Token"), value(&tokens), None));
    }
    if let Some(output) = usage.output_tokens {
        rows.push((name("最後一次回覆"), value(&trf("{0} 輸出", &[&usage::tokens_text(output)])), None));
    }
    if let (Some(added), Some(removed)) = (usage.lines_added, usage.lines_removed) {
        let lines = gtk::Box::builder().spacing(6).halign(gtk::Align::End).build();
        lines.append(&label(&format!("+{}", grouped(added)), &["numeric", "term-added"]));
        lines.append(&label(&format!("−{}", grouped(removed)), &["numeric", "term-removed"]));
        rows.push((name(tr("行數")), lines.upcast(), None));
    }
    rows
}

/// The card the bar's meter opens.
pub fn card(usage: &Usage) -> gtk::Box {
    let card = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(14)
        .width_request(CARD_WIDTH)
        .css_classes(["term-context-card"])
        .build();
    let window = usage.context_window;
    card.append(&head(usage, window));
    if let Some(window) = window.filter(|&window| window > 0) {
        let parts = parts(usage, window);
        card.append(&parts_bar(&parts, window));
        let rows: Vec<_> = parts
            .iter()
            .map(|(name, class, tokens)| {
                (named_swatch(name, class), value(&usage::tokens_text(*tokens)), Some(percent(*tokens, window)))
            })
            .collect();
        card.append(&table(&rows));
    }
    let rows = session_rows(usage);
    if !rows.is_empty() {
        card.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        card.append(&label(tr("這個工作階段"), &["caption", "dim-label"]));
        card.append(&table(&rows));
    }
    card
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_group_their_thousands() {
        assert_eq!(grouped(316), "316");
        assert_eq!(grouped(1_204), "1,204");
        assert_eq!(grouped(1_234_567), "1,234,567");
    }

    #[test]
    fn the_parts_fill_the_window() {
        let usage = Usage {
            input_tokens: Some(2_300),
            cache_creation_tokens: Some(36_900),
            cache_read_tokens: Some(541_000),
            ..Usage::default()
        };
        let parts = parts(&usage, 1_000_000);
        assert_eq!(parts.iter().map(|(_, _, tokens)| tokens).sum::<u64>(), 1_000_000);
        assert_eq!(percent(parts[0].2, 1_000_000), "54.1%");
    }
}
