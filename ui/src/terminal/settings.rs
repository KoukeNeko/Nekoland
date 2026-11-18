//! The settings as a tab of the window, laid out as Nekominal's page: a column of the sections at the left with the
//! search on top and the settings file at the bottom, and the section's title over its items at the right. A search
//! shows what it finds in every section, under the name of each. Each change is saved and shown at once.

use std::{
    cell::{Cell, RefCell},
    f64::consts::PI,
    rc::Rc,
};

use adw::prelude::*;
use gtk::{gdk, gio, glib};

use super::{
    lang::{tr, Language},
    agent::Status,
    config::{self, BellSound, Choice, Config, CursorShape, NoAgent, Osc52Policy, RightClick, TabLayout, TabStyle},
    profile_settings,
    push::{PushKind, PushWhen},
    push_settings, saved_settings,
    theme::{self, Scheme},
    title, trigger_settings, window, SHORTCUTS,
};

const DELETE_ICON: &str = "user-trash-symbolic";

thread_local! {
    /// The colours of the schemes' tiles, written again when a scheme comes or goes.
    static SCHEME_CSS: gtk::CssProvider = gtk::CssProvider::new();
}

/// Gives the display the tiles' colours.
pub fn show_scheme_css(display: &gdk::Display) {
    SCHEME_CSS.with(|provider| {
        gtk::style_context_add_provider_for_display(display, provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    });
    reload_scheme_css();
}

pub fn reload_scheme_css() {
    SCHEME_CSS.with(|provider| provider.load_from_string(&scheme_css()));
}

/// Nekominal's measures of the page.
const NAV_WIDTH: i32 = 220;
const NAV_PAD: i32 = 12;
const PAD: i32 = 32;
const CONTENT_MAX: i32 = 1600;
const FIELD_WIDTH: i32 = 280;
const THUMB_WIDTH: i32 = 200;
const THUMB_RATIO: f64 = 0.6;
const SCHEME_COLUMNS: u32 = 6;
/// How many of a scheme's colours its tile shows: red to cyan, as Nekominal's do.
const SWATCHES: std::ops::Range<usize> = 1..7;
const REPO: &str = "https://github.com/KoukeNeko/Nekoland";
const ERROR_CLASS: &str = "error";
const SEARCH_PLACEHOLDER: &str = "搜尋設定";
const FILE_TITLE: &str = "設定 JSON";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Appearance,
    Terminal,
    Profiles,
    Agents,
    Previews,
    Triggers,
    Keys,
    Advanced,
    About,
}

impl Section {
    /// Nekominal's order of the pages.
    const ALL: [Self; 9] = [
        Self::Appearance,
        Self::Terminal,
        Self::Profiles,
        Self::Agents,
        Self::Previews,
        Self::Triggers,
        Self::Keys,
        Self::Advanced,
        Self::About,
    ];

    /// The section a name in an action stands for, as Nekominal's file names its pages.
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|section| section.id() == id)
    }

    fn id(self) -> &'static str {
        match self {
            Self::Appearance => "appearance",
            Self::Terminal => "terminal",
            Self::Profiles => "profiles",
            Self::Agents => "agents",
            Self::Previews => "previews",
            Self::Triggers => "triggers",
            Self::Keys => "keys",
            Self::Advanced => "advanced",
            Self::About => "about",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Appearance => tr("外觀"),
            Self::Terminal => tr("終端機"),
            Self::Profiles => tr("設定檔"),
            Self::Agents => "Agent",
            Self::Previews => tr("預覽"),
            Self::Triggers => tr("觸發規則"),
            Self::Keys => tr("快速鍵"),
            Self::Advanced => tr("進階"),
            Self::About => tr("關於"),
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Self::Appearance => "applications-graphics-symbolic",
            Self::Terminal => "utilities-terminal-symbolic",
            Self::Profiles => "avatar-default-symbolic",
            Self::Agents => "system-run-symbolic",
            Self::Previews => "image-x-generic-symbolic",
            Self::Triggers => "thunderbolt-symbolic",
            Self::Keys => "preferences-desktop-keyboard-shortcuts-symbolic",
            Self::Advanced => "preferences-other-symbolic",
            Self::About => "help-about-symbolic",
        }
    }
}

type Get<T> = fn(&Config) -> T;
type Set<T> = fn(&mut Config, T);
/// Whether a line of text may be taken.
type Check = fn(&str) -> bool;

/// What an item of the page holds.
#[derive(Clone, Copy)]
enum Control {
    /// A heading over the items after it on its page.
    Heading,
    Layouts,
    Styles,
    Themes,
    Toggle(Get<bool>, Set<bool>),
    Number { get: Get<f64>, set: Set<f64>, range: (f64, f64), step: f64 },
    Options { labels: fn() -> Vec<&'static str>, get: Get<usize>, set: Set<usize> },
    Text { get: Get<String>, set: Set<String>, check: Check },
    /// The profiles, one of them the default, each with its editor.
    Profiles,
    /// The rules of the Triggers page and the ready-made ones.
    Triggers,
    /// The snippets, each a name and what it types.
    Snippets,
    /// The window layouts saved under a name.
    SavedLayouts,
    /// Where phone notifications go, their token, which events, and a test.
    PushUrl,
    PushToken,
    PushEvents,
    PushTest,
    /// The shortcut of a command, by its place in [`SHORTCUTS`].
    Key(usize),
    Version,
    Repo,
}

#[derive(Clone, Copy)]
struct Item {
    section: Section,
    label: &'static str,
    control: Control,
}

fn labels<T: Choice>() -> Vec<&'static str> {
    T::ALL.iter().map(|choice| choice.label()).collect()
}

fn index_of<T: Choice>(chosen: T) -> usize {
    T::ALL.iter().position(|listed| *listed == chosen).unwrap_or_default()
}

fn nth<T: Choice>(index: usize) -> T {
    T::ALL.get(index).copied().unwrap_or(T::ALL[0])
}

const fn toggle(section: Section, label: &'static str, get: Get<bool>, set: Set<bool>) -> Item {
    Item { section, label, control: Control::Toggle(get, set) }
}

/// A whole number of the settings, stepped by `step` in `range`.
const fn count(section: Section, label: &'static str, get: Get<f64>, set: Set<f64>, range: (f64, f64), step: f64) -> Item {
    Item { section, label, control: Control::Number { get, set, range, step } }
}

const fn text(section: Section, label: &'static str, get: Get<String>, set: Set<String>, check: Check) -> Item {
    Item { section, label, control: Control::Text { get, set, check } }
}

const PERCENT: f64 = 100.0;

fn is_given(text: &str) -> bool {
    !text.trim().is_empty()
}

fn whole(value: f64) -> u32 {
    value.round().max(0.0) as u32
}

