//! One tab of the window: a folder, the recent files, a tag's files or what a search found,
//! seen as icons, a list, columns or a gallery, with the path at the bottom and a preview
//! at the side.

use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    rc::{Rc, Weak},
    time::Duration,
};

use adw::prelude::*;
use gtk::{gdk, gio, glib};

use crate::{
    browse::{forget, home, info_file, is_dir, listing, modified_label, EDGE, GAP, READ_WAIT},
    ops,
    preview::Facts,
    search, tags, thumbs, Window,
};

/// What each listed file is read with.
pub const ATTRIBUTES: &str = "standard::name,standard::display-name,standard::type,standard::icon,\
standard::symbolic-icon,standard::is-hidden,standard::is-backup,standard::is-symlink,standard::content-type,\
standard::size,standard::target-uri,time::modified,time::created,time::access,thumbnail::path,xattr::xdg.tags,\
trash::orig-path,trash::deletion-date,unix::mode,owner::user,owner::group,access::can-write,access::can-rename,\
access::can-trash,access::can-delete";

/// How long a drag waits over a folder before the folder springs open.
const SPRING: Duration = Duration::from_millis(600);
/// The springing folder blinks twice, as the CSS animation `nf-spring` does, then opens.
const SPRING_BLINK: Duration = Duration::from_millis(240);
const COLUMN_WIDTH: i32 = 232;
pub const ICON_SIZES: (f64, f64) = (32.0, 128.0);

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum View {
    Icons,
    List,
    Columns,
    Gallery,
}

impl View {
    pub const ALL: [View; 4] = [View::Icons, View::List, View::Columns, View::Gallery];

    pub fn name(self) -> &'static str {
        match self {
            View::Icons => "icons",
            View::List => "list",
            View::Columns => "columns",
            View::Gallery => "gallery",
        }
    }

    pub fn from_name(name: &str) -> Option<View> {
        View::ALL.into_iter().find(|v| v.name() == name)
    }
}

/// What the list puts files under headers by.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Group {
    None,
    Kind,
    Modified,
    Size,
}

impl Group {
    pub const ALL: [Group; 4] = [Group::None, Group::Kind, Group::Modified, Group::Size];

    pub fn name(self) -> &'static str {
        match self {
            Group::None => "none",
            Group::Kind => "kind",
            Group::Modified => "modified",
            Group::Size => "size",
        }
    }

    pub fn from_name(name: &str) -> Option<Group> {
        Group::ALL.into_iter().find(|g| g.name() == name)
    }
}

/// The columns of the list that can be shown or not, by key, with their titles.
pub const OPTIONAL_COLUMNS: [(&str, &str); 4] = [
    ("modified", "修改日期"),
    ("created", "建立日期"),
    ("size", "大小"),
    ("kind", "種類"),
];

#[derive(Clone, Debug)]
pub enum Where {
    Recent,
    Folder(gio::File),
    Tag(String),
}

impl PartialEq for Where {
    fn eq(&self, other: &Where) -> bool {
        match (self, other) {
            (Where::Recent, Where::Recent) => true,
            (Where::Folder(a), Where::Folder(b)) => a.equal(b),
            (Where::Tag(a), Where::Tag(b)) => a == b,
            _ => false,
        }
    }
}

pub use crate::browse::trash;

/// A folder's name as the window says it: home as the user, a drive as its name.
pub fn folder_title(file: &gio::File) -> String {
    if file.equal(&gio::File::for_path(home())) {
        return glib::user_name().to_string_lossy().into_owned();
    }
    if file.equal(&trash()) {
        return "垃圾桶".into();
    }
    if file.uri() == "network:///" {
        return "網路".into();
    }
    if let Some(mount) = gio::VolumeMonitor::get()
        .mounts()
        .into_iter()
        .find(|m| m.root().equal(file))
    {
        return mount.name().into();
    }
    match file.parent() {
        Some(_) => file
            .query_info(
                "standard::display-name",
                gio::FileQueryInfoFlags::NONE,
                gio::Cancellable::NONE,
            )
            .map(|info| info.display_name().to_string())
            .ok()
            .or_else(|| file.basename().map(|b| b.to_string_lossy().into_owned()))
            .unwrap_or_default(),
        None => "/".into(),
    }
}

impl Where {
    pub fn title(&self) -> String {
        match self {
            Where::Recent => "最近使用".into(),
            Where::Tag(tag) => tag.clone(),
            Where::Folder(file) => folder_title(file),
        }
    }

    pub fn folder(&self) -> Option<gio::File> {
        match self {
            Where::Folder(file) => Some(file.clone()),
            _ => None,
        }
    }

    pub fn is_trash(&self) -> bool {
        matches!(self, Where::Folder(f) if f.uri_scheme().as_deref() == Some("trash"))
    }
}

/// A search going on in a tab: what, where, and narrowed to what.
struct Search {
    query: String,
    /// The whole computer, or the folder the tab is in.
    everywhere: bool,
    kind: Option<usize>,
    since: Option<usize>,
    _walk: search::Walk,
}

/// A column of the columns view: one folder.
struct Column {
    folder: gio::File,
    scroller: gtk::ScrolledWindow,
    list: gtk::DirectoryList,
    selection: gtk::MultiSelection,
}

pub struct Pane {
    win: Weak<Window>,
    pub root: gtk::Box,
    here: RefCell<Where>,
    back: RefCell<Vec<Where>>,
    forward: RefCell<Vec<Where>>,
    directory: RefCell<gtk::DirectoryList>,
    /// The directory's handlers, to take off when another folder is listed.
    watching: RefCell<Vec<glib::SignalHandlerId>>,
    results: gio::ListStore,
    shown: gtk::CustomFilter,
    filtered: gtk::FilterListModel,
    pub sorter: gtk::MultiSorter,
    /// The files in order, which a grouping orders by group first.
    sorted: gtk::SortListModel,
    flat: gtk::MultiSelection,
    tree_selection: gtk::MultiSelection,
    expandable: Rc<Cell<bool>>,
    pub column_view: gtk::ColumnView,
    pub sort_columns: Vec<(&'static str, gtk::ColumnViewColumn)>,
    where_column: gtk::ColumnViewColumn,
    orig_column: gtk::ColumnViewColumn,
    deleted_column: gtk::ColumnViewColumn,
    grid: gtk::GridView,
    columns_box: gtk::Box,
    columns_scroller: gtk::ScrolledWindow,
    columns: RefCell<Vec<Column>>,
    peek: RefCell<Option<gtk::Widget>>,
    /// Set while the window chooses in the columns itself, which must not navigate.
    choosing: Cell<bool>,
    stage: gtk::Picture,
    stage_icon: gtk::Image,
    stage_shown: RefCell<String>,
    strip: gtk::ListView,
    /// What was chosen last, to choose again when the listing replaces it in place.
    chosen_files: RefCell<Vec<gio::File>>,
    gallery_facts: Facts,
    views: gtk::Stack,
    /// The files of where the tab was, fading out over the ones of where it is.
    fade: gtk::Picture,
    fading: adw::TimedAnimation,
    status: gtk::Label,
    view: Cell<View>,
    search: RefCell<Option<Search>>,
    scope: gtk::Revealer,
    everywhere: gtk::ToggleButton,
    this_folder: gtk::ToggleButton,
    chips: gtk::Box,
    pub preview: gtk::Box,
    /// Whether the preview is turned on; the columns show theirs in a column instead.
    preview_wanted: Cell<bool>,
    facts: Facts,
    crumbs: gtk::Box,
    count: gtk::Label,
    zoom: gtk::Scale,
    icon_size: Cell<i32>,
    /// What is being made, as how far along: compressing shows on the new file's row.
    jobs: RefCell<Vec<(String, f64)>>,
    bars: RefCell<Vec<glib::WeakRef<gtk::ProgressBar>>>,
    /// The tag walk of a tag's tab, or nothing.
    tag_walk: RefCell<Option<search::Walk>>,
}

impl Pane {
    pub fn new(win: &Rc<Window>, at: Where) -> Rc<Pane> {
        let prefs = win.prefs.borrow();
        let directory = gtk::DirectoryList::new(Some(ATTRIBUTES), None::<&gio::File>);
        let results = gio::ListStore::new::<gio::FileInfo>();
        let shown = gtk::CustomFilter::new(|_| true);
        let filtered = gtk::FilterListModel::new(None::<gio::ListModel>, Some(shown.clone()));
        let sorter = gtk::MultiSorter::new();
        sorter.append(gtk::CustomSorter::new(|a, b| {
            let dir = |o: &glib::Object| o.downcast_ref::<gio::FileInfo>().is_some_and(is_dir);
            dir(b).cmp(&dir(a)).into()
        }));
        let sorted = gtk::SortListModel::new(Some(filtered.clone()), Some(sorter.clone()));
        let flat = gtk::MultiSelection::new(Some(sorted.clone()));
        let expandable = Rc::new(Cell::new(true));
        let tree = {
            let (shown, sorter, expandable) = (shown.clone(), sorter.clone(), expandable.clone());
            gtk::TreeListModel::new(sorted.clone(), false, false, move |item| {
                let info = item.downcast_ref::<gio::FileInfo>()?;
                if !expandable.get() || !is_dir(info) {
                    return None;
                }
                // Asked as each folder row is shown, so a folder is read before it unfolds.
                let list = listing(&info_file(info)?, ATTRIBUTES);
                let filtered = gtk::FilterListModel::new(Some(list), Some(shown.clone()));
                Some(gtk::SortListModel::new(Some(filtered), Some(sorter.clone())).upcast())
            })
        };
        let tree_selection = gtk::MultiSelection::new(Some(tree));

        let column_view = gtk::ColumnView::builder()
            .show_column_separators(false)
            .enable_rubberband(true)
            .css_classes(["nf-list"])
            .build();
        let grid = gtk::GridView::builder()
            .min_columns(2)
            .max_columns(24)
            .enable_rubberband(true)
            .css_classes(["nf-grid"])
            .build();
        let columns_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        // On the view's colour past the last column too, as the other views are.
        let columns_scroller = gtk::ScrolledWindow::builder()
            .vscrollbar_policy(gtk::PolicyType::Never)
            .css_classes(["view"])
            .child(&columns_box)
            .build();

        // The gallery: the picture large, a strip of all of them, and what it is.
        let stage = gtk::Picture::builder()
            .content_fit(gtk::ContentFit::Contain)
            .vexpand(true)
            .hexpand(true)
            .build();
        let stage_icon = gtk::Image::builder().pixel_size(192).vexpand(true).build();
        let stage_stack = gtk::Stack::new();
        stage_stack.add_named(&stage, Some("picture"));
        stage_stack.add_named(&stage_icon, Some("icon"));
        let strip = gtk::ListView::builder()
            .orientation(gtk::Orientation::Horizontal)
            .css_classes(["nf-strip"])
            .build();
        let gallery_facts = Facts::new(false, true);
        let gallery_main = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let stage_frame = gtk::Box::builder()
            .margin_top(EDGE)
            .margin_bottom(EDGE)
            .margin_start(EDGE)
            .margin_end(EDGE)
            .vexpand(true)
            .build();
        stage_frame.append(&stage_stack);
        gallery_main.append(&stage_frame);
        gallery_main.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        gallery_main.append(
            &gtk::ScrolledWindow::builder()
                .vscrollbar_policy(gtk::PolicyType::Never)
                .child(&strip)
                .build(),
        );
        let gallery = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        gallery_main.set_hexpand(true);
        gallery.append(&gallery_main);
        gallery.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        gallery_facts.root.set_width_request(272);
        gallery.append(&crate::preview::scrolled_facts(&gallery_facts));

        let status = gtk::Label::builder()
            .css_classes(["dim-label", "nf-status"])
            .build();
        let views = gtk::Stack::builder()
            .vexpand(true)
            .hexpand(true)
            .transition_type(gtk::StackTransitionType::Crossfade)
            .transition_duration(120)
            .css_classes(["view"])
            .build();
        views.add_named(&crate::browse::scrolled(&grid), Some("icons"));
        views.add_named(&crate::browse::scrolled(&column_view), Some("list"));
        views.add_named(&columns_scroller, Some("columns"));
        views.add_named(&gallery, Some("gallery"));
        views.add_named(&status, Some("status"));

        // Above the files while searching: where, and narrowed to what.
        let everywhere = gtk::ToggleButton::with_label("這台電腦");
        let this_folder = gtk::ToggleButton::builder().group(&everywhere).build();
        let scope_buttons = gtk::Box::builder().css_classes(["linked"]).build();
        scope_buttons.append(&everywhere);
        scope_buttons.append(&this_folder);
        let chips = gtk::Box::new(gtk::Orientation::Horizontal, GAP);
        let add_condition = gtk::MenuButton::builder()
            .icon_name("list-add-symbolic")
            .tooltip_text("加入條件")
            .css_classes(["flat"])
            .menu_model(&conditions_menu())
            .build();
        let scope_bar = gtk::Box::builder()
            .spacing(GAP)
            .margin_top(GAP)
            .margin_bottom(GAP)
            .margin_start(EDGE)
            .margin_end(EDGE)
            .build();
        scope_bar.append(&scope_buttons);
        scope_bar.append(&chips);
        scope_bar.append(&add_condition);
        let scope_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        scope_box.append(&scope_bar);
        scope_box.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        let scope = gtk::Revealer::builder()
            .child(&scope_box)
            .transition_type(gtk::RevealerTransitionType::SlideDown)
            .transition_duration(200)
            .build();

        // The preview at the side.
        let facts = Facts::new(true, false);
        let preview_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        preview_box.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        facts.root.set_width_request(272);
        preview_box.append(&crate::preview::scrolled_facts(&facts));
        // Shown or hidden outright: a revealer shut here keeps the pane's width, blank.
        preview_box.set_visible(prefs.preview);

        // The path at the bottom, how many there are, and how large icons are.
        let crumbs = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        let count = gtk::Label::builder()
            .css_classes(["dim-label", "numeric", "nf-small"])
            .build();
        let zoom = gtk::Scale::with_range(gtk::Orientation::Horizontal, ICON_SIZES.0, ICON_SIZES.1, 8.0);
        zoom.set_width_request(96);
        zoom.set_value(prefs.icon_size as f64);
        let path_bar = gtk::Box::builder()
            .spacing(GAP)
            .margin_start(EDGE)
            .margin_end(EDGE)
            .css_classes(["nf-pathbar"])
            .build();
        path_bar.append(
            &gtk::ScrolledWindow::builder()
                .child(&crumbs)
                .hexpand(true)
                .vscrollbar_policy(gtk::PolicyType::Never)
                .hscrollbar_policy(gtk::PolicyType::External)
                .build(),
        );
        path_bar.append(&count);
        path_bar.append(&zoom);

        let fade = gtk::Picture::builder()
            .can_target(false)
            .content_fit(gtk::ContentFit::Fill)
            .visible(false)
            .build();
        let fading = adw::TimedAnimation::new(
            &fade,
            1.0,
            0.0,
            150,
            adw::PropertyAnimationTarget::new(&fade, "opacity"),
        );
        fading.set_easing(adw::Easing::EaseOutCubic);
        fading.connect_done(|fading| {
            if let Some(fade) = fading.widget().downcast_ref::<gtk::Picture>() {
                fade.set_visible(false);
                fade.set_paintable(None::<&gdk::Paintable>);
            }
        });
        let fading_over = gtk::Overlay::builder().child(&views).build();
        fading_over.add_overlay(&fade);
        let body = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        body.append(&fading_over);
        body.append(&preview_box);
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.append(&scope);
        root.append(&body);
        root.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        root.append(&path_bar);

        let pane = Rc::new(Pane {
            win: Rc::downgrade(win),
            root,
            here: RefCell::new(Where::Recent),
            back: RefCell::new(Vec::new()),
            forward: RefCell::new(Vec::new()),
            directory: RefCell::new(directory),
            watching: RefCell::new(Vec::new()),
            results,
            shown,
            filtered,
            sorter,
            sorted: sorted.clone(),
            flat,
            tree_selection,
            expandable,
            column_view,
            sort_columns: Vec::new(),
            where_column: gtk::ColumnViewColumn::default(),
            orig_column: gtk::ColumnViewColumn::default(),
            deleted_column: gtk::ColumnViewColumn::default(),
            grid,
            columns_box,
            columns_scroller,
            columns: RefCell::new(Vec::new()),
            peek: RefCell::new(None),
            choosing: Cell::new(false),
            stage,
            stage_icon,
            stage_shown: RefCell::new(String::new()),
            strip: strip.clone(),
            chosen_files: RefCell::new(Vec::new()),
            gallery_facts,
            views,
            fade,
            fading,
            status,
            view: Cell::new(prefs.view),
            search: RefCell::new(None),
            scope,
            everywhere,
            this_folder,
            chips,
            preview: preview_box,
            preview_wanted: Cell::new(prefs.preview),
            facts,
            crumbs,
            count,
            zoom,
            icon_size: Cell::new(prefs.icon_size),
            jobs: RefCell::new(Vec::new()),
            bars: RefCell::new(Vec::new()),
            tag_walk: RefCell::new(None),
        });
        drop(prefs);
        // The columns are made with the pane, whose cells hold it weakly.
        let pane = {
            let mut pane = Rc::try_unwrap(pane).ok().expect("nothing holds the pane yet");
            let (sort_columns, where_column, orig_column, deleted_column) = Pane::make_columns(&pane);
            pane.sort_columns = sort_columns;
            pane.where_column = where_column;
            pane.orig_column = orig_column;
            pane.deleted_column = deleted_column;
            Rc::new(pane)
        };
        pane.finish_columns();
        pane.set_up_grid();
        pane.set_up_strip(&strip);
        pane.connect();
        pane.set_view(pane.view.get());
        pane.go(at, false);
        pane
    }

