//! The subagents of the session in front, as Nekominal's bar shows them: a piece right of the context while one runs
//! ("3 個執行中 · 12 個已完成"), and its card with every workflow (its phases as rows of dots, one an agent) and every
//! subagent (its kind, title and time; the last thing it said once it ended). A row opens the drawer.

use gtk::{glib, prelude::*};

use super::lang::{tr, trf};
use super::subagents::{self, Phase, State, Subagents};

const WIDTH: i32 = 380;
const PAD: i32 = 16;
const DOT_GAP: i32 = 3;
/// How many dots of a phase go on a line before it wraps.
const DOTS_PER_LINE: usize = 22;
/// The column of a phase's name and of its count.
const PHASE_NAME: i32 = 58;
const PHASE_COUNT: i32 = 44;
/// The column of a subagent's kind, so that the titles after it line up.
const KIND_WIDTH: i32 = 104;
/// The most subagents the card lists; the others are counted.
const MOST_AGENTS: usize = 8;
/// The action a row runs to open the drawer, with what it opens as its target.
pub const OPEN_ACTION: &str = "win.subagent";

pub fn css() -> &'static str {
    r#"
.term-step { min-width: 8px; min-height: 8px; border-radius: 2px; background: alpha(currentColor, 0.18); }
.term-step.big { min-width: 10px; min-height: 10px; border-radius: 3px; }
.term-step.running { background: @accent_bg_color; }
.term-step.failed { background: @error_color; }
.term-step-button { padding: 0; min-width: 0; min-height: 0; border-radius: 3px; }
.term-agent-dot { min-width: 8px; min-height: 8px; border-radius: 4px; background: @accent_bg_color; }
.term-agent-dot.done { background: none; box-shadow: inset 0 0 0 1.5px alpha(currentColor, 0.3); }
.term-subagents-mark { color: @accent_color; }
.term-subagents-mark .term-step { background: @accent_bg_color; }
.term-subagent-row { padding: 3px 6px; min-height: 0; border-radius: 6px; }
.term-subagent-row label { font-weight: normal; }
.term-phase-name { padding: 0 2px; min-height: 0; margin-left: -2px; }
"#
}

/// What a row of the card or the drawer opens: a subagent, a workflow, or an agent of a workflow (its run and id).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Agent(String),
    Workflow(String),
    Step(String, String),
}

const AGENT: &str = "agent";
const WORKFLOW: &str = "workflow";
const STEP: &str = "step";
const SEPARATOR: char = '\u{1f}';

impl Target {
    /// As the target of [`OPEN_ACTION`].
    pub fn encode(&self) -> String {
        match self {
            Self::Agent(id) => format!("{AGENT}{SEPARATOR}{id}"),
            Self::Workflow(run) => format!("{WORKFLOW}{SEPARATOR}{run}"),
            Self::Step(run, id) => format!("{STEP}{SEPARATOR}{run}{SEPARATOR}{id}"),
        }
    }

    pub fn decode(text: &str) -> Option<Self> {
        let mut parts = text.split(SEPARATOR);
        let target = match (parts.next()?, parts.next()?, parts.next()) {
            (AGENT, id, None) => Self::Agent(id.to_owned()),
            (WORKFLOW, run, None) => Self::Workflow(run.to_owned()),
            (STEP, run, Some(id)) => Self::Step(run.to_owned(), id.to_owned()),
            _ => return None,
        };
        Some(target)
    }
}

/// "3 個執行中 · 12 個已完成", or the running alone when nothing ended yet.
pub fn counts(subagents: &Subagents) -> String {
    let running = trf("{0} 個執行中", &[&subagents.running()]);
    match subagents.done() {
        0 => running,
        done => format!("{running} · {}", trf("{0} 個已完成", &[&done])),
    }
}

/// A square in the colour of a step's state.
pub fn step_square(state: State, big: bool) -> gtk::Box {
    let square = gtk::Box::builder().css_classes(["term-step", state.css_class()]).valign(gtk::Align::Center).build();
    if big {
        square.add_css_class("big");
    }
    square
}

