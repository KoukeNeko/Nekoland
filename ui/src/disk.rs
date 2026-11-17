//! `nekoland-disk`, 磁碟用量: where a folder's space went. A folder or disk is scanned, then
//! shown as a sunburst or treemap beside a list of what is in it, which can be searched and
//! filtered. Items are looked at, shown in Files, or put on a cleanup list and then in the
//! trash together. Each scan is kept, to reopen and to compare with a later one.

use std::{
    cell::{Cell, RefCell},
    path::{Path, PathBuf},
    rc::Rc,
    sync::{atomic::Ordering, Arc},
    time::Duration,
};

use adw::prelude::*;
use gtk::{gdk, gio, glib, pango};
use serde::{Deserialize, Serialize};

mod alert;
#[path = "disk/chart.rs"]
mod chart;
#[path = "disk/scan.rs"]
mod scan;

use chart::{Item, Tile, Wedge};
use scan::{Kind, Tree, ROOT};

const APP_ID: &str = "io.github.koukeneko.Nekoland.Disk";
/// How often the scanning page reads how far the scan has gone.
const PROGRESS_EVERY: Duration = Duration::from_millis(120);
/// The list shows at most this many items; the chart shows all.
const LISTED: usize = 1000;
/// How much of a text file 快速查看 reads.
const TEXT_LIMIT: usize = 64 << 10;
/// The sizes the list can be filtered to, at least.
const AT_LEAST: [u64; 4] = [0, 1_000_000, 100_000_000, 1_000_000_000];

/// What is kept between runs, in ~/.config/nekoland/disk.json.
#[derive(Default, Serialize, Deserialize)]
struct Prefs {
    #[serde(default)]
    excluded: Vec<PathBuf>,
    #[serde(default)]
    show_free: bool,
    #[serde(default)]
    treemap: bool,
}

fn prefs_path() -> PathBuf {
    glib::user_config_dir().join("nekoland/disk.json")
}

impl Prefs {
    fn load() -> Prefs {
        std::fs::read(prefs_path())
            .ok()
            .and_then(|text| serde_json::from_slice(&text).ok())
            .unwrap_or_default()
    }