    fn window(&self) -> Option<Rc<Window>> {
        self.win.upgrade()
    }

    pub fn here(&self) -> Where {
        self.here.borrow().clone()
    }

    pub fn view(&self) -> View {
        self.view.get()
    }

    pub fn can_go_back(&self) -> bool {
        !self.back.borrow().is_empty()
    }

    pub fn can_go_forward(&self) -> bool {
        !self.forward.borrow().is_empty()
    }

    pub fn query(&self) -> String {
        self.search
            .borrow()
            .as_ref()
            .map(|s| s.query.clone())
            .unwrap_or_default()
    }

    pub fn is_searching(&self) -> bool {
        self.search.borrow().is_some()
    }

    // The list's columns.

    /// Makes the list's columns before the pane is shared: the cells hold it weakly, and the
    /// pane keeps the columns that come and go with what is shown.
    #[allow(clippy::type_complexity)]
    fn make_columns(
        pane: &Pane,
    ) -> (
        Vec<(&'static str, gtk::ColumnViewColumn)>,
        gtk::ColumnViewColumn,
        gtk::ColumnViewColumn,
        gtk::ColumnViewColumn,
    ) {
        let _ = pane;
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
        let column = |title: &str, width: i32, sorter: Option<gtk::CustomSorter>| {
            let column = gtk::ColumnViewColumn::builder().title(title).build();
            if width > 0 {
                column.set_fixed_width(width);
            } else {
                column.set_expand(true);
                column.set_resizable(true);
            }
            column.set_sorter(sorter.as_ref());
            column
        };
        let name = column(
            "名稱",
            0,
            Some(by(|a, b| natural(&a.display_name(), &b.display_name()))),
        );
        let modified = column(
            "修改日期",
            140,
            Some(by(|a, b| modified_time(a).cmp(&modified_time(b)))),
        );
        let created_column = column(
            "建立日期",
            140,
            Some(by(|a, b| {
                let when = |info| created(info).map_or(0, |t| t.to_unix());
                when(a).cmp(&when(b))
            })),
        );
        let size = column("大小", 84, Some(by(|a, b| a.size().cmp(&b.size()))));
        let kind_column = column("種類", 160, Some(by(|a, b| kind_label(a).cmp(&kind_label(b)))));
        let place = column("位置", 160, None);
        let orig = column("原本位置", 160, None);
        let deleted = column("刪除日期", 140, None);
        (
            vec![
                ("name", name),
                ("modified", modified),
                ("size", size),
                ("kind", kind_column),
                ("created", created_column),
            ],
            place,
            orig,
            deleted,
        )
    }

    fn finish_columns(self: &Rc<Self>) {
        let [name, modified, size, kind, made] = [0, 1, 2, 3, 4].map(|i| self.sort_columns[i].1.clone());
        made.set_factory(Some(&self.text_factory(|info| date_label(created(info)))));
        name.set_factory(Some(&self.name_factory()));
        modified.set_factory(Some(&self.text_factory(|info| {
            let when = info.modification_date_time().and_then(|t| t.to_local().ok());
            date_label(when)
        })));
        size.set_factory(Some(&self.text_factory(|info| {
            if is_dir(info) {
                "—".into()
            } else {
                glib::format_size(info.size() as u64).to_string()
            }
        })));
        kind.set_factory(Some(&self.text_factory(kind_label)));
        self.where_column.set_factory(Some(&self.text_factory(|info| {
            info_file(info)
                .and_then(|f| f.parent())
                .map(|folder| folder_title(&folder))
                .unwrap_or_default()
        })));
        self.orig_column.set_factory(Some(&self.text_factory(|info| {
            info.attribute_byte_string("trash::orig-path")
                .and_then(|path| gio::File::for_path(path.as_str()).parent())
                .map(|folder| folder_title(&folder))
                .unwrap_or_default()
        })));
        self.deleted_column.set_factory(Some(&self.text_factory(|info| {
            let when = info
                .attribute_string("trash::deletion-date")
                .and_then(|text| glib::DateTime::from_iso8601(&text, Some(&glib::TimeZone::local())).ok());
            date_label(when)
        })));
        self.column_view.append_column(&name);
        self.column_view.append_column(&self.where_column);
        self.column_view.append_column(&self.orig_column);
        self.column_view.append_column(&modified);
        self.column_view.append_column(&made);
        self.column_view.append_column(&self.deleted_column);
        self.column_view.append_column(&size);
        self.column_view.append_column(&kind);
        // A right click on the header: which columns to show.
        let columns_menu = ops::columns_menu();
        for column in self.sort_columns.iter().map(|(_, c)| c).chain([
            &self.where_column,
            &self.orig_column,
            &self.deleted_column,
        ]) {
            column.set_header_menu(Some(&columns_menu));
        }
        if let Some(column_sorter) = self.column_view.sorter() {
            self.sorter.append(column_sorter);
        }
        self.column_view
            .sort_by_column(Some(&name), gtk::SortType::Ascending);
    }

    /// The name column: the disclosure triangle, the icon, the name, its tags, and how far
    /// along it is while it is being made.
    fn name_factory(self: &Rc<Self>) -> gtk::SignalListItemFactory {
        let factory = gtk::SignalListItemFactory::new();
        let weak = Rc::downgrade(self);
        factory.connect_setup(move |_, item| {
            let (Some(item), Some(pane)) = (item.downcast_ref::<gtk::ListItem>(), weak.upgrade()) else {
                return;
            };
            // As tall for a CJK name as for a Latin one.
            let line = gtk::Box::builder().spacing(GAP).height_request(20).build();
            line.append(&pane.check_for(item));
            line.append(&gtk::Image::builder().pixel_size(16).build());
            // Not expanding: the tags follow the name, as Finder's do.
            line.append(
                &gtk::Label::builder()
                    .xalign(0.0)
                    .ellipsize(gtk::pango::EllipsizeMode::Middle)
                    .build(),
            );
            line.append(&tags::dots(&[]));
            let bar = gtk::ProgressBar::builder()
                .width_request(64)
                .valign(gtk::Align::Center)
                .visible(false)
                .build();
            line.append(&bar);
            pane.bars.borrow_mut().push(bar.downgrade());
            let expander = gtk::TreeExpander::builder().child(&line).build();
            item.set_child(Some(&expander));
            pane.set_up_item(&expander.clone().upcast(), item);
        });
        let weak = Rc::downgrade(self);
        factory.connect_bind(move |_, item| {
            let (Some(item), Some(pane)) = (item.downcast_ref::<gtk::ListItem>(), weak.upgrade()) else {
                return;
            };
            let Some(expander) = item.child().and_downcast::<gtk::TreeExpander>() else {
                return;
            };
            // Under headers the list is no tree.
            expander.set_list_row(item.item().and_downcast::<gtk::TreeListRow>().as_ref());
            let Some(info) = list_item_info(item) else {
                return;
            };
            let line = expander.child().expect("set up above");
            let check = line.first_child().expect("set up above");
            check.set_visible(pane.checks_shown());
            let image = check
                .next_sibling()
                .and_downcast::<gtk::Image>()
                .expect("set up above");
            let label = image
                .next_sibling()
                .and_downcast::<gtk::Label>()
                .expect("set up above");
            let dots = label
                .next_sibling()
                .and_downcast::<gtk::Box>()
                .expect("set up above");
            let bar = line
                .last_child()
                .and_downcast::<gtk::ProgressBar>()
                .expect("set up above");
            image.set_from_gicon(&thumbs::icon(&info));
            label.set_label(&info.display_name());
            tags::fill_dots(&dots, &tags::of(&info));
            let uri = info_file(&info).map(|f| f.uri().to_string()).unwrap_or_default();
            bar.set_widget_name(&uri);
            pane.show_job(&bar);
        });
        factory
    }

    fn text_factory(
        self: &Rc<Self>,
        text: impl Fn(&gio::FileInfo) -> String + 'static,
    ) -> gtk::SignalListItemFactory {
        let factory = gtk::SignalListItemFactory::new();
        let weak = Rc::downgrade(self);
        factory.connect_setup(move |_, item| {
            let (Some(item), Some(pane)) = (item.downcast_ref::<gtk::ListItem>(), weak.upgrade()) else {
                return;
            };
            let label = gtk::Label::builder()
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .css_classes(["dim-label", "numeric"])
                .build();
            item.set_child(Some(&label));
            pane.set_up_item(&label.upcast(), item);
        });
        factory.connect_bind(move |_, item| {
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let (Some(label), Some(info)) = (item.child().and_downcast::<gtk::Label>(), list_item_info(item))
            else {
                return;
            };
            label.set_label(&text(&info));
        });
        factory
    }

    fn set_up_grid(self: &Rc<Self>) {
        let factory = gtk::SignalListItemFactory::new();
        let weak = Rc::downgrade(self);
        factory.connect_setup(move |_, item| {
            let (Some(item), Some(pane)) = (item.downcast_ref::<gtk::ListItem>(), weak.upgrade()) else {
                return;
            };
            let tile = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(GAP / 2)
                .css_classes(["nf-tile"])
                .build();
            let picture = gtk::Picture::builder()
                .content_fit(gtk::ContentFit::Contain)
                .halign(gtk::Align::Center)
                .css_classes(["nf-thumb"])
                .overflow(gtk::Overflow::Hidden)
                .build();
            let image = gtk::Image::new();
            let label = gtk::Label::builder()
                .wrap(true)
                .wrap_mode(gtk::pango::WrapMode::WordChar)
                .lines(2)
                .ellipsize(gtk::pango::EllipsizeMode::Middle)
                .justify(gtk::Justification::Center)
                .css_classes(["nf-small"])
                .build();
            tile.append(&picture);
            tile.append(&image);
            tile.append(&label);
            let dots = tags::dots(&[]);
            dots.set_halign(gtk::Align::Center);
            tile.append(&dots);
            let overlay = gtk::Overlay::builder().child(&tile).build();
            let check = pane.check_for(item);
            check.set_halign(gtk::Align::Start);
            check.set_valign(gtk::Align::Start);
            overlay.add_overlay(&check);
            item.set_child(Some(&overlay));
            pane.set_up_item(&overlay.upcast(), item);
        });
        let weak = Rc::downgrade(self);
        factory.connect_bind(move |_, item| {
            let (Some(item), Some(pane)) = (item.downcast_ref::<gtk::ListItem>(), weak.upgrade()) else {
                return;
            };
            let (Some(overlay), Some(info)) =
                (item.child().and_downcast::<gtk::Overlay>(), list_item_info(item))
            else {
                return;
            };
            let tile = overlay.child().expect("set up above");
            if let Some(check) = tile.next_sibling() {
                check.set_visible(pane.checks_shown());
            }
            let picture = tile
                .first_child()
                .and_downcast::<gtk::Picture>()
                .expect("set up above");
            let image = picture
                .next_sibling()
                .and_downcast::<gtk::Image>()
                .expect("set up above");
            let label = image
                .next_sibling()
                .and_downcast::<gtk::Label>()
                .expect("set up above");
            let dots = tile
                .last_child()
                .and_downcast::<gtk::Box>()
                .expect("set up above");
            let size = pane.icon_size.get();
            tile.set_width_request(size + 2 * EDGE + GAP);
            picture.set_size_request(size, size);
            image.set_pixel_size(size);
            label.set_max_width_chars(((size + 2 * EDGE) / 7).max(10));
            thumbs::show(&picture, &image, &info, info_file(&info).as_ref());
            label.set_label(&info.display_name());
            tags::fill_dots(&dots, &tags::of(&info));
        });
        self.grid.set_factory(Some(&factory));
    }

    fn set_up_strip(self: &Rc<Self>, strip: &gtk::ListView) {
        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup(|_, item| {
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let tile = gtk::Box::builder().css_classes(["nf-strip-tile"]).build();
            let picture = gtk::Picture::builder()
                .content_fit(gtk::ContentFit::Cover)
                .width_request(56)
                .height_request(56)
                .css_classes(["nf-thumb"])
                .overflow(gtk::Overflow::Hidden)
                .build();
            let image = gtk::Image::builder()
                .pixel_size(48)
                .width_request(56)
                .height_request(56)
                .build();
            tile.append(&picture);
            tile.append(&image);
            item.set_child(Some(&tile));
        });
        factory.connect_bind(|_, item| {
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let (Some(tile), Some(info)) = (item.child(), list_item_info(item)) else {
                return;
            };
            let picture = tile
                .first_child()
                .and_downcast::<gtk::Picture>()
                .expect("set up above");
            let image = tile
                .last_child()
                .and_downcast::<gtk::Image>()
                .expect("set up above");
            thumbs::show(&picture, &image, &info, info_file(&info).as_ref());
        });
        strip.set_factory(Some(&factory));
        strip.connect_activate(on!(self, |p, _s, position| p.activate_flat(position)));
    }

    /// What every item's widget does, whatever the view: the menu on a right click, being
    /// dragged, and taking what is dropped on a folder.
    fn set_up_item(self: &Rc<Self>, widget: &gtk::Widget, item: &gtk::ListItem) {
        let item = item.downgrade();
        ITEM_WIDGETS.with_borrow_mut(|all| {
            all.retain(|(widget, _)| widget.upgrade().is_some());
            all.push((widget.downgrade(), item.clone()));
        });
        let click = gtk::GestureClick::builder().button(gdk::BUTTON_SECONDARY).build();
        {
            let (weak, item, widget_weak) = (Rc::downgrade(self), item.clone(), widget.downgrade());
            click.connect_pressed(move |click, _, x, y| {
                let (Some(pane), Some(item), Some(widget)) =
                    (weak.upgrade(), item.upgrade(), widget_weak.upgrade())
                else {
                    return;
                };
                click.set_state(gtk::EventSequenceState::Claimed);
                if !item.is_selected() {
                    pane.select_only(&widget, item.position());
                }
                pane.menu_at(&widget, x, y);
            });
        }
        widget.add_controller(click);

        // A middle click opens a folder in a new tab.
        let middle = gtk::GestureClick::builder().button(gdk::BUTTON_MIDDLE).build();
        {
            let (weak, item) = (Rc::downgrade(self), item.clone());
            middle.connect_released(move |click, _, _, _| {
                let (Some(pane), Some(info)) =
                    (weak.upgrade(), item.upgrade().and_then(|i| list_item_info(&i)))
                else {
                    return;
                };
                if let (true, Some(win), Some(folder)) = (is_dir(&info), pane.window(), ops::target(&info)) {
                    click.set_state(gtk::EventSequenceState::Claimed);
                    win.new_tab(Where::Folder(folder));
                }
            });
        }
        widget.add_controller(middle);

        let drag = gtk::DragSource::builder()
            .actions(gdk::DragAction::COPY | gdk::DragAction::MOVE)
            .build();
        {
            // How many are dragged, and where the pointer took the item, for the icon.
            let grabbed = Rc::new(Cell::new((0, 0.0, 0.0)));
            let (weak, item, widget_weak) = (Rc::downgrade(self), item.clone(), widget.downgrade());
            let taken = grabbed.clone();
            drag.connect_prepare(move |source, x, y| {
                let (pane, item, widget) = (weak.upgrade()?, item.upgrade()?, widget_weak.upgrade()?);
                if !item.is_selected() {
                    pane.select_only(&widget, item.position());
                }
                let files: Vec<gio::File> = pane.selected().iter().filter_map(info_file).collect();
                if files.is_empty() {
                    return None;
                }
                taken.set((files.len(), x, y));
                if files.len() == 1 {
                    source.set_icon(
                        Some(&gtk::WidgetPaintable::new(Some(&widget))),
                        x as i32,
                        y as i32,
                    );
                }
                Some(gdk::ContentProvider::for_value(
                    &gdk::FileList::from_array(&files).to_value(),
                ))
            });
            // Several: the item under the pointer with how many in a badge on its corner.
            let widget_weak = widget.downgrade();
            drag.connect_drag_begin(move |_, drag| {
                let (count, x, y) = grabbed.get();
                let Some(widget) = widget_weak.upgrade().filter(|_| count > 1) else {
                    return;
                };
                let picture = gtk::Picture::builder()
                    .paintable(&gtk::WidgetPaintable::new(Some(&widget)))
                    .can_shrink(false)
                    .build();
                let overlay = gtk::Overlay::builder().child(&picture).build();
                overlay.add_overlay(
                    &gtk::Label::builder()
                        .label(count.to_string())
                        .halign(gtk::Align::End)
                        .valign(gtk::Align::Start)
                        .css_classes(["nf-drag-count", "numeric"])
                        .build(),
                );
                gtk::DragIcon::for_drag(drag)
                    .downcast::<gtk::DragIcon>()
                    .expect("GTK makes a drag's icon a GtkDragIcon")
                    .set_child(Some(&overlay));
                drag.set_hotspot(x as i32, y as i32);
            });
        }
        widget.add_controller(drag);

        let drop = gtk::DropTarget::new(
            gdk::FileList::static_type(),
            gdk::DragAction::COPY | gdk::DragAction::MOVE,
        );
        let folder_of = {
            let item = item.clone();
            move || {
                let item = item.upgrade()?;
                let info = list_item_info(&item)?;
                is_dir(&info).then(|| info_file(&info)).flatten()
            }
        };
        let folder_of = Rc::new(folder_of);
        {
            let folder_of = folder_of.clone();
            drop.connect_accept(move |_, _| folder_of().is_some_and(|f| f.path().is_some()));
        }
        {
            let folder_of = folder_of.clone();
            drop.connect_drop(move |target, value, _, _| {
                let (Ok(list), Some(folder)) = (value.get::<gdk::FileList>(), folder_of()) else {
                    return false;
                };
                ops::drop_into(&list.files(), &folder, target.current_event_state());
                true
            });
        }
        self.spring(&drop, widget, move || folder_of().map(Where::Folder));
        widget.add_controller(drop);
    }

    /// Opens a folder a drag waits over in this tab.
    pub fn spring(
        self: &Rc<Self>,
        drop: &gtk::DropTarget,
        widget: &gtk::Widget,
        target: impl Fn() -> Option<Where> + 'static,
    ) {
        let weak = Rc::downgrade(self);
        spring(drop, widget, target, move |to| {
            if let Some(pane) = weak.upgrade() {
                pane.go(to, true);
            }
        });
    }

    fn connect(self: &Rc<Self>) {
        {
            let weak = Rc::downgrade(self);
            self.shown.set_filter_func(move |item| {
                let (Some(info), Some(pane)) = (item.downcast_ref::<gio::FileInfo>(), weak.upgrade()) else {
                    return false;
                };
                pane.shows(info)
            });
        }
        self.filtered
            .connect_items_changed(on!(self, |p, _m, _a, _b, _c| p.update_status()));
        self.views.connect_visible_child_name_notify(on!(self, |p, _v| {
            p.feed_views(false);
            // Once the view going is gone, or has started to fade.
            let weak = Rc::downgrade(&p);
            glib::idle_add_local_once(move || {
                if let Some(pane) = weak.upgrade() {
                    pane.feed_views(true);
                }
            });
        }));
        self.views
            .connect_transition_running_notify(on!(self, |p, _v| p.feed_views(true)));
        self.regroup();
        self.feed_views(true);
        self.flat
            .connect_selection_changed(on!(self, |p, _m, _a, _b| p.selection_changed()));
        self.tree_selection
            .connect_selection_changed(on!(self, |p, _m, _a, _b| p.selection_changed()));
        for model in [&self.flat, &self.tree_selection] {
            model.connect_items_changed(on!(self, |p, m, at, removed, added| {
                p.keep_chosen(m.upcast_ref(), at, removed, added)
            }));
        }
        self.column_view
            .connect_activate(on!(self, |p, _v, position| p.activate_tree(position)));
        self.grid
            .connect_activate(on!(self, |p, _v, position| p.activate_flat(position)));
        self.zoom
            .connect_value_changed(on!(self, |p, scale| p.set_icon_size(scale.value() as i32)));
        // Ctrl and the wheel make the icons larger or smaller.
        let wheel = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        let zoom = self.zoom.downgrade();
        wheel.connect_scroll(move |wheel, _, dy| {
            let Some(zoom) = zoom.upgrade() else {
                return glib::Propagation::Proceed;
            };
            if !wheel
                .current_event_state()
                .contains(gdk::ModifierType::CONTROL_MASK)
            {
                return glib::Propagation::Proceed;
            }
            zoom.set_value(zoom.value() - dy * zoom.adjustment().step_increment());
            glib::Propagation::Stop
        });
        self.grid.add_controller(wheel);
        self.everywhere
            .connect_toggled(on!(self, |p, b| if b.is_active() {
                p.restart_search(|s| s.everywhere = true)
            }));
        self.this_folder
            .connect_toggled(on!(self, |p, b| if b.is_active() {
                p.restart_search(|s| s.everywhere = false)
            }));

        // The search's conditions, as the menu at the scope bar sets them.
        let actions = gio::SimpleActionGroup::new();
        let kind = gio::SimpleAction::new("kind", Some(glib::VariantTy::INT32));
        kind.connect_activate(on!(self, |p, _a, value| {
            let kind = value.and_then(|v| v.get::<i32>()).map(|k| k as usize);
            p.restart_search(|s| s.kind = kind);
        }));
        let since = gio::SimpleAction::new("since", Some(glib::VariantTy::INT32));
        since.connect_activate(on!(self, |p, _a, value| {
            let since = value.and_then(|v| v.get::<i32>()).map(|k| k as usize);
            p.restart_search(|s| s.since = since);
        }));
        actions.add_action(&kind);
        actions.add_action(&since);
        self.root.insert_action_group("pane", Some(&actions));

        // A right click on nothing: the folder's own menu.
        for view in [
            self.grid.clone().upcast::<gtk::Widget>(),
            self.column_view.clone().upcast(),
            self.status.clone().upcast(),
        ] {
            let click = gtk::GestureClick::builder().button(gdk::BUTTON_SECONDARY).build();
            let (weak, widget) = (Rc::downgrade(self), view.downgrade());
            click.connect_pressed(move |_, _, x, y| {
                let (Some(pane), Some(widget)) = (weak.upgrade(), widget.upgrade()) else {
                    return;
                };
                // On an item, the item's own menu opens; this one is for the space around them.
                let on_item = widget.pick(x, y, gtk::PickFlags::DEFAULT).is_some_and(|picked| {
                    std::iter::successors(Some(picked), |w| w.parent())
                        .take_while(|w| w != &widget)
                        .any(|w| matches!(w.css_name().as_str(), "child" | "row"))
                });
                if on_item {
                    return;
                }
                pane.unselect_all();
                pane.menu_at(&widget, x, y);
            });
            view.add_controller(click);
            // Dropped on no folder in particular: into this one.
            let drop = gtk::DropTarget::new(
                gdk::FileList::static_type(),
                gdk::DragAction::COPY | gdk::DragAction::MOVE,
            );
            let weak = Rc::downgrade(self);
            drop.connect_drop(move |target, value, _, _| {
                let (Ok(list), Some(pane)) = (value.get::<gdk::FileList>(), weak.upgrade()) else {
                    return false;
                };
                let Some(folder) = pane.here().folder().filter(|f| f.path().is_some()) else {
                    return false;
                };
                ops::drop_into(&list.files(), &folder, target.current_event_state());
                true
            });
            view.add_controller(drop);
        }
    }

    // Where the pane is.

    pub fn go(self: &Rc<Self>, to: Where, remember: bool) {
        if remember && *self.here.borrow() == to && !self.is_searching() {
            return;
        }
        if remember {
            let from = self.here.replace(to.clone());
            self.back.borrow_mut().push(from);
            self.forward.borrow_mut().clear();
        } else {
            self.here.replace(to.clone());
        }
        self.search.replace(None);
        self.tag_walk.replace(None);
        self.scope.set_reveal_child(false);
        self.expandable.set(true);
        match &to {
            Where::Folder(file) => self.set_directory(listing(file, ATTRIBUTES)),
            Where::Recent => {
                self.crossfade();
                self.fill_recent();
                self.expandable.set(false);
                self.filtered.set_model(Some(&self.results));
            }
            Where::Tag(tag) => {
                self.crossfade();
                self.results.remove_all();
                self.expandable.set(false);
                self.filtered.set_model(Some(&self.results));
                let weak = Rc::downgrade(self);
                let walk = search::walk(
                    vec![home()],
                    search::Find::Tag(tag.clone()),
                    false,
                    move |batch, _| {
                        if let Some(pane) = weak.upgrade() {
                            pane.add_results(batch);
                        }
                    },
                );
                self.tag_walk.replace(Some(walk));
            }
        }
        self.show_columns_for_mode();
        self.fill_crumbs();
        if self.view.get() == View::Columns {
            self.sync_columns();
        }
        self.update_status();
        self.selection_changed();
        if let Some(win) = self.window() {
            win.pane_changed(self);
        }
    }

    /// Lists a folder's files here. Until the first of them are read, the folder before
    /// stays in view, not a frame of nothing, unless the reading takes a while.
    fn set_directory(self: &Rc<Self>, list: gtk::DirectoryList) {
        let old = self.directory.replace(list.clone());
        for id in self.watching.take() {
            old.disconnect(id);
        }
        self.watching.replace(vec![
            list.connect_items_changed(on!(self, |p, _l, _a, _r, _d| p.show_directory())),
            list.connect_loading_notify(on!(self, |p, _l| {
                p.show_directory();
                p.update_status()
            })),
            list.connect_error_notify(on!(self, |p, _l| {
                p.show_directory();
                p.update_status()
            })),
        ]);
        if list.n_items() > 0 || !list.is_loading() {
            self.show_directory();
            return;
        }
        // What was chosen there is not chosen here; the columns keep the choices that led here.
        if self.view.get() != View::Columns {
            self.unselect_all();
        }
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(READ_WAIT, move || {
            if let Some(pane) = weak.upgrade() {
                pane.show_directory();
                pane.update_status();
            }
        });
    }

    /// Shows the directory's files, if the tab is still in that folder and not searching.
    fn show_directory(&self) {
        let list = self.directory.borrow().clone();
        let here =
            matches!(&*self.here.borrow(), Where::Folder(f) if list.file().is_some_and(|l| l.equal(f)));
        if here && !self.is_searching() && self.filtered.model().as_ref() != Some(list.upcast_ref()) {
            self.crossfade();
            // Emptied first, in the same moment, so the views start at the top as for any
            // folder rather than keep where they were in the one before.
            self.filtered.set_model(None::<&gio::ListModel>);
            self.filtered.set_model(Some(&list));
        }
    }

    /// What the tab shows fades into what it shows next, as the sidebar's choice moves to
    /// where it goes: a picture of it now, over the files that replace them, fading out.
    fn crossfade(&self) {
        // The columns move along to the folder themselves.
        if self.view.get() == View::Columns || !self.views.is_mapped() {
            return;
        }
        let now = gtk::WidgetPaintable::new(Some(&self.views)).current_image();
        self.fade.set_paintable(Some(&now));
        self.fade.set_visible(true);
        self.fading.reset();
        self.fading.play();
    }

    /// Back (-1) or forward (1) in where the tab has been.
    pub fn step(self: &Rc<Self>, direction: i32) {
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

    pub fn go_up(self: &Rc<Self>) {
        if let Some(parent) = self.here().folder().and_then(|f| f.parent()) {
            self.go(Where::Folder(parent), true);
        }
    }

    pub fn reload(self: &Rc<Self>) {
        let here = self.here();
        if let Some(folder) = here.folder() {
            forget(&folder);
        }
        self.go(here, false);
    }

    fn fill_recent(&self) {
        self.results.remove_all();
        let mut items = gtk::RecentManager::default().items();
        items.sort_by_key(|item| std::cmp::Reverse(item.modified().to_unix()));
        for item in items
            .into_iter()
            .filter(|item| item.is_local() && item.exists())
            .take(100)
        {
            if let Some(info) = query(&gio::File::for_uri(&item.uri())) {
                self.results.append(&info);
            }
        }
    }

    fn add_results(&self, batch: Vec<std::path::PathBuf>) {
        let infos: Vec<gio::FileInfo> = batch
            .iter()
            .filter_map(|path| query(&gio::File::for_path(path)))
            .collect();
        self.results.extend_from_slice(&infos);
    }

    /// Which columns the list has: where each found file is, or where each item in the
    /// trash came from and when it went there.
    pub fn show_columns_for_mode(&self) {
        let trash = self.here.borrow().is_trash();
        let found = self.is_searching() || matches!(*self.here.borrow(), Where::Recent | Where::Tag(_));
        let changed = Cell::new(false);
        let show = |column: &gtk::ColumnViewColumn, shown: bool| {
            changed.set(changed.get() || column.is_visible() != shown);
            column.set_visible(shown);
        };
        show(&self.where_column, found);
        show(&self.orig_column, trash);
        show(&self.deleted_column, trash);
        let shown = self
            .window()
            .map(|w| w.prefs.borrow().columns.clone())
            .unwrap_or_default();
        for (key, column) in &self.sort_columns {
            let wanted = *key == "name" || shown.iter().any(|k| k == key);
            // The trash has its own dates and no kinds.
            let here = !trash || matches!(*key, "name" | "size");
            show(column, wanted && here);
        }
        // GTK gives a section's header a cell of each column shown after the header was made.
        if changed.get() && self.sorted.section_sorter().is_some() {
            self.rebind();
        }
    }

    // Searching.

    pub fn search(self: &Rc<Self>, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            if self.is_searching() {
                self.search.replace(None);
                let here = self.here();
                self.go(here, false);
            }
            return;
        }
        let (everywhere, kind, since) = match &*self.search.borrow() {
            Some(s) => (s.everywhere, s.kind, s.since),
            None => (
                self.here().folder().is_none() || self.here.borrow().is_trash(),
                None,
                None,
            ),
        };
        self.start_search(text.to_owned(), everywhere, kind, since);
    }

    fn restart_search(self: &Rc<Self>, change: impl FnOnce(&mut Search)) {
        let Some(mut current) = self.search.take() else {
            return;
        };
        change(&mut current);
        let Search {
            query,
            everywhere,
            kind,
            since,
            ..
        } = current;
        self.start_search(query, everywhere, kind, since);
    }

    fn start_search(
        self: &Rc<Self>,
        query: String,
        everywhere: bool,
        kind: Option<usize>,
        since: Option<usize>,
    ) {
        let here = self.here().folder().filter(|f| f.path().is_some());
        let everywhere = everywhere || here.is_none();
        let roots = if everywhere {
            let mut roots = vec![home()];
            for mount in gio::VolumeMonitor::get().mounts() {
                if let Some(path) = mount.root().path().filter(|p| !p.starts_with(home())) {
                    roots.push(path);
                }
            }
            roots
        } else {
            here.iter().filter_map(|f| f.path()).collect()
        };
        self.results.remove_all();
        let weak = Rc::downgrade(self);
        let hidden = self.window().is_some_and(|w| w.prefs.borrow().hidden);
        let walk = search::walk(
            roots,
            search::Find::Name(query.clone()),
            hidden,
            move |batch, _| {
                if let Some(pane) = weak.upgrade() {
                    pane.add_results(batch);
                }
            },
        );
        self.search.replace(Some(Search {
            query,
            everywhere,
            kind,
            since,
            _walk: walk,
        }));
        self.expandable.set(false);
        self.filtered.set_model(Some(&self.results));
        self.shown.changed(gtk::FilterChange::Different);
        // The scope bar: the folder's name on its button, and the conditions as chips.
        self.this_folder.set_visible(here.is_some());
        if let Some(here) = &here {
            self.this_folder.set_label(&format!("「{}」", folder_title(here)));
        }
        if everywhere {
            self.everywhere.set_active(true);
        } else {
            self.this_folder.set_active(true);
        }
        self.fill_chips(kind, since);
        self.scope.set_reveal_child(true);
        if matches!(self.view.get(), View::Columns | View::Gallery) {
            self.views.set_visible_child_name("list");
        }
        self.show_columns_for_mode();
        self.update_status();
    }

    fn fill_chips(self: &Rc<Self>, kind: Option<usize>, since: Option<usize>) {
        while let Some(chip) = self.chips.first_child() {
            self.chips.remove(&chip);
        }
        let add = |label: String, clear: &'static str| {
            let chip = gtk::Box::builder()
                .spacing(GAP / 2)
                .css_classes(["nf-chip"])
                .build();
            chip.append(&gtk::Label::new(Some(&label)));
            let remove = gtk::Button::builder()
                .icon_name("window-close-symbolic")
                .tooltip_text("移除")
                .css_classes(["flat", "circular", "nf-chip-remove"])
                .action_name(clear)
                .build();
            chip.append(&remove);
            self.chips.append(&chip);
        };
        if let Some((name, _)) = kind.and_then(|k| search::KINDS.get(k)) {
            add(format!("種類：{name}"), "pane.kind");
        }
        if let Some((name, _)) = since.and_then(|s| search::SINCE.get(s)) {
            add(format!("修改日期：{name}"), "pane.since");
        }
    }