/// The mark after a tab's name while its agent runs subagents: a square and their number, in the accent's colour.
pub fn mark(running: usize) -> gtk::Box {
    let mark = gtk::Box::builder().spacing(4).css_classes(["term-subagents-mark", "caption"]).valign(gtk::Align::Center).build();
    mark.append(&step_square(State::Running, false));
    mark.append(&gtk::Label::builder().label(running.to_string()).css_classes(["numeric"]).build());
    mark
}

fn label(text: &str, classes: &[&str]) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .css_classes(classes.to_vec())
        .build()
}

/// The dots of a phase, [DOTS_PER_LINE] a line; each opens its agent when `run` is given.
pub fn phase_dots(phase: &Phase, run: Option<&str>, big: bool) -> gtk::Grid {
    let dots = gtk::Grid::builder()
        .column_spacing(DOT_GAP)
        .row_spacing(DOT_GAP)
        .hexpand(true)
        .halign(gtk::Align::Start)
        .valign(gtk::Align::Start)
        .margin_top(DOT_GAP)
        .build();
    for (index, step) in phase.steps.iter().enumerate() {
        let square = step_square(step.state, big);
        let cell: gtk::Widget = match run {
            Some(run) => gtk::Button::builder()
                .child(&square)
                .css_classes(["flat", "term-step-button"])
                .tooltip_text(&step.label)
                .action_name(OPEN_ACTION)
                .action_target(&Target::Step(run.to_owned(), step.id.clone()).encode().to_variant())
                .build()
                .upcast(),
            None => square.upcast(),
        };
        let (column, row) = (index % DOTS_PER_LINE, index / DOTS_PER_LINE);
        dots.attach(&cell, column as i32, row as i32, 1, 1);
    }
    dots
}

/// A phase as a line: its name, its dots, how many of them finished.
pub fn phase_line(phase: &Phase, run: Option<&str>, big: bool, picked: bool) -> gtk::Box {
    let line = gtk::Box::builder().spacing(0).build();
    let name = label(&phase.title, &["caption"]);
    if !picked {
        name.add_css_class("dim-label");
    }
    name.set_width_request(PHASE_NAME);
    name.set_valign(gtk::Align::Start);
    line.append(&name);
    line.append(&phase_dots(phase, run, big));
    let count = gtk::Label::builder()
        .label(format!("{}/{}", phase.finished(), phase.steps.len()))
        .width_request(PHASE_COUNT)
        .xalign(1.0)
        .valign(gtk::Align::Start)
        .css_classes(["caption", "numeric"])
        .build();
    if phase.count(State::Running) == 0 {
        count.add_css_class("dim-label");
    }
    line.append(&count);
    line
}

/// A row that opens `target` in the drawer.
fn opener(child: &impl IsA<gtk::Widget>, target: &Target) -> gtk::Button {
    gtk::Button::builder()
        .child(child)
        .css_classes(["flat", "term-subagent-row"])
        .action_name(OPEN_ACTION)
        .action_target(&target.encode().to_variant())
        .build()
}

