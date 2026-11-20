//! The drawer a row of the subagents card opens, as Nekominal's: on the right of the window under the bar, over the
//! panes without resizing them (a narrower pane would make the agent in it draw its whole screen again). It shows one
//! subagent (what it said last, what it was asked, the tools it called), a workflow (its phases, the agents of the one
//! picked), or an agent of a workflow. Esc or the arrow goes back a level, the cross closes it.

use std::{
    cell::{Cell, RefCell},
    path::{Path, PathBuf},
    rc::Rc,
    time::SystemTime,
};

use gtk::{gdk, gio, glib, prelude::*};

use super::{
    lang::{tr, trf},
    subagents::{self, Detail, State, Subagents},
    subagents_card::{self, Target},
};

const WIDTH: i32 = 440;
const PAD: i32 = 18;
/// The most agents of a phase the drawer lists, and lines of a prompt it shows.
const MOST_STEPS: usize = 8;
const PROMPT_LINES: i32 = 4;
const TITLE_LINES: i32 = 2;
const TOOL_WIDTH: i32 = 80;
const TITLE: &str = "子代理";

pub fn css() -> &'static str {
    r#"
.term-drawer { background-color: @view_bg_color; color: @view_fg_color; box-shadow: -1px 0 0 alpha(currentColor, 0.12), -6px 0 18px alpha(black, 0.12); }
.term-drawer-title { font-size: 15px; font-weight: bold; }
.term-drawer-box { padding: 10px; border-radius: 6px; background: alpha(currentColor, 0.05); }
.term-drawer-state.running { color: var(--accent-color); }
.term-drawer-state.failed { color: @error_color; }
.term-call-dot { min-width: 6px; min-height: 6px; border-radius: 3px; background: alpha(currentColor, 0.2); }
.term-call-dot.open { background: var(--accent-bg-color); }
"#
}

/// What the drawer last read of a transcript, kept while the file does not change.
struct Cached {
    file: PathBuf,
    stamp: Option<SystemTime>,
    detail: Detail,
}

pub struct Drawer {
    pub widget: gtk::Box,
    back_label: gtk::Label,
    body: gtk::Box,
    foot: gtk::Box,
    target: RefCell<Option<Target>>,
    /// The phase of a workflow picked by its name; else the last one with an agent running.
    phase: Cell<Option<usize>>,
    cached: RefCell<Option<Cached>>,
}

fn label(text: &str, classes: &[&str]) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .css_classes(classes.to_vec())
        .build()
}

/// Text in at most `lines` lines, wrapped.
fn wrapped(text: &str, lines: i32, classes: &[&str]) -> gtk::Label {
    let shown = label(text, classes);
    shown.set_wrap(true);
    shown.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    shown.set_lines(lines);
    shown
}