    /// Whether a file is listed: hidden ones only when asked for, and what a search is
    /// narrowed to.
    fn shows(&self, info: &gio::FileInfo) -> bool {
        let hidden = self.window().is_some_and(|w| w.prefs.borrow().hidden);
        if !hidden && (info.is_hidden() || info.is_backup()) {
            return false;
        }
        let search = self.search.borrow();
        let Some(search) = search.as_ref() else {
            return true;
        };
        if let Some(kind) = search.kind {
            let content_type = info.content_type().map(|t| t.to_string()).unwrap_or_default();
            if !search::is_kind(&content_type, kind) {
                return false;
            }
        }
        if let Some(since) = search.since {
            let (Some(when), Ok(now)) = (
                info.modification_date_time().and_then(|t| t.to_local().ok()),
                glib::DateTime::now_local(),
            ) else {
                return false;
            };
            if !search::is_since(&when, &now, since) {
                return false;
            }
        }
        true
    }

    pub fn refilter(&self) {
        self.shown.changed(gtk::FilterChange::Different);
    }

    // Views.

    pub fn set_view(self: &Rc<Self>, view: View) {
        let chosen: Vec<gio::File> = self.selected().iter().filter_map(info_file).collect();
        self.view.set(view);
        self.zoom.set_visible(view == View::Icons);
        if view == View::Columns {
            self.sync_columns();
        }
        self.show_preview(self.preview_wanted.get());
        self.update_status();
        self.select_files(&chosen);
        if view == View::Gallery && self.selected().is_empty() && self.flat.n_items() > 0 {
            self.flat.select_item(0, true);
        }
        self.selection_changed();
        self.focus_content();
    }

