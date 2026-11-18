//! The badges of a tab: the pull request of the branch its focused pane is on (`github.rs`), and the TCP ports the
//! programs of its panes listen on (`ports.rs`). Read with the job tick, once a second, and drawn as small pills on a
//! line of the tab's row in the sidebar, and for the tab in front in the title bar; a click opens the pull request or the
//! port in the browser.

use std::{cell::RefCell, rc::Rc};

use gtk::prelude::*;

use super::{
    github::{self, PullRequests},
    page::Page,
    ports,
};

const PASSING_ICON: &str = "object-select-symbolic";
const FAILING_ICON: &str = "window-close-symbolic";
const PORT_ICON: &str = "web-browser-symbolic";
const BADGE_SPACING: u32 = 4;
const MARK_SIZE: i32 = 11;

/// How a badge looks: a pull request by its state and its checks, or a port.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Look {
    Passing,
    Failing,
    Running,
    /// Open without checks.
    Open,
    Draft,
    Merged,
    Closed,
    Port,
}

impl Look {
    fn css_class(self) -> &'static str {
        match self {
            Look::Passing | Look::Open => "term-badge-passing",
            Look::Failing | Look::Closed => "term-badge-failing",
            Look::Running => "term-badge-running",
            Look::Draft => "term-badge-draft",
            Look::Merged => "term-badge-merged",
            Look::Port => "term-badge-port",
        }
    }

    /// The mark before the text: an icon, or a ring drawn by the style (dashed while checks run).
    fn mark(self) -> gtk::Widget {
        let icon = match self {
            Look::Passing => PASSING_ICON,
            Look::Failing => FAILING_ICON,
            Look::Port => PORT_ICON,
            Look::Running | Look::Open | Look::Draft | Look::Closed | Look::Merged => {
                let ring = gtk::Box::builder().valign(gtk::Align::Center).css_classes(["term-badge-ring"]).build();
                if self == Look::Running {
                    ring.add_css_class("term-badge-ring-running");
                } else if self == Look::Merged {
                    ring.add_css_class("term-badge-ring-filled");
                }
                return ring.upcast();
            }
        };
        gtk::Image::builder().icon_name(icon).pixel_size(MARK_SIZE).build().upcast()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chip {
    pub look: Look,
    pub text: String,
    /// The failing checks, in a bubble after the number.
    pub count: Option<u32>,
    /// What a click opens.
    pub url: String,
}

/// The badges of a pull request and of ports: the pull request first, then each port.
pub fn chips(pull_request: Option<&github::PullRequest>, listening: &[u16]) -> Vec<Chip> {
    let pull_request = pull_request.map(|pull_request| {
        let checks = pull_request.checks;
        let look = match pull_request.state {
            github::State::Merged => Look::Merged,
            github::State::Closed => Look::Closed,
            github::State::Draft => Look::Draft,
            github::State::Open if checks.failing > 0 => Look::Failing,
            github::State::Open if checks.pending > 0 => Look::Running,
            github::State::Open if checks.passing > 0 => Look::Passing,
            github::State::Open => Look::Open,
        };
        Chip {
            look,
            text: format!("#{}", pull_request.number),
            count: (look == Look::Failing).then_some(checks.failing),
            url: pull_request.url.clone(),
        }
    });
    let ports = listening
        .iter()
        .map(|&port| Chip { look: Look::Port, text: port.to_string(), count: None, url: ports::url(port) });
    pull_request.into_iter().chain(ports).collect()
}

/// The badges of a tab as a line of text: `#4 merged · :3000 :5173`.
pub fn text(pull_request: Option<&github::PullRequest>, listening: &[u16]) -> Option<String> {
    let ports = (!listening.is_empty())
        .then(|| listening.iter().map(|port| format!(":{port}")).collect::<Vec<_>>().join(" "));
    let parts: Vec<String> = [pull_request.map(github::PullRequest::summary), ports].into_iter().flatten().collect();
    (!parts.is_empty()).then(|| parts.join(" · "))
}

fn open_url(widget: &gtk::Widget, url: &str) {
    let window = widget.root().and_downcast::<gtk::Window>();
    gtk::UriLauncher::new(url).launch(window.as_ref(), gtk::gio::Cancellable::NONE, |result| {
        if let Err(error) = result {
            eprintln!("nekoland-terminal: cannot open a badge's link: {error}");
        }
    });
}

/// One badge: a pill of its colour with its mark, its text and the bubble of failing checks.
fn pill(chip: &Chip) -> gtk::Button {
    let content = gtk::Box::builder().spacing(3).build();
    content.append(&chip.look.mark());
    content.append(&gtk::Label::new(Some(&chip.text)));
    if let Some(count) = chip.count {
        content.append(&gtk::Label::builder().label(count.to_string()).css_classes(["term-badge-count"]).build());
    }
    let button = gtk::Button::builder()
        .child(&content)
        .tooltip_text(&chip.url)
        .valign(gtk::Align::Center)
        .css_classes(["flat", "term-badge-pill", chip.look.css_class()])
        .build();
    let url = chip.url.clone();
    button.connect_clicked(move |button| open_url(button.upcast_ref(), &url));
    button
}

/// The badges on a row of the sidebar, wrapping to more lines when they do not fit; none without any.
pub fn row_line(chips: &[Chip]) -> Option<gtk::FlowBox> {
    if chips.is_empty() {
        return None;
    }
    let line = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .column_spacing(BADGE_SPACING)
        .row_spacing(BADGE_SPACING)
        .max_children_per_line(u32::MAX)
        .homogeneous(false)
        .css_classes(["term-badges"])
        .build();
    for chip in chips {
        line.append(&pill(chip));
    }
    Some(line)
}

/// The badges in the title bar, refilled for the tab in front.
pub fn fill_bar(bar: &gtk::Box, chips: &[Chip]) {
    while let Some(child) = bar.first_child() {
        bar.remove(&child);
    }
    for chip in chips {
        bar.append(&pill(chip));
    }
    bar.set_visible(!chips.is_empty());
}

pub fn css() -> &'static str {
    r#"
.term-badges { min-height: 0; }
.term-badges > flowboxchild { padding: 0; min-height: 0; }
button.term-badge-pill { min-height: 16px; padding: 0 5px 0 3px; border-radius: 999px; font-size: 10.5px; font-weight: normal; background: alpha(currentColor, 0.14); }
button.term-badge-pill:hover { background: alpha(currentColor, 0.25); }
.term-badge-passing { color: @success_color; }
.term-badge-failing { color: @error_color; }
.term-badge-running { color: @warning_color; }
.term-badge-draft { color: alpha(@window_fg_color, 0.6); }
.term-badge-merged { color: @purple_2; }
.term-badge-port { color: @blue_2; }
.term-badge-ring { min-width: 6px; min-height: 6px; margin: 0 2px; border: 1.2px solid currentColor; border-radius: 999px; }
.term-badge-ring-running { border-style: dashed; }
.term-badge-ring-filled { background: currentColor; }
.term-badge-count { padding: 0 4px; margin-left: 2px; border-radius: 999px; background: alpha(currentColor, 0.3); }
"#
}