    fn save(&self) {
        let path = prefs_path();
        let written = std::fs::create_dir_all(path.parent().unwrap_or(Path::new("/")))
            .and_then(|()| std::fs::write(&path, serde_json::to_vec_pretty(self).unwrap_or_default()));
        if let Err(err) = written {
            eprintln!("{}: {err}", path.display());
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Sort {
    Name,
    Modified,
    Size,
}

struct Window {
    window: adw::ApplicationWindow,
    places: gtk::ListBox,
    recent: gtk::ListBox,
    stack: gtk::Stack,
    scanning_count: gtk::Label,
    scanning_at: gtk::Label,
    banner: adw::Banner,
    crumbs: gtk::Box,
    back: gtk::Button,
    forward: gtk::Button,
    rescan: gtk::Button,
    pile_button: gtk::Button,
    chart: gtk::DrawingArea,
    list: gtk::ListBox,
    search: gtk::SearchEntry,
    whole_scan: gtk::CheckButton,
    kind_filter: gtk::DropDown,
    size_filter: gtk::DropDown,
    sort_buttons: Vec<(Sort, gtk::Button)>,
    inspector: gtk::Box,
    menu: gtk::PopoverMenu,
    tree: RefCell<Option<Tree>>,
    /// A scan opened from a file or the sidebar, whose items may no longer be as it says.
    read_only: Cell<bool>,
    focus: Cell<usize>,
    backward_trail: RefCell<Vec<usize>>,
    forward_trail: RefCell<Vec<usize>>,
    hovered: Cell<Option<Item>>,
    chosen: Cell<Option<usize>>,
    sort: Cell<Sort>,
    listed: RefCell<Vec<usize>>,
    /// The chart's shapes, and what they were laid out for: the tree's version, the folder,
    /// the size and the kind of chart.
    wedges: RefCell<Vec<Wedge>>,
    tiles: RefCell<Vec<Tile>>,
    laid_out: Cell<(u64, usize, i32, i32, bool)>,
    version: Cell<u64>,
    pile: RefCell<Vec<usize>>,
    prefs: RefCell<Prefs>,
    progress: RefCell<Option<Arc<scan::Progress>>>,
}

impl Window {
    fn open(app: &adw::Application) -> Rc<Window> {
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("磁碟用量")
            .default_width(1180)
            .default_height(760)
            .build();

        let places = gtk::ListBox::builder().css_classes(["navigation-sidebar"]).build();
        let recent = gtk::ListBox::builder().css_classes(["navigation-sidebar"]).build();
        let choose = gtk::Button::builder().label("選擇檔案夾…").css_classes(["flat"]).build();
        let sidebar_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        sidebar_box.append(&heading("位置"));
        sidebar_box.append(&places);
        sidebar_box.append(&choose);
        sidebar_box.append(&heading("最近的掃描"));
        sidebar_box.append(&recent);
        let sidebar_view = adw::ToolbarView::new();
        sidebar_view.add_top_bar(&adw::HeaderBar::new());
        sidebar_view.set_content(Some(&gtk::ScrolledWindow::builder().child(&sidebar_box).vexpand(true).build()));
        let sidebar = adw::NavigationPage::builder().title("磁碟用量").child(&sidebar_view).build();

        let back = gtk::Button::builder().icon_name("go-previous-symbolic").tooltip_text("上一頁").sensitive(false).build();
        let forward = gtk::Button::builder().icon_name("go-next-symbolic").tooltip_text("下一頁").sensitive(false).build();
        let rescan = gtk::Button::builder().icon_name("view-refresh-symbolic").tooltip_text("重新掃描").visible(false).build();
        let pile_button = gtk::Button::builder().label("清理清單").visible(false).build();
        let menu_model = gio::Menu::new();
        let scans = gio::Menu::new();
        scans.append(Some("開啟掃描檔…"), Some("win.open-scan"));
        scans.append(Some("儲存掃描檔…"), Some("win.save-scan"));
        scans.append(Some("與先前的掃描比較…"), Some("win.compare"));
        menu_model.append_section(None, &scans);
        let view = gio::Menu::new();
        view.append(Some("矩形樹狀圖"), Some("win.treemap"));
        view.append(Some("顯示可用空間"), Some("win.show-free"));
        view.append(Some("排除的位置…"), Some("win.excluded"));
        menu_model.append_section(None, &view);
        let menu_button = gtk::MenuButton::builder().icon_name("open-menu-symbolic").menu_model(&menu_model).build();
        let header = adw::HeaderBar::new();
        header.pack_start(&back);
        header.pack_start(&forward);
        header.pack_end(&menu_button);
        header.pack_end(&pile_button);
        header.pack_end(&rescan);

        let empty = adw::StatusPage::builder()
            .icon_name("drive-harddisk-symbolic")
            .title("選擇要掃描的位置")
            .build();
        let scanning_count = gtk::Label::builder().css_classes(["title-2", "numeric"]).build();
        let scanning_at = gtk::Label::builder()
            .ellipsize(pango::EllipsizeMode::Middle)
            .max_width_chars(60)
            .css_classes(["dim-label"])
            .build();
        let stop = gtk::Button::builder().label("停止").halign(gtk::Align::Center).css_classes(["pill"]).build();
        let scanning = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .valign(gtk::Align::Center)
            .halign(gtk::Align::Center)
            .build();
        scanning.append(&gtk::Spinner::builder().spinning(true).width_request(32).height_request(32).build());
        scanning.append(&scanning_count);
        scanning.append(&scanning_at);
        scanning.append(&stop);

        let banner = adw::Banner::new("");
        let crumbs = gtk::Box::builder().spacing(2).margin_start(12).margin_end(12).margin_top(6).build();
        let chart = gtk::DrawingArea::builder().hexpand(true).vexpand(true).width_request(360).build();
        let chart_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        chart_box.append(&crumbs);
        chart_box.append(&chart);

        let search = gtk::SearchEntry::builder().placeholder_text("搜尋").hexpand(true).build();
        let whole_scan = gtk::CheckButton::builder().label("整個掃描").build();
        let kind_filter = gtk::DropDown::from_strings(&["所有種類", "檔案夾", "檔案"]);
        let size_filter = gtk::DropDown::from_strings(&["任何大小", "1 MB 以上", "100 MB 以上", "1 GB 以上"]);
        let filters = gtk::Box::builder().spacing(6).margin_start(8).margin_end(8).margin_top(8).build();
        filters.append(&search);
        filters.append(&whole_scan);
        let filters_two = gtk::Box::builder().spacing(6).margin_start(8).margin_end(8).build();
        filters_two.append(&kind_filter);
        filters_two.append(&size_filter);
        let columns = gtk::Box::builder().margin_start(8).margin_end(8).css_classes(["dk-columns"]).build();
        let mut sort_buttons = Vec::new();
        for (sort, title, grow) in [(Sort::Name, "名稱", true), (Sort::Modified, "修改日期", false), (Sort::Size, "大小", false)] {
            let button = gtk::Button::builder().label(title).hexpand(grow).css_classes(["flat", "dk-column"]).build();
            if let Some(label) = button.child().and_downcast::<gtk::Label>() {
                label.set_xalign(if grow { 0.0 } else { 1.0 });
            }
            if !grow {
                button.set_width_request(if sort == Sort::Size { 120 } else { 110 });
            }
            columns.append(&button);
            sort_buttons.push((sort, button));
        }
        let list = gtk::ListBox::builder().css_classes(["dk-list"]).build();
        let list_scroll = gtk::ScrolledWindow::builder().child(&list).vexpand(true).hscrollbar_policy(gtk::PolicyType::Never).build();
        let inspector = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(4)
            .margin_start(12)
            .margin_end(12)
            .margin_top(8)
            .margin_bottom(12)
            .build();
        let side = gtk::Box::new(gtk::Orientation::Vertical, 4);
        side.append(&filters);
        side.append(&filters_two);
        side.append(&columns);
        side.append(&list_scroll);
        side.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        side.append(&inspector);
        side.set_width_request(420);
        let paned = gtk::Paned::builder()
            .orientation(gtk::Orientation::Horizontal)
            .start_child(&chart_box)
            .end_child(&side)
            .resize_end_child(false)
            .shrink_end_child(false)
            .build();
        let result = gtk::Box::new(gtk::Orientation::Vertical, 0);
        result.append(&banner);
        result.append(&paned);

        let stack = gtk::Stack::new();
        stack.add_named(&empty, Some("empty"));
        stack.add_named(&scanning, Some("scanning"));
        stack.add_named(&result, Some("result"));
        let content_view = adw::ToolbarView::new();
        content_view.add_top_bar(&header);
        content_view.set_content(Some(&stack));
        let content = adw::NavigationPage::builder().title("磁碟用量").child(&content_view).build();
        let split = adw::NavigationSplitView::builder().sidebar(&sidebar).content(&content).build();
        window.set_content(Some(&split));

        let item_menu = gio::Menu::new();
        let first = gio::Menu::new();
        first.append(Some("快速查看"), Some("win.quick-look"));
        first.append(Some("在「檔案」中顯示"), Some("win.reveal"));
        first.append(Some("複製路徑"), Some("win.copy-path"));
        item_menu.append_section(None, &first);
        let last = gio::Menu::new();
        last.append(Some("加入清理清單"), Some("win.pile"));
        last.append(Some("丟到垃圾桶"), Some("win.trash"));
        item_menu.append_section(None, &last);
        let menu = gtk::PopoverMenu::from_model(Some(&item_menu));
        menu.set_has_arrow(false);
        menu.set_parent(&window);

        let win = Rc::new(Window {
            window,
            places,
            recent,
            stack,
            scanning_count,
            scanning_at,
            banner,
            crumbs,
            back,
            forward,
            rescan,
            pile_button,
            chart,
            list,
            search,
            whole_scan,
            kind_filter,
            size_filter,
            sort_buttons,
            inspector,
            menu,
            tree: RefCell::new(None),
            read_only: Cell::new(false),
            focus: Cell::new(ROOT),
            backward_trail: RefCell::new(Vec::new()),
            forward_trail: RefCell::new(Vec::new()),
            hovered: Cell::new(None),
            chosen: Cell::new(None),
            sort: Cell::new(Sort::Size),
            listed: RefCell::new(Vec::new()),
            wedges: RefCell::new(Vec::new()),
            tiles: RefCell::new(Vec::new()),
            laid_out: Cell::new((u64::MAX, 0, 0, 0, false)),
            version: Cell::new(0),
            pile: RefCell::new(Vec::new()),
            prefs: RefCell::new(Prefs::load()),
            progress: RefCell::new(None),
        });
        win.connect(&choose, &stop);
        win.add_actions();
        win.fill_places();
        win.fill_recent();
        win.show_sort();
        win.window.present();
        win
    }

    fn connect(self: &Rc<Self>, choose: &gtk::Button, stop: &gtk::Button) {
        let weak = Rc::downgrade(self);
        let me = move || weak.upgrade();

        let found = me.clone();
        choose.connect_clicked(move |_| {
            if let Some(win) = found() {
                win.choose_folder();
            }
        });
        let found = me.clone();
        stop.connect_clicked(move |_| {
            if let Some(progress) = found().and_then(|win| win.progress.borrow().clone()) {
                progress.stop.store(true, Ordering::Relaxed);
            }
        });
        let found = me.clone();
        self.places.connect_row_activated(move |_, row| {
            if let Some(win) = found() {
                let path = PathBuf::from(row.widget_name().as_str());
                win.scan(&path);
            }
        });
        let found = me.clone();
        self.recent.connect_row_activated(move |_, row| {
            if let Some(win) = found() {
                let path = PathBuf::from(row.widget_name().as_str());
                win.open_scan(&path);
            }
        });
        let found = me.clone();
        self.rescan.connect_clicked(move |_| {
            if let Some(win) = found() {
                let root = win.tree.borrow().as_ref().map(|tree| tree.root.clone());
                if let Some(root) = root {
                    win.scan(&root);
                }
            }
        });
        let found = me.clone();
        self.back.connect_clicked(move |_| {
            if let Some(win) = found() {
                win.step(true);
            }
        });
        let found = me.clone();
        self.forward.connect_clicked(move |_| {
            if let Some(win) = found() {
                win.step(false);
            }
        });
        let found = me.clone();
        self.pile_button.connect_clicked(move |_| {
            if let Some(win) = found() {
                win.show_pile();
            }
        });
        for (sort, button) in &self.sort_buttons {
            let (found, sort) = (me.clone(), *sort);
            button.connect_clicked(move |_| {
                if let Some(win) = found() {
                    win.sort.set(sort);
                    win.show_sort();
                    win.fill_list();
                }
            });
        }
        let found = me.clone();
        self.search.connect_search_changed(move |_| {
            if let Some(win) = found() {
                win.fill_list();
            }
        });
        for filter in [&self.kind_filter, &self.size_filter] {
            let found = me.clone();
            filter.connect_selected_notify(move |_| {
                if let Some(win) = found() {
                    win.fill_list();
                }
            });
        }
        let found = me.clone();
        self.whole_scan.connect_toggled(move |_| {
            if let Some(win) = found() {
                win.fill_list();
            }
        });
        let found = me.clone();
        self.list.connect_row_selected(move |_, row| {
            let Some(win) = found() else { return };
            let id = row.and_then(|row| win.listed.borrow().get(row.index() as usize).copied());
            if id.is_some() && id != win.chosen.get() {
                win.chosen.set(id);
                win.chart.queue_draw();
                win.fill_inspector();
            }
        });
        let found = me.clone();
        self.list.connect_row_activated(move |_, row| {
            let Some(win) = found() else { return };
            let Some(id) = win.listed.borrow().get(row.index() as usize).copied() else { return };
            win.go_into(id);
        });
        let list_click = gtk::GestureClick::builder().button(gdk::BUTTON_SECONDARY).build();
        let found = me.clone();
        list_click.connect_pressed(move |gesture, _, x, y| {
            let Some(win) = found() else { return };
            let Some(row) = win.list.row_at_y(y as i32) else { return };
            win.list.select_row(Some(&row));
            if let Some((x, y)) = in_window(&win.list, &win.window, x, y) {
                win.show_menu(x, y);
            }
            gesture.set_state(gtk::EventSequenceState::Claimed);
        });
        self.list.add_controller(list_click);
        let keys = gtk::EventControllerKey::new();
        let found = me.clone();
        keys.connect_key_pressed(move |_, key, _, _| {
            let Some(win) = found() else { return glib::Propagation::Proceed };
            if key == gdk::Key::space && !win.search.has_focus() {
                win.quick_look();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        self.list.add_controller(keys);

        let found = me.clone();
        self.chart.set_draw_func(move |area, cr, width, height| {
            if let Some(win) = found() {
                win.draw(area, cr, width, height);
            }
        });
        let motion = gtk::EventControllerMotion::new();
        let found = me.clone();
        motion.connect_motion(move |_, x, y| {
            if let Some(win) = found() {
                let item = win.item_at(x, y).flatten();
                if item != win.hovered.get() {
                    win.hovered.set(item);
                    win.chart.queue_draw();
                    win.fill_inspector();
                }
            }
        });
        let found = me.clone();
        motion.connect_leave(move |_| {
            if let Some(win) = found() {
                win.hovered.set(None);
                win.chart.queue_draw();
                win.fill_inspector();
            }
        });
        self.chart.add_controller(motion);
        let click = gtk::GestureClick::builder().button(0).build();
        let found = me.clone();
        click.connect_pressed(move |gesture, presses, x, y| {
            let Some(win) = found() else { return };
            let at = win.item_at(x, y);
            let secondary = gesture.current_button() == gdk::BUTTON_SECONDARY;
            match at {
                // The middle of a sunburst goes up a folder.
                Some(None) if !secondary => {
                    let parent = win.tree.borrow().as_ref().and_then(|tree| tree.nodes[win.focus.get()].parent);
                    if let Some(parent) = parent {
                        win.go_to(parent);
                    }
                }
                Some(Some(Item::Node(id))) => {
                    win.choose(id);
                    if secondary {
                        if let Some((x, y)) = in_window(&win.chart, &win.window, x, y) {
                            win.show_menu(x, y);
                        }
                    } else if presses == 2 {
                        win.go_into(id);
                    }
                }
                _ => {}
            }
        });
        self.chart.add_controller(click);

        let drop = gtk::DropTarget::new(gio::File::static_type(), gdk::DragAction::COPY);
        let found = me.clone();
        drop.connect_drop(move |_, value, _, _| {
            let Some(win) = found() else { return false };
            let Some(path) = value.get::<gio::File>().ok().and_then(|file| file.path()) else { return false };
            if !path.is_dir() {
                return false;
            }
            win.scan(&path);
            true
        });
        self.window.add_controller(drop);
    }

    fn add_actions(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        let add = |name: &str, does: Box<dyn Fn(&Rc<Window>)>| {
            let action = gio::SimpleAction::new(name, None);
            let weak = weak.clone();
            action.connect_activate(move |_, _| {
                if let Some(win) = weak.upgrade() {
                    does(&win);
                }
            });
            self.window.add_action(&action);
        };
        add("open-scan", Box::new(|win| win.choose_scan_file()));
        add("save-scan", Box::new(|win| win.save_scan()));
        add("compare", Box::new(|win| win.compare()));
        add("excluded", Box::new(|win| win.show_excluded()));
        add("quick-look", Box::new(|win| win.quick_look()));
        add("reveal", Box::new(|win| win.with_chosen(|path| show_in_files(path))));
        add(
            "copy-path",
            Box::new(|win| {
                let clipboard = win.window.clipboard();
                win.with_chosen(|path| clipboard.set_text(&path.display().to_string()));
            }),
        );
        add(
            "pile",
            Box::new(|win| {
                if let Some(id) = win.chosen.get() {
                    win.add_to_pile(id);
                }
            }),
        );
        add(
            "trash",
            Box::new(|win| {
                if let Some(id) = win.chosen.get() {
                    win.trash(vec![id]);
                }
            }),
        );
        for (name, on) in [("treemap", self.prefs.borrow().treemap), ("show-free", self.prefs.borrow().show_free)] {
            let action = gio::SimpleAction::new_stateful(name, None, &on.to_variant());
            let weak = weak.clone();
            let name = name.to_string();
            action.connect_activate(move |action, _| {
                let on = !action.state().and_then(|state| state.get::<bool>()).unwrap_or(false);
                action.set_state(&on.to_variant());
                let Some(win) = weak.upgrade() else { return };
                {
                    let mut prefs = win.prefs.borrow_mut();
                    if name == "treemap" {
                        prefs.treemap = on;
                    } else {
                        prefs.show_free = on;
                    }
                    prefs.save();
                }
                win.changed();
            });
            self.window.add_action(&action);
        }
        self.update_actions();
    }

    /// The item actions, as what is chosen allows them.
    fn update_actions(&self) {
        let chosen = self.chosen.get().and_then(|id| {
            let tree = self.tree.borrow();
            tree.as_ref().map(|tree| (tree.nodes[id].kind, id != ROOT))
        });
        let real = chosen.is_some_and(|(kind, _)| kind != Kind::Small);
        let movable = real && chosen.is_some_and(|(_, not_root)| not_root) && !self.read_only.get();
        let has_tree = self.tree.borrow().is_some();
        for (name, on) in [
            ("quick-look", real),
            ("reveal", real),
            ("copy-path", real),
            ("pile", movable),
            ("trash", movable),
            ("save-scan", has_tree),
            ("compare", has_tree),
        ] {
            if let Some(action) = self.window.lookup_action(name).and_downcast::<gio::SimpleAction>() {
                action.set_enabled(on);
            }
        }
    }

    fn fill_places(&self) {
        let home = glib::home_dir();
        let mut places: Vec<(String, PathBuf, &str)> = vec![("個人專屬檔案夾".into(), home.clone(), "user-home-symbolic")];
        for (directory, icon) in [
            (glib::UserDirectory::Desktop, "user-desktop-symbolic"),
            (glib::UserDirectory::Documents, "folder-documents-symbolic"),
            (glib::UserDirectory::Downloads, "folder-download-symbolic"),
        ] {
            if let Some(path) = glib::user_special_dir(directory).filter(|path| *path != home && path.is_dir()) {
                let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
                places.push((name, path, icon));
            }
        }
        places.push(("系統磁碟".into(), PathBuf::from("/"), "drive-harddisk-symbolic"));
        for mount in gio::VolumeMonitor::get().mounts() {
            if let Some(path) = mount.root().path().filter(|path| path != Path::new("/")) {
                places.push((mount.name().to_string(), path, "drive-removable-media-symbolic"));
            }
        }
        for (name, path, icon) in places {
            let row = adw::ActionRow::builder().title(&name).activatable(true).build();
            row.add_prefix(&gtk::Image::from_icon_name(icon));
            row.set_widget_name(&path.to_string_lossy());
            self.places.append(&row);
        }
    }

    fn fill_recent(self: &Rc<Self>) {
        self.recent.remove_all();
        for kept in scan::kept() {
            let name = kept.root.file_name().map_or_else(|| kept.root.display().to_string(), |name| name.to_string_lossy().into_owned());
            let row = adw::ActionRow::builder()
                .title(&name)
                .subtitle(format!("{}，{}", date_time(kept.scanned), glib::format_size(kept.size)))
                .activatable(true)
                .build();
            row.set_widget_name(&kept.path.to_string_lossy());
            let forget = gtk::Button::builder()
                .icon_name("window-close-symbolic")
                .tooltip_text("從最近的掃描中移除")
                .valign(gtk::Align::Center)
                .css_classes(["flat", "circular"])
                .build();
            let weak = Rc::downgrade(self);
            let path = kept.path.clone();
            forget.connect_clicked(move |_| {
                if let Err(err) = scan::forget(&path) {
                    eprintln!("{}: {err}", path.display());
                }
                if let Some(win) = weak.upgrade() {
                    win.fill_recent();
                }
            });
            row.add_suffix(&forget);
            self.recent.append(&row);
        }
    }

    fn choose_folder(self: &Rc<Self>) {
        let dialog = gtk::FileDialog::builder().title("選擇要掃描的檔案夾").modal(true).build();
        let weak = Rc::downgrade(self);
        dialog.select_folder(Some(&self.window), gio::Cancellable::NONE, move |chosen| {
            if let (Some(win), Some(path)) = (weak.upgrade(), chosen.ok().and_then(|file| file.path())) {
                win.scan(&path);
            }
        });
    }

    /// Scans `root` on a thread of its own, showing how far it has gone, then shows it and
    /// keeps it.
    fn scan(self: &Rc<Self>, root: &Path) {
        if let Some(progress) = self.progress.borrow().as_ref() {
            progress.stop.store(true, Ordering::Relaxed);
        }
        let progress = Arc::new(scan::Progress::default());
        self.progress.replace(Some(progress.clone()));
        self.scanning_count.set_label("");
        self.scanning_at.set_label(&root.display().to_string());
        self.stack.set_visible_child_name("scanning");
        self.rescan.set_visible(false);
        let (sender, receiver) = std::sync::mpsc::channel();
        let (root, excluded, reading) = (root.to_path_buf(), self.prefs.borrow().excluded.clone(), progress.clone());
        std::thread::spawn(move || {
            let tree = scan::scan(&root, &excluded, &reading);
            // Kept on this thread too, as a large tree takes a while to write.
            if let Some(tree) = &tree {
                if let Err(err) = scan::keep(tree) {
                    eprintln!("keeping the scan of {}: {err}", root.display());
                }
            }
            // The window may have closed; then there is no one to tell.
            let _ = sender.send(tree);
        });
        let weak = Rc::downgrade(self);
        glib::timeout_add_local(PROGRESS_EVERY, move || {
            let Some(win) = weak.upgrade() else { return glib::ControlFlow::Break };
            if !win.progress.borrow().as_ref().is_some_and(|shown| Arc::ptr_eq(shown, &progress)) {
                return glib::ControlFlow::Break;
            }
            match receiver.try_recv() {
                Ok(tree) => {
                    win.progress.replace(None);
                    match tree {
                        Some(tree) => {
                            win.read_only.set(false);
                            win.show_tree(tree);
                            win.fill_recent();
                        }
                        None => win.stack.set_visible_child_name(if win.tree.borrow().is_some() { "result" } else { "empty" }),
                    }
                    win.rescan.set_visible(win.tree.borrow().is_some());
                    glib::ControlFlow::Break
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    let files = progress.files.load(Ordering::Relaxed);
                    let bytes = progress.bytes.load(Ordering::Relaxed);
                    win.scanning_count.set_label(&format!("{files} 個檔案，{}", glib::format_size(bytes)));
                    if let Ok(at) = progress.at.lock() {
                        win.scanning_at.set_label(&at.display().to_string());
                    }
                    glib::ControlFlow::Continue
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
            }
        });
    }

    fn open_scan(self: &Rc<Self>, path: &Path) {
        match scan::load(path) {
            Ok(tree) => {
                self.read_only.set(true);
                self.show_tree(tree);
                self.rescan.set_visible(true);
            }
            Err(why) => alert::notice(&self.window, "無法開啟掃描檔", Some(&why)),
        }
    }

    fn choose_scan_file(self: &Rc<Self>) {
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("掃描檔"));
        filter.add_suffix(scan::EXTENSION);
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        let dialog = gtk::FileDialog::builder().title("開啟掃描檔").filters(&filters).modal(true).build();
        let weak = Rc::downgrade(self);
        dialog.open(Some(&self.window), gio::Cancellable::NONE, move |chosen| {
            if let (Some(win), Some(path)) = (weak.upgrade(), chosen.ok().and_then(|file| file.path())) {
                win.open_scan(&path);
            }
        });
    }

    fn save_scan(self: &Rc<Self>) {
        let Some((name, scanned)) = self.tree.borrow().as_ref().map(|tree| {
            let name = tree.root.file_name().map_or("root".into(), |name| name.to_string_lossy().into_owned());
            (name, tree.scanned)
        }) else {
            return;
        };
        let dialog = gtk::FileDialog::builder()
            .title("儲存掃描檔")
            .initial_name(format!("{name} {}.{}", date(scanned).replace('/', "-"), scan::EXTENSION))
            .modal(true)
            .build();
        let weak = Rc::downgrade(self);
        dialog.save(Some(&self.window), gio::Cancellable::NONE, move |chosen| {
            let (Some(win), Some(path)) = (weak.upgrade(), chosen.ok().and_then(|file| file.path())) else { return };
            let saved = win.tree.borrow().as_ref().map(|tree| scan::save(tree, &path));
            if let Some(Err(err)) = saved {
                alert::notice(&win.window, "無法儲存掃描檔", Some(&err.to_string()));
            }
        });
    }

    fn show_tree(self: &Rc<Self>, tree: Tree) {
        let denied = tree.denied;
        self.tree.replace(Some(tree));
        self.focus.set(ROOT);
        self.backward_trail.borrow_mut().clear();
        self.forward_trail.borrow_mut().clear();
        self.chosen.set(None);
        self.hovered.set(None);
        self.pile.borrow_mut().clear();
        self.show_pile_button();
        self.banner.set_title(&format!("有 {denied} 個檔案夾無法讀取，未計入"));
        self.banner.set_revealed(denied > 0);
        self.stack.set_visible_child_name("result");
        self.changed();
    }

    /// Shows the tree again after it, or how it is shown, changed.
    fn changed(&self) {
        self.version.set(self.version.get() + 1);
        self.chart.queue_draw();
        self.fill_crumbs();
        self.fill_list();
        self.fill_inspector();
        self.update_actions();
    }

    fn go_to(&self, id: usize) {
        if id == self.focus.get() {
            return;
        }
        self.backward_trail.borrow_mut().push(self.focus.get());
        self.forward_trail.borrow_mut().clear();
        self.focus.set(id);
        self.chosen.set(None);
        self.changed();
    }

    /// Opens a folder in the chart, or looks at a file.
    fn go_into(&self, id: usize) {
        let kind = self.tree.borrow().as_ref().map(|tree| tree.nodes[id].kind);
        match kind {
            Some(Kind::Folder) => self.go_to(id),
            Some(Kind::File) => {
                self.chosen.set(Some(id));
                self.quick_look();
            }
            _ => {}
        }
    }

    fn step(&self, backward: bool) {
        let (from, to) = if backward {
            (&self.backward_trail, &self.forward_trail)
        } else {
            (&self.forward_trail, &self.backward_trail)
        };
        let Some(id) = from.borrow_mut().pop() else { return };
        to.borrow_mut().push(self.focus.get());
        self.focus.set(id);
        self.chosen.set(None);
        self.changed();
    }

    fn choose(&self, id: usize) {
        self.chosen.set(Some(id));
        let index = self.listed.borrow().iter().position(|listed| *listed == id);
        match index.and_then(|index| self.list.row_at_index(index as i32)) {
            Some(row) => self.list.select_row(Some(&row)),
            None => self.list.unselect_all(),
        }
        self.chart.queue_draw();
        self.fill_inspector();
        self.update_actions();
    }

    fn fill_crumbs(&self) {
        while let Some(child) = self.crumbs.first_child() {
            self.crumbs.remove(&child);
        }
        self.back.set_sensitive(!self.backward_trail.borrow().is_empty());
        self.forward.set_sensitive(!self.forward_trail.borrow().is_empty());
        let tree = self.tree.borrow();
        let Some(tree) = tree.as_ref() else { return };
        let trail = tree.trail(self.focus.get());
        for (index, id) in trail.iter().enumerate() {
            if index > 0 {
                self.crumbs.append(&gtk::Image::from_icon_name("go-next-symbolic"));
            }
            let name = if *id == ROOT { tree.root.display().to_string() } else { tree.nodes[*id].name.clone() };
            let button = gtk::Button::builder().label(name).css_classes(["flat"]).build();
            if index + 1 == trail.len() {
                button.add_css_class("dk-here");
            }
            let id = *id;
            let crumbs = self.crumbs.clone();
            button.connect_clicked(move |_| {
                crumbs.activate_action("win.go-to", Some(&(id as u64).to_variant())).ok();
            });
            self.crumbs.append(&button);
        }
    }

    fn show_sort(&self) {
        for (sort, button) in &self.sort_buttons {
            if *sort == self.sort.get() {
                button.add_css_class("dk-sorted");
            } else {
                button.remove_css_class("dk-sorted");
            }
        }
    }

    /// What is in the folder shown, or what the search found, filtered and sorted.
    fn fill_list(&self) {
        self.list.remove_all();
        let tree = self.tree.borrow();
        let Some(tree) = tree.as_ref() else { return };
        let text = self.search.text();
        let searching = !text.trim().is_empty();
        let mut items = if searching {
            tree.find(if self.whole_scan.is_active() { ROOT } else { self.focus.get() }, text.trim())
        } else {
            tree.children(self.focus.get())
        };
        let at_least = AT_LEAST[self.size_filter.selected() as usize % AT_LEAST.len()];
        let kind = match self.kind_filter.selected() {
            1 => Some(Kind::Folder),
            2 => Some(Kind::File),
            _ => None,
        };
        items.retain(|id| {
            let node = &tree.nodes[*id];
            node.size >= at_least && kind.is_none_or(|kind| node.kind == kind)
        });
        match self.sort.get() {
            Sort::Size => {}
            Sort::Name => items.sort_by_cached_key(|id| tree.nodes[*id].name.to_lowercase()),
            Sort::Modified => items.sort_by_key(|id| std::cmp::Reverse(tree.nodes[*id].modified)),
        }
        items.truncate(LISTED);
        let largest = items.iter().map(|id| tree.nodes[*id].size).max().unwrap_or(1).max(1);
        for id in &items {
            let node = &tree.nodes[*id];
            let line = gtk::Box::builder().spacing(8).css_classes(["dk-line"]).build();
            line.append(&gtk::Image::from_icon_name(match node.kind {
                Kind::Folder => "folder-symbolic",
                Kind::File => "text-x-generic-symbolic",
                Kind::Small => "view-more-horizontal-symbolic",
            }));
            let names = gtk::Box::builder().orientation(gtk::Orientation::Vertical).hexpand(true).valign(gtk::Align::Center).build();
            names.append(&gtk::Label::builder().label(&node.name).xalign(0.0).ellipsize(pango::EllipsizeMode::Middle).build());
            if searching {
                let parent = node.parent.map(|parent| tree.path(parent).display().to_string()).unwrap_or_default();
                names.append(
                    &gtk::Label::builder()
                        .label(parent)
                        .xalign(0.0)
                        .ellipsize(pango::EllipsizeMode::Start)
                        .css_classes(["dim-label", "caption"])
                        .build(),
                );
            }
            line.append(&names);
            line.append(
                &gtk::Label::builder()
                    .label(if node.kind == Kind::Small { String::new() } else { date(node.modified) })
                    .width_request(110)
                    .xalign(1.0)
                    .css_classes(["dim-label", "numeric"])
                    .build(),
            );
            let sized = gtk::Box::builder().orientation(gtk::Orientation::Vertical).width_request(120).valign(gtk::Align::Center).build();
            sized.append(&gtk::Label::builder().label(glib::format_size(node.size)).xalign(1.0).css_classes(["numeric"]).build());
            sized.append(&gtk::LevelBar::builder().value(node.size as f64 / largest as f64).css_classes(["dk-bar"]).build());
            line.append(&sized);
            self.list.append(&line);
        }
        self.listed.replace(items);
        if let Some(index) = self.chosen.get().and_then(|id| self.listed.borrow().iter().position(|listed| *listed == id)) {
            self.list.select_row(self.list.row_at_index(index as i32).as_ref());
        }
    }

    /// About what the pointer is on, or else what is chosen, or else the folder shown.
    fn fill_inspector(&self) {
        while let Some(child) = self.inspector.first_child() {
            self.inspector.remove(&child);
        }
        let tree = self.tree.borrow();
        let Some(tree) = tree.as_ref() else { return };
        let item = self.hovered.get().or(self.chosen.get().map(Item::Node)).unwrap_or(Item::Node(self.focus.get()));
        let Item::Node(id) = item else {
            let (free, total) = tree.free.unwrap_or_default();
            self.inspector.append(&title_label("可用空間"));
            self.inspector.append(&fact("大小", &glib::format_size(free)));
            self.inspector.append(&fact("磁碟容量", &glib::format_size(total)));
            return;
        };
        let node = &tree.nodes[id];
        let name = if id == ROOT { tree.root.display().to_string() } else { node.name.clone() };
        self.inspector.append(&title_label(&name));
        self.inspector.append(&fact("大小", &glib::format_size(node.size)));
        if node.kind != Kind::File {
            self.inspector.append(&fact("檔案", &node.files.to_string()));
        }
        if let Some(parent) = node.parent {
            let share = node.size as f64 * 100.0 / tree.nodes[parent].size.max(1) as f64;
            self.inspector.append(&fact("占所在檔案夾", &format!("{share:.1}%")));
            self.inspector.append(&fact("位置", &tree.path(parent).display().to_string()));
        }
        if node.kind != Kind::Small {
            self.inspector.append(&fact("修改日期", &date(node.modified)));
            if let Some(access) = access(&tree.path(id)) {
                self.inspector.append(&fact("權限", &access));
            }
        }
        if node.kind == Kind::Folder {
            let largest = tree.largest_files(id, 5);
            if !largest.is_empty() {
                self.inspector.append(&gtk::Label::builder().label("最大的檔案").xalign(0.0).css_classes(["heading"]).margin_top(6).build());
                for file in largest {
                    self.inspector.append(&fact(&tree.nodes[file].name, &glib::format_size(tree.nodes[file].size)));
                }
            }
        }
    }

    /// What is under `(x, y)` on the chart: an item, the sunburst's middle as Some(None), or
    /// None.
    fn item_at(&self, x: f64, y: f64) -> Option<Option<Item>> {
        let (width, height) = (self.chart.width() as f64, self.chart.height() as f64);
        if self.prefs.borrow().treemap {
            return chart::tile_at(&self.tiles.borrow(), x, y).map(|tile| Some(tile.item));
        }
        let radius = width.min(height) / 2.0 - 8.0;
        chart::wedge_at(&self.wedges.borrow(), radius, x - width / 2.0, y - height / 2.0).map(|wedge| wedge.map(|wedge| wedge.item))
    }

    fn draw(&self, area: &gtk::DrawingArea, cr: &gtk::cairo::Context, width: i32, height: i32) {
        let tree = self.tree.borrow();
        let Some(tree) = tree.as_ref() else { return };
        let treemap = self.prefs.borrow().treemap;
        let focus = self.focus.get();
        let free = (self.prefs.borrow().show_free && focus == ROOT).then_some(tree.free.map(|(free, _)| free)).flatten();
        let wanted = (self.version.get(), focus, width, height, treemap);
        if self.laid_out.get() != wanted {
            if treemap {
                self.tiles.replace(chart::treemap(tree, focus, width as f64, height as f64, free));
            } else {
                self.wedges.replace(chart::sunburst(tree, focus, free));
            }
            self.laid_out.set(wanted);
        }
        // The scanned folder by its own name, which fits the middle; the crumbs give its path.
        let name = match focus {
            ROOT => tree.root.file_name().map_or_else(|| tree.root.display().to_string(), |name| name.to_string_lossy().into_owned()),
            _ => tree.nodes[focus].name.clone(),
        };
        let size = glib::format_size(tree.nodes[focus].size);
        let look = chart::Look {
            dark: adw::StyleManager::default().is_dark(),
            hovered: self.hovered.get(),
            chosen: self.chosen.get().map(Item::Node),
            middle: (&name, &size),
            ink: area.color(),
        };
        let layout = area.create_pango_layout(None);
        if treemap {
            chart::draw_treemap(cr, &layout, tree, &self.tiles.borrow(), &look);
        } else {
            chart::draw_sunburst(cr, &layout, &self.wedges.borrow(), width as f64, height as f64, &look);
        }
    }

    fn show_menu(&self, x: f64, y: f64) {
        self.update_actions();
        self.menu.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        self.menu.popup();
    }

    fn with_chosen(&self, does: impl FnOnce(&Path)) {
        let path = self.chosen.get().and_then(|id| self.tree.borrow().as_ref().map(|tree| tree.path(id)));
        if let Some(path) = path {
            does(&path);
        }
    }

    fn add_to_pile(&self, id: usize) {
        let tree = self.tree.borrow();
        let Some(tree) = tree.as_ref() else { return };
        let mut pile = self.pile.borrow_mut();
        // What is in a folder on the list goes with it already.
        if pile.iter().any(|kept| tree.is_under(id, *kept)) {
            return;
        }
        pile.retain(|kept| !tree.is_under(*kept, id));
        pile.push(id);
        drop(pile);
        self.show_pile_button();
    }

    fn show_pile_button(&self) {
        let pile = self.pile.borrow();
        let tree = self.tree.borrow();
        let size: u64 = tree.as_ref().map_or(0, |tree| pile.iter().map(|id| tree.nodes[*id].size).sum());
        self.pile_button.set_visible(!pile.is_empty());
        self.pile_button.set_label(&format!("清理清單（{}）", glib::format_size(size)));
    }

    fn show_pile(self: &Rc<Self>) {
        let page = adw::PreferencesPage::new();
        let rows = adw::PreferencesGroup::new();
        page.add(&rows);
        let dialog_weak: Rc<RefCell<Option<glib::WeakRef<adw::Dialog>>>> = Rc::new(RefCell::new(None));
        {
            let tree = self.tree.borrow();
            let Some(tree) = tree.as_ref() else { return };
            for id in self.pile.borrow().iter().copied() {
                let row = adw::ActionRow::builder()
                    .title(&tree.nodes[id].name)
                    .subtitle(tree.path(id).display().to_string())
                    .build();
                row.add_suffix(&gtk::Label::builder().label(glib::format_size(tree.nodes[id].size)).css_classes(["dim-label", "numeric"]).build());
                let remove = gtk::Button::builder()
                    .icon_name("list-remove-symbolic")
                    .tooltip_text("從清理清單中移除")
                    .valign(gtk::Align::Center)
                    .css_classes(["flat", "circular"])
                    .build();
                let (weak, group, shown) = (Rc::downgrade(self), rows.clone(), row.clone());
                let closer = dialog_weak.clone();
                remove.connect_clicked(move |_| {
                    let Some(win) = weak.upgrade() else { return };
                    win.pile.borrow_mut().retain(|kept| *kept != id);
                    win.show_pile_button();
                    group.remove(&shown);
                    if win.pile.borrow().is_empty() {
                        if let Some(dialog) = closer.borrow().as_ref().and_then(|dialog| dialog.upgrade()) {
                            dialog.close();
                        }
                    }
                });
                row.add_suffix(&remove);
                rows.add(&row);
            }
        }
        let trash = gtk::Button::builder()
            .label("丟到垃圾桶")
            .halign(gtk::Align::Center)
            .css_classes(["destructive-action", "pill"])
            .build();
        let end = adw::PreferencesGroup::new();
        end.add(&trash);
        page.add(&end);
        let view = adw::ToolbarView::new();
        view.add_top_bar(&adw::HeaderBar::new());
        view.set_content(Some(&page));
        let dialog = adw::Dialog::builder().title("清理清單").content_width(560).content_height(480).child(&view).build();
        dialog_weak.replace(Some(dialog.downgrade()));
        let weak = Rc::downgrade(self);
        trash.connect_clicked(glib::clone!(
            #[weak]
            dialog,
            move |_| {
                if let Some(win) = weak.upgrade() {
                    dialog.close();
                    let pile = win.pile.borrow().clone();
                    win.trash(pile);
                }
            }
        ));
        dialog.present(Some(&self.window));
    }

    /// Puts each item in the trash, takes it out of the scan, and tells what could not be.
    fn trash(&self, ids: Vec<usize>) {
        let paths: Vec<(usize, PathBuf)> = {
            let tree = self.tree.borrow();
            let Some(tree) = tree.as_ref() else { return };
            ids.iter().map(|id| (*id, tree.path(*id))).collect()
        };
        let mut failed = Vec::new();
        for (id, path) in paths {
            match gio::File::for_path(&path).trash(gio::Cancellable::NONE) {
                Ok(()) => {
                    if let Some(tree) = self.tree.borrow_mut().as_mut() {
                        tree.remove(id);
                    }
                    self.pile.borrow_mut().retain(|kept| *kept != id);
                }
                Err(err) => failed.push(format!("{}：{}", path.display(), err.message())),
            }
        }
        // Gone from the folder shown, or the folder itself gone: the nearest one left is shown.
        let focus = self.focus.get();
        let still = self.tree.borrow().as_ref().map_or(ROOT, |tree| {
            tree.trail(focus).into_iter().take_while(|id| *id == ROOT || tree.nodes[*id].parent.is_some_and(|parent| tree.nodes[parent].children.contains(id))).last().unwrap_or(ROOT)
        });
        self.focus.set(still);
        self.chosen.set(None);
        self.hovered.set(None);
        self.show_pile_button();
        self.changed();
        if !failed.is_empty() {
            alert::notice(&self.window, "有些項目無法丟到垃圾桶", Some(&failed.join("\n")));
        }
    }

    fn quick_look(&self) {
        let Some(id) = self.chosen.get() else { return };
        let (path, size, kind) = {
            let tree = self.tree.borrow();
            let Some(tree) = tree.as_ref() else { return };
            (tree.path(id), tree.nodes[id].size, tree.nodes[id].kind)
        };
        if kind == Kind::Small {
            return;
        }
        let file = gio::File::for_path(&path);
        let info = file.query_info("standard::content-type,standard::icon", gio::FileQueryInfoFlags::NONE, gio::Cancellable::NONE).ok();
        let content_type = info.as_ref().and_then(|info| info.content_type()).map(|kind| kind.to_string()).unwrap_or_default();
        let body: gtk::Widget = if content_type.starts_with("image/") {
            gtk::Picture::builder().file(&file).content_fit(gtk::ContentFit::Contain).hexpand(true).vexpand(true).build().upcast()
        } else if gio::content_type_is_a(&content_type, "text/plain") {
            let mut text = Vec::new();
            if let Ok(opened) = std::fs::File::open(&path) {
                use std::io::Read;
                if let Err(err) = opened.take(TEXT_LIMIT as u64).read_to_end(&mut text) {
                    eprintln!("{}: {err}", path.display());
                }
            }
            let view = gtk::TextView::builder().editable(false).monospace(true).wrap_mode(gtk::WrapMode::WordChar).left_margin(12).right_margin(12).top_margin(12).build();
            view.buffer().set_text(&String::from_utf8_lossy(&text));
            gtk::ScrolledWindow::builder().child(&view).hexpand(true).vexpand(true).build().upcast()
        } else {
            let icon = gtk::Image::builder().pixel_size(128).build();
            if let Some(gicon) = info.as_ref().and_then(|info| info.icon()) {
                icon.set_from_gicon(&gicon);
            }
            let about = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(12).valign(gtk::Align::Center).vexpand(true).build();
            about.append(&icon);
            about.append(&gtk::Label::builder().label(glib::format_size(size)).css_classes(["dim-label"]).build());
            about.upcast()
        };
        let open = gtk::Button::builder().label("打開").build();
        let header = adw::HeaderBar::new();
        header.pack_end(&open);
        let view = adw::ToolbarView::new();
        view.add_top_bar(&header);
        view.set_content(Some(&body));
        let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        let dialog = adw::Dialog::builder().title(&name).content_width(720).content_height(560).child(&view).build();
        open.connect_clicked(glib::clone!(
            #[weak]
            dialog,
            move |_| {
                if let Err(err) = gio::AppInfo::launch_default_for_uri(&file.uri(), gio::AppLaunchContext::NONE) {
                    eprintln!("{}: {err}", path.display());
                }
                dialog.close();
            }
        ));
        dialog.present(Some(&self.window));
    }

    fn show_excluded(self: &Rc<Self>) {
        let page = adw::PreferencesPage::new();
        let rows = adw::PreferencesGroup::builder().description("掃描時略過這些位置").build();
        page.add(&rows);
        let shown = Rc::new(RefCell::new(Vec::<adw::ActionRow>::new()));
        let fill: Rc<RefCell<Option<Box<dyn Fn()>>>> = Rc::new(RefCell::new(None));
        let weak = Rc::downgrade(self);
        let refill = {
            let (rows, shown, fill) = (rows.clone(), shown.clone(), fill.clone());
            move || {
                let Some(win) = weak.upgrade() else { return };
                for row in shown.take() {
                    rows.remove(&row);
                }
                for path in win.prefs.borrow().excluded.clone() {
                    let row = adw::ActionRow::builder().title(path.display().to_string()).build();
                    let remove = gtk::Button::builder()
                        .icon_name("list-remove-symbolic")
                        .tooltip_text("移除")
                        .valign(gtk::Align::Center)
                        .css_classes(["flat", "circular"])
                        .build();
                    let (weak, fill) = (Rc::downgrade(&win), fill.clone());
                    remove.connect_clicked(move |_| {
                        if let Some(win) = weak.upgrade() {
                            let mut prefs = win.prefs.borrow_mut();
                            prefs.excluded.retain(|kept| *kept != path);
                            prefs.save();
                        }
                        if let Some(fill) = fill.borrow().as_ref() {
                            fill();
                        }
                    });
                    row.add_suffix(&remove);
                    rows.add(&row);
                    shown.borrow_mut().push(row);
                }
            }
        };
        refill();
        fill.replace(Some(Box::new(refill)));
        let add = gtk::Button::builder().label("新增…").halign(gtk::Align::Center).css_classes(["pill"]).build();
        let end = adw::PreferencesGroup::new();
        end.add(&add);
        page.add(&end);
        let view = adw::ToolbarView::new();
        view.add_top_bar(&adw::HeaderBar::new());
        view.set_content(Some(&page));
        let dialog = adw::Dialog::builder().title("排除的位置").content_width(520).content_height(420).child(&view).build();
        let weak = Rc::downgrade(self);
        add.connect_clicked(move |_| {
            let Some(win) = weak.upgrade() else { return };
            let chooser = gtk::FileDialog::builder().title("選擇要排除的檔案夾").modal(true).build();
            let (weak, fill) = (Rc::downgrade(&win), fill.clone());
            chooser.select_folder(Some(&win.window), gio::Cancellable::NONE, move |chosen| {
                let (Some(win), Some(path)) = (weak.upgrade(), chosen.ok().and_then(|file| file.path())) else { return };
                {
                    let mut prefs = win.prefs.borrow_mut();
                    if !prefs.excluded.contains(&path) {
                        prefs.excluded.push(path);
                        prefs.save();
                    }
                }
                if let Some(fill) = fill.borrow().as_ref() {
                    fill();
                }
            });
        });
        dialog.present(Some(&self.window));
    }

    /// Lists the kept scans of the same folder to compare the one shown with, then what changed.
    fn compare(self: &Rc<Self>) {
        let Some((root, scanned)) = self.tree.borrow().as_ref().map(|tree| (tree.root.clone(), tree.scanned)) else { return };
        let earlier: Vec<scan::Kept> = scan::kept().into_iter().filter(|kept| kept.root == root && kept.scanned != scanned).collect();
        if earlier.is_empty() {
            alert::notice(&self.window, "沒有可比較的掃描", Some("同一個位置需要有另一次掃描。"));
            return;
        }
        let page = adw::PreferencesPage::new();
        let rows = adw::PreferencesGroup::builder().title("與哪一次掃描比較").build();
        page.add(&rows);
        let view = adw::ToolbarView::new();
        view.add_top_bar(&adw::HeaderBar::new());
        view.set_content(Some(&page));
        let dialog = adw::Dialog::builder().title("比較掃描").content_width(600).content_height(520).child(&view).build();
        for kept in earlier {
            let row = adw::ActionRow::builder()
                .title(date_time(kept.scanned))
                .subtitle(glib::format_size(kept.size))
                .activatable(true)
                .build();
            let (weak, path) = (Rc::downgrade(self), kept.path.clone());
            row.connect_activated(glib::clone!(
                #[weak]
                page,
                #[weak]
                rows,
                move |_| {
                    let Some(win) = weak.upgrade() else { return };
                    let other = match scan::load(&path) {
                        Ok(other) => other,
                        Err(why) => return alert::notice(&win.window, "無法開啟掃描檔", Some(&why)),
                    };
                    let tree = win.tree.borrow();
                    let Some(tree) = tree.as_ref() else { return };
                    // The earlier of the two first, whichever is shown.
                    let changes = if other.scanned < tree.scanned { scan::compare(&other, tree) } else { scan::compare(tree, &other) };
                    page.remove(&rows);
                    page.add(&changes_group(&changes));
                }
            ));
            rows.add(&row);
        }
        dialog.present(Some(&self.window));
    }
}

fn changes_group(changes: &[(PathBuf, scan::Change, u64, u64)]) -> adw::PreferencesGroup {
    let rows = adw::PreferencesGroup::new();
    if changes.is_empty() {
        rows.add(&adw::ActionRow::builder().title("沒有變化").css_classes(["dim-label"]).build());
    }
    for (path, change, then, now) in changes.iter().take(LISTED) {
        let (what, sign) = match change {
            scan::Change::Grew => ("變大", "+"),
            scan::Change::Shrank => ("變小", "−"),
            scan::Change::Appeared => ("新增", "+"),
            scan::Change::Disappeared => ("消失", "−"),
        };
        let row = adw::ActionRow::builder()
            .title(path.display().to_string())
            .subtitle(format!("{what}，{} → {}", glib::format_size(*then), glib::format_size(*now)))
            .build();
        row.add_suffix(&gtk::Label::builder().label(format!("{sign}{}", glib::format_size(now.abs_diff(*then)))).css_classes(["numeric"]).build());
        rows.add(&row);
    }
    rows
}

fn in_window(widget: &impl IsA<gtk::Widget>, window: &adw::ApplicationWindow, x: f64, y: f64) -> Option<(f64, f64)> {
    let point = widget.compute_point(window, &gtk::graphene::Point::new(x as f32, y as f32))?;
    Some((point.x().into(), point.y().into()))
}

fn heading(title: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(title)
        .xalign(0.0)
        .margin_start(18)
        .margin_top(12)
        .margin_bottom(4)
        .css_classes(["heading", "dim-label"])
        .build()
}

fn title_label(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .ellipsize(pango::EllipsizeMode::Middle)
        .css_classes(["title-4"])
        .build()
}

fn fact(title: &str, value: &str) -> gtk::Box {
    let line = gtk::Box::builder().spacing(12).build();
    line.append(&gtk::Label::builder().label(title).xalign(0.0).ellipsize(pango::EllipsizeMode::Middle).hexpand(true).css_classes(["dim-label"]).build());
    line.append(&gtk::Label::builder().label(value).xalign(1.0).ellipsize(pango::EllipsizeMode::Start).selectable(true).css_classes(["numeric"]).build());
    line
}

/// Whether the user can read and change the item, as its permissions say.
fn access(path: &Path) -> Option<String> {
    let info = gio::File::for_path(path)
        .query_info("access::can-read,access::can-write", gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS, gio::Cancellable::NONE)
        .ok()?;
    let (read, write) = (info.boolean("access::can-read"), info.boolean("access::can-write"));
    Some(match (read, write) {
        (true, true) => "讀取與寫入".into(),
        (true, false) => "唯讀".into(),
        (false, true) => "唯寫".into(),
        (false, false) => "無權限".into(),
    })
}

fn date(seconds: i64) -> String {
    glib::DateTime::from_unix_local(seconds)
        .and_then(|time| time.format("%Y/%m/%d"))
        .map(|text| text.to_string())
        .unwrap_or_default()
}

fn date_time(seconds: i64) -> String {
    glib::DateTime::from_unix_local(seconds)
        .and_then(|time| time.format("%Y/%m/%d %H:%M"))
        .map(|text| text.to_string())
        .unwrap_or_default()
}

fn show_in_files(path: &Path) {
    let uri = gio::File::for_path(path).uri().to_string();
    let shown = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE).and_then(|bus| {
        bus.call_sync(
            Some("org.freedesktop.FileManager1"),
            "/org/freedesktop/FileManager1",
            "org.freedesktop.FileManager1",
            "ShowItems",
            Some(&(vec![uri], "").to_variant()),
            None,
            gio::DBusCallFlags::NONE,
            5000,
            gio::Cancellable::NONE,
        )
    });
    if let Err(err) = shown {
        eprintln!("show {}: {err}", path.display());
    }
}

fn css() -> &'static str {
    "
.dk-columns { font-size: 12px; border-bottom: 1px solid var(--border-color); }
.dk-column { padding: 2px 6px; min-height: 20px; font-weight: 400; }
.dk-sorted { font-weight: 600; }
.dk-list { background: none; }
.dk-line { padding: 3px 8px; min-height: 26px; }
.dk-bar { min-height: 3px; }
.dk-bar block.filled { background: var(--accent-bg-color); }
.dk-here { font-weight: 600; }
"
}

fn main() -> glib::ExitCode {
    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_OPEN)
        .build();
    app.connect_startup(|_| {
        let provider = gtk::CssProvider::new();
        provider.load_from_string(css());
        if let Some(display) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
        }
    });
    let windows: Rc<RefCell<Vec<Rc<Window>>>> = Rc::default();
    let open = {
        let windows = windows.clone();
        move |app: &adw::Application| -> Rc<Window> {
            let win = Window::open(app);
            let go_to = gio::SimpleAction::new("go-to", Some(glib::VariantTy::UINT64));
            let weak = Rc::downgrade(&win);
            go_to.connect_activate(move |_, id| {
                if let (Some(win), Some(id)) = (weak.upgrade(), id.and_then(|id| id.get::<u64>())) {
                    win.go_to(id as usize);
                }
            });
            win.window.add_action(&go_to);
            let (all, closing) = (windows.clone(), Rc::downgrade(&win));
            win.window.connect_close_request(move |_| {
                if let Some(closing) = closing.upgrade() {
                    all.borrow_mut().retain(|kept| !Rc::ptr_eq(kept, &closing));
                }
                glib::Propagation::Proceed
            });
            windows.borrow_mut().push(win.clone());
            win
        }
    };
    let opener = open.clone();
    app.connect_activate(move |app| match app.active_window() {
        Some(window) => window.present(),
        None => {
            opener(app);
        }
    });
    // A folder opened with it is scanned; a scan file, shown.
    app.connect_open(move |app, files, _| {
        for file in files {
            let Some(path) = file.path() else { continue };
            let win = open(app);
            if path.extension().is_some_and(|extension| extension == scan::EXTENSION) {
                win.open_scan(&path);
            } else if path.is_dir() {
                win.scan(&path);
            }
        }
    });
    app.run()
}