    /// The keyboard at what is shown, so arrows, space and typing go to the files.
    pub fn focus_content(&self) {
        match self.view.get() {
            View::Icons => self.grid.grab_focus(),
            View::List => self.column_view.grab_focus(),
            View::Columns => {
                let list = self.columns.borrow().last().and_then(|c| c.scroller.child());
                list.is_some_and(|list| list.grab_focus())
            }
            // On what is chosen, not the strip's first item, so only one tile is marked.
            View::Gallery => match (0..self.flat.n_items()).find(|&i| self.flat.is_selected(i)) {
                Some(chosen) => {
                    self.strip.scroll_to(chosen, gtk::ListScrollFlags::FOCUS, None);
                    true
                }
                None => self.strip.grab_focus(),
            },
        };
    }

    /// Puts the files under headers by what the window groups them by, or not.
    pub fn regroup(&self) {
        let group = self.window().map_or(Group::None, |w| w.prefs.borrow().group);
        let Some(now) = glib::DateTime::now_local().ok().filter(|_| group != Group::None) else {
            self.sorted.set_section_sorter(None::<&gtk::Sorter>);
            self.column_view.set_header_factory(None::<&gtk::ListItemFactory>);
            self.feed_views(false);
            return;
        };
        let sorter = {
            let now = now.clone();
            gtk::CustomSorter::new(move |a, b| {
                let (Some(a), Some(b)) = (
                    a.downcast_ref::<gio::FileInfo>(),
                    b.downcast_ref::<gio::FileInfo>(),
                ) else {
                    return gtk::Ordering::Equal;
                };
                group_of(a, group, &now).cmp(&group_of(b, group, &now)).into()
            })
        };
        // The headers are only for a list of sections: on any other list GTK makes one header.
        let headers = gtk::SignalListItemFactory::new();
        headers.connect_setup(|_, header| {
            if let Some(header) = header.downcast_ref::<gtk::ListHeader>() {
                header.set_child(Some(
                    &gtk::Label::builder()
                        .xalign(0.0)
                        .margin_start(EDGE)
                        .css_classes(["heading"])
                        .build(),
                ));
            }
        });
        headers.connect_bind(move |_, header| {
            let Some(header) = header.downcast_ref::<gtk::ListHeader>() else {
                return;
            };
            if let (Some(label), Some(info)) = (
                header.child().and_downcast::<gtk::Label>(),
                header.item().and_downcast::<gio::FileInfo>(),
            ) {
                label.set_label(&group_of(&info, group, &now).1);
            }
        });
        self.sorted.set_section_sorter(Some(&sorter));
        self.column_view.set_header_factory(Some(&headers));
        self.feed_views(false);
    }