/// Every item, section by section, in the order the page shows them; the shortcuts come from [`SHORTCUTS`].
const ITEMS: &[Item] = &[
    Item { section: Section::Appearance, label: "分頁版面", control: Control::Layouts },
    Item { section: Section::Appearance, label: TAB_STYLE, control: Control::Styles },
    toggle(Section::Appearance, SIDEBAR_OPEN, |c| c.sidebar_open, |c, on| c.sidebar_open = on),
    toggle(Section::Appearance, GROUP_BY_STATE, |c| c.group_by_state, |c, on| c.group_by_state = on),
    Item { section: Section::Appearance, label: "色彩配置", control: Control::Themes },
    count(
        Section::Appearance,
        "背景不透明度（%）",
        |c| (c.opacity * PERCENT).round(),
        |c, percent| c.opacity = percent.round() / PERCENT,
        (config::OPACITY_RANGE.0 * PERCENT, config::OPACITY_RANGE.1 * PERCENT),
        5.0,
    ),
    text(Section::Appearance, "字型", |c| c.font_family.clone(), |c, family| c.font_family = family, is_given),
    Item {
        section: Section::Appearance,
        label: "字型大小",
        control: Control::Number {
            get: |c| c.font_size,
            set: |c, size| c.font_size = size,
            range: config::FONT_SIZE_RANGE,
            step: 1.0,
        },
    },
    Item {
        section: Section::Appearance,
        label: "行高",
        control: Control::Number {
            get: |c| c.line_height,
            set: |c, height| c.line_height = height,
            range: config::LINE_HEIGHT_RANGE,
            step: 0.05,
        },
    },
    toggle(Section::Appearance, LIGATURES, |c| c.ligatures, |c, on| c.ligatures = on),
    Item {
        section: Section::Appearance,
        label: "游標形狀",
        control: Control::Options {
            labels: labels::<CursorShape>,
            get: |c| index_of(c.cursor_shape),
            set: |c, i| c.cursor_shape = nth(i),
        },
    },
    toggle(Section::Appearance, "游標閃爍", |c| c.cursor_blink, |c, on| c.cursor_blink = on),
    count(
        Section::Appearance,
        BLINK_RATE,
        |c| f64::from(c.cursor_blink_rate),
        |c, rate| c.cursor_blink_rate = whole(rate),
        config::BLINK_RATE_RANGE,
        50.0,
    ),
    text(Section::Appearance, "標題格式", |c| c.tab_title.clone(), |c, format| c.tab_title = format, title::is_valid),
    count(
        Section::Terminal,
        "捲動緩衝區行數",
        |c| f64::from(c.scrollback),
        |c, lines| c.scrollback = whole(lines),
        config::SCROLLBACK_RANGE,
        1_000.0,
    ),
    toggle(Section::Terminal, "選取時複製", |c| c.copy_on_select, |c, on| c.copy_on_select = on),
    toggle(Section::Terminal, "捲動時將指令列固定在頂端", |c| c.sticky_header, |c, on| c.sticky_header = on),
    Item {
        section: Section::Terminal,
        label: "右鍵",
        control: Control::Options {
            labels: labels::<RightClick>,
            get: |c| index_of(c.right_click),
            set: |c, i| c.right_click = nth(i),
        },
    },
    toggle(Section::Terminal, "貼上多行前確認", |c| c.warn_multiline, |c, on| c.warn_multiline = on),
    count(
        Section::Terminal,
        "貼上大量文字前確認（KiB）",
        |c| f64::from(c.warn_large_kib),
        |c, kib| c.warn_large_kib = whole(kib),
        config::LARGE_PASTE_RANGE,
        1.0,
    ),
    toggle(Section::Terminal, "鈴聲視覺提示", |c| c.bell_visual, |c, on| c.bell_visual = on),
    toggle(Section::Terminal, "工作列提示", |c| c.bell_taskbar, |c, on| c.bell_taskbar = on),
    Item {
        section: Section::Terminal,
        label: "鈴聲音效",
        control: Control::Options {
            labels: labels::<BellSound>,
            get: |c| index_of(c.bell_sound),
            set: |c, i| c.bell_sound = nth(i),
        },
    },
    toggle(Section::Terminal, "關閉前確認", |c| c.confirm_close, |c, on| c.confirm_close = on),
    count(
        Section::Terminal,
        "觸發確認的工作階段數",
        |c| f64::from(c.confirm_close_threshold),
        |c, sessions| c.confirm_close_threshold = whole(sessions),
        config::CLOSE_THRESHOLD_RANGE,
        1.0,
    ),
    toggle(Section::Terminal, "啟動時還原工作階段", |c| c.restore_session, |c, on| c.restore_session = on),
    Item { section: Section::Terminal, label: "桌面通知", control: Control::Heading },
    toggle(Section::Terminal, "顯示桌面通知", |c| c.notifications, |c, on| c.notifications = on),
    count(
        Section::Terminal,
        "長指令結束時通知（秒，0 為關閉）",
        |c| f64::from(c.long_command),
        |c, seconds| c.long_command = whole(seconds),
        config::LONG_COMMAND_RANGE,
        5.0,
    ),
    Item { section: Section::Terminal, label: push_settings::HEADING, control: Control::Heading },
    Item { section: Section::Terminal, label: push_settings::URL, control: Control::PushUrl },
    Item {
        section: Section::Terminal,
        label: push_settings::KIND,
        control: Control::Options {
            labels: labels::<PushKind>,
            get: |c| index_of(c.push.kind),
            set: |c, i| c.push.kind = nth(i),
        },
    },
    Item {
        section: Section::Terminal,
        label: push_settings::WHEN,
        control: Control::Options {
            labels: labels::<PushWhen>,
            get: |c| index_of(c.push.when),
            set: |c, i| c.push.when = nth(i),
        },
    },
    Item { section: Section::Terminal, label: push_settings::TOKEN, control: Control::PushToken },
    Item { section: Section::Terminal, label: push_settings::EVENTS, control: Control::PushEvents },
    Item { section: Section::Terminal, label: push_settings::TEST, control: Control::PushTest },
    Item { section: Section::Terminal, label: saved_settings::SNIPPETS, control: Control::Snippets },
    Item { section: Section::Profiles, label: "預設設定檔", control: Control::Profiles },
    Item { section: Section::Profiles, label: saved_settings::LAYOUTS, control: Control::SavedLayouts },
    toggle(Section::Agents, "代理程式環境變數", |c| c.agent_environment, |c, on| c.agent_environment = on),
    toggle(Section::Agents, "顯示狀態列", |c| c.status_bar, |c, on| c.status_bar = on),
    Item {
        section: Section::Agents,
        label: "沒有 agent 時",
        control: Control::Options {
            labels: labels::<NoAgent>,
            get: |c| index_of(c.status_no_agent),
            set: |c, i| c.status_no_agent = nth(i),
        },
    },
    toggle(Section::Agents, "狀態列：工作狀態", |c| c.status_state, |c, on| c.status_state = on),
    toggle(Section::Agents, "狀態列：模型", |c| c.status_model, |c, on| c.status_model = on),
    toggle(Section::Agents, "狀態列：脈絡用量", |c| c.status_context, |c, on| c.status_context = on),
    toggle(Section::Agents, "狀態列：費用", |c| c.status_cost, |c, on| c.status_cost = on),
    toggle(Section::Agents, "狀態列：工作階段時間", |c| c.status_time, |c, on| c.status_time = on),
    toggle(Section::Agents, "狀態列：Git 分支", |c| c.status_branch, |c, on| c.status_branch = on),
    toggle(Section::Agents, "狀態列：資料夾", |c| c.status_folder, |c, on| c.status_folder = on),
    toggle(Section::Agents, "kitty 鍵盤協定", |c| c.kitty_keyboard, |c, on| c.kitty_keyboard = on),
    toggle(Section::Agents, "Shell 整合", |c| c.shell_integration, |c, on| c.shell_integration = on),
    Item {
        section: Section::Agents,
        label: "OSC 52 剪貼簿",
        control: Control::Options {
            labels: labels::<Osc52Policy>,
            get: |c| index_of(c.osc52),
            set: |c, i| c.osc52 = nth(i),
        },
    },
    toggle(Section::Previews, "游標指到圖片時顯示預覽", |c| c.preview_images, |c, on| c.preview_images = on),
    count(
        Section::Previews,
        PREVIEW_SIZE,
        |c| f64::from(c.preview_size),
        |c, size| c.preview_size = whole(size),
        config::PREVIEW_SIZE_RANGE,
        20.0,
    ),
    Item { section: Section::Triggers, label: "觸發規則", control: Control::Triggers },
    Item {
        section: Section::Advanced,
        label: "語言",
        control: Control::Options {
            labels: labels::<Language>,
            get: |c| index_of(c.language),
            set: |c, i| c.language = nth(i),
        },
    },
    toggle(
        Section::Advanced,
        "新分頁沿用終端機啟動時的環境變數",
        |c| c.inherit_environment,
        |c, on| c.inherit_environment = on,
    ),
    // Empty is the desktop's own editor for the file.
    text(Section::Advanced, "開啟「檔案:行」的編輯器", |c| c.editor.clone(), |c, editor| c.editor = editor, |_| true),
    Item { section: Section::About, label: "終端機", control: Control::Version },
    Item { section: Section::About, label: "原始碼", control: Control::Repo },
];

