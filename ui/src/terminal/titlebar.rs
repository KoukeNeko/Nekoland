//! The bar across the top of the window, laid out as Nekominal's with GTK's own widgets. With the tabs on top it holds
//! the tabs, then a new tab and the menu of new things; with the tabs in a sidebar, the sidebar button, the new tab and
//! its menu, then the name of the tab in front. On the right, in both: how many other tabs need the user, the state of
//! the agent in front, its branch and folder, and the window's menu.

use std::{cell::RefCell, rc::Rc, time::Duration};

use gtk::{gio, glib, prelude::*};

use super::{
    lang::{tr, trf},
    agent::{Status, STATUS_ORDER},
    config::{self, Config, NoAgent, TabLayout},
    badges,
    context_card,
    subagents_card,
    page::Page,
    profiles,
    strip_layout::StripLayout,
    usage::{self, Usage},
};

const SIDEBAR_TOOLTIP: &str = "切換側邊欄";
const NEW_TAB_TOOLTIP: &str = "新增分頁";
const PROFILES_TOOLTIP: &str = "設定檔";
const WAITING_TOOLTIP: &str = "下一個需要回應的分頁 (Ctrl+Shift+U)";
const CONTEXT_TOOLTIP: &str = "脈絡";
const MENU_TOOLTIP: &str = "主選單";
const NUDGE: Duration = Duration::from_millis(300);
const NUDGE_CLASS: &str = "nudge";
const BRANCH_ICON: &str = "media-playlist-consecutive-symbolic";
const FOLDER_ICON: &str = "folder-symbolic";
/// How wide the bar's meter of the context is.
const METER_WIDTH: i32 = 40;

/// A glyph and its words, as each piece on the right of the bar is.
struct Piece {
    widget: gtk::Box,
    label: gtk::Label,
}

impl Piece {
    /// Words alone, as the model, the cost and the time are.
    fn plain() -> Self {
        let label = gtk::Label::builder().css_classes(["numeric"]).build();
        let widget = gtk::Box::builder()
            .spacing(6)
            .css_classes(["dim-label", "term-piece"])
            .valign(gtk::Align::Center)
            .build();
        widget.append(&label);
        Self { widget, label }
    }

    fn new(icon: &str) -> Self {
        let piece = Self::plain();
        piece.widget.prepend(&gtk::Image::from_icon_name(icon));
        piece
    }

    fn show(&self, text: Option<&str>) {
        self.widget.set_visible(text.is_some());
        self.label.set_text(text.unwrap_or_default());
    }
}

/// A small round mark in a state's colour.
pub fn dot() -> gtk::Box {
    gtk::Box::builder()
        .css_classes(["term-dot"])
        .valign(gtk::Align::Center)
        .build()
}

/// Gives a dot the colour of `status`, taking off the one it had.
pub fn colour_dot(dot: &gtk::Box, status: Status) {
    for other in STATUS_ORDER {
        dot.remove_css_class(other.css_class());
    }
    dot.add_css_class(status.css_class());
}

/// The share of the context in use as a meter and its number; pressed, the card of what fills it.
struct Context {
    button: gtk::MenuButton,
    content: gtk::Box,
    usage: Rc<RefCell<Option<Usage>>>,
}

impl Context {
    fn new() -> Self {
        let content = gtk::Box::builder().spacing(8).build();
        let usage: Rc<RefCell<Option<Usage>>> = Rc::default();
        let popover = gtk::Popover::new();
        popover.connect_show(glib::clone!(
            #[strong]
            usage,
            move |popover| {
                let card = usage.borrow().as_ref().map(context_card::card);
                popover.set_child(card.as_ref());
            }
        ));
        let button = gtk::MenuButton::builder()
            .child(&content)
            .popover(&popover)
            .always_show_arrow(false)
            .tooltip_text(tr(CONTEXT_TOOLTIP))
            .css_classes(["flat", "term-context-button"])
            .valign(gtk::Align::Center)
            .build();
        Self { button, content, usage }
    }

    fn show(&self, usage: Option<&Usage>) {
        let share = usage.and_then(|usage| usage.context);
        self.button.set_visible(share.is_some());
        self.usage.replace(usage.cloned());
        let Some(share) = share else {
            return;
        };
        while let Some(child) = self.content.first_child() {
            self.content.remove(&child);
        }
        self.content.append(&context_card::meter(share, METER_WIDTH));
        self.content.append(&gtk::Label::builder().label(format!("{share:.0}%")).css_classes(["numeric"]).build());
    }
}