    /// What the list is, in order: a tree of folders, or the files under their headers.
    fn list_selection(&self) -> &gtk::MultiSelection {
        if self.sorted.section_sorter().is_some() {
            &self.flat
        } else {
            &self.tree_selection
        }
    }

    fn checks_shown(&self) -> bool {
        self.window().is_some_and(|w| w.prefs.borrow().checks)
    }

    /// Makes the views make their rows and tiles again, as for check boxes shown or not.
    pub fn rebind(&self) {
        self.grid.set_model(None::<&gtk::SelectionModel>);
        self.column_view.set_model(None::<&gtk::SelectionModel>);
        self.feed_views(false);
    }

    /// A check box that shows whether `item` is chosen, and chooses it.
    fn check_for(self: &Rc<Self>, item: &gtk::ListItem) -> gtk::CheckButton {
        let check = gtk::CheckButton::builder().valign(gtk::Align::Center).build();
        item.bind_property("selected", &check, "active")
            .sync_create()
            .build();
        let (weak, item) = (Rc::downgrade(self), item.downgrade());
        check.connect_toggled(move |check| {
            let (Some(pane), Some(item)) = (weak.upgrade(), item.upgrade()) else {
                return;
            };
            if check.is_active() == item.is_selected() {
                return;
            }
            let model = pane.active_selection();
            if check.is_active() {
                model.select_item(item.position(), false);
            } else {
                model.unselect_item(item.position());
            }
        });
        check
    }

    /// Gives the files to the view in sight alone, and to the one fading out: a view out of
    /// sight would still make a row or tile of each file, and read its picture, at every
    /// folder.
    fn feed_views(&self, starve: bool) {
        let shown = self.views.visible_child_name();
        let starve = starve && !self.views.is_transition_running();
        let fed = |name: &str, fed: bool| shown.as_deref() == Some(name) || (fed && !starve);
        self.grid
            .set_model(fed("icons", self.grid.model().is_some()).then_some(&self.flat));
        self.strip
            .set_model(fed("gallery", self.strip.model().is_some()).then_some(&self.flat));
        self.column_view
            .set_model(fed("list", self.column_view.model().is_some()).then_some(self.list_selection()));
    }

    fn set_icon_size(self: &Rc<Self>, size: i32) {
        if size == self.icon_size.get() {
            return;
        }
        self.icon_size.set(size);
        // Binds every tile again at the new size.
        if self.grid.model().is_some() {
            self.grid.set_model(None::<&gtk::SelectionModel>);
            self.grid.set_model(Some(&self.flat));
        }
        if let Some(win) = self.window() {
            win.prefs.borrow_mut().icon_size = size;
            win.prefs.borrow().save();
        }
    }

    fn update_status(&self) {
        let view = self.view.get();
        let error = matches!(*self.here.borrow(), Where::Folder(_))
            && !self.is_searching()
            && self.directory.borrow().error().is_some();
        let loading = self.directory.borrow().is_loading() && matches!(*self.here.borrow(), Where::Folder(_));
        let empty = self.filtered.n_items() == 0 && !loading;
        let text = if error {
            Some("無法讀取這個檔案夾")
        } else if empty && self.is_searching() {
            Some("沒有符合的檔案")
        } else if empty && view != View::Columns {
            Some("沒有項目")
        } else {
            None
        };
        let name = match text {
            Some(text) => {
                self.status.set_label(text);
                "status"
            }
            None if self.is_searching() && matches!(view, View::Columns | View::Gallery) => "list",
            None => view.name(),
        };
        if self.views.visible_child_name().as_deref() != Some(name) {
            // Under a crossfade the page changes at once; a fade of its own would show the
            // page going already emptied.
            let transition = if self.fading.state() == adw::AnimationState::Playing {
                gtk::StackTransitionType::None
            } else {
                self.views.transition_type()
            };
            self.views.set_visible_child_full(name, transition);
        }
        self.update_count();
    }

