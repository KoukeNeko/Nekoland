//! The Triggers page of the settings, as Nekominal's: a plate a rule with its name, its pattern, the tag of its action
//! and its switch, which opens its form in place; the ready-made rules under them; and the button that adds a rule at
//! the right of the title. A new rule is a form of its own until its pattern is given, and emptying the pattern of a
//! rule deletes it.

use std::{cell::Cell, rc::Rc};

use adw::prelude::*;
use gtk::glib;
use regex::Regex;

use super::{
    lang::tr,
    config::{self, Choice, Trigger, TriggerAction, DEFAULT_HIGHLIGHT, HIGHLIGHTS, MAX_TRIGGERS},
    settings::{self, change},
    triggers, window,
};

pub const INTRO: &str = "輸出出現符合的文字時做一件事";
const ADD: &str = "新增規則";
const FORM_LABEL: i32 = 120;
const MARGIN: i32 = 12;
const GAP: i32 = 8;
/// How many lines of the pane the form matches its pattern against.
const SAMPLE_LINES: usize = 3;
const NEEDS_VALUE: &str = "不能空白";
const BAD_PATTERN: &str = "不是有效的正規表示式";
const ENTER: &str = "\u{23CE}";
const NOTIFY_DEFAULT: &str = "$0";
const HIT_FOREGROUND: &str = "#ffffff";

type Rerender = Rc<dyn Fn()>;
type Open = Rc<Cell<Option<usize>>>;

/// The colours of the highlight swatches, for the page's style.
pub fn css() -> String {
    HIGHLIGHTS
        .iter()
        .enumerate()
        .map(|(index, colour)| format!(".term-highlight-{index} {{ background: {colour}; }}\n"))
        .collect()
}

/// The button at the right of the title that opens a new rule.
pub fn add_button(open: &Open, rerender: &Rerender) -> gtk::Button {
    let button = gtk::Button::builder()
        .child(&adw::ButtonContent::builder().icon_name("list-add-symbolic").label(tr(ADD)).build())
        .css_classes(["suggested-action"])
        .sensitive(config::current().triggers.len() < MAX_TRIGGERS)
        .build();
    button.connect_clicked(glib::clone!(
        #[strong]
        open,
        #[strong]
        rerender,
        move |_| {
            open.set(Some(config::current().triggers.len()));
            rerender();
        }
    ));
    button
}

/// The rule at `index`, or the one being added past the last: a highlight, as the file's default.
fn rule_at(rules: &[Trigger], index: usize) -> Trigger {
    rules.get(index).cloned().unwrap_or_default()
}

/// A send's text as its field shows it: its Enter written `\r`, which a line cannot hold.
pub fn shown_text(text: &str) -> String {
    text.replace('\r', "\\r")
}

pub fn typed_text(shown: &str) -> String {
    shown.replace("\\r", "\r")
}

/// What the tag of a rule says it does: a send also says what it types, its Enter as ⏎.
fn action_text(rule: &Trigger) -> String {
    let name = rule.action.label();
    match rule.action {
        TriggerAction::Send => format!("{name} {}", rule.text.replace('\r', ENTER)),
        _ => name.to_string(),
    }
}

fn change_rule(index: usize, edit: impl FnOnce(&mut Trigger)) {
    change(move |config| {
        if let Some(rule) = config.triggers.get_mut(index) {
            edit(rule);
        }
    });
}

pub fn widgets(open: &Open, rerender: &Rerender) -> Vec<gtk::Widget> {
    let rules = config::current().triggers;
    let opened = open.get().filter(|index| *index <= rules.len());
    let count = rules.len() + usize::from(opened == Some(rules.len()));
    let mut widgets: Vec<gtk::Widget> = Vec::new();
    if count > 0 {
        let plates = settings::plates();
        for index in 0..count {
            let saved = index < rules.len();
            plates.append(&rule_row(index, &rule_at(&rules, index), saved, opened == Some(index), open, rerender));
        }
        widgets.push(plates.upcast());
    }
    widgets.push(presets(rules.len(), open, rerender).upcast());
    widgets
}

