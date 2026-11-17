//! `nekoland-files`: Nekoland's file manager. Folders in tabs, seen as icons, a list,
//! columns or a gallery; a sidebar of places, drives, servers and tags; search, Quick Look,
//! 取得資訊, tags, renaming several at once, compressing and the trash. Copying and
//! moving go to nekoland-fileop. It serves `org.freedesktop.FileManager1`, which the Dock
//! and other programs ask to show a folder, a file or its information.

use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    rc::Rc,
    time::{Duration, Instant},
};

use adw::prelude::*;
use gtk::{gdk, gio, glib};

/// A handler that holds what it works on weakly, as everything it is connected to is
/// owned by that.
macro_rules! on {
    ($rc:expr, |$d:ident $(, $arg:pat_param)*| $body:expr) => {{
        let weak = std::rc::Rc::downgrade($rc);
        move |$($arg),*| {
            if let Some($d) = weak.upgrade() {
                $body;
            }
        }
    }};
}

mod alert;
mod browse;
mod dropdowns;
mod focus;
#[path = "files/info.rs"]
mod info;
#[path = "files/ops.rs"]
mod ops;
#[path = "files/pane.rs"]
mod pane;
#[path = "files/preview.rs"]
mod preview;
#[path = "files/quicklook.rs"]
mod quicklook;
#[path = "files/rename.rs"]
mod rename;
#[path = "files/search.rs"]
mod search;
#[path = "files/server.rs"]
mod server;
#[path = "files/sidebar.rs"]
mod sidebar;
#[path = "files/tags.rs"]
mod tags;
#[path = "files/thumbs.rs"]
mod thumbs;

use browse::{home, info_file, GAP};
use pane::{Pane, View, Where};

const APP_ID: &str = "io.github.koukeneko.Nekoland.Files";
const MANAGER_NAME: &str = "org.freedesktop.FileManager1";
const MANAGER_PATH: &str = "/org/freedesktop/FileManager1";
const MANAGER_XML: &str = r#"<node><interface name="org.freedesktop.FileManager1">
<method name="ShowFolders"><arg type="as" name="URIs" direction="in"/><arg type="s" name="StartupId" direction="in"/></method>
<method name="ShowItems"><arg type="as" name="URIs" direction="in"/><arg type="s" name="StartupId" direction="in"/></method>
<method name="ShowItemProperties"><arg type="as" name="URIs" direction="in"/><arg type="s" name="StartupId" direction="in"/></method>
</interface></node>"#;
const HOVER_NAME: &str = "io.github.koukeneko.Nekoland.Files.Hover";
const HOVER_XML: &str = r#"<node><interface name="io.github.koukeneko.Nekoland.Files.Hover">
<method name="ItemAt"><arg type="s" name="Window" direction="in"/><arg type="i" name="X" direction="in"/><arg type="i" name="Y" direction="in"/>
<arg type="s" name="Path" direction="out"/><arg type="i" name="Left" direction="out"/><arg type="i" name="Top" direction="out"/><arg type="i" name="Width" direction="out"/><arg type="i" name="Height" direction="out"/></method>
</interface></node>"#;
/// Typed letters choose the first item whose name starts with them, until a pause.
const TYPE_AHEAD: Duration = Duration::from_millis(1000);

const CSS: &str = "

.nf-small { font-size: 12px; }
.nf-status { font-size: 15px; }
.nf-list > listview > row > cell, .nf-list > header > button { padding-left: 6px; padding-right: 6px; }
.nf-list > listview > row > cell:first-child, .nf-list > header > button:first-child { padding-left: 12px; }
.nf-list > listview > row > cell:last-child, .nf-list > header > button:last-child { padding-right: 12px; }
/* Tiles 8px apart, 4px each side, and 12px from the edge with the grid's own 8px. */
.nf-grid { padding: 8px; }
.nf-grid > child { padding: 8px; margin: 4px; border-radius: 10px; }
.nf-thumb { border-radius: 8px; }
.nf-strip { padding: 8px; background: none; }
.nf-strip > row { padding: 4px; margin: 0 4px; border-radius: 8px; }
.nf-column { background: none; padding: 4px 0; }
.nf-column > row { padding: 4px 8px; border-radius: 6px; margin: 0 4px; }
.nf-column-frame { border-right: 1px solid var(--border-color); }
.nf-facts { background: none; }
.nf-pathbar { min-height: 30px; }
/* A toggle group's buttons 8px apart, as the other buttons are, not joined by lines. */
toggle-group > separator { min-width: 0; margin: 0 4px; background: none; }
/* The design language's gutter and gap in the toolbar, where libadwaita leaves 7px and 6px;
 * the window buttons keep 4px of their own inside the gutter. A box's border-spacing adds
 * to the 6px spacing libadwaita gives these two, so 2px more makes 8. */
