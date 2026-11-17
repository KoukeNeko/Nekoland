//! `nekoland-store`: the App Store. One list for software from every place it comes
//! from (Flathub, the Snap Store, Ubuntu's archive and other apt repositories, Homebrew),
//! each program once however many places have it, and on its page the places side by side
//! to install it from.

mod alert;
mod focus;
#[path = "store/sources.rs"]
mod sources;

use std::{cell::RefCell, rc::Rc};

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use sources::{App, Kind, Offer, Repository};

const APP_ID: &str = "io.github.koukeneko.Nekoland.Store";
/// Opens on a page other than Explore: installed, updates, a sidebar row's index,
/// search:WORDS, or app:WORDS for the first found app's page; for screenshots, as NEKOLAND_SETTINGS_PAGE is for Settings.
const PAGE_ENV: &str = "NEKOLAND_STORE_PAGE";

/// The categories Explore shows, as freedesktop names them, with programs known to be
/// wanted put first.
const CATEGORIES: [(&str, &str, &[&str]); 7] = [
    (
        "繪圖與相片",
        "Graphics",
        &[
            "org.gimp.GIMP",
            "org.kde.krita",
            "org.inkscape.Inkscape",
            "org.darktable.Darktable",
            "org.blender.Blender",
            "com.github.PintaProject.Pinta",
        ],
    ),
    (
        "開發",
        "Development",
        &[
            "com.visualstudio.code",
            "org.gnome.Builder",
            "io.podman_desktop.PodmanDesktop",
            "com.jetbrains.IntelliJ-IDEA-Community",
        ],
    ),
    (
        "影音",
        "AudioVideo",
        &[
            "org.videolan.VLC",
            "org.kde.kdenlive",
            "com.obsproject.Studio",
            "org.audacityteam.Audacity",
            "com.spotify.Client",
        ],
    ),
    (
        "辦公",
        "Office",
        &[
            "org.libreoffice.LibreOffice",
            "md.obsidian.Obsidian",
            "org.onlyoffice.desktopeditors",
            "org.gnome.Evince",
        ],
    ),
    (
        "網路",
        "Network",
        &[
            "org.mozilla.firefox",
            "org.telegram.desktop",
            "org.signal.Signal",
            "com.discordapp.Discord",
            "org.mozilla.Thunderbird",
        ],
    ),
    (
        "遊戲",
        "Game",
        &[
            "com.valvesoftware.Steam",
            "net.lutris.Lutris",
            "com.heroicgameslauncher.hgl",
            "org.prismlauncher.PrismLauncher",
        ],
    ),
    (
        "工具",
        "Utility",
        &[
            "com.usebottles.bottles",
            "com.github.tchx84.Flatseal",
            "io.missioncenter.MissionCenter",
            "org.gnome.Calculator",
        ],
    ),
];

const STANDARD: [(&str, &str); 4] = [
    ("Flathub", "flathub"),
    ("Snap Store", "snap"),
    ("Ubuntu (apt)", "apt"),
    ("Homebrew", "brew"),
];

const CSS: &str = "
.store-tile { border-radius: 22%; background: var(--nekoland-fill, alpha(currentColor, .08)); }
.store-tile label { font-weight: 600; color: alpha(currentColor, .7); }
.store-name { font-size: 22px; font-weight: 600; }
.store-mono { font-family: monospace; }
.store-head-row { min-height: 30px; }
.store-head-row label { font-size: 12px; opacity: .7; }
row.store-source { min-height: 44px; }
row.store-source.picked { background: alpha(currentColor, .08); }
.store-muted { opacity: .55; }
.store-chip { font-size: 11px; padding: 1px 7px; border-radius: 6px; border: 1px solid alpha(currentColor, .15); }
.store-chip.installed { background: var(--nekoland-ink, @accent_bg_color); color: var(--nekoland-on-ink, @accent_fg_color); border-color: transparent; }
.store-command { font-family: monospace; font-size: 15px; }
.store-fact-label { font-size: 12px; opacity: .7; }
.store-screenshot { border-radius: 10px; }
.store-section { font-weight: 600; }
.store-category { min-height: 0; padding: 4px 12px; border-radius: 999px; font-weight: normal; }
";

struct Store {
    window: adw::ApplicationWindow,
    nav: adw::NavigationView,
    toasts: adw::ToastOverlay,
    sidebar: gtk::ListBox,
    updates_label: gtk::Label,
    /// What each sidebar row opens, by its index.
    rows: RefCell<Vec<Place>>,
}

#[derive(Clone)]
enum Place {
    Explore,
    Installed,
    Updates,
    Source(&'static str),
    Repository(Repository),
}

fn main() -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(build);
    app.run()
}

fn build(app: &adw::Application) {
    if let Some(window) = app.active_window() {
        window.present();
        return;
    }
    let css = gtk::CssProvider::new();
    css.load_from_string(CSS);
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(&display, &css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    }

    let search = gtk::SearchEntry::builder()
        .placeholder_text("搜尋")
        .margin_start(8)
        .margin_end(8)
        .margin_bottom(8)
        .build();
    let sidebar = gtk::ListBox::builder()
        .css_classes(["navigation-sidebar"])
        .build();
    let updates_label = gtk::Label::builder()
        .css_classes(["dim-label", "numeric"])
        .build();
    let sidebar_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    sidebar_box.append(&search);
    sidebar_box.append(
        &gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&sidebar)
            .build(),
    );
    let sidebar_view = adw::ToolbarView::new();
    sidebar_view.add_top_bar(&adw::HeaderBar::new());
    sidebar_view.set_content(Some(&sidebar_box));

    let nav = adw::NavigationView::new();
    let toasts = adw::ToastOverlay::new();
    toasts.set_child(Some(&nav));
    let split = adw::NavigationSplitView::builder()
        .sidebar(
            &adw::NavigationPage::builder()
                .title("App Store")
                .child(&sidebar_view)
                .build(),
        )
        .content(
            &adw::NavigationPage::builder()
                .title("App Store")
                .child(&toasts)
                .build(),
        )
        .build();
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("App Store")
        .default_width(1120)
        .default_height(760)
        .content(&split)
        .build();
    // A narrow window folds the sidebar away behind the content, as libadwaita does.
    let narrow = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
        adw::BreakpointConditionLengthType::MaxWidth,
        760.0,
        adw::LengthUnit::Sp,
    ));
    narrow.add_setter(&split, "collapsed", Some(&true.to_value()));
    window.add_breakpoint(narrow);

    let store = Rc::new(Store {
        window,
        nav,
        toasts,
        sidebar,
        updates_label,
        rows: RefCell::new(Vec::new()),
    });
    // Everything else holds the store weakly; the application keeps it while it runs.
    let kept = store.clone();
    app.connect_shutdown(move |_| drop(kept.rows.take()));
    fill_sidebar(&store);
    store.sidebar.connect_row_selected(glib::clone!(
        #[weak]
        store,
        #[weak]
        split,
        move |_, row| {
            let Some(row) = row else { return };
            let Some(place) = store.rows.borrow().get(row.index() as usize).cloned() else {
                return;
            };
            let page = match place {
                Place::Explore => explore_page(&store),
                Place::Installed => installed_page(&store),
                Place::Updates => updates_page(&store),
                Place::Source(id) => source_page(&store, id),
                Place::Repository(repo) => repository_page(&store, repo),
            };
            store.nav.replace(&[page]);
            split.set_show_content(true);
        }
    ));
    search.connect_search_changed(glib::clone!(
        #[weak]
        store,
        #[weak]
        split,
        move |entry| {
            let query = entry.text().trim().to_string();
            if query.chars().count() < 2 {
                return;
            }
            store.sidebar.unselect_all();
            store.nav.replace(&[search_page(&store, &query)]);
            split.set_show_content(true);
        }
    ));
    let start = std::env::var(PAGE_ENV).unwrap_or_default();
    if let Some(query) = start.strip_prefix("search:") {
        search.set_text(query);
    } else if let Some(query) = start.strip_prefix("app:") {
        let query = query.to_string();
        background(
            move || sources::search(&query).into_iter().next(),
            glib::clone!(
                #[weak]
                store,
                move |found: Option<App>| {
                    if let Some(app) = found {
                        store.nav.replace(&[detail_page(&store, app)]);
                    }
                }
            ),
        );
    } else {
        let at = match start.as_str() {
            "installed" => 1,
            "updates" => 2,
            other => other.parse().unwrap_or(0),
        };
        store.sidebar.select_row(store.sidebar.row_at_index(at).as_ref());
    }
    count_updates(&store);
    store.window.present();
    focus::let_fields_go(&store.window);
}