    fn update_count(&self) {
        let total = self.filtered.n_items();
        let chosen = self.selected();
        let items = if chosen.is_empty() {
            format!("{total} 個項目")
        } else {
            format!("已選取 {} / {total} 個項目", chosen.len())
        };
        // The files' size, as folders are not added up here; then not the space left.
        let size: u64 = chosen
            .iter()
            .filter(|i| !is_dir(i))
            .map(|i| i.size().max(0) as u64)
            .sum();
        if chosen.iter().any(|i| !is_dir(i)) {
            self.count
                .set_label(&format!("{items}，{}", glib::format_size(size)));
            return;
        }
        self.count.set_label(&items);
        let Some(folder) = self.here().folder().filter(|_| !self.is_searching()) else {
            return;
        };
        let count = self.count.downgrade();
        folder.query_filesystem_info_async(
            "filesystem::free",
            glib::Priority::LOW,
            gio::Cancellable::NONE,
            move |info| {
                let (Some(count), Ok(info)) = (count.upgrade(), info) else {
                    return;
                };
                // Unless what is chosen changed the line since.
                if info.has_attribute("filesystem::free") && count.label() == items {
                    let free = glib::format_size(info.attribute_uint64("filesystem::free"));
                    count.set_label(&format!("{items}，可用 {free}"));
                }
            },
        );
    }

    fn fill_crumbs(self: &Rc<Self>) {
        while let Some(child) = self.crumbs.first_child() {
            self.crumbs.remove(&child);
        }
        let mut trail: Vec<(String, Where)> = Vec::new();
        match &*self.here.borrow() {
            Where::Folder(file) => {
                let home = gio::File::for_path(home());
                let mounts = gio::VolumeMonitor::get().mounts();
                let mut at = Some(file.clone());
                while let Some(folder) = at {
                    let top = folder.equal(&home)
                        || folder.uri_scheme().as_deref() != Some("file") && folder.parent().is_none()
                        || mounts.iter().any(|m| m.root().equal(&folder));
                    let parent = folder.parent();
                    trail.push((folder_title(&folder), Where::Folder(folder)));
                    at = if top { None } else { parent };
                }
                trail.reverse();
            }
            other => trail.push((other.title(), other.clone())),
        }
        let last = trail.len().saturating_sub(1);
        for (index, (label, to)) in trail.into_iter().enumerate() {
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
                .css_classes(["flat", "nf-crumb", "nf-small"])
                .build();
            if index == last {
                button.add_css_class("current");
            }
            let going = to.clone();
            button.connect_clicked(on!(self, |p, _b| p.go(going.clone(), true)));
            // A crumb takes what is dropped on it, and springs open.
            if let Where::Folder(folder) = &to {
                let drop = gtk::DropTarget::new(
                    gdk::FileList::static_type(),
                    gdk::DragAction::COPY | gdk::DragAction::MOVE,
                );
                let folder = folder.clone();
                let into = folder.clone();
                drop.connect_drop(move |target, value, _, _| {
                    let Ok(list) = value.get::<gdk::FileList>() else {
                        return false;
                    };
                    ops::drop_into(&list.files(), &into, target.current_event_state());
                    true
                });
                self.spring(&drop, button.upcast_ref(), move || {
                    Some(Where::Folder(folder.clone()))
                });
                button.add_controller(drop);
            }
            self.crumbs.append(&button);
        }
    }

    // The columns view.

    /// The folders the columns show: from home, a drive or the root down to here.
    fn chain(&self) -> Vec<gio::File> {
        let Some(here) = self.here().folder() else {
            return Vec::new();
        };
        let home = gio::File::for_path(home());
        let mounts = gio::VolumeMonitor::get().mounts();
        let mut chain = Vec::new();
        let mut at = Some(here);
        while let Some(folder) = at {
            let top = folder.equal(&home) || mounts.iter().any(|m| m.root().equal(&folder));
            let parent = folder.parent();
            chain.push(folder);
            at = if top { None } else { parent };
        }
        chain.reverse();
        chain
    }

    fn sync_columns(self: &Rc<Self>) {
        let chain = self.chain();
        if let Some(peek) = self.peek.take() {
            self.columns_box.remove(&peek);
        }
        {
            let mut columns = self.columns.borrow_mut();
            let keep = columns
                .iter()
                .zip(&chain)
                .take_while(|(column, folder)| column.folder.equal(*folder))
                .count();
            for column in columns.drain(keep..) {
                self.columns_box.remove(&column.scroller);
            }
        }
        let start = self.columns.borrow().len();
        for folder in &chain[start..] {
            let column = self.make_column(folder);
            self.columns_box.append(&column.scroller);
            self.columns.borrow_mut().push(column);
        }
        // Each column chooses the folder the next one shows.
        self.choosing.set(true);
        let columns = self.columns.borrow();
        for (index, column) in columns.iter().enumerate() {
            match chain.get(index + 1) {
                Some(next) => choose_when_read(&column.list, &column.selection, std::slice::from_ref(next)),
                None => {
                    column.selection.unselect_all();
                }
            }
        }
        drop(columns);
        self.choosing.set(false);
        self.scroll_columns_to_end();
    }

    fn make_column(self: &Rc<Self>, folder: &gio::File) -> Column {
        let list = listing(folder, ATTRIBUTES);
        let filtered = gtk::FilterListModel::new(Some(list.clone()), Some(self.shown.clone()));
        let sorted = gtk::SortListModel::new(Some(filtered), Some(self.sorter.clone()));
        let selection = gtk::MultiSelection::new(Some(sorted));
        let factory = gtk::SignalListItemFactory::new();
        let weak = Rc::downgrade(self);
        factory.connect_setup(move |_, item| {
            let (Some(item), Some(pane)) = (item.downcast_ref::<gtk::ListItem>(), weak.upgrade()) else {
                return;
            };
            // A CJK name's line is taller than a Latin one's; every row is as tall as the taller.
            let line = gtk::Box::builder().spacing(GAP).height_request(20).build();
            line.append(&gtk::Image::builder().pixel_size(16).build());
            line.append(
                &gtk::Label::builder()
                    .xalign(0.0)
                    .hexpand(true)
                    .ellipsize(gtk::pango::EllipsizeMode::Middle)
                    .build(),
            );
            line.append(&tags::dots(&[]));
            line.append(
                &gtk::Image::builder()
                    .icon_name("go-next-symbolic")
                    .pixel_size(12)
                    .css_classes(["dim-label"])
                    .build(),
            );
            item.set_child(Some(&line));
            pane.set_up_item(&line.upcast(), item);
        });
        factory.connect_bind(|_, item| {
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let (Some(line), Some(info)) = (item.child(), list_item_info(item)) else {
                return;
            };
            let image = line
                .first_child()
                .and_downcast::<gtk::Image>()
                .expect("set up above");
            let label = image
                .next_sibling()
                .and_downcast::<gtk::Label>()
                .expect("set up above");
            let dots = label
                .next_sibling()
                .and_downcast::<gtk::Box>()
                .expect("set up above");
            let arrow = dots.next_sibling().expect("set up above");
            image.set_from_gicon(&thumbs::icon(&info));
            label.set_label(&info.display_name());
            tags::fill_dots(&dots, &tags::of(&info));
            arrow.set_opacity(if is_dir(&info) { 1.0 } else { 0.0 });
        });
        let view = gtk::ListView::builder()
            .model(&selection)
            .factory(&factory)
            .enable_rubberband(true)
            .css_classes(["nf-column"])
            .build();
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .width_request(COLUMN_WIDTH)
            .child(&view)
            .build();
        scroller.add_css_class("nf-column-frame");
        let chosen_folder = folder.clone();
        selection.connect_selection_changed(on!(self, |p, s, _a, _b| p.column_chosen(&chosen_folder, s)));
        view.connect_activate(on!(self, |p, v, position| {
            if let Some(info) = v
                .model()
                .and_then(|m| m.item(position))
                .and_downcast::<gio::FileInfo>()
            {
                if !is_dir(&info) {
                    ops::open(&p.win, &[info]);
                }
            }
        }));
        // A right click below the last item: this folder's menu.
        Column {
            folder: folder.clone(),
            scroller,
            list,
            selection,
        }
    }

    fn column_chosen(self: &Rc<Self>, folder: &gio::File, selection: &gtk::MultiSelection) {
        if self.choosing.get() {
            return;
        }
        let chosen: Vec<gio::FileInfo> = selected_of(selection)
            .into_iter()
            .filter_map(|o| o.downcast().ok())
            .collect();
        match chosen.as_slice() {
            [info] if is_dir(info) => {
                // A column chooses the folder after it once read, which may be long after it
                // was made: choosing where the tab already is, or a folder above, goes nowhere.
                let here = self.here().folder();
                let child =
                    info_file(info).filter(|c| !here.as_ref().is_some_and(|h| h.equal(c) || h.has_prefix(c)));
                if let Some(child) = child {
                    self.go(Where::Folder(child), true);
                }
            }
            [] => {
                if let Some(peek) = self.peek.take() {
                    self.columns_box.remove(&peek);
                }
            }
            // A file, or several items: this column is the last, as no one folder is open.
            chosen => {
                if !self.here().folder().is_some_and(|h| h.equal(folder)) {
                    self.go(Where::Folder(folder.clone()), true);
                    // Going there chose nothing in this column; what was chosen again.
                    if let Some(column) = self.columns.borrow().last() {
                        self.choosing.set(true);
                        let files: Vec<gio::File> = chosen.iter().filter_map(info_file).collect();
                        choose_when_read(&column.list, &column.selection, &files);
                        self.choosing.set(false);
                    }
                }
                match chosen {
                    [info] => self.show_peek(info),
                    _ => {
                        if let Some(peek) = self.peek.take() {
                            self.columns_box.remove(&peek);
                        }
                    }
                }
            }
        }
        self.selection_changed();
    }

    fn show_peek(self: &Rc<Self>, info: &gio::FileInfo) {
        if let Some(peek) = self.peek.take() {
            self.columns_box.remove(&peek);
        }
        let facts = Facts::new(true, false);
        facts.root.set_width_request(300);
        facts.show(std::slice::from_ref(info), None, &self.win);
        let peek = crate::preview::scrolled_facts(&facts);
        self.columns_box.append(&peek);
        self.peek.replace(Some(peek.upcast()));
        self.scroll_columns_to_end();
    }

    fn scroll_columns_to_end(&self) {
        let adjustment = self.columns_scroller.hadjustment();
        glib::idle_add_local_once(move || adjustment.set_value(adjustment.upper()));
    }

    // What is chosen.

    pub fn selected(&self) -> Vec<gio::FileInfo> {
        match self.view.get() {
            View::List => selected_of(self.list_selection())
                .into_iter()
                .filter_map(item_info)
                .collect(),
            View::Columns if !self.is_searching() => self
                .columns
                .borrow()
                .iter()
                .rev()
                .map(|c| selected_of(&c.selection))
                .find(|chosen| !chosen.is_empty())
                .unwrap_or_default()
                .into_iter()
                .filter_map(|o| o.downcast::<gio::FileInfo>().ok())
                .collect(),
            _ => selected_of(&self.flat)
                .into_iter()
                .filter_map(|o| o.downcast::<gio::FileInfo>().ok())
                .collect(),
        }
    }