thread_local! {
    static PULL_REQUESTS: RefCell<PullRequests> = RefCell::default();
}

/// Reads the badges of every tab again: the ports their panes listen on and the pull request of their branch. True
/// when one changed.
pub fn refresh(pages: &[Rc<Page>]) -> bool {
    let snapshot = ports::snapshot();
    PULL_REQUESTS.with_borrow_mut(|pull_requests| {
        pull_requests.collect();
        let mut keys = Vec::new();
        let mut changed = false;
        for page in pages {
            let listening = snapshot.as_ref().map(|snapshot| snapshot.ports_of(&page.shell_pids())).unwrap_or_default();
            let key = page.repository().and_then(|repository| Some((repository.root, repository.branch?)));
            let pull_request = key.as_ref().and_then(|key| pull_requests.get(key));
            changed |= page.set_badges(pull_request, listening);
            keys.extend(key);
        }
        pull_requests.retain(&keys);
        changed
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pull_request(state: &str, draft: bool, rollup: &str) -> github::PullRequest {
        github::parse(&format!(
            r#"{{"number":142,"state":"{state}","isDraft":{draft},"url":"https://github.com/o/r/pull/142","statusCheckRollup":[{rollup}]}}"#
        ))
        .unwrap()
    }

    #[test]
    fn every_state_and_mix_of_checks_has_its_look() {
        let (pass, fail, run) = (r#"{"state":"SUCCESS"}"#, r#"{"state":"FAILURE"}"#, r#"{"state":"PENDING"}"#);
        for mask in 0..8 {
            let rollup = [(1, pass), (2, fail), (4, run)]
                .iter()
                .filter(|(bit, _)| mask & bit != 0)
                .map(|(_, check)| *check)
                .collect::<Vec<_>>()
                .join(",");
            let open = match mask {
                0 => Look::Open,
                mask if mask & 2 != 0 => Look::Failing,
                mask if mask & 4 != 0 => Look::Running,
                _ => Look::Passing,
            };
            for (state, draft, look) in [
                ("OPEN", false, open),
                ("OPEN", true, Look::Draft),
                ("MERGED", false, Look::Merged),
                ("CLOSED", false, Look::Closed),
            ] {
                let chip = chips(Some(&pull_request(state, draft, &rollup)), &[]).remove(0);
                assert_eq!(chip.look, look, "{state} draft={draft} [{rollup}]");
                assert_eq!(chip.count.is_some(), look == Look::Failing, "only a failing badge counts");
            }
        }
    }

    #[test]
    fn the_pull_request_comes_first_and_the_ports_follow_it() {
        let failing = chips(Some(&pull_request("OPEN", false, r#"{"state":"FAILURE"},{"state":"ERROR"}"#)), &[8080]);
        assert_eq!((failing[0].text.as_str(), failing[0].count), ("#142", Some(2)));
        assert_eq!(failing[1], Chip { look: Look::Port, text: "8080".into(), count: None, url: ports::url(8080) });
        assert!(chips(None, &[]).is_empty());
    }

    #[test]
    fn the_text_says_the_pull_request_then_the_ports() {
        assert_eq!(text(None, &[]), None);
        assert_eq!(text(None, &[3000, 5173]).as_deref(), Some(":3000 :5173"));
        let merged = pull_request("MERGED", false, "");
        assert_eq!(text(Some(&merged), &[3000]).as_deref(), Some("#142 merged · :3000"));
    }
}
