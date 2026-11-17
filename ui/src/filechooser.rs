//! `nekoland-filechooser`: the window programs open and save files in. It serves the
//! portal's FileChooser for `xdg-desktop-portal`, which asks it for every program that
//! chooses a file through the portal: browsers, Flatpak and Snap applications, GTK 4's file
//! dialog and Electron. D-Bus starts it. With NEKOLAND_FILECHOOSER_TRY set to `open`,
//! `save` or `folder` it shows one such window without the portal and prints what was chosen.

use std::{
    cell::{Cell, RefCell},
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
    rc::Rc,
};

use adw::prelude::*;
use browse::{home, icon_button, info_file, is_dir, modified_label, places, scrolled, Location, EDGE, GAP};
use gtk::{gdk, gio, glib};

mod alert;
mod browse;

const APP_ID: &str = "io.github.koukeneko.Nekoland.FileChooser";
const BUS_NAME: &str = "org.freedesktop.impl.portal.desktop.nekoland";
const OBJECT_PATH: &str = "/org/freedesktop/portal/desktop";
const INTERFACE: &str = "org.freedesktop.impl.portal.FileChooser";
const TRY_ENV: &str = "NEKOLAND_FILECHOOSER_TRY";

/// The portal's answers: chosen, cancelled, or ended some other way.
const RESPONSE_OK: u32 = 0;
const RESPONSE_CANCELLED: u32 = 1;
const RESPONSE_ENDED: u32 = 2;
/// A portal filter rule is a glob or a MIME type.
const RULE_GLOB: u32 = 0;
const RULE_MIME: u32 = 1;

/// What the window reads of each file.
const ATTRIBUTES: &str = "standard::name,standard::display-name,standard::type,standard::icon,\
standard::is-hidden,standard::is-backup,standard::content-type,standard::size,time::modified,thumbnail::path";

const SIDEBAR_WIDTH: i32 = 188;
/// The save window with only the name and the folder.
const COLLAPSED_WIDTH: i32 = 520;
/// The name a new folder is offered, as the desktop names one.
const NEW_FOLDER: &str = "未命名檔案夾";

const CSS: &str = "
.fc-sidebar { background-color: var(--sidebar-bg-color, @sidebar_bg_color); }
.fc-sidebar .navigation-sidebar { background: none; padding: 0; }
.fc-sidebar .navigation-sidebar > row { margin: 0; padding: 0 8px; }
.fc-crumb { min-height: 28px; padding: 0 6px; font-weight: 500; }
.fc-crumb.current { font-weight: 600; }
.fc-list > listview > row > cell, .fc-list > header > button { padding-left: 6px; padding-right: 6px; }
.fc-list > listview > row > cell:first-child, .fc-list > header > button:first-child { padding-left: 12px; }
.fc-list > listview > row > cell:last-child, .fc-list > header > button:last-child { padding-right: 12px; }
.fc-off { opacity: .38; }
.fc-thumb { border-radius: 8px; }
.fc-grid > child { padding: 6px; border-radius: 10px; }
.fc-status { font-size: 15px; }
";

fn interface_xml() -> String {
    let method = |name: &str| {
        format!(
            r#"<method name="{name}">
<arg type="o" name="handle" direction="in"/><arg type="s" name="app_id" direction="in"/>
<arg type="s" name="parent_window" direction="in"/><arg type="s" name="title" direction="in"/>
<arg type="a{{sv}}" name="options" direction="in"/>
<arg type="u" name="response" direction="out"/><arg type="a{{sv}}" name="results" direction="out"/>
</method>"#
        )
    };
    format!(
        r#"<node><interface name="{INTERFACE}">{}{}{}<property name="version" type="u" access="read"/></interface>
<interface name="org.freedesktop.impl.portal.Request"><method name="Close"/></interface></node>"#,
        method("OpenFile"),
        method("SaveFile"),
        method("SaveFiles"),
    )
}

fn main() -> glib::ExitCode {
    // GTK and libadwaita ask the portal for the dark or light setting as they start, and the
    // portal waits for this backend to start: a deadlock. They read GNOME's settings instead,
    // as xdg-desktop-portal-gtk has them do.
    std::env::set_var("GDK_DEBUG", "no-portals");
    std::env::set_var("ADW_DISABLE_PORTAL", "1");
    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.connect_startup(|_| {
        let css = gtk::CssProvider::new();
        css.load_from_string(CSS);
        if let Some(display) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &css,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
    });
    match std::env::var(TRY_ENV) {
        Ok(kind) => app.connect_activate(move |app| try_one(app, &kind)),
        Err(_) => app.connect_activate(serve),
    };
    app.run_with_args::<&str>(&[])
}

/// Takes the portal's name and answers its calls for as long as the session lasts.
fn serve(app: &adw::Application) {
    let hold = RefCell::new(Some(app.hold()));
    let quit = app.downgrade();
    let serving = app.downgrade();
    gio::bus_own_name(
        gio::BusType::Session,
        BUS_NAME,
        gio::BusNameOwnerFlags::NONE,
        move |connection, _| {
            if let Some(app) = serving.upgrade() {
                if let Err(err) = register(&connection, &app) {
                    eprintln!("nekoland-filechooser: cannot serve {INTERFACE}: {err}");
                }
            }
        },
        |_, _| {},
        move |_, name| {
            eprintln!("nekoland-filechooser: lost or could not take {name}");
            drop(hold.take());
            if let Some(app) = quit.upgrade() {
                app.quit();
            }
        },
    );
}

fn register(connection: &gio::DBusConnection, app: &adw::Application) -> Result<(), glib::Error> {
    let node = gio::DBusNodeInfo::for_xml(&interface_xml())?;
    let chooser = node.lookup_interface(INTERFACE).expect("the XML above has it");
    let app = app.downgrade();
    connection
        .register_object(OBJECT_PATH, &chooser)
        .method_call(move |connection, _, _, _, method, params, invocation| {
            let mode = match method {
                "OpenFile" => Mode::Open,
                "SaveFile" => Mode::Save,
                "SaveFiles" => Mode::SaveFiles,
                _ => {
                    invocation.return_error(gio::DBusError::UnknownMethod, method);
                    return;
                }
            };
            let (Some(handle), Some(title), Some(app)) = (
                params.child_value(0).str().map(str::to_owned),
                params.child_value(3).str().map(str::to_owned),
                app.upgrade(),
            ) else {
                invocation.return_error(gio::DBusError::InvalidArgs, "expected (osssa{sv})");
                return;
            };
            let request = Request::parse(mode, title, &params.child_value(4));
            answer(&connection, &app, &node, handle, request, invocation);
        })
        .property(|_, _, _, _, _| 4u32.to_variant())
        .build()?;
    Ok(())
}

/// Shows the window a call asks for, with the Request object the portal closes it through,
/// and answers the call when it is done.
fn answer(
    connection: &gio::DBusConnection,
    app: &adw::Application,
    node: &gio::DBusNodeInfo,
    handle: String,
    request: Request,
    invocation: gio::DBusMethodInvocation,
) {
    let registration = Rc::new(RefCell::new(None));
    let done = {
        let connection = connection.clone();
        let registration = registration.clone();
        move |response: u32, results: glib::Variant| {
            invocation.return_value(Some(&glib::Variant::tuple_from_iter([
                response.to_variant(),
                results,
            ])));
            if let Some(id) = registration.borrow_mut().take() {
                let _ = connection.unregister_object(id);
            }
        }
    };
    let dialog = Dialog::open(app, request, Box::new(done));
    let closing = Rc::downgrade(&dialog);
    let request_interface = node
        .lookup_interface("org.freedesktop.impl.portal.Request")
        .expect("the XML above has it");
    match connection
        .register_object(&handle, &request_interface)
        .method_call(move |_, _, _, _, _, _, invocation| {
            invocation.return_value(None);
            if let Some(dialog) = closing.upgrade() {
                dialog.finish(RESPONSE_ENDED, Vec::new());
            }
        })
        .build()
    {
        Ok(id) => *registration.borrow_mut() = Some(id),
        // The portal can still be answered; it only cannot close the window early.
        Err(err) => eprintln!("nekoland-filechooser: no Request object at {handle}: {err}"),
    }
}

