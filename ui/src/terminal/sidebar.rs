//! The sidebar's rows, in each of Nekominal's ways of listing the tabs: the activity list (a tab's name and state, and
//! what it last did), agent cards (the same in a card, with a peek at what it said), by repository (a line a tab under
//! the repository it is in), and icons (a column of cells ringed in their state's colour).

use std::rc::Rc;

use gtk::{pango, prelude::*};

use super::{
    lang::{tr, trf},
    subagents_card,
    agent::{Status, GROUP_HEADINGS},
    badges,
    config::{TabLayout, TabStyle},
    context_card,
    page::Page,
    titlebar,
    usage::{self, Usage},
};

/// Nekominal's widths of the sidebar: the list in each style, and the column of icons.
const LIST_WIDTH: f64 = 220.0;
const CARDS_WIDTH: f64 = 280.0;
const REPO_WIDTH: f64 = 240.0;
const ICONS_WIDTH: f64 = 52.0;
const TERMINAL_ICON: &str = "utilities-terminal-symbolic";
const SETTINGS_ICON: &str = "emblem-system-symbolic";
const SETTINGS_TITLE: &str = "設定";
const CLOSE_ICON: &str = "window-close-symbolic";
/// The heading of the tabs that are in no repository, listed after the repositories.
const LOOSE_TABS: &str = "其他";
/// What a card shows of what its agent said.
const PEEK_LINES: i32 = 2;
/// How wide the meter of the context is on a line under a repository.
const REPO_METER_WIDTH: i32 = 28;

/// A tab as the sidebar lists it: a terminal, or the settings, which has no pane.
pub struct Entry {
    pub tab: adw::TabPage,
    pub page: Option<Rc<Page>>,
}

impl Entry {
    fn status(&self) -> Status {
        self.page.as_ref().map_or(Status::Idle, |page| page.status())
    }

    pub fn title(&self) -> String {
        self.page
            .as_ref()
            .map_or_else(|| tr(SETTINGS_TITLE).to_string(), |page| page.title())
    }

    fn place(&self) -> String {
        self.page.as_ref().map(|page| page.place_beside(&self.title())).unwrap_or_default()
    }

    fn message(&self) -> Option<String> {
        self.page.as_ref().and_then(|page| page.message())
    }

    fn icon(&self) -> &'static str {
        if self.page.is_some() {
            TERMINAL_ICON
        } else {
            SETTINGS_ICON
        }
    }

    fn repository(&self) -> Option<String> {
        Some(self.page.as_ref()?.repository()?.name)
    }

    /// Under its repository it is known by its branch.
    fn branch_or_title(&self) -> String {
        self.page
            .as_ref()
            .and_then(|page| page.repository())
            .and_then(|repository| repository.branch)
            .unwrap_or_else(|| self.title())
    }

    fn colour(&self) -> Option<&'static str> {
        self.page.as_ref()?.colour.get()
    }

    fn usage(&self) -> Option<Usage> {
        self.page.as_ref()?.usage()
    }

    /// Its pull request and ports, on a line of their own.
    fn badges(&self) -> Option<gtk::FlowBox> {
        badges::row_line(&self.page.as_ref()?.badges())
    }
}

/// A heading of the sidebar: its name, how many tabs are under it, and what their agents cost together.
pub struct Heading {
    pub title: String,
    pub count: Option<String>,
    pub cost: Option<String>,
}

/// A heading of the sidebar over the tabs (indices into the entries) under it; none when the list is not grouped.
pub struct Group {
    pub heading: Option<Heading>,
    pub members: Vec<usize>,
}

/// The sidebar's width in `layout` and `style`.
pub fn width(layout: TabLayout, style: TabStyle) -> f64 {
    match (layout, style) {
        (TabLayout::Icons, _) => ICONS_WIDTH,
        (_, TabStyle::Activity) => LIST_WIDTH,
        (_, TabStyle::Cards) => CARDS_WIDTH,
        (_, TabStyle::Repo) => REPO_WIDTH,
    }
}

/// The tabs in the order the sidebar lists them: under their repositories in that style, by state when grouped, else in
/// their own order under no heading.
pub fn groups(layout: TabLayout, style: TabStyle, by_state: bool, entries: &[Entry]) -> Vec<Group> {
    let list = layout == TabLayout::List;
    if list && style == TabStyle::Repo {
        return repository_groups(entries);
    }
    if !list || !by_state {
        return vec![Group { heading: None, members: (0..entries.len()).collect() }];
    }
    GROUP_HEADINGS
        .iter()
        .enumerate()
        .map(|(group, heading)| Group {
            heading: Some(Heading { title: tr(heading).to_string(), count: None, cost: None }),
            members: (0..entries.len())
                .filter(|&i| entries[i].status().group() == group)
                .collect(),
        })
        .filter(|group| !group.members.is_empty())
        .collect()
}