/// The card under the bar's piece.
pub fn card(subagents: &Subagents) -> gtk::Box {
    let card = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .width_request(WIDTH)
        .margin_start(PAD / 2)
        .margin_end(PAD / 2)
        .margin_top(PAD / 2)
        .margin_bottom(PAD / 2)
        .build();
    let head = gtk::Box::builder().spacing(8).build();
    head.append(&label(tr("子代理"), &["heading"]));
    head.append(&label(&counts(subagents), &["caption", "dim-label"]));
    card.append(&head);
    for workflow in &subagents.workflows {
        card.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        let content = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(6).build();
        let title = gtk::Box::builder().spacing(8).build();
        title.append(&label(&workflow.name, &["heading"]));
        let time = subagents::duration_text(workflow.started, workflow.finished);
        title.append(&label(&format!("Workflow · {time}"), &["caption", "dim-label"]));
        content.append(&title);
        if let Some(summary) = &workflow.summary {
            content.append(&label(summary, &["caption", "dim-label"]));
        }
        for phase in &workflow.phases {
            content.append(&phase_line(phase, None, false, false));
        }
        card.append(&opener(&content, &Target::Workflow(workflow.run.clone())));
    }
    let listed = subagents.agents.len().min(MOST_AGENTS);
    if listed > 0 {
        card.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    }
    for agent in &subagents.agents[..listed] {
        card.append(&opener(&agent_row(agent), &Target::Agent(agent.id.clone())));
    }
    if subagents.agents.len() > listed {
        card.append(&label(&trf("還有 {0} 個", &[&(subagents.agents.len() - listed)]), &["caption", "dim-label"]));
    }
    card
}

fn agent_row(agent: &subagents::Agent) -> gtk::Grid {
    let grid = gtk::Grid::builder().column_spacing(10).build();
    let dot = gtk::Box::builder().css_classes(["term-agent-dot"]).valign(gtk::Align::Center).build();
    if agent.finished.is_some() {
        dot.add_css_class("done");
    }
    grid.attach(&dot, 0, 0, 1, 1);
    let kind = label(&agent.kind, &["caption", "dim-label"]);
    kind.set_width_request(KIND_WIDTH);
    grid.attach(&kind, 1, 0, 1, 1);
    let title = label(&agent.title, &[]);
    title.set_hexpand(true);
    if agent.finished.is_some() {
        title.add_css_class("dim-label");
    }
    grid.attach(&title, 2, 0, 1, 1);
    let time = label(&subagents::duration_text(agent.started, agent.finished), &["caption", "dim-label", "numeric"]);
    time.set_xalign(1.0);
    grid.attach(&time, 3, 0, 1, 1);
    if let Some(reply) = &agent.reply {
        grid.attach(&label(reply, &["caption", "dim-label"]), 2, 1, 2, 1);
    }
    grid
}

/// The piece of the bar and its card, shown while a subagent runs.
pub struct Piece {
    pub button: gtk::MenuButton,
    label: gtk::Label,
    subagents: std::rc::Rc<std::cell::RefCell<Subagents>>,
}

impl Piece {
    pub fn new() -> Self {
        let label = gtk::Label::builder().css_classes(["numeric"]).build();
        let content = gtk::Box::builder().spacing(6).build();
        content.append(&gtk::Image::from_icon_name("view-paged-symbolic"));
        content.append(&label);
        let subagents: std::rc::Rc<std::cell::RefCell<Subagents>> = std::rc::Rc::default();
        let popover = gtk::Popover::new();
        popover.connect_show(glib::clone!(
            #[strong]
            subagents,
            move |popover| popover.set_child(Some(&card(&subagents.borrow())))
        ));
        let button = gtk::MenuButton::builder()
            .child(&content)
            .popover(&popover)
            .always_show_arrow(false)
            .css_classes(["flat", "term-context-button"])
            .valign(gtk::Align::Center)
            .visible(false)
            .build();
        Self { button, label, subagents }
    }

    pub fn show(&self, subagents: Option<Subagents>) {
        let shown = subagents.filter(|subagents| subagents.running() > 0);
        self.button.set_visible(shown.is_some());
        if let Some(subagents) = shown {
            self.label.set_text(&counts(&subagents));
            self.subagents.replace(subagents);
        }
    }

    /// Closes the card, as a row of it opens the drawer.
    pub fn close(&self) {
        self.button.popdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_target_comes_back_as_it_went() {
        for target in [Target::Agent("a1".into()), Target::Workflow("wf_1".into()), Target::Step("wf_1".into(), "w2".into())] {
            assert_eq!(Target::decode(&target.encode()), Some(target));
        }
        assert_eq!(Target::decode("nothing"), None);
    }
}
