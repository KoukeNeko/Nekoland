//! The dialog of "Remove this worktree", as Nekominal's: what was checked (changes, pushed commits, merged branch, the
//! tabs and the agent in the folder), the files changed, the choice between deleting the branch with the folder and
//! keeping it, and Cancel beside the main button. A worktree with changes or commits not pushed offers neither choice:
//! the main button shows the tab in the folder instead.

use std::{cell::Cell, rc::Rc};

use adw::prelude::*;

use super::{
    lang::{tr, trf},
    worktree::{Refusal, Status},
};

pub const DELETE_BRANCH: usize = 0;
pub const ARCHIVE: usize = 1;
/// The changed files the dialog names at most.
const FILES: usize = 5;
const CANCEL_RESPONSE: &str = "cancel";
const PRIMARY_RESPONSE: &str = "primary";
const OK_ICON: &str = "object-select-symbolic";
const WARN_ICON: &str = "dialog-warning-symbolic";
const NOTE_ICON: &str = "dialog-information-symbolic";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    Ok,
    Warn,
    /// Something to know that is neither: the tabs that close, the agent that runs.
    Note,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    pub mark: Mark,
    pub text: String,
    /// The changed files under the line of changes.
    pub files: Vec<(char, String)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    pub label: String,
    /// What the choice does, or why it cannot be chosen.
    pub note: String,
    pub enabled: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Primary {
    Remove,
    /// Shows the tab in the folder, to commit or push there.
    ShowTab,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dialog {
    pub title: String,
    pub path: String,
    pub checks: Vec<Check>,
    pub choices: [Choice; 2],
    /// The choice taken; none when neither can be.
    pub chosen: Option<usize>,
    pub primary: Primary,
}

/// What the window knows of the folder besides git.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    pub pull_request: Option<u64>,
    pub tabs: usize,
    pub agent: Option<String>,
}

fn line(mark: Mark, text: String) -> Check {
    Check { mark, text, files: Vec::new() }
}

fn changes_check(status: &Status) -> Check {
    if status.dirty() {
        let files = status.files.iter().take(FILES).cloned().collect();
        Check { mark: Mark::Warn, text: trf("{0} 個檔案有未提交的變更", &[&(status.changed + status.untracked)]), files }
    } else {
        line(Mark::Ok, tr("沒有未提交的變更").to_string())
    }
}

fn push_check(status: &Status) -> Option<Check> {
    match (&status.remote, status.unpushed) {
        (None, _) => Some(line(Mark::Note, tr("這個儲存庫沒有遠端").to_string())),
        (Some(_), Some(count)) if count > 0 => Some(line(Mark::Warn, trf("{0} 個提交還沒推送", &[&count]))),
        (Some(remote), Some(_)) => Some(line(Mark::Ok, trf("所有提交都已推送到 {0}", &[remote]))),
        (Some(_), None) => None,
    }
}

fn merge_check(status: &Status, pull_request: Option<u64>) -> Option<Check> {
    match (&status.default_branch, status.merged, pull_request) {
        (Some(default), Some(true), Some(number)) => {
            Some(line(Mark::Ok, trf("已合併到 {0}（PR #{1}）", &[default, &number])))
        }
        (Some(default), Some(true), None) => Some(line(Mark::Ok, trf("已合併到 {0}", &[default]))),
        (Some(default), Some(false), _) => Some(line(Mark::Note, trf("還沒合併到 {0}", &[default]))),
        _ => None,
    }
}

fn delete_note(dirty: bool, unpushed: bool, merged: bool, ignored: bool) -> String {
    let text = match (dirty, unpushed) {
        (true, true) => "要先提交並推送",
        (true, false) => "要先提交",
        (false, true) => "要先推送",
        (false, false) if merged && ignored => "分支已合併，但被忽略的檔案會刪除",
        (false, false) if merged => "分支已合併，不會遺失任何東西",
        (false, false) => "git 只刪除已合併的分支",
    };
    tr(text).to_string()
}