fn rule_row(index: usize, rule: &Trigger, saved: bool, opened: bool, open: &Open, rerender: &Rerender) -> gtk::ListBoxRow {
    let content = gtk::Box::builder().orientation(gtk::Orientation::Vertical).build();
    content.append(&head(index, rule, saved, opened, open, rerender));
    if opened {
        content.append(&form(index, rule, saved, open, rerender));
    }
    gtk::ListBoxRow::builder().child(&content).activatable(false).build()
}

/// The line of the name, which opens and closes the form; a press on its switch only switches the rule.
fn head(index: usize, rule: &Trigger, saved: bool, opened: bool, open: &Open, rerender: &Rerender) -> gtk::Box {
    let line = gtk::Box::builder()
        .spacing(10)
        .margin_start(MARGIN)
        .margin_end(MARGIN)
        .margin_top(GAP)
        .margin_bottom(GAP)
        .height_request(34)
        .build();
    let name = gtk::Label::builder()
        .label(if rule.name.is_empty() { tr(ADD) } else { &rule.name })
        .css_classes(["heading"])
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    if rule.name.is_empty() {
        name.add_css_class("dim-label");
    }
    line.append(&name);
    if !rule.pattern.is_empty() {
        line.append(
            &gtk::Label::builder()
                .label(&rule.pattern)
                .css_classes(["term-code", "dim-label"])
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .valign(gtk::Align::Center)
                .build(),
        );
    }
    line.append(&gtk::Box::builder().hexpand(true).build());
    let tag = gtk::Label::builder()
        .label(action_text(rule))
        .css_classes(["term-tag", "caption"])
        .valign(gtk::Align::Center)
        .build();
    match rule.action {
        TriggerAction::Highlight => tag.add_css_class("mark"),
        TriggerAction::Send => tag.add_css_class("warn"),
        _ => {}
    }
    line.append(&tag);
    let switch = gtk::Switch::builder()
        .active(rule.enabled)
        .sensitive(saved)
        .valign(gtk::Align::Center)
        .build();
    switch.connect_active_notify(move |switch| {
        let on = switch.is_active();
        change_rule(index, |rule| rule.enabled = on);
    });
    line.append(&switch);
    let click = gtk::GestureClick::new();
    click.connect_released(glib::clone!(
        #[strong]
        open,
        #[strong]
        rerender,
        #[weak]
        line,
        #[weak]
        switch,
        move |_, _, x, y| {
            let on_switch = line
                .pick(x, y, gtk::PickFlags::DEFAULT)
                .is_some_and(|picked| picked == switch || picked.is_ancestor(&switch));
            if on_switch {
                return;
            }
            open.set((!opened).then_some(index));
            rerender();
        }
    ));
    line.add_controller(click);
    line
}

/// A line of the form: its name at the left, its field at the right of it.
fn attach(grid: &gtk::Grid, line: i32, label: &str, field: &impl IsA<gtk::Widget>) {
    let name = gtk::Label::builder()
        .label(label)
        .xalign(0.0)
        .width_request(FORM_LABEL)
        .css_classes(["dim-label"])
        .build();
    grid.attach(&name, 0, line, 1, 1);
    grid.attach(field, 1, line, 1, 1);
}

fn form(index: usize, rule: &Trigger, saved: bool, open: &Open, rerender: &Rerender) -> gtk::Grid {
    let grid = gtk::Grid::builder()
        .row_spacing(GAP)
        .column_spacing(14)
        .margin_start(MARGIN)
        .margin_end(MARGIN)
        .margin_bottom(MARGIN)
        .build();
    let mut line = 0;
    let name = name_field(index, rule);
    name.set_sensitive(saved);
    attach(&grid, line, tr("名稱"), &name);
    line += 1;
    let sample = sample_label(rule);
    attach(&grid, line, tr("符合的文字"), &pattern_line(index, rule, saved, &sample, open, rerender));
    line += 1;
    if let Some(sample) = sample {
        grid.attach(&sample, 1, line, 1, 1);
        line += 1;
    }
    let program = program_line(index, rule);
    program.set_sensitive(saved);
    attach(&grid, line, tr("只在執行"), &program);
    line += 1;
    let actions = action_buttons(index, rule, rerender);
    actions.set_sensitive(saved);
    attach(&grid, line, tr("動作"), &actions);
    line += 1;
    if let Some((label, widget)) = action_field(index, rule, rerender) {
        widget.set_sensitive(saved);
        attach(&grid, line, label, &widget);
    }
    grid
}

