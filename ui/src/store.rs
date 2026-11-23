//! `nekoland-store`: Market. One list for software from every place it comes from
//! (Flathub, the Snap Store, Ubuntu's archive and other apt repositories, Homebrew), each
//! program once however many places have it, and on its page the places side by side to
//! install it from; icon and cursor themes from Pling, tried on before they are applied; and
//! `.deb` package files it is given to open.

mod alert;
mod apt;
mod program;
#[path = "store/browsers.rs"]
mod browsers;
#[path = "store/choice.rs"]
mod choice;
#[path = "store/deb.rs"]
mod deb;
mod focus;
mod ipc;
#[path = "store/pling.rs"]
mod pling;
mod selfupdate;
#[path = "store/sources.rs"]
mod sources;
mod removal;
// Where the Dock finds an app's icon.
#[allow(dead_code)]
#[path = "launcher/apps.rs"]
mod apps;
mod themes;
mod web;

use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    rc::{Rc, Weak},
};

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use pling::{Look, Theme};
use sources::{App, Kind, Offer, Repository, STANDARD};

const APP_ID: &str = "io.github.koukeneko.Nekoland.Store";
/// Opens on a page other than Explore: browsers, installed, updates, icons, cursors, a sidebar row's index,
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
.store-theme { padding: 0; }
.store-preview { background: alpha(currentColor, .04); border-bottom: 1px solid alpha(currentColor, .1); }
.store-zone { min-height: 120px; transition: background 120ms ease-out; }
.store-zone:hover { background: alpha(currentColor, .06); }
.store-zone + .store-zone { border-left: 1px solid alpha(currentColor, .1); }
.store-using { padding: 6px 14px; border-radius: 999px; background: alpha(currentColor, .08); font-weight: 600; }
.store-dock { padding: 12px 16px; border-radius: 18px; background: alpha(currentColor, .06); }
.store-hero { padding: 0 40px; min-height: 220px; border-radius: 18px; background: @window_fg_color; color: @window_bg_color; }
.store-hero-title { font-size: 36px; font-weight: 700; }
.store-hero-line { opacity: .72; }
row.store-choice { min-height: 56px; }
row.store-choice.picked { background: @window_fg_color; color: @window_bg_color; }
row.store-choice.picked .store-chip { border-color: alpha(currentColor, .3); }
";

/// How the 瀏覽器 page's banner fans its icons: how big, how far apart, how much each tilts
/// and drops from the middle one.
const FAN_ICON: i32 = 96;
const FAN_STEP: f32 = 78.0;
const FAN_TILT_DEGREES: f32 = 7.0;
const FAN_DROP: f32 = 12.0;
const FAN_ICONS: usize = 5;

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
    Browsers,
    Installed,
    Updates,
    Look(Look),
    Source(&'static str),
    Repository(Repository),
}

fn main() -> glib::ExitCode {
    if std::env::args().any(|arg| arg == choice::FLAG) {
        return choice::run();
    }
    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_OPEN)
        .build();
    let shown: Rc<RefCell<Weak<Store>>> = Rc::default();
    app.connect_activate(glib::clone!(
        #[strong]
        shown,
        move |app| {
            show(app, &shown);
        }
    ));
    // Package files, from the command line, Files, or another program asking it to open them.
    app.connect_open(move |app, files, _| {
        let store = show(app, &shown);
        open_files(&store, files);
    });
    app.run()
}

/// The window, built the first time; the application holds the store while it runs.
fn show(app: &adw::Application, shown: &RefCell<Weak<Store>>) -> Rc<Store> {
    if let Some(store) = shown.borrow().upgrade() {
        store.window.present();
        return store;
    }
    let store = build(app);
    *shown.borrow_mut() = Rc::downgrade(&store);
    store
}

fn open_files(store: &Rc<Store>, files: &[gio::File]) {
    for file in files {
        match file.path() {
            Some(path) => store.nav.push(&deb_page(store, path)),
            None => store
                .toasts
                .add_toast(adw::Toast::new(&format!("無法讀取 {}", file.uri()))),
        }
    }
}

fn load_css() {
    let css = gtk::CssProvider::new();
    css.load_from_string(CSS);
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(&display, &css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    }
}

fn build(app: &adw::Application) -> Rc<Store> {
    load_css();

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
                .title("Market")
                .child(&sidebar_view)
                .build(),
        )
        .content(
            &adw::NavigationPage::builder()
                .title("Market")
                .child(&toasts)
                .build(),
        )
        .build();
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Market")
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
                Place::Browsers => browsers_page(&store),
                Place::Installed => installed_page(&store),
                Place::Updates => updates_page(&store),
                Place::Look(look) => look_page(&store, look),
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
            "browsers" => 1,
            "installed" => 2,
            "updates" => 3,
            "icons" => 4,
            "cursors" => 5,
            other => other.parse().unwrap_or(0),
        };
        store.sidebar.select_row(store.sidebar.row_at_index(at).as_ref());
    }
    count_updates(&store);
    store.window.present();
    focus::let_fields_go(&store.window);
    store
}