/// The repositories in the order their first tab comes, the tabs in none last.
fn repository_groups(entries: &[Entry]) -> Vec<Group> {
    let mut names: Vec<Option<String>> = Vec::new();
    for entry in entries {
        let name = entry.repository();
        if !names.contains(&name) {
            names.push(name);
        }
    }
    // The loose tabs after every repository.
    names.sort_by_key(Option::is_none);
    names
        .into_iter()
        .map(|name| {
            let members: Vec<usize> = (0..entries.len())
                .filter(|&i| entries[i].repository() == name)
                .collect();
            let count = trf("{0} 個分頁", &[&members.len()]);
            let title = name.unwrap_or_else(|| tr(LOOSE_TABS).to_string());
            let cost: f64 = members.iter().filter_map(|&i| entries[i].usage()?.cost).sum();
            let cost = (cost > 0.0).then(|| usage::cost_text(cost));
            Group { heading: Some(Heading { title, count: Some(count), cost }), members }
        })
        .collect()
}

pub fn heading_row(heading: &Heading) -> gtk::ListBoxRow {
    let line = gtk::Box::builder().spacing(6).css_classes(["term-heading"]).build();
    let classes = if heading.count.is_some() { ["caption-heading", "term-repo"] } else { ["caption-heading", "dim-label"] };
    line.append(
        &gtk::Label::builder()
            .label(&heading.title)
            .xalign(0.0)
            .ellipsize(pango::EllipsizeMode::End)
            .css_classes(classes)
            .build(),
    );
    if let Some(count) = &heading.count {
        line.append(&gtk::Label::builder().label(count).css_classes(["caption", "dim-label"]).build());
    }
    if let Some(cost) = &heading.cost {
        line.append(
            &gtk::Label::builder()
                .label(cost)
                .hexpand(true)
                .xalign(1.0)
                .css_classes(["caption", "dim-label", "numeric"])
                .build(),
        );
    }
    gtk::ListBoxRow::builder()
        .child(&line)
        .selectable(false)
        .activatable(false)
        .build()
}

/// The row of `entry` in `layout` and `style`; `close` closes its tab.
pub fn row(layout: TabLayout, style: TabStyle, entry: &Entry, close: impl Fn() + Clone + 'static) -> gtk::ListBoxRow {
    let child = match (layout, style) {
        (TabLayout::Icons, _) => icon_cell(entry),
        (_, TabStyle::Activity) => activity(entry, close),
        (_, TabStyle::Cards) => card(entry, close),
        (_, TabStyle::Repo) => repository_line(entry),
    };
    let row = gtk::ListBoxRow::builder().child(&child).build();
    let tip = match entry.message() {
        Some(message) => format!("{}\n{message}", entry.title()),
        None => entry.title(),
    };
    row.set_tooltip_text(Some(&tip));
    row
}

fn icon(entry: &Entry) -> gtk::Image {
    gtk::Image::builder()
        .icon_name(entry.icon())
        .valign(gtk::Align::Start)
        .build()
}

/// Its name: in its state's colour while it needs the user, else quiet unless it is selected; after it, how many
/// subagents its agent runs.
fn name(entry: &Entry, text: &str) -> gtk::Box {
    let label = gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .ellipsize(pango::EllipsizeMode::End)
        .css_classes(["term-name"])
        .build();
    let status = entry.status();
    if status.is_urgent() {
        label.add_css_class(status.css_class());
        label.add_css_class("urgent");
    }
    let line = gtk::Box::builder().spacing(6).hexpand(true).build();
    line.append(&label);
    let running = entry.page.as_ref().and_then(|page| page.subagents()).map_or(0, |subagents| subagents.running());
    if running > 0 {
        line.append(&subagents_card::mark(running));
    }
    line
}

/// The state at the end of a row in its colour, a spinner before it while it works, a pulse while it waits; at the
/// same place, the close button while the pointer is on the row.
fn state_or_close(entry: &Entry, close: impl Fn() + 'static) -> gtk::Overlay {
    let status = entry.status();
    let state = gtk::Box::builder()
        .spacing(5)
        .halign(gtk::Align::End)
        .valign(gtk::Align::Center)
        .css_classes(["caption", status.css_class(), "term-state"])
        .build();
    match status {
        Status::Working => state.append(&gtk::Spinner::builder().spinning(true).build()),
        Status::Waiting => state.append(&titlebar::dot()),
        _ => {}
    }
    if let Some(word) = status.word() {
        state.append(&gtk::Label::new(Some(word)));
    }
    let button = gtk::Button::builder()
        .icon_name(CLOSE_ICON)
        .tooltip_text(tr("關閉分頁"))
        .halign(gtk::Align::End)
        .valign(gtk::Align::Center)
        .css_classes(["flat", "circular", "term-close"])
        .build();
    button.connect_clicked(move |_| close());
    let overlay = gtk::Overlay::builder().child(&state).build();
    overlay.add_overlay(&button);
    overlay
}

