//! The choice of a web browser, asked at login while this computer has none, as Nekoland
//! comes with none: the browsers Market can install here, in a new order each time so that
//! none is favoured, and none picked until the person picks one; it is installed, made the
//! default and opened. Skipping it is remembered; Market's 瀏覽器 page stays for later.

use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    rc::Rc,
};

use adw::prelude::*;
use gtk::{gio, glib};

use crate::{
    alert, background, browsers, chip, confirm_repository, focus, follow_progress, icon, load_css,
    place_name, sources, web, App,
};

/// The argument `nekoland-store` asks the choice with; the compositor passes it at login.
pub const FLAG: &str = "--choose-browser";
/// Its own application, so that Market opened meanwhile is a window of its own; its desktop
/// entry names it Market in the menu bar and the Dock.
const APP_ID: &str = "io.github.koukeneko.Nekoland.BrowserChoice";
const PROGRAM: &str = "nekoland-store";
const WIDTH: i32 = 560;
const ICON_SIZE: i32 = 36;
/// Where skipping is remembered, under the state directory.
const SKIPPED_FILE: &str = "nekoland/browser-choice-skipped";

struct Choice {
    window: adw::ApplicationWindow,
    apps: Vec<App>,
    rows: RefCell<Vec<adw::ActionRow>>,
    picked: Cell<Option<usize>>,
    list: gtk::ListBox,
    skip: gtk::Button,
    install: gtk::Button,
    /// Holds the install's progress bar while it runs.
    progress: gtk::Box,
    busy: Cell<bool>,
    /// Set once a browser is in, so that the window closing then is not skipping.
    done: Cell<bool>,
}

pub fn run() -> glib::ExitCode {
    if !web::installed().is_empty() || skipped_path().exists() {
        return glib::ExitCode::SUCCESS;
    }
    let mut apps = browsers::available();
    if apps.is_empty() {
        return glib::ExitCode::SUCCESS;
    }
    shuffle(&mut apps);
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(move |app| present(app, apps.clone()));
    app.run_with_args(&[PROGRAM])
}

fn shuffle(apps: &mut [App]) {
    for at in (1..apps.len()).rev() {
        let other = glib::random_int_range(0, at as i32 + 1) as usize;
        apps.swap(at, other);
    }
}

fn skipped_path() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| glib::home_dir().join(".local/state"))
        .join(SKIPPED_FILE)
}

fn remember_skip() {
    let path = skipped_path();
    let written = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&path, ""));
    if let Err(err) = written {
        eprintln!("{}: {err}", path.display());
    }
}

fn present(app: &adw::Application, apps: Vec<App>) {
    if let Some(window) = app.active_window() {
        window.present();
        return;
    }
    load_css();
    let list = gtk::ListBox::builder()
        .css_classes(["boxed-list"])
        .selection_mode(gtk::SelectionMode::None)
        .build();
    let install = gtk::Button::builder()
        .label("安裝")
        .sensitive(false)
        .css_classes(["suggested-action"])
        .build();
    let skip = gtk::Button::builder().label("略過").css_classes(["flat"]).build();
    let progress = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .margin_top(16)
        .visible(false)
        .build();
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("選擇網頁瀏覽器")
        .default_width(WIDTH)
        .resizable(false)
        .content(&layout(&list, &progress, &skip, &install))
        .build();
    let choice = Rc::new(Choice {
        window,
        apps,
        rows: RefCell::default(),
        picked: Cell::new(None),
        list,
        skip,
        install,
        progress,
        busy: Cell::new(false),
        done: Cell::new(false),
    });
    fill_list(&choice);
    connect(&choice);
    choice.window.present();
    // GTK would give the first radio button the focus, which shows it as picked.
    focus::let_fields_go(&choice.window);
}

fn layout(
    list: &gtk::ListBox,
    progress: &gtk::Box,
    skip: &gtk::Button,
    install: &gtk::Button,
) -> adw::ToolbarView {
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .margin_start(28)
        .margin_end(28)
        .margin_bottom(20)
        .build();
    content.append(
        &gtk::Label::builder()
            .label("選擇網頁瀏覽器")
            .xalign(0.0)
            .css_classes(["title-1"])
            .build(),
    );
    content.append(
        &gtk::Label::builder()
            .label("Nekoland 不附瀏覽器。清單順序每次隨機。")
            .xalign(0.0)
            .wrap(true)
            .css_classes(["dim-label"])
            .build(),
    );
    list.set_margin_top(12);
    content.append(list);
    content.append(progress);

    let footer = gtk::Box::builder()
        .spacing(8)
        .halign(gtk::Align::End)
        .margin_top(16)
        .build();
    footer.append(skip);
    footer.append(install);
    content.append(&footer);

    let header = adw::HeaderBar::builder()
        .title_widget(&gtk::Label::new(None))
        .build();
    let view = adw::ToolbarView::new();
    view.add_top_bar(&header);
    view.set_content(Some(&content));
    view
}