const SIDEBAR_OPEN: &str = "開啟時顯示側邊欄";
const GROUP_BY_STATE: &str = "依狀態分組";
const BLINK_RATE: &str = "游標閃爍間隔（毫秒）";
const LIGATURES: &str = "字型連字";
/// The rows that say what their setting does, as Nekominal's notes.
const NOTES: [(&str, &str); 2] = [
    (LIGATURES, "=>、>= 等組合顯示為一個字形"),
    (TAB_STYLE, "側邊欄 · 清單時，每個分頁顯示多少"),
];
const TAB_STYLE: &str = "側邊欄的分頁";
const PREVIEW_SIZE: &str = "預覽大小（最長邊，像素）";

/// Whether the item does anything with the others as they are, as Nekominal decides it: the sidebar's do nothing with
/// the tabs on top, and the groups only in a list not by repository.
fn applies(item: &Item, config: &Config) -> bool {
    match item.label {
        SIDEBAR_OPEN => config.tab_layout.is_side(),
        GROUP_BY_STATE => config.tab_layout == TabLayout::List && config.tab_style != TabStyle::Repo,
        BLINK_RATE => config.cursor_blink,
        PREVIEW_SIZE => config.preview_images,
        _ => true,
    }
}

fn all_items() -> impl Iterator<Item = Item> {
    let keys = SHORTCUTS.iter().enumerate().map(|(index, (_, _, label))| Item {
        section: Section::Keys,
        label,
        control: Control::Key(index),
    });
    ITEMS.iter().copied().chain(keys)
}

/// The items `query` finds, by section in the order of the sections; headings are not found.
fn found(query: &str) -> Vec<Item> {
    let query = query.trim().to_lowercase();
    let mut items: Vec<Item> = all_items()
        .filter(|item| !matches!(item.control, Control::Heading))
        .filter(|item| [item.label, tr(item.label)].iter().any(|label| label.to_lowercase().contains(&query)))
        .collect();
    items.sort_by_key(|item| Section::ALL.iter().position(|section| *section == item.section));
    items
}

/// The settings the command palette lists, each with its page: the shortcuts are its commands already.
pub fn searchable() -> Vec<(&'static str, Section)> {
    all_items()
        .filter(|item| !matches!(item.control, Control::Heading | Control::Key(_)))
        .map(|item| (item.label, item.section))
        .collect()
}

/// The items of `section` as it shows them: the styles of the list only while the tabs are a list.
fn in_section(section: Section, config: &Config) -> Vec<Item> {
    all_items()
        .filter(|item| item.section == section)
        .filter(|item| !matches!(item.control, Control::Styles) || config.tab_layout == TabLayout::List)
        .collect()
}

/// Saves a change and shows it in every window.
pub fn change(edit: impl FnOnce(&mut Config)) {
    config::update(edit);
    window::apply_config();
}

pub struct SettingsPage {
    pub widget: gtk::Box,
    nav: gtk::ListBox,
    counts: Vec<gtk::Label>,
    /// The names of the sections and of the settings file, named again when the language changes.
    nav_titles: Vec<gtk::Label>,
    file_title: gtk::Label,
    search: gtk::SearchEntry,
    title: gtk::Label,
    /// At the right of the title: what adds to the page, as the Triggers page's new rule.
    title_action: gtk::Box,
    /// Under the title: what the page is for, where Nekominal says it.
    intro: gtk::Label,
    items: gtk::Box,
    /// The rows shown, to grey the ones that do nothing as the settings change.
    rows: RefCell<Vec<(gtk::Widget, Item)>>,
    scroller: gtk::ScrolledWindow,
    section: RefCell<Section>,
    /// The profile whose editor the Profiles page shows instead of the list.
    profile: Rc<RefCell<Option<String>>>,
    /// The rule of the Triggers page open in place; one past the last is the one being added.
    trigger: Rc<Cell<Option<usize>>>,
}