fn fill_sidebar(store: &Rc<Store>) {
    while let Some(row) = store.sidebar.row_at_index(0) {
        store.sidebar.remove(&row);
    }
    let mut places = vec![Place::Explore, Place::Browsers, Place::Installed, Place::Updates];
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
    store.sidebar.append(&row("瀏覽器", None));
    store.sidebar.append(&row("已安裝", None));
    store.sidebar.append(&row("更新", Some(&store.updates_label)));
    let first_look = places.len();
    for look in [Look::Icons, Look::Cursors] {
        store.sidebar.append(&row(look.name(), None));
        places.push(Place::Look(look));
    }
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
        let title = if index == first_look {
            Some("外觀")
        } else if index == first_source {
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
                let count = found.count();
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

/// An app's icon: its own file, its theme's, or the first letter of its name on a tile; the
/// tile gives way to the icon on the web, where there is one, once it is fetched.
fn icon(app: &App, size: i32) -> gtk::Widget {
    let local = local_icon(app, size);
    if app.icon.is_some() || app.icon_url.is_empty() || local.is::<gtk::Image>() {
        return local;
    }
    let holder = gtk::Box::builder()
        .halign(gtk::Align::Start)
        .valign(gtk::Align::Center)
        .build();
    holder.append(&local);
    let url = app.icon_url.clone();
    background(
        move || sources::fetch(&url),
        glib::clone!(
            #[weak]
            holder,
            move |bytes: Option<Vec<u8>>| {
                let Some(texture) =
                    bytes.and_then(|bytes| gdk::Texture::from_bytes(&glib::Bytes::from_owned(bytes)).ok())
                else {
                    return;
                };
                if let Some(tile) = holder.first_child() {
                    holder.remove(&tile);
                }
                holder.append(&gtk::Image::builder().paintable(&texture).pixel_size(size).build());
            }
        ),
    );
    holder.upcast()
}

thread_local! {
    /// Where the Dock finds each icon name, as it is asked for, and where it looks.
    static DOCK_ICONS: (RefCell<std::collections::HashMap<String, Option<PathBuf>>>, Vec<PathBuf>, Vec<String>) = {
        let look = apps::Look::load();
        let themes = look.icon_themes().into_iter().map(String::from).collect();
        (RefCell::default(), apps::data_dirs(), themes)
    };
}

/// The icon the Dock shows for the first of `names` it has one for.
fn dock_icon(names: &[&str]) -> Option<PathBuf> {
    DOCK_ICONS.with(|(found, dirs, themes)| {
        let themes: Vec<&str> = themes.iter().map(String::as_str).collect();
        names.iter().find_map(|name| {
            found
                .borrow_mut()
                .entry(name.to_string())
                .or_insert_with(|| apps::find_icon(dirs, &themes, name))
                .clone()
        })
    })
}

fn local_icon(app: &App, size: i32) -> gtk::Widget {
    let names: Vec<&str> = [app.icon_name.as_str(), app.key.as_str()]
        .into_iter()
        .chain(app.offers.iter().map(|offer| offer.id.as_str()))
        .filter(|name| !name.is_empty() && !name.starts_with('/'))
        .collect();
    if let Some(path) = dock_icon(&names) {
        return gtk::Image::builder().file(path.to_string_lossy()).pixel_size(size).build().upcast();
    }
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
                holder.append(&card_grid(&store, apps));
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

// Browsers: Nekoland comes with none.

fn browsers_page(store: &Rc<Store>) -> adw::NavigationPage {
    let content = column();
    let fan = gtk::Fixed::builder().valign(gtk::Align::Center).build();
    content.append(&browsers_hero(&fan));
    let grid_holder = gtk::Box::new(gtk::Orientation::Vertical, 0);
    grid_holder.append(&loading());
    content.append(&grid_holder);
    background(
        browsers::available,
        glib::clone!(
            #[weak]
            store,
            #[weak]
            grid_holder,
            #[weak]
            fan,
            move |apps: Vec<App>| {
                while let Some(child) = grid_holder.first_child() {
                    grid_holder.remove(&child);
                }
                if apps.is_empty() {
                    grid_holder.append(&empty("沒有可安裝的瀏覽器"));
                    return;
                }
                fill_fan(&fan, &apps);
                grid_holder.append(&card_grid(&store, apps));
            }
        ),
    );
    page("瀏覽器", &content, None)
}

/// The banner: what the default browser is, and the browsers fanned out.
fn browsers_hero(fan: &gtk::Fixed) -> gtk::Widget {
    let hero = gtk::Box::builder().css_classes(["store-hero"]).spacing(24).build();
    let text = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .valign(gtk::Align::Center)
        .hexpand(true)
        .spacing(10)
        .build();
    text.append(
        &gtk::Label::builder()
            .label("網頁瀏覽器")
            .xalign(0.0)
            .css_classes(["store-hero-title"])
            .build(),
    );
    text.append(
        &gtk::Label::builder()
            .label(browser_state())
            .xalign(0.0)
            .css_classes(["store-hero-line"])
            .build(),
    );
    hero.append(&text);
    hero.append(fan);
    hero.upcast()
}

fn browser_state() -> String {
    match web::default() {
        Some(app) => format!("預設：{}", app.display_name()),
        None if web::installed().is_empty() => "尚未安裝".into(),
        None => "未設定預設".into(),
    }
}

/// The first browsers' icons side by side, each tilted away from the middle one, which is
/// drawn last so that it lies on top.
fn fill_fan(fan: &gtk::Fixed, apps: &[App]) {
    let shown = &apps[..apps.len().min(FAN_ICONS)];
    let middle = (shown.len() as f32 - 1.0) / 2.0;
    let half = FAN_ICON as f32 / 2.0;
    let mut order: Vec<usize> = (0..shown.len()).collect();
    order.sort_by(|a, b| (*b as f32 - middle).abs().total_cmp(&(*a as f32 - middle).abs()));
    for at in order {
        let offset = at as f32 - middle;
        let widget = icon(&shown[at], FAN_ICON);
        fan.put(&widget, 0.0, 0.0);
        let transform = gtk::gsk::Transform::new()
            .translate(&gtk::graphene::Point::new(
                at as f32 * FAN_STEP + half,
                offset.abs() * FAN_DROP + half + FAN_DROP,
            ))
            .rotate(offset * FAN_TILT_DEGREES)
            .translate(&gtk::graphene::Point::new(-half, -half));
        fan.set_child_transform(&widget, Some(&transform));
    }
    let reach = (shown.len() as f32 - 1.0) * FAN_STEP + FAN_ICON as f32;
    fan.set_size_request(reach as i32, (FAN_ICON as f32 + middle * FAN_DROP * 2.0 + FAN_DROP) as i32);
}

fn card_grid(store: &Rc<Store>, apps: Vec<App>) -> gtk::FlowBox {
    let grid = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .min_children_per_line(2)
        .max_children_per_line(4)
        .column_spacing(10)
        .row_spacing(10)
        .homogeneous(true)
        .build();
    for app in apps {
        grid.append(&card(store, app));
    }
    grid
}

// Looks: icon and cursor themes.

/// Common applications, by the names icon themes draw them under, for a theme's card.
const SAMPLE_ICONS: [&[&str]; 6] = [
    &["system-file-manager", "org.gnome.Nautilus", "folder"],
    &["utilities-terminal", "org.gnome.Terminal", "terminal"],
    &["web-browser", "firefox", "org.mozilla.firefox"],
    &["accessories-text-editor", "org.gnome.TextEditor", "text-editor"],
    &["multimedia-audio-player", "rhythmbox", "audio-x-generic"],
    &["internet-mail", "thunderbird", "mail-client"],
];

const FOLDERS: [(&str, &str); 5] = [
    ("家目錄", "user-home"),
    ("文件", "folder-documents"),
    ("下載", "folder-download"),
    ("圖片", "folder-pictures"),
    ("音樂", "folder-music"),
];

/// An icon theme looked up on its own, not the one the window uses.
fn icon_theme(name: &str) -> gtk::IconTheme {
    let theme = gtk::IconTheme::new();
    let dirs = themes::dirs();
    theme.set_search_path(&dirs.iter().map(|dir| dir.as_path()).collect::<Vec<_>>());
    theme.set_theme_name(Some(name));
    theme
}

/// The first of `names` the theme draws.
fn themed(theme: &gtk::IconTheme, names: &[&str], size: i32) -> Option<gtk::Image> {
    let name = names.iter().find(|name| theme.has_icon(name))?;
    let paintable = theme.lookup_icon(
        name,
        &[],
        size,
        2,
        gtk::TextDirection::None,
        gtk::IconLookupFlags::empty(),
    );
    Some(
        gtk::Image::builder()
            .paintable(&paintable)
            .pixel_size(size)
            .build(),
    )
}

fn cursor_image(frame: &pling::Frame, size: i32) -> gtk::Image {
    gtk::Image::builder()
        .paintable(&frame.texture())
        .pixel_size(size)
        .build()
}

/// A picture from the web, which goes away if it cannot be fetched.
fn web_picture(url: &str, height: i32) -> gtk::Picture {
    let picture = gtk::Picture::builder()
        .content_fit(gtk::ContentFit::Cover)
        .height_request(height)
        .build();
    let url = url.to_string();
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
    picture
}

/// The top of a theme's card: its cursors or icons when it is here, else Pling's picture.
fn theme_preview(theme: &Theme) -> gtk::Widget {
    let strip = gtk::Box::builder()
        .spacing(14)
        .height_request(92)
        .halign(gtk::Align::Fill)
        .css_classes(["store-preview"])
        .build();
    let inner = gtk::Box::builder()
        .spacing(if theme.look == Look::Cursors { 14 } else { 8 })
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .hexpand(true)
        .build();
    strip.append(&inner);
    let here = theme
        .folders
        .first()
        .and_then(|folder| pling::folder_path(folder));
    match (here, theme.look) {
        (Some(path), Look::Cursors) => {
            for frame in pling::shapes(&path, 48).into_iter().flatten() {
                inner.append(&cursor_image(&frame, 26));
            }
        }
        (Some(_), Look::Icons) => {
            let icons = icon_theme(&theme.folders[0]);
            for names in SAMPLE_ICONS {
                if let Some(image) = themed(&icons, names, 28) {
                    inner.append(&image);
                }
            }
        }
        (None, _) => {
            if let Some(url) = theme.previews.first() {
                // Over the strip, so that the picture's own width does not widen the card.
                strip.remove(&inner);
                let over = gtk::Overlay::builder().child(&inner).hexpand(true).build();
                over.add_overlay(&web_picture(url, 92));
                strip.append(&over);
            }
        }
    }
    strip.set_overflow(gtk::Overflow::Hidden);
    strip.upcast()
}

fn theme_card(store: &Rc<Store>, theme: Theme, using: &str) -> gtk::Widget {
    let button = gtk::Button::builder()
        .css_classes(["card", "store-theme"])
        .overflow(gtk::Overflow::Hidden)
        .build();
    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    body.append(&theme_preview(&theme));
    let meta = gtk::Box::builder()
        .spacing(8)
        .margin_top(10)
        .margin_bottom(12)
        .margin_start(14)
        .margin_end(12)
        .build();
    // The labels' widths are capped, since the grid makes every card as wide as its widest.
    let text = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .hexpand(true)
        .build();
    text.append(
        &gtk::Label::builder()
            .label(&theme.name)
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .max_width_chars(16)
            .css_classes(["heading"])
            .build(),
    );
    if !theme.author.is_empty() {
        text.append(
            &gtk::Label::builder()
                .label(&theme.author)
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .max_width_chars(16)
                .css_classes(["caption", "dim-label"])
                .build(),
        );
    }
    meta.append(&text);
    if theme.folders.iter().any(|folder| folder == using) {
        meta.append(&chip("使用中", true));
    } else if !theme.folders.is_empty() {
        meta.append(&chip("已安裝", false));
    }
    body.append(&meta);
    button.set_child(Some(&body));
    button.connect_clicked(glib::clone!(
        #[weak]
        store,
        move |_| store.nav.push(&theme_page(&store, theme.clone()))
    ));
    button.upcast()
}

fn theme_grid(store: &Rc<Store>, found: Vec<Theme>, using: &str) -> gtk::FlowBox {
    let grid = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .min_children_per_line(2)
        .max_children_per_line(3)
        .column_spacing(12)
        .row_spacing(12)
        .homogeneous(true)
        .build();
    for theme in found {
        grid.append(&theme_card(store, theme, using));
    }
    grid
}

fn look_page(store: &Rc<Store>, look: Look) -> adw::NavigationPage {
    let content = column();
    let page = page(look.name(), &content, None);
    // Filled each time it is shown, so that coming back from a theme shows what changed there.
    page.connect_shown(glib::clone!(
        #[weak]
        store,
        #[weak]
        content,
        move |_| {
            if content.first_child().is_none() {
                content.append(&loading());
            }
            background(
                move || (pling::local(look), pling::current(look), pling::popular(look)),
                glib::clone!(
                    #[weak]
                    store,
                    #[weak]
                    content,
                    move |(local, using, popular): (Vec<Theme>, String, Option<Vec<Theme>>)| {
                        while let Some(child) = content.first_child() {
                            content.remove(&child);
                        }
                        content.append(&section("已安裝", &format!("{} 個", local.len())));
                        content.append(&theme_grid(&store, local, &using));
                        content.append(&section("熱門", ""));
                        match popular {
                            None => content.append(&empty("無法連線到 Pling")),
                            Some(found) if found.is_empty() => content.append(&empty("沒有主題")),
                            Some(found) => content.append(&theme_grid(&store, found, &using)),
                        }
                    }
                ),
            );
        }
    ));
    page
}

struct ThemeDetail {
    theme: RefCell<Theme>,
    /// The file to install, or once installed the folder to apply, by its index.
    picked: Cell<usize>,
    busy: Cell<bool>,
    using: RefCell<String>,
    /// Counts the hero's loads, so that a slow one for an earlier pick is dropped.
    round: Cell<u32>,
    tile: gtk::Box,
    actions: gtk::Box,
    hero: gtk::Box,
    facts: gtk::Box,
}

fn theme_page(store: &Rc<Store>, theme: Theme) -> adw::NavigationPage {
    let content = column();
    let head = gtk::Box::builder()
        .spacing(18)
        .margin_top(8)
        .margin_bottom(8)
        .build();
    let tile = gtk::Box::builder()
        .width_request(72)
        .height_request(72)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .css_classes(["store-tile"])
        .build();
    head.append(&tile);
    let names = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .valign(gtk::Align::Center)
        .hexpand(true)
        .spacing(2)
        .build();
    names.append(
        &gtk::Label::builder()
            .label(&theme.name)
            .xalign(0.0)
            .wrap(true)
            .css_classes(["store-name"])
            .build(),
    );
    let byline = [theme.author.as_str(), theme.look.name()]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    names.append(
        &gtk::Label::builder()
            .label(&byline)
            .xalign(0.0)
            .css_classes(["dim-label"])
            .build(),
    );
    head.append(&names);
    let actions = gtk::Box::builder().spacing(8).valign(gtk::Align::Center).build();
    head.append(&actions);
    content.append(&head);
    let hero = gtk::Box::new(gtk::Orientation::Vertical, 12);
    content.append(&hero);
    let facts = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&facts);
    if !theme.description.is_empty() {
        // Pling's descriptions run long, a line each paragraph; the label's own limit counts only wrapped lines.
        let description: Vec<&str> = theme.description.lines().take(6).collect();
        content.append(
            &gtk::Label::builder()
                .label(description.join("\n"))
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
    let title = theme.name.clone();
    let look = theme.look;
    let detail = Rc::new(ThemeDetail {
        theme: RefCell::new(theme),
        picked: Cell::new(0),
        busy: Cell::new(false),
        using: RefCell::new(String::new()),
        round: Cell::new(0),
        tile,
        actions,
        hero,
        facts,
    });
    render_theme(store, &detail);
    show_hero(&detail);
    let page = page(&title, &content, None);
    page.connect_destroy(glib::clone!(
        #[strong]
        detail,
        move |_| {
            let _ = &detail;
        }
    ));
    background(
        move || pling::current(look),
        glib::clone!(
            #[weak]
            store,
            #[weak]
            detail,
            move |using: String| {
                *detail.using.borrow_mut() = using;
                render_theme(&store, &detail);
            }
        ),
    );
    page
}

fn render_theme(store: &Rc<Store>, detail: &Rc<ThemeDetail>) {
    for holder in [&detail.actions, &detail.facts] {
        while let Some(child) = holder.first_child() {
            holder.remove(&child);
        }
    }
    let theme = detail.theme.borrow();
    let pick = glib::clone!(
        #[weak]
        store,
        #[weak]
        detail,
        move |at| {
            detail.picked.set(at);
            render_theme(&store, &detail);
            show_hero(&detail);
        }
    );
    if detail.busy.get() {
        detail.actions.append(&working(None));
    } else if !theme.folders.is_empty() {
        let at = detail.picked.get().min(theme.folders.len() - 1);
        let folder = theme.folders[at].clone();
        let using = detail.using.borrow().clone();
        if theme.folders.len() > 1 {
            detail.actions.append(&chooser(&theme.folders, at, pick));
        }
        if !theme.system && !theme.folders.contains(&using) {
            let remove = gtk::Button::builder().label("移除").css_classes(["flat"]).build();
            remove.connect_clicked(glib::clone!(
                #[weak]
                store,
                #[weak]
                detail,
                move |_| remove_theme(&store, &detail)
            ));
            detail.actions.append(&remove);
        }
        if folder == using {
            detail.actions.append(
                &gtk::Label::builder()
                    .label("使用中")
                    .css_classes(["store-using"])
                    .build(),
            );
        } else {
            let apply = gtk::Button::builder()
                .label("套用")
                .css_classes(["suggested-action", "pill"])
                .build();
            apply.connect_clicked(glib::clone!(
                #[weak]
                store,
                #[weak]
                detail,
                move |_| apply_theme(&store, &detail, folder.clone())
            ));
            detail.actions.append(&apply);
        }
    } else if !theme.files.is_empty() {
        let at = detail.picked.get().min(theme.files.len() - 1);
        if theme.files.len() > 1 {
            let names: Vec<String> = theme.files.iter().map(|file| file.name.clone()).collect();
            detail.actions.append(&chooser(&names, at, pick));
        }
        let install = gtk::Button::builder()
            .label("安裝")
            .css_classes(["suggested-action", "pill"])
            .build();
        install.connect_clicked(glib::clone!(
            #[weak]
            store,
            #[weak]
            detail,
            move |_| install_theme(&store, &detail)
        ));
        detail.actions.append(&install);
    }

    let place = theme
        .path
        .as_ref()
        .map(|path| path.display().to_string())
        .unwrap_or_default();
    let size = theme
        .files
        .get(detail.picked.get())
        .map(|file| file.size.clone())
        .unwrap_or_default();
    let facts: Vec<(&str, &str)> = if theme.id.is_empty() {
        vec![("位置", place.as_str())]
    } else {
        vec![
            ("作者", theme.author.as_str()),
            ("更新", theme.changed.as_str()),
            ("大小", size.as_str()),
            ("來源", "Pling"),
        ]
    };
    let facts: Vec<(&str, &str)> = facts.into_iter().filter(|(_, value)| !value.is_empty()).collect();
    detail.facts.append(&facts_line(&facts));
}

/// The cursors to try on, or the person's Dock in the icons; before an icon theme is
/// installed, Pling's picture of it.
fn show_hero(detail: &Rc<ThemeDetail>) {
    while let Some(child) = detail.hero.first_child() {
        detail.hero.remove(&child);
    }
    let round = detail.round.get() + 1;
    detail.round.set(round);
    let theme = detail.theme.borrow().clone();
    let at = detail.picked.get();
    match theme.look {
        Look::Cursors => {
            detail.hero.append(&section("試用", ""));
            let pad = gtk::Box::builder()
                .homogeneous(true)
                .css_classes(["card"])
                .overflow(gtk::Overflow::Hidden)
                .build();
            pad.append(&loading());
            detail.hero.append(&pad);
            let folder = theme
                .folders
                .get(at.min(theme.folders.len().saturating_sub(1)))
                .cloned();
            background(
                move || {
                    // Not installed, the pack is fetched to the cache to try it on.
                    let path = match folder {
                        Some(folder) => {
                            pling::folder_path(&folder).ok_or_else(|| format!("找不到 {folder}"))?
                        }
                        None => {
                            let file = theme.files.get(at).ok_or_else(String::new)?;
                            pling::fetch(&theme, file)?.remove(0)
                        }
                    };
                    Ok((pling::shapes(&path, 48), pling::shapes(&path, 24)))
                },
                glib::clone!(
                    #[weak]
                    detail,
                    #[weak]
                    pad,
                    move |found: Result<(pling::Shapes, pling::Shapes), String>| {
                        if detail.round.get() != round {
                            return;
                        }
                        while let Some(child) = pad.first_child() {
                            pad.remove(&child);
                        }
                        let (big, small) = match found {
                            Ok(frames) => frames,
                            Err(err) => {
                                pad.append(&empty(&err));
                                return;
                            }
                        };
                        while let Some(child) = detail.tile.first_child() {
                            detail.tile.remove(&child);
                        }
                        if let Some(arrow) = &big[0] {
                            let image = cursor_image(arrow, 40);
                            image.set_hexpand(true);
                            detail.tile.append(&image);
                        }
                        for (((label, _), big), small) in pling::SHAPES.iter().zip(big).zip(small) {
                            let zone = gtk::Box::builder()
                                .orientation(gtk::Orientation::Vertical)
                                .spacing(14)
                                .css_classes(["store-zone"])
                                .build();
                            let inner = gtk::Box::builder()
                                .orientation(gtk::Orientation::Vertical)
                                .spacing(14)
                                .valign(gtk::Align::Center)
                                .vexpand(true)
                                .build();
                            if let Some(big) = &big {
                                inner.append(&cursor_image(big, 30));
                            }
                            inner.append(
                                &gtk::Label::builder()
                                    .label(*label)
                                    .css_classes(["caption", "dim-label"])
                                    .build(),
                            );
                            zone.append(&inner);
                            if let Some(small) = small {
                                zone.set_cursor(Some(&gdk::Cursor::from_texture(
                                    &small.texture(),
                                    small.xhot as i32,
                                    small.yhot as i32,
                                    None,
                                )));
                            }
                            pad.append(&zone);
                        }
                    }
                ),
            );
        }
        Look::Icons => {
            let Some(folder) = theme
                .folders
                .get(at.min(theme.folders.len().saturating_sub(1)))
                .cloned()
            else {
                if let Some(url) = theme.previews.first() {
                    let picture = web_picture(url, 320);
                    picture.set_content_fit(gtk::ContentFit::Contain);
                    picture.add_css_class("store-screenshot");
                    detail.hero.append(&picture);
                }
                return;
            };
            let icons = icon_theme(&folder);
            while let Some(child) = detail.tile.first_child() {
                detail.tile.remove(&child);
            }
            if let Some(image) = themed(&icons, SAMPLE_ICONS[0], 72) {
                detail.tile.remove_css_class("store-tile");
                detail.tile.append(&image);
            }
            detail.hero.append(&section("你的 Dock", ""));
            let stage = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(22)
                .css_classes(["card"])
                .build();
            let dock = gtk::Box::builder()
                .spacing(10)
                .halign(gtk::Align::Center)
                .margin_top(28)
                .css_classes(["store-dock"])
                .build();
            stage.append(&dock);
            let folders = gtk::Box::builder()
                .spacing(28)
                .halign(gtk::Align::Center)
                .margin_bottom(22)
                .build();
            for (label, name) in FOLDERS {
                let figure = gtk::Box::new(gtk::Orientation::Vertical, 6);
                if let Some(image) = themed(&icons, &[name, "folder"], 44) {
                    figure.append(&image);
                }
                figure.append(
                    &gtk::Label::builder()
                        .label(label)
                        .css_classes(["caption"])
                        .build(),
                );
                folders.append(&figure);
            }
            stage.append(&folders);
            detail.hero.append(&stage);
            background(pling::dock, move |apps: Vec<String>| {
                for app in apps {
                    let info = gio::DesktopAppInfo::new(&format!("{app}.desktop"))
                        .or_else(|| gio::DesktopAppInfo::new(&app));
                    let names: Vec<String> = info
                        .and_then(|info| info.icon())
                        .and_then(|icon| icon.downcast::<gio::ThemedIcon>().ok())
                        .map(|icon| icon.names().iter().map(|name| name.to_string()).collect())
                        .unwrap_or_default();
                    let names: Vec<&str> = names.iter().map(String::as_str).collect();
                    if let Some(image) = themed(&icons, &names, 44) {
                        dock.append(&image);
                    }
                }
            });
        }
    }
}

/// Says a change failed, with a way to try it again.
fn say_failed(store: &Rc<Store>, err: &str, retry: impl Fn() + 'static) {
    let toast = adw::Toast::builder()
        .title(err)
        .button_label("重試")
        .timeout(0)
        .build();
    toast.connect_button_clicked(move |_| retry());
    store.toasts.add_toast(toast);
}

fn install_theme(store: &Rc<Store>, detail: &Rc<ThemeDetail>) {
    let theme = detail.theme.borrow().clone();
    let Some(file) = theme.files.get(detail.picked.get()).cloned() else {
        return;
    };
    detail.busy.set(true);
    render_theme(store, detail);
    background(
        move || pling::install(&theme, &file),
        glib::clone!(
            #[weak]
            store,
            #[strong]
            detail,
            move |result: Result<Vec<String>, String>| {
                detail.busy.set(false);
                match result {
                    Ok(folders) => {
                        detail.theme.borrow_mut().folders = folders;
                        detail.picked.set(0);
                        show_hero(&detail);
                    }
                    Err(err) => say_failed(
                        &store,
                        &err,
                        glib::clone!(
                            #[weak]
                            store,
                            #[weak]
                            detail,
                            move || install_theme(&store, &detail)
                        ),
                    ),
                }
                render_theme(&store, &detail);
            }
        ),
    );
}

fn apply_theme(store: &Rc<Store>, detail: &Rc<ThemeDetail>, folder: String) {
    let look = detail.theme.borrow().look;
    background(
        {
            let folder = folder.clone();
            move || pling::apply(look, &folder)
        },
        glib::clone!(
            #[weak]
            store,
            #[weak]
            detail,
            move |result: Result<(), String>| {
                match result {
                    Ok(()) => *detail.using.borrow_mut() = folder.clone(),
                    Err(err) => say_failed(
                        &store,
                        &err,
                        glib::clone!(
                            #[weak]
                            store,
                            #[weak]
                            detail,
                            #[strong]
                            folder,
                            move || apply_theme(&store, &detail, folder.clone())
                        ),
                    ),
                }
                render_theme(&store, &detail);
            }
        ),
    );
}

fn remove_theme(store: &Rc<Store>, detail: &Rc<ThemeDetail>) {
    let name = detail.theme.borrow().name.clone();
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
                let theme = detail.theme.borrow().clone();
                background(
                    move || pling::remove(&theme),
                    glib::clone!(
                        #[weak]
                        store,
                        #[weak]
                        detail,
                        move |result: Result<(), String>| match result {
                            // Known only from this computer, there is nothing left to show.
                            Ok(()) if detail.theme.borrow().id.is_empty() => {
                                store.nav.pop();
                            }
                            Ok(()) => {
                                detail.theme.borrow_mut().folders.clear();
                                detail.picked.set(0);
                                render_theme(&store, &detail);
                                show_hero(&detail);
                            }
                            Err(err) => say_failed(
                                &store,
                                &err,
                                glib::clone!(
                                    #[weak]
                                    store,
                                    #[weak]
                                    detail,
                                    move || remove_theme(&store, &detail)
                                ),
                            ),
                        }
                    ),
                );
            }
        ),
    );
}

// Search.

fn search_page(store: &Rc<Store>, query: &str) -> adw::NavigationPage {
    let content = column();
    content.append(&loading());
    let query = query.to_string();
    let page = page(&format!("「{query}」的結果"), &content, None);
    background(
        move || {
            std::thread::scope(|scope| {
                let found = scope.spawn(|| pling::search(&query));
                (sources::search(&query), found.join().unwrap_or_default())
            })
        },
        glib::clone!(
            #[weak]
            store,
            #[weak]
            content,
            move |(apps, found): (Vec<App>, Vec<Theme>)| {
                while let Some(child) = content.first_child() {
                    content.remove(&child);
                }
                if apps.is_empty() && found.is_empty() {
                    content.append(&empty("沒有結果"));
                    return;
                }
                if !found.is_empty() {
                    let list = gtk::ListBox::builder()
                        .css_classes(["boxed-list"])
                        .selection_mode(gtk::SelectionMode::None)
                        .build();
                    let count = found.len();
                    for theme in found {
                        let row = adw::ActionRow::builder()
                            .title(glib::markup_escape_text(&theme.name))
                            .subtitle(glib::markup_escape_text(&theme.author))
                            .activatable(true)
                            .build();
                        row.add_suffix(&chip(theme.look.name(), false));
                        row.connect_activated(glib::clone!(
                            #[weak]
                            store,
                            move |_| store.nav.push(&theme_page(&store, theme.clone()))
                        ));
                        list.append(&row);
                    }
                    // After the apps, which are what a search is mostly for.
                    content.append(&section("主題", &format!("{count} 個")));
                    content.append(&list);
                }
                if apps.is_empty() {
                    return;
                }
                content.prepend(&app_list(
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
        installed_with_icons,
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

/// What is installed, each app with the icon its desktop file names, as the Dock shows it.
fn installed_with_icons() -> std::collections::BTreeMap<u8, Vec<App>> {
    let mut found = sources::installed();
    for app in found.values_mut().flatten() {
        let Some(icon) = removal::launcher(app, &app.offers[0])
            .and_then(|info| info.downcast_ref::<gio::DesktopAppInfo>()?.string("Icon"))
        else {
            continue;
        };
        if std::path::Path::new(icon.as_str()).is_absolute() {
            app.icon = Some(PathBuf::from(icon.as_str()));
        } else {
            app.icon_name = icon.to_string();
        }
    }
    found
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
        installed_with_icons,
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
    // Removing deletes the repository's file, so not one that names others too.
    let remove = gtk::Button::builder()
        .label("移除套件庫")
        .css_classes(["flat"])
        .visible(sources::removable(&repo))
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
    /// Nekoland's own, when main has a newer commit than the one installed.
    nekoland: Option<selfupdate::Status>,
}

impl Updates {
    fn count(&self) -> usize {
        self.admin.len() + self.user.len() + self.snaps.len() + usize::from(self.nekoland.is_some())
    }
}

fn updates() -> Updates {
    let (apt, flatpaks, brew, (snaps, next), nekoland) = std::thread::scope(|scope| {
        let nekoland = scope.spawn(selfupdate::check);
        let apt = scope.spawn(sources::apt_updates);
        let flatpaks = scope.spawn(sources::flatpak_updates);
        let brew = scope.spawn(sources::brew_updates);
        let snaps = scope.spawn(sources::snap_updates);
        (
            apt.join().unwrap_or_default(),
            flatpaks.join().unwrap_or_default(),
            brew.join().unwrap_or_default(),
            snaps.join().unwrap_or_default(),
            nekoland
                .join()
                .ok()
                .and_then(Result::ok)
                .filter(|status| !status.is_current()),
        )
    });
    let system_flatpaks: Vec<String> =
        output_lines("flatpak", &["list", "--system", "--app", "--columns=application"]);
    let mut found = Updates {
        admin: apt
            .into_iter()
            .map(|(n, o, v)| (n, "apt".into(), format!("{o} → {v}")))
            .collect(),
        nekoland,
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
                let count = found.count();
                store.updates_label.set_label(&if count > 0 {
                    count.to_string()
                } else {
                    String::new()
                });
                if count == 0 {
                    content.append(&empty("沒有更新"));
                    return;
                }
                if let Some(status) = &found.nekoland {
                    let list = gtk::ListBox::builder()
                        .css_classes(["boxed-list"])
                        .selection_mode(gtk::SelectionMode::None)
                        .build();
                    list.append(&selfupdate::row(Some(status.clone())));
                    content.append(&list);
                }
                let band = |title: &str, end: String, items: &[(String, String, String)]| {
                    if items.is_empty() {
                        return;
                    }
                    content.append(&section(title, &end));
                    content.append(&package_list(items));
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

                // Nekoland updates from its own row.
                all.set_sensitive(count > usize::from(found.nekoland.is_some()));
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

/// Packages, each its name over where it comes from, and at its end its versions.
fn package_list(items: &[(String, String, String)]) -> gtk::ListBox {
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
    list
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
        let mut command: Vec<String> = sources::APT_GET
            .iter()
            .chain(&["install", "--only-upgrade"])
            .map(|word| word.to_string())
            .collect();
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
    /// How far the change running, if one is, has got.
    watch: RefCell<Option<sources::Watch>>,
    actions: gtk::Box,
    table: gtk::Box,
    command: gtk::Box,
}

/// The top of an app's page: its icon, name and summary, and a box for its buttons, which is
/// returned with it.
fn app_head(app: &App) -> (gtk::Box, gtk::Box) {
    let head = gtk::Box::builder()
        .spacing(18)
        .margin_top(8)
        .margin_bottom(8)
        .build();
    head.append(&icon(app, 72));
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
    (head, actions)
}

fn description_label(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .halign(gtk::Align::Start)
        .wrap(true)
        .max_width_chars(80)
        .lines(8)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .css_classes(["dim-label"])
        .build()
}

fn detail_page(store: &Rc<Store>, app: App) -> adw::NavigationPage {
    let content = column();
    let (head, actions) = app_head(&app);
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
        content.append(&description_label(&app.description));
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
        watch: RefCell::default(),
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
        facts.push(("首頁", bare_address(&app.homepage)));
    }
    fact_grid(facts)
}

/// A web address as the facts show it, without its scheme or a trailing slash.
fn bare_address(url: &str) -> String {
    url.trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_string()
}

/// Facts side by side on a card, each its name over its value; None for no facts.
fn fact_grid(facts: Vec<(&str, String)>) -> Option<gtk::Widget> {
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
        detail.actions.append(&working(detail.watch.borrow().clone()));
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
            if let Some(info) = removal::launcher(&app, installed) {
                detail.actions.append(&open_button(info));
            }
        }
    } else {
        let offer = &app.offers[picked];
        if app.offers.len() > 1 {
            let origins: Vec<String> = app.offers.iter().map(|o| o.origin.clone()).collect();
            detail.actions.append(&chooser(
                &origins,
                picked,
                glib::clone!(
                    #[weak]
                    store,
                    #[weak]
                    detail,
                    move |at| {
                        *detail.picked.borrow_mut() = at;
                        render(&store, &detail);
                    }
                ),
            ));
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

fn open_button(info: gio::AppInfo) -> gtk::Button {
    let open = gtk::Button::builder()
        .label("打開")
        .css_classes(["suggested-action", "pill"])
        .build();
    open.connect_clicked(move |_| {
        if let Err(err) = info.launch(&[], gio::AppLaunchContext::NONE) {
            eprintln!("open: {err}");
        }
    });
    open
}

fn or_dash(value: &str) -> String {
    if value.is_empty() {
        "—".into()
    } else {
        value.to_string()
    }
}

/// What a page's main button becomes while it works.
fn working(watch: Option<sources::Watch>) -> gtk::Widget {
    let bar = gtk::ProgressBar::builder()
        .width_request(120)
        .valign(gtk::Align::Center)
        .build();
    follow_progress(&bar, watch);
    bar.upcast()
}

/// How often a working bar moves, or looks at how far its change has got.
const PROGRESS_TICK: std::time::Duration = std::time::Duration::from_millis(120);

/// Keeps `bar` at how far the change `watch` follows has got, going back and forth until it
/// says (while the password is asked, say) or where nothing tells.
fn follow_progress(bar: &gtk::ProgressBar, watch: Option<sources::Watch>) {
    glib::timeout_add_local(
        PROGRESS_TICK,
        glib::clone!(
            #[weak]
            bar,
            #[upgrade_or]
            glib::ControlFlow::Break,
            move || {
                let progress = watch.as_ref().and_then(|watch| *watch.lock().ok()?);
                match progress {
                    Some(progress) => {
                        let stage = if progress.downloading { "下載中" } else { "安裝中" };
                        bar.set_fraction(progress.fraction);
                        bar.set_text(Some(&format!("{stage} {:.0}%", progress.fraction * 100.0)));
                        bar.set_show_text(true);
                    }
                    None => bar.pulse(),
                }
                glib::ControlFlow::Continue
            }
        ),
    );
}

/// A button showing the one of `labels` at `at`, whose menu picks another.
fn chooser(labels: &[String], at: usize, pick: impl Fn(usize) + 'static) -> gtk::MenuButton {
    let menu = gio::Menu::new();
    for (at, label) in labels.iter().enumerate() {
        menu.append(Some(label), Some(&format!("detail.pick({at})")));
    }
    let button = gtk::MenuButton::builder()
        .label(&labels[at])
        .menu_model(&menu)
        .css_classes(["flat"])
        .build();
    let group = gio::SimpleActionGroup::new();
    let action = gio::SimpleAction::new("pick", Some(glib::VariantTy::INT32));
    action.connect_activate(move |_, at| {
        if let Some(at) = at.and_then(|v| v.get::<i32>()) {
            pick(at as usize);
        }
    });
    group.add_action(&action);
    button.insert_action_group("detail", Some(&group));
    button
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

fn start_install(store: &Rc<Store>, detail: &Rc<Detail>, offer: Offer, replacing: Option<Offer>) {
    let Some(vendor) = offer.repo else {
        install(store, detail, offer, replacing);
        return;
    };
    confirm_repository(
        &store.window,
        vendor,
        glib::clone!(
            #[weak]
            store,
            #[weak]
            detail,
            move || install(&store, &detail, offer.clone(), replacing.clone())
        ),
    );
}

/// Asks before adding a vendor's repository, which can then install or update any package;
/// `then` installs with it.
fn confirm_repository(parent: &impl IsA<gtk::Widget>, vendor: &sources::Vendor, then: impl Fn() + 'static) {
    let dialog = adw::AlertDialog::new(
        Some(&format!("加入 {} 的套件庫？", vendor.name)),
        Some(&format!(
            "{}\n加入後，{} 可以隨系統更新安裝或更新任何套件。",
            vendor.uri, vendor.name
        )),
    );
    dialog.add_responses(&[("cancel", "取消"), ("add", "加入")]);
    dialog.set_response_appearance("add", adw::ResponseAppearance::Suggested);
    dialog.connect_response(Some("add"), move |_, _| then());
    dialog.present(Some(parent));
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
    let app = detail.app.borrow().clone();
    let (id, name) = (offer.id.clone(), app.name.clone());
    removal::confirm(
        &store.window,
        &app,
        &offer,
        glib::clone!(
            #[weak]
            store,
            #[weak]
            detail,
            move |plan, leftovers| {
                let window = store.window.clone();
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
                        removal::trash_leftovers(&window, &leftovers);
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
        // The administrator's part of the removal follows the install's in the same sudo;
        // the person's own waits for both, so that a password not given keeps the old one.
        let remove = sources::remove_plan(old, false);
        plan.admin.extend(remove.admin);
        plan.after.extend(remove.user);
    }
    let browser = browsers::is_browser(&offer).then(|| offer.clone());
    let (id, origin, old_id) = (offer.id.clone(), offer.origin.clone(), replacing.map(|o| o.id));
    change(
        store,
        detail,
        plan,
        format!("安裝 {name}"),
        Rc::new(move |app: &mut App| {
            if let Some(offer) = &browser {
                default_browser_if_none(offer.clone());
            }
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

/// The first browser installed opens links, as there was nothing else to open them.
fn default_browser_if_none(offer: Offer) {
    if web::default().is_some() {
        return;
    }
    web::when_listed(
        move || browsers::entry_of(&offer),
        |found| match found {
            Some(app) => web::make_default(&app),
            None => eprintln!("store: the installed browser's desktop entry did not appear"),
        },
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
    let watch = sources::Watch::default();
    *detail.busy.borrow_mut() = true;
    *detail.watch.borrow_mut() = Some(watch.clone());
    render(store, detail);
    let plan = Rc::new(plan);
    let job = {
        let plan = sources::Plan::clone(&plan);
        let reason = reason.clone();
        move || sources::run_watched(&plan, &reason, &watch)
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
                *detail.watch.borrow_mut() = None;
                match result {
                    Ok(()) => {
                        done(&mut detail.app.borrow_mut());
                        count_updates(&store);
                    }
                    // Asked for the password and cancelled: back as it was, with nothing to say.
                    Err(err) if err.is_empty() => {}
                    Err(err) => say_failed(
                        &store,
                        &err,
                        glib::clone!(
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
                            move || {
                                let plan = sources::Plan::clone(&plan);
                                change(&store, &detail, plan, reason.clone(), done.clone());
                            }
                        ),
                    ),
                }
                render(&store, &detail);
            }
        ),
    );
}

// A package file.

struct DebPage {
    path: PathBuf,
    busy: Cell<bool>,
    /// How far the install running, if one is, has got.
    watch: RefCell<Option<sources::Watch>>,
    /// What the file is once read, or why it could not be read.
    found: RefCell<Option<Result<deb::Inspection, String>>>,
    content: gtk::Box,
    page: glib::WeakRef<adw::NavigationPage>,
}

fn deb_page(store: &Rc<Store>, path: PathBuf) -> adw::NavigationPage {
    let content = column();
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let page = page(&file_name, &content, None);
    let deb = Rc::new(DebPage {
        path,
        busy: Cell::new(false),
        watch: RefCell::default(),
        found: RefCell::default(),
        content,
        page: page.downgrade(),
    });
    // As an app's page does, the page holds what it shows until it is popped and disposed.
    page.connect_destroy(glib::clone!(
        #[strong]
        deb,
        move |_| {
            let _ = &deb;
        }
    ));
    render_deb(store, &deb);
    read_deb(store, &deb);
    page
}

/// Reads the file and asks about it, as again once it is installed, and shows what it is.
fn read_deb(store: &Rc<Store>, deb: &Rc<DebPage>) {
    let path = deb.path.clone();
    background(
        move || deb::inspect(&path),
        glib::clone!(
            #[weak]
            store,
            #[weak]
            deb,
            move |found: Result<deb::Inspection, String>| {
                deb.busy.set(false);
                *deb.found.borrow_mut() = Some(found);
                render_deb(&store, &deb);
            }
        ),
    );
}

fn render_deb(store: &Rc<Store>, deb: &Rc<DebPage>) {
    while let Some(child) = deb.content.first_child() {
        deb.content.remove(&child);
    }
    let found = deb.found.borrow();
    let found = match found.as_ref() {
        None => {
            deb.content.append(&loading());
            return;
        }
        Some(Err(err)) => {
            deb.content.append(&empty(err));
            return;
        }
        Some(Ok(found)) => found,
    };
    let package = &found.package;
    if let Some(page) = deb.page.upgrade() {
        page.set_title(&package.name);
    }
    let launcher = deb_launcher(&found.entries);
    let app = App {
        name: package.name.clone(),
        summary: package.summary.clone(),
        developer: package.maintainer.clone(),
        icon_name: launcher.as_ref().map(themed_icon_name).unwrap_or_default(),
        ..Default::default()
    };
    let (head, actions) = app_head(&app);
    deb.content.append(&head);
    deb_actions(store, deb, found, launcher, &actions);
    let facts: Vec<(&str, String)> = [
        ("版本", package.version.clone()),
        ("大小", package.size(found.file_bytes)),
        ("架構", package.architecture.clone()),
        ("首頁", bare_address(&package.homepage)),
    ]
    .into_iter()
    .filter(|(_, value)| !value.is_empty())
    .collect();
    if let Some(grid) = fact_grid(facts) {
        deb.content.append(&grid);
    }
    if !package.description.is_empty() {
        deb.content.append(&description_label(&package.description));
    }
    match &found.changes {
        Ok(changes) => {
            let installs: Vec<(String, String, String)> = changes
                .installs
                .iter()
                .map(|change| {
                    let versions = match &change.from {
                        Some(from) => format!("{from} → {}", change.version),
                        None => change.version.clone(),
                    };
                    (change.name.clone(), String::new(), versions)
                })
                .collect();
            let removes: Vec<(String, String, String)> = changes
                .removes
                .iter()
                .map(|name| (name.clone(), String::new(), String::new()))
                .collect();
            for (title, items) in [("一併安裝", installs), ("一併移除", removes)] {
                if items.is_empty() {
                    continue;
                }
                deb.content.append(&section(title, &format!("{} 個", items.len())));
                deb.content.append(&package_list(&items));
            }
        }
        Err(err) => {
            deb.content.append(&section("相依套件", ""));
            deb.content.append(
                &gtk::Label::builder()
                    .label(err)
                    .xalign(0.0)
                    .wrap(true)
                    .css_classes(["dim-label"])
                    .build(),
            );
        }
    }
}

/// The first program the installed package lists, to open it with.
fn deb_launcher(entries: &[PathBuf]) -> Option<gio::AppInfo> {
    entries
        .iter()
        .find_map(|path| gio::DesktopAppInfo::from_filename(path).filter(|info| info.should_show()))
        .map(|info| info.upcast())
}

fn themed_icon_name(info: &gio::AppInfo) -> String {
    info.icon()
        .and_then(|icon| icon.downcast::<gio::ThemedIcon>().ok())
        .and_then(|icon| icon.names().first().map(|name| name.to_string()))
        .unwrap_or_default()
}

/// The header's buttons: how the file stands to what is installed, and the install, which
/// is withheld where apt says it cannot be done.
fn deb_actions(
    store: &Rc<Store>,
    deb: &Rc<DebPage>,
    found: &deb::Inspection,
    launcher: Option<gio::AppInfo>,
    actions: &gtk::Box,
) {
    let state = |text: &str| {
        gtk::Label::builder()
            .label(text)
            .css_classes(["dim-label"])
            .build()
    };
    if deb.busy.get() {
        actions.append(&working(deb.watch.borrow().clone()));
        return;
    }
    if !found.fits {
        actions.append(&state(&format!("不支援 {} 架構", found.package.architecture)));
        return;
    }
    let standing = found.standing();
    let (said, verb) = match &standing {
        deb::Standing::New => (None, "安裝"),
        deb::Standing::Upgrade(from) => (Some(format!("已安裝較舊的 {from}")), "更新"),
        deb::Standing::Same => (Some("已安裝此版本".to_string()), "重新安裝"),
        deb::Standing::Downgrade(from) => (Some(format!("已安裝較新的 {from}")), "降級"),
    };
    if let Some(said) = said {
        actions.append(&state(&said));
    }
    let launcher = launcher.filter(|_| standing == deb::Standing::Same);
    let install = gtk::Button::builder().sensitive(found.changes.is_ok()).build();
    install.set_child(Some(&button_content(true, verb)));
    // Installed as it is, opening it is what is mostly wanted.
    let classes: &[&str] = if launcher.is_some() {
        &["flat"]
    } else {
        &["suggested-action", "pill"]
    };
    for class in classes {
        install.add_css_class(class);
    }
    install.connect_clicked(glib::clone!(
        #[weak]
        store,
        #[weak]
        deb,
        move |_| start_deb_install(&store, &deb)
    ));
    actions.append(&install);
    if let Some(info) = launcher {
        actions.append(&open_button(info));
    }
}

/// Installs the file, asking first when it would put an older version in place of a newer.
fn start_deb_install(store: &Rc<Store>, deb: &Rc<DebPage>) {
    let (name, version, standing) = match deb.found.borrow().as_ref() {
        Some(Ok(found)) => (
            found.package.name.clone(),
            found.package.version.clone(),
            found.standing(),
        ),
        _ => return,
    };
    if !matches!(standing, deb::Standing::Downgrade(_)) {
        install_deb(store, deb, &name, &standing);
        return;
    }
    alert::confirm(
        &store.window,
        &format!("將 {name} 降級到 {version}？"),
        None,
        "降級",
        glib::clone!(
            #[weak]
            store,
            #[weak]
            deb,
            move || install_deb(&store, &deb, &name, &standing)
        ),
    );
}

fn install_deb(store: &Rc<Store>, deb: &Rc<DebPage>, name: &str, standing: &deb::Standing) {
    let plan = deb::install_plan(&deb.path, standing);
    let reason = format!("安裝 {name}");
    let watch = sources::Watch::default();
    deb.busy.set(true);
    *deb.watch.borrow_mut() = Some(watch.clone());
    render_deb(store, deb);
    background(
        move || sources::run_watched(&plan, &reason, &watch),
        glib::clone!(
            #[weak]
            store,
            #[weak]
            deb,
            move |result: Result<(), String>| {
                *deb.watch.borrow_mut() = None;
                match result {
                    // Read again, so that the page says what is now installed; it keeps
                    // working until then.
                    Ok(()) => {
                        count_updates(&store);
                        read_deb(&store, &deb);
                    }
                    Err(err) => {
                        // Empty when the password was not given: back as it was.
                        if !err.is_empty() {
                            say_failed(
                                &store,
                                &err,
                                glib::clone!(
                                    #[weak]
                                    store,
                                    #[weak]
                                    deb,
                                    move || start_deb_install(&store, &deb)
                                ),
                            );
                        }
                        deb.busy.set(false);
                        render_deb(&store, &deb);
                    }
                }
            }
        ),
    );
}