fn fill_list(choice: &Rc<Choice>) {
    let mut group: Option<gtk::CheckButton> = None;
    let mut rows = Vec::new();
    for (at, app) in choice.apps.iter().enumerate() {
        let row = adw::ActionRow::builder()
            .title(glib::markup_escape_text(&app.name))
            .subtitle(glib::markup_escape_text(&app.summary))
            .css_classes(["store-choice"])
            .build();
        row.add_prefix(&icon(app, ICON_SIZE));
        if let Some(offer) = app.offers.first() {
            row.add_suffix(&chip(&place_name(offer), false));
        }
        let radio = gtk::CheckButton::builder().valign(gtk::Align::Center).build();
        radio.set_group(group.as_ref());
        group.get_or_insert_with(|| radio.clone());
        radio.connect_toggled(glib::clone!(
            #[weak]
            choice,
            move |radio| {
                if radio.is_active() {
                    pick(&choice, at);
                }
            }
        ));
        row.add_suffix(&radio);
        row.set_activatable_widget(Some(&radio));
        choice.list.append(&row);
        rows.push(row);
    }
    *choice.rows.borrow_mut() = rows;
}

fn pick(choice: &Rc<Choice>, at: usize) {
    choice.picked.set(Some(at));
    for (index, row) in choice.rows.borrow().iter().enumerate() {
        if index == at {
            row.add_css_class("picked");
        } else {
            row.remove_css_class("picked");
        }
    }
    choice
        .install
        .set_label(&format!("安裝 {}", choice.apps[at].name));
    choice.install.set_sensitive(true);
}

fn connect(choice: &Rc<Choice>) {
    choice.skip.connect_clicked(glib::clone!(
        #[weak]
        choice,
        move |_| choice.window.close()
    ));
    choice.install.connect_clicked(glib::clone!(
        #[weak]
        choice,
        move |_| start(&choice)
    ));
    // The only strong hold on the choice, so that it lives as long as its window; every
    // other handler holds it weakly.
    choice.window.connect_close_request(glib::clone!(
        #[strong]
        choice,
        move |_| {
            // Quitting would leave the install half done.
            if choice.busy.get() {
                return glib::Propagation::Stop;
            }
            if !choice.done.get() {
                remember_skip();
            }
            glib::Propagation::Proceed
        }
    ));
}

fn start(choice: &Rc<Choice>) {
    let Some(offer) = choice
        .picked
        .get()
        .and_then(|at| choice.apps[at].offers.first().cloned())
    else {
        return;
    };
    match offer.repo {
        Some(vendor) => confirm_repository(
            &choice.window,
            vendor,
            glib::clone!(
                #[weak]
                choice,
                move || install(&choice, offer.clone())
            ),
        ),
        None => install(choice, offer),
    }
}

fn picked_name(choice: &Choice) -> String {
    choice
        .picked
        .get()
        .map(|at| choice.apps[at].name.clone())
        .unwrap_or_default()
}

fn install(choice: &Rc<Choice>, offer: sources::Offer) {
    let watch = sources::Watch::default();
    set_busy(choice, Some(watch.clone()));
    let name = picked_name(choice);
    let plan = sources::install_plan(&offer);
    let reason = format!("安裝 {name}");
    background(
        move || sources::run_watched(&plan, &reason, &watch),
        glib::clone!(
            #[weak]
            choice,
            move |result: Result<(), String>| match result {
                Ok(()) => finish(&choice, offer),
                Err(err) => {
                    set_busy(&choice, None);
                    // An empty error is the password dialog cancelled: nothing to say.
                    if !err.is_empty() {
                        alert::notice(&choice.window, &format!("無法安裝 {name}"), Some(&err));
                    }
                }
            }
        ),
    );
}

/// Shows the install following `watch` while it runs, or the choice again once it is not.
fn set_busy(choice: &Choice, watch: Option<sources::Watch>) {
    let busy = watch.is_some();
    choice.busy.set(busy);
    choice.list.set_sensitive(!busy);
    choice.skip.set_sensitive(!busy);
    choice.install.set_sensitive(!busy);
    while let Some(bar) = choice.progress.first_child() {
        choice.progress.remove(&bar);
    }
    if busy {
        let bar = gtk::ProgressBar::new();
        follow_progress(&bar, watch);
        choice.progress.append(&bar);
    }
    choice.progress.set_visible(busy);
}

/// Makes the installed browser the default and opens it, once its desktop entry is listed.
fn finish(choice: &Rc<Choice>, offer: sources::Offer) {
    choice.done.set(true);
    web::when_listed(
        move || browsers::entry_of(&offer),
        glib::clone!(
            #[weak]
            choice,
            move |found: Option<gio::AppInfo>| {
                match found {
                    Some(app) => {
                        web::make_default(&app);
                        if let Err(err) = app.launch(&[], None::<&gio::AppLaunchContext>) {
                            eprintln!("{}: {err}", app.display_name());
                        }
                    }
                    None => eprintln!("choice: the installed browser's desktop entry did not appear"),
                }
                choice.busy.set(false);
                choice.window.close();
            }
        ),
    );
}