fn fill_sidebar(store: &Rc<Store>) {
    while let Some(row) = store.sidebar.row_at_index(0) {
        store.sidebar.remove(&row);
    }
    let mut places = vec![Place::Explore, Place::Installed, Place::Updates];
    let row = |label: &str, end: Option<&gtk::Label>| {
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        line.append(
            &gtk::Label::builder()
                .label(label)
                .xalign(0.0)
                .hexpand(true)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build(),
        );
        if let Some(end) = end {
            line.append(end);
        }
        line
    };
    store.sidebar.append(&row("探索", None));
    store.sidebar.append(&row("已安裝", None));
    store.sidebar.append(&row("更新", Some(&store.updates_label)));
    let first_source = places.len();
    for (label, id) in STANDARD {
        store.sidebar.append(&row(label, None));
        places.push(Place::Source(id));
    }
    let repos = sources::repositories();
    let first_repo = places.len();
    for repo in repos {
        store.sidebar.append(&row(&repo.name, None));
        places.push(Place::Repository(repo));
    }
    let has_repos = places.len() > first_repo;
    store.sidebar.set_header_func(move |row, _| {
        let index = row.index() as usize;
        let title = if index == first_source {
            Some("來源")
        } else if index == first_repo && has_repos {
            Some("其他套件庫")
        } else {
            None
        };
        row.set_header(
            title
                .map(|title| {
                    gtk::Label::builder()
                        .label(title)
                        .xalign(0.0)
                        .css_classes(["caption", "dim-label"])
                        .margin_top(14)
                        .margin_start(10)
                        .margin_bottom(4)
                        .build()
                })
                .as_ref(),
        );
    });
    *store.rows.borrow_mut() = places;
}

/// Work off the main thread, and what to do with its result back on it.
fn background<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static, done: impl FnOnce(T) + 'static) {
    glib::spawn_future_local(async move {
        match gio::spawn_blocking(work).await {
            Ok(value) => done(value),
            Err(_) => eprintln!("store: background work failed"),
        }
    });
}

fn count_updates(store: &Rc<Store>) {
    background(
        updates,
        glib::clone!(
            #[weak]
            store,
            move |found: Updates| {
                let count = found.admin.len() + found.user.len() + found.snaps.len();
                store.updates_label.set_label(&if count > 0 {
                    count.to_string()
                } else {
                    String::new()
                });
            }
        ),
    );
}

// Pieces the pages share.

fn page(title: &str, content: &impl IsA<gtk::Widget>, end: Option<&gtk::Widget>) -> adw::NavigationPage {
    let header = adw::HeaderBar::new();
    if let Some(end) = end {
        header.pack_end(end);
    }
    let view = adw::ToolbarView::new();
    view.add_top_bar(&header);
    let clamp = adw::Clamp::builder()
        .maximum_size(920)
        .tightening_threshold(700)
        .child(content)
        .build();
    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&clamp)
        .build();
    view.set_content(Some(&scroller));
    adw::NavigationPage::builder().title(title).child(&view).build()
}

fn column() -> gtk::Box {
    gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(12)
        .margin_bottom(32)
        .margin_start(24)
        .margin_end(24)
        .build()
}

fn loading() -> gtk::Widget {
    let spinner = gtk::Spinner::builder()
        .spinning(true)
        .width_request(24)
        .height_request(24)
        .margin_top(48)
        .halign(gtk::Align::Center)
        .build();
    spinner.upcast()
}

fn empty(text: &str) -> gtk::Widget {
    gtk::Label::builder()
        .label(text)
        .css_classes(["dim-label"])
        .margin_top(48)
        .build()
        .upcast()
}

fn section(title: &str, end: &str) -> gtk::Box {
    let line = gtk::Box::builder().margin_top(12).build();
    line.append(
        &gtk::Label::builder()
            .label(title)
            .xalign(0.0)
            .hexpand(true)
            .css_classes(["store-section"])
            .build(),
    );
    if !end.is_empty() {
        line.append(
            &gtk::Label::builder()
                .label(end)
                .css_classes(["dim-label", "caption", "numeric"])
                .build(),
        );
    }
    line
}

/// An app's icon, or the first letter of its name on a tile.
fn icon(app: &App, size: i32) -> gtk::Widget {
    if let Some(path) = &app.icon {
        return gtk::Image::builder()
            .file(path.to_string_lossy())
            .pixel_size(size)
            .build()
            .upcast();
    }
    let themed = gdk::Display::default().is_some_and(|display| {
        !app.icon_name.is_empty() && gtk::IconTheme::for_display(&display).has_icon(&app.icon_name)
    });
    if themed {
        return gtk::Image::builder()
            .icon_name(&app.icon_name)
            .pixel_size(size)
            .build()
            .upcast();
    }
    let tile = gtk::Box::builder()
        .width_request(size)
        .height_request(size)
        .css_classes(["store-tile"])
        .valign(gtk::Align::Center)
        .halign(gtk::Align::Start)
        .build();
    // The letter's expanding would otherwise stretch the tile across its row and page.
    tile.set_hexpand(false);
    tile.set_vexpand(false);
    let letter = app.name.chars().next().map(|c| c.to_string()).unwrap_or_default();
    let label = gtk::Label::builder()
        .label(letter)
        .hexpand(true)
        .vexpand(true)
        .build();
    label.set_attributes(Some(&{
        let attrs = gtk::pango::AttrList::new();
        attrs.insert(gtk::pango::AttrSize::new_size_absolute(
            size * gtk::pango::SCALE * 2 / 5,
        ));
        attrs
    }));
    tile.append(&label);
    tile.upcast()
}