impl SettingsPage {
    pub fn new(section: Section) -> Rc<Self> {
        let search = gtk::SearchEntry::builder()
            .placeholder_text(tr(SEARCH_PLACEHOLDER))
            .margin_start(NAV_PAD)
            .margin_end(NAV_PAD)
            .margin_top(NAV_PAD)
            .margin_bottom(NAV_PAD)
            .build();
        let nav = gtk::ListBox::builder()
            .css_classes(["navigation-sidebar"])
            .vexpand(true)
            .build();
        let mut counts = Vec::new();
        let mut nav_titles = Vec::new();
        for section in Section::ALL {
            let (row, count, title) = nav_row(section);
            nav.append(&row);
            counts.push(count);
            nav_titles.push(title);
        }
        let file_title = gtk::Label::builder().label(tr(FILE_TITLE)).xalign(0.0).build();
        let file = file_button(&file_title);
        let column = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .width_request(NAV_WIDTH)
            .css_classes(["term-settings-nav"])
            .build();
        column.append(&search);
        column.append(&nav);
        column.append(&file);

        let title = gtk::Label::builder()
            .xalign(0.0)
            .hexpand(true)
            .css_classes(["title-2"])
            .build();
        let title_action = gtk::Box::builder().valign(gtk::Align::Center).build();
        let title_line = gtk::Box::builder().spacing(12).build();
        title_line.append(&title);
        title_line.append(&title_action);
        let intro = gtk::Label::builder()
            .xalign(0.0)
            .css_classes(["dim-label"])
            .margin_top(6)
            .visible(false)
            .build();
        let header = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .margin_bottom(18)
            .build();
        header.append(&title_line);
        header.append(&intro);
        let items = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        let page = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .margin_start(PAD)
            .margin_end(PAD)
            .margin_top(18)
            .margin_bottom(PAD)
            .build();
        page.append(&header);
        page.append(&items);
        let clamp = adw::Clamp::builder()
            .maximum_size(CONTENT_MAX)
            .tightening_threshold(CONTENT_MAX)
            .child(&page)
            .build();
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .hexpand(true)
            .vexpand(true)
            .child(&clamp)
            .build();

        let widget = gtk::Box::builder().css_classes(["term-settings"]).build();
        widget.append(&column);
        widget.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        widget.append(&scroller);
        let this = Rc::new(Self {
            widget,
            nav,
            counts,
            nav_titles,
            file_title,
            search,
            title,
            title_action,
            intro,
            items,
            rows: RefCell::default(),
            scroller,
            section: RefCell::new(section),
            profile: Rc::default(),
            trigger: Rc::default(),
        });
        this.connect();
        this.show_section(section);
        this
    }

    fn connect(self: &Rc<Self>) {
        self.nav.connect_row_activated(glib::clone!(
            #[weak(rename_to = this)]
            self,
            move |_, row| {
                if let Some(section) = Section::ALL.get(row.index() as usize) {
                    this.search.set_text("");
                    this.profile.take();
                    this.trigger.take();
                    this.show_section(*section);
                }
            }
        ));
        self.search.connect_search_changed(glib::clone!(
            #[weak(rename_to = this)]
            self,
            move |_| this.render()
        ));
    }

    pub fn show_section(self: &Rc<Self>, section: Section) {
        self.section.replace(section);
        self.render();
    }

    /// Opens what `detail` names on the page shown: a profile's editor by its name, or a rule's form by its number.
    pub fn open_detail(self: &Rc<Self>, detail: &str) {
        let section = *self.section.borrow();
        match section {
            Section::Profiles => {
                self.profile.replace(Some(detail.to_string()));
            }
            Section::Triggers => self.trigger.set(detail.parse().ok()),
            _ => return,
        }
        self.render();
    }

    /// Shows what changed outside the page, as a layout saved from the palette.
    /// Names everything again in the language now chosen.
    pub fn relabel(self: &Rc<Self>) {
        self.search.set_placeholder_text(Some(tr(SEARCH_PLACEHOLDER)));
        for (title, section) in self.nav_titles.iter().zip(Section::ALL) {
            title.set_text(section.title());
        }
        self.file_title.set_text(tr(FILE_TITLE));
        self.refresh();
    }

    pub fn refresh(self: &Rc<Self>) {
        (self.rerender_soon())();
    }

    /// Lays the page out again where it is scrolled to, after the event being handled: for a change that adds or takes
    /// away rows.
    fn rerender_soon(self: &Rc<Self>) -> Rc<dyn Fn()> {
        let weak = Rc::downgrade(self);
        Rc::new(move || {
            let weak = weak.clone();
            glib::idle_add_local_once(move || {
                let Some(this) = weak.upgrade() else {
                    return;
                };
                let scrolled = this.scroller.vadjustment().value();
                this.render();
                let adjustment = this.scroller.vadjustment();
                glib::idle_add_local_once(move || adjustment.set_value(scrolled));
            });
        })
    }

    /// The title's line: the Triggers page adds a rule from it and says under it what a rule is.
    fn show_header(self: &Rc<Self>, section: Section, searching: bool) {
        self.title
            .set_text(if searching { tr("搜尋結果") } else { section.title() });
        while let Some(child) = self.title_action.first_child() {
            self.title_action.remove(&child);
        }
        let triggers = !searching && section == Section::Triggers;
        self.intro.set_visible(triggers);
        if triggers {
            self.intro.set_text(tr(trigger_settings::INTRO));
            self.title_action.append(&trigger_settings::add_button(&self.trigger, &self.rerender_soon()));
        }
    }

    /// Lays the page out again: the section's items, or what the search finds.
    fn render(self: &Rc<Self>) {
        let query = self.search.text();
        let searching = !query.trim().is_empty();
        let config = config::current();
        let section = *self.section.borrow();
        let found = found(&query);
        for (index, listed) in Section::ALL.iter().enumerate() {
            let count = found.iter().filter(|item| item.section == *listed).count();
            self.counts[index].set_visible(searching);
            self.counts[index].set_text(&count.to_string());
        }
        match (searching, Section::ALL.iter().position(|listed| *listed == section)) {
            (false, Some(index)) => self.nav.select_row(self.nav.row_at_index(index as i32).as_ref()),
            _ => self.nav.unselect_all(),
        }
        self.show_header(section, searching);
        while let Some(child) = self.items.first_child() {
            self.items.remove(&child);
        }
        self.rows.borrow_mut().clear();
        if searching {
            self.show_found(&found);
        } else {
            self.show_items(&in_section(section, &config));
        }
        self.scroller.vadjustment().set_value(0.0);
    }

    fn show_found(self: &Rc<Self>, found: &[Item]) {
        if found.is_empty() {
            self.items.append(
                &gtk::Label::builder()
                    .label(tr("沒有符合的設定"))
                    .xalign(0.0)
                    .css_classes(["dim-label"])
                    .build(),
            );
            return;
        }
        for section in Section::ALL {
            let items: Vec<Item> = found.iter().copied().filter(|item| item.section == section).collect();
            if !items.is_empty() {
                self.items.append(&heading(section.title()));
                self.show_items(&items);
            }
        }
    }

    /// The items in runs of plates, a heading starting a new run; the profiles and the rules lay out their own.
    fn show_items(self: &Rc<Self>, items: &[Item]) {
        let mut run: Vec<Item> = Vec::new();
        for item in items {
            let own: Option<Vec<gtk::Widget>> = match item.control {
                Control::Heading => Some(vec![heading(item.label).upcast()]),
                Control::Profiles => Some(profile_settings::widgets(&self.profile, &self.rerender_soon())),
                Control::Triggers => Some(trigger_settings::widgets(&self.trigger, &self.rerender_soon())),
                Control::Snippets => Some(saved_settings::snippet_widgets(&self.rerender_soon())),
                Control::SavedLayouts => Some(saved_settings::layout_widgets()),
                _ => None,
            };
            let Some(widgets) = own else {
                run.push(*item);
                continue;
            };
            if !run.is_empty() {
                self.items.append(&self.item_list(&run));
                run.clear();
            }
            for widget in widgets {
                self.items.append(&widget);
            }
        }
        if !run.is_empty() {
            self.items.append(&self.item_list(&run));
        }
    }

