//! A terminal window laid out as Nekominal's: a title bar across it with the tabs on it or the
//! name of the tab in front, a sidebar listing the tabs in the layout and style the settings
//! choose, and a find bar. Every window and tab is
//! kept here, so that the count, the next waiting tab and a notification's tab are found across
//! windows, and a tab dragged out keeps its panes.

use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    rc::Rc,
};

use adw::prelude::*;
use gtk::{gio, glib};
use vte4::prelude::*;

use super::{
    lang::{self, tr, trf},
    agent::Status,
    config,
    page::{colour_named, Page, TAB_COLOURS},
    palette::Palette,
    pane::Pane,
    profiles,
    push::{self, Happening},
    scheme_import,
    restore::{self, SavedTab, SavedWindow},
    settings,
    sidebar::{self, Entry},
    subagent_drawer::Drawer,
    subagents_card::{self, Target},
    theme,
    titlebar::TitleBar,
};

const DEFAULT_WIDTH: i32 = 1180;
const DEFAULT_HEIGHT: i32 = 760;
const FIND_PLACEHOLDER: &str = "尋找";
/// The classes of the sidebar's list, one for each way of listing the tabs.
const LIST_CLASSES: [&str; 4] = ["term-list", "term-cards", "term-repos", "term-icons"];
const NO_COLOUR: &str = "none";
const ZOOM_STEP: f64 = 0.1;
const ZOOM_MIN: f64 = 0.5;
const ZOOM_MAX: f64 = 3.0;
/// PCRE2 flags for finding: any case, and `^` and `$` at each line.
const PCRE2_CASELESS: u32 = 0x0000_0008;
const PCRE2_MULTILINE: u32 = 0x0000_0400;

#[path = "window_palette.rs"]
mod palette_items;
#[path = "window_worktree.rs"]
mod worktree_flows;
#[path = "window_api.rs"]
mod api_answers;
#[path = "window_drop_down.rs"]
mod drop_down;
#[path = "window_carry.rs"]
mod carry_flows;

pub use api_answers::{answer as answer_api, publish_agent_states};
pub use carry_flows::{open_carried, update_and_restart};
pub use drop_down::toggle_drop_down;

thread_local! {
    static WINDOWS: RefCell<Vec<Rc<Window>>> = RefCell::default();
    static PAGES: RefCell<Vec<Rc<Page>>> = RefCell::default();
    static REFRESH_QUEUED: Cell<bool> = const { Cell::new(false) };
}

/// A tab and its page.
type Tab = (adw::TabPage, Rc<Page>);
/// A tab that waits, with the window it is in and its page.
type WaitingTab = (Rc<Window>, adw::TabPage, Rc<Page>);

pub struct Window {
    pub window: adw::ApplicationWindow,
    pub tab_view: adw::TabView,
    split_view: adw::OverlaySplitView,
    sessions: gtk::ListBox,
    rows: RefCell<Vec<(gtk::ListBoxRow, adw::TabPage)>>,
    titlebar: TitleBar,
    search_bar: gtk::SearchBar,
    search: gtk::SearchEntry,
    /// The tab whose menu is open, which the menu's commands act on.
    menu_tab: RefCell<Option<adw::TabPage>>,
    /// Set once the user agreed to end what runs in the window.
    closing: Cell<bool>,
    /// Set while tabs close without asking, as a worktree's do once its removal was agreed to.
    quiet_close: Cell<bool>,
    /// The command palette, over the top of the tabs.
    palette: Rc<Palette>,
    toasts: adw::ToastOverlay,
    /// What a subagent or workflow did, at the right over the panes.
    drawer: Rc<Drawer>,
    /// The drop-down window, which Super+` shows and hides.
    drop_down: Cell<bool>,
}