/// The dialog for the worktree `status` describes; the main checkout and a worktree on no branch are refused before it.
pub fn model(status: &Status, facts: &Facts) -> Result<Dialog, Refusal> {
    if status.main {
        return Err(Refusal::Main);
    }
    let Some(branch) = status.branch.as_deref() else {
        return Err(Refusal::Detached);
    };
    let dirty = status.dirty();
    // With no remote there is nowhere to push to: nothing waits on it.
    let unpushed = status.remote.is_some() && status.unpushed.is_some_and(|count| count > 0);
    let merged = status.merged == Some(true);
    let mut checks: Vec<Check> = [Some(changes_check(status)), push_check(status), merge_check(status, facts.pull_request)]
        .into_iter()
        .flatten()
        .collect();
    if status.ignored > 0 {
        checks.push(line(Mark::Warn, trf("{0} 個被 git 忽略的檔案或資料夾（例如 .env、建置結果）會隨資料夾刪除", &[&status.ignored])));
    }
    if facts.tabs > 0 {
        checks.push(line(Mark::Note, trf("{0} 個分頁在這個資料夾，會一起關閉", &[&facts.tabs])));
    }
    if let Some(agent) = &facts.agent {
        checks.push(line(Mark::Note, trf("{0} 正在這個資料夾執行", &[agent])));
    }
    let blocked = dirty || unpushed;
    let archive_note = if dirty {
        tr("未提交的變更會遺失").to_string()
    } else if unpushed {
        tr("要先推送").to_string()
    } else {
        trf("保留分支 {0}，之後可以再建立工作樹", &[&branch])
    };
    Ok(Dialog {
        title: trf("移除工作樹 {0}？", &[&branch]),
        path: status.path.display().to_string(),
        checks,
        choices: [
            Choice {
                label: tr("移除資料夾並刪除分支").to_string(),
                note: delete_note(dirty, unpushed, merged, status.ignored > 0),
                enabled: !blocked,
            },
            Choice { label: tr("只移除資料夾").to_string(), note: archive_note, enabled: !blocked },
        ],
        chosen: (!blocked).then_some(if merged { DELETE_BRANCH } else { ARCHIVE }),
        primary: if blocked { Primary::ShowTab } else { Primary::Remove },
    })
}

fn check_row(check: &Check) -> gtk::Box {
    let (icon, class) = match check.mark {
        Mark::Ok => (OK_ICON, "success"),
        Mark::Warn => (WARN_ICON, "warning"),
        Mark::Note => (NOTE_ICON, "dim-label"),
    };
    let row = gtk::Box::builder().spacing(10).build();
    row.append(&gtk::Image::builder().icon_name(icon).valign(gtk::Align::Start).css_classes([class]).build());
    let text = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(4).build();
    text.append(&gtk::Label::builder().label(&check.text).xalign(0.0).wrap(true).build());
    for (letter, path) in &check.files {
        text.append(
            &gtk::Label::builder()
                .label(format!("{letter}  {path}"))
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::Middle)
                .css_classes(["caption", "monospace", "dim-label"])
                .build(),
        );
    }
    row.append(&text);
    row
}

/// The two choices as a list of radio rows; `chosen` follows the one taken.
fn choice_list(dialog: &Dialog, chosen: &Rc<Cell<Option<usize>>>) -> gtk::ListBox {
    let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).css_classes(["boxed-list"]).build();
    let mut group: Option<gtk::CheckButton> = None;
    for (index, choice) in dialog.choices.iter().enumerate() {
        let radio = gtk::CheckButton::builder()
            .active(dialog.chosen == Some(index))
            .sensitive(choice.enabled)
            .valign(gtk::Align::Center)
            .build();
        radio.set_group(group.as_ref());
        group.get_or_insert_with(|| radio.clone());
        let chosen = chosen.clone();
        radio.connect_toggled(move |radio| {
            if radio.is_active() {
                chosen.set(Some(index));
            }
        });
        let row = adw::ActionRow::builder()
            .title(&choice.label)
            .subtitle(&choice.note)
            .activatable_widget(&radio)
            .sensitive(choice.enabled)
            .build();
        row.add_prefix(&radio);
        list.append(&row);
    }
    list
}