/// A colour the user gave the tab, as a bar down its left edge.
fn colour_bar(entry: &Entry) -> Option<gtk::Box> {
    let colour = entry.colour()?;
    let class = format!("term-colour-{colour}");
    Some(gtk::Box::builder().css_classes(["term-colour-bar", class.as_str()]).build())
}

/// The second line: what it last did, in its state's colour while it needs the user, else where it is.
fn activity_line(entry: &Entry) -> gtk::Label {
    let status = entry.status();
    let message = entry.message();
    let line = gtk::Label::builder()
        .label(message.clone().unwrap_or_else(|| entry.place()))
        .xalign(0.0)
        .ellipsize(pango::EllipsizeMode::End)
        .css_classes(["caption"])
        .build();
    line.set_visible(!line.text().is_empty());
    if message.is_some() && status.is_urgent() {
        line.add_css_class(status.css_class());
    } else {
        line.add_css_class("dim-label");
    }
    line
}

fn panes_badge(entry: &Entry) -> Option<gtk::Label> {
    let count = entry.page.as_ref()?.pane_count();
    (count > 1).then(|| {
        gtk::Label::builder()
            .label(trf("{0} 窗格", &[&count]))
            .css_classes(["caption", "dim-label", "term-badge"])
            .build()
    })
}

fn activity(entry: &Entry, close: impl Fn() + Clone + 'static) -> gtk::Widget {
    let first = gtk::Box::builder().spacing(8).build();
    first.append(&name(entry, &entry.title()));
    if let Some(badge) = panes_badge(entry) {
        first.append(&badge);
    }
    first.append(&state_or_close(entry, close));
    let text = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .hexpand(true)
        .build();
    text.append(&first);
    text.append(&activity_line(entry));
    if let Some(usage) = entry.usage() {
        if let Some(share) = usage.context {
            text.append(&context_card::context_line(share, usage.cost));
        }
    }
    if let Some(badges) = entry.badges() {
        text.append(&badges);
    }
    // Centred as a whole, the icon level with the first line however many lines follow it.
    let content = gtk::Box::builder().spacing(10).valign(gtk::Align::Center).css_classes(["term-activity"]).build();
    if let Some(bar) = colour_bar(entry) {
        content.append(&bar);
    }
    content.append(&icon(entry));
    content.append(&text);
    content.upcast()
}

fn card(entry: &Entry, close: impl Fn() + Clone + 'static) -> gtk::Widget {
    let status = entry.status();
    let first = gtk::Box::builder().spacing(8).build();
    first.append(&icon(entry));
    first.append(&name(entry, &entry.title()));
    // The state at the right edge; the close button takes its place while the pointer is on the card.
    let pill = gtk::Label::builder()
        .label(status.word().unwrap_or_default())
        .visible(status.word().is_some())
        .halign(gtk::Align::End)
        .valign(gtk::Align::Center)
        .css_classes(["caption", status.css_class(), "term-pill", "term-state"])
        .build();
    let close_button = gtk::Button::builder()
        .icon_name(CLOSE_ICON)
        .tooltip_text(tr("關閉分頁"))
        .halign(gtk::Align::End)
        .valign(gtk::Align::Center)
        .css_classes(["flat", "circular", "term-close"])
        .build();
    close_button.connect_clicked(move |_| close());
    let pill_or_close = gtk::Overlay::builder().child(&pill).build();
    pill_or_close.add_overlay(&close_button);
    first.append(&pill_or_close);
    let card = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .css_classes(["term-card"])
        .build();
    card.append(&first);
    let place = entry.place();
    if !place.is_empty() {
        card.append(
            &gtk::Label::builder()
                .label(place)
                .xalign(0.0)
                .ellipsize(pango::EllipsizeMode::End)
                .css_classes(["caption", "dim-label"])
                .build(),
        );
    }
    if let Some(message) = entry.message() {
        card.append(
            &gtk::Label::builder()
                .label(message)
                .xalign(0.0)
                .wrap(true)
                .wrap_mode(pango::WrapMode::WordChar)
                .lines(PEEK_LINES)
                .ellipsize(pango::EllipsizeMode::End)
                .css_classes(["caption", "monospace", "term-peek"])
                .build(),
        );
    }
    if let Some(usage) = entry.usage() {
        card.append(&card_footer(&usage));
    }
    if let Some(badges) = entry.badges() {
        card.append(&badges);
    }
    card.upcast()
}