/// The name of the tab in front, with the tabs in a sidebar: its state, its name, and where it is.
struct Current {
    widget: gtk::Box,
    dot: gtk::Box,
    title: gtk::Label,
    place: gtk::Label,
}

impl Current {
    fn new() -> Self {
        let dot = dot();
        let title = gtk::Label::builder()
            .css_classes(["heading"])
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        let place = gtk::Label::builder()
            .css_classes(["caption", "dim-label"])
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        let widget = gtk::Box::builder()
            .spacing(8)
            .hexpand(true)
            .margin_start(8)
            .valign(gtk::Align::Center)
            .build();
        widget.append(&dot);
        widget.append(&title);
        widget.append(&place);
        Self { widget, dot, title, place }
    }

    fn show(&self, page: Option<&Page>, title: &str) {
        let status = page.map_or(Status::Idle, |page| page.status());
        self.dot.set_visible(status != Status::Idle);
        colour_dot(&self.dot, status);
        self.title.set_text(title);
        self.place
            .set_text(&page.map(|page| page.place()).unwrap_or_default());
    }
}

pub struct TitleBar {
    pub widget: gtk::WindowHandle,
    left: gtk::Box,
    /// With the tabs on top: the tabs, then the new tab and its menu right after the last.
    strip: gtk::Box,
    tab_bar: adw::TabBar,
    sidebar_button: gtk::Button,
    new_tab: gtk::Button,
    profiles: gtk::MenuButton,
    current: Current,
    waiting: gtk::Button,
    waiting_label: gtk::Label,
    badges: gtk::Box,
    state: gtk::Box,
    state_dot: gtk::Box,
    state_label: gtk::Label,
    model: Piece,
    context: Context,
    subagents: subagents_card::Piece,
    cost: Piece,
    time: Piece,
    branch: Piece,
    folder: Piece,
    menu: gtk::MenuButton,
    layout: std::cell::Cell<Option<TabLayout>>,
}

impl TitleBar {
    pub fn new(tab_view: &adw::TabView) -> Self {
        let tab_bar = adw::TabBar::builder()
            .view(tab_view)
            .autohide(false)
            .expand_tabs(false)
            .css_classes(["inline"])
            .build();
        let strip = tab_strip(tab_view);
        let (waiting, waiting_label) = waiting_button();
        let badges = gtk::Box::builder().spacing(4).visible(false).css_classes(["term-piece"]).build();
        let state_label = gtk::Label::new(None);
        let state_dot = dot();
        let state = gtk::Box::builder()
            .spacing(6)
            .css_classes(["term-piece"])
            .valign(gtk::Align::Center)
            .build();
        state.append(&state_dot);
        state.append(&state_label);
        let model = Piece::plain();
        let context = Context::new();
        let subagents = subagents_card::Piece::new();
        let cost = Piece::plain();
        let time = Piece::plain();
        let branch = Piece::new(BRANCH_ICON);
        let folder = Piece::new(FOLDER_ICON);

        // A header bar would centre the tabs and squeeze them; a toolbar keeps GTK's own look and lets them take the
        // width.
        let left = gtk::Box::builder().hexpand(true).build();
        let bar = gtk::Box::builder().css_classes(["toolbar"]).build();
        bar.append(&left);
        bar.append(&waiting);
        bar.append(&badges);
        bar.append(&state);
        bar.append(&model.widget);
        bar.append(&context.button);
        bar.append(&subagents.button);
        bar.append(&cost.widget);
        bar.append(&time.widget);
        bar.append(&branch.widget);
        bar.append(&folder.widget);
        let menu = window_menu_button();
        bar.append(&menu);
        bar.append(&gtk::WindowControls::new(gtk::PackType::End));
        let widget = gtk::WindowHandle::builder().child(&bar).build();
        let this = Self {
            widget,
            left,
            strip,
            tab_bar,
            sidebar_button: tool_button("sidebar-show-symbolic", tr(SIDEBAR_TOOLTIP), "win.toggle-sidebar"),
            new_tab: tool_button("tab-new-symbolic", tr(NEW_TAB_TOOLTIP), "win.new-tab"),
            profiles: new_menu_button(),
            current: Current::new(),
            waiting,
            waiting_label,
            badges,
            state,
            state_dot,
            state_label,
            model,
            context,
            subagents,
            cost,
            time,
            branch,
            folder,
            menu,
            layout: std::cell::Cell::default(),
        };
        this.set_layout(config::current().tab_layout);
        this
    }