headerbar > windowhandle > box { padding-left: 12px; padding-right: 8px; }
headerbar box.start, headerbar box.end { border-spacing: 2px; }
/* libadwaita sets an icon button's words bold; the design language sets every button's medium. */
buttoncontent > box > label { font-weight: inherit; }
.nf-crumb { min-width: 0; min-height: 22px; padding: 0 4px; font-weight: 500; }
/* The first crumb's text lines up with the edge. */
.nf-pathbar box > button.nf-crumb:first-child { padding-left: 0; }
.nf-crumb.current { font-weight: 600; }
.nf-chip { min-height: 22px; padding: 0 8px; border-radius: 11px; background-color: alpha(currentColor, .08); }
button.nf-chip { padding: 0 8px; font-weight: 500; }
.nf-chip-remove { min-width: 18px; min-height: 18px; padding: 0; }
/* Its glyph, not its 22px target, at the row's edge, as the icons at the other end are. */
.nf-eject { min-width: 22px; min-height: 22px; padding: 0; margin-right: -3px; }
.nf-before-after { background-color: var(--window-bg-color); border-radius: 10px; padding: 4px; }
.nf-before-after > row { padding: 4px 8px; }
.nf-tag-row { padding: 4px 8px; }
.nf-tag-menu > contents { padding: 6px; }
.nf-tag-menu list { background: none; }
.nf-tag-menu row { min-height: 28px; padding: 0 8px; border-radius: 6px; }
.nf-tag-menu row:hover { background-color: var(--nekoland-ink); color: var(--nekoland-on-ink); }
.nf-tag-menu row > separator { margin: 4px 0; }
.nf-dot { min-width: 10px; min-height: 10px; border-radius: 5px; box-shadow: 0 0 0 1.5px var(--view-bg-color); }
.nf-dots > .nf-dot + .nf-dot { margin-left: -3px; }
.nf-tag-button { min-width: 18px; min-height: 18px; padding: 0; border-radius: 9px; }
.nf-tag-button.on, .nf-tag-button:checked { box-shadow: 0 0 0 2px var(--popover-bg-color), 0 0 0 3.5px var(--window-fg-color); }
.red { background-color: #d4493f; }
.orange { background-color: #dc8436; }
.yellow { background-color: #cfa92e; }
.green { background-color: #4f9a61; }
.blue { background-color: #4a7fc0; }
.purple { background-color: #8e63b5; }
.gray { background-color: #8e8c90; }
@keyframes nf-blink { 50% { outline-color: transparent; } }
.nf-spring { outline: 2px solid currentColor; outline-offset: -2px; border-radius: 8px; animation: nf-blink 120ms steps(1) 2; }
.nf-quick-look { background-color: var(--view-bg-color); }
.nf-drag-count { min-width: 20px; min-height: 20px; padding: 0 6px; border-radius: 10px; background-color: var(--window-fg-color); color: var(--window-bg-color); font-size: 12px; font-weight: 600; }
@keyframes nf-zoom-in { from { opacity: 0; transform: scale(.92); } }
.nf-zoom-in { animation: nf-zoom-in 180ms cubic-bezier(.2, .8, .2, 1); }
";

/// What the window remembers between uses.
pub struct Prefs {
    pub view: View,
    pub preview: bool,
    pub hidden: bool,
    pub icon_size: i32,
    pub sidebar: bool,
}

impl Prefs {
    fn path() -> PathBuf {
        glib::user_config_dir().join("nekoland/files.ini")
    }

    fn load() -> Prefs {
        let file = glib::KeyFile::new();
        let read = file
            .load_from_file(Self::path(), glib::KeyFileFlags::NONE)
            .is_ok();
        let flag = |key: &str, default: bool| {
            if read {
                file.boolean("files", key).unwrap_or(default)
            } else {
                default
            }
        };
        Prefs {
            view: file
                .string("files", "view")
                .ok()
                .and_then(|v| View::from_name(&v))
                .unwrap_or(View::List),
            preview: flag("preview", false),
            hidden: flag("hidden", false),
            icon_size: file
                .integer("files", "icon-size")
                .unwrap_or(64)
                .clamp(pane::ICON_SIZES.0 as i32, pane::ICON_SIZES.1 as i32),
            sidebar: flag("sidebar", true),
        }
    }

    pub fn save(&self) {
        let file = glib::KeyFile::new();
        file.set_string("files", "view", self.view.name());
        file.set_boolean("files", "preview", self.preview);
        file.set_boolean("files", "hidden", self.hidden);
        file.set_integer("files", "icon-size", self.icon_size);
        file.set_boolean("files", "sidebar", self.sidebar);
        let path = Self::path();
        let written = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .map_err(|err| err.to_string())
            .and_then(|()| file.save_to_file(&path).map_err(|err| err.to_string()));
        if let Err(err) = written {
            eprintln!("nekoland-files: cannot save {}: {err}", path.display());
        }
    }
}

fn main() -> glib::ExitCode {
    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_OPEN)
        // Started by D-Bus for FileManager1 (--gapplication-service), it waits for the call
        // that started it rather than quitting before it arrives.
        .inactivity_timeout(10_000)
        .build();
    app.connect_startup(|app| {
        let css = gtk::CssProvider::new();
        css.load_from_string(CSS);
        if let Some(display) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &css,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
        dropdowns::drop_tails();
        app.set_menubar(Some(&menubar()));
        serve_file_manager(app);
        serve_hover(app);
    });
    app.connect_activate(|app| match app.active_window() {
        Some(window) => window.present(),
        None => {
            Window::open(app, Where::Folder(gio::File::for_path(home())));
        }
    });
    app.connect_open(|app, files, _| {
        for file in files {
            let is_dir = file.query_file_type(gio::FileQueryInfoFlags::NONE, gio::Cancellable::NONE)
                == gio::FileType::Directory;
            if is_dir {
                Window::open(app, Where::Folder(file.clone()));
            } else {
                show_item(app, file);
            }
        }
    });
    app.run()
}

/// Tells the compositor which file is at a point of a window, for its hover panel: the
/// window as GTK names it on the bus, and the point from its top left corner.
fn serve_hover(app: &adw::Application) {
    let Some(connection) = app.dbus_connection() else {
        return;
    };
    let node = gio::DBusNodeInfo::for_xml(HOVER_XML).expect("the XML above parses");
    let interface = node.lookup_interface(HOVER_NAME).expect("the XML above has it");
    let weak = app.downgrade();
    let registered = connection
        .register_object(MANAGER_PATH, &interface)
        .method_call(move |_, _, _, _, _, params, invocation| {
            let (Some(app), Some((window, x, y))) = (weak.upgrade(), params.get::<(String, i32, i32)>())
            else {
                invocation.return_error(gio::DBusError::InvalidArgs, "expected (sii)");
                return;
            };
            let window = window
                .rsplit('/')
                .next()
                .and_then(|id| id.parse().ok())
                .and_then(|id| app.window_by_id(id))
                .or_else(|| (app.windows().len() == 1).then(|| app.windows()[0].clone()));
            match window.and_then(|window| pane::file_at(&window, f64::from(x), f64::from(y))) {
                Some((path, area)) => invocation.return_value(Some(
                    &(
                        path.to_string_lossy().into_owned(),
                        area.x().round() as i32,
                        area.y().round() as i32,
                        area.width().round() as i32,
                        area.height().round() as i32,
                    )
                        .to_variant(),
                )),
                None => invocation.return_error(gio::DBusError::Failed, "no file there"),
            }
        })
        .build();
    if let Err(err) = registered {
        eprintln!("nekoland-files: cannot serve {HOVER_NAME}: {err}");
    }
}

/// Answers FileManager1 for as long as the application runs: the Dock's 在檔案中顯示 and
/// 取得資訊 come this way.
fn serve_file_manager(app: &adw::Application) {
    let Some(connection) = app.dbus_connection() else {
        return;
    };
    let node = gio::DBusNodeInfo::for_xml(MANAGER_XML).expect("the XML above parses");
    let interface = node.lookup_interface(MANAGER_NAME).expect("the XML above has it");
    let weak = app.downgrade();
    let registered = connection
        .register_object(MANAGER_PATH, &interface)
        .method_call(move |_, _, _, _, method, params, invocation| {
            let (Some(app), Some(uris)) = (weak.upgrade(), params.child_value(0).get::<Vec<String>>()) else {
                invocation.return_error(gio::DBusError::InvalidArgs, "expected (ass)");
                return;
            };
            let files: Vec<gio::File> = uris.iter().map(|uri| gio::File::for_uri(uri)).collect();
            match method {
                "ShowFolders" => {
                    for folder in files {
                        Window::open(&app, Where::Folder(folder));
                    }
                }
                "ShowItems" => {
                    for file in &files {
                        show_item(&app, file);
                    }
                }
                "ShowItemProperties" => {
                    let win = app
                        .active_window()
                        .and_then(|w| {
                            WINDOWS.with_borrow(|all| {
                                all.iter()
                                    .find(|x| x.window.upcast_ref::<gtk::Window>() == &w)
                                    .cloned()
                            })
                        })
                        .unwrap_or_else(|| Window::open(&app, Where::Folder(gio::File::for_path(home()))));
                    for info in files.iter().filter_map(pane::query) {
                        info::show(&win, &info);
                    }
                }
                _ => {
                    invocation.return_error(gio::DBusError::UnknownMethod, method);
                    return;
                }
            }
            invocation.return_value(None);
        })
        .build();
    if let Err(err) = registered {
        eprintln!("nekoland-files: cannot serve {MANAGER_NAME}: {err}");
        return;
    }
    // Asked for directly: gio's bus_own_name panics as the bus closes at logout, when GLib
    // reports the name lost with no connection.
    const DO_NOT_QUEUE: u32 = 4;
    connection.call(
        Some("org.freedesktop.DBus"),
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
        "RequestName",
        Some(&(MANAGER_NAME, DO_NOT_QUEUE).to_variant()),
        Some(glib::VariantTy::new("(u)").expect("a valid type")),
        gio::DBusCallFlags::NONE,
        -1,
        gio::Cancellable::NONE,
        |reply| match reply.map(|r| r.child_value(0).get::<u32>()) {
            // The primary owner, now or already.
            Ok(Some(1 | 4)) => {}
            Ok(_) => eprintln!("nekoland-files: {MANAGER_NAME} is another program's"),
            Err(err) => eprintln!("nekoland-files: cannot ask for {MANAGER_NAME}: {err}"),
        },
    );
}

/// A window on the folder `file` is in, with it chosen.
fn show_item(app: &adw::Application, file: &gio::File) {
    let Some(folder) = file.parent() else { return };
    let win = Window::open(app, Where::Folder(folder));
    if let Some(pane) = win.pane() {
        pane.select_when_listed(file.clone(), |_| {});
    }
}

/// The menu bar the compositor shows for this program.
fn menubar() -> gio::Menu {
    let item = |label: &str, action: &str, accel: Option<&str>| {
        let item = gio::MenuItem::new(Some(label), Some(action));
        if let Some(accel) = accel {
            item.set_attribute_value("accel", Some(&accel.to_variant()));
        }
        item
    };
    let section = |items: Vec<gio::MenuItem>| {
        let section = gio::Menu::new();
        for i in items {
            section.append_item(&i);
        }
        section
    };
    let file = gio::Menu::new();
    file.append_section(
        None,
        &section(vec![
            item("新增視窗", "win.new-window", Some("<Control>n")),
            item("新增分頁", "win.new-tab", Some("<Control>t")),
            item("新增檔案夾", "win.new-folder", Some("<Control><Shift>n")),
            item("打開", "win.open", Some("<Alt>Down")),
            item("關閉分頁", "win.close-tab", Some("<Control>w")),
        ]),
    );
    file.append_section(
        None,
        &section(vec![
            item("取得資訊", "win.info", Some("<Control>i")),
            item("重新命名", "win.rename", Some("F2")),
            item("壓縮", "win.compress", None),
            item("製作複本", "win.duplicate", Some("<Control>d")),
            item("製作連結", "win.link", None),
            item("快速查看", "win.quick-look", Some("space")),
        ]),
    );
    file.append_section(
        None,
        &section(vec![
            item("丟到垃圾桶", "win.trash", Some("Delete")),
            item("清空垃圾桶", "win.empty-trash", None),
        ]),
    );
    let edit = gio::Menu::new();
    edit.append_section(
        None,
        &section(vec![
            item("剪下", "win.cut", Some("<Control>x")),
            item("拷貝", "win.copy", Some("<Control>c")),
            item("貼上", "win.paste", Some("<Control>v")),
            item("全選", "win.select-all", Some("<Control>a")),
        ]),
    );
    let view = gio::Menu::new();
    view.append_section(
        None,
        &section(vec![
            item("圖示", "win.view::icons", Some("<Control>1")),
            item("列表", "win.view::list", Some("<Control>2")),
            item("欄位", "win.view::columns", Some("<Control>3")),
            item("圖庫", "win.view::gallery", Some("<Control>4")),
        ]),
    );
    let sorting = gio::Menu::new();
    sorting.append_submenu(Some("排序方式"), &ops::sort_menu());
    view.append_section(None, &sorting);
    view.append_section(
        None,
        &section(vec![
            item("顯示側欄", "win.sidebar", None),
            item("顯示預覽", "win.preview", None),
            item("顯示隱藏檔案", "win.hidden", Some("<Control>h")),
        ]),
    );
    let go = gio::Menu::new();
    go.append_section(
        None,
        &section(vec![
            item("上一頁", "win.back", Some("<Alt>Left")),
            item("下一頁", "win.forward", Some("<Alt>Right")),
            item("上層檔案夾", "win.up", Some("<Alt>Up")),
        ]),
    );
    go.append_section(
        None,
        &section(vec![
            item("最近使用", "win.go::recent", None),
            item(&glib::user_name().to_string_lossy(), "win.go::home", None),
            item("桌面", "win.go::desktop", None),
            item("文件", "win.go::documents", None),
            item("下載", "win.go::downloads", None),
            item("垃圾桶", "win.go::trash", None),
            item("網路", "win.go::network", None),
        ]),
    );
    go.append_section(
        None,
        &section(vec![
            item("前往檔案夾⋯", "win.goto", Some("<Control>l")),
            item("連接伺服器⋯", "win.connect-server", Some("<Control>k")),
        ]),
    );
    let menubar = gio::Menu::new();
    menubar.append_submenu(Some("檔案"), &file);
    menubar.append_submenu(Some("編輯"), &edit);
    menubar.append_submenu(Some("顯示方式"), &view);
    menubar.append_submenu(Some("前往"), &go);
    menubar
}

thread_local! {
    static WINDOWS: RefCell<Vec<Rc<Window>>> = const { RefCell::new(Vec::new()) };
}

pub struct Window {
    pub window: adw::ApplicationWindow,
    split: adw::OverlaySplitView,
    tabs: adw::TabView,
    sidebar: RefCell<Option<Rc<sidebar::Sidebar>>>,
    panes: RefCell<Vec<(adw::TabPage, Rc<Pane>)>>,
    title: gtk::Label,
    back: gtk::Button,
    forward: gtk::Button,
    /// An AdwToggleGroup, its `active` the index in View::ALL.
    views: gtk::Widget,
    search: gtk::SearchEntry,
    preview: gtk::ToggleButton,
    empty_trash: gtk::Button,
    pub prefs: RefCell<Prefs>,
    quick_look: RefCell<Option<Rc<quicklook::QuickLook>>>,
    actions: gio::SimpleActionGroup,
    /// Set while the toolbar follows the tab, which must not change the tab.
    syncing: Cell<bool>,
    typed: RefCell<(String, Option<Instant>)>,
}

impl Window {
    pub fn open(app: &adw::Application, at: Where) -> Rc<Window> {
        let prefs = Prefs::load();
        let tabs = adw::TabView::new();
        let tab_bar = adw::TabBar::builder().view(&tabs).autohide(true).build();
        let new_tab = browse::icon_button("tab-new-symbolic", "新增分頁");
        new_tab.set_action_name(Some("win.new-tab"));
        tab_bar.set_end_action_widget(Some(&new_tab));

        // The toolbar: the sidebar, back and forward, where it is; how it is seen, tags, more,
        // the preview and search.
        let show_sidebar = gtk::ToggleButton::builder()
            .icon_name("sidebar-show-symbolic")
            .tooltip_text("顯示側欄")
            .css_classes(["flat"])
            .active(prefs.sidebar)
            .build();
        let back = browse::icon_button("go-previous-symbolic", "上一頁");
        back.set_action_name(Some("win.back"));
        let forward = browse::icon_button("go-next-symbolic", "下一頁");
        forward.set_action_name(Some("win.forward"));
        let arrows = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        arrows.append(&back);
        arrows.append(&forward);
        let title = gtk::Label::builder()
            .css_classes(["heading"])
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .margin_start(GAP / 2)
            .build();
        // How it is seen, in View::ALL's order, the chosen one solid: libadwaita's toggle group.
        // shortcut: made from its name, as settings' segmented is, since the crate stops at
        // libadwaita 1.5 and the group came with 1.7; its own type once the crate is moved up.
        let toggles: String = [
            ("view-grid-symbolic", "圖示"),
            ("view-list-symbolic", "列表"),
            ("view-dual-symbolic", "欄位"),
            ("view-paged-symbolic", "圖庫"),
        ]
        .iter()
        .map(|(icon, tip)| {
            format!(
                "<child><object class=\"AdwToggle\"><property name=\"icon-name\">{icon}</property>\
                 <property name=\"tooltip\">{tip}</property></object></child>"
            )
        })
        .collect();
        let views: gtk::Widget = gtk::Builder::from_string(&format!(
            "<interface><object class=\"AdwToggleGroup\" id=\"group\">{toggles}</object></interface>"
        ))
        .object("group")
        .expect("the toggle group just described");
        views.set_valign(gtk::Align::Center);
        let tag_icon = gtk::Image::from_gicon(&gio::ThemedIcon::from_names(&[
            "tag-symbolic",
            "xsi-tag-symbolic",
        ]));
        let tag_menu = gtk::MenuButton::builder()
            .child(&tag_icon)
            .tooltip_text("標籤")
            .has_frame(false)
            .build();
        let more_menu = gio::Menu::new();
        let first = gio::Menu::new();
        first.append(Some("新增檔案夾"), Some("win.new-folder"));
        first.append(Some("新增分頁"), Some("win.new-tab"));
        more_menu.append_section(None, &first);
        let arranging = gio::Menu::new();
        arranging.append_submenu(Some("排序方式"), &ops::sort_menu());
        more_menu.append_section(None, &arranging);
        let showing = gio::Menu::new();
        showing.append(Some("顯示預覽"), Some("win.preview"));
        showing.append(Some("顯示隱藏檔案"), Some("win.hidden"));
        more_menu.append_section(None, &showing);
        let going = gio::Menu::new();
        going.append(Some("連接伺服器⋯"), Some("win.connect-server"));
        going.append(Some("打開終端機"), Some("win.terminal"));
        more_menu.append_section(None, &going);
        let more = gtk::MenuButton::builder()
            .icon_name("view-more-symbolic")
            .tooltip_text("更多")
            .menu_model(&more_menu)
            .has_frame(false)
            .build();
        let preview = gtk::ToggleButton::builder()
            .icon_name("sidebar-show-right-symbolic")
            .tooltip_text("顯示預覽")
            .css_classes(["flat"])
            .active(prefs.preview)
            .build();
        let search = gtk::SearchEntry::builder()
            .placeholder_text("搜尋")
            .width_chars(16)
            .build();
        let empty_trash = gtk::Button::builder()
            .label("清空")
            .action_name("win.empty-trash")
            .visible(false)
            .build();
        // Loose: the two ends are not held to half the bar each, as the end holds the most.
        let header = adw::HeaderBar::builder()
            .show_title(false)
            .centering_policy(adw::CenteringPolicy::Loose)
            .build();
        header.pack_start(&show_sidebar);
        header.pack_start(&arrows);
        header.pack_start(&title);
        header.pack_end(&search);
        header.pack_end(&preview);
        header.pack_end(&more);
        header.pack_end(&tag_menu);
        header.pack_end(&views);
        header.pack_end(&empty_trash);

        let content = adw::ToolbarView::builder()
            .top_bar_style(adw::ToolbarStyle::RaisedBorder)
            .build();
        content.add_top_bar(&header);
        content.add_top_bar(&tab_bar);
        content.set_content(Some(&tabs));
        let sidebar_view = adw::ToolbarView::new();
        // The app's name over the sidebar, as Settings has its own.
        sidebar_view.add_top_bar(
            &adw::HeaderBar::builder()
                .title_widget(&adw::WindowTitle::new("檔案", ""))
                .build(),
        );
        let split = adw::OverlaySplitView::builder()
            .sidebar(&sidebar_view)
            .content(&content)
            .min_sidebar_width(180.0)
            .max_sidebar_width(240.0)
            .show_sidebar(prefs.sidebar)
            .build();
        show_sidebar
            .bind_property("active", &split, "show-sidebar")
            .bidirectional()
            .build();
        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&split));
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("檔案")
            .default_width(1200)
            .default_height(740)
            .width_request(360)
            .height_request(294)
            .content(&toasts)
            .build();
        let narrow = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
            adw::BreakpointConditionLengthType::MaxWidth,
            720.0,
            adw::LengthUnit::Sp,
        ));
        narrow.add_setter(&split, "collapsed", Some(&true.to_value()));
        window.add_breakpoint(narrow);

        let win = Rc::new(Window {
            window,
            split,
            tabs,
            sidebar: RefCell::new(None),
            panes: RefCell::new(Vec::new()),
            title,
            back,
            forward,
            views,
            search,
            preview,
            empty_trash,
            prefs: RefCell::new(prefs),
            quick_look: RefCell::new(None),
            actions: gio::SimpleActionGroup::new(),
            syncing: Cell::new(false),
            typed: RefCell::new((String::new(), None)),
        });
        let sidebar = sidebar::Sidebar::new(&win);
        sidebar_view.set_content(Some(&sidebar.root));
        win.sidebar.replace(Some(sidebar));
        tag_menu.set_popover(Some(&ops::tag_popover(&Rc::downgrade(&win))));
        win.add_actions();
        win.connect();
        win.new_tab(at);
        WINDOWS.with_borrow_mut(|all| all.push(win.clone()));
        let closing = Rc::downgrade(&win);
        win.window.connect_close_request(move |_| {
            if let Some(win) = closing.upgrade() {
                WINDOWS.with_borrow_mut(|all| all.retain(|w| !Rc::ptr_eq(w, &win)));
                win.panes.borrow_mut().clear();
            }
            glib::Propagation::Proceed
        });
        win.window.present();
        focus::let_fields_go(&win.window);
        if let Some(pane) = win.pane() {
            pane.focus_content();
        }
        win
    }

    pub fn pane(&self) -> Option<Rc<Pane>> {
        let page = self.tabs.selected_page()?;
        self.panes
            .borrow()
            .iter()
            .find(|(p, _)| *p == page)
            .map(|(_, pane)| pane.clone())
    }

    pub fn new_tab(self: &Rc<Self>, at: Where) -> Rc<Pane> {
        let pane = Pane::new(self, at);
        let page = self.tabs.append(&pane.root);
        self.panes.borrow_mut().push((page.clone(), pane.clone()));
        self.tabs.set_selected_page(&page);
        self.pane_changed(&pane);
        pane.focus_content();
        pane
    }

    fn connect(self: &Rc<Self>) {
        self.tabs.connect_selected_page_notify(on!(self, |w, _t| {
            if let Some(pane) = w.pane() {
                w.pane_changed(&pane);
                w.selection_changed(&pane);
            }
        }));
        let weak = Rc::downgrade(self);
        self.tabs.connect_close_page(move |tabs, page| {
            tabs.close_page_finish(page, true);
            if let Some(w) = weak.upgrade() {
                w.panes.borrow_mut().retain(|(p, _)| p != page);
                if w.panes.borrow().is_empty() {
                    w.window.close();
                }
            }
            glib::Propagation::Stop
        });
        self.views.connect_notify_local(
            Some("active"),
            on!(self, |w, group, _| if !w.syncing.get() {
                if let Some(view) = View::ALL.get(group.property::<u32>("active") as usize) {
                    w.set_view(*view);
                }
            }),
        );
        self.preview
            .connect_toggled(on!(self, |w, toggle| if !w.syncing.get() {
                w.set_preview(toggle.is_active());
            }));
        self.split.connect_show_sidebar_notify(on!(self, |w, split| {
            w.prefs.borrow_mut().sidebar = split.shows_sidebar();
            w.prefs.borrow().save();
        }));
        self.search
            .connect_search_changed(on!(self, |w, entry| if !w.syncing.get() {
                if let Some(pane) = w.pane() {
                    pane.search(&entry.text());
                }
            }));
        self.search.connect_stop_search(on!(self, |w, entry| {
            entry.set_text("");
            if let Some(pane) = w.pane() {
                pane.search("");
            }
        }));

        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, modifiers| {
            let Some(win) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            if win.key(key, modifiers) {
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        self.window.add_controller(keys);
    }

    /// The window's keys, unless a field is being typed in or a dialog is over it.
    fn key(self: &Rc<Self>, key: gdk::Key, modifiers: gdk::ModifierType) -> bool {
        if self.window.visible_dialog().is_some() {
            return false;
        }
        let ctrl = modifiers.contains(gdk::ModifierType::CONTROL_MASK);
        let shift = modifiers.contains(gdk::ModifierType::SHIFT_MASK);
        let alt = modifiers.contains(gdk::ModifierType::ALT_MASK);
        let typing = gtk::prelude::GtkWindowExt::focus(&self.window)
            .is_some_and(|w| w.ancestor(gtk::Editable::static_type()).is_some() || w.is::<gtk::Text>());
        let activate = |name: &str, target: Option<glib::Variant>| {
            gio::prelude::ActionGroupExt::activate_action(&self.actions, name, target.as_ref());
            true
        };
        // What works while typing too.
        match key {
            gdk::Key::f if ctrl => {
                self.search.grab_focus();
                return true;
            }
            gdk::Key::t if ctrl => return activate("new-tab", None),
            gdk::Key::w if ctrl => return activate("close-tab", None),
            gdk::Key::n if ctrl && shift => return activate("new-folder", None),
            gdk::Key::N if ctrl => return activate("new-folder", None),
            gdk::Key::n if ctrl => return activate("new-window", None),
            gdk::Key::l if ctrl => return activate("goto", None),
            gdk::Key::k if ctrl => return activate("connect-server", None),
            gdk::Key::Left if alt => return activate("back", None),
            gdk::Key::Right if alt => return activate("forward", None),
            gdk::Key::Up if alt => return activate("up", None),
            _ => {}
        }
        if typing {
            return false;
        }
        let view = |name: &str| activate("view", Some(name.to_variant()));
        match key {
            gdk::Key::space => activate("quick-look", None),
            gdk::Key::Delete => activate(
                if self.here_is_trash() {
                    "delete-now"
                } else {
                    "trash"
                },
                None,
            ),
            gdk::Key::BackSpace if ctrl => activate("trash", None),
            gdk::Key::F2 => activate("rename", None),
            gdk::Key::Down if alt => activate("open", None),
            gdk::Key::i if ctrl => activate("info", None),
            gdk::Key::c if ctrl => activate("copy", None),
            gdk::Key::x if ctrl => activate("cut", None),
            gdk::Key::v if ctrl => activate("paste", None),
            gdk::Key::a if ctrl => activate("select-all", None),
            gdk::Key::d if ctrl => activate("duplicate", None),
            gdk::Key::h if ctrl => activate("hidden", None),
            gdk::Key::_1 if ctrl => view("icons"),
            gdk::Key::_2 if ctrl => view("list"),
            gdk::Key::_3 if ctrl => view("columns"),
            gdk::Key::_4 if ctrl => view("gallery"),
            gdk::Key::Escape if self.pane().is_some_and(|p| p.is_searching()) => {
                self.search.set_text("");
                true
            }
            _ if !ctrl && !alt => match key.to_unicode().filter(|c| !c.is_control() && !c.is_whitespace()) {
                Some(character) => {
                    self.type_ahead(character);
                    true
                }
                None => false,
            },
            _ => false,
        }
    }

    /// Chooses the first item whose name starts with what has just been typed.
    fn type_ahead(&self, character: char) {
        let mut typed = self.typed.borrow_mut();
        if typed.1.is_none_or(|at| at.elapsed() > TYPE_AHEAD) {
            typed.0.clear();
        }
        typed.0.extend(character.to_lowercase());
        typed.1 = Some(Instant::now());
        let Some(pane) = self.pane() else { return };
        let found = pane
            .listed()
            .into_iter()
            .find(|info| info.display_name().to_lowercase().starts_with(typed.0.as_str()));
        if let Some(file) = found.and_then(|info| info_file(&info)) {
            pane.select_files(&[file]);
        }
    }

    fn here_is_trash(&self) -> bool {
        self.pane().is_some_and(|p| p.here().is_trash())
    }

    fn set_view(self: &Rc<Self>, view: View) {
        if let Some(pane) = self.pane() {
            pane.set_view(view);
        }
        self.prefs.borrow_mut().view = view;
        self.prefs.borrow().save();
        self.sync_toolbar();
    }

    fn set_preview(self: &Rc<Self>, shown: bool) {
        for (_, pane) in self.panes.borrow().iter() {
            pane.show_preview(shown);
        }
        self.prefs.borrow_mut().preview = shown;
        self.prefs.borrow().save();
        self.sync_toolbar();
    }

    /// The tab went somewhere: its title, the toolbar and the sidebar follow when it is the one shown.
    pub fn pane_changed(self: &Rc<Self>, pane: &Rc<Pane>) {
        let title = pane.here().title();
        let icon = match pane.here() {
            Where::Recent => gio::ThemedIcon::new("document-open-recent-symbolic").upcast::<gio::Icon>(),
            Where::Tag(_) => gio::ThemedIcon::from_names(&["tag-symbolic", "xsi-tag-symbolic"]).upcast(),
            Where::Folder(ref f) if f.equal(&pane::trash()) => {
                gio::ThemedIcon::new("user-trash-symbolic").upcast()
            }
            Where::Folder(_) => gio::ThemedIcon::new("folder-symbolic").upcast(),
        };
        let page = self
            .panes
            .borrow()
            .iter()
            .find(|(_, p)| Rc::ptr_eq(p, pane))
            .map(|(page, _)| page.clone());
        if let Some(page) = &page {
            page.set_title(&title);
            page.set_icon(Some(&icon));
        }
        if !self.pane().is_some_and(|p| Rc::ptr_eq(&p, pane)) {
            return;
        }
        if let Some(sidebar) = self.sidebar.borrow().as_ref() {
            sidebar.sync(&pane.here());
        }
        self.window.set_title(Some(&title));
        self.sync_toolbar();
    }

    fn sync_toolbar(&self) {
        let Some(pane) = self.pane() else { return };
        self.syncing.set(true);
        self.title.set_label(&pane.here().title());
        let shown = View::ALL
            .iter()
            .position(|v| *v == pane.view())
            .unwrap_or_default();
        self.views.set_property("active", shown as u32);
        self.preview.set_active(self.prefs.borrow().preview);
        if self.search.text() != pane.query() {
            self.search.set_text(&pane.query());
        }
        self.empty_trash.set_visible(pane.here().is_trash());
        self.syncing.set(false);
        self.set_state("view", pane.view().name().to_variant());
        self.set_state("preview", self.prefs.borrow().preview.to_variant());
        self.set_state("hidden", self.prefs.borrow().hidden.to_variant());
        let sorted_by = pane
            .column_view
            .sorter()
            .and_downcast::<gtk::ColumnViewSorter>()
            .and_then(|s| s.primary_sort_column())
            .and_then(|c| {
                pane.sort_columns
                    .iter()
                    .find(|(_, col)| *col == c)
                    .map(|(key, _)| *key)
            });
        if let Some(key) = sorted_by {
            self.set_state("sort", key.to_variant());
        }
        self.update_actions(&pane);
    }

    pub fn selection_changed(self: &Rc<Self>, pane: &Rc<Pane>) {
        if !self.pane().is_some_and(|p| Rc::ptr_eq(&p, pane)) {
            return;
        }
        let chosen = pane.selected();
        for (index, (tag, _)) in tags::TAGS.iter().enumerate() {
            let all = !chosen.is_empty() && chosen.iter().all(|i| tags::of(i).iter().any(|t| t == tag));
            self.set_state(&format!("tag-{index}"), all.to_variant());
        }
        if let Some(look) = self.quick_look.borrow().as_ref() {
            look.follow(&chosen);
        }
        self.update_actions(pane);
    }

    /// Which actions can be taken on what is chosen, where it is.
    fn update_actions(&self, pane: &Rc<Pane>) {
        let chosen = pane.selected();
        let here = pane.here();
        let trash = here.is_trash();
        let in_folder = here.folder().is_some_and(|f| f.path().is_some()) && !pane.is_searching();
        let some = !chosen.is_empty();
        let local = some
            && chosen
                .iter()
                .all(|i| info_file(i).is_some_and(|f| f.path().is_some()));
        let enable = |name: &str, on: bool| {
            if let Some(action) = self
                .actions
                .lookup_action(name)
                .and_downcast::<gio::SimpleAction>()
            {
                action.set_enabled(on);
            }
        };
        enable("back", pane.can_go_back());
        enable("forward", pane.can_go_forward());
        enable("up", here.folder().and_then(|f| f.parent()).is_some());
        enable("new-folder", in_folder);
        enable("paste", in_folder);
        enable("open", some && !trash);
        enable("quick-look", some);
        enable("info", true);
        for name in ["rename", "compress", "duplicate", "link", "copy", "cut"] {
            enable(name, local && !trash);
        }
        enable("trash", some && !trash);
        enable("restore", some && trash);
        enable("delete-now", some && trash);
        enable("reveal", some && chosen.len() == 1);
        let pictures = local
            && chosen
                .iter()
                .all(|i| i.content_type().is_some_and(|t| t.starts_with("image/")));
        enable("rotate", pictures);
        enable("wallpaper", pictures && chosen.len() == 1);
        for index in 0..tags::TAGS.len() {
            enable(&format!("tag-{index}"), local && !trash);
        }
        enable("all-tags", local && !trash);
        self.back.set_sensitive(pane.can_go_back());
        self.forward.set_sensitive(pane.can_go_forward());
    }

    fn set_state(&self, name: &str, state: glib::Variant) {
        if let Some(action) = self
            .actions
            .lookup_action(name)
            .and_downcast::<gio::SimpleAction>()
        {
            if action.state().as_ref() != Some(&state) {
                action.set_state(&state);
            }
        }
    }

    fn add_actions(self: &Rc<Self>) {
        let simple = |name: &str, run: Box<dyn Fn(&Rc<Window>, &Rc<Pane>)>| {
            let action = gio::SimpleAction::new(name, None);
            let weak = Rc::downgrade(self);
            action.connect_activate(move |_, _| {
                if let Some(win) = weak.upgrade() {
                    if let Some(pane) = win.pane() {
                        run(&win, &pane);
                    }
                }
            });
            self.actions.add_action(&action);
        };
        simple(
            "new-window",
            Box::new(|win, pane| {
                if let Some(app) = win.window.application().and_downcast::<adw::Application>() {
                    Window::open(&app, pane.here());
                }
            }),
        );
        simple(
            "new-tab",
            Box::new(|win, pane| {
                win.new_tab(pane.here());
            }),
        );
        simple(
            "close-tab",
            Box::new(|win, _| {
                if let Some(page) = win.tabs.selected_page() {
                    win.tabs.close_page(&page);
                }
            }),
        );
        simple("new-folder", Box::new(ops::new_folder));
        simple(
            "open",
            Box::new(|win, pane| ops::open(&Rc::downgrade(win), &pane.selected())),
        );
        simple(
            "open-tab",
            Box::new(|win, pane| {
                for file in pane.selected_files() {
                    win.new_tab(Where::Folder(file));
                }
            }),
        );
        simple(
            "info",
            Box::new(|win, pane| {
                let chosen = pane.selected();
                if chosen.is_empty() {
                    if let Some(info) = pane.here().folder().as_ref().and_then(pane::query) {
                        info::show(win, &info);
                    }
                }
                for info in chosen.iter().take(10) {
                    info::show(win, info);
                }
            }),
        );
        simple(
            "rename",
            Box::new(|win, pane| ops::rename(win, pane.selected_files())),
        );
        simple(
            "compress",
            Box::new(|win, pane| ops::compress(win, pane, &pane.selected_files())),
        );
        simple(
            "duplicate",
            Box::new(|_, pane| ops::duplicate(&pane.selected_files())),
        );
        simple(
            "link",
            Box::new(|win, pane| ops::make_links(win, &pane.selected_files())),
        );
        simple("quick-look", Box::new(|win, _| win.quick_look()));
        simple(
            "all-tags",
            Box::new(|win, pane| ops::edit_tags(win, pane.selected())),
        );
        simple(
            "trash",
            Box::new(|win, pane| ops::trash(win, pane.selected_files())),
        );
        simple(
            "restore",
            Box::new(|win, pane| ops::restore(win, &pane.selected())),
        );
        simple(
            "delete-now",
            Box::new(|win, pane| ops::delete_now(win, pane.selected_files())),
        );
        simple("empty-trash", Box::new(|win, _| ops::empty_trash(win)));
        simple("connect-server", Box::new(|win, _| server::ask(win)));
        simple(
            "copy",
            Box::new(|win, pane| ops::copy_to_clipboard(win, &pane.selected_files(), false)),
        );
        simple(
            "cut",
            Box::new(|win, pane| ops::copy_to_clipboard(win, &pane.selected_files(), true)),
        );
        simple(
            "paste",
            Box::new(|win, pane| {
                if let Some(folder) = pane.here().folder() {
                    ops::paste(win, folder);
                }
            }),
        );
        simple("select-all", Box::new(|_, pane| pane.select_all()));
        simple("back", Box::new(|_, pane| pane.step(-1)));
        simple("forward", Box::new(|_, pane| pane.step(1)));
        simple("up", Box::new(|_, pane| pane.go_up()));
        simple("goto", Box::new(|win, _| win.ask_folder()));
        simple(
            "reveal",
            Box::new(|_, pane| {
                if let Some(file) = pane.selected_files().into_iter().next() {
                    if let Some(folder) = file.parent() {
                        pane.go(Where::Folder(folder), true);
                        pane.select_when_listed(file, |_| {});
                    }
                }
            }),
        );
        simple(
            "bookmark",
            Box::new(|win, pane| {
                for file in pane.selected_files() {
                    ops::add_bookmark(win, &file);
                }
            }),
        );
        simple(
            "wallpaper",
            Box::new(|win, pane| {
                if let Some(file) = pane.selected_files().first() {
                    ops::set_wallpaper(win, file);
                }
            }),
        );
        simple(
            "rotate",
            Box::new(|win, pane| ops::rotate_left(win, pane.selected_files())),
        );
        simple(
            "terminal",
            Box::new(|win, pane| {
                let folder = pane.here().folder().and_then(|f| f.path()).unwrap_or_else(home);
                let spawned = std::process::Command::new("foot")
                    .arg("--working-directory")
                    .arg(&folder)
                    .spawn();
                if let Err(err) = spawned {
                    win.alert("無法打開終端機", Some(&err.to_string()));
                }
            }),
        );

        let open_with = gio::SimpleAction::new("open-with", Some(glib::VariantTy::STRING));
        open_with.connect_activate(on!(self, |w, _a, value| {
            if let (Some(id), Some(pane)) = (value.and_then(|v| v.get::<String>()), w.pane()) {
                ops::open_with(&w, &id, &pane.selected_files());
            }
        }));
        self.actions.add_action(&open_with);
        let go = gio::SimpleAction::new("go", Some(glib::VariantTy::STRING));
        go.connect_activate(on!(self, |w, _a, value| {
            let (Some(name), Some(pane)) = (value.and_then(|v| v.get::<String>()), w.pane()) else {
                return;
            };
            let special = |dir| glib::user_special_dir(dir).map(gio::File::for_path);
            let to = match name.as_str() {
                "recent" => Some(Where::Recent),
                "home" => Some(Where::Folder(gio::File::for_path(home()))),
                "desktop" => special(glib::UserDirectory::Desktop).map(Where::Folder),
                "documents" => special(glib::UserDirectory::Documents).map(Where::Folder),
                "downloads" => special(glib::UserDirectory::Downloads).map(Where::Folder),
                "trash" => Some(Where::Folder(pane::trash())),
                "network" => Some(Where::Folder(gio::File::for_uri("network:///"))),
                _ => None,
            };
            if let Some(to) = to {
                pane.go(to, true);
            }
        }));
        self.actions.add_action(&go);

        let stateful = |name: &str, state: glib::Variant, run: Box<dyn Fn(&Rc<Window>, &glib::Variant)>| {
            let action = gio::SimpleAction::new_stateful(
                name,
                Some(state.type_()).filter(|t| *t != glib::VariantTy::BOOLEAN),
                &state,
            );
            let weak = Rc::downgrade(self);
            action.connect_activate(move |action, value| {
                let Some(win) = weak.upgrade() else { return };
                // A check item toggles; a radio item says what it is.
                let wanted = match value {
                    Some(value) => value.clone(),
                    None => (!action.state().and_then(|s| s.get::<bool>()).unwrap_or(false)).to_variant(),
                };
                action.set_state(&wanted);
                run(&win, &wanted);
            });
            self.actions.add_action(&action);
        };
        let prefs = self.prefs.borrow();
        stateful(
            "view",
            prefs.view.name().to_variant(),
            Box::new(|win, value| {
                if let Some(view) = value.get::<String>().and_then(|v| View::from_name(&v)) {
                    win.set_view(view);
                }
            }),
        );
        stateful(
            "preview",
            prefs.preview.to_variant(),
            Box::new(|win, value| {
                win.set_preview(value.get::<bool>().unwrap_or(false));
            }),
        );
        stateful(
            "sidebar",
            prefs.sidebar.to_variant(),
            Box::new(|win, value| {
                win.split.set_show_sidebar(value.get::<bool>().unwrap_or(true));
            }),
        );
        stateful(
            "hidden",
            prefs.hidden.to_variant(),
            Box::new(|win, value| {
                win.prefs.borrow_mut().hidden = value.get::<bool>().unwrap_or(false);
                win.prefs.borrow().save();
                for (_, pane) in win.panes.borrow().iter() {
                    pane.refilter();
                }
            }),
        );
        stateful(
            "sort",
            "name".to_variant(),
            Box::new(|win, value| {
                let (Some(key), Some(pane)) = (value.get::<String>(), win.pane()) else {
                    return;
                };
                let Some((_, column)) = pane.sort_columns.iter().find(|(k, _)| *k == key) else {
                    return;
                };
                // Dates and sizes newest and largest first, as the Finder sorts them.
                let order = if matches!(key.as_str(), "modified" | "size") {
                    gtk::SortType::Descending
                } else {
                    gtk::SortType::Ascending
                };
                pane.column_view.sort_by_column(Some(column), order);
            }),
        );
        for (index, (tag, _)) in tags::TAGS.iter().enumerate() {
            stateful(
                &format!("tag-{index}"),
                false.to_variant(),
                Box::new(move |win, value| {
                    if let Some(pane) = win.pane() {
                        ops::set_tag(win, &pane.selected(), tag, value.get::<bool>().unwrap_or(false));
                        pane.refilter();
                        win.selection_changed(&pane);
                    }
                }),
            );
        }
        drop(prefs);
        self.window.insert_action_group("win", Some(&self.actions));
    }

    /// Adds or takes away a tag from what is chosen: on for all unless all have it.
    pub fn toggle_tag(self: &Rc<Self>, index: usize) {
        let name = format!("tag-{index}");
        gio::prelude::ActionGroupExt::activate_action(&self.actions, &name, None);
    }

    pub fn quick_look(self: &Rc<Self>) {
        let Some(pane) = self.pane() else { return };
        let look = self
            .quick_look
            .borrow_mut()
            .get_or_insert_with(|| quicklook::QuickLook::new(self))
            .clone();
        if look.window.is_visible() {
            look.window.close();
        } else {
            look.open_on(pane.selected());
        }
    }

    pub fn places_changed(&self) {
        if let Some(sidebar) = self.sidebar.borrow().as_ref() {
            sidebar.fill();
        }
    }

    /// 前往檔案夾: a typed path, from home with ~.
    fn ask_folder(self: &Rc<Self>) {
        let here = self
            .pane()
            .and_then(|p| p.here().folder())
            .and_then(|f| f.path())
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default();
        let weak = Rc::downgrade(self);
        let entry = alert::prompt(&self.window, "前往檔案夾", &here, -1, "前往", move |text| {
            let Some(win) = weak.upgrade() else { return };
            let path = match text.strip_prefix('~') {
                Some(rest) => home().join(rest.trim_start_matches('/')),
                None => PathBuf::from(&text),
            };
            let file = if text.contains("://") {
                gio::File::for_uri(&text)
            } else {
                gio::File::for_path(&path)
            };
            match file.query_file_type(gio::FileQueryInfoFlags::NONE, gio::Cancellable::NONE) {
                gio::FileType::Directory => {
                    if let Some(pane) = win.pane() {
                        pane.go(Where::Folder(file), true);
                    }
                }
                gio::FileType::Unknown => win.alert(&format!("找不到「{text}」"), None),
                _ => {
                    if let (Some(pane), Some(folder)) = (win.pane(), file.parent()) {
                        pane.go(Where::Folder(folder), true);
                        pane.select_when_listed(file, |_| {});
                    }
                }
            }
        });
        entry.set_placeholder_text(Some("~/文件"));
    }

    pub fn alert(&self, title: &str, body: Option<&str>) {
        alert::notice(&self.window, title, body);
    }

    /// Asks before something that cannot be undone; `then` does it.
    pub fn confirm(
        self: &Rc<Self>,
        title: &str,
        body: &str,
        verb: &str,
        then: impl FnOnce(&Rc<Window>) + 'static,
    ) {
        let weak = Rc::downgrade(self);
        alert::confirm(&self.window, title, Some(body), verb, move || {
            if let Some(win) = weak.upgrade() {
                then(&win);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_manager_interface_parses() {
        let node = gio::DBusNodeInfo::for_xml(MANAGER_XML).unwrap();
        let manager = node.lookup_interface(MANAGER_NAME).unwrap();
        for method in ["ShowFolders", "ShowItems", "ShowItemProperties"] {
            assert!(manager.lookup_method(method).is_some());
        }
    }
}