    fn item_list(self: &Rc<Self>, items: &[Item]) -> gtk::ListBox {
        let list = plates();
        let config = config::current();
        for item in items {
            let row = self.item_row(item, &config);
            row.set_sensitive(applies(item, &config));
            list.append(&row);
            self.rows.borrow_mut().push((row, *item));
        }
        list
    }

    /// Greys the rows that do nothing with the settings as they now are, and lets the others be pressed again.
    pub fn show_applies(&self) {
        let config = config::current();
        for (row, item) in self.rows.borrow().iter() {
            row.set_sensitive(applies(item, &config));
        }
    }

    fn item_row(self: &Rc<Self>, item: &Item, config: &Config) -> gtk::Widget {
        let label = tr(item.label);
        match item.control {
            Control::Heading | Control::Profiles | Control::Triggers | Control::Snippets | Control::SavedLayouts => {
                heading(label).upcast()
            }
            Control::Layouts => self.layout_row(label, config).upcast(),
            Control::Styles => style_row(label, config).upcast(),
            Control::Themes => scheme_row(label, &config.scheme).upcast(),
            Control::Toggle(get, set) => {
                let row = switch_row(label, get(config), set);
                if let Some((_, note)) = NOTES.iter().find(|(noted, _)| *noted == item.label) {
                    row.set_subtitle(tr(note));
                }
                row.upcast()
            }
            Control::Number { get, set, range, step } => {
                spin_row(label, get(config), range, step, set).upcast()
            }
            Control::Options { labels, get, set } => {
                options_row(label, &labels(), get(config), set).upcast()
            }
            Control::Text { get, set, check } => text_row(label, get, set, check).upcast(),
            Control::Key(index) => key_row(index).upcast(),
            Control::Version => version_row(label).upcast(),
            Control::Repo => repo_row(label).upcast(),
            Control::PushUrl => push_settings::url_row().upcast(),
            Control::PushToken => push_settings::token_row().upcast(),
            Control::PushEvents => push_settings::events_row().upcast(),
            Control::PushTest => push_settings::test_row().upcast(),
        }
    }

    /// The layouts, as pictures; picking one shows or hides the styles of the list under it.
    fn layout_row(self: &Rc<Self>, label: &str, config: &Config) -> gtk::ListBoxRow {
        let chosen = index_of(config.tab_layout);
        let weak = Rc::downgrade(self);
        pictures_row(label, None, &labels::<TabLayout>(), chosen, Picture::Layout, move |index| {
            change(|config| config.tab_layout = nth(index));
            if let Some(this) = weak.upgrade() {
                // After the click that is still being handled, not under it.
                glib::idle_add_local_once(move || this.render());
            }
        })
    }
}

fn nav_row(section: Section) -> (gtk::ListBoxRow, gtk::Label, gtk::Label) {
    let count = gtk::Label::builder()
        .css_classes(["caption", "dim-label"])
        .visible(false)
        .build();
    let line = gtk::Box::builder().spacing(12).build();
    line.append(&gtk::Image::from_icon_name(section.icon()));
    let title = gtk::Label::builder().label(section.title()).xalign(0.0).hexpand(true).build();
    line.append(&title);
    line.append(&count);
    (gtk::ListBoxRow::builder().child(&line).build(), count, title)
}

/// The settings file, opened in the editor the desktop has for it.
fn file_button(title: &gtk::Label) -> gtk::Button {
    let content = gtk::Box::builder().spacing(12).build();
    content.append(&gtk::Image::from_icon_name("document-edit-symbolic"));
    content.append(title);
    let button = gtk::Button::builder()
        .child(&content)
        .css_classes(["flat"])
        .margin_start(NAV_PAD - 6)
        .margin_end(NAV_PAD - 6)
        .margin_bottom(NAV_PAD)
        .build();
    button.connect_clicked(|button| {
        // The file is written out first: until a setting changes there may be none to open.
        if let Err(error) = config::save(&config::current()) {
            eprintln!("nekoland-terminal: could not write {}: {error}", config::path().display());
            return;
        }
        let launcher = gtk::FileLauncher::new(Some(&gio::File::for_path(config::path())));
        let window = button.root().and_downcast::<gtk::Window>();
        launcher.launch(window.as_ref(), gio::Cancellable::NONE, |result| {
            if let Err(error) = result {
                eprintln!("nekoland-terminal: could not open the settings file: {error}");
            }
        });
    });
    button
}

/// A list of plates, the items of the page on it.
pub fn plates() -> gtk::ListBox {
    gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["term-items"])
        .build()
}

pub fn heading(text: &'static str) -> gtk::Label {
    gtk::Label::builder()
        .label(tr(text))
        .xalign(0.0)
        .css_classes(["heading", "dim-label", "term-settings-heading"])
        .build()
}

fn switch_row(title: &str, active: bool, set: Set<bool>) -> adw::SwitchRow {
    let row = adw::SwitchRow::builder().title(title).active(active).build();
    row.connect_active_notify(move |row| {
        let on = row.is_active();
        change(|config| set(config, on));
    });
    row
}

fn spin_row(title: &str, value: f64, (min, max): (f64, f64), step: f64, set: Set<f64>) -> adw::SpinRow {
    let adjustment = gtk::Adjustment::new(value, min, max, step, step * 4.0, 0.0);
    let digits = if step.fract() == 0.0 { 0 } else { 2 };
    let row = adw::SpinRow::builder()
        .title(title)
        .adjustment(&adjustment)
        .digits(digits)
        .build();
    row.connect_value_notify(move |row| {
        let value = row.value();
        change(|config| set(config, value));
    });
    row
}

/// A choice of a few: a linked button an option at the right of the row, as Nekominal's option buttons.
fn options_row(title: &str, options: &[&str], chosen: usize, set: Set<usize>) -> adw::ActionRow {
    let buttons = gtk::Box::builder()
        .css_classes(["linked"])
        .valign(gtk::Align::Center)
        .build();
    let mut first: Option<gtk::ToggleButton> = None;
    for (index, option) in options.iter().enumerate() {
        let button = gtk::ToggleButton::builder()
            .label(*option)
            .active(index == chosen)
            .build();
        if let Some(first) = &first {
            button.set_group(Some(first));
        } else {
            first = Some(button.clone());
        }
        button.connect_toggled(move |button| {
            if button.is_active() {
                change(|config| set(config, index));
            }
        });
        buttons.append(&button);
    }
    let row = adw::ActionRow::builder().title(title).build();
    row.add_suffix(&buttons);
    row
}