    /// Puts the left of the bar in the order of `layout`.
    pub fn set_layout(&self, layout: TabLayout) {
        if self.layout.replace(Some(layout)) == Some(layout) {
            return;
        }
        for container in [&self.left, &self.strip] {
            while let Some(child) = container.first_child() {
                container.remove(&child);
            }
        }
        let new_tab: &gtk::Widget = self.new_tab.upcast_ref();
        let profiles: &gtk::Widget = self.profiles.upcast_ref();
        if layout.is_side() {
            for widget in [
                self.sidebar_button.upcast_ref(),
                new_tab,
                profiles,
                self.current.widget.upcast_ref(),
            ] {
                self.left.append(widget);
            }
        } else {
            for widget in [self.tab_bar.upcast_ref(), new_tab, profiles] {
                self.strip.append(widget);
            }
            self.left.append(&self.strip);
        }
    }

    /// Shakes the count, when there is no tab to go to.
    pub fn nudge(&self) {
        self.waiting.add_css_class(NUDGE_CLASS);
        let waiting = self.waiting.clone();
        glib::timeout_add_local_once(NUDGE, move || waiting.remove_css_class(NUDGE_CLASS));
    }

    /// The tab in front: its name and place, and on the right its agent's state, its branch and its folder.
    pub fn show_current(&self, page: Option<&Page>, title: &str) {
        self.current.show(page, title);
        badges::fill_bar(&self.badges, &page.map(Page::badges).unwrap_or_default());
        let config = config::current();
        let status = page.map_or(Status::Idle, |page| page.status());
        let agent = page.is_some_and(|page| page.runs_agent());
        // Without an agent the bar keeps the folder and branch, or goes, as the settings say.
        let shown = config.status_bar && (agent || config.status_no_agent == NoAgent::Place);
        let word = status.word().filter(|_| shown && agent && config.status_state);
        self.state.set_visible(word.is_some());
        self.state_label.set_text(word.unwrap_or_default());
        colour_dot(&self.state_dot, status);
        self.state_label.set_css_classes(&[status.css_class()]);
        let usage = page.and_then(|page| page.usage()).filter(|_| shown);
        self.show_usage(usage.as_ref(), &config);
        self.subagents.show(page.and_then(|page| page.subagents()).filter(|_| shown));
        let branch = page
            .and_then(|page| page.repository())
            .and_then(|repository| repository.branch)
            .filter(|_| shown && config.status_branch);
        self.branch.show(branch.as_deref());
        let folder = page
            .and_then(|page| page.folder_label())
            .filter(|_| shown && config.status_folder);
        self.folder.show(folder.as_deref());
    }

    /// What Claude Code said of the session in front: its model, its context, its cost and its time, as the settings
    /// show them.
    fn show_usage(&self, usage: Option<&Usage>, config: &Config) {
        let model = usage.and_then(|usage| usage.model.clone()).filter(|_| config.status_model);
        self.model.show(model.as_deref());
        self.context.show(usage.filter(|_| config.status_context));
        let cost = usage.and_then(|usage| usage.cost).filter(|_| config.status_cost);
        self.cost.show(cost.map(usage::cost_text).as_deref());
        let time = usage.and_then(|usage| usage.duration_ms).filter(|_| config.status_time);
        self.time.show(time.map(usage::clock).as_deref());
    }

    /// Names the buttons again in the language now chosen.
    pub fn relabel(&self) {
        for (widget, tooltip) in [
            (self.sidebar_button.upcast_ref::<gtk::Widget>(), SIDEBAR_TOOLTIP),
            (self.new_tab.upcast_ref(), NEW_TAB_TOOLTIP),
            (self.profiles.upcast_ref(), PROFILES_TOOLTIP),
            (self.waiting.upcast_ref(), WAITING_TOOLTIP),
            (self.context.button.upcast_ref(), CONTEXT_TOOLTIP),
            (self.menu.upcast_ref(), MENU_TOOLTIP),
        ] {
            widget.set_tooltip_text(Some(tr(tooltip)));
        }
    }

    /// Closes the subagents card, as one of its rows opens the drawer.
    pub fn close_subagents(&self) {
        self.subagents.close();
    }