impl Drawer {
    pub fn new() -> Rc<Self> {
        let back_label = gtk::Label::builder().css_classes(["caption"]).build();
        let back_content = gtk::Box::builder().spacing(4).build();
        back_content.append(&gtk::Image::from_icon_name("go-previous-symbolic"));
        back_content.append(&back_label);
        let back = gtk::Button::builder().child(&back_content).css_classes(["flat"]).build();
        let close = gtk::Button::builder().icon_name("window-close-symbolic").css_classes(["flat", "circular"]).tooltip_text(tr("關閉")).build();
        let nav = gtk::Box::builder().margin_start(10).margin_end(10).margin_top(6).margin_bottom(6).build();
        nav.append(&back);
        nav.append(&gtk::Box::builder().hexpand(true).build());
        nav.append(&close);
        let body = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(4)
            .margin_start(PAD)
            .margin_end(PAD)
            .margin_top(16)
            .margin_bottom(PAD)
            .build();
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&body)
            .build();
        let foot = gtk::Box::builder().spacing(6).margin_start(14).margin_end(PAD).margin_top(10).margin_bottom(10).build();
        let widget = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .width_request(WIDTH)
            .halign(gtk::Align::End)
            .css_classes(["term-drawer"])
            .visible(false)
            .build();
        widget.append(&nav);
        widget.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        widget.append(&scroller);
        widget.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        widget.append(&foot);
        let this = Rc::new(Self {
            widget,
            back_label,
            body,
            foot,
            target: RefCell::default(),
            phase: Cell::default(),
            cached: RefCell::default(),
        });
        let weak = Rc::downgrade(&this);
        back.connect_clicked(glib::clone!(
            #[strong]
            weak,
            move |_| {
                if let Some(drawer) = weak.upgrade() {
                    drawer.back();
                }
            }
        ));
        close.connect_clicked(move |_| {
            if let Some(drawer) = weak.upgrade() {
                drawer.close();
            }
        });
        this
    }

    pub fn is_open(&self) -> bool {
        self.target.borrow().is_some()
    }

    pub fn open(&self, target: Target) {
        self.phase.set(None);
        self.target.replace(Some(target));
    }

    pub fn close(&self) {
        self.target.take();
        self.widget.set_visible(false);
    }

    /// One level up: from an agent of a workflow to the workflow, from anything else out of the drawer.
    pub fn back(&self) {
        let up = match self.target.take() {
            Some(Target::Step(run, _)) => Some(Target::Workflow(run)),
            _ => None,
        };
        match up {
            Some(target) => {
                self.target.replace(Some(target));
                self.widget.activate_action("win.refresh-drawer", None).unwrap_or_else(|error| {
                    eprintln!("nekoland-terminal: could not show the drawer: {error}");
                });
            }
            None => self.close(),
        }
    }

    fn pick_phase(&self, phase: usize) {
        self.phase.set(Some(phase));
        if let Err(error) = self.widget.activate_action("win.refresh-drawer", None) {
            eprintln!("nekoland-terminal: could not show the drawer: {error}");
        }
    }

    /// Lays the drawer out for the subagents of the session in front; it closes when what it showed is gone.
    pub fn show(self: &Rc<Self>, subagents: &Subagents, folder: Option<&Path>) {
        let Some(target) = self.target.borrow().clone() else {
            self.widget.set_visible(false);
            return;
        };
        let Some(folder) = folder else {
            self.close();
            return;
        };
        clear(&self.body);
        clear(&self.foot);
        let shown = match &target {
            Target::Agent(id) => self.show_agent(subagents, folder, id),
            Target::Workflow(run) => self.show_workflow(subagents, folder, run),
            Target::Step(run, id) => self.show_step(subagents, folder, run, id),
        };
        if !shown {
            self.close();
            return;
        }
        let up = match &target {
            Target::Step(run, _) => subagents
                .workflows
                .iter()
                .find(|workflow| &workflow.run == run)
                .map_or_else(|| run.clone(), |workflow| workflow.name.clone()),
            _ => tr(TITLE).to_owned(),
        };
        self.back_label.set_text(&up);
        let hint = label(tr("Esc 返回"), &["caption", "dim-label"]);
        hint.set_hexpand(true);
        hint.set_xalign(1.0);
        self.foot.append(&hint);
        self.widget.set_visible(true);
    }

    /// What a transcript says, read again only when it changed.
    fn detail_of(&self, file: PathBuf) -> Detail {
        let stamp = std::fs::metadata(&file).and_then(|meta| meta.modified()).ok();
        if let Some(cached) = self.cached.borrow().as_ref().filter(|cached| cached.file == file && cached.stamp == stamp) {
            return cached.detail.clone();
        }
        let detail = subagents::detail(&file);
        self.cached.replace(Some(Cached { file, stamp, detail: detail.clone() }));
        detail
    }

    fn show_agent(&self, subagents: &Subagents, folder: &Path, id: &str) -> bool {
        let Some(agent) = subagents.agents.iter().find(|agent| agent.id == id) else {
            return false;
        };
        let state = if agent.finished.is_some() { State::Done } else { State::Running };
        let time = subagents::duration_text(agent.started, agent.finished);
        self.body.append(&head(&agent.kind, state, &time, &agent.title));
        let file = subagents::transcript_of(folder, None, id);
        let detail = self.detail_of(file.clone());
        let reply = agent.reply.clone().or_else(|| detail.reply.clone().filter(|_| agent.finished.is_some()));
        self.show_detail(&detail, reply, tr("回覆"), file);
        true
    }

    fn show_step(&self, subagents: &Subagents, folder: &Path, run: &str, id: &str) -> bool {
        let found = subagents.workflows.iter().find(|workflow| workflow.run == run).and_then(|workflow| {
            workflow.phases.iter().find_map(|phase| Some((phase.title.clone(), phase.steps.iter().find(|step| step.id == id)?.clone())))
        });
        let Some((phase, step)) = found else {
            return false;
        };
        self.body.append(&head(&phase, step.state, "", &step.label));
        let file = subagents::transcript_of(folder, Some(run), id);
        let detail = self.detail_of(file.clone());
        let result = step.result.clone().or_else(|| detail.reply.clone().filter(|_| step.state != State::Running));
        self.show_detail(&detail, result, tr("結果"), file);
        true
    }

    /// What a subagent said last, what it was asked and the tools it called, and the foot's buttons for them.
    fn show_detail(&self, detail: &Detail, reply: Option<String>, reply_heading: &str, transcript: PathBuf) {
        if let Some(reply) = &reply {
            self.body.append(&section(reply_heading, reply, true));
            self.foot.append(&copy_button(reply));
        }
        if let Some(prompt) = &detail.prompt {
            self.body.append(&section(tr("任務"), prompt, false));
        }
        if detail.total_calls > 0 {
            self.body.append(&calls(detail));
        }
        self.foot.append(&open_button(tr("開啟紀錄"), transcript));
    }

    fn show_workflow(self: &Rc<Self>, subagents: &Subagents, folder: &Path, run: &str) -> bool {
        let Some(workflow) = subagents.workflows.iter().find(|workflow| workflow.run == run) else {
            return false;
        };
        let state = if workflow.is_running() { State::Running } else { State::Done };
        let time = subagents::duration_text(workflow.started, workflow.finished);
        self.body.append(&head("Workflow", state, &time, &workflow.name));
        if let Some(summary) = &workflow.summary {
            let summary = wrapped(summary, TITLE_LINES, &["caption", "dim-label"]);
            summary.set_margin_bottom(8);
            self.body.append(&summary);
        }
        let last = workflow.phases.len().saturating_sub(1);
        let running = workflow.phases.iter().rposition(|phase| phase.count(State::Running) > 0);
        let picked = self.phase.get().filter(|&phase| phase <= last).or(running).unwrap_or(last);
        for (index, phase) in workflow.phases.iter().enumerate() {
            let line = subagents_card::phase_line(phase, Some(run), true, index == picked);
            // The phase's name picks it: a button in its place.
            if let Some(name) = line.first_child() {
                line.remove(&name);
                let button = gtk::Button::builder().child(&name).css_classes(["flat", "term-phase-name"]).valign(gtk::Align::Start).build();
                let weak = Rc::downgrade(self);
                button.connect_clicked(move |_| {
                    if let Some(drawer) = weak.upgrade() {
                        drawer.pick_phase(index);
                    }
                });
                line.prepend(&button);
            }
            self.body.append(&line);
        }
        if let Some(phase) = workflow.phases.get(picked) {
            self.body.append(&phase_agents(phase, run));
        }
        self.foot.append(&open_button(tr("開啟日誌"), subagents::journal_of(folder, run)));
        true
    }
}