/// Marks a field whose text cannot be taken, `why` in its icon's tooltip; None takes the mark away.
pub fn mark_refused(field: &gtk::Entry, why: Option<&'static str>) {
    let why = why.map(tr);
    match why {
        Some(why) => {
            field.add_css_class(ERROR_CLASS);
            field.set_secondary_icon_name(Some("dialog-error-symbolic"));
            field.set_secondary_icon_tooltip_text(Some(why));
        }
        None => {
            field.remove_css_class(ERROR_CLASS);
            field.set_secondary_icon_name(None);
        }
    }
}

/// A field that hands its text to `taken` when it is confirmed or left changed.
pub fn field(text: &str, taken: impl Fn(&gtk::Entry, String) + 'static) -> gtk::Entry {
    let field = gtk::Entry::builder()
        .text(text)
        .valign(gtk::Align::Center)
        .build();
    let taken = Rc::new(taken);
    let shown = Rc::new(RefCell::new(text.to_string()));
    field.connect_activate(glib::clone!(
        #[strong]
        taken,
        #[strong]
        shown,
        move |field| {
            shown.replace(field.text().to_string());
            taken(field, field.text().to_string());
        }
    ));
    let focus = gtk::EventControllerFocus::new();
    focus.connect_leave(glib::clone!(
        #[weak]
        field,
        move |_| {
            let text = field.text().to_string();
            if *shown.borrow() != text {
                shown.replace(text.clone());
                taken(&field, text);
            }
        }
    ));
    field.add_controller(focus);
    field
}

/// A line of text in a field at the right; text `check` refuses is marked and not taken.
fn text_row(title: &str, get: Get<String>, set: Set<String>, check: Check) -> adw::ActionRow {
    let field = field(&get(&config::current()), move |field, text| {
        let text = text.trim().to_string();
        if !check(&text) {
            mark_refused(field, Some(tr("無法使用這個值")));
            return;
        }
        mark_refused(field, None);
        change(|config| set(config, text));
    });
    field.set_width_request(FIELD_WIDTH);
    let row = adw::ActionRow::builder().title(title).build();
    row.add_suffix(&field);
    row
}

fn key_row(index: usize) -> adw::ActionRow {
    let (_, accels, label) = SHORTCUTS[index];
    let row = adw::ActionRow::builder().title(tr(label)).build();
    for accel in accels.iter().rev() {
        row.add_suffix(
            &gtk::ShortcutLabel::builder()
                .accelerator(*accel)
                .valign(gtk::Align::Center)
                .build(),
        );
    }
    row
}

fn version_row(title: &str) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(format!("{title}  {}", env!("CARGO_PKG_VERSION")))
        .subtitle(tr("版本"))
        .build();
    row.add_prefix(&gtk::Image::builder().icon_name("utilities-terminal").pixel_size(32).build());
    // An update installed a newer copy over the one running: it takes over the tabs, their programs still running.
    if super::carry::is_replaced() {
        row.set_subtitle(tr("已安裝新版本"));
        let restart = gtk::Button::builder()
            .label(tr("更新並重新啟動"))
            .action_name("app.update-restart")
            .valign(gtk::Align::Center)
            .css_classes(["suggested-action"])
            .build();
        row.add_suffix(&restart);
    }
    row
}

fn repo_row(title: &str) -> adw::ActionRow {
    let address = gtk::Label::builder()
        .label(REPO.trim_start_matches("https://"))
        .css_classes(["caption", "dim-label"])
        .build();
    let open = gtk::Button::builder()
        .label(tr("開啟"))
        .valign(gtk::Align::Center)
        .build();
    open.connect_clicked(|button| {
        let window = button.root().and_downcast::<gtk::Window>();
        gtk::UriLauncher::new(REPO).launch(window.as_ref(), gio::Cancellable::NONE, |result| {
            if let Err(error) = result {
                eprintln!("nekoland-terminal: could not open {REPO}: {error}");
            }
        });
    });
    let row = adw::ActionRow::builder().title(title).build();
    row.add_suffix(&address);
    row.add_suffix(&open);
    row
}

/// An item whose control sits under its label, across the plate: the pictures, the tiles.
pub fn wide_row(title: &str, control: &impl IsA<gtk::Widget>) -> gtk::ListBoxRow {
    noted_wide_row(title, None, control)
}

/// A wide row with a line under its title that says what the setting does.
fn noted_wide_row(title: &str, note: Option<&str>, control: &impl IsA<gtk::Widget>) -> gtk::ListBoxRow {
    let heading = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(2).build();
    heading.append(&gtk::Label::builder().label(title).xalign(0.0).build());
    if let Some(note) = note {
        heading.append(&gtk::Label::builder().label(note).xalign(0.0).css_classes(["caption", "dim-label"]).build());
    }
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_start(12)
        .margin_end(12)
        .margin_top(14)
        .margin_bottom(14)
        .build();
    content.append(&heading);
    content.append(control);
    gtk::ListBoxRow::builder()
        .child(&content)
        .activatable(false)
        .build()
}

#[derive(Clone, Copy)]
enum Picture {
    Layout,
    Style,
}

/// Three pictures side by side, each named under it, the chosen one ringed.
fn pictures_row(
    title: &str,
    note: Option<&str>,
    names: &[&str],
    chosen: usize,
    picture: Picture,
    picked: impl Fn(usize) + Clone + 'static,
) -> gtk::ListBoxRow {
    let pictures = gtk::Box::builder().spacing(8).homogeneous(true).build();
    let mut first: Option<gtk::ToggleButton> = None;
    for (index, name) in names.iter().enumerate() {
        let drawing = gtk::DrawingArea::builder()
            .content_width(THUMB_WIDTH)
            .content_height((f64::from(THUMB_WIDTH) * THUMB_RATIO) as i32)
            .css_classes(["term-miniature"])
            .build();
        drawing.set_draw_func(move |area, cairo, width, height| {
            let colours = Colours::of(area);
            let result = match picture {
                Picture::Layout => draw_layout(cairo, &colours, width, height, TabLayout::ALL[index]),
                Picture::Style => draw_style(cairo, &colours, width, height, TabStyle::ALL[index]),
            };
            if let Err(error) = result {
                eprintln!("nekoland-terminal: could not draw a miniature: {error}");
            }
        });
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(4)
            .build();
        content.append(&drawing);
        content.append(&gtk::Label::builder().label(*name).css_classes(["caption"]).build());
        let button = gtk::ToggleButton::builder()
            .child(&content)
            .active(index == chosen)
            .css_classes(["flat", "term-picture"])
            .build();
        if let Some(first) = &first {
            button.set_group(Some(first));
        } else {
            first = Some(button.clone());
        }
        let picked = picked.clone();
        button.connect_toggled(move |button| {
            if button.is_active() {
                picked(index);
            }
        });
        pictures.append(&button);
    }
    noted_wide_row(title, note, &pictures)
}