impl Window {
    pub fn new(app: &adw::Application) -> Rc<Self> {
        let tab_view = adw::TabView::new();
        tab_view.set_menu_model(Some(&tab_menu()));

        let (sidebar, sessions) = sidebar();
        let titlebar = TitleBar::new(&tab_view);
        let search = gtk::SearchEntry::builder()
            .placeholder_text(tr(FIND_PLACEHOLDER))
            .hexpand(true)
            .build();
        let search_bar = gtk::SearchBar::builder()
            .child(&search)
            .show_close_button(true)
            .build();
        search_bar.connect_entry(&search);

        let split_view = adw::OverlaySplitView::builder()
            .sidebar(&sidebar)
            .content(&tab_view)
            .build();
        let palette = Palette::new();
        let overlay = gtk::Overlay::builder().child(&split_view).build();
        let drawer = Drawer::new();
        overlay.add_overlay(&drawer.widget);
        overlay.add_overlay(&palette.widget);
        let content = adw::ToolbarView::new();
        content.add_top_bar(&titlebar.widget);
        content.add_top_bar(&search_bar);
        content.set_content(Some(&overlay));
        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&content));

        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title(tr("終端機"))
            .default_width(DEFAULT_WIDTH)
            .default_height(DEFAULT_HEIGHT)
            .content(&toasts)
            .build();
        // Added, not set: setting the classes would drop GTK's own `csd`, and with it the window's corners and shadow.
        window.add_css_class(theme::WINDOW_CLASS);

        let this = Rc::new(Self {
            window,
            tab_view,
            split_view,
            sessions,
            rows: RefCell::default(),
            titlebar,
            search_bar,
            search,
            menu_tab: RefCell::default(),
            closing: Cell::new(false),
            quiet_close: Cell::new(false),
            palette,
            toasts,
            drawer,
            drop_down: Cell::new(false),
        });
        this.connect_tabs();
        this.connect_sidebar();
        this.connect_search();
        this.connect_window();
        this.add_actions();
        WINDOWS.with_borrow_mut(|windows| windows.push(this.clone()));
        this.refresh();
        this
    }

    /// Opens a tab of `profile`, or the default one, after `after` when given, else at the end.
    pub fn open_tab(
        &self,
        directory: Option<PathBuf>,
        profile: Option<&str>,
        after: Option<&adw::TabPage>,
    ) -> Option<Rc<Page>> {
        match Page::new(directory.as_deref(), profile, &[]) {
            Ok(page) => Some(self.add_page(page, after)),
            Err(error) => {
                eprintln!("nekoland-terminal: could not start a shell: {error}");
                None
            }
        }
    }

    /// Puts a tab in the window, after `after` when given, else at the end, and brings it up.
    fn add_page(&self, page: Rc<Page>, after: Option<&adw::TabPage>) -> Rc<Page> {
        let tab = match after {
            Some(after) => self
                .tab_view
                .insert(&page.root, self.tab_view.page_position(after) + 1),
            None => self.tab_view.append(&page.root),
        };
        page.tab.replace(Some(tab.clone()));
        page.refresh_tab();
        PAGES.with_borrow_mut(|pages| pages.push(page.clone()));
        self.tab_view.set_selected_page(&tab);
        focus_pane(&page);
        refresh_soon();
        page
    }

    /// Opens the tabs saved of a window as they were.
    fn open_saved(&self, tabs: &[SavedTab]) {
        for saved in tabs {
            let Some(page) = self.open_tab(saved.directory.clone(), saved.profile.as_deref(), None) else {
                continue;
            };
            page.custom_title.replace(saved.title.clone());
            page.colour.set(saved.colour.as_deref().and_then(colour_named));
            page.refresh_tab();
        }
    }

    /// What of the window is kept for the next start.
    fn saved(&self) -> SavedWindow {
        self.pages()
            .iter()
            .map(|(_, page)| SavedTab {
                directory: page.focused_pane().and_then(|pane| pane.directory()),
                title: page.custom_title.borrow().clone(),
                colour: page.colour.get().map(str::to_string),
                profile: page.profile(),
            })
            .collect()
    }

    fn connect_tabs(self: &Rc<Self>) {
        let view = &self.tab_view;
        view.connect_close_page(glib::clone!(
            #[weak(rename_to = this)]
            self,
            #[upgrade_or]
            glib::Propagation::Stop,
            move |view, tab| {
                this.close_tab(view, tab);
                glib::Propagation::Stop
            }
        ));
        view.connect_setup_menu(glib::clone!(
            #[weak(rename_to = this)]
            self,
            move |_, tab| match tab {
                Some(tab) => {
                    this.menu_tab.replace(Some(tab.clone()));
                }
                // The menu closes before its command runs; forget the tab after.
                None => {
                    glib::idle_add_local_once(glib::clone!(
                        #[weak]
                        this,
                        move || {
                            this.menu_tab.take();
                        }
                    ));
                }
            }
        ));
        view.connect_selected_page_notify(|view| {
            if let Some(page) = view.selected_page().as_ref().and_then(page_of) {
                page.acknowledge();
                focus_pane(&page);
            }
            refresh_soon();
        });
        view.connect_page_attached(|_, _, _| refresh_soon());
        view.connect_page_detached(|_, _, _| refresh_soon());
        view.connect_n_pages_notify(glib::clone!(
            #[weak(rename_to = this)]
            self,
            move |view| {
                if view.n_pages() == 0 {
                    this.window.close();
                }
            }
        ));
        let app = self.window.application();
        view.connect_create_window(move |_| {
            let app = app.as_ref()?.downcast_ref::<adw::Application>()?;
            let window = Window::new(app);
            window.window.present();
            Some(window.tab_view.clone())
        });
    }

    fn close_tab(&self, view: &adw::TabView, tab: &adw::TabPage) {
        let Some(page) = page_of(tab) else {
            view.close_page_finish(tab, true);
            refresh_soon();
            return;
        };
        let programs = page.running_programs();
        if self.quiet_close.get() || !needs_confirmation(&programs) {
            finish_closing(view, tab, &page, true);
            return;
        }
        let view = view.clone();
        let tab = tab.clone();
        confirm_close(&self.window, tr("關閉這個分頁？"), &programs, move |confirmed| {
            finish_closing(&view, &tab, &page, confirmed);
        });
    }

    fn connect_sidebar(self: &Rc<Self>) {
        self.sessions.connect_row_activated(glib::clone!(
            #[weak(rename_to = this)]
            self,
            move |_, row| {
                let tab = this
                    .rows
                    .borrow()
                    .iter()
                    .find(|(listed, _)| listed == row)
                    .map(|(_, tab)| tab.clone());
                if let Some(tab) = tab {
                    this.tab_view.set_selected_page(&tab);
                }
            }
        ));
    }

    fn connect_search(self: &Rc<Self>) {
        self.search.connect_search_changed(glib::clone!(
            #[weak(rename_to = this)]
            self,
            move |entry| this.find(&entry.text())
        ));
        let find_next = glib::clone!(
            #[weak(rename_to = this)]
            self,
            move |_: &gtk::SearchEntry| {
                if let Some(pane) = this.focused_pane() {
                    pane.screen.search_find_next();
                }
            }
        );
        self.search.connect_activate(find_next.clone());
        self.search.connect_next_match(find_next);
        self.search.connect_previous_match(glib::clone!(
            #[weak(rename_to = this)]
            self,
            move |_| {
                if let Some(pane) = this.focused_pane() {
                    pane.screen.search_find_previous();
                }
            }
        ));
        self.search.connect_stop_search(glib::clone!(
            #[weak(rename_to = this)]
            self,
            move |_| {
                this.search_bar.set_search_mode(false);
                if let Some(pane) = this.focused_pane() {
                    pane.screen.grab_focus();
                }
            }
        ));
    }

    fn find(&self, text: &str) {
        let Some(pane) = self.focused_pane() else {
            return;
        };
        if text.is_empty() {
            pane.screen.search_set_regex(None, 0);
            return;
        }
        let pattern = glib::Regex::escape_string(text);
        match vte4::Regex::for_search(&pattern, PCRE2_CASELESS | PCRE2_MULTILINE) {
            Ok(regex) => {
                pane.screen.search_set_regex(Some(&regex), 0);
                pane.screen.search_set_wrap_around(true);
                pane.screen.search_find_previous();
            }
            Err(error) => eprintln!("nekoland-terminal: could not search for {text:?}: {error}"),
        }
    }

    fn connect_window(self: &Rc<Self>) {
        self.window.connect_is_active_notify(glib::clone!(
            #[weak(rename_to = this)]
            self,
            move |window| {
                if window.is_active() {
                    if let Some(page) = this.selected_page() {
                        page.acknowledge();
                    }
                    refresh_soon();
                }
            }
        ));
        self.window.connect_close_request(glib::clone!(
            #[weak(rename_to = this)]
            self,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |_| this.close_request()
        ));
    }

    fn close_request(self: &Rc<Self>) -> glib::Propagation {
        let pages = self.pages();
        let programs: Vec<String> = pages
            .iter()
            .flat_map(|(_, page)| page.running_programs())
            .collect();
        if needs_confirmation(&programs) && !self.closing.get() {
            confirm_close(
                &self.window,
                tr("關閉視窗？"),
                &programs,
                glib::clone!(
                    #[weak(rename_to = this)]
                    self,
                    move |confirmed| {
                        if confirmed {
                            this.closing.set(true);
                            this.window.close();
                        }
                    }
                ),
            );
            return glib::Propagation::Stop;
        }
        PAGES.with_borrow_mut(|all| {
            all.retain(|page| !pages.iter().any(|(_, closed)| Rc::ptr_eq(page, closed)));
        });
        WINDOWS.with_borrow_mut(|windows| windows.retain(|window| !Rc::ptr_eq(window, self)));
        refresh_soon();
        glib::Propagation::Proceed
    }

    fn add_actions(self: &Rc<Self>) {
        self.add_action("new-tab", |this| {
            let directory = this.focused_pane().and_then(|pane| pane.directory());
            this.open_tab(directory, None, None);
        });
        self.add_action("new-window", |this| {
            let Some(app) = this.application() else {
                return;
            };
            let window = Window::new(&app);
            window.open_tab(None, None, None);
            window.window.present();
        });
        self.add_action("close-tab", |this| {
            if let Some(tab) = this.target_tab() {
                this.tab_view.close_page(&tab);
            }
        });
        self.add_action("close-right", |this| {
            if let Some(tab) = this.target_tab() {
                this.tab_view.close_pages_after(&tab);
            }
        });
        self.add_action("rename-tab", |this| this.rename_tab());
        self.add_action("duplicate-tab", |this| {
            let Some(tab) = this.target_tab() else {
                return;
            };
            let page = page_of(&tab);
            let directory = page
                .as_ref()
                .and_then(|page| page.focused_pane())
                .and_then(|pane| pane.directory());
            let profile = page.and_then(|page| page.profile());
            this.open_tab(directory, profile.as_deref(), Some(&tab));
        });
        self.add_action("move-to-window", |this| {
            let (Some(tab), Some(app)) = (this.target_tab(), this.application()) else {
                return;
            };
            let window = Window::new(&app);
            this.tab_view.transfer_page(&tab, &window.tab_view, 0);
            window.window.present();
        });
        self.add_action("split-right", |this| this.split(gtk::Orientation::Horizontal));
        self.add_action("split-down", |this| this.split(gtk::Orientation::Vertical));
        self.add_action("copy", |this| {
            // With nothing selected VTE would empty the clipboard.
            if let Some(pane) = this.focused_pane().filter(|pane| pane.screen.has_selection()) {
                pane.screen.copy_clipboard_format(vte4::Format::Text);
            }
        });
        self.add_action("paste", |this| {
            if let Some(pane) = this.focused_pane() {
                pane.paste();
            }
        });
        self.add_action("find", |this| {
            this.search_bar.set_search_mode(true);
            this.search.grab_focus();
        });
        // Whether the sidebar shows is a setting: every window follows it.
        self.add_action("toggle-sidebar", |_| {
            config::update(|config| config.sidebar_open = !config.sidebar_open);
            apply_config();
        });
        self.add_action("zoom-in", |this| this.zoom(Some(ZOOM_STEP)));
        self.add_action("zoom-out", |this| this.zoom(Some(-ZOOM_STEP)));
        self.add_action("zoom-reset", |this| this.zoom(None));
        self.add_action("palette", |this| {
            if this.palette.is_open() {
                this.palette.close();
            } else {
                this.open_palette();
            }
        });
        self.add_action("search-tabs", |this| this.open_tab_search());
        self.add_action("new-worktree", |this| this.ask_worktree_branch());
        self.add_action("remove-worktree", |this| this.ask_remove_worktree());
        self.add_action("import-schemes", |this| this.import_schemes(scheme_import::Source::WindowsTerminal));
        self.add_action("import-iterm-colors", |this| this.import_schemes(scheme_import::Source::Iterm));
        self.add_action("hints", |this| {
            if let Some(pane) = this.focused_pane() {
                pane.toggle_hints();
            }
        });
        self.add_action("save-layout", |this| this.ask_layout_name(None));
        self.add_action("refresh-drawer", |this| this.refresh_drawer());
        self.add_subagent_actions();

        let colour = gio::SimpleAction::new("tab-colour", Some(glib::VariantTy::STRING));
        colour.connect_activate(glib::clone!(
            #[weak(rename_to = this)]
            self,
            move |_, parameter| {
                let name = parameter.and_then(|parameter| parameter.str());
                let Some(page) = this.target_tab().as_ref().and_then(page_of) else {
                    return;
                };
                page.colour.set(name.and_then(colour_named));
                page.changed();
            }
        ));
        self.window.add_action(&colour);

        let profile = gio::SimpleAction::new("new-tab-profile", Some(glib::VariantTy::STRING));
        profile.connect_activate(glib::clone!(
            #[weak(rename_to = this)]
            self,
            move |_, parameter| {
                let name = parameter.and_then(|parameter| parameter.str());
                let directory = this.focused_pane().and_then(|pane| pane.directory());
                this.open_tab(directory, name, None);
            }
        ));
        self.window.add_action(&profile);
    }

    /// Opening the drawer at a row of the subagents card, and Esc taking it a level back.
    fn add_subagent_actions(self: &Rc<Self>) {
        let open = gio::SimpleAction::new(subagents_card::OPEN_ACTION.trim_start_matches("win."), Some(glib::VariantTy::STRING));
        open.connect_activate(glib::clone!(
            #[weak(rename_to = this)]
            self,
            move |_, parameter| {
                let Some(target) = parameter.and_then(|parameter| parameter.str()).and_then(Target::decode) else {
                    return;
                };
                this.titlebar.close_subagents();
                this.drawer.open(target);
                this.refresh_drawer();
            }
        ));
        self.window.add_action(&open);
        let keys = gtk::EventControllerKey::builder().propagation_phase(gtk::PropagationPhase::Capture).build();
        let drawer = Rc::downgrade(&self.drawer);
        keys.connect_key_pressed(move |_, key, _, _| match drawer.upgrade() {
            Some(drawer) if key == gtk::gdk::Key::Escape && drawer.is_open() => {
                drawer.back();
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        });
        self.window.add_controller(keys);
    }

    /// Lays the drawer out again for the session in front.
    fn refresh_drawer(&self) {
        let page = self.selected_page();
        let subagents = page.as_ref().and_then(|page| page.subagents()).unwrap_or_default();
        let folder = page.as_ref().and_then(|page| page.subagents_folder());
        self.drawer.show(&subagents, folder.as_deref());
    }

    fn add_action(self: &Rc<Self>, name: &str, activate: impl Fn(&Rc<Self>) + 'static) {
        let action = gio::SimpleAction::new(name, None);
        action.connect_activate(glib::clone!(
            #[weak(rename_to = this)]
            self,
            move |_, _| activate(&this)
        ));
        self.window.add_action(&action);
    }

    /// Asks for a file of `source` and brings its schemes in, saying in a toast what came.
    fn import_schemes(self: &Rc<Self>, source: scheme_import::Source) {
        let filter = gtk::FileFilter::new();
        filter.add_pattern(source.pattern());
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        let dialog = gtk::FileDialog::builder().filters(&filters).modal(true).build();
        dialog.open(
            Some(&self.window),
            gio::Cancellable::NONE,
            glib::clone!(
                #[weak(rename_to = this)]
                self,
                move |result| {
                    let path = match result {
                        Ok(file) => file.path(),
                        Err(error) if error.matches(gtk::DialogError::Dismissed) => return,
                        Err(error) => {
                            this.toasts.add_toast(adw::Toast::new(&trf("無法匯入：{0}", &[&error])));
                            return;
                        }
                    };
                    let outcome = path
                        .ok_or_else(|| tr("沒有新的色彩配置可以匯入").to_string())
                        .and_then(|path| scheme_import::import_file(&path, source));
                    this.toasts.add_toast(adw::Toast::new(&import_message(outcome)));
                    settings::reload_scheme_css();
                    apply_config();
                    refresh_settings();
                }
            ),
        );
    }

    fn application(&self) -> Option<adw::Application> {
        self.window.application().and_downcast()
    }

    fn target_tab(&self) -> Option<adw::TabPage> {
        self.menu_tab
            .borrow()
            .clone()
            .or_else(|| self.tab_view.selected_page())
    }

    fn selected_page(&self) -> Option<Rc<Page>> {
        self.tab_view.selected_page().as_ref().and_then(page_of)
    }

    fn focused_pane(&self) -> Option<Rc<Pane>> {
        self.selected_page()?.focused_pane()
    }

    /// Brings up `tab` with the find bar open on `query`, for a line a search of every tab found there.
    fn find_in_tab(&self, tab: &adw::TabPage, query: &str) {
        if !self.pages().iter().any(|(listed, _)| listed == tab) {
            return;
        }
        self.tab_view.set_selected_page(tab);
        self.window.present();
        self.search_bar.set_search_mode(true);
        self.search.set_text(query);
        self.find(query);
        self.search.grab_focus();
    }

    fn split(&self, orientation: gtk::Orientation) {
        let Some(page) = self.target_tab().as_ref().and_then(page_of) else {
            return;
        };
        if let Err(error) = page.split(orientation) {
            eprintln!("nekoland-terminal: could not start a shell: {error}");
        }
        page.changed();
    }

    fn zoom(&self, step: Option<f64>) {
        let Some(pane) = self.focused_pane() else {
            return;
        };
        let scale = step.map_or(1.0, |step| {
            (pane.screen.font_scale() + step).clamp(ZOOM_MIN, ZOOM_MAX)
        });
        pane.screen.set_font_scale(scale);
    }

    fn rename_tab(&self) {
        let Some(page) = self.target_tab().as_ref().and_then(page_of) else {
            return;
        };
        let entry = gtk::Entry::builder()
            .text(page.title())
            .activates_default(true)
            .build();
        let dialog = adw::AlertDialog::new(Some(tr("重新命名分頁")), None);
        dialog.set_extra_child(Some(&entry));
        dialog.add_responses(&[("cancel", tr("取消")), ("rename", tr("重新命名"))]);
        dialog.set_response_appearance("rename", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("rename"));
        dialog.set_close_response("cancel");
        dialog.connect_response(Some("rename"), move |_, _| {
            let title = entry.text().trim().to_string();
            // An empty name gives the tab back the name of what runs in it.
            page.custom_title
                .replace((!title.is_empty()).then_some(title));
            page.changed();
        });
        dialog.present(Some(&self.window));
    }

    fn relabel(&self) {
        self.titlebar.relabel();
        self.search.set_placeholder_text(Some(tr(FIND_PLACEHOLDER)));
        self.tab_view.set_menu_model(Some(&tab_menu()));
    }

    /// The tabs in order, with their pages.
    fn pages(&self) -> Vec<Tab> {
        (0..self.tab_view.n_pages())
            .map(|index| self.tab_view.nth_page(index))
            .filter_map(|tab| page_of(&tab).map(|page| (tab, page)))
            .collect()
    }

    fn contains(&self, page: &Page) -> bool {
        page.tab
            .borrow()
            .as_ref()
            .is_some_and(|tab| self.pages().iter().any(|(listed, _)| listed == tab))
    }

    /// Every tab in order, with its page.
    fn entries(&self) -> Vec<Entry> {
        self.pages().into_iter().map(|(tab, page)| Entry { tab, page }).collect()
    }

    fn refresh(&self) {
        let config = config::current();
        let layout = config.tab_layout;
        self.titlebar.set_layout(layout);
        let width = sidebar::width(layout, config.tab_style);
        self.split_view.set_min_sidebar_width(width);
        self.split_view.set_max_sidebar_width(width);
        self.split_view
            .set_show_sidebar(layout.is_side() && config.sidebar_open);
        for other in LIST_CLASSES {
            self.sessions.remove_css_class(other);
        }
        self.sessions
            .add_css_class(sidebar::list_class(layout, config.tab_style));
        let entries = self.entries();
        let selected = self.tab_view.selected_page();
        self.refresh_sidebar(&entries, selected.as_ref());
        if self.drawer.is_open() {
            self.refresh_drawer();
        }
        // The tab in front is not counted: the user is already there.
        let others_waiting = waiting_tabs()
            .iter()
            .filter(|(_, tab, _)| Some(tab) != selected.as_ref())
            .count();
        self.titlebar.show_waiting(others_waiting);
        let front = selected
            .as_ref()
            .and_then(|tab| entries.iter().find(|entry| entry.tab == *tab));
        let title = front.map(Entry::title).unwrap_or_default();
        self.titlebar
            .show_current(front.map(|entry| entry.page.as_ref()), &title);
        if front.is_some() {
            self.window.set_title(Some(&title));
        }
    }

    fn refresh_sidebar(&self, entries: &[Entry], selected: Option<&adw::TabPage>) {
        let config = config::current();
        let view = self.tab_view.clone();
        let rows = sidebar::fill(
            &self.sessions,
            config.tab_layout,
            config.tab_style,
            config.group_by_state,
            entries,
            selected,
            move |tab| view.close_page(tab),
        );
        self.rows.replace(rows);
    }
}