    pub fn selected_files(&self) -> Vec<gio::File> {
        self.selected().iter().filter_map(info_file).collect()
    }

    /// Everything listed, in order, at the top level.
    pub fn listed(&self) -> Vec<gio::FileInfo> {
        (0..self.flat.n_items())
            .filter_map(|i| self.flat.item(i).and_downcast::<gio::FileInfo>())
            .collect()
    }

    /// What choosing works on: in the columns, the one with the keyboard, else the folder
    /// shown.
    fn active_selection(&self) -> gtk::SelectionModel {
        match self.view.get() {
            View::List => self.list_selection().clone().upcast(),
            View::Columns if !self.is_searching() => {
                let columns = self.columns.borrow();
                columns
                    .iter()
                    .find(|c| c.scroller.state_flags().contains(gtk::StateFlags::FOCUS_WITHIN))
                    .or(columns.last())
                    .map_or_else(|| self.flat.clone().upcast(), |c| c.selection.clone().upcast())
            }
            _ => self.flat.clone().upcast(),
        }
    }

    /// Chooses the item at `position` of the view `widget` is in, alone.
    fn select_only(&self, widget: &gtk::Widget, position: u32) {
        let model = if self.view.get() == View::Columns && !self.is_searching() {
            let columns = self.columns.borrow();
            match columns.iter().find(|c| widget.is_ancestor(&c.scroller)) {
                Some(column) => column.selection.clone().upcast(),
                None => return,
            }
        } else {
            self.active_selection()
        };
        model.select_item(position, true);
    }

    pub fn unselect_all(&self) {
        self.active_selection().unselect_all();
    }

    pub fn select_all(&self) {
        self.active_selection().select_all();
    }

    /// Chooses what is not chosen, and nothing of what is.
    pub fn invert_selection(&self) {
        let model = self.active_selection();
        let all = gtk::Bitset::new_range(0, model.n_items());
        let inverted = all.copy();
        inverted.subtract(&model.selection());
        model.set_selection(&inverted, &all);
    }

    /// Chooses these files where the view lists them.
    pub fn select_files(&self, files: &[gio::File]) {
        if files.is_empty() {
            return;
        }
        if self.view.get() == View::Columns {
            // The last column is the folder shown: the file chosen there, its facts beside it.
            let last = self
                .columns
                .borrow()
                .last()
                .map(|c| (c.list.clone(), c.selection.clone()));
            if let Some((list, selection)) = last {
                choose_when_read(&list, &selection, files);
            }
            return;
        }
        let model = self.active_selection();
        model.unselect_all();
        for position in 0..model.n_items() {
            if model
                .item(position)
                .and_then(item_file)
                .is_some_and(|f| files.iter().any(|g| g.equal(&f)))
            {
                model.select_item(position, false);
            }
        }
    }

    /// Chooses the file of this name once the folder has been read, as a new folder or a
    /// pasted file is chosen.
    pub fn select_when_listed(self: &Rc<Self>, file: gio::File, then: impl FnOnce(&Rc<Pane>) + 'static) {
        let weak = Rc::downgrade(self);
        let mut then = Some(then);
        let mut tries = 0;
        glib::timeout_add_local(Duration::from_millis(50), move || {
            let Some(pane) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            tries += 1;
            let listed = pane
                .listed()
                .iter()
                .any(|i| info_file(i).is_some_and(|f| f.equal(&file)));
            if listed {
                pane.select_files(std::slice::from_ref(&file));
                if let Some(then) = then.take() {
                    then(&pane);
                }
                return glib::ControlFlow::Break;
            }
            if tries > 40 {
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
    }

    /// One step through what is listed, as Quick Look's arrows go.
    pub fn move_selection(&self, by: i32) {
        let model = self.active_selection();
        let n = model.n_items() as i32;
        if n == 0 {
            return;
        }
        let first = (0..n)
            .find(|&i| model.is_selected(i as u32))
            .unwrap_or(-by.signum());
        let next = (first + by).clamp(0, n - 1);
        model.select_item(next as u32, true);
        match self.view.get() {
            View::List => self
                .column_view
                .scroll_to(next as u32, None, gtk::ListScrollFlags::FOCUS, None),
            View::Gallery => self
                .strip
                .scroll_to(next as u32, gtk::ListScrollFlags::FOCUS, None),
            View::Columns if !self.is_searching() => {}
            _ => self
                .grid
                .scroll_to(next as u32, gtk::ListScrollFlags::FOCUS, None),
        }
    }

    /// An item the listing replaces in place, as when its tags change, stays chosen.
    fn keep_chosen(&self, model: &gtk::SelectionModel, at: u32, removed: u32, added: u32) {
        if removed == 0 || added == 0 {
            return;
        }
        let chosen = self.chosen_files.borrow().clone();
        for position in (at..at + added).filter(|p| !model.is_selected(*p)) {
            let file = model.item(position).and_then(item_file);
            if file.is_some_and(|f| chosen.iter().any(|c| c.equal(&f))) {
                model.select_item(position, false);
            }
        }
    }

    fn selection_changed(self: &Rc<Self>) {
        let chosen = self.selected();
        // A folder chosen is read while the second click of opening it comes.
        if let [one] = chosen.as_slice() {
            if let Some(folder) = info_file(one).filter(|f| is_dir(one) && f.is_native()) {
                listing(&folder, ATTRIBUTES);
            }
        }
        self.chosen_files
            .replace(chosen.iter().filter_map(info_file).collect());
        if self.preview.is_visible() {
            self.facts.show(&chosen, self.here().folder().as_ref(), &self.win);
        }
        if self.view.get() == View::Gallery {
            self.show_stage(chosen.first());
        }
        self.update_count();
        if let Some(win) = self.window() {
            win.selection_changed(self);
        }
    }

    pub fn show_preview(self: &Rc<Self>, wanted: bool) {
        self.preview_wanted.set(wanted);
        // Columns and the gallery show what is chosen in a column of their own.
        let shown = wanted && !matches!(self.view.get(), View::Columns | View::Gallery);
        self.preview.set_visible(shown);
        if shown {
            self.facts
                .show(&self.selected(), self.here().folder().as_ref(), &self.win);
        }
    }

    fn show_stage(&self, info: Option<&gio::FileInfo>) {
        let stack = self
            .stage
            .parent()
            .and_downcast::<gtk::Stack>()
            .expect("set up in new");
        let Some(info) = info else {
            self.stage_shown.replace(String::new());
            self.stage_icon.set_icon_name(Some("folder-pictures-symbolic"));
            stack.set_visible_child_name("icon");
            return;
        };
        let file = info_file(info);
        let uri = file.as_ref().map(|f| f.uri().to_string()).unwrap_or_default();
        if *self.stage_shown.borrow() == uri {
            return;
        }
        self.stage_shown.replace(uri.clone());
        self.gallery_facts
            .show(std::slice::from_ref(info), None, &self.win);
        self.stage_icon.set_from_gicon(&thumbs::icon(info));
        stack.set_visible_child_name("icon");
        let content_type = info.content_type().map(|t| t.to_string()).unwrap_or_default();
        let Some(path) = file.and_then(|f| f.path()) else {
            return;
        };
        let (stage, shown) = (self.stage.downgrade(), self.stage_shown.clone());
        let show = move |texture: Option<gdk::Texture>| {
            let (Some(stage), Some(texture)) = (stage.upgrade(), texture) else {
                return;
            };
            if *shown.borrow() == uri {
                stage.set_paintable(Some(&texture));
                if let Some(stack) = stage.parent().and_downcast::<gtk::Stack>() {
                    stack.set_visible_child_name("picture");
                }
            }
        };
        if content_type.starts_with("image/") || content_type == "application/pdf" {
            thumbs::read(&path, 1600, show);
        } else if let Some(frame) = info
            .attribute_byte_string("thumbnail::path")
            .filter(|_| thumbs::can_make(&content_type))
        {
            // A video's frame, from its thumbnail.
            thumbs::read(std::path::Path::new(frame.as_str()), 1600, show);
        }
    }

    // Doing.

    fn activate_tree(self: &Rc<Self>, position: u32) {
        if let Some(info) = self.list_selection().item(position).and_then(item_info) {
            self.activate(&info);
        }
    }

    fn activate_flat(self: &Rc<Self>, position: u32) {
        if let Some(info) = self.flat.item(position).and_downcast::<gio::FileInfo>() {
            self.activate(&info);
        }
    }

    /// A double click or Enter: into a folder, the file opened.
    fn activate(self: &Rc<Self>, info: &gio::FileInfo) {
        if is_dir(info) {
            if let Some(file) = ops::target(info) {
                self.go(Where::Folder(file), true);
            }
        } else if self.here.borrow().is_trash() {
            if let Some(win) = self.window() {
                win.quick_look();
            }
        } else {
            ops::open(&self.win, std::slice::from_ref(info));
        }
    }

    /// The menu from the keyboard: under the first of what is chosen that is in view, or
    /// the folder's own at its corner.
    pub fn menu_from_keys(self: &Rc<Self>) {
        let chosen = ITEM_WIDGETS.with_borrow(|all| {
            all.iter().find_map(|(widget, item)| {
                let widget = widget.upgrade()?;
                (item.upgrade()?.is_selected() && widget.is_mapped() && widget.is_ancestor(&self.root))
                    .then_some(widget)
            })
        });
        match chosen {
            Some(widget) => self.menu_at(&widget, 0.0, f64::from(widget.height())),
            None => self.menu_at(self.root.upcast_ref(), 0.0, 0.0),
        }
    }

    fn menu_at(self: &Rc<Self>, widget: &gtk::Widget, x: f64, y: f64) {
        let Some(win) = self.window() else { return };
        let (menu, custom) = ops::menu(&win, self, &self.selected());
        let popover = gtk::PopoverMenu::from_model(Some(&menu));
        for (id, child) in custom {
            popover.add_child(&child, &id);
        }
        popover.set_parent(&self.root);
        popover.set_has_arrow(false);
        popover.set_halign(gtk::Align::Start);
        let point = widget
            .compute_point(&self.root, &gtk::graphene::Point::new(x as f32, y as f32))
            .unwrap_or_else(|| gtk::graphene::Point::new(x as f32, y as f32));
        popover.set_pointing_to(Some(&gdk::Rectangle::new(
            point.x() as i32,
            point.y() as i32,
            1,
            1,
        )));
        let weak = Rc::downgrade(self);
        popover.connect_closed(move |popover| {
            // Back to the items, or GTK gives the focus to the sidebar, which then chooses
            // its first row; unless what was chosen in the menu opened a dialog.
            if let Some(pane) = weak.upgrade() {
                let in_dialog = popover
                    .root()
                    .and_then(|root| root.focus())
                    .is_some_and(|focus| focus.ancestor(adw::Dialog::static_type()).is_some());
                if !in_dialog {
                    pane.focus_content();
                }
            }
            let popover = popover.clone();
            glib::idle_add_local_once(move || popover.unparent());
        });
        popover.popup();
    }

    // Work that shows on a row.

    pub fn set_job(&self, uri: &str, fraction: Option<f64>) {
        {
            let mut jobs = self.jobs.borrow_mut();
            jobs.retain(|(u, _)| u != uri);
            if let Some(fraction) = fraction {
                jobs.push((uri.to_owned(), fraction));
            }
        }
        self.bars.borrow_mut().retain(|bar| bar.upgrade().is_some());
        for bar in self.bars.borrow().iter().filter_map(|b| b.upgrade()) {
            if bar.widget_name() == uri {
                self.show_job(&bar);
            }
        }
    }

    fn show_job(&self, bar: &gtk::ProgressBar) {
        let name = bar.widget_name();
        let job = self
            .jobs
            .borrow()
            .iter()
            .find(|(uri, _)| *uri == name)
            .map(|(_, f)| *f);
        bar.set_visible(job.is_some());
        if let Some(fraction) = job {
            bar.set_fraction(fraction);
        }
    }
}

/// Opens what a drag waits over, a folder in a view or a place in the sidebar: after a
/// moment it blinks twice and `go` goes there.
pub fn spring(
    drop: &gtk::DropTarget,
    widget: &gtk::Widget,
    target: impl Fn() -> Option<Where> + 'static,
    go: impl Fn(Where) + 'static,
) {
    let timer: Rc<RefCell<Option<glib::SourceId>>> = Rc::default();
    let (target, go) = (Rc::new(target), Rc::new(go));
    {
        let (timer, widget) = (timer.clone(), widget.downgrade());
        drop.connect_enter(move |_, _, _| {
            let (timer_in, widget, target, go) = (timer.clone(), widget.clone(), target.clone(), go.clone());
            if let Some(old) = timer.borrow_mut().take() {
                old.remove();
            }
            *timer.borrow_mut() = Some(glib::timeout_add_local_once(SPRING, move || {
                timer_in.borrow_mut().take();
                let (Some(widget), Some(to)) = (widget.upgrade(), target()) else {
                    return;
                };
                widget.add_css_class("nf-spring");
                glib::timeout_add_local_once(SPRING_BLINK, move || {
                    widget.remove_css_class("nf-spring");
                    go(to);
                });
            }));
            gdk::DragAction::MOVE
        });
    }
    drop.connect_leave(move |_| {
        if let Some(old) = timer.borrow_mut().take() {
            old.remove();
        }
    });
}

fn conditions_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    let kinds = gio::Menu::new();
    for (index, (name, _)) in search::KINDS.iter().enumerate() {
        kinds.append(Some(name), Some(&format!("pane.kind({index})")));
    }
    let since = gio::Menu::new();
    for (index, (name, _)) in search::SINCE.iter().enumerate() {
        since.append(Some(name), Some(&format!("pane.since({index})")));
    }
    menu.append_submenu(Some("種類"), &kinds);
    menu.append_submenu(Some("修改日期"), &since);
    menu
}

/// Selects `file` in a column once its folder has been read.
/// The file an item of a selection model is, a row of the list or the item itself.
fn item_file(item: glib::Object) -> Option<gio::File> {
    item_info(item).and_then(|i| info_file(&i))
}

/// The file's info an item of a selection model is, a row of the list or the item itself.
fn item_info(item: glib::Object) -> Option<gio::FileInfo> {
    match item.downcast::<gtk::TreeListRow>() {
        Ok(row) => row.item().and_downcast(),
        Err(item) => item.downcast().ok(),
    }
}

/// Chooses these files in a column, and nothing else there, once its folder has been read.
fn choose_when_read(list: &gtk::DirectoryList, selection: &gtk::MultiSelection, files: &[gio::File]) {
    let choose = {
        let (selection, files) = (selection.downgrade(), files.to_vec());
        move || {
            let Some(selection) = selection.upgrade() else {
                return;
            };
            let all = gtk::Bitset::new_range(0, selection.n_items());
            let chosen = gtk::Bitset::new_empty();
            for position in 0..selection.n_items() {
                let file = selection.item(position).and_then(item_file);
                if file.is_some_and(|f| files.iter().any(|g| g.equal(&f))) {
                    chosen.add(position);
                }
            }
            selection.set_selection(&chosen, &all);
        }
    };
    if !list.is_loading() {
        choose();
        return;
    }
    let handler: Rc<RefCell<Option<glib::SignalHandlerId>>> = Rc::default();
    let held = handler.clone();
    let id = list.connect_loading_notify(move |list| {
        if !list.is_loading() {
            choose();
            if let Some(id) = held.borrow_mut().take() {
                list.disconnect(id);
            }
        }
    });
    *handler.borrow_mut() = Some(id);
}

fn selected_of(model: &gtk::MultiSelection) -> Vec<glib::Object> {
    let set = model.selection();
    (0..set.size())
        .filter_map(|i| model.item(set.nth(i as u32)))
        .collect()
}

thread_local! {
    /// Every view's item widgets, with the list item each shows, for `file_at`.
    static ITEM_WIDGETS: RefCell<Vec<(glib::WeakRef<gtk::Widget>, glib::WeakRef<gtk::ListItem>)>> =
        const { RefCell::new(Vec::new()) };
}

/// The file shown at a point of a window, and the whole row or tile it is on, both in the
/// window's coordinates, for the compositor's hover panel.
pub fn file_at(window: &gtk::Window, x: f64, y: f64) -> Option<(PathBuf, gtk::graphene::Rect)> {
    let picked = window.pick(x, y, gtk::PickFlags::DEFAULT)?;
    let (widget, item) =
        std::iter::successors(Some(picked), |widget| widget.parent()).find_map(|widget| {
            let item = ITEM_WIDGETS.with_borrow(|all| {
                all.iter()
                    .find(|(shown, _)| shown.upgrade().as_ref() == Some(&widget))
                    .and_then(|(_, item)| item.upgrade())
            })?;
            Some((widget, item))
        })?;
    let path = info_file(&list_item_info(&item)?)?.path()?;
    let row = std::iter::successors(Some(widget), |widget| widget.parent())
        .find(|widget| widget.parent().is_some_and(|parent| parent.is::<gtk::ListBase>()))?;
    Some((path, row.compute_bounds(window)?))
}

/// The file a list item shows, whether the list is a tree of them or not.
pub fn list_item_info(item: &gtk::ListItem) -> Option<gio::FileInfo> {
    let object = item.item()?;
    match object.downcast::<gtk::TreeListRow>() {
        Ok(row) => row.item().and_downcast(),
        Err(object) => object.downcast().ok(),
    }
}

/// A file's info, with the file itself, as the views read it from a folder.
pub fn query(file: &gio::File) -> Option<gio::FileInfo> {
    let info = file
        .query_info(ATTRIBUTES, gio::FileQueryInfoFlags::NONE, gio::Cancellable::NONE)
        .ok()?;
    info.set_attribute_object("standard::file", file);
    Some(info)
}

pub fn kind_label(info: &gio::FileInfo) -> String {
    if is_dir(info) {
        return "檔案夾".into();
    }
    info.content_type()
        .map(|t| gio::content_type_get_description(&t).to_string())
        .unwrap_or_default()
}

fn date_label(when: Option<glib::DateTime>) -> String {
    match (when, glib::DateTime::now_local().ok()) {
        (Some(when), Some(now)) => modified_label(&when, &now),
        _ => "—".into(),
    }
}

/// Names compared as people read them: any case, and 檔案 2 before 檔案 10.
pub fn natural(a: &str, b: &str) -> std::cmp::Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let number = |chars: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut digits = String::new();
                    while let Some(d) = chars.next_if(char::is_ascii_digit) {
                        digits.push(d);
                    }
                    digits.trim_start_matches('0').to_owned()
                };
                let (m, n) = (number(&mut a), number(&mut b));
                let order = m.len().cmp(&n.len()).then_with(|| m.cmp(&n));
                if order.is_ne() {
                    return order;
                }
            }
            (Some(x), Some(y)) => {
                let order = x.to_lowercase().cmp(y.to_lowercase());
                if order.is_ne() {
                    return order;
                }
                a.next();
                b.next();
            }
        }
    }
}