/// Shows `dialog` over `parent`; `decided` is told the main button's work and whether to delete the branch, and is not
/// told of a cancel.
pub fn present(parent: &impl IsA<gtk::Widget>, dialog: &Dialog, decided: impl Fn(Primary, bool) + 'static) {
    let content = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(12).build();
    for check in &dialog.checks {
        content.append(&check_row(check));
    }
    let chosen = Rc::new(Cell::new(dialog.chosen));
    content.append(&choice_list(dialog, &chosen));
    let alert = adw::AlertDialog::builder()
        .heading(&dialog.title)
        .body(&dialog.path)
        .extra_child(&content)
        .build();
    let (primary_label, appearance) = match dialog.primary {
        Primary::Remove => (tr("移除"), adw::ResponseAppearance::Destructive),
        Primary::ShowTab => (tr("開啟分頁查看"), adw::ResponseAppearance::Suggested),
    };
    alert.add_responses(&[(CANCEL_RESPONSE, tr("取消")), (PRIMARY_RESPONSE, primary_label)]);
    alert.set_response_appearance(PRIMARY_RESPONSE, appearance);
    alert.set_default_response(Some(PRIMARY_RESPONSE));
    alert.set_close_response(CANCEL_RESPONSE);
    let primary = dialog.primary;
    alert.connect_response(None, move |_, response| {
        if response == PRIMARY_RESPONSE {
            decided(primary, chosen.get() == Some(DELETE_BRANCH));
        }
    });
    alert.present(Some(parent));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn status() -> Status {
        Status {
            path: PathBuf::from("/src/app-fix"),
            branch: Some("fix".into()),
            main: false,
            changed: 0,
            untracked: 0,
            ignored: 0,
            files: Vec::new(),
            remote: Some("origin".into()),
            unpushed: Some(0),
            default_branch: Some("origin/main".into()),
            merged: Some(false),
        }
    }

    #[test]
    fn a_clean_merged_worktree_offers_to_delete_the_branch_first() {
        let merged = Status { merged: Some(true), ..status() };
        let dialog = model(&merged, &Facts { pull_request: Some(12), ..Facts::default() }).unwrap();
        assert_eq!((dialog.chosen, dialog.primary), (Some(DELETE_BRANCH), Primary::Remove));
        assert!(dialog.checks.iter().any(|check| check.text.contains("PR #12")));
    }

    #[test]
    fn a_clean_worktree_not_merged_keeps_its_branch_unless_asked() {
        let dialog = model(&status(), &Facts::default()).unwrap();
        assert_eq!(dialog.chosen, Some(ARCHIVE));
        assert!(dialog.choices.iter().all(|choice| choice.enabled));
    }

    #[test]
    fn changes_or_unpushed_commits_disable_both_choices_and_show_the_tab() {
        let files: Vec<(char, String)> = (0..7).map(|index| ('M', format!("f{index}"))).collect();
        let dirty = Status { changed: 7, files, unpushed: Some(2), ..status() };
        let dialog = model(&dirty, &Facts::default()).unwrap();
        assert_eq!((dialog.chosen, dialog.primary), (None, Primary::ShowTab));
        assert!(dialog.choices.iter().all(|choice| !choice.enabled));
        assert_eq!(dialog.checks[0].files.len(), FILES, "five files are named, the count says all");
        let no_remote = Status { remote: None, unpushed: Some(4), ..status() };
        assert_eq!(model(&no_remote, &Facts::default()).unwrap().primary, Primary::Remove, "nowhere to push to");
    }

    #[test]
    fn the_main_checkout_and_a_detached_head_are_refused_before_the_dialog() {
        assert_eq!(model(&Status { main: true, ..status() }, &Facts::default()), Err(Refusal::Main));
        assert_eq!(model(&Status { branch: None, ..status() }, &Facts::default()), Err(Refusal::Detached));
    }
}