fn sidebar() -> (gtk::ScrolledWindow, gtk::ListBox) {
    let sessions = gtk::ListBox::builder()
        .css_classes(["navigation-sidebar"])
        .selection_mode(gtk::SelectionMode::Single)
        .build();
    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&sessions)
        .vexpand(true)
        .build();
    (scroller, sessions)
}

fn tab_menu() -> gio::Menu {
    let name = gio::Menu::new();
    name.append(Some(tr("重新命名…")), Some("win.rename-tab"));
    let colours = gio::Menu::new();
    let none = gio::MenuItem::new(Some(tr("無")), None);
    none.set_action_and_target_value(Some("win.tab-colour"), Some(&NO_COLOUR.to_variant()));
    colours.append_item(&none);
    for (id, label, _) in TAB_COLOURS {
        let item = gio::MenuItem::new(Some(tr(label)), None);
        item.set_action_and_target_value(Some("win.tab-colour"), Some(&id.to_variant()));
        colours.append_item(&item);
    }
    name.append_submenu(Some(tr("顏色")), &colours);
    let arrange = gio::Menu::new();
    arrange.append(Some(tr("複製分頁")), Some("win.duplicate-tab"));
    arrange.append(Some(tr("向右分割")), Some("win.split-right"));
    arrange.append(Some(tr("向下分割")), Some("win.split-down"));
    arrange.append(Some(tr("移到新視窗")), Some("win.move-to-window"));
    let close = gio::Menu::new();
    close.append(Some(tr("關閉右側分頁")), Some("win.close-right"));
    close.append(Some(tr("關閉分頁")), Some("win.close-tab"));
    let menu = gio::Menu::new();
    menu.append_section(None, &name);
    menu.append_section(None, &arrange);
    menu.append_section(None, &close);
    menu
}

