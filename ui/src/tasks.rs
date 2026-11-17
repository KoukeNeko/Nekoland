//! `nekoland-tasks`: the task manager. Four meters across the top, the CPU, memory, disks
//! and network over the last minute, and below them each application with its processes,
//! then the user's other processes. A meter, pressed, sorts the table by what it measures.
//! 結束 asks an application to quit, or a process to end; 強制結束 kills it.

use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet, VecDeque},
    rc::Rc,
    time::{Duration, Instant},
};

use adw::prelude::*;
use gtk::{gdk, gio, glib};

mod alert;
mod ipc;
#[path = "tasks/proc.rs"]
mod proc;

use proc::{Machine, Proc};

const APP_ID: &str = "io.github.koukeneko.Nekoland.Tasks";
/// How far back the meters draw, in readings a second apart.
const HISTORY: usize = 60;
/// How long a row that was ended stays hidden while what it ended quits; it comes back if
/// that did not.
const ENDING: Duration = Duration::from_secs(5);
/// Readings at 90% or more in a row before a meter is marked as running hot.
const HOT_FOR: usize = 5;
/// How wide each column of amounts is.
const COLUMN_WIDTH: i32 = 132;
/// How far an amount, and its column's title, stand in from the column's right edge.
const CELL_INSET: i32 = 8;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Resource {
    Cpu,
    Memory,
    Disk,
}

const RESOURCES: [Resource; 3] = [Resource::Cpu, Resource::Memory, Resource::Disk];

/// One row of the table: an application with its processes, or one process.
#[derive(Clone, Debug, PartialEq)]
struct Entry {
    /// `app:ID` or `pid:N`.
    key: String,
    name: String,
    /// The application's ID, for an application's row.
    app: Option<String>,
    /// The application's row, for one of its processes.
    parent: Option<String>,
    /// The process with the window, or the process itself.
    main: i32,
    pids: Vec<i32>,
    responsive: bool,
    /// Share of the whole machine's CPU, in percent.
    cpu: f64,
    memory: u64,
    /// Bytes a second.
    disk: f64,
}

impl Entry {
    fn amount(&self, resource: Resource) -> f64 {
        match resource {
            Resource::Cpu => self.cpu,
            Resource::Memory => self.memory as f64,
            Resource::Disk => self.disk,
        }
    }

    fn of_apps(&self) -> bool {
        self.app.is_some() || self.parent.is_some()
    }
}

/// A window, as `nekoland msg tasks` tells of it: `pid`, app ID, and 1 if it answers.
#[derive(Debug, PartialEq)]
struct TaskWindow {
    pid: i32,
    app: String,
    responsive: bool,
}

fn parse_windows(reply: &str) -> Vec<TaskWindow> {
    reply
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            Some(TaskWindow {
                pid: fields.next()?.parse().ok()?,
                app: fields.next()?.to_string(),
                responsive: fields.next()? != "0",
            })
        })
        .collect()
}

/// The rows, from this reading and the one `seconds` before, when the machine had used
/// `elapsed` fewer clock ticks: each application, then its processes when it has more than
/// one, then the user's own processes that belong to none.
fn entries(
    procs: &[Proc],
    before: &HashMap<i32, Proc>,
    elapsed: u64,
    seconds: f64,
    windows: &[TaskWindow],
    uid: u32,
) -> Vec<Entry> {
    let windows_of: HashMap<i32, String> = windows
        .iter()
        .map(|window| (window.pid, window.app.clone()))
        .collect();
    let owners = proc::owners(procs, &windows_of);
    let mut apps: Vec<Entry> = Vec::new();
    for window in windows {
        match apps
            .iter_mut()
            .find(|app| app.app.as_deref() == Some(&window.app))
        {
            Some(app) => app.responsive &= window.responsive,
            None => apps.push(Entry {
                key: format!("app:{}", window.app),
                name: window.app.clone(),
                app: Some(window.app.clone()),
                parent: None,
                main: window.pid,
                pids: Vec::new(),
                responsive: window.responsive,
                cpu: 0.0,
                memory: 0,
                disk: 0.0,
            }),
        }
    }
    let mut children = Vec::new();
    let mut background = Vec::new();
    for proc in procs {
        let (cpu, disk) = before
            .get(&proc.pid)
            .filter(|old| old.started == proc.started)
            .map_or((0.0, 0.0), |old| {
                (
                    proc.ticks.saturating_sub(old.ticks) as f64 / elapsed.max(1) as f64 * 100.0,
                    proc.io
                        .zip(old.io)
                        .map_or(0.0, |(now, then)| now.saturating_sub(then) as f64 / seconds),
                )
            });
        let memory = proc.memory.unwrap_or(0);
        let process = |parent: Option<String>| Entry {
            key: format!("pid:{}", proc.pid),
            name: proc.name.clone(),
            app: None,
            parent,
            main: proc.pid,
            pids: vec![proc.pid],
            responsive: true,
            cpu,
            memory,
            disk,
        };
        match owners.get(&proc.pid) {
            Some(owner) => {
                let app = apps
                    .iter_mut()
                    .find(|app| app.app.as_deref() == Some(*owner))
                    .expect("every owner is a window's application");
                app.pids.push(proc.pid);
                app.cpu += cpu;
                app.memory += memory;
                app.disk += disk;
                children.push(process(Some(app.key.clone())));
            }
            // A kernel thread has no memory of its own.
            None if proc.uid == uid && proc.memory.is_some() => background.push(process(None)),
            None => {}
        }
    }
    // A window whose process ended between the two readings.
    apps.retain(|app| !app.pids.is_empty());
    children.retain(|child| {
        apps.iter()
            .any(|app| Some(&app.key) == child.parent.as_ref() && app.pids.len() > 1)
    });
    apps.into_iter().chain(children).chain(background).collect()
}