fn style_row(title: &str, config: &Config) -> gtk::ListBoxRow {
    let note = NOTES.iter().find(|(noted, _)| *noted == TAB_STYLE).map(|(_, note)| tr(note));
    pictures_row(title, note, &labels::<TabStyle>(), index_of(config.tab_style), Picture::Style, |index| {
        change(|config| config.tab_style = nth(index));
    })
}

/// Every scheme as a tile in its own colours, the chosen one selected. Schemes are imported from the command palette,
/// as in Nekominal, whose page has only the tiles.
fn scheme_row(title: &str, chosen: &str) -> gtk::ListBoxRow {
    let tiles = scheme_tiles(Some(chosen), |name| change(|config| config.scheme = name.to_string()));
    wide_row(title, &tiles)
}

/// Takes a scheme of the user's own out of the settings.
fn delete_scheme(name: &str) {
    change(|config| config.schemes.retain(|scheme| scheme.name != name));
    reload_scheme_css();
    window::refresh_settings();
}

/// The schemes as tiles, `chosen` selected; `picked` is told the name of the one picked.
pub fn scheme_tiles(chosen: Option<&str>, picked: impl Fn(&str) + 'static) -> gtk::FlowBox {
    let tiles = gtk::FlowBox::builder()
        .css_classes(["term-schemes"])
        .selection_mode(gtk::SelectionMode::Single)
        .homogeneous(true)
        .min_children_per_line(2)
        .max_children_per_line(SCHEME_COLUMNS)
        .column_spacing(8)
        .row_spacing(8)
        .build();
    for (index, scheme) in theme::all_schemes().into_iter().enumerate() {
        // The user's own come after the built-in ones, and may share a name with one.
        tiles.append(&scheme_tile(index, scheme, index >= theme::SCHEMES.len()));
        if Some(scheme.name) == chosen {
            if let Some(child) = tiles.child_at_index(index as i32) {
                tiles.select_child(&child);
            }
        }
    }
    tiles.connect_selected_children_changed(move |tiles| {
        let Some(index) = tiles.selected_children().first().map(|child| child.index()) else {
            return;
        };
        if let Some(scheme) = theme::all_schemes().get(index as usize) {
            picked(scheme.name);
        }
    });
    tiles
}

fn scheme_tile(index: usize, scheme: &'static Scheme, is_own: bool) -> gtk::Box {
    let swatches = gtk::Box::builder().spacing(4).build();
    for swatch in SWATCHES {
        swatches.append(
            &gtk::Box::builder()
                .css_classes(["term-swatch", &swatch_class(index, swatch)])
                .build(),
        );
    }
    let tile = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(10)
        .css_classes(["term-scheme", &scheme_class(index)])
        .build();
    let name_line = gtk::Box::builder().spacing(4).build();
    name_line.append(
        &gtk::Label::builder()
            .label(scheme.name)
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["caption"])
            .build(),
    );
    // A scheme of the user's own can go again; the built-in ones cannot.
    if is_own {
        let delete = gtk::Button::builder()
            .icon_name(DELETE_ICON)
            .tooltip_text(tr("刪除色彩配置"))
            .css_classes(["flat", "circular", "term-scheme-delete"])
            .build();
        delete.connect_clicked(move |_| delete_scheme(scheme.name));
        name_line.append(&delete);
    }
    tile.append(&name_line);
    tile.append(&swatches);
    tile
}

fn scheme_class(index: usize) -> String {
    format!("term-scheme-{index}")
}

fn swatch_class(index: usize, swatch: usize) -> String {
    format!("term-scheme-{index}-{swatch}")
}

/// The colours of the tiles, which CSS gives each widget: one rule for a tile and one for each swatch.
fn scheme_css() -> String {
    theme::all_schemes()
        .into_iter()
        .enumerate()
        .flat_map(|(index, scheme)| {
            let tile = format!(
                ".{} {{ background: {}; color: {}; }}\n",
                scheme_class(index),
                scheme.background,
                scheme.foreground
            );
            let swatches = SWATCHES.map(move |swatch| {
                format!(
                    ".{} {{ background: {}; }}\n",
                    swatch_class(index, swatch),
                    scheme.palette[swatch]
                )
            });
            std::iter::once(tile).chain(swatches)
        })
        .collect()
}

// --- the miniatures: a window in a few rectangles, as Nekominal draws them ------------------------------------------

/// The colours a miniature is drawn in: the text's at a few strengths, the accent and three states'.
struct Colours {
    line: gdk::RGBA,
    faint: gdk::RGBA,
    strong: gdk::RGBA,
    accent: gdk::RGBA,
    wait: gdk::RGBA,
    work: gdk::RGBA,
    done: gdk::RGBA,
}

impl Colours {
    fn of(widget: &impl IsA<gtk::Widget>) -> Self {
        let text = widget.color();
        let with_alpha = |alpha: f32| gdk::RGBA::new(text.red(), text.green(), text.blue(), alpha);
        let state = |status: Status| gdk::RGBA::parse(status.colour()).unwrap_or(text);
        #[allow(deprecated)]
        let accent = widget
            .style_context()
            .lookup_color("accent_color")
            .unwrap_or_else(|| state(Status::Working));
        Self {
            line: with_alpha(0.16),
            faint: with_alpha(0.12),
            strong: with_alpha(0.43),
            accent,
            wait: state(Status::Waiting),
            work: state(Status::Working),
            done: state(Status::Done),
        }
    }
}

fn tinted(colour: gdk::RGBA, alpha: f32) -> gdk::RGBA {
    gdk::RGBA::new(colour.red(), colour.green(), colour.blue(), alpha)
}

/// A rounded rectangle filled with `fill`, and outlined with `edge` when given.
fn round_rect(
    cairo: &gtk::cairo::Context,
    (x, y, width, height): (f64, f64, f64, f64),
    radius: f64,
    fill: gdk::RGBA,
    edge: Option<gdk::RGBA>,
) -> Result<(), gtk::cairo::Error> {
    let radius = radius.min(width / 2.0).min(height / 2.0).max(0.0);
    cairo.new_sub_path();
    cairo.arc(x + width - radius, y + radius, radius, -PI / 2.0, 0.0);
    cairo.arc(x + width - radius, y + height - radius, radius, 0.0, PI / 2.0);
    cairo.arc(x + radius, y + height - radius, radius, PI / 2.0, PI);
    cairo.arc(x + radius, y + radius, radius, PI, 1.5 * PI);
    cairo.close_path();
    cairo.set_source_color(&fill);
    match edge {
        Some(edge) => {
            cairo.fill_preserve()?;
            cairo.set_source_color(&edge);
            cairo.set_line_width(1.0);
            cairo.stroke()
        }
        None => cairo.fill(),
    }
}

fn fill_rect(cairo: &gtk::cairo::Context, (x, y, width, height): (f64, f64, f64, f64), colour: gdk::RGBA) -> Result<(), gtk::cairo::Error> {
    cairo.rectangle(x, y, width, height);
    cairo.set_source_color(&colour);
    cairo.fill()
}