fn chip(text: &str, installed: bool) -> gtk::Label {
    let label = gtk::Label::builder()
        .label(text)
        .css_classes(["store-chip"])
        .valign(gtk::Align::Center)
        .build();
    if installed {
        label.add_css_class("installed");
    }
    label
}

/// A short name for where an offer is, for the chips on lists.
fn place_name(offer: &Offer) -> String {
    match offer.kind {
        Some(Kind::Apt) if offer.origin == sources::UBUNTU => "apt".into(),
        Some(Kind::Brew) => "brew".into(),
        Some(Kind::Snap) => "Snap".into(),
        _ => offer.origin.clone(),
    }
}

/// A list of apps, each opening its page.
fn app_list(
    store: &Rc<Store>,
    apps: Vec<App>,
    line: impl Fn(&App) -> String,
    end: impl Fn(&App) -> Option<gtk::Widget>,
) -> gtk::ListBox {
    let list = gtk::ListBox::builder()
        .css_classes(["boxed-list"])
        .selection_mode(gtk::SelectionMode::None)
        .build();
    for app in apps {
        let row = adw::ActionRow::builder()
            .title(glib::markup_escape_text(&app.name))
            .subtitle(glib::markup_escape_text(&line(&app)))
            .activatable(true)
            .build();
        row.set_subtitle_lines(1);
        row.add_prefix(&icon(&app, 40));
        if app.cli {
            row.add_css_class("store-mono-title");
        }
        if let Some(end) = end(&app) {
            row.add_suffix(&end);
        }
        let app = Rc::new(app);
        row.connect_activated(glib::clone!(
            #[weak]
            store,
            move |_| store.nav.push(&detail_page(&store, (*app).clone()))
        ));
        list.append(&row);
    }
    list
}

fn chips(app: &App) -> Option<gtk::Widget> {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    for offer in &app.offers {
        line.append(&chip(&place_name(offer), offer.installed.is_some()));
    }
    Some(line.upcast())
}

// Explore.

fn explore_page(store: &Rc<Store>) -> adw::NavigationPage {
    let content = column();
    let chooser = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let grid_holder = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&chooser);
    content.append(&grid_holder);
    let mut first: Option<gtk::ToggleButton> = None;
    for (label, category, known) in CATEGORIES {
        let button = gtk::ToggleButton::builder()
            .label(label)
            .css_classes(["store-category"])
            .build();
        if let Some(first) = &first {
            button.set_group(Some(first));
        }
        button.connect_toggled(glib::clone!(
            #[weak]
            store,
            #[weak]
            grid_holder,
            move |button| {
                if !button.is_active() {
                    return;
                }
                show_category(&store, &grid_holder, category, known);
            }
        ));
        chooser.append(&button);
        first.get_or_insert(button);
    }
    let page = page("探索", &content, None);
    if let Some(first) = first {
        first.set_active(true);
    }
    page
}

fn show_category(
    store: &Rc<Store>,
    holder: &gtk::Box,
    category: &'static str,
    known: &'static [&'static str],
) {
    while let Some(child) = holder.first_child() {
        holder.remove(&child);
    }
    holder.append(&loading());
    background(
        move || sources::category(category, known),
        glib::clone!(
            #[weak]
            store,
            #[weak]
            holder,
            move |apps: Vec<App>| {
                while let Some(child) = holder.first_child() {
                    holder.remove(&child);
                }
                if apps.is_empty() {
                    holder.append(&empty("沒有軟體"));
                    return;
                }
                let grid = gtk::FlowBox::builder()
                    .selection_mode(gtk::SelectionMode::None)
                    .min_children_per_line(2)
                    .max_children_per_line(4)
                    .column_spacing(10)
                    .row_spacing(10)
                    .homogeneous(true)
                    .build();
                for app in apps {
                    grid.append(&card(&store, app));
                }
                holder.append(&grid);
            }
        ),
    );
}

fn card(store: &Rc<Store>, app: App) -> gtk::Widget {
    let button = gtk::Button::builder().css_classes(["card"]).build();
    let line = gtk::Box::builder()
        .spacing(12)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();
    line.append(&icon(&app, 44));
    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.append(
        &gtk::Label::builder()
            .label(&app.name)
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["heading"])
            .build(),
    );
    text.append(
        &gtk::Label::builder()
            .label(&app.summary)
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["caption", "dim-label"])
            .build(),
    );
    if let Some(chips) = chips(&app) {
        chips.set_margin_top(6);
        text.append(&chips);
    }
    line.append(&text);
    button.set_child(Some(&line));
    button.connect_clicked(glib::clone!(
        #[weak]
        store,
        move |_| store.nav.push(&detail_page(&store, app.clone()))
    ));
    button.upcast()
}

// Search.

fn search_page(store: &Rc<Store>, query: &str) -> adw::NavigationPage {
    let content = column();
    content.append(&loading());
    let query = query.to_string();
    let page = page(&format!("「{query}」的結果"), &content, None);
    background(
        move || sources::search(&query),
        glib::clone!(
            #[weak]
            store,
            #[weak]
            content,
            move |apps: Vec<App>| {
                while let Some(child) = content.first_child() {
                    content.remove(&child);
                }
                if apps.is_empty() {
                    content.append(&empty("沒有結果"));
                    return;
                }
                content.append(&app_list(
                    &store,
                    apps,
                    |app| {
                        if app.developer.is_empty() {
                            app.summary.clone()
                        } else {
                            format!("{} · {}", app.summary, app.developer)
                        }
                    },
                    chips,
                ));
            }
        ),
    );
    page
}

// Installed.

fn installed_page(store: &Rc<Store>) -> adw::NavigationPage {
    let content = column();
    content.append(&loading());
    let page = page("已安裝", &content, None);
    background(
        sources::installed,
        glib::clone!(
            #[weak]
            store,
            #[weak]
            content,
            move |found| {
                while let Some(child) = content.first_child() {
                    content.remove(&child);
                }
                for (at, apps) in found {
                    if apps.is_empty() {
                        continue;
                    }
                    content.append(&section(STANDARD[at as usize].0, &format!("{} 個", apps.len())));
                    content.append(&app_list(
                        &store,
                        apps,
                        |app| app.offers[0].installed.clone().unwrap_or_default(),
                        size_label,
                    ));
                }
                if content.first_child().is_none() {
                    content.append(&empty("沒有已安裝的軟體"));
                }
            }
        ),
    );
    page
}