/// Where each row goes, sorted by `by`, most first: the applications, each followed by its
/// processes, then the other processes.
fn ranks(entries: &[Entry], by: Resource) -> HashMap<String, usize> {
    let most_first = |a: &&Entry, b: &&Entry| {
        b.amount(by)
            .total_cmp(&a.amount(by))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    };
    let mut apps: Vec<&Entry> = entries.iter().filter(|entry| entry.app.is_some()).collect();
    apps.sort_by(most_first);
    let mut background: Vec<&Entry> = entries.iter().filter(|entry| !entry.of_apps()).collect();
    background.sort_by(most_first);
    let mut order = Vec::new();
    for app in apps {
        order.push(app);
        let mut children: Vec<&Entry> = entries
            .iter()
            .filter(|entry| entry.parent.as_ref() == Some(&app.key))
            .collect();
        children.sort_by(most_first);
        order.extend(children);
    }
    order.extend(background);
    order
        .into_iter()
        .enumerate()
        .map(|(rank, entry)| (entry.key.clone(), rank))
        .collect()
}

fn amount_text(resource: Resource, amount: f64) -> String {
    match resource {
        Resource::Cpu => format!("{amount:.1}%"),
        Resource::Memory => glib::format_size(amount as u64).to_string(),
        Resource::Disk if amount < 1.0 => String::new(),
        Resource::Disk => format!("{}/s", glib::format_size(amount as u64)),
    }
}

/// A meter: what it measures now, over a curve of the last minute.
struct Meter {
    root: gtk::Widget,
    value: gtk::Label,
    graph: gtk::DrawingArea,
    history: Rc<RefCell<VecDeque<f64>>>,
}

impl Meter {
    /// `full` is the amount at the top of the curve; None for the largest it has seen.
    fn new(title: &str, toggle: bool, full: Option<f64>) -> Self {
        let value = gtk::Label::builder()
            .xalign(0.0)
            .css_classes(["tm-meter-value"])
            .build();
        let graph = gtk::DrawingArea::builder()
            .content_height(28)
            .hexpand(true)
            .margin_top(4)
            .build();
        let history = Rc::new(RefCell::new(VecDeque::with_capacity(HISTORY)));
        let points = history.clone();
        graph.set_draw_func(move |graph, cr, width, height| {
            let points = points.borrow();
            let top = full.unwrap_or_else(|| points.iter().copied().fold(1.0, f64::max));
            let (width, height) = (width as f64, height as f64);
            let step = width / (HISTORY - 1) as f64;
            let start = width - step * (points.len().max(1) - 1) as f64;
            let at = |index: usize, amount: f64| {
                (
                    start + step * index as f64,
                    0.5 + (height - 1.0) * (1.0 - (amount / top).min(1.0)),
                )
            };
            let ink = graph.color();
            let (red, green, blue) = (ink.red().into(), ink.green().into(), ink.blue().into());
            for (index, amount) in points.iter().enumerate() {
                let (x, y) = at(index, *amount);
                if index == 0 {
                    cr.move_to(x, y);
                } else {
                    cr.line_to(x, y);
                }
            }
            if points.is_empty() {
                return;
            }
            let path = cr.copy_path();
            cr.line_to(width, height);
            cr.line_to(start, height);
            cr.close_path();
            cr.set_source_rgba(red, green, blue, ink.alpha() as f64 * 0.18);
            // A failed fill or stroke leaves the curve blank, with nothing better to show.
            let _ = cr.fill();
            if let Ok(path) = path {
                cr.append_path(&path);
            }
            cr.set_source_rgba(red, green, blue, ink.alpha().into());
            cr.set_line_width(1.0);
            let _ = cr.stroke();
        });
        let content = gtk::Box::new(gtk::Orientation::Vertical, 2);
        content.append(
            &gtk::Label::builder()
                .label(title)
                .xalign(0.0)
                .css_classes(["tm-meter-title"])
                .build(),
        );
        content.append(&value);
        content.append(&graph);
        let root: gtk::Widget = if toggle {
            gtk::ToggleButton::builder()
                .child(&content)
                .hexpand(true)
                .css_classes(["tm-meter"])
                .build()
                .upcast()
        } else {
            content.set_hexpand(true);
            content.add_css_class("tm-meter");
            content.upcast()
        };
        Self {
            root,
            value,
            graph,
            history,
        }
    }

    fn push(&self, amount: f64, text: &str) {
        let mut history = self.history.borrow_mut();
        if history.len() == HISTORY {
            history.pop_front();
        }
        history.push_back(amount);
        let hot =
            history.len() >= HOT_FOR && history.iter().rev().take(HOT_FOR).all(|amount| *amount >= 90.0);
        drop(history);
        self.value.set_label(text);
        if hot {
            self.root.add_css_class("tm-hot");
        } else {
            self.root.remove_css_class("tm-hot");
        }
        self.graph.queue_draw();
    }

    /// The curve's last point down to where it now is, as soon as something is ended.
    fn drop_by(&self, amount: f64) {
        if let Some(last) = self.history.borrow_mut().back_mut() {
            *last = (*last - amount).max(0.0);
        }
        self.graph.queue_draw();
    }
}

/// A row's widgets.
struct Row {
    row: gtk::ListBoxRow,
    revealer: gtk::Revealer,
    expander: gtk::Button,
    name: gtk::Label,
    tag: gtk::Label,
    force: gtk::Button,
    /// Each resource's bar, its length as a share of the longest, and its amount.
    cells: Vec<(gtk::DrawingArea, Rc<Cell<f64>>, gtk::Label)>,
}