fn name_field(index: usize, rule: &Trigger) -> gtk::Entry {
    let field = settings::field(&rule.name, move |_, text| {
        let name = text.trim().to_string();
        // A rule without a name is called by its pattern, as the file does.
        change_rule(index, |rule| rule.name = if name.is_empty() { rule.pattern.clone() } else { name });
    });
    field.set_hexpand(true);
    field
}

/// The pattern, and whether a line counts before its line break; the sample shows what the pattern being typed finds.
fn pattern_line(
    index: usize,
    rule: &Trigger,
    saved: bool,
    sample: &Option<gtk::Label>,
    open: &Open,
    rerender: &Rerender,
) -> gtk::Box {
    let colour = highlight_colour(rule);
    let field = settings::field(&rule.pattern, glib::clone!(
        #[strong]
        open,
        #[strong]
        rerender,
        move |field, pattern| commit_pattern(index, saved, field, pattern, &open, &rerender)
    ));
    field.set_hexpand(true);
    if let Some(sample) = sample {
        let lines = window::sample_lines(SAMPLE_LINES);
        field.connect_changed(glib::clone!(
            #[weak]
            sample,
            move |field| {
                let pattern = field.text();
                let valid = pattern.is_empty() || triggers::is_valid(&pattern);
                settings::mark_refused(field, (!valid).then_some(BAD_PATTERN));
                sample.set_markup(&sample_markup(&lines, &pattern, &colour));
            }
        ));
    }
    let instant = gtk::ToggleButton::builder()
        .label(tr("還沒換行也算"))
        .active(rule.instant)
        .sensitive(saved)
        .valign(gtk::Align::Center)
        .build();
    instant.connect_toggled(move |button| {
        let on = button.is_active();
        change_rule(index, |rule| rule.instant = on);
    });
    let line = gtk::Box::builder().spacing(GAP).build();
    line.append(&field);
    line.append(&instant);
    if !saved {
        // The field can take the focus only once it is in the window.
        field.connect_map(|field| {
            field.grab_focus();
        });
    }
    line
}

/// A pattern given: kept when it is a regular expression, a new rule made of it; empty, the rule goes.
fn commit_pattern(index: usize, saved: bool, field: &gtk::Entry, pattern: String, open: &Open, rerender: &Rerender) {
    if pattern.is_empty() {
        if saved {
            change(|config| {
                if index < config.triggers.len() {
                    config.triggers.remove(index);
                }
            });
        }
        open.set(None);
        rerender();
        return;
    }
    if !triggers::is_valid(&pattern) {
        settings::mark_refused(field, Some(BAD_PATTERN));
        return;
    }
    settings::mark_refused(field, None);
    change(move |config| {
        let room = config.triggers.len() < MAX_TRIGGERS;
        match config.triggers.get_mut(index) {
            Some(rule) => {
                if rule.name.is_empty() || rule.name == rule.pattern {
                    rule.name = pattern.clone();
                }
                rule.pattern = pattern;
            }
            None if room => config.triggers.push(Trigger {
                name: pattern.clone(),
                pattern,
                ..Trigger::default()
            }),
            None => {}
        }
    });
    rerender();
}

fn highlight_colour(rule: &Trigger) -> String {
    if rule.action == TriggerAction::Highlight {
        rule.color.clone()
    } else {
        DEFAULT_HIGHLIGHT.to_string()
    }
}