    /// How many other tabs, in every window, need the user; hidden when none does.
    pub fn show_waiting(&self, count: usize) {
        self.waiting.set_visible(count > 0);
        self.waiting_label.set_text(&trf("{0} 個等待回應", &[&count]));
    }
}

/// The tabs, then the new tab and its menu right after the last; see [`StripLayout`].
fn tab_strip(tab_view: &adw::TabView) -> gtk::Box {
    let strip = gtk::Box::builder().hexpand(true).build();
    strip.set_layout_manager(Some(StripLayout::default()));
    // A tab more or less changes the width the bar wants.
    tab_view.connect_n_pages_notify(glib::clone!(
        #[weak]
        strip,
        move |_| strip.queue_resize()
    ));
    strip
}

fn waiting_button() -> (gtk::Button, gtk::Label) {
    let label = gtk::Label::new(None);
    let content = gtk::Box::builder().spacing(6).build();
    let mark = dot();
    mark.add_css_class(Status::Waiting.css_class());
    content.append(&mark);
    content.append(&label);
    let button = gtk::Button::builder()
        .child(&content)
        .action_name("app.next-waiting")
        .tooltip_text(tr(WAITING_TOOLTIP))
        .css_classes(["pill", "term-waiting"])
        .valign(gtk::Align::Center)
        .visible(false)
        .build();
    (button, label)
}

fn tool_button(icon: &str, tooltip: &str, action: &str) -> gtk::Button {
    gtk::Button::builder()
        .icon_name(icon)
        .tooltip_text(tooltip)
        .action_name(action)
        .build()
}

fn menu_button(icon: &str, tooltip: &str, menu: &gio::Menu) -> gtk::MenuButton {
    gtk::MenuButton::builder()
        .icon_name(icon)
        .tooltip_text(tooltip)
        .menu_model(menu)
        .build()
}

/// The profiles a tab can start, the default one first, as Nekominal's menu beside the new tab; then a new window.
fn profiles_menu() -> gio::Menu {
    let default = config::current().default_profile;
    let mut all = profiles::all();
    all.sort_by_key(|profile| profile.name != default);
    let shells = gio::Menu::new();
    for profile in all {
        let item = gio::MenuItem::new(Some(&profile.name), None);
        item.set_action_and_target_value(Some("win.new-tab-profile"), Some(&profile.name.to_variant()));
        shells.append_item(&item);
    }
    let windows = gio::Menu::new();
    windows.append(Some(tr("新視窗")), Some("win.new-window"));
    let menu = gio::Menu::new();
    menu.append_section(None, &shells);
    menu.append_section(None, &windows);
    menu
}

/// The chevron beside the new tab; its list is read when it opens, so that a profile added shows.
fn new_menu_button() -> gtk::MenuButton {
    let button = menu_button("pan-down-symbolic", tr(PROFILES_TOOLTIP), &profiles_menu());
    button.set_create_popup_func(|button| button.set_menu_model(Some(&profiles_menu())));
    button.set_tooltip_text(Some(tr(PROFILES_TOOLTIP)));
    button
}

/// The window's menu, behind the three dots.
fn window_menu() -> gio::Menu {
    let tabs = gio::Menu::new();
    tabs.append(Some(tr("重新命名分頁")), Some("win.rename-tab"));
    tabs.append(Some(tr("向右分割")), Some("win.split-right"));
    tabs.append(Some(tr("向下分割")), Some("win.split-down"));
    tabs.append(Some(tr("尋找")), Some("win.find"));
    let view = gio::Menu::new();
    view.append(Some(tr("切換側邊欄")), Some("win.toggle-sidebar"));
    view.append(Some(tr("放大字型")), Some("win.zoom-in"));
    view.append(Some(tr("縮小字型")), Some("win.zoom-out"));
    view.append(Some(tr("重設字型大小")), Some("win.zoom-reset"));
    let app = gio::Menu::new();
    app.append(Some(tr("設定")), Some("app.preferences"));
    app.append(Some(tr("關於終端機")), Some("app.about"));
    let menu = gio::Menu::new();
    menu.append_section(None, &tabs);
    menu.append_section(None, &view);
    menu.append_section(None, &app);
    menu
}

/// The three dots; the menu is built as it opens, in the language chosen then.
fn window_menu_button() -> gtk::MenuButton {
    let button = menu_button("open-menu-symbolic", tr(MENU_TOOLTIP), &window_menu());
    button.set_create_popup_func(|button| button.set_menu_model(Some(&window_menu())));
    button
}