fn size_label(app: &App) -> Option<gtk::Widget> {
    let size = &app.offers[0].size;
    (!size.is_empty()).then(|| {
        gtk::Label::builder()
            .label(size)
            .css_classes(["dim-label", "numeric"])
            .build()
            .upcast()
    })
}

// One source.

fn source_page(store: &Rc<Store>, id: &'static str) -> adw::NavigationPage {
    let (title, _) = STANDARD
        .iter()
        .find(|(_, key)| *key == id)
        .copied()
        .unwrap_or(("", ""));
    let content = column();
    let facts: &[(&str, &str)] = match id {
        "flathub" => &[("權限", "按權限"), ("安裝到", "使用者")],
        "snap" => &[("權限", "受限"), ("更新", "自動")],
        "apt" => &[("權限", "需要管理員"), ("更新", "隨系統")],
        _ => &[("權限", "使用者層級"), ("更新", "手動")],
    };
    content.append(&facts_line(facts));
    content.append(&loading());
    let page = page(title, &content, None);
    let at = STANDARD.iter().position(|(_, key)| *key == id).unwrap_or(0) as u8;
    background(
        sources::installed,
        glib::clone!(
            #[weak]
            store,
            #[weak]
            content,
            move |mut found| {
                if let Some(last) = content.last_child() {
                    content.remove(&last);
                }
                let mut apps = found.remove(&at).unwrap_or_default();
                if at == 2 {
                    apps.retain(|app| app.offers[0].origin == sources::UBUNTU);
                }
                if apps.is_empty() {
                    content.append(&empty("沒有已安裝的軟體"));
                    return;
                }
                content.append(&section("已安裝", &format!("{} 個", apps.len())));
                content.append(&app_list(
                    &store,
                    apps,
                    |app| app.offers[0].installed.clone().unwrap_or_default(),
                    size_label,
                ));
            }
        ),
    );
    page
}

fn facts_line(facts: &[(&str, &str)]) -> gtk::Box {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 24);
    for (name, value) in facts {
        let label = gtk::Label::builder()
            .use_markup(true)
            .label(format!(
                "<span alpha=\"70%\">{}</span> <b>{}</b>",
                glib::markup_escape_text(name),
                glib::markup_escape_text(value)
            ))
            .build();
        line.append(&label);
    }
    line
}

fn repository_page(store: &Rc<Store>, repo: Repository) -> adw::NavigationPage {
    let content = column();
    let host = repo
        .uri
        .split("://")
        .nth(1)
        .unwrap_or(&repo.uri)
        .trim_end_matches('/')
        .to_string();
    content.append(&facts_line(&[
        ("位址", &host),
        ("類型", "apt"),
        ("權限", "需要管理員"),
    ]));
    content.append(&loading());
    let remove = gtk::Button::builder()
        .label("移除套件庫")
        .css_classes(["flat"])
        .build();
    remove.connect_clicked(glib::clone!(
        #[weak]
        store,
        #[strong]
        repo,
        move |_| {
            let repo = repo.clone();
            alert::confirm(
                &store.window,
                &format!("移除 {} 套件庫？", repo.name),
                Some("已安裝的套件會留著，但不會再收到更新。"),
                "移除",
                glib::clone!(
                    #[weak]
                    store,
                    move || {
                        let plan = sources::remove_repository(&repo);
                        let reason = format!("移除 {} 套件庫", repo.name);
                        background(
                            move || sources::run(&plan, &reason),
                            glib::clone!(
                                #[weak]
                                store,
                                move |result: Result<(), String>| match result {
                                    Ok(()) => {
                                        fill_sidebar(&store);
                                        store.sidebar.select_row(store.sidebar.row_at_index(0).as_ref());
                                    }
                                    Err(err) if err.is_empty() => {}
                                    Err(err) => store.toasts.add_toast(adw::Toast::new(&err)),
                                }
                            ),
                        );
                    }
                ),
            );
        }
    ));
    let page = page(&repo.name, &content, Some(remove.upcast_ref()));
    background(
        move || sources::repository_packages(&repo),
        glib::clone!(
            #[weak]
            store,
            #[weak]
            content,
            move |apps: Vec<App>| {
                if let Some(last) = content.last_child() {
                    content.remove(&last);
                }
                if apps.is_empty() {
                    content.append(&empty("沒有套件"));
                    return;
                }
                content.append(&app_list(
                    &store,
                    apps,
                    |app| {
                        let offer = &app.offers[0];
                        match &offer.installed {
                            Some(_) => format!("{} · 已安裝", offer.version),
                            None => offer.version.clone(),
                        }
                    },
                    size_label,
                ));
            }
        ),
    );
    page
}

// Updates.

#[derive(Default)]
struct Updates {
    /// apt packages, with the versions they go from and to, and system Flatpaks.
    admin: Vec<(String, String, String)>,
    /// The person's own: Flatpaks and Homebrew.
    user: Vec<(String, String, String)>,
    snaps: Vec<String>,
    next_snap_refresh: String,
}

fn updates() -> Updates {
    let (apt, flatpaks, brew, (snaps, next)) = std::thread::scope(|scope| {
        let apt = scope.spawn(sources::apt_updates);
        let flatpaks = scope.spawn(sources::flatpak_updates);
        let brew = scope.spawn(sources::brew_updates);
        let snaps = scope.spawn(sources::snap_updates);
        (
            apt.join().unwrap_or_default(),
            flatpaks.join().unwrap_or_default(),
            brew.join().unwrap_or_default(),
            snaps.join().unwrap_or_default(),
        )
    });
    let system_flatpaks: Vec<String> =
        output_lines("flatpak", &["list", "--system", "--app", "--columns=application"]);
    let mut found = Updates {
        admin: apt
            .into_iter()
            .map(|(n, o, v)| (n, "apt".into(), format!("{o} → {v}")))
            .collect(),
        ..Default::default()
    };
    for (id, _) in flatpaks {
        let entry = (id.clone(), "Flathub".into(), String::new());
        if system_flatpaks.contains(&id) {
            found.admin.push(entry);
        } else {
            found.user.push(entry);
        }
    }
    found
        .user
        .extend(brew.into_iter().map(|name| (name, "brew".into(), String::new())));
    found.snaps = snaps;
    found.next_snap_refresh = next;
    found
}