/// The pane with the selected row's details.
struct Info {
    root: gtk::Box,
    icon: gtk::Image,
    name: gtk::Label,
    /// PID, 程序, 執行緒, 使用者, 開始時間, 路徑: each title and value.
    facts: Vec<(gtk::Label, gtk::Label)>,
}

const FACTS: [&str; 6] = ["PID", "程序", "執行緒", "使用者", "開始時間", "路徑"];

impl Info {
    fn new() -> Self {
        let icon = gtk::Image::builder()
            .pixel_size(32)
            .halign(gtk::Align::Start)
            .build();
        let name = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .css_classes(["title-4"])
            .build();
        let grid = gtk::Grid::builder()
            .row_spacing(6)
            .column_spacing(12)
            .margin_top(8)
            .build();
        let facts = FACTS
            .iter()
            .enumerate()
            .map(|(row, title)| {
                let title = gtk::Label::builder()
                    .label(*title)
                    .xalign(0.0)
                    .yalign(0.0)
                    .css_classes(["dim-label"])
                    .build();
                let value = gtk::Label::builder()
                    .xalign(0.0)
                    .wrap(true)
                    .wrap_mode(gtk::pango::WrapMode::WordChar)
                    .selectable(true)
                    .hexpand(true)
                    .css_classes(["tm-num"])
                    .build();
                grid.attach(&title, 0, row as i32, 1, 1);
                grid.attach(&value, 1, row as i32, 1, 1);
                (title, value)
            })
            .collect();
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .margin_top(16)
            .margin_bottom(16)
            .margin_start(16)
            .margin_end(16)
            .build();
        root.append(&icon);
        root.append(&name);
        root.append(&grid);
        Self {
            root,
            icon,
            name,
            facts,
        }
    }
}

struct Tasks {
    window: adw::ApplicationWindow,
    list: gtk::ListBox,
    split: adw::OverlaySplitView,
    search: gtk::SearchEntry,
    /// CPU, memory, disks, network.
    meters: Vec<Meter>,
    columns: Vec<gtk::Label>,
    count: gtk::Label,
    info: Info,
    sort: Cell<Resource>,
    rows: RefCell<HashMap<String, Row>>,
    entries: RefCell<HashMap<String, Entry>>,
    ranks: RefCell<HashMap<String, usize>>,
    expanded: RefCell<HashSet<String>>,
    ended: RefCell<HashMap<String, Instant>>,
    /// Each application's name and icon, by its ID.
    apps: RefCell<HashMap<String, (String, Option<gio::Icon>)>>,
    /// The last reading: the processes, the machine, and when.
    before: RefCell<(HashMap<i32, Proc>, Machine, Instant)>,
    uid: u32,
    clock_ticks: u64,
}

thread_local! {
    static TASKS: RefCell<Option<Rc<Tasks>>> = const { RefCell::new(None) };
}