fn modified_time(info: &gio::FileInfo) -> i64 {
    info.modification_date_time().map_or(0, |t| t.to_unix())
}

/// When a file was made, where the file system keeps it.
pub fn created(info: &gio::FileInfo) -> Option<glib::DateTime> {
    info.has_attribute("time::created")
        .then(|| glib::DateTime::from_unix_local(info.attribute_uint64("time::created") as i64).ok())
        .flatten()
}

/// The group a file is in, ordered as the groups are listed, with the group's title.
pub fn group_of(info: &gio::FileInfo, group: Group, now: &glib::DateTime) -> (u32, String) {
    let titled = |rank: u32, title: &str| (rank, title.to_owned());
    match group {
        Group::None => titled(0, ""),
        Group::Kind if is_dir(info) => titled(0, "檔案夾"),
        Group::Kind => (1, kind_label(info)),
        Group::Modified => {
            let Some(when) = info.modification_date_time() else {
                return titled(5, "更早");
            };
            let (year, month, day) = now.ymd();
            let start = |month: i32, day: i32| glib::DateTime::from_local(year, month, day, 0, 0, 0.0).ok();
            let today = start(month, day);
            let since = [
                today.clone(),
                today.as_ref().and_then(|t| t.add_days(-1).ok()),
                today
                    .as_ref()
                    .and_then(|t| t.add_days(1 - now.day_of_week()).ok()),
                start(month, 1),
                start(1, 1),
            ];
            let titles = ["今天", "昨天", "本週稍早", "本月稍早", "今年稍早"];
            since
                .iter()
                .zip(titles)
                .enumerate()
                .find(|(_, (since, _))| since.as_ref().is_some_and(|s| when >= *s))
                .map_or_else(
                    || titled(5, "更早"),
                    |(rank, (_, title))| titled(rank as u32, title),
                )
        }
        Group::Size if is_dir(info) => titled(0, "檔案夾"),
        Group::Size => {
            const KB: i64 = 1024;
            let size = info.size();
            if size == 0 {
                return titled(1, "空白");
            }
            let sizes = [
                (2, "極小", 16 * KB),
                (3, "小", KB * KB),
                (4, "中", 128 * KB * KB),
                (5, "大", KB * KB * KB),
                (6, "非常大", 4 * KB * KB * KB),
            ];
            sizes
                .iter()
                .find(|(_, _, under)| size < *under)
                .map_or_else(|| titled(7, "超大"), |(rank, title, _)| titled(*rank, title))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_fall_into_groups() {
        let now = glib::DateTime::from_local(2026, 11, 25, 15, 0, 0.0).unwrap(); // a Wednesday
        let file = |size: i64, days_ago: f64| {
            let info = gio::FileInfo::new();
            info.set_file_type(gio::FileType::Regular);
            info.set_size(size);
            info.set_modification_date_time(&now.add_seconds(-days_ago * 86400.0).unwrap());
            info
        };
        let title = |info: &gio::FileInfo, group| group_of(info, group, &now).1;
        assert_eq!(title(&file(0, 0.1), Group::Modified), "今天");
        assert_eq!(title(&file(0, 1.0), Group::Modified), "昨天");
        assert_eq!(title(&file(0, 2.0), Group::Modified), "本週稍早");
        assert_eq!(title(&file(0, 10.0), Group::Modified), "本月稍早");
        assert_eq!(title(&file(0, 100.0), Group::Modified), "今年稍早");
        assert_eq!(title(&file(0, 400.0), Group::Modified), "更早");
        assert_eq!(title(&file(0, 0.0), Group::Size), "空白");
        assert_eq!(title(&file(10_000, 0.0), Group::Size), "極小");
        assert_eq!(title(&file(500_000, 0.0), Group::Size), "小");
        assert_eq!(title(&file(5 << 30, 0.0), Group::Size), "超大");
        let ranks: Vec<u32> = [0, 10_000, 500_000, 5 << 30]
            .map(|size| group_of(&file(size, 0.0), Group::Size, &now).0)
            .into();
        assert!(ranks.is_sorted());
        assert!(
            group_of(&file(0, 1.0), Group::Modified, &now) > group_of(&file(0, 0.0), Group::Modified, &now)
        );
    }

    #[test]
    fn names_sort_as_they_are_read() {
        let mut names = vec!["檔案 10", "檔案 2", "b", "A", "檔案 02x"];
        names.sort_by(|a, b| natural(a, b));
        assert_eq!(names, ["A", "b", "檔案 2", "檔案 02x", "檔案 10"]);
    }
}