/// The foot of a card: the model and the session's time, then the line of the context, or the cost alone.
fn card_footer(usage: &Usage) -> gtk::Box {
    let words: Vec<String> =
        [usage.model.clone(), usage.duration_ms.map(usage::clock)].into_iter().flatten().collect();
    let foot = gtk::Box::builder().spacing(10).build();
    foot.append(
        &gtk::Label::builder()
            .label(words.join(" · "))
            .xalign(0.0)
            .ellipsize(pango::EllipsizeMode::End)
            .css_classes(["caption", "dim-label"])
            .build(),
    );
    match (usage.context, usage.cost) {
        (Some(share), cost) => {
            let line = context_card::context_line(share, cost);
            line.set_hexpand(true);
            foot.append(&line);
        }
        (None, Some(cost)) => foot.append(
            &gtk::Label::builder()
                .label(usage::cost_text(cost))
                .hexpand(true)
                .xalign(1.0)
                .css_classes(["caption", "dim-label", "numeric"])
                .build(),
        ),
        (None, None) => {}
    }
    foot
}

fn repository_line(entry: &Entry) -> gtk::Widget {
    let line = gtk::Box::builder().spacing(8).css_classes(["term-repo-row"]).build();
    let icon = icon(entry);
    icon.set_valign(gtk::Align::Center);
    line.append(&icon);
    line.append(&name(entry, &entry.branch_or_title()));
    if let Some(usage) = entry.usage() {
        if let Some(cost) = usage.cost {
            line.append(&gtk::Label::builder().label(usage::cost_text(cost)).css_classes(["caption", "dim-label", "numeric"]).build());
        }
        if let Some(share) = usage.context {
            line.append(&context_card::meter(share, REPO_METER_WIDTH));
        }
    }
    let status = entry.status();
    if matches!(status, Status::Working | Status::Waiting) {
        let mark = titlebar::dot();
        mark.add_css_class(status.css_class());
        line.append(&mark);
    }
    let Some(badges) = entry.badges() else {
        return line.upcast();
    };
    let rows = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(4).build();
    rows.append(&line);
    rows.append(&badges);
    rows.upcast()
}

/// A cell of the column: the tab's icon in a ring of its state's colour, a spinner in its corner while it works.
fn icon_cell(entry: &Entry) -> gtk::Widget {
    let status = entry.status();
    let cell = gtk::Box::builder()
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .css_classes(["term-cell"])
        .build();
    if status != Status::Idle {
        cell.add_css_class("ringed");
        cell.add_css_class(status.css_class());
    }
    let image = gtk::Image::builder()
        .icon_name(entry.icon())
        .hexpand(true)
        .halign(gtk::Align::Center)
        .css_classes(["dim-label"])
        .build();
    cell.append(&image);
    let overlay = gtk::Overlay::builder()
        .child(&cell)
        .halign(gtk::Align::Center)
        .build();
    if status == Status::Working {
        let spinner = gtk::Spinner::builder()
            .spinning(true)
            .halign(gtk::Align::End)
            .valign(gtk::Align::End)
            .css_classes(["term-cell-spinner", status.css_class()])
            .build();
        overlay.add_overlay(&spinner);
    }
    overlay.upcast()
}

/// Glues `entries`' rows and headings into `list`, selecting `selected`; returns each row with its tab.
pub fn fill(
    list: &gtk::ListBox,
    layout: TabLayout,
    style: TabStyle,
    by_state: bool,
    entries: &[Entry],
    selected: Option<&adw::TabPage>,
    close: impl Fn(&adw::TabPage) + Clone + 'static,
) -> Vec<(gtk::ListBoxRow, adw::TabPage)> {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
    let mut rows = Vec::new();
    for group in groups(layout, style, by_state, entries) {
        if let Some(heading) = &group.heading {
            list.append(&heading_row(heading));
        }
        for index in group.members {
            let entry = &entries[index];
            let tab = entry.tab.clone();
            let close = close.clone();
            let row = row(layout, style, entry, move || close(&tab));
            list.append(&row);
            if selected == Some(&entry.tab) {
                list.select_row(Some(&row));
            }
            rows.push((row, entry.tab.clone()));
        }
    }
    rows
}

/// The class of the list for `layout` and `style`, which the stylesheet sizes its rows by.
pub fn list_class(layout: TabLayout, style: TabStyle) -> &'static str {
    match (layout, style) {
        (TabLayout::Icons, _) => "term-icons",
        (_, TabStyle::Activity) => "term-list",
        (_, TabStyle::Cards) => "term-cards",
        (_, TabStyle::Repo) => "term-repos",
    }
}