/// The frame every miniature has: the window, its bar, and the waiting count on the bar.
fn draw_frame(cairo: &gtk::cairo::Context, colours: &Colours, width: f64) -> Result<(), gtk::cairo::Error> {
    fill_rect(cairo, (0.0, 14.0, width, 1.0), colours.line)?;
    round_rect(cairo, (width - 38.0, 4.0, 30.0, 6.0), 3.0, tinted(colours.wait, 0.5), None)
}

fn draw_layout(cairo: &gtk::cairo::Context, colours: &Colours, width: i32, height: i32, layout: TabLayout) -> Result<(), gtk::cairo::Error> {
    let (width, height) = (f64::from(width), f64::from(height));
    draw_frame(cairo, colours, width)?;
    let states = [colours.wait, colours.work, colours.done];
    let side = match layout {
        TabLayout::Top => {
            for n in 0..3 {
                let colour = if n == 0 { colours.accent } else { colours.faint };
                round_rect(cairo, (8.0 + f64::from(n) * 33.0, 5.0, 26.0, 4.0), 2.0, colour, None)?;
            }
            0.0
        }
        TabLayout::List => {
            round_rect(cairo, (8.0, 5.0, 10.0, 4.0), 2.0, colours.faint, None)?;
            for n in 0..4 {
                let y = 23.0 + f64::from(n) * 18.0;
                round_rect(cairo, (6.0, y, 84.0, 12.0), 3.0, colours.faint, None)?;
                let dot = states.get(n as usize).copied().unwrap_or(colours.line);
                round_rect(cairo, (10.0, y + 4.0, 4.0, 4.0), 2.0, dot, None)?;
            }
            96.0
        }
        TabLayout::Icons => {
            round_rect(cairo, (8.0, 5.0, 10.0, 4.0), 2.0, colours.faint, None)?;
            for n in 0..4 {
                let ring = states.get(n as usize).copied();
                round_rect(cairo, (8.0, 23.0 + f64::from(n) * 20.0, 12.0, 12.0), 3.0, colours.faint, ring)?;
            }
            28.0
        }
    };
    if side > 0.0 {
        fill_rect(cairo, (side, 15.0, 1.0, height - 15.0), colours.line)?;
    }
    for (n, share) in [0.7, 0.5, 0.8, 0.4].into_iter().enumerate() {
        let line = (side + 10.0, 25.0 + n as f64 * 10.0, (width - side - 20.0) * share, 4.0);
        round_rect(cairo, line, 2.0, colours.faint, None)?;
    }
    Ok(())
}

fn draw_style(cairo: &gtk::cairo::Context, colours: &Colours, width: i32, height: i32, style: TabStyle) -> Result<(), gtk::cairo::Error> {
    let (width, height) = (f64::from(width), f64::from(height));
    draw_frame(cairo, colours, width)?;
    let side = (width * 0.46).round();
    fill_rect(cairo, (side, 15.0, 1.0, height - 15.0), colours.line)?;
    for (n, share) in [0.8, 0.55, 0.7, 0.4].into_iter().enumerate() {
        let line = (side + 9.0, 25.0 + n as f64 * 10.0, (width - side - 18.0) * share, 4.0);
        round_rect(cairo, line, 2.0, colours.faint, None)?;
    }
    let (x, w) = (6.0, side - 12.0);
    let dot = |x: f64, y: f64, colour: gdk::RGBA| round_rect(cairo, (x, y, 5.0, 5.0), 2.5, colour, None);
    match style {
        TabStyle::Activity => {
            let rows = [(colours.wait, 0.62), (colours.work, 0.41), (colours.work, 0.18), (colours.done, 0.0)];
            for (n, (colour, share)) in rows.into_iter().enumerate() {
                let y = 21.0 + n as f64 * 22.0;
                if n == 0 {
                    round_rect(cairo, (x, y - 3.0, w, 21.0), 3.0, colours.faint, None)?;
                }
                dot(x + 4.0, y, colour)?;
                round_rect(cairo, (x + 13.0, y, w * 0.55, 4.0), 2.0, colours.strong, None)?;
                let second = if n == 0 { tinted(colours.wait, 0.55) } else { colours.faint };
                round_rect(cairo, (x + 13.0, y + 7.0, w * 0.75, 3.0), 1.5, second, None)?;
                if share > 0.0 {
                    round_rect(cairo, (x + 13.0, y + 13.0, w - 17.0, 2.0), 1.0, colours.faint, None)?;
                    round_rect(cairo, (x + 13.0, y + 13.0, (w - 17.0) * share, 2.0), 1.0, colours.accent, None)?;
                }
            }
        }
        TabStyle::Cards => {
            for (n, colour) in [colours.wait, colours.work].into_iter().enumerate() {
                let (cx, cy, cw) = (x, 20.0 + n as f64 * 40.0, w);
                let edge = if n == 0 { colours.accent } else { colours.line };
                round_rect(cairo, (cx, cy, cw, 35.0), 4.0, colours.faint, Some(edge))?;
                dot(cx + 5.0, cy + 5.0, colour)?;
                round_rect(cairo, (cx + 14.0, cy + 5.0, cw * 0.45, 4.0), 2.0, colours.strong, None)?;
                round_rect(cairo, (cx + cw - 22.0, cy + 4.5, 16.0, 6.0), 3.0, tinted(colour, 0.43), None)?;
                round_rect(cairo, (cx + 5.0, cy + 13.0, cw - 10.0, 10.0), 2.0, gdk::RGBA::new(0.0, 0.0, 0.0, 0.27), None)?;
                round_rect(cairo, (cx + 5.0, cy + 28.0, cw * 0.5, 3.0), 1.5, colours.faint, None)?;
            }
        }
        TabStyle::Repo => {
            let mut y = 21.0;
            let repositories: [&[(gdk::RGBA, bool)]; 3] = [
                &[(colours.wait, false)],
                &[(colours.work, false), (colours.work, true)],
                &[(colours.done, false)],
            ];
            for (n, tabs) in repositories.into_iter().enumerate() {
                round_rect(cairo, (x + 2.0, y, w * 0.38, 3.0), 1.5, colours.strong, None)?;
                y += 7.0;
                for (k, (colour, tree)) in tabs.iter().enumerate() {
                    let indent = if *tree { 7.0 } else { 0.0 };
                    if n == 0 && k == 0 {
                        round_rect(cairo, (x, y - 2.0, w, 10.0), 2.0, colours.faint, None)?;
                    }
                    dot(x + 3.0 + indent, y + 0.5, *colour)?;
                    round_rect(cairo, (x + 12.0 + indent, y + 1.0, w - 30.0 - indent, 3.0), 1.5, colours.strong, None)?;
                    y += 11.0;
                }
                y += 3.0;
            }
        }
    }
    Ok(())
}