impl Tasks {
    fn open(app: &adw::Application) -> Rc<Self> {
        let meters = vec![
            Meter::new("CPU", true, Some(100.0)),
            Meter::new("記憶體", true, Some(100.0)),
            Meter::new("磁碟", true, None),
            Meter::new("網路", false, None),
        ];
        let meter_row = gtk::Box::builder()
            .spacing(8)
            .homogeneous(true)
            .css_classes(["tm-meters"])
            .build();
        for meter in &meters {
            meter_row.append(&meter.root);
        }
        for meter in &meters[1..3] {
            if let (Some(first), Some(this)) = (
                meters[0].root.downcast_ref::<gtk::ToggleButton>(),
                meter.root.downcast_ref::<gtk::ToggleButton>(),
            ) {
                this.set_group(Some(first));
            }
        }

        let columns_row = gtk::Box::builder().spacing(8).css_classes(["tm-columns"]).build();
        columns_row.append(
            &gtk::Label::builder()
                .label("名稱")
                .xalign(0.0)
                .hexpand(true)
                .margin_start(48)
                .build(),
        );
        let columns: Vec<gtk::Label> = RESOURCES
            .iter()
            .map(|_| {
                let label = gtk::Label::builder()
                    .xalign(1.0)
                    .width_request(COLUMN_WIDTH - CELL_INSET)
                    .margin_end(CELL_INSET)
                    .build();
                columns_row.append(&label);
                label
            })
            .collect();

        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::Single)
            .css_classes(["tm-list"])
            .build();
        let scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&list)
            .build();
        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(&columns_row);
        content.append(&scrolled);

        let info = Info::new();
        let split = adw::OverlaySplitView::builder()
            .sidebar_position(gtk::PackType::End)
            .show_sidebar(false)
            .min_sidebar_width(260.0)
            .max_sidebar_width(300.0)
            .content(&content)
            // Scrolled, or a long path in it makes the window taller even while it is hidden.
            .sidebar(
                &gtk::ScrolledWindow::builder()
                    .hscrollbar_policy(gtk::PolicyType::Never)
                    .child(&info.root)
                    .build(),
            )
            .build();

        let end = gtk::Button::builder()
            .label("結束")
            .action_name("win.end")
            .build();
        let more = gtk::MenuButton::builder()
            .icon_name("view-more-symbolic")
            .tooltip_text("更多")
            .menu_model(&menu(false))
            .build();
        let show_info = gtk::ToggleButton::builder()
            .icon_name("sidebar-show-right-symbolic")
            .tooltip_text("資訊")
            .build();
        show_info
            .bind_property("active", &split, "show-sidebar")
            .bidirectional()
            .build();
        let search = gtk::SearchEntry::builder().placeholder_text("搜尋").build();
        let header = adw::HeaderBar::new();
        header.pack_end(&search);
        header.pack_end(&show_info);
        header.pack_end(&more);
        header.pack_end(&end);

        let count = gtk::Label::builder()
            .xalign(0.0)
            .css_classes(["tm-status"])
            .build();
        let view = adw::ToolbarView::new();
        view.add_top_bar(&header);
        view.add_top_bar(&meter_row);
        view.set_content(Some(&split));
        view.add_bottom_bar(&count);

        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("工作管理員")
            .default_width(1000)
            .default_height(680)
            .width_request(640)
            .height_request(400)
            .content(&view)
            .build();
        search.set_key_capture_widget(Some(&window));

        let procs = proc::read_procs();
        let tasks = Rc::new(Self {
            window,
            list,
            split,
            search,
            meters,
            columns,
            count,
            info,
            sort: Cell::new(Resource::Cpu),
            rows: RefCell::default(),
            entries: RefCell::default(),
            ranks: RefCell::default(),
            expanded: RefCell::default(),
            ended: RefCell::default(),
            apps: RefCell::default(),
            before: RefCell::new((
                procs.into_iter().map(|proc| (proc.pid, proc)).collect(),
                proc::read_machine(),
                Instant::now(),
            )),
            // SAFETY: neither call has preconditions.
            uid: unsafe { libc::getuid() },
            clock_ticks: u64::try_from(unsafe { libc::sysconf(libc::_SC_CLK_TCK) }).unwrap_or(100),
        });
        tasks.connect();
        tasks.show_sort();
        TASKS.set(Some(tasks.clone()));
        tasks.window.connect_close_request(|_| {
            TASKS.set(None);
            glib::Propagation::Proceed
        });
        // The first rates from a short wait, so that the window is not empty for a second.
        glib::timeout_add_local_once(
            Duration::from_millis(250),
            glib::clone!(
                #[weak]
                tasks,
                move || tasks.refresh()
            ),
        );
        glib::timeout_add_seconds_local(
            1,
            glib::clone!(
                #[weak]
                tasks,
                #[upgrade_or]
                glib::ControlFlow::Break,
                move || {
                    tasks.refresh();
                    glib::ControlFlow::Continue
                }
            ),
        );
        tasks.window.present();
        tasks
    }

    fn connect(self: &Rc<Self>) {
        for (resource, meter) in RESOURCES.iter().zip(&self.meters) {
            let Some(toggle) = meter.root.downcast_ref::<gtk::ToggleButton>() else {
                continue;
            };
            toggle.set_active(*resource == self.sort.get());
            let resource = *resource;
            toggle.connect_toggled(glib::clone!(
                #[weak(rename_to = tasks)]
                self,
                move |toggle| {
                    if toggle.is_active() {
                        tasks.sort.set(resource);
                        tasks.show_sort();
                    }
                }
            ));
        }

        self.list.set_sort_func(glib::clone!(
            #[weak(rename_to = tasks)]
            self,
            #[upgrade_or]
            gtk::Ordering::Equal,
            move |a, b| {
                let ranks = tasks.ranks.borrow();
                let rank = |row: &gtk::ListBoxRow| ranks.get(row.widget_name().as_str()).copied();
                rank(a).cmp(&rank(b)).into()
            }
        ));
        self.list.set_filter_func(glib::clone!(
            #[weak(rename_to = tasks)]
            self,
            #[upgrade_or]
            true,
            move |row| tasks.shows(row.widget_name().as_str())
        ));
        self.list.set_header_func(glib::clone!(
            #[weak(rename_to = tasks)]
            self,
            move |row, before| {
                let entries = tasks.entries.borrow();
                let of_apps =
                    |row: &gtk::ListBoxRow| entries.get(row.widget_name().as_str()).map(Entry::of_apps);
                let section = of_apps(row);
                if before.and_then(of_apps) == section {
                    row.set_header(gtk::Widget::NONE);
                    return;
                }
                let title = if section == Some(true) {
                    "應用程式"
                } else {
                    "背景處理程序"
                };
                if row
                    .header()
                    .and_downcast::<gtk::Label>()
                    .is_some_and(|header| header.label() == title)
                {
                    return;
                }
                row.set_header(Some(
                    &gtk::Label::builder()
                        .label(title)
                        .xalign(0.0)
                        .css_classes(["tm-section"])
                        .build(),
                ));
            }
        ));
        self.list.connect_selected_rows_changed(glib::clone!(
            #[weak(rename_to = tasks)]
            self,
            move |_| tasks.show_selection()
        ));
        self.search.connect_search_changed(glib::clone!(
            #[weak(rename_to = tasks)]
            self,
            move |_| tasks.list.invalidate_filter()
        ));
        self.split.connect_show_sidebar_notify(glib::clone!(
            #[weak(rename_to = tasks)]
            self,
            move |_| tasks.show_amounts()
        ));

        let click = gtk::GestureClick::builder().button(gdk::BUTTON_SECONDARY).build();
        click.connect_pressed(glib::clone!(
            #[weak(rename_to = tasks)]
            self,
            move |click, _, x, y| {
                let Some(row) = tasks.list.row_at_y(y as i32) else {
                    return;
                };
                click.set_state(gtk::EventSequenceState::Claimed);
                tasks.list.select_row(Some(&row));
                let popover = gtk::PopoverMenu::from_model(Some(&menu(true)));
                popover.set_parent(&tasks.list);
                popover.set_has_arrow(false);
                popover.set_halign(gtk::Align::Start);
                popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
                popover.connect_closed(|popover| {
                    // Unparented once its action has run, as closing comes before it.
                    let popover = popover.clone();
                    glib::idle_add_local_once(move || popover.unparent());
                });
                popover.popup();
            }
        ));
        self.list.add_controller(click);

        let keys = gtk::ShortcutController::new();
        keys.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("Delete"),
            Some(gtk::NamedAction::new("win.end")),
        ));
        self.list.add_controller(keys);

        let end = gio::SimpleAction::new("end", None);
        end.connect_activate(glib::clone!(
            #[weak(rename_to = tasks)]
            self,
            move |_, _| tasks.end_selected()
        ));
        let force = gio::SimpleAction::new("force-quit", None);
        force.connect_activate(glib::clone!(
            #[weak(rename_to = tasks)]
            self,
            move |_, _| {
                if let Some(key) = tasks.selected() {
                    tasks.ask_force_quit(&key);
                }
            }
        ));
        let info = gio::SimpleAction::new("info", None);
        info.connect_activate(glib::clone!(
            #[weak(rename_to = tasks)]
            self,
            move |_, _| tasks.split.set_show_sidebar(true)
        ));
        for action in [&end, &force] {
            action.set_enabled(false);
        }
        self.window.add_action(&end);
        self.window.add_action(&force);
        self.window.add_action(&info);
    }

    /// Reads /proc and the compositor's windows again, and shows what changed.
    fn refresh(self: &Rc<Self>) {
        let procs = proc::read_procs();
        let machine = proc::read_machine();
        let now = Instant::now();
        let windows = ipc::msg("tasks")
            .map(|reply| parse_windows(&reply))
            .unwrap_or_default();
        let mut list = {
            let (before, machine_before, then) = &*self.before.borrow();
            let seconds = now.duration_since(*then).as_secs_f64().max(0.001);
            let elapsed = machine.total.saturating_sub(machine_before.total);
            let busy = machine.busy.saturating_sub(machine_before.busy);
            let cpu = busy as f64 / elapsed.max(1) as f64 * 100.0;
            self.meters[0].push(cpu, &format!("{cpu:.0}%"));
            let used = machine.memory_total.saturating_sub(machine.memory_available);
            self.meters[1].push(
                used as f64 / machine.memory_total.max(1) as f64 * 100.0,
                &format!(
                    "{} / {}",
                    glib::format_size(used),
                    glib::format_size(machine.memory_total)
                ),
            );
            for (meter, (now, then)) in self.meters[2..].iter().zip([
                (machine.disk, machine_before.disk),
                (machine.network, machine_before.network),
            ]) {
                let rate = now.saturating_sub(then) as f64 / seconds;
                meter.push(rate, &format!("{}/s", glib::format_size(rate as u64)));
            }
            entries(&procs, before, elapsed, seconds, &windows, self.uid)
        };
        for entry in &mut list {
            if let Some(app) = &entry.app {
                entry.name = self.app_name(app).0;
            }
        }
        let processes: usize = list
            .iter()
            .filter(|entry| entry.parent.is_none())
            .map(|entry| entry.pids.len())
            .sum();
        self.count.set_label(&format!("{processes} 個程序"));
        self.ranks.replace(ranks(&list, self.sort.get()));
        self.before.replace((
            procs.into_iter().map(|proc| (proc.pid, proc)).collect(),
            machine,
            now,
        ));
        self.ended.borrow_mut().retain(|_, at| at.elapsed() < ENDING);

        let mut rows = self.rows.borrow_mut();
        rows.retain(|key, row| {
            let keep = list.iter().any(|entry| &entry.key == key);
            if !keep {
                self.list.remove(&row.row);
            }
            keep
        });
        for entry in &list {
            if !rows.contains_key(&entry.key) {
                let row = self.new_row(entry);
                self.list.append(&row.row);
                rows.insert(entry.key.clone(), row);
            }
        }
        drop(rows);
        self.entries
            .replace(list.into_iter().map(|entry| (entry.key.clone(), entry)).collect());
        self.show_amounts();
        self.list.invalidate_sort();
        self.list.invalidate_filter();
        self.list.invalidate_headers();
        self.show_selection();
    }

    fn app_name(&self, app: &str) -> (String, Option<gio::Icon>) {
        self.apps
            .borrow_mut()
            .entry(app.to_string())
            .or_insert_with(|| match gio::DesktopAppInfo::new(&format!("{app}.desktop")) {
                Some(info) => (info.name().to_string(), info.icon()),
                None => (app.to_string(), None),
            })
            .clone()
    }

    fn new_row(self: &Rc<Self>, entry: &Entry) -> Row {
        let line = gtk::Box::builder().spacing(8).css_classes(["tm-line"]).build();
        let expander = gtk::Button::builder()
            .icon_name("pan-end-symbolic")
            .valign(gtk::Align::Center)
            .css_classes(["flat", "tm-expander"])
            .build();
        expander.set_child_visible(false);
        let key = entry.key.clone();
        expander.connect_clicked(glib::clone!(
            #[weak(rename_to = tasks)]
            self,
            move |expander| {
                let open = {
                    let mut expanded = tasks.expanded.borrow_mut();
                    if !expanded.remove(&key) {
                        expanded.insert(key.clone());
                    }
                    expanded.contains(&key)
                };
                expander.set_icon_name(if open {
                    "pan-down-symbolic"
                } else {
                    "pan-end-symbolic"
                });
                tasks.list.invalidate_filter();
            }
        ));
        if entry.parent.is_some() {
            expander.set_margin_start(24);
        }
        line.append(&expander);
        match &entry.app {
            Some(app) => {
                let icon = gtk::Image::builder().pixel_size(16).build();
                match self.app_name(app).1 {
                    Some(gicon) => icon.set_from_gicon(&gicon),
                    None => icon.set_icon_name(Some("application-x-executable-symbolic")),
                }
                line.append(&icon);
            }
            None => line.append(&dot()),
        }
        let name = gtk::Label::builder()
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        line.append(&name);
        let tag = gtk::Label::builder()
            .label("沒有回應")
            .valign(gtk::Align::Center)
            .visible(false)
            .css_classes(["tm-tag"])
            .build();
        line.append(&tag);
        let force = gtk::Button::builder()
            .label("強制結束")
            .valign(gtk::Align::Center)
            .visible(false)
            .css_classes(["tm-force"])
            .build();
        let key = entry.key.clone();
        force.connect_clicked(glib::clone!(
            #[weak(rename_to = tasks)]
            self,
            move |_| tasks.ask_force_quit(&key)
        ));
        line.append(&force);
        let cells = RESOURCES
            .iter()
            .map(|_| {
                let length = Rc::new(Cell::new(0.0));
                let bar = gtk::DrawingArea::new();
                let share = length.clone();
                bar.set_draw_func(move |bar, cr, width, height| {
                    let length = width as f64 * share.get();
                    if length < 1.0 {
                        return;
                    }
                    let ink = bar.color();
                    cr.set_source_rgba(
                        ink.red().into(),
                        ink.green().into(),
                        ink.blue().into(),
                        ink.alpha() as f64 * 0.18,
                    );
                    // 2px clear above and below, so that the bars of rows next to each other stay apart.
                    cr.rectangle(0.0, 2.0, length, height as f64 - 4.0);
                    // A failed fill leaves the bar out, with nothing better to show.
                    let _ = cr.fill();
                });
                let amount = gtk::Label::builder()
                    .halign(gtk::Align::End)
                    .margin_end(CELL_INSET)
                    .css_classes(["tm-num"])
                    .build();
                // The bar behind the amount, the whole cell high.
                let cell = gtk::Overlay::builder()
                    .child(&bar)
                    .width_request(COLUMN_WIDTH)
                    .hexpand(false)
                    .build();
                cell.add_overlay(&amount);
                line.append(&cell);
                (bar, length, amount)
            })
            .collect();
        let revealer = gtk::Revealer::builder()
            .transition_type(gtk::RevealerTransitionType::SlideUp)
            .transition_duration(160)
            .reveal_child(true)
            .child(&line)
            .build();
        let row = gtk::ListBoxRow::builder()
            .name(entry.key.as_str())
            .child(&revealer)
            .build();
        Row {
            row,
            revealer,
            expander,
            name,
            tag,
            force,
            cells,
        }
    }

    /// Each row's name, amounts and bars, from the entries.
    fn show_amounts(&self) {
        let entries = self.entries.borrow();
        let sort = self.sort.get();
        let longest = entries
            .values()
            .map(|entry| entry.amount(sort))
            .fold(0.0, f64::max);
        let info_shown = self.split.shows_sidebar();
        let ended = self.ended.borrow();
        for (key, row) in self.rows.borrow().iter() {
            let Some(entry) = entries.get(key) else { continue };
            let count = entry.pids.len();
            if entry.app.is_some() && count > 1 {
                row.name.set_label(&format!("{}（{count}）", entry.name));
            } else {
                row.name.set_label(&entry.name);
            }
            row.expander.set_child_visible(entry.app.is_some() && count > 1);
            row.tag.set_visible(!entry.responsive);
            row.force.set_visible(!entry.responsive && !info_shown);
            if row.row.has_css_class("tm-alert") == entry.responsive {
                if entry.responsive {
                    row.row.remove_css_class("tm-alert");
                } else {
                    row.row.add_css_class("tm-alert");
                }
            }
            let hidden = ended.contains_key(key)
                || entry
                    .parent
                    .as_ref()
                    .is_some_and(|parent| ended.contains_key(parent));
            row.revealer.set_reveal_child(!hidden);
            for (resource, (bar, length, amount)) in RESOURCES.iter().zip(&row.cells) {
                amount.set_label(&amount_text(*resource, entry.amount(*resource)));
                let share = if *resource == sort && longest > 0.0 {
                    entry.amount(*resource) / longest
                } else {
                    0.0
                };
                if length.replace(share) != share {
                    bar.queue_draw();
                }
            }
        }
    }

    /// The column the table is sorted by, and the order and bars that follow from it.
    fn show_sort(&self) {
        let sort = self.sort.get();
        for ((resource, label), title) in RESOURCES.iter().zip(&self.columns).zip(["CPU", "記憶體", "磁碟"])
        {
            if *resource == sort {
                label.set_label(&format!("{title} ▾"));
                label.add_css_class("tm-sorted");
            } else {
                label.set_label(title);
                label.remove_css_class("tm-sorted");
            }
        }
        let list: Vec<Entry> = self.entries.borrow().values().cloned().collect();
        self.ranks.replace(ranks(&list, sort));
        self.show_amounts();
        self.list.invalidate_sort();
        self.list.invalidate_headers();
    }

    /// Whether a row is in the table: an application's processes when it is open, and with
    /// a search, the rows whose names have it.
    fn shows(&self, key: &str) -> bool {
        let entries = self.entries.borrow();
        let Some(entry) = entries.get(key) else {
            return false;
        };
        let query = self.search.text().trim().to_lowercase();
        if query.is_empty() {
            return entry
                .parent
                .as_ref()
                .is_none_or(|parent| self.expanded.borrow().contains(parent));
        }
        let matches = |entry: &Entry| entry.name.to_lowercase().contains(&query);
        matches(entry)
            || (entry.app.is_some()
                && entries
                    .values()
                    .any(|child| child.parent.as_deref() == Some(key) && matches(child)))
    }

    fn selected(&self) -> Option<String> {
        self.list
            .selected_row()
            .map(|row| row.widget_name().to_string())
            .filter(|key| self.entries.borrow().contains_key(key))
    }

    /// The actions and the information pane, for the selected row.
    fn show_selection(&self) {
        let selected = self.selected();
        for name in ["end", "force-quit"] {
            if let Some(action) = self
                .window
                .lookup_action(name)
                .and_downcast::<gio::SimpleAction>()
            {
                action.set_enabled(selected.is_some());
            }
        }
        let entries = self.entries.borrow();
        let Some(entry) = selected.and_then(|key| entries.get(&key)) else {
            self.info.icon.set_visible(false);
            self.info.name.set_label("未選取");
            for (title, value) in &self.info.facts {
                title.set_visible(false);
                value.set_visible(false);
            }
            return;
        };
        match entry.app.as_deref().and_then(|app| self.app_name(app).1) {
            Some(icon) => self.info.icon.set_from_gicon(&icon),
            None => self
                .info
                .icon
                .set_icon_name(Some("application-x-executable-symbolic")),
        }
        self.info.icon.set_visible(true);
        self.info.name.set_label(&entry.name);
        let before = self.before.borrow();
        let main = before.0.get(&entry.main);
        let threads: u32 = entry
            .pids
            .iter()
            .filter_map(|pid| before.0.get(pid))
            .map(|proc| proc.threads)
            .sum();
        let started = main.and_then(|main| {
            let boot = before.1.boot + main.started / self.clock_ticks.max(1);
            glib::DateTime::from_unix_local(boot as i64)
                .ok()?
                .format("%-m月%-d日 %H:%M")
                .ok()
        });
        let path = std::fs::read_link(format!("/proc/{}/exe", entry.main))
            .ok()
            .map(|path| path.display().to_string());
        let values = [
            Some(entry.main.to_string()),
            (entry.pids.len() > 1).then(|| entry.pids.len().to_string()),
            Some(threads.to_string()),
            main.and_then(|main| proc::user_name(main.uid)),
            started.map(|started| started.to_string()),
            path,
        ];
        for ((title, label), value) in self.info.facts.iter().zip(values) {
            title.set_visible(value.is_some());
            label.set_visible(value.is_some());
            label.set_label(value.as_deref().unwrap_or_default());
        }
    }

    fn end_selected(&self) {
        let Some(key) = self.selected() else { return };
        let Some(entry) = self.entries.borrow().get(&key).cloned() else {
            return;
        };
        let quitting = entry
            .app
            .as_ref()
            .is_some_and(|app| ipc::msg(&format!("quit-app {app}")).is_some());
        if quitting || self.signal(&entry, libc::SIGTERM) {
            self.collapse(&entry);
        }
    }

    fn ask_force_quit(self: &Rc<Self>, key: &str) {
        let Some(entry) = self.entries.borrow().get(key).cloned() else {
            return;
        };
        alert::confirm(
            &self.window,
            &format!("強制結束「{}」？", entry.name),
            Some("未儲存的變更會遺失。"),
            "強制結束",
            glib::clone!(
                #[weak(rename_to = tasks)]
                self,
                move || {
                    let killed = entry
                        .app
                        .as_ref()
                        .is_some_and(|app| ipc::msg(&format!("force-quit-app {app}")).is_some());
                    if killed || tasks.signal(&entry, libc::SIGKILL) {
                        tasks.collapse(&entry);
                    }
                }
            ),
        );
    }

    /// Sends each of an entry's processes `signal`; false, with an alert, if one could not
    /// be sent.
    fn signal(&self, entry: &Entry, signal: i32) -> bool {
        for pid in &entry.pids {
            // SAFETY: kill has no preconditions; a pid that has gone only fails.
            if unsafe { libc::kill(*pid, signal) } == -1 {
                let err = std::io::Error::last_os_error();
                if err.raw_os_error() != Some(libc::ESRCH) {
                    alert::notice(
                        &self.window,
                        &format!("無法結束「{}」", entry.name),
                        Some(&err.to_string()),
                    );
                    return false;
                }
            }
        }
        true
    }

    /// Folds an ended row away, and takes what it used off the meters, before the next
    /// reading shows it gone.
    fn collapse(&self, entry: &Entry) {
        self.ended.borrow_mut().insert(entry.key.clone(), Instant::now());
        self.list.unselect_all();
        let memory_total = self.before.borrow().1.memory_total.max(1) as f64;
        self.meters[0].drop_by(entry.cpu);
        self.meters[1].drop_by(entry.memory as f64 / memory_total * 100.0);
        self.meters[2].drop_by(entry.disk);
        self.show_amounts();
    }
}