/// Whether closing ends enough running programs to ask first, as the settings say.
fn needs_confirmation(programs: &[String]) -> bool {
    let config = config::current();
    config.confirm_close && !programs.is_empty() && programs.len() >= config.confirm_close_threshold as usize
}

/// Asks before ending `programs`; `decided` is told the answer.
fn confirm_close(
    window: &adw::ApplicationWindow,
    heading: &str,
    programs: &[String],
    decided: impl Fn(bool) + 'static,
) {
    let named: Vec<String> = programs.iter().map(|program| trf("「{0}」", &[program])).collect();
    let body = trf("{0}還在執行，關閉後會結束。", &[&named.join(tr("、"))]);
    let dialog = adw::AlertDialog::new(Some(heading), Some(&body));
    dialog.add_responses(&[("cancel", tr("取消")), ("close", tr("關閉"))]);
    dialog.set_response_appearance("close", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    dialog.connect_response(None, move |_, response| decided(response == "close"));
    dialog.present(Some(window));
}

fn finish_closing(view: &adw::TabView, tab: &adw::TabPage, page: &Rc<Page>, confirmed: bool) {
    view.close_page_finish(tab, confirmed);
    if confirmed {
        PAGES.with_borrow_mut(|pages| pages.retain(|kept| !Rc::ptr_eq(kept, page)));
        refresh_soon();
    }
}

fn focus_pane(page: &Page) {
    if let Some(pane) = page.focused_pane() {
        pane.screen.grab_focus();
    }
}

fn page_of(tab: &adw::TabPage) -> Option<Rc<Page>> {
    let child = tab.child();
    PAGES.with_borrow(|pages| pages.iter().find(|page| page.root == child).cloned())
}

fn window_of(page: &Page) -> Option<Rc<Window>> {
    WINDOWS.with_borrow(|windows| windows.iter().find(|window| window.contains(page)).cloned())
}

fn all_windows() -> Vec<Rc<Window>> {
    WINDOWS.with_borrow(Clone::clone)
}

/// What a toast says of an import: the schemes that came, that none did, or why it failed.
fn import_message(outcome: Result<Vec<String>, String>) -> String {
    match outcome {
        Ok(added) if added.is_empty() => tr("沒有新的色彩配置可以匯入").to_string(),
        Ok(added) => trf("已匯入 {0} 個色彩配置：{1}", &[&added.len(), &added.join("、")]),
        Err(error) => trf("無法匯入：{0}", &[&error]),
    }
}

/// Shows on the settings what changed outside them.
pub fn refresh_settings() {
    if let Some(settings) = settings::open_page() {
        settings.refresh();
    }
}

pub fn all_pages() -> Vec<Rc<Page>> {
    PAGES.with_borrow(Clone::clone)
}

/// Whether the user is looking at the tab: it is selected in the window in front.
pub fn is_watched(page: &Page) -> bool {
    window_of(page).is_some_and(|window| {
        window.window.is_active() && window.tab_view.selected_page() == *page.tab.borrow()
    })
}

/// Every waiting tab, window by window, in tab order.
fn waiting_tabs() -> Vec<WaitingTab> {
    let windows = WINDOWS.with_borrow(Clone::clone);
    windows
        .iter()
        .flat_map(|window| {
            window
                .pages()
                .into_iter()
                .filter(|(_, page)| page.status().is_urgent())
                .map(|(tab, page)| (window.clone(), tab, page))
        })
        .collect()
}

/// Goes to the waiting tab after the one in front, round to the first; shakes the count
/// when none waits.
pub fn show_next_waiting() {
    let waiting = waiting_tabs();
    let front = WINDOWS.with_borrow(|windows| {
        windows
            .iter()
            .find(|window| window.window.is_active())
            .cloned()
    });
    let current = front.as_ref().and_then(|window| window.tab_view.selected_page());
    let position = waiting
        .iter()
        .position(|(_, tab, _)| Some(tab) == current.as_ref());
    let next = position.map_or(0, |position| position + 1) % waiting.len().max(1);
    match waiting.get(next) {
        Some((window, tab, page)) => {
            window.tab_view.set_selected_page(tab);
            window.window.present();
            focus_waiting_pane(page);
        }
        None => {
            if let Some(window) = front {
                window.titlebar.nudge();
            }
        }
    }
}

fn focus_waiting_pane(page: &Page) {
    let waiting = page
        .panes()
        .into_iter()
        .find(|pane| pane.status() == Status::Waiting);
    match waiting {
        Some(pane) => {
            pane.screen.grab_focus();
        }
        None => focus_pane(page),
    }
}

/// Brings up the tab a notification was about.
pub fn show_page(id: u64) {
    let page = PAGES.with_borrow(|pages| pages.iter().find(|page| page.id == id).cloned());
    let Some(page) = page else {
        return;
    };
    let (Some(window), Some(tab)) = (window_of(&page), page.tab.borrow().clone()) else {
        return;
    };
    window.tab_view.set_selected_page(&tab);
    window.window.present();
    focus_waiting_pane(&page);
}

/// A pane waits, finished or failed while no one looked at its tab.
pub fn notify(page: &Page, pane: &Pane, status: Status) {
    let name = pane.agent_name().unwrap_or_else(|| page.title());
    let (title, happening) = match status {
        Status::Waiting => (trf("{0} 等你", &[&name]), Happening::Waiting),
        Status::Done => (trf("{0} 完成", &[&name]), Happening::Done),
        Status::Failed => (trf("{0} 失敗", &[&name]), Happening::Failed),
        Status::Working | Status::Idle => return,
    };
    let message = pane.message();
    push::notify(&title, message.as_deref().unwrap_or_default(), happening, page.id, terminal_in_front());
    if !config::current().notifications {
        return;
    }
    let Some(app) = gio::Application::default() else {
        return;
    };
    let notification = gio::Notification::new(&title);
    if let Some(message) = message {
        notification.set_body(Some(&message));
    }
    notification.set_default_action_and_target_value("app.show-tab", Some(&page.id.to_variant()));
    app.send_notification(Some(&format!("tab-{}", page.id)), &notification);
}

/// What a pane asked to tell the user: a trigger's text, or a long command's end.
pub fn notify_message(page: &Page, title: &str, body: &str, happening: Happening) {
    push::notify(title, body, happening, page.id, terminal_in_front());
    if !config::current().notifications {
        return;
    }
    let Some(app) = gio::Application::default() else {
        return;
    };
    let notification = gio::Notification::new(title);
    notification.set_body(Some(body));
    notification.set_default_action_and_target_value("app.show-tab", Some(&page.id.to_variant()));
    app.send_notification(None, &notification);
}

/// Whether a window of the terminal is in front.
fn terminal_in_front() -> bool {
    all_windows().iter().any(|window| window.window.is_active())
}

/// A tab whose last shell exited closes.
pub fn close_emptied(page: &Page) {
    let (Some(window), Some(tab)) = (window_of(page), page.tab.borrow().clone()) else {
        return;
    };
    window.tab_view.close_page(&tab);
}

/// Shows changed settings on every screen and in every window.
pub fn apply_config() {
    theme::show_opacity(&config::current());
    if lang::apply(config::current().language) {
        relabel_all();
    }
    let profiles = profiles::all();
    for page in all_pages() {
        for pane in page.panes() {
            theme::set_up(&pane.screen, &profiles::named_in(&profiles, Some(pane.profile())));
        }
        page.refresh_tab();
    }
    if let Some(settings) = settings::open_page() {
        settings.show_applies();
    }
    refresh_soon();
}

/// Names every menu, button and page again in the language now chosen.
fn relabel_all() {
    for page in all_pages() {
        for pane in page.panes() {
            pane.relabel();
        }
    }
    for window in all_windows() {
        window.relabel();
    }
    settings::relabel();
    if let Some(app) = gio::Application::default().and_downcast::<gtk::Application>() {
        app.set_menubar(Some(&crate::menubar()));
    }
}

/// Redraws every sidebar and count once the current events are handled.
pub fn refresh_soon() {
    if REFRESH_QUEUED.replace(true) {
        return;
    }
    glib::idle_add_local_once(|| {
        REFRESH_QUEUED.set(false);
        let windows = WINDOWS.with_borrow(Clone::clone);
        windows.iter().for_each(|window| window.refresh());
        save_session(&windows);
    });
}

/// Keeps the open tabs for the next start; the last window closing leaves them as they were.
fn save_session(windows: &[Rc<Window>]) {
    if windows.is_empty() || !config::current().restore_session {
        return;
    }
    let saved: Vec<SavedWindow> =
        windows.iter().filter(|window| !window.drop_down.get()).map(|window| window.saved()).collect();
    // The drop-down window alone keeps the tabs saved of the others.
    if saved.is_empty() {
        return;
    }
    restore::save(&saved);
}

/// Opens the first window: the tabs of the last run when the settings keep them, else one tab.
pub fn open_first(app: &adw::Application) {
    let saved = if config::current().restore_session {
        restore::load()
    } else {
        Vec::new()
    };
    if saved.is_empty() {
        let window = Window::new(app);
        window.open_tab(None, None, None);
        window.window.present();
        return;
    }
    for tabs in &saved {
        let window = Window::new(app);
        window.open_saved(tabs);
        if window.tab_view.n_pages() == 0 {
            window.open_tab(None, None, None);
        }
        window.window.present();
    }
}

/// The last lines that are not blank of the terminal in front of the window in front, for the form of a trigger to
/// match its pattern against.
pub fn sample_lines(count: usize) -> Vec<String> {
    let Some(window) = front_window() else {
        return Vec::new();
    };
    let front = window.tab_view.selected_page().as_ref().and_then(page_of);
    let Some(pane) = front.or_else(|| window.pages().into_iter().next().map(|(_, page)| page)).and_then(|page| page.focused_pane()) else {
        return Vec::new();
    };
    let text = pane.screen.text_format(vte4::Format::Text).map(|text| text.to_string()).unwrap_or_default();
    let mut lines: Vec<String> = text
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.trim().is_empty())
        .map(str::to_string)
        .collect();
    lines.drain(..lines.len().saturating_sub(count));
    lines
}

/// The window in front, for a command that is not any window's own: the active one, else the last shown.
pub fn front_window() -> Option<Rc<Window>> {
    WINDOWS.with_borrow(|windows| {
        windows
            .iter()
            .find(|window| window.window.is_active())
            .or_else(|| windows.iter().rev().find(|window| window.window.is_visible()))
            .cloned()
    })
}

/// A window to bring up when the app is opened again: one that is not the drop-down window.
pub fn shown_window() -> Option<Rc<Window>> {
    WINDOWS.with_borrow(|windows| windows.iter().rev().find(|window| !window.drop_down.get()).cloned())
}