fn clear(container: &gtk::Box) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}

/// The head: its kind and state on one line, its title under them.
fn head(kind: &str, state: State, time: &str, title: &str) -> gtk::Box {
    let words = match state {
        State::Running => trf("執行中 {0}", &[&time]),
        State::Done if time.is_empty() => tr("已完成").to_owned(),
        State::Done => trf("已完成 · {0}", &[&time]),
        State::Failed => tr("失敗").to_owned(),
    };
    let line = gtk::Box::builder().spacing(12).build();
    line.append(&label(kind, &["caption", "dim-label"]));
    let state_line = gtk::Box::builder().spacing(6).css_classes(["term-drawer-state", state.css_class()]).build();
    let dot = gtk::Box::builder().css_classes(["term-agent-dot"]).valign(gtk::Align::Center).build();
    if state != State::Running {
        dot.add_css_class("done");
    }
    state_line.append(&dot);
    state_line.append(&label(&words, &["caption"]));
    if state == State::Done {
        state_line.add_css_class("dim-label");
    }
    line.append(&state_line);
    let head = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(4).margin_bottom(14).build();
    head.append(&line);
    head.append(&wrapped(title, TITLE_LINES, &["term-drawer-title"]));
    head
}

/// A section: its heading and `body` in a few lines, on a lighter ground when `boxed`.
fn section(heading: &str, body: &str, boxed: bool) -> gtk::Box {
    let flat = body.lines().map(str::trim).filter(|line| !line.is_empty()).collect::<Vec<_>>().join(" ");
    let section = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(6).margin_bottom(12).build();
    section.append(&label(heading, &["caption", "dim-label"]));
    let text = wrapped(&flat, PROMPT_LINES, &[]);
    if boxed {
        text.add_css_class("term-drawer-box");
    }
    section.append(&text);
    section
}