/// The last lines of the pane, what `pattern` matches in them marked in `colour`.
fn sample_markup(lines: &[String], pattern: &str, colour: &str) -> String {
    let regex = (!pattern.is_empty()).then(|| Regex::new(pattern).ok()).flatten();
    lines
        .iter()
        .map(|line| {
            let Some(regex) = &regex else {
                return glib::markup_escape_text(line).to_string();
            };
            let mut marked = String::new();
            let mut last = 0;
            for found in regex.find_iter(line).filter(|found| !found.is_empty()) {
                marked.push_str(&glib::markup_escape_text(&line[last..found.start()]));
                marked.push_str(&format!(
                    "<span background=\"{colour}\" foreground=\"{HIT_FOREGROUND}\">{}</span>",
                    glib::markup_escape_text(found.as_str())
                ));
                last = found.end();
            }
            marked.push_str(&glib::markup_escape_text(&line[last..]));
            marked
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The sample under the pattern, when the pane has lines to show.
fn sample_label(rule: &Trigger) -> Option<gtk::Label> {
    let lines = window::sample_lines(SAMPLE_LINES);
    if lines.is_empty() {
        return None;
    }
    let label = gtk::Label::builder()
        .xalign(0.0)
        .css_classes(["term-sample"])
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    label.set_markup(&sample_markup(&lines, &rule.pattern, &highlight_colour(rule)));
    Some(label)
}

fn program_line(index: usize, rule: &Trigger) -> gtk::Box {
    let field = settings::field(&rule.program, move |_, text| {
        let program = text.trim().to_string();
        change_rule(index, |rule| rule.program = program);
    });
    field.set_hexpand(true);
    let line = gtk::Box::builder().spacing(GAP).build();
    line.append(&field);
    line.append(&gtk::Label::builder().label(tr("時；空白就是任何程式")).css_classes(["dim-label", "caption"]).build());
    line
}

fn action_buttons(index: usize, rule: &Trigger, rerender: &Rerender) -> gtk::Box {
    let buttons = gtk::Box::builder()
        .css_classes(["linked"])
        .halign(gtk::Align::Start)
        .build();
    let mut first: Option<gtk::ToggleButton> = None;
    for action in TriggerAction::ALL.iter().copied() {
        let button = gtk::ToggleButton::builder()
            .label(action.label())
            .active(action == rule.action)
            .build();
        match &first {
            Some(first) => button.set_group(Some(first)),
            None => first = Some(button.clone()),
        }
        let rerender = rerender.clone();
        button.connect_toggled(move |button| {
            if !button.is_active() {
                return;
            }
            change_rule(index, |rule| set_action(rule, action));
            rerender();
        });
        buttons.append(&button);
    }
    buttons
}

/// The action changed: a send starts without text to type, a notification with the match.
fn set_action(rule: &mut Trigger, action: TriggerAction) {
    match action {
        TriggerAction::Send if rule.text == NOTIFY_DEFAULT => rule.text.clear(),
        TriggerAction::Notify if rule.text.is_empty() => rule.text = NOTIFY_DEFAULT.to_string(),
        _ => {}
    }
    rule.action = action;
}

/// What the action needs: the text of a notification or of a send, the colour of a highlight.
fn action_field(index: usize, rule: &Trigger, rerender: &Rerender) -> Option<(&'static str, gtk::Widget)> {
    match rule.action {
        TriggerAction::Notify => {
            let field = settings::field(&rule.text, move |_, text| {
                let text = if text.is_empty() { NOTIFY_DEFAULT.to_string() } else { text };
                change_rule(index, |rule| rule.text = text);
            });
            field.set_placeholder_text(Some(NOTIFY_DEFAULT));
            field.set_hexpand(true);
            Some((tr("通知內容"), field.upcast()))
        }
        TriggerAction::Send => {
            let field = settings::field(&shown_text(&rule.text), glib::clone!(
                #[strong]
                rerender,
                move |field, text| {
                    let typed = typed_text(&text);
                    if typed.is_empty() {
                        settings::mark_refused(field, Some(NEEDS_VALUE));
                        return;
                    }
                    settings::mark_refused(field, None);
                    change_rule(index, |rule| rule.text = typed);
                    rerender();
                }
            ));
            field.set_placeholder_text(Some(tr("要送出的文字，\\r 是 Enter")));
            field.set_hexpand(true);
            Some((tr("送出的文字"), field.upcast()))
        }
        TriggerAction::Highlight => Some((tr("標示顏色"), swatches(index, rule, rerender).upcast())),
        TriggerAction::Badge | TriggerAction::Bell => None,
    }
}

fn swatches(index: usize, rule: &Trigger, rerender: &Rerender) -> gtk::Box {
    let line = gtk::Box::builder().spacing(GAP).build();
    let mut first: Option<gtk::ToggleButton> = None;
    for (number, colour) in HIGHLIGHTS.iter().enumerate() {
        let swatch = gtk::Box::builder()
            .css_classes(["term-swatch", &format!("term-highlight-{number}")])
            .build();
        let button = gtk::ToggleButton::builder()
            .child(&swatch)
            .active(rule.color.eq_ignore_ascii_case(colour))
            .css_classes(["flat", "term-swatch-button"])
            .tooltip_text(*colour)
            .build();
        match &first {
            Some(first) => button.set_group(Some(first)),
            None => first = Some(button.clone()),
        }
        let rerender = rerender.clone();
        button.connect_toggled(move |button| {
            if button.is_active() {
                change_rule(index, |rule| rule.color = colour.to_string());
                rerender();
            }
        });
        line.append(&button);
    }
    line
}

/// A chip a ready-made rule, as many in a line as fit; one picked is added and opened.
fn presets(count: usize, open: &Open, rerender: &Rerender) -> gtk::FlowBox {
    let chips = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .column_spacing(GAP as u32)
        .row_spacing(GAP as u32)
        .margin_top(MARGIN)
        .max_children_per_line(triggers::presets().len() as u32)
        .build();
    for preset in triggers::presets() {
        let chip = gtk::Button::builder()
            .child(&adw::ButtonContent::builder().icon_name("list-add-symbolic").label(&preset.name).build())
            .sensitive(count < MAX_TRIGGERS)
            .build();
        chip.connect_clicked(glib::clone!(
            #[strong]
            open,
            #[strong]
            rerender,
            move |_| {
                let preset = preset.clone();
                let mut added = None;
                change(|config| {
                    if config.triggers.len() < MAX_TRIGGERS {
                        config.triggers.push(preset);
                        added = Some(config.triggers.len() - 1);
                    }
                });
                open.set(added);
                rerender();
            }
        ));
        chips.append(&chip);
    }
    chips
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_send_is_tagged_with_what_it_types() {
        let rule = Trigger { action: TriggerAction::Send, text: "y\r".to_string(), ..Trigger::default() };
        assert_eq!(action_text(&rule), "送出文字 y\u{23CE}");
        assert_eq!(typed_text(&shown_text("y\r")), "y\r");
    }

    #[test]
    fn the_sample_marks_what_the_pattern_finds() {
        let lines = vec!["a <b> error: x".to_string()];
        let marked = sample_markup(&lines, "error:", "#7a5c00");
        assert_eq!(marked, "a &lt;b&gt; <span background=\"#7a5c00\" foreground=\"#ffffff\">error:</span> x");
        assert_eq!(sample_markup(&lines, "(", "#7a5c00"), "a &lt;b&gt; error: x");
    }

    #[test]
    fn a_send_starts_without_text_and_a_notification_with_the_match() {
        let mut rule = Trigger::default();
        set_action(&mut rule, TriggerAction::Send);
        assert!(rule.text.is_empty());
        set_action(&mut rule, TriggerAction::Notify);
        assert_eq!(rule.text, NOTIFY_DEFAULT);
    }
}
