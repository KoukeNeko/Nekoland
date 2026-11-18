//! What the automation API (`api.rs`) asks of the windows, answered on the main thread: the windows, tabs and panes,
//! and the commands on a pane, found in whichever window has it. And the agent states for the subscribers, from the poll.

use std::{cell::RefCell, collections::HashMap, path::PathBuf, rc::Rc};

use gtk::prelude::*;
use vte4::prelude::*;
use serde_json::{json, Value};

use super::{
    super::{
        agent::Status,
        api::{self, Direction, Request, Topic},
        page::Page,
        pane::Pane,
        profiles,
        push::Happening,
    },
    all_windows, focus_pane, front_window, notify_message, palette_items, Window,
};

thread_local! {
    /// The agent state each pane was last seen in, by its id, while someone follows `agent-state`.
    static STATES: RefCell<HashMap<String, &'static str>> = RefCell::new(HashMap::new());
}

const NO_WINDOW: &str = "the terminal has no window";
const LINE_BREAKS: [char; 2] = ['\r', '\n'];
const ENTER: &str = "\r";

/// An agent state by the name Nekominal's events use; none without an agent.
fn state_name(pane: &Pane) -> &'static str {
    if pane.agent_name().is_none() {
        return "none";
    }
    match pane.status() {
        Status::Idle => "idle",
        Status::Working => "working",
        Status::Waiting => "blocked",
        Status::Done => "done",
        Status::Failed => "error",
    }
}

/// The last `lines` lines of `text`.
fn tail(text: &[String], lines: usize) -> String {
    text[text.len().saturating_sub(lines)..].join("\n")
}

/// The screen's lines without the blank ones under the last that has text.
fn screen_text(pane: &Pane) -> Vec<String> {
    let mut lines = palette_items::screen_lines(pane.screen.upcast_ref());
    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    lines
}

fn describe_pane(pane: &Pane, focused: bool) -> Value {
    json!({
        "id": pane.id(),
        "title": pane.label(),
        "cwd": pane.directory(),
        "job": pane.running_program(),
        "profile": pane.profile(),
        "agent": pane.agent_name(),
        "agentState": state_name(pane),
        "focused": focused,
    })
}

/// Where the pane with this id is: its window and its tab.
fn find_pane(id: &str) -> Option<(Rc<Window>, Rc<Page>, Rc<Pane>)> {
    all_windows().into_iter().find_map(|window| {
        let found = window.pages().into_iter().find_map(|(_, page)| {
            let pane = page.panes().into_iter().find(|pane| pane.id() == id)?;
            Some((page, pane))
        });
        found.map(|(page, pane)| (window, page, pane))
    })
}

fn profile_exists(wanted: &str) -> bool {
    profiles::all().iter().any(|profile| profile.name == wanted)
}

/// Answers a request from the windows of the process.
pub fn answer(request: Request) -> api::Answer {
    match request {
        Request::List => Ok(json!({
            "pid": std::process::id(),
            "windows": all_windows().iter().map(|window| window.describe()).collect::<Vec<_>>(),
        })),
        Request::NewTab { profile, cwd, command } => {
            front_window().ok_or(NO_WINDOW)?.api_new_tab(profile.as_deref(), cwd.map(PathBuf::from), &command)
        }
        Request::Notify { title, body, pane: None } => {
            let page = front_window().and_then(|window| window.selected_page()).ok_or("the window has no tab")?;
            notify_message(&page, &title, &body, Happening::Message);
            Ok(json!({ "shown": true }))
        }
        Request::Subscribe { .. } => Err("subscribe is answered by the socket".to_string()),
        Request::Notify { pane: Some(ref pane), .. }
        | Request::SendText { ref pane, .. }
        | Request::GetText { ref pane, .. }
        | Request::Split { ref pane, .. }
        | Request::Focus { ref pane }
        | Request::Close { ref pane }
        | Request::SetTitle { ref pane, .. } => {
            let found = find_pane(pane).ok_or_else(|| format!("no pane {pane}"))?;
            on_pane(request, found)
        }
    }
}