/// One window without the portal, to try it: prints the chosen URIs.
fn try_one(app: &adw::Application, kind: &str) {
    let pdf = Filter {
        name: "PDF 文件".into(),
        rules: vec![(RULE_GLOB, "*.pdf".into())],
    };
    let any = Filter {
        name: "所有檔案".into(),
        rules: vec![(RULE_GLOB, "*".into())],
    };
    let png = Filter {
        name: "PNG 圖片".into(),
        rules: vec![(RULE_MIME, "image/png".into()), (RULE_GLOB, "*.png".into())],
    };
    let jpeg = Filter {
        name: "JPEG 圖片".into(),
        rules: vec![(RULE_GLOB, "*.jpg".into())],
    };
    let mut request = Request::parse(Mode::Open, "打開".into(), &glib::VariantDict::new(None).end());
    match kind {
        "save" => {
            request.mode = Mode::Save;
            request.title = "儲存".into();
            request.filters = vec![png, jpeg];
            request.current_name = "海報.png".into();
        }
        "folder" => request.directory = true,
        _ => {
            request.multiple = true;
            request.filters = vec![pdf, any];
            request.choices = vec![Choice {
                id: "readonly".into(),
                label: "唯讀打開".into(),
                options: Vec::new(),
                default: "false".into(),
            }];
        }
    }
    Dialog::open(
        app,
        request,
        Box::new(|response, results| println!("{response} {}", results.print(true))),
    );
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Mode {
    Open,
    Save,
    SaveFiles,
}

#[derive(Clone, PartialEq, Debug)]
struct Filter {
    name: String,
    rules: Vec<(u32, String)>,
}

impl Filter {
    fn file_filter(&self) -> gtk::FileFilter {
        let filter = gtk::FileFilter::new();
        for (kind, pattern) in &self.rules {
            match *kind {
                RULE_MIME => filter.add_mime_type(pattern),
                _ => filter.add_pattern(pattern),
            }
        }
        filter
    }
}

#[derive(Clone, PartialEq, Debug)]
struct Choice {
    id: String,
    label: String,
    /// A drop-down's (id, label) options; none makes it a check box of "true" or "false".
    options: Vec<(String, String)>,
    default: String,
}

/// What a call asks for, from its options, as the portal documents them.
#[derive(Debug)]
struct Request {
    mode: Mode,
    title: String,
    accept_label: Option<String>,
    multiple: bool,
    directory: bool,
    filters: Vec<Filter>,
    current_filter: Option<Filter>,
    choices: Vec<Choice>,
    current_name: String,
    current_folder: Option<PathBuf>,
    current_file: Option<PathBuf>,
    /// The names SaveFiles saves into the chosen folder.
    files: Vec<String>,
}

impl Request {
    fn parse(mode: Mode, title: String, options: &glib::Variant) -> Request {
        let dict = glib::VariantDict::new(Some(options));
        let path = |key: &str| {
            dict.lookup::<Vec<u8>>(key)
                .ok()
                .flatten()
                .map(|bytes| bytes_to_path(&bytes))
                .filter(|path| !path.as_os_str().is_empty())
        };
        let filter = |(name, rules): (String, Vec<(u32, String)>)| Filter { name, rules };
        let mut filters: Vec<Filter> = dict
            .lookup::<Vec<(String, Vec<(u32, String)>)>>("filters")
            .ok()
            .flatten()
            .unwrap_or_default()
            .into_iter()
            .map(filter)
            .collect();
        let current_filter = dict
            .lookup::<(String, Vec<(u32, String)>)>("current_filter")
            .ok()
            .flatten()
            .map(filter);
        // The portal allows a current filter that is not among the filters, alone.
        if let Some(current) = &current_filter {
            if !filters.contains(current) {
                filters.insert(0, current.clone());
            }
        }
        let choices = dict
            .lookup::<Vec<(String, String, Vec<(String, String)>, String)>>("choices")
            .ok()
            .flatten()
            .unwrap_or_default()
            .into_iter()
            .map(|(id, label, options, default)| Choice {
                id,
                label,
                options,
                default,
            })
            .collect();
        Request {
            mode,
            title,
            accept_label: dict
                .lookup::<String>("accept_label")
                .ok()
                .flatten()
                .filter(|l| !l.is_empty()),
            multiple: dict.lookup::<bool>("multiple").ok().flatten().unwrap_or(false),
            directory: dict.lookup::<bool>("directory").ok().flatten().unwrap_or(false),
            filters,
            current_filter,
            choices,
            current_name: dict
                .lookup::<String>("current_name")
                .ok()
                .flatten()
                .unwrap_or_default(),
            current_folder: path("current_folder"),
            current_file: path("current_file"),
            files: dict
                .lookup::<Vec<Vec<u8>>>("files")
                .ok()
                .flatten()
                .unwrap_or_default()
                .iter()
                .filter_map(|bytes| {
                    bytes_to_path(bytes)
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                })
                .collect(),
        }
    }

    /// Whether what is chosen is a folder rather than files.
    fn picks_folder(&self) -> bool {
        self.mode == Mode::SaveFiles || (self.mode == Mode::Open && self.directory)
    }
}

/// A portal byte string, which ends in a NUL, as a path.
fn bytes_to_path(bytes: &[u8]) -> PathBuf {
    let bytes = bytes.strip_suffix(&[0]).unwrap_or(bytes);
    PathBuf::from(std::ffi::OsStr::from_bytes(bytes))
}

/// `name` with the extension of the filter's first plain `*.ext` glob, as a save dialog
/// changes it when another type is chosen; None when the filter names no such extension.
fn with_extension(name: &str, filter: &Filter) -> Option<String> {
    let ext = filter.rules.iter().find_map(|(kind, pattern)| {
        (*kind == RULE_GLOB)
            .then(|| pattern.strip_prefix("*."))
            .flatten()
            .filter(|ext| !ext.is_empty() && !ext.contains(['*', '?', '[', '.']))
    })?;
    let stem = match name.rfind('.') {
        Some(dot) if dot > 0 => &name[..dot],
        _ => name,
    };
    Some(format!("{stem}.{ext}"))
}

/// How many characters of a name come before its extension, to select it for typing over.
fn stem_chars(name: &str) -> i32 {
    let stem = match name.rfind('.') {
        Some(dot) if dot > 0 => &name[..dot],
        _ => name,
    };
    stem.chars().count() as i32
}

/// What the window remembers between uses.
#[derive(Default)]
struct Prefs {
    grid: bool,
    hidden: bool,
    expanded: bool,
}

impl Prefs {
    fn path() -> PathBuf {
        glib::user_config_dir().join("nekoland/file-chooser.ini")
    }

    fn load() -> Prefs {
        let file = glib::KeyFile::new();
        if file
            .load_from_file(Self::path(), glib::KeyFileFlags::NONE)
            .is_err()
        {
            return Prefs::default();
        }
        let get = |key: &str| file.boolean("chooser", key).unwrap_or(false);
        Prefs {
            grid: get("grid"),
            hidden: get("hidden"),
            expanded: get("expanded"),
        }
    }

    fn save(&self) {
        let file = glib::KeyFile::new();
        file.set_boolean("chooser", "grid", self.grid);
        file.set_boolean("chooser", "hidden", self.hidden);
        file.set_boolean("chooser", "expanded", self.expanded);
        let path = Self::path();
        let written = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .map_err(|err| err.to_string())
            .and_then(|()| file.save_to_file(&path).map_err(|err| err.to_string()));
        if let Err(err) = written {
            eprintln!("nekoland-filechooser: cannot save {}: {err}", path.display());
        }
    }
}

enum ChoiceWidget {
    Check(gtk::CheckButton),
    Pick(gtk::DropDown, Vec<String>),
}

impl ChoiceWidget {
    fn value(&self) -> String {
        match self {
            ChoiceWidget::Check(check) => check.is_active().to_string(),
            ChoiceWidget::Pick(drop_down, ids) => ids
                .get(drop_down.selected() as usize)
                .cloned()
                .unwrap_or_default(),
        }
    }
}

type Reply = Box<dyn FnOnce(u32, glib::Variant)>;

struct Dialog {
    request: Request,
    window: adw::Window,
    reply: RefCell<Option<Reply>>,
    prefs: RefCell<Prefs>,
    here: RefCell<Location>,
    back: RefCell<Vec<Location>>,
    forward: RefCell<Vec<Location>>,
    directory: gtk::DirectoryList,
    recent: gio::ListStore,
    shown: gtk::CustomFilter,
    filtered: gtk::FilterListModel,
    selection: gtk::SelectionModel,
    /// The type chosen at the bottom: what does not match it is shown faded.
    filter: RefCell<Option<gtk::FileFilter>>,
    filter_index: Cell<usize>,
    places: gtk::ListBox,
    place_locations: RefCell<Vec<Location>>,
    crumbs: gtk::Box,
    path_stack: gtk::Stack,
    path_entry: gtk::Entry,
    back_button: gtk::Button,
    forward_button: gtk::Button,
    search: gtk::SearchEntry,
    views: gtk::Stack,
    status: gtk::Label,
    browser: gtk::Revealer,
    expand: gtk::Button,
    name: gtk::Entry,
    location_row: Vec<gtk::Widget>,
    location: gtk::DropDown,
    location_files: RefCell<Vec<gio::File>>,
    /// Set while the location drop-down is rebuilt, which must not navigate.
    syncing: Cell<bool>,
    new_folder: gtk::Button,
    accept: gtk::Button,
    choices: Vec<(String, ChoiceWidget)>,
}

impl Dialog {
    fn open(app: &adw::Application, request: Request, reply: Reply) -> Rc<Dialog> {
        let prefs = Prefs::load();
        let save = request.mode == Mode::Save;

        let directory = gtk::DirectoryList::new(Some(ATTRIBUTES), None::<&gio::File>);
        let recent = gio::ListStore::new::<gio::FileInfo>();
        let shown = gtk::CustomFilter::new(|_| true);
        let filtered = gtk::FilterListModel::new(None::<gio::ListModel>, Some(shown.clone()));

        let column_view = gtk::ColumnView::builder().show_column_separators(false).build();
        column_view.add_css_class("fc-list");
        let folders_first = gtk::CustomSorter::new(|a, b| {
            let dir = |o: &glib::Object| o.downcast_ref::<gio::FileInfo>().is_some_and(is_dir);
            dir(b).cmp(&dir(a)).into()
        });
        let sorter = gtk::MultiSorter::new();
        sorter.append(folders_first);
        let sorted = gtk::SortListModel::new(Some(filtered.clone()), Some(sorter.clone()));
        let selection: gtk::SelectionModel = if request.mode == Mode::Open && request.multiple {
            gtk::MultiSelection::new(Some(sorted)).upcast()
        } else {
            let single = gtk::SingleSelection::new(Some(sorted));
            single.set_autoselect(false);
            single.set_can_unselect(true);
            single.upcast()
        };
        column_view.set_model(Some(&selection));
        let grid_view = gtk::GridView::builder()
            .model(&selection)
            .min_columns(3)
            .max_columns(12)
            .css_classes(["fc-grid"])
            .build();

        let window = adw::Window::builder()
            .application(app)
            .title(&request.title)
            .default_width(if save && !prefs.expanded {
                COLLAPSED_WIDTH
            } else {
                780
            })
            .default_height(match (save, prefs.expanded) {
                (false, _) => 500,
                (true, true) => 560,
                (true, false) => -1,
            })
            // libadwaita's windows are at least 200 high, more than the folded save window.
            .height_request(0)
            .width_request(360)
            .build();

        // The path bar.
        let back_button = icon_button("go-previous-symbolic", "上一頁");
        let forward_button = icon_button("go-next-symbolic", "下一頁");
        let crumbs = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        let path_entry = gtk::Entry::builder().hexpand(true).build();
        let path_stack = gtk::Stack::builder().hexpand(true).hhomogeneous(false).build();
        path_stack.add_named(
            &gtk::ScrolledWindow::builder()
                .child(&crumbs)
                .vscrollbar_policy(gtk::PolicyType::Never)
                .hscrollbar_policy(gtk::PolicyType::External)
                .build(),
            Some("crumbs"),
        );
        path_stack.add_named(&path_entry, Some("entry"));
        let list_toggle = gtk::ToggleButton::builder()
            .icon_name("view-list-symbolic")
            .tooltip_text("列表")
            .build();
        let grid_toggle = gtk::ToggleButton::builder()
            .icon_name("view-grid-symbolic")
            .tooltip_text("圖示")
            .group(&list_toggle)
            .build();
        list_toggle.set_active(!prefs.grid);
        grid_toggle.set_active(prefs.grid);
        let toggles = gtk::Box::builder().css_classes(["linked"]).build();
        toggles.append(&list_toggle);
        toggles.append(&grid_toggle);
        let search = gtk::SearchEntry::builder()
            .placeholder_text("搜尋")
            .width_chars(14)
            .build();
        let bar = gtk::Box::builder()
            .spacing(GAP)
            .margin_top(EDGE)
            .margin_bottom(EDGE)
            .margin_start(EDGE)
            .margin_end(EDGE)
            .build();
        let arrows = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        arrows.append(&back_button);
        arrows.append(&forward_button);
        bar.append(&arrows);
        bar.append(&path_stack);
        bar.append(&toggles);
        bar.append(&search);

        // The files, as a list or a grid, or why there are none.
        let status = gtk::Label::builder()
            .css_classes(["dim-label", "fc-status"])
            .build();
        let views = gtk::Stack::builder()
            .vexpand(true)
            .hexpand(true)
            .css_classes(["view"])
            .build();
        views.add_named(&scrolled(&column_view), Some("list"));
        views.add_named(&scrolled(&grid_view), Some("grid"));
        views.add_named(&status, Some("status"));

        let main = gtk::Box::new(gtk::Orientation::Vertical, 0);
        main.append(&gtk::WindowHandle::builder().child(&bar).build());
        main.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        main.append(&views);

        let places = gtk::ListBox::builder()
            .css_classes(["navigation-sidebar"])
            .margin_top(EDGE)
            .margin_bottom(EDGE)
            .margin_start(EDGE)
            .margin_end(EDGE)
            .build();
        let sidebar = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .width_request(SIDEBAR_WIDTH)
            .css_classes(["fc-sidebar"])
            .child(&places)
            .build();
        let split = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        split.append(&sidebar);
        split.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        split.append(&main);
        let browser_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        if save {
            browser_box.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        }
        browser_box.append(&split);
        browser_box.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        let browser = gtk::Revealer::builder()
            .child(&browser_box)
            .transition_type(gtk::RevealerTransitionType::SlideDown)
            .transition_duration(220)
            .reveal_child(!save || prefs.expanded)
            .vexpand(true)
            .build();

        // Saving: the name, and where when the browser is folded away.
        let name = gtk::Entry::builder()
            .hexpand(true)
            .activates_default(true)
            .text(&request.current_name)
            .build();
        let location = gtk::DropDown::builder().hexpand(true).build();
        let expand = icon_button("pan-down-symbolic", "展開");
        let form = gtk::Grid::builder()
            .column_spacing(GAP)
            .row_spacing(GAP)
            .margin_top(EDGE)
            .margin_bottom(EDGE)
            .margin_start(EDGE)
            .margin_end(EDGE)
            .build();
        let label = |text: &str| {
            gtk::Label::builder()
                .label(text)
                .xalign(1.0)
                .css_classes(["dim-label"])
                .build()
        };
        let where_label = label("位置");
        form.attach(&label("名稱"), 0, 0, 1, 1);
        form.attach(&name, 1, 0, 1, 1);
        form.attach(&expand, 2, 0, 1, 1);
        form.attach(&where_label, 0, 1, 1, 1);
        form.attach(&location, 1, 1, 1, 1);

        // The bottom: a new folder, the types, the program's own choices, and the answer.
        let new_folder = gtk::Button::builder()
            .label("新增檔案夾")
            .css_classes(["flat"])
            .build();
        let filter_pick = gtk::DropDown::from_strings(
            &request
                .filters
                .iter()
                .map(|f| f.name.as_str())
                .collect::<Vec<_>>(),
        );
        filter_pick.set_visible(!request.filters.is_empty() && request.mode != Mode::SaveFiles);
        let footer = gtk::Box::builder()
            .spacing(GAP)
            .margin_top(EDGE)
            .margin_bottom(EDGE)
            .margin_start(EDGE)
            .margin_end(EDGE)
            .build();
        footer.append(&new_folder);
        footer.append(&filter_pick);
        let mut choices = Vec::new();
        for choice in &request.choices {
            let widget = if choice.options.is_empty() {
                let check = gtk::CheckButton::builder()
                    .label(&choice.label)
                    .active(choice.default == "true")
                    .build();
                footer.append(&check);
                ChoiceWidget::Check(check)
            } else {
                let labels: Vec<&str> = choice.options.iter().map(|(_, l)| l.as_str()).collect();
                let pick = gtk::DropDown::from_strings(&labels);
                let ids: Vec<String> = choice.options.iter().map(|(id, _)| id.clone()).collect();
                if let Some(index) = ids.iter().position(|id| *id == choice.default) {
                    pick.set_selected(index as u32);
                }
                footer.append(
                    &gtk::Label::builder()
                        .label(&choice.label)
                        .css_classes(["dim-label"])
                        .build(),
                );
                footer.append(&pick);
                ChoiceWidget::Pick(pick, ids)
            };
            choices.push((choice.id.clone(), widget));
        }
        footer.append(&gtk::Box::builder().hexpand(true).build());
        let cancel = gtk::Button::with_label("取消");
        let default_label = match request.mode {
            Mode::Open if request.directory => "選擇",
            Mode::Open => "打開",
            Mode::Save | Mode::SaveFiles => "儲存",
        };
        let accept = gtk::Button::builder()
            .label(request.accept_label.as_deref().unwrap_or(default_label))
            .use_underline(true)
            .css_classes(["suggested-action"])
            .build();
        footer.append(&cancel);
        footer.append(&accept);

        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        if save {
            root.append(&gtk::WindowHandle::builder().child(&form).build());
        }
        root.append(&browser);
        root.append(&gtk::WindowHandle::builder().child(&footer).build());
        window.set_content(Some(&root));
        window.set_default_widget(Some(&accept));

        let filter_index = request
            .current_filter
            .as_ref()
            .and_then(|current| request.filters.iter().position(|f| f == current))
            .unwrap_or(0);
        filter_pick.set_selected(filter_index as u32);

        let dialog = Rc::new(Dialog {
            filter: RefCell::new(request.filters.get(filter_index).map(Filter::file_filter)),
            filter_index: Cell::new(filter_index),
            request,
            window,
            reply: RefCell::new(Some(reply)),
            prefs: RefCell::new(prefs),
            here: RefCell::new(Location::Recent),
            back: RefCell::new(Vec::new()),
            forward: RefCell::new(Vec::new()),
            directory,
            recent,
            shown,
            filtered,
            selection,
            places,
            place_locations: RefCell::new(Vec::new()),
            crumbs,
            path_stack,
            path_entry,
            back_button,
            forward_button,
            search,
            views,
            status,
            browser,
            expand,
            name,
            location_row: vec![where_label.upcast(), location.clone().upcast()],
            location,
            location_files: RefCell::new(Vec::new()),
            syncing: Cell::new(false),
            new_folder,
            accept,
            choices,
        });
        dialog.add_columns(&column_view, &sorter);
        dialog.set_up_grid(&grid_view);
        dialog.connect(
            &column_view,
            &grid_view,
            &list_toggle,
            &grid_toggle,
            &filter_pick,
            &cancel,
        );
        dialog.fill_places();
        dialog.show_expanded(dialog.browser.reveals_child());
        dialog.update_status();

        let start = dialog.start_location();
        dialog.go(start, false);
        if let (Mode::Save, Some(file)) = (dialog.request.mode, &dialog.request.current_file) {
            if let Some(name) = file.file_name() {
                dialog.name.set_text(&name.to_string_lossy());
            }
        }
        // Everything else holds the dialog weakly; the window keeps it while it is open.
        let kept = RefCell::new(Some(dialog.clone()));
        dialog.window.connect_destroy(move |_| drop(kept.take()));
        // Focused before it is shown, or the sidebar takes the focus and chooses its first place.
        if save {
            dialog.name.grab_focus();
        } else {
            column_view.grab_focus();
        }
        dialog.window.present();
        if save {
            let stem = stem_chars(&dialog.name.text());
            dialog.name.select_region(0, stem);
        }
        dialog
    }

    fn start_location(&self) -> Location {
        let folder = self
            .request
            .current_file
            .as_ref()
            .and_then(|file| file.parent().map(Path::to_path_buf))
            .or_else(|| self.request.current_folder.clone())
            .filter(|path| path.is_dir());
        match folder {
            Some(path) => Location::Folder(gio::File::for_path(path)),
            // Opening starts with what was used last; saving needs a folder.
            None if self.request.mode == Mode::Open && !self.request.directory => Location::Recent,
            None => Location::Folder(gio::File::for_path(
                glib::user_special_dir(glib::UserDirectory::Documents)
                    .filter(|p| p.is_dir())
                    .unwrap_or_else(home),
            )),
        }
    }

    fn add_columns(self: &Rc<Self>, view: &gtk::ColumnView, sorter: &gtk::MultiSorter) {
        let name_factory = self.cell_factory(
            || {
                let line = gtk::Box::new(gtk::Orientation::Horizontal, GAP);
                line.append(&gtk::Image::builder().pixel_size(16).build());
                line.append(
                    &gtk::Label::builder()
                        .xalign(0.0)
                        .ellipsize(gtk::pango::EllipsizeMode::Middle)
                        .build(),
                );
                line.upcast()
            },
            |widget, info| {
                let image = widget
                    .first_child()
                    .and_downcast::<gtk::Image>()
                    .expect("set up above");
                let label = widget
                    .last_child()
                    .and_downcast::<gtk::Label>()
                    .expect("set up above");
                image.set_from_gicon(
                    &info
                        .icon()
                        .unwrap_or_else(|| gio::ThemedIcon::new("text-x-generic").upcast()),
                );
                label.set_label(&info.display_name());
            },
        );
        let meta = || {
            gtk::Label::builder()
                .xalign(1.0)
                .css_classes(["dim-label", "numeric"])
                .build()
                .upcast::<gtk::Widget>()
        };
        let modified_factory = self.cell_factory(meta, |widget, info| {
            let label = widget.downcast_ref::<gtk::Label>().expect("set up above");
            let when = info.modification_date_time().and_then(|t| t.to_local().ok());
            let now = glib::DateTime::now_local().ok();
            label.set_label(&match (when, now) {
                (Some(when), Some(now)) => modified_label(&when, &now),
                _ => "—".into(),
            });
        });
        let size_factory = self.cell_factory(meta, |widget, info| {
            let label = widget.downcast_ref::<gtk::Label>().expect("set up above");
            label.set_label(&if is_dir(info) {
                "—".into()
            } else {
                glib::format_size(info.size() as u64).to_string()
            });
        });

        let by = |key: fn(&gio::FileInfo, &gio::FileInfo) -> std::cmp::Ordering| {
            gtk::CustomSorter::new(move |a, b| {
                match (
                    a.downcast_ref::<gio::FileInfo>(),
                    b.downcast_ref::<gio::FileInfo>(),
                ) {
                    (Some(a), Some(b)) => key(a, b).into(),
                    _ => gtk::Ordering::Equal,
                }
            })
        };
        let name_column = gtk::ColumnViewColumn::builder()
            .title("名稱")
            .factory(&name_factory)
            .expand(true)
            .sorter(&by(|a, b| {
                a.display_name()
                    .to_lowercase()
                    .cmp(&b.display_name().to_lowercase())
            }))
            .build();
        let modified_column = gtk::ColumnViewColumn::builder()
            .title("修改日期")
            .factory(&modified_factory)
            .fixed_width(140)
            .sorter(&by(|a, b| {
                let time = |i: &gio::FileInfo| i.modification_date_time().map(|t| t.to_unix()).unwrap_or(0);
                time(a).cmp(&time(b))
            }))
            .build();
        let size_column = gtk::ColumnViewColumn::builder()
            .title("大小")
            .factory(&size_factory)
            .fixed_width(96)
            .sorter(&by(|a, b| a.size().cmp(&b.size())))
            .build();
        view.append_column(&name_column);
        view.append_column(&modified_column);
        view.append_column(&size_column);
        if let Some(column_sorter) = view.sorter() {
            sorter.append(column_sorter);
        }
        view.sort_by_column(Some(&name_column), gtk::SortType::Ascending);

        // What does not match the chosen type can be seen but not chosen.
        let rows = gtk::SignalListItemFactory::new();
        let dialog = Rc::downgrade(self);
        rows.connect_bind(move |_, row| {
            let (Some(row), Some(dialog)) = (row.downcast_ref::<gtk::ColumnViewRow>(), dialog.upgrade())
            else {
                return;
            };
            let ok = row
                .item()
                .and_downcast::<gio::FileInfo>()
                .is_some_and(|info| dialog.choosable(&info));
            row.set_selectable(ok);
            row.set_activatable(ok);
        });
        view.set_row_factory(Some(&rows));
    }

    /// A column's cells: `setup` makes one, `bind` shows a file in it, faded when it cannot be chosen.
    fn cell_factory(
        self: &Rc<Self>,
        setup: impl Fn() -> gtk::Widget + 'static,
        bind: impl Fn(&gtk::Widget, &gio::FileInfo) + 'static,
    ) -> gtk::SignalListItemFactory {
        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup(move |_, item| {
            if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
                item.set_child(Some(&setup()));
            }
        });
        let dialog = Rc::downgrade(self);
        factory.connect_bind(move |_, item| {
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let (Some(widget), Some(info), Some(dialog)) = (
                item.child(),
                item.item().and_downcast::<gio::FileInfo>(),
                dialog.upgrade(),
            ) else {
                return;
            };
            bind(&widget, &info);
            if dialog.choosable(&info) {
                widget.remove_css_class("fc-off");
            } else {
                widget.add_css_class("fc-off");
            }
        });
        factory
    }

    fn set_up_grid(self: &Rc<Self>, view: &gtk::GridView) {
        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup(|_, item| {
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let tile = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(GAP / 2)
                .width_request(96)
                .build();
            let picture = gtk::Picture::builder()
                .content_fit(gtk::ContentFit::Cover)
                .width_request(64)
                .height_request(64)
                .halign(gtk::Align::Center)
                .css_classes(["fc-thumb"])
                .overflow(gtk::Overflow::Hidden)
                .build();
            let image = gtk::Image::builder().pixel_size(64).build();
            let label = gtk::Label::builder()
                .wrap(true)
                .wrap_mode(gtk::pango::WrapMode::WordChar)
                .lines(2)
                .ellipsize(gtk::pango::EllipsizeMode::Middle)
                .justify(gtk::Justification::Center)
                .max_width_chars(12)
                .build();
            tile.append(&picture);
            tile.append(&image);
            tile.append(&label);
            item.set_child(Some(&tile));
        });
        let dialog = Rc::downgrade(self);
        factory.connect_bind(move |_, item| {
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let (Some(tile), Some(info), Some(dialog)) = (
                item.child(),
                item.item().and_downcast::<gio::FileInfo>(),
                dialog.upgrade(),
            ) else {
                return;
            };
            let picture = tile
                .first_child()
                .and_downcast::<gtk::Picture>()
                .expect("set up above");
            let image = picture
                .next_sibling()
                .and_downcast::<gtk::Image>()
                .expect("set up above");
            let label = tile
                .last_child()
                .and_downcast::<gtk::Label>()
                .expect("set up above");
            // shortcut: thumbnails already made by Files or tumbler, loaded as the tile is shown; no thumbnailer of its own.
            let thumbnail = info
                .attribute_byte_string("thumbnail::path")
                .filter(|p| Path::new(p.as_str()).exists());
            picture.set_filename(thumbnail.as_deref());
            picture.set_visible(thumbnail.is_some());
            image.set_visible(thumbnail.is_none());
            image.set_from_gicon(
                &info
                    .icon()
                    .unwrap_or_else(|| gio::ThemedIcon::new("text-x-generic").upcast()),
            );
            label.set_label(&info.display_name());
            let ok = dialog.choosable(&info);
            tile.set_opacity(if ok { 1.0 } else { 0.38 });
            item.set_selectable(ok);
            item.set_activatable(ok);
        });
        view.set_factory(Some(&factory));
    }

    fn connect(
        self: &Rc<Self>,
        column_view: &gtk::ColumnView,
        grid_view: &gtk::GridView,
        list_toggle: &gtk::ToggleButton,
        grid_toggle: &gtk::ToggleButton,
        filter_pick: &gtk::DropDown,
        cancel: &gtk::Button,
    ) {
        // A handler that holds the window weakly, as everything it is connected to is the window's.
        macro_rules! on {
            (|$d:ident $(, $arg:ident)*| $body:expr) => {{
                let weak = Rc::downgrade(self);
                move |$($arg),*| {
                    if let Some($d) = weak.upgrade() {
                        $body;
                    }
                }
            }};
        }

        {
            let weak = Rc::downgrade(self);
            self.shown.set_filter_func(move |item| {
                let (Some(info), Some(dialog)) = (item.downcast_ref::<gio::FileInfo>(), weak.upgrade())
                else {
                    return false;
                };
                dialog.shows(info)
            });
        }
        self.filtered
            .connect_items_changed(on!(|d, _m, _p, _r, _a| d.update_status()));
        self.directory
            .connect_loading_notify(on!(|d, _l| d.update_status()));
        self.directory
            .connect_error_notify(on!(|d, _l| d.update_status()));
        self.selection
            .connect_selection_changed(on!(|d, _m, _p, _n| d.selected_changed()));

        column_view.connect_activate(on!(|d, _v, position| d.activate(position)));
        grid_view.connect_activate(on!(|d, _v, position| d.activate(position)));
        list_toggle.connect_toggled(on!(|d, button| if button.is_active() {
            d.prefs.borrow_mut().grid = false;
            d.prefs.borrow().save();
            d.update_status();
        }));
        grid_toggle.connect_toggled(on!(|d, button| if button.is_active() {
            d.prefs.borrow_mut().grid = true;
            d.prefs.borrow().save();
            d.update_status();
        }));
        self.search
            .connect_search_changed(on!(|d, _e| d.shown.changed(gtk::FilterChange::Different)));
        self.back_button.connect_clicked(on!(|d, _b| d.step(-1)));
        self.forward_button.connect_clicked(on!(|d, _b| d.step(1)));
        self.places.connect_row_activated(on!(|d, _l, row| {
            let location = d.place_locations.borrow().get(row.index() as usize).cloned();
            if let Some(location) = location {
                d.go(location, true);
            }
        }));
        self.path_entry
            .connect_activate(on!(|d, entry| d.go_to_typed(&entry.text())));
        filter_pick.connect_selected_notify(on!(|d, pick| d.choose_filter(pick.selected() as usize)));
        self.location.connect_selected_notify(on!(|d, pick| {
            if !d.syncing.get() {
                let file = d.location_files.borrow().get(pick.selected() as usize).cloned();
                if let Some(file) = file {
                    d.go(Location::Folder(file), true);
                }
            }
        }));
        self.expand.connect_clicked(on!(|d, _b| {
            let expanded = !d.browser.reveals_child();
            d.prefs.borrow_mut().expanded = expanded;
            d.prefs.borrow().save();
            d.show_expanded(expanded);
        }));
        // Folded, the window shrinks to what is left once the browser has slid away.
        self.browser.connect_child_revealed_notify(on!(|d, revealer| {
            if d.request.mode == Mode::Save && !revealer.is_child_revealed() {
                if let Some(content) = d.window.content() {
                    let (_, height, _, _) = content.measure(gtk::Orientation::Vertical, COLLAPSED_WIDTH);
                    d.window.set_default_size(COLLAPSED_WIDTH, height);
                }
            }
        }));
        self.name.connect_changed(on!(|d, _e| d.update_accept()));
        self.new_folder.connect_clicked(on!(|d, _b| d.ask_new_folder()));
        cancel.connect_clicked(on!(|d, _b| d.finish(RESPONSE_CANCELLED, Vec::new())));
        self.accept.connect_clicked(on!(|d, _b| d.accept()));
        {
            let weak = Rc::downgrade(self);
            self.window.connect_close_request(move |_| {
                if let Some(dialog) = weak.upgrade() {
                    dialog.reply(RESPONSE_CANCELLED, Vec::new());
                }
                glib::Propagation::Proceed
            });
        }

        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, modifiers| {
            let Some(d) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            // A question asked over the window takes its own keys.
            if d.window.visible_dialog().is_some() {
                return glib::Propagation::Proceed;
            }
            let ctrl = modifiers.contains(gdk::ModifierType::CONTROL_MASK);
            let alt = modifiers.contains(gdk::ModifierType::ALT_MASK);
            let typing = gtk::prelude::GtkWindowExt::focus(&d.window)
                .is_some_and(|w| w.ancestor(gtk::Editable::static_type()).is_some() || w.is::<gtk::Text>());
            match key {
                gdk::Key::Escape if d.path_stack.visible_child_name().as_deref() == Some("entry") => {
                    d.show_path_entry(None)
                }
                gdk::Key::Escape => d.finish(RESPONSE_CANCELLED, Vec::new()),
                gdk::Key::l if ctrl => d.show_path_entry(Some("")),
                gdk::Key::h if ctrl => {
                    let hidden = !d.prefs.borrow().hidden;
                    d.prefs.borrow_mut().hidden = hidden;
                    d.prefs.borrow().save();
                    d.shown.changed(gtk::FilterChange::Different);
                }
                gdk::Key::Left if alt => d.step(-1),
                gdk::Key::Right if alt => d.step(1),
                gdk::Key::Up if alt => d.go_up(),
                gdk::Key::slash | gdk::Key::asciitilde if !typing && d.browser.reveals_child() => {
                    let text = if key == gdk::Key::slash { "/" } else { "~/" };
                    d.show_path_entry(Some(text));
                }
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
        self.window.add_controller(keys);
    }

    /// Whether a file is listed: hidden ones only when asked for, folders only when one is
    /// being chosen, and what the search has in its name.
    fn shows(&self, info: &gio::FileInfo) -> bool {
        if !self.prefs.borrow().hidden && (info.is_hidden() || info.is_backup()) {
            return false;
        }
        if self.request.picks_folder() && !is_dir(info) {
            return false;
        }
        let query = self.search.text().to_lowercase();
        query.is_empty() || info.display_name().to_lowercase().contains(query.trim())
    }

    /// Whether a listed file can be chosen: folders always, files of the chosen type.
    fn choosable(&self, info: &gio::FileInfo) -> bool {
        is_dir(info)
            || self
                .filter
                .borrow()
                .as_ref()
                .is_none_or(|filter| filter.match_(info))
    }

    fn here_folder(&self) -> Option<gio::File> {
        match &*self.here.borrow() {
            Location::Folder(file) => Some(file.clone()),
            Location::Recent => None,
        }
    }

    fn go(self: &Rc<Self>, location: Location, remember: bool) {
        if remember && *self.here.borrow() == location {
            return;
        }
        if remember {
            let from = self.here.replace(location.clone());
            self.back.borrow_mut().push(from);
            self.forward.borrow_mut().clear();
        } else {
            self.here.replace(location.clone());
        }
        self.search.set_text("");
        match &location {
            Location::Folder(file) => {
                self.directory.set_file(Some(file));
                self.filtered.set_model(Some(&self.directory));
            }
            Location::Recent => {
                self.fill_recent();
                self.filtered.set_model(Some(&self.recent));
            }
        }
        self.show_path_entry(None);
        self.fill_crumbs();
        self.sync_places();
        self.fill_location();
        self.back_button.set_sensitive(!self.back.borrow().is_empty());
        self.forward_button
            .set_sensitive(!self.forward.borrow().is_empty());
        self.update_status();
        self.update_accept();
    }

    /// Back (-1) or forward (1) in where the window has been.
    fn step(self: &Rc<Self>, direction: i32) {
        let (from, to) = if direction < 0 {
            (&self.back, &self.forward)
        } else {
            (&self.forward, &self.back)
        };
        let Some(location) = from.borrow_mut().pop() else {
            return;
        };
        to.borrow_mut().push(self.here.borrow().clone());
        self.go(location, false);
    }

    fn go_up(self: &Rc<Self>) {
        if let Some(parent) = self.here_folder().and_then(|f| f.parent()) {
            self.go(Location::Folder(parent), true);
        }
    }

    /// What was used last, newest first, from the recent files GTK keeps.
    fn fill_recent(&self) {
        self.recent.remove_all();
        let mut items = gtk::RecentManager::default().items();
        items.sort_by_key(|item| std::cmp::Reverse(item.modified().to_unix()));
        for item in items
            .into_iter()
            .filter(|item| item.is_local() && item.exists())
            .take(50)
        {
            let file = gio::File::for_uri(&item.uri());
            if let Ok(info) =
                file.query_info(ATTRIBUTES, gio::FileQueryInfoFlags::NONE, gio::Cancellable::NONE)
            {
                info.set_attribute_object("standard::file", &file);
                self.recent.append(&info);
            }
        }
    }

    fn fill_crumbs(self: &Rc<Self>) {
        while let Some(child) = self.crumbs.first_child() {
            self.crumbs.remove(&child);
        }
        let mut trail: Vec<(String, Location)> = Vec::new();
        match &*self.here.borrow() {
            Location::Recent => trail.push(("最近使用".into(), Location::Recent)),
            Location::Folder(file) => {
                let home = gio::File::for_path(home());
                let mounts = gio::VolumeMonitor::get().mounts();
                let mut at = Some(file.clone());
                while let Some(folder) = at {
                    let top = if folder.equal(&home) {
                        Some(glib::user_name().to_string_lossy().into_owned())
                    } else {
                        mounts
                            .iter()
                            .find(|m| m.root().equal(&folder))
                            .map(|m| m.name().to_string())
                    };
                    let parent = folder.parent();
                    let label = top.clone().unwrap_or_else(|| match &parent {
                        Some(_) => folder
                            .basename()
                            .map(|b| b.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                        None => "/".into(),
                    });
                    trail.push((label, Location::Folder(folder)));
                    at = if top.is_some() { None } else { parent };
                }
                trail.reverse();
            }
        }
        let last = trail.len().saturating_sub(1);
        for (index, (label, location)) in trail.into_iter().enumerate() {
            if index > 0 {
                self.crumbs.append(
                    &gtk::Image::builder()
                        .icon_name("go-next-symbolic")
                        .pixel_size(12)
                        .css_classes(["dim-label"])
                        .build(),
                );
            }
            let button = gtk::Button::builder()
                .label(label)
                .css_classes(["flat", "fc-crumb"])
                .build();
            if index == last {
                button.add_css_class("current");
            }
            let weak = Rc::downgrade(self);
            button.connect_clicked(move |_| {
                if let Some(dialog) = weak.upgrade() {
                    dialog.go(location.clone(), true);
                }
            });
            self.crumbs.append(&button);
        }
    }

    fn fill_places(&self) {
        let mut locations = Vec::new();
        let mut first_of_section = Vec::new();
        for (title, section) in places() {
            if section.is_empty() {
                continue;
            }
            first_of_section.push((locations.len() as i32, title));
            for place in section {
                let line = gtk::Box::new(gtk::Orientation::Horizontal, GAP);
                line.append(&gtk::Image::from_gicon(&place.icon));
                line.append(
                    &gtk::Label::builder()
                        .label(&place.label)
                        .xalign(0.0)
                        .ellipsize(gtk::pango::EllipsizeMode::End)
                        .build(),
                );
                self.places.append(&line);
                locations.push(place.location);
            }
        }
        self.places.set_header_func(move |row, _| {
            let title = first_of_section
                .iter()
                .find(|(index, _)| *index == row.index())
                .and_then(|(_, title)| *title);
            row.set_header(
                title
                    .map(|title| {
                        gtk::Label::builder()
                            .label(title)
                            .xalign(0.0)
                            .css_classes(["caption", "dim-label"])
                            .margin_top(EDGE)
                            .margin_start(GAP)
                            .margin_bottom(GAP / 2)
                            .build()
                    })
                    .as_ref(),
            );
        });
        *self.place_locations.borrow_mut() = locations;
    }

    fn sync_places(&self) {
        let here = self.here.borrow();
        let index = self.place_locations.borrow().iter().position(|l| *l == *here);
        match index.and_then(|i| self.places.row_at_index(i as i32)) {
            Some(row) => self.places.select_row(Some(&row)),
            None => self.places.unselect_all(),
        }
    }

    /// The folded save window's folder menu: here, then the places' folders.
    fn fill_location(&self) {
        let mut files: Vec<gio::File> = Vec::new();
        let mut labels: Vec<String> = Vec::new();
        for (_, section) in places() {
            for place in section {
                if let Location::Folder(file) = place.location {
                    files.push(file);
                    labels.push(place.label);
                }
            }
        }
        let here = self.here_folder();
        let index = match here.as_ref().and_then(|h| files.iter().position(|f| f.equal(h))) {
            Some(index) => index,
            None => {
                if let Some(here) = here {
                    labels.insert(
                        0,
                        here.basename()
                            .map(|b| b.to_string_lossy().into_owned())
                            .unwrap_or_else(|| "/".into()),
                    );
                    files.insert(0, here);
                }
                0
            }
        };
        self.syncing.set(true);
        self.location.set_model(Some(&gtk::StringList::new(
            &labels.iter().map(String::as_str).collect::<Vec<_>>(),
        )));
        self.location.set_selected(index as u32);
        self.syncing.set(false);
        *self.location_files.borrow_mut() = files;
    }

    fn show_expanded(&self, expanded: bool) {
        let save = self.request.mode == Mode::Save;
        self.browser.set_reveal_child(!save || expanded);
        for widget in &self.location_row {
            widget.set_visible(save && !expanded);
        }
        self.new_folder
            .set_visible((save && expanded) || self.request.picks_folder());
        self.expand.set_icon_name(if expanded {
            "pan-up-symbolic"
        } else {
            "pan-down-symbolic"
        });
        self.expand
            .set_tooltip_text(Some(if expanded { "收合" } else { "展開" }));
        // shortcut: the window jumps to its new size while the browser slides; animating the size needs the compositor's help.
        if save && expanded {
            self.window.set_default_size(780, 560);
        }
    }

    fn show_path_entry(&self, text: Option<&str>) {
        match text {
            Some(text) => {
                self.path_stack.set_visible_child_name("entry");
                self.path_entry.remove_css_class("error");
                self.path_entry.set_text(text);
                self.path_entry.grab_focus();
                self.path_entry.set_position(-1);
            }
            None => self.path_stack.set_visible_child_name("crumbs"),
        }
    }

    /// Goes where a typed path says: into a folder, or to a file's folder with it chosen.
    fn go_to_typed(self: &Rc<Self>, typed: &str) {
        let typed = typed.trim();
        let path = if let Some(rest) = typed.strip_prefix('~') {
            home().join(rest.trim_start_matches('/'))
        } else if Path::new(typed).is_absolute() {
            PathBuf::from(typed)
        } else if let Some(here) = self.here_folder().and_then(|f| f.path()) {
            here.join(typed)
        } else {
            home().join(typed)
        };
        if path.is_dir() {
            self.go(Location::Folder(gio::File::for_path(&path)), true);
            return;
        }
        let parent = path.parent().filter(|p| p.is_dir());
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
        match (parent, name) {
            (Some(parent), Some(name)) if path.exists() || self.request.mode == Mode::Save => {
                self.go(Location::Folder(gio::File::for_path(parent)), true);
                if self.request.mode == Mode::Save {
                    self.name.set_text(&name);
                } else {
                    self.select_named(&name);
                }
            }
            _ => self.path_entry.add_css_class("error"),
        }
    }

    /// Chooses the listed file of this name once the folder has been read.
    fn select_named(self: &Rc<Self>, name: &str) {
        let name = name.to_owned();
        let weak = Rc::downgrade(self);
        let pick = move || {
            let Some(dialog) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if dialog.directory.is_loading() {
                return glib::ControlFlow::Continue;
            }
            for position in 0..dialog.selection.n_items() {
                let found = dialog
                    .selection
                    .item(position)
                    .and_downcast::<gio::FileInfo>()
                    .is_some_and(|info| info.name().to_string_lossy() == name);
                if found {
                    dialog.selection.select_item(position, true);
                    break;
                }
            }
            glib::ControlFlow::Break
        };
        glib::timeout_add_local(std::time::Duration::from_millis(30), pick);
    }

    fn choose_filter(self: &Rc<Self>, index: usize) {
        let Some(filter) = self.request.filters.get(index) else {
            return;
        };
        self.filter_index.set(index);
        self.filter.replace(Some(filter.file_filter()));
        if self.request.mode == Mode::Save {
            if let Some(renamed) = with_extension(&self.name.text(), filter) {
                self.name.set_text(&renamed);
            }
        }
        // Binds every row again, to fade what no longer matches.
        let model = self.filtered.model();
        self.filtered.set_model(None::<&gio::ListModel>);
        self.filtered.set_model(model.as_ref());
        self.update_accept();
    }

    fn update_status(&self) {
        let grid = self.prefs.borrow().grid;
        let error = matches!(*self.here.borrow(), Location::Folder(_)) && self.directory.error().is_some();
        let empty = self.filtered.n_items() == 0 && !self.directory.is_loading();
        let text = if error {
            Some("無法讀取這個檔案夾")
        } else if empty && !self.search.text().is_empty() {
            Some("沒有符合的檔案")
        } else if empty {
            Some("沒有項目")
        } else {
            None
        };
        match text {
            Some(text) => {
                self.status.set_label(text);
                self.views.set_visible_child_name("status");
            }
            None => self
                .views
                .set_visible_child_name(if grid { "grid" } else { "list" }),
        }
    }

    fn chosen(&self) -> Vec<gio::FileInfo> {
        (0..self.selection.n_items())
            .filter(|&position| self.selection.is_selected(position))
            .filter_map(|position| self.selection.item(position).and_downcast::<gio::FileInfo>())
            .collect()
    }

    fn selected_changed(&self) {
        if self.request.mode == Mode::Save {
            if let [info] = self.chosen().as_slice() {
                if !is_dir(info) {
                    self.name.set_text(&info.display_name());
                }
            }
        }
        self.update_accept();
    }

    fn update_accept(&self) {
        let in_folder = self.here_folder().is_some();
        let ok = match self.request.mode {
            Mode::Save => {
                let name = self.name.text();
                let bad = name.contains('/');
                if bad {
                    self.name.add_css_class("error");
                } else {
                    self.name.remove_css_class("error");
                }
                in_folder && !name.trim().is_empty() && !bad
            }
            Mode::SaveFiles => in_folder,
            Mode::Open if self.request.directory => in_folder || !self.chosen().is_empty(),
            Mode::Open => !self.chosen().is_empty(),
        };
        self.accept.set_sensitive(ok);
    }

    /// A double click or Enter on a listed file.
    fn activate(self: &Rc<Self>, position: u32) {
        let Some(info) = self.selection.item(position).and_downcast::<gio::FileInfo>() else {
            return;
        };
        if is_dir(&info) {
            if let Some(file) = info_file(&info) {
                self.go(Location::Folder(file), true);
            }
        } else if self.choosable(&info) {
            self.selection.select_item(position, true);
            self.accept();
        }
    }

    fn accept(self: &Rc<Self>) {
        if !self.accept.is_sensitive() {
            return;
        }
        match self.request.mode {
            Mode::Open => {
                let chosen = self.chosen();
                // Opening a file with a folder chosen goes into it, as a double click does.
                if !self.request.directory {
                    if let [info] = chosen.as_slice() {
                        if is_dir(info) {
                            if let Some(file) = info_file(info) {
                                self.go(Location::Folder(file), true);
                            }
                            return;
                        }
                    }
                }
                let files: Vec<gio::File> = if chosen.is_empty() {
                    self.here_folder().into_iter().collect()
                } else {
                    chosen.iter().filter_map(info_file).collect()
                };
                self.finish(RESPONSE_OK, files.iter().map(|f| f.uri().to_string()).collect());
            }
            Mode::SaveFiles => {
                let Some(folder) = self.here_folder() else { return };
                let taken: Vec<String> = self
                    .request
                    .files
                    .iter()
                    .filter(|name| folder.child(name).query_exists(gio::Cancellable::NONE))
                    .cloned()
                    .collect();
                let uris = self
                    .request
                    .files
                    .iter()
                    .map(|name| folder.child(name).uri().to_string())
                    .collect();
                if taken.is_empty() {
                    self.finish(RESPONSE_OK, uris);
                } else {
                    self.ask_replace(&taken, uris);
                }
            }
            Mode::Save => {
                let Some(folder) = self.here_folder() else { return };
                let name = self.name.text().trim().to_owned();
                let target = folder.child(&name);
                if target.query_exists(gio::Cancellable::NONE) {
                    self.ask_replace(&[name], vec![target.uri().to_string()]);
                } else {
                    self.finish(RESPONSE_OK, vec![target.uri().to_string()]);
                }
            }
        }
    }

    /// Asks before saving over what is there: `taken` are the names already used.
    fn ask_replace(self: &Rc<Self>, taken: &[String], uris: Vec<String>) {
        let weak = Rc::downgrade(self);
        let title = match taken {
            [name] => format!("取代「{name}」？"),
            _ => format!("取代 {} 個項目？", taken.len()),
        };
        alert::confirm(
            &self.window,
            &title,
            Some("原本的檔案會被取代。"),
            "取代",
            move || {
                if let Some(dialog) = weak.upgrade() {
                    dialog.finish(RESPONSE_OK, uris);
                }
            },
        );
    }

    fn ask_new_folder(self: &Rc<Self>) {
        let Some(folder) = self.here_folder() else { return };
        let weak = Rc::downgrade(self);
        alert::prompt(
            &self.window,
            "新增檔案夾",
            NEW_FOLDER,
            -1,
            "建立",
            move |name| {
                let Some(dialog) = weak.upgrade() else { return };
                let new = folder.child(&name);
                match new.make_directory(gio::Cancellable::NONE) {
                    Ok(()) => dialog.go(Location::Folder(new), true),
                    Err(err) => alert::notice(&dialog.window, "無法建立檔案夾", Some(err.message())),
                }
            },
        );
    }

    /// Answers the portal and closes the window.
    fn finish(&self, response: u32, uris: Vec<String>) {
        self.reply(response, uris);
        self.window.close();
    }

    /// Answers the portal, once.
    fn reply(&self, response: u32, uris: Vec<String>) {
        let Some(reply) = self.reply.borrow_mut().take() else {
            return;
        };
        let results = glib::VariantDict::new(None);
        if response == RESPONSE_OK {
            results.insert_value("uris", &uris.to_variant());
            let choices: Vec<(String, String)> = self
                .choices
                .iter()
                .map(|(id, widget)| (id.clone(), widget.value()))
                .collect();
            results.insert_value("choices", &choices.to_variant());
            if let Some(filter) = self.request.filters.get(self.filter_index.get()) {
                results.insert_value(
                    "current_filter",
                    &(filter.name.clone(), filter.rules.clone()).to_variant(),
                );
            }
            if self.request.mode == Mode::Open {
                results.insert_value("writable", &true.to_variant());
            }
        }
        reply(response, results.end());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn glob(pattern: &str) -> Filter {
        Filter {
            name: String::new(),
            rules: vec![(RULE_GLOB, pattern.into())],
        }
    }

    #[test]
    fn a_new_type_changes_the_extension() {
        assert_eq!(
            with_extension("海報.png", &glob("*.jpg")).as_deref(),
            Some("海報.jpg")
        );
        assert_eq!(
            with_extension("海報", &glob("*.jpg")).as_deref(),
            Some("海報.jpg")
        );
        assert_eq!(
            with_extension(".bashrc", &glob("*.txt")).as_deref(),
            Some(".bashrc.txt")
        );
        assert_eq!(
            with_extension("a.tar.gz", &glob("*.zip")).as_deref(),
            Some("a.tar.zip")
        );
        assert_eq!(with_extension("海報.png", &glob("*")), None);
        assert_eq!(with_extension("海報.png", &glob("*.[jJ][pP][gG]")), None);
        let mime = Filter {
            name: String::new(),
            rules: vec![(RULE_MIME, "image/png".into())],
        };
        assert_eq!(with_extension("海報.jpg", &mime), None);
    }

    #[test]
    fn the_extension_is_left_out_of_what_is_selected() {
        assert_eq!(stem_chars("海報.png"), 2);
        assert_eq!(stem_chars("海報"), 2);
        assert_eq!(stem_chars(".bashrc"), 7);
    }

    #[test]
    fn a_portal_request_is_read_from_its_options() {
        let options = glib::VariantDict::new(None);
        options.insert_value("accept_label", &"_Upload".to_variant());
        options.insert_value("multiple", &true.to_variant());
        let filters = vec![("Images".to_string(), vec![(RULE_MIME, "image/png".to_string())])];
        options.insert_value("filters", &filters.to_variant());
        options.insert_value(
            "current_filter",
            &("PDF".to_string(), vec![(RULE_GLOB, "*.pdf".to_string())]).to_variant(),
        );
        let choices = vec![(
            "encoding".to_string(),
            "Encoding".to_string(),
            vec![("utf8".to_string(), "UTF-8".to_string())],
            "utf8".to_string(),
        )];
        options.insert_value("choices", &choices.to_variant());
        options.insert_value("current_folder", &b"/tmp\0".to_vec().to_variant());
        let request = Request::parse(Mode::Open, "Open".into(), &options.end());
        assert_eq!(request.accept_label.as_deref(), Some("_Upload"));
        assert!(request.multiple && !request.directory);
        // A current filter not among the filters is offered first.
        assert_eq!(
            request
                .filters
                .iter()
                .map(|f| f.name.as_str())
                .collect::<Vec<_>>(),
            ["PDF", "Images"]
        );
        assert_eq!(
            request.choices[0].options,
            [("utf8".to_string(), "UTF-8".to_string())]
        );
        assert_eq!(request.current_folder.as_deref(), Some(Path::new("/tmp")));
    }

    #[test]
    fn the_interface_xml_parses() {
        let node = gio::DBusNodeInfo::for_xml(&interface_xml()).unwrap();
        let chooser = node.lookup_interface(INTERFACE).unwrap();
        assert!(chooser.lookup_method("SaveFiles").is_some());
        assert!(node
            .lookup_interface("org.freedesktop.impl.portal.Request")
            .is_some());
    }
}