fn output_lines(program: &str, args: &[&str]) -> Vec<String> {
    std::process::Command::new(program)
        .args(args)
        .output()
        .map(|out| {
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn updates_page(store: &Rc<Store>) -> adw::NavigationPage {
    let content = column();
    content.append(&loading());
    let all = gtk::Button::builder()
        .label("全部更新")
        .css_classes(["suggested-action", "pill"])
        .sensitive(false)
        .build();
    let page = page("更新", &content, Some(all.upcast_ref()));
    background(
        updates,
        glib::clone!(
            #[weak]
            store,
            #[weak]
            content,
            #[weak]
            all,
            move |found: Updates| {
                while let Some(child) = content.first_child() {
                    content.remove(&child);
                }
                let count = found.admin.len() + found.user.len() + found.snaps.len();
                store.updates_label.set_label(&if count > 0 {
                    count.to_string()
                } else {
                    String::new()
                });
                if count == 0 {
                    content.append(&empty("沒有更新"));
                    return;
                }
                let band = |title: &str, end: String, items: &[(String, String, String)]| {
                    if items.is_empty() {
                        return;
                    }
                    content.append(&section(title, &end));
                    let list = gtk::ListBox::builder()
                        .css_classes(["boxed-list"])
                        .selection_mode(gtk::SelectionMode::None)
                        .build();
                    for (name, from, versions) in items {
                        let row = adw::ActionRow::builder()
                            .title(glib::markup_escape_text(name))
                            .subtitle(from.as_str())
                            .build();
                        if !versions.is_empty() {
                            row.add_suffix(
                                &gtk::Label::builder()
                                    .label(versions)
                                    .css_classes(["dim-label", "numeric"])
                                    .build(),
                            );
                        }
                        list.append(&row);
                    }
                    content.append(&list);
                };
                band("需要管理員", format!("{} 項", found.admin.len()), &found.admin);
                band("使用者層級", format!("{} 項", found.user.len()), &found.user);
                let snaps: Vec<_> = found
                    .snaps
                    .iter()
                    .map(|name| (name.clone(), "Snap".to_string(), String::new()))
                    .collect();
                band(
                    "自動",
                    if found.next_snap_refresh.is_empty() {
                        String::new()
                    } else {
                        format!("下次 {}", found.next_snap_refresh)
                    },
                    &snaps,
                );

                all.set_sensitive(true);
                let found = Rc::new(found);
                all.connect_clicked(glib::clone!(
                    #[weak]
                    store,
                    move |button| {
                        button.set_sensitive(false);
                        let plan = update_plan(&found);
                        background(
                            move || sources::run(&plan, "更新軟體"),
                            glib::clone!(
                                #[weak]
                                store,
                                move |result: Result<(), String>| {
                                    match &result {
                                        Err(err) if !err.is_empty() => {
                                            store.toasts.add_toast(adw::Toast::new(err))
                                        }
                                        _ => {}
                                    }
                                    store.nav.replace(&[updates_page(&store)]);
                                }
                            ),
                        );
                    }
                ));
            }
        ),
    );
    page
}

fn update_plan(found: &Updates) -> sources::Plan {
    let mut plan = sources::Plan::default();
    let apt: Vec<&str> = found
        .admin
        .iter()
        .filter(|(_, from, _)| from == "apt")
        .map(|(name, ..)| name.as_str())
        .collect();
    if !apt.is_empty() {
        let mut command: Vec<String> = [
            "env",
            "DEBIAN_FRONTEND=noninteractive",
            "apt-get",
            "-y",
            "install",
            "--only-upgrade",
        ]
        .map(String::from)
        .to_vec();
        command.extend(apt.iter().map(|name| name.to_string()));
        plan.admin.push(command);
    }
    if found.admin.iter().any(|(_, from, _)| from == "Flathub") {
        plan.admin.push(
            ["flatpak", "update", "--system", "-y", "--noninteractive"]
                .map(String::from)
                .to_vec(),
        );
    }
    if found.user.iter().any(|(_, from, _)| from == "Flathub") {
        plan.user.push(
            ["flatpak", "update", "--user", "-y", "--noninteractive"]
                .map(String::from)
                .to_vec(),
        );
    }
    if found.user.iter().any(|(_, from, _)| from == "brew") {
        plan.user.push(["brew", "upgrade"].map(String::from).to_vec());
    }
    if !found.snaps.is_empty() {
        plan.admin.push(["snap", "refresh"].map(String::from).to_vec());
    }
    plan
}

// An app's page.

struct Detail {
    app: RefCell<App>,
    /// The offer the header's button installs, by its index in the app's offers.
    picked: RefCell<usize>,
    busy: RefCell<bool>,
    actions: gtk::Box,
    table: gtk::Box,
    command: gtk::Box,
}

fn detail_page(store: &Rc<Store>, app: App) -> adw::NavigationPage {
    let content = column();
    let head = gtk::Box::builder()
        .spacing(18)
        .margin_top(8)
        .margin_bottom(8)
        .build();
    head.append(&icon(&app, 72));
    let names = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .valign(gtk::Align::Center)
        .hexpand(true)
        .spacing(2)
        .build();
    let name = gtk::Label::builder()
        .label(&app.name)
        .xalign(0.0)
        .wrap(true)
        .css_classes(["store-name"])
        .build();
    if app.cli {
        name.add_css_class("store-mono");
    }
    names.append(&name);
    let byline = [app.summary.as_str(), app.developer.as_str()]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    names.append(
        &gtk::Label::builder()
            .label(&byline)
            .xalign(0.0)
            .wrap(true)
            .css_classes(["dim-label"])
            .build(),
    );
    head.append(&names);
    let actions = gtk::Box::builder().spacing(8).valign(gtk::Align::Center).build();
    head.append(&actions);
    content.append(&head);

    let command = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&command);
    if !app.cli && !app.screenshot.is_empty() {
        let picture = gtk::Picture::builder()
            .content_fit(gtk::ContentFit::Contain)
            .height_request(320)
            .css_classes(["store-screenshot"])
            .build();
        content.append(&picture);
        let url = app.screenshot.clone();
        background(
            move || sources::fetch(&url),
            glib::clone!(
                #[weak]
                picture,
                move |bytes: Option<Vec<u8>>| match bytes
                    .and_then(|bytes| gdk::Texture::from_bytes(&glib::Bytes::from_owned(bytes)).ok())
                {
                    Some(texture) => picture.set_paintable(Some(&texture)),
                    None => picture.set_visible(false),
                }
            ),
        );
    }
    if app.cli {
        if let Some(facts) = cli_facts(&app) {
            content.append(&facts);
        }
    }
    if !app.description.is_empty() {
        content.append(
            &gtk::Label::builder()
                .label(&app.description)
                .xalign(0.0)
                .halign(gtk::Align::Start)
                .wrap(true)
                .max_width_chars(80)
                .lines(8)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .css_classes(["dim-label"])
                .build(),
        );
    }
    let table = gtk::Box::new(gtk::Orientation::Vertical, 10);
    content.append(&table);

    let picked = app.installed().map_or(0, |installed| {
        app.offers
            .iter()
            .position(|o| std::ptr::eq(o, installed))
            .unwrap_or(0)
    });
    let detail = Rc::new(Detail {
        app: RefCell::new(app),
        picked: RefCell::new(picked),
        busy: RefCell::new(false),
        actions,
        table,
        command,
    });
    render(store, &detail);
    let title = detail.app.borrow().name.clone();
    let page = page(&title, &content, None);
    // The page's buttons hold the detail weakly; the page holds it for as long as it is shown,
    // and lets go when it is popped and disposed.
    page.connect_destroy(glib::clone!(
        #[strong]
        detail,
        move |_| drop(detail.busy.borrow())
    ));

    // Other places that have it: a search by its name finds them, kept if they are it.
    let current = detail.app.borrow().clone();
    background(
        move || {
            let query = if current.cli {
                current.key.clone()
            } else {
                current.name.clone()
            };
            let found = sources::search(&query)
                .into_iter()
                .find(|found| found.key == current.key);
            // Sizes are filled after merging, since merging keeps only one of two equal offers.
            let mut merged = sources::merge([Some(current), found].into_iter().flatten()).remove(0);
            sources::fill_sizes(&mut merged);
            merged
        },
        glib::clone!(
            #[weak]
            store,
            #[strong]
            detail,
            move |merged: App| {
                // The installed offer keeps what this page knew of it.
                *detail.picked.borrow_mut() = merged
                    .installed()
                    .and_then(|i| merged.offers.iter().position(|o| std::ptr::eq(o, i)))
                    .unwrap_or(0);
                *detail.app.borrow_mut() = merged;
                if !*detail.busy.borrow() {
                    render(&store, &detail);
                }
            }
        ),
    );
    page
}

fn cli_facts(app: &App) -> Option<gtk::Widget> {
    let offer = app
        .offers
        .iter()
        .find(|o| o.kind == Some(Kind::Brew))
        .or(app.offers.first());
    let mut facts: Vec<(&str, String)> = Vec::new();
    if let Some(offer) = offer.filter(|o| o.kind == Some(Kind::Brew)) {
        facts.push((
            "Tap",
            if offer.via == "brew" {
                "homebrew/core".into()
            } else {
                offer.via.clone()
            },
        ));
        facts.push((
            "類型",
            if offer.cask {
                "cask".into()
            } else {
                "formula".into()
            },
        ));
    }
    if !app.license.is_empty() {
        facts.push(("授權", app.license.clone()));
    }
    if !app.homepage.is_empty() {
        facts.push((
            "首頁",
            app.homepage
                .trim_start_matches("https://")
                .trim_start_matches("http://")
                .trim_end_matches('/')
                .to_string(),
        ));
    }
    if facts.is_empty() {
        return None;
    }
    let grid = gtk::Box::builder()
        .css_classes(["card"])
        .homogeneous(true)
        .build();
    for (name, value) in facts {
        let cell = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(2)
            .margin_top(10)
            .margin_bottom(10)
            .margin_start(14)
            .margin_end(14)
            .build();
        cell.append(
            &gtk::Label::builder()
                .label(name)
                .xalign(0.0)
                .css_classes(["store-fact-label"])
                .build(),
        );
        cell.append(
            &gtk::Label::builder()
                .label(&value)
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .selectable(true)
                .build(),
        );
        grid.append(&cell);
    }
    Some(grid.upcast())
}

/// Draws the header's buttons, the command an installed command-line program gives, and
/// the source table, from the app as it now is.
fn render(store: &Rc<Store>, detail: &Rc<Detail>) {
    for holder in [&detail.actions, &detail.table, &detail.command] {
        while let Some(child) = holder.first_child() {
            holder.remove(&child);
        }
    }
    let app = detail.app.borrow();
    if app.offers.is_empty() {
        return;
    }
    let picked = (*detail.picked.borrow()).min(app.offers.len() - 1);
    let installed = app.installed().cloned();
    let busy = *detail.busy.borrow();

    if busy {
        let bar = gtk::ProgressBar::builder()
            .width_request(120)
            .valign(gtk::Align::Center)
            .build();
        glib::timeout_add_local(
            std::time::Duration::from_millis(120),
            glib::clone!(
                #[weak]
                bar,
                #[upgrade_or]
                glib::ControlFlow::Break,
                move || {
                    bar.pulse();
                    glib::ControlFlow::Continue
                }
            ),
        );
        detail.actions.append(&bar);
    } else if let Some(installed) = &installed {
        detail.actions.append(
            &gtk::Label::builder()
                .label(format!("來自 {}", installed.origin))
                .css_classes(["dim-label"])
                .build(),
        );
        let remove = gtk::Button::builder().label("移除").css_classes(["flat"]).build();
        let offer = installed.clone();
        remove.connect_clicked(glib::clone!(
            #[weak]
            store,
            #[weak]
            detail,
            move |_| confirm_remove(&store, &detail, offer.clone())
        ));
        detail.actions.append(&remove);
        if !app.cli {
            if let Some(info) = launcher(&app, installed) {
                let open = gtk::Button::builder()
                    .label("打開")
                    .css_classes(["suggested-action", "pill"])
                    .build();
                open.connect_clicked(move |_| {
                    if let Err(err) = info.launch(&[], gio::AppLaunchContext::NONE) {
                        eprintln!("open: {err}");
                    }
                });
                detail.actions.append(&open);
            }
        }
    } else {
        let offer = &app.offers[picked];
        if app.offers.len() > 1 {
            let menu = gio::Menu::new();
            for (at, offer) in app.offers.iter().enumerate() {
                menu.append(Some(&offer.origin), Some(&format!("detail.pick({at})")));
            }
            let chooser = gtk::MenuButton::builder()
                .label(&offer.origin)
                .menu_model(&menu)
                .css_classes(["flat"])
                .build();
            let group = gio::SimpleActionGroup::new();
            let pick = gio::SimpleAction::new("pick", Some(glib::VariantTy::INT32));
            pick.connect_activate(glib::clone!(
                #[weak]
                store,
                #[weak]
                detail,
                move |_, at| {
                    if let Some(at) = at.and_then(|v| v.get::<i32>()) {
                        *detail.picked.borrow_mut() = at as usize;
                        render(&store, &detail);
                    }
                }
            ));
            group.add_action(&pick);
            chooser.insert_action_group("detail", Some(&group));
            detail.actions.append(&chooser);
        }
        let install = gtk::Button::builder()
            .css_classes(["suggested-action", "pill"])
            .build();
        install.set_child(Some(&button_content(offer.admin, "安裝")));
        install.connect_clicked(glib::clone!(
            #[weak]
            store,
            #[weak]
            detail,
            move |_| {
                let offer = detail.app.borrow().offers[*detail.picked.borrow()].clone();
                start_install(&store, &detail, offer, None);
            }
        ));
        detail.actions.append(&install);
    }

    // What an installed command-line program is run as.
    if app.cli {
        if let Some(installed) = installed.clone().filter(|_| !busy) {
            let holder = detail.command.clone();
            background(
                move || sources::commands(&installed),
                move |commands: Vec<String>| {
                    if commands.is_empty() {
                        return;
                    }
                    let line = gtk::Box::builder().spacing(12).css_classes(["card"]).build();
                    let text = gtk::Label::builder()
                        .label(format!("$ {}", commands.join("  ")))
                        .xalign(0.0)
                        .hexpand(true)
                        .selectable(true)
                        .margin_start(16)
                        .margin_top(14)
                        .margin_bottom(14)
                        .css_classes(["store-command"])
                        .build();
                    line.append(&text);
                    let copy = gtk::Button::builder()
                        .label("拷貝")
                        .valign(gtk::Align::Center)
                        .margin_end(8)
                        .build();
                    let first = commands[0].clone();
                    copy.connect_clicked(move |button| button.clipboard().set_text(&first));
                    line.append(&copy);
                    holder.append(&line);
                },
            );
        }
    }

    // The source table: one row each place, its facts in columns.
    detail
        .table
        .append(&section("來源", &format!("{} 個", app.offers.len())));
    let list = gtk::ListBox::builder()
        .css_classes(["boxed-list"])
        .selection_mode(gtk::SelectionMode::None)
        .build();
    let groups: Vec<gtk::SizeGroup> = (0..7)
        .map(|_| gtk::SizeGroup::new(gtk::SizeGroupMode::Horizontal))
        .collect();
    let cell = |text: &str, at: usize, dim: bool| {
        let label = gtk::Label::builder()
            .label(text)
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .width_chars(if at == 0 {
                16
            } else if at == 5 {
                14
            } else {
                8
            })
            .max_width_chars(if at == 0 || at == 5 { 22 } else { 12 })
            .build();
        if dim {
            label.add_css_class("store-muted");
        }
        groups[at].add_widget(&label);
        label
    };
    let header = gtk::Box::builder()
        .spacing(12)
        .margin_start(14)
        .margin_end(14)
        .css_classes(["store-head-row"])
        .build();
    for (at, title) in ["來源", "版本", "大小", "更新", "隔離", "發行者"]
        .iter()
        .enumerate()
    {
        header.append(&cell(title, at, false));
    }
    let head_row = gtk::ListBoxRow::builder()
        .activatable(false)
        .selectable(false)
        .child(&header)
        .build();
    list.append(&head_row);
    let unique = |value: &str, field: fn(&Offer) -> String| {
        app.offers.iter().filter(|o| field(o) == value).count() == 1
    };
    for (at, offer) in app.offers.iter().enumerate() {
        let line = gtk::Box::builder()
            .spacing(12)
            .margin_start(14)
            .margin_end(14)
            .build();
        let place = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        place.append(
            &gtk::Label::builder()
                .label(&offer.origin)
                .css_classes(["heading"])
                .build(),
        );
        place.append(
            &gtk::Label::builder()
                .label(&offer.via)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .css_classes(["caption", "dim-label"])
                .build(),
        );
        groups[0].add_widget(&place);
        place.set_size_request(150, -1);
        line.append(&place);
        let version = offer.installed.clone().unwrap_or_else(|| offer.version.clone());
        let values: [(String, fn(&Offer) -> String); 4] = [
            (or_dash(&version), |o| {
                o.installed.clone().unwrap_or_else(|| o.version.clone())
            }),
            (or_dash(&offer.size), |o| o.size.clone()),
            (offer.updates().to_string(), |o| o.updates().to_string()),
            (offer.sandbox().to_string(), |o| o.sandbox().to_string()),
        ];
        for (column, (value, field)) in values.into_iter().enumerate() {
            let label = cell(&value, column + 1, !unique(&value, field) || value == "—");
            if column < 2 {
                label.add_css_class("numeric");
            }
            line.append(&label);
        }
        let publisher = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        publisher.append(&cell(&or_dash(&offer.publisher), 5, offer.publisher.is_empty()));
        if offer.verified {
            publisher.append(&gtk::Image::from_icon_name("object-select-symbolic"));
        }
        publisher.set_hexpand(true);
        line.append(&publisher);
        line.append(&row_end(
            store,
            detail,
            offer,
            at,
            installed.as_ref(),
            busy && at == picked,
        ));
        let row = gtk::ListBoxRow::builder()
            .activatable(installed.is_none() && !busy)
            .child(&line)
            .css_classes(["store-source"])
            .build();
        if installed.is_none() && at == picked {
            row.add_css_class("picked");
        }
        list.append(&row);
    }
    list.connect_row_activated(glib::clone!(
        #[weak]
        store,
        #[weak]
        detail,
        move |_, row| {
            let at = row.index() as usize;
            if at == 0 || *detail.busy.borrow() || detail.app.borrow().installed().is_some() {
                return;
            }
            *detail.picked.borrow_mut() = at - 1;
            render(&store, &detail);
        }
    ));
    detail.table.append(&list);
}

fn or_dash(value: &str) -> String {
    if value.is_empty() {
        "—".into()
    } else {
        value.to_string()
    }
}

fn button_content(admin: bool, label: &str) -> gtk::Widget {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    if admin {
        line.append(&gtk::Image::from_icon_name("system-lock-screen-symbolic"));
    }
    line.append(&gtk::Label::new(Some(label)));
    line.set_halign(gtk::Align::Center);
    line.upcast()
}

/// What the right end of a source row holds: a spinner while installing there, 已安裝,
/// 改用 on the others once one is installed, or 未加入 for a repository not yet added.
fn row_end(
    store: &Rc<Store>,
    detail: &Rc<Detail>,
    offer: &Offer,
    _at: usize,
    installed: Option<&Offer>,
    working: bool,
) -> gtk::Widget {
    let end = gtk::Box::builder()
        .width_request(88)
        .halign(gtk::Align::End)
        .build();
    if working {
        end.append(
            &gtk::Spinner::builder()
                .spinning(true)
                .halign(gtk::Align::End)
                .hexpand(true)
                .build(),
        );
    } else if offer.installed.is_some() {
        end.append(
            &gtk::Label::builder()
                .label("已安裝")
                .hexpand(true)
                .xalign(1.0)
                .css_classes(["dim-label", "caption"])
                .build(),
        );
    } else if let Some(installed) = installed {
        let button = gtk::Button::builder()
            .css_classes(["small", "pill"])
            .halign(gtk::Align::End)
            .hexpand(true)
            .build();
        button.set_child(Some(&button_content(offer.admin || installed.admin, "改用")));
        let (to, from) = (offer.clone(), installed.clone());
        button.connect_clicked(glib::clone!(
            #[weak]
            store,
            #[weak]
            detail,
            move |_| confirm_switch(&store, &detail, to.clone(), from.clone())
        ));
        end.append(&button);
    } else if offer.repo.is_some() {
        end.append(
            &gtk::Label::builder()
                .label("未加入")
                .hexpand(true)
                .xalign(1.0)
                .css_classes(["dim-label", "caption"])
                .build(),
        );
    }
    end.upcast()
}

/// The program an installed offer gave the system, to open it with.
fn launcher(app: &App, offer: &Offer) -> Option<gio::AppInfo> {
    let wanted: Vec<String> = match offer.kind {
        Some(Kind::Flatpak) => vec![format!("{}.desktop", offer.id)],
        Some(Kind::Snap) => vec![
            format!("{}_{}.desktop", offer.id, offer.id),
            format!("{}_", offer.id),
        ],
        _ => vec![format!("{}.desktop", app.key), format!("{}.desktop", offer.id)],
    };
    gio::AppInfo::all().into_iter().find(|info| {
        let id = info.id().map(|id| id.to_lowercase()).unwrap_or_default();
        wanted.iter().any(|want| {
            if want.ends_with('_') {
                id.starts_with(want)
            } else {
                id == want.to_lowercase()
            }
        })
    })
}

fn start_install(store: &Rc<Store>, detail: &Rc<Detail>, offer: Offer, replacing: Option<Offer>) {
    let Some(vendor) = offer.repo else {
        install(store, detail, offer, replacing);
        return;
    };
    let dialog = adw::AlertDialog::new(
        Some(&format!("加入 {} 的套件庫？", vendor.name)),
        Some(&format!(
            "{}\n加入後，{} 可以隨系統更新安裝或更新任何套件。",
            vendor.uri, vendor.name
        )),
    );
    dialog.add_responses(&[("cancel", "取消"), ("add", "加入")]);
    dialog.set_response_appearance("add", adw::ResponseAppearance::Suggested);
    dialog.connect_response(
        Some("add"),
        glib::clone!(
            #[weak]
            store,
            #[weak]
            detail,
            move |_, _| install(&store, &detail, offer.clone(), replacing.clone())
        ),
    );
    dialog.present(Some(&store.window));
}

fn confirm_switch(store: &Rc<Store>, detail: &Rc<Detail>, to: Offer, from: Offer) {
    let dialog = adw::AlertDialog::new(
        Some(&format!("改用 {}？", to.origin)),
        Some(&format!("裝好後會移除 {} 的版本。設定不會帶過去。", from.origin)),
    );
    dialog.add_responses(&[("cancel", "取消"), ("switch", "改用")]);
    dialog.set_response_appearance("switch", adw::ResponseAppearance::Suggested);
    dialog.connect_response(
        Some("switch"),
        glib::clone!(
            #[weak]
            store,
            #[weak]
            detail,
            move |_, _| start_install(&store, &detail, to.clone(), Some(from.clone()))
        ),
    );
    dialog.present(Some(&store.window));
}

fn confirm_remove(store: &Rc<Store>, detail: &Rc<Detail>, offer: Offer) {
    let name = detail.app.borrow().name.clone();
    alert::confirm(
        &store.window,
        &format!("移除 {name}？"),
        None,
        "移除",
        glib::clone!(
            #[weak]
            store,
            #[weak]
            detail,
            move || {
                let plan = sources::remove_plan(&offer);
                let id = offer.id.clone();
                change(
                    &store,
                    &detail,
                    plan,
                    format!("移除 {name}"),
                    Rc::new(move |app: &mut App| {
                        for o in &mut app.offers {
                            if o.id == id {
                                o.installed = None;
                            }
                        }
                    }),
                );
            }
        ),
    );
}

/// Installs an offer; switching, the one it replaces is removed only once the new one is
/// in, so that a failed install leaves the old one as it was.
fn install(store: &Rc<Store>, detail: &Rc<Detail>, offer: Offer, replacing: Option<Offer>) {
    let name = detail.app.borrow().name.clone();
    let mut plan = sources::install_plan(&offer);
    if let Some(old) = &replacing {
        let remove = sources::remove_plan(old);
        plan.user.extend(remove.user);
        plan.admin.extend(remove.admin);
    }
    let (id, origin, old_id) = (offer.id.clone(), offer.origin.clone(), replacing.map(|o| o.id));
    change(
        store,
        detail,
        plan,
        format!("安裝 {name}"),
        Rc::new(move |app: &mut App| {
            for o in &mut app.offers {
                if o.id == id && o.origin == origin {
                    o.installed = Some(o.version.clone());
                    o.repo = None;
                } else if Some(&o.id) == old_id.as_ref() {
                    o.installed = None;
                }
            }
        }),
    );
}

/// Runs a change in the background, the page showing it working, and on success updates
/// what the page knows; a failure is said in a toast with a way to try again.
fn change(
    store: &Rc<Store>,
    detail: &Rc<Detail>,
    plan: sources::Plan,
    reason: String,
    done: Rc<dyn Fn(&mut App)>,
) {
    *detail.busy.borrow_mut() = true;
    render(store, detail);
    let plan = Rc::new(plan);
    let job = {
        let plan = plan.clone();
        let user = plan.user.clone();
        let admin = plan.admin.clone();
        let reason = reason.clone();
        move || sources::run(&sources::Plan { user, admin }, &reason)
    };
    background(
        job,
        glib::clone!(
            #[weak]
            store,
            #[strong]
            detail,
            move |result: Result<(), String>| {
                *detail.busy.borrow_mut() = false;
                match result {
                    Ok(()) => {
                        done(&mut detail.app.borrow_mut());
                        count_updates(&store);
                    }
                    // Asked for the password and cancelled: back as it was, with nothing to say.
                    Err(err) if err.is_empty() => {}
                    Err(err) => {
                        let toast = adw::Toast::builder()
                            .title(&err)
                            .button_label("重試")
                            .timeout(0)
                            .build();
                        toast.connect_button_clicked(glib::clone!(
                            #[weak]
                            store,
                            #[strong]
                            detail,
                            #[strong]
                            plan,
                            #[strong]
                            reason,
                            #[strong]
                            done,
                            move |_| {
                                let plan = sources::Plan {
                                    user: plan.user.clone(),
                                    admin: plan.admin.clone(),
                                };
                                change(&store, &detail, plan, reason.clone(), done.clone());
                            }
                        ));
                        store.toasts.add_toast(toast);
                    }
                }
                render(&store, &detail);
            }
        ),
    );
}