/// What stands for a process with no icon of its own: a small ring.
fn dot() -> gtk::DrawingArea {
    let dot = gtk::DrawingArea::builder()
        .content_width(16)
        .content_height(16)
        .valign(gtk::Align::Center)
        .css_classes(["dim-label"])
        .build();
    dot.set_draw_func(|dot, cr, width, height| {
        let ink = dot.color();
        cr.set_source_rgba(
            ink.red().into(),
            ink.green().into(),
            ink.blue().into(),
            ink.alpha().into(),
        );
        cr.set_line_width(1.0);
        cr.arc(
            width as f64 / 2.0,
            height as f64 / 2.0,
            2.5,
            0.0,
            std::f64::consts::TAU,
        );
        // A failed stroke leaves the ring out, with nothing better to show.
        let _ = cr.stroke();
    });
    dot
}

/// The ⋯ menu, and with 結束 first, the menu of a row.
fn menu(with_end: bool) -> gio::Menu {
    let menu = gio::Menu::new();
    if with_end {
        menu.append(Some("結束"), Some("win.end"));
    }
    menu.append(Some("強制結束"), Some("win.force-quit"));
    menu.append(Some("顯示資訊"), Some("win.info"));
    menu
}

/// The one colour beyond paper and ink, for what is not answering or running hot; dark
/// text on it in the dark, where it is lighter.
fn css(dark: bool) -> String {
    let (alert, on_alert) = if dark {
        ("#ff7a45", "#1c1c1e")
    } else {
        ("#d9480f", "#ffffff")
    };
    format!(
        "
.tm-meters {{ padding: 4px 12px 12px; }}
.tm-meter {{ padding: 8px 10px; border-radius: 6px; border: 1px solid var(--border-color); background: var(--card-bg-color); }}
.tm-meter:checked {{ background: var(--accent-bg-color); color: var(--accent-fg-color); border-color: transparent; }}
.tm-meter-title {{ font-size: 12px; font-weight: 500; }}
.tm-meter-value {{ font-size: 22px; font-weight: 500; font-feature-settings: \"tnum\"; }}
.tm-meter.tm-hot {{ border-color: {alert}; }}
.tm-meter.tm-hot .tm-meter-value {{ color: {alert}; }}
.tm-columns {{ padding: 6px 12px; font-size: 12px; border-bottom: 1px solid var(--border-color); }}
.tm-sorted {{ font-weight: 600; }}
.tm-list {{ background: none; }}
.tm-list > row {{ padding: 0; border-radius: 0; }}
.tm-list > row:selected {{ background: var(--accent-bg-color); color: var(--accent-fg-color); }}
.tm-line {{ padding: 0 12px; min-height: 24px; }}
.tm-expander {{ min-width: 16px; min-height: 16px; padding: 0; }}
.tm-num {{ font-feature-settings: \"tnum\"; }}
.tm-section {{ padding: 10px 12px 4px 36px; font-size: 12px; font-weight: 600; opacity: 0.7; }}
.tm-tag {{ padding: 0 6px; border-radius: 4px; font-size: 12px; color: {alert}; border: 1px solid alpha({alert}, 0.4); background: alpha({alert}, 0.1); }}
.tm-force {{ min-height: 22px; padding: 0 8px; border-radius: 6px; background: {alert}; color: {on_alert}; }}
.tm-status {{ padding: 4px 12px; font-size: 12px; }}
"
    )
}

fn main() -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_startup(|_| {
        let provider = gtk::CssProvider::new();
        let styles = adw::StyleManager::default();
        provider.load_from_string(&css(styles.is_dark()));
        styles.connect_dark_notify(glib::clone!(
            #[strong]
            provider,
            move |styles| provider.load_from_string(&css(styles.is_dark()))
        ));
        if let Some(display) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
    });
    app.connect_activate(|app| match app.active_window() {
        Some(window) => window.present(),
        None => {
            Tasks::open(app);
        }
    });
    app.run()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proc(pid: i32, ppid: i32, uid: u32, ticks: u64, memory: Option<u64>) -> Proc {
        Proc {
            pid,
            ppid,
            uid,
            name: format!("p{pid}"),
            ticks,
            started: 7,
            threads: 1,
            memory,
            io: Some(ticks * 10),
        }
    }

    #[test]
    fn windows_are_read_from_the_compositor() {
        assert_eq!(
            parse_windows("41\tfirefox\t1\n52\tgedit\t0\nbad\n"),
            [
                TaskWindow {
                    pid: 41,
                    app: "firefox".into(),
                    responsive: true
                },
                TaskWindow {
                    pid: 52,
                    app: "gedit".into(),
                    responsive: false
                },
            ]
        );
    }

    #[test]
    fn processes_are_grouped_under_their_application() {
        let before: HashMap<i32, Proc> = [
            proc(41, 1, 1000, 0, Some(1)),
            proc(42, 41, 1000, 0, Some(1)),
            proc(60, 1, 1000, 0, Some(1)),
        ]
        .into_iter()
        .map(|proc| (proc.pid, proc))
        .collect();
        let now = [
            proc(41, 1, 1000, 30, Some(100)),
            proc(42, 41, 1000, 10, Some(50)),
            proc(52, 1, 1000, 0, Some(10)),
            proc(60, 1, 1000, 20, Some(5)),
            // A kernel thread, and someone else's process.
            proc(2, 0, 0, 0, None),
            proc(70, 1, 0, 0, Some(5)),
        ];
        let windows = parse_windows("41\tfirefox\t1\n52\tgedit\t0\n");
        let list = entries(&now, &before, 200, 1.0, &windows, 1000);
        let keys: Vec<&str> = list.iter().map(|entry| entry.key.as_str()).collect();
        assert_eq!(keys, ["app:firefox", "app:gedit", "pid:41", "pid:42", "pid:60"]);
        let firefox = &list[0];
        assert_eq!(
            (firefox.pids.len(), firefox.cpu, firefox.memory, firefox.disk),
            (2, 20.0, 150, 400.0)
        );
        assert!(!list[1].responsive);
        assert_eq!(list[2].parent.as_deref(), Some("app:firefox"));
        assert_eq!(list[4].cpu, 10.0);

        let order = ranks(&list, Resource::Memory);
        let mut sorted = keys.clone();
        sorted.sort_by_key(|key| order[*key]);
        assert_eq!(sorted, ["app:firefox", "pid:41", "pid:42", "app:gedit", "pid:60"]);
    }
}