/// The last tool calls, the one still running lit, and how many came before them.
fn calls(detail: &Detail) -> gtk::Box {
    let list = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(4).build();
    list.append(&label(&trf("{0} 次工具呼叫", &[&detail.total_calls]), &["caption", "dim-label"]));
    for call in &detail.calls {
        let line = gtk::Box::builder().spacing(9).build();
        let dot = gtk::Box::builder().css_classes(["term-call-dot"]).valign(gtk::Align::Center).build();
        if call.open {
            dot.add_css_class("open");
        }
        line.append(&dot);
        let tool = label(&call.tool, &["caption", "dim-label"]);
        tool.set_width_request(TOOL_WIDTH);
        line.append(&tool);
        line.append(&label(&call.arg, &["caption"]));
        list.append(&line);
    }
    let earlier = detail.total_calls - detail.calls.len();
    if earlier > 0 {
        let more = label(&trf("更早的 {0} 次", &[&earlier]), &["caption", "dim-label"]);
        more.set_margin_start(15);
        list.append(&more);
    }
    list
}

/// The agents of a phase: running, failed, then done, each opening its own view.
fn phase_agents(phase: &subagents::Phase, run: &str) -> gtk::Box {
    let list = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(2).margin_top(8).build();
    let counts: Vec<String> = [(State::Running, "{0} 個執行中"), (State::Failed, "{0} 個失敗"), (State::Done, "{0} 個已完成")]
        .into_iter()
        .filter(|(state, _)| phase.count(*state) > 0)
        .map(|(state, words)| trf(words, &[&phase.count(state)]))
        .collect();
    list.append(&label(&format!("{} · {}", phase.title, counts.join(" · ")), &["caption", "dim-label"]));
    let mut steps = phase.steps.clone();
    steps.sort_by_key(|step| match step.state {
        State::Running => 0,
        State::Failed => 1,
        State::Done => 2,
    });
    let shown = steps.len().min(MOST_STEPS);
    for step in &steps[..shown] {
        let grid = gtk::Grid::builder().column_spacing(10).build();
        grid.attach(&subagents_card::step_square(step.state, false), 0, 0, 1, 1);
        let name = label(&step.label, &[]);
        name.set_hexpand(true);
        grid.attach(&name, 1, 0, 1, 1);
        if let Some(result) = &step.result {
            let result = label(result, &["caption"]);
            result.add_css_class(if step.state == State::Failed { "error" } else { "dim-label" });
            grid.attach(&result, 1, 1, 1, 1);
        }
        let button = gtk::Button::builder()
            .child(&grid)
            .css_classes(["flat", "term-subagent-row"])
            .action_name(subagents_card::OPEN_ACTION)
            .action_target(&Target::Step(run.to_owned(), step.id.clone()).encode().to_variant())
            .build();
        list.append(&button);
    }
    if steps.len() > shown {
        let more = label(&trf("還有 {0} 個", &[&(steps.len() - shown)]), &["caption", "dim-label"]);
        more.set_margin_start(18);
        list.append(&more);
    }
    list
}

fn copy_button(text: &str) -> gtk::Button {
    let button = gtk::Button::builder().label(tr("複製回覆")).css_classes(["caption"]).build();
    let text = text.to_owned();
    button.connect_clicked(move |button| {
        if let Some(display) = gdk::Display::default() {
            display.clipboard().set_text(&text);
        }
        button.grab_focus();
    });
    button
}

fn open_button(title: &str, file: PathBuf) -> gtk::Button {
    let button = gtk::Button::builder().label(title).css_classes(["caption"]).build();
    button.connect_clicked(move |button| {
        let window = button.root().and_downcast::<gtk::Window>();
        let shown = file.display().to_string();
        gtk::FileLauncher::new(Some(&gio::File::for_path(&file))).launch(window.as_ref(), gio::Cancellable::NONE, move |result| {
            if let Err(error) = result {
                eprintln!("nekoland-terminal: could not open {shown}: {error}");
            }
        });
    });
    button
}