/// A request about one pane.
fn on_pane(request: Request, (window, page, pane): (Rc<Window>, Rc<Page>, Rc<Pane>)) -> api::Answer {
    match request {
        Request::SendText { text, .. } => {
            // A bracketed paste takes its line breaks as text, so `ls\r` would only be typed: the break at its end is
            // pressed as Enter after it, for a script to run what it sends.
            let body = text.trim_end_matches(LINE_BREAKS);
            let enter = body.len() < text.len();
            let sent = body.is_empty() || pane.paste_checked(body.to_string());
            if !sent {
                window.show(&page, &pane);
            } else if enter {
                pane.send(ENTER);
            }
            Ok(json!({ "sent": sent, "asked": !sent }))
        }
        Request::GetText { lines, .. } => {
            let lines = lines.unwrap_or_else(|| usize::try_from(pane.screen.row_count()).unwrap_or_default());
            Ok(json!({ "text": tail(&screen_text(&pane), lines) }))
        }
        Request::Split { direction, cwd, command, .. } => {
            let orientation = match direction {
                Direction::Right => gtk::Orientation::Horizontal,
                Direction::Down => gtk::Orientation::Vertical,
            };
            let cwd = cwd.map(PathBuf::from);
            let new = page.split_pane(&pane, orientation, cwd.as_deref(), &command).map_err(|error| error.to_string())?;
            page.changed();
            Ok(json!({ "pane": new.id() }))
        }
        Request::Focus { .. } => {
            window.show(&page, &pane);
            Ok(json!({}))
        }
        Request::Close { .. } => {
            pane.hang_up();
            Ok(json!({ "asked": false }))
        }
        Request::SetTitle { title, .. } => {
            page.custom_title.replace((!title.is_empty()).then_some(title));
            page.changed();
            Ok(json!({}))
        }
        Request::Notify { title, body, .. } => {
            notify_message(&page, &title, &body, Happening::Message);
            Ok(json!({ "shown": true }))
        }
        Request::List | Request::NewTab { .. } | Request::Subscribe { .. } => Err("not a request about a pane".to_string()),
    }
}

/// From the poll: the agents whose state changed, while someone follows. A pane first seen is not news.
pub fn publish_agent_states(pages: &[Rc<Page>]) {
    if !api::wants(Topic::AgentState) {
        STATES.with_borrow_mut(HashMap::clear);
        return;
    }
    let mut seen = HashMap::new();
    for pane in pages.iter().flat_map(|page| page.panes()) {
        let state = state_name(&pane);
        let before = STATES.with_borrow(|states| states.get(pane.id()).copied());
        if before.is_some_and(|before| before != state) {
            api::publish(Topic::AgentState, pane.id(), json!({ "state": state }));
        }
        seen.insert(pane.id().to_string(), state);
    }
    // Closed panes leave with this.
    STATES.with_borrow_mut(|states| *states = seen);
}

impl Window {
    /// The window, its tabs and their panes, for `list`.
    fn describe(&self) -> Value {
        let selected = self.tab_view.selected_page();
        let tabs: Vec<Value> = self
            .pages()
            .into_iter()
            .map(|(tab, page)| {
                let is_selected = Some(&tab) == selected.as_ref();
                let focused = page.focused_pane();
                let panes: Vec<Value> = page
                    .panes()
                    .iter()
                    .map(|pane| {
                        let is_focused = is_selected && focused.as_ref().is_some_and(|focused| Rc::ptr_eq(focused, pane));
                        describe_pane(pane, is_focused)
                    })
                    .collect();
                json!({ "id": page.id, "title": page.title(), "selected": is_selected, "panes": panes })
            })
            .collect();
        json!({ "active": self.window.is_active(), "tabs": tabs })
    }

    /// Brings up the pane's tab and the window, and focuses the pane.
    fn show(&self, page: &Page, pane: &Pane) {
        if let Some(tab) = page.tab.borrow().as_ref() {
            self.tab_view.set_selected_page(tab);
        }
        self.window.present();
        pane.screen.grab_focus();
    }

    /// A tab, as the command line opens one: a profile by name, a folder, a program.
    fn api_new_tab(&self, profile: Option<&str>, cwd: Option<PathBuf>, command: &[String]) -> api::Answer {
        if let Some(wanted) = profile.filter(|wanted| !profile_exists(wanted)) {
            return Err(format!("no profile {wanted}"));
        }
        let page = Page::new(cwd.as_deref(), profile, command).map_err(|error| error.to_string())?;
        let page = self.add_page(page, None);
        focus_pane(&page);
        let pane = page.focused_pane().map(|pane| pane.id().to_string());
        Ok(json!({ "tab": page.id, "pane": pane }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_text_gives_the_last_lines_asked_for_or_all_there_are() {
        let text: Vec<String> = ["a", "b", "c", "d"].map(str::to_string).to_vec();
        assert_eq!(tail(&text, 2), "c\nd");
        assert_eq!(tail(&text[..2], 10), "a\nb");
        assert_eq!(tail(&[], 3), "");
    }
}
