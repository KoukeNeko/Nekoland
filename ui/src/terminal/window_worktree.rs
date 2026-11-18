//! Worktrees from the command palette, as Nekominal's: a new tab in a new worktree on a branch typed in the palette, and
//! "Remove this worktree", which shows what would be lost first, closes the tabs in the folder, then removes it.

use std::{
    path::{Path, PathBuf},
    rc::Rc,
};

use super::{
    super::{
        lang::{tr, trf},
        palette::{Item, Source},
        remove_dialog::{self, Facts, Primary},
        worktree::{self, Error, Refusal},
    },
    Tab, Window,
};

const NOT_REPO: &str = "這個窗格的資料夾不在 git 儲存庫裡";

fn refusal_text(refusal: &Refusal) -> String {
    match refusal {
        Refusal::Main => tr("這是儲存庫的主要簽出，不會被移除").to_string(),
        Refusal::Detached => tr("工作樹不在任何分支上：移除它會遺失它的提交").to_string(),
        Refusal::Dirty { changed, untracked } => {
            trf("工作樹有 {0} 個未提交的變更與 {1} 個未追蹤的檔案：請先提交或捨棄它們", &[changed, untracked])
        }
    }
}

fn error_text(error: &Error) -> String {
    match error {
        Error::NotRepo => tr(NOT_REPO).to_string(),
        Error::Git(said) => said.clone(),
        Error::Refused(refusal) => refusal_text(refusal),
    }
}

/// The tabs of the window with a pane in `root`.
fn tabs_in(tabs: Vec<Tab>, root: &Path) -> Vec<Tab> {
    tabs.into_iter()
        .filter(|(_, page)| {
            page.panes().iter().any(|pane| pane.directory().is_some_and(|folder| worktree::contains(root, &folder)))
        })
        .collect()
}

impl Window {
    pub(super) fn toast(&self, text: &str) {
        self.toasts.add_toast(adw::Toast::new(text));
    }

    fn focused_folder(&self) -> Option<PathBuf> {
        self.focused_pane()?.directory()
    }

    /// Asks the new branch's name in the palette, then makes its worktree and opens a tab in it.
    pub(super) fn ask_worktree_branch(self: &Rc<Self>) {
        let Some(folder) = self.focused_folder().filter(|folder| worktree::repository_root(folder).is_ok()) else {
            self.toast(tr(NOT_REPO));
            return;
        };
        let weak = Rc::downgrade(self);
        let find = Rc::new(move |text: &str| {
            let branch = text.trim().to_string();
            if branch.is_empty() {
                return Vec::new();
            }
            if !worktree::valid_branch(&branch) {
                return vec![Item::new(trf("「{0}」不能當分支名稱", &[&branch]), "", || {})];
            }
            let (weak, folder) = (weak.clone(), folder.clone());
            let label = trf("建立分支「{0}」與它的工作樹", &[&branch]);
            vec![Item::new(label, "", move || {
                if let Some(this) = weak.upgrade() {
                    this.open_worktree(&folder, &branch);
                }
            })]
        });
        self.palette.open(Source { placeholder: tr("輸入新分支的名稱"), empty: "", counted: false, find });
    }

    /// Makes the worktree and opens a tab in it like the one in front, its setup script typed for the user to run.
    fn open_worktree(self: &Rc<Self>, folder: &Path, branch: &str) {
        let (path, copy_failed) = match worktree::create(folder, branch) {
            Ok(made) => made,
            Err(error) => {
                self.toast(&trf("無法建立工作樹：{0}", &[&error_text(&error)]));
                return;
            }
        };
        let profile = self.selected_page().and_then(|page| page.profile());
        let after = self.tab_view.selected_page();
        let page = self.open_tab(Some(path.clone()), profile.as_deref(), after.as_ref());
        let pane = page.and_then(|page| page.focused_pane());
        if let (Some(pane), Some(command)) = (pane, worktree::setup_command(&path)) {
            pane.send(&command);
        }
        if let Some(error) = copy_failed {
            self.toast(&trf("工作樹已建立，但 .worktreeinclude 列出的檔案沒有複製過去：{0}", &[&error]));
        }
    }

    /// Shows what removing the worktree of the focused pane would lose, and removes it when asked.
    pub(super) fn ask_remove_worktree(self: &Rc<Self>) {
        match self.focused_folder() {
            Some(folder) => self.ask_remove_worktree_at(&folder),
            None => self.toast(tr(NOT_REPO)),
        }
    }

    fn ask_remove_worktree_at(self: &Rc<Self>, folder: &Path) {
        let status = match worktree::inspect(folder) {
            Ok(status) => status,
            Err(error) => {
                self.toast(&error_text(&error));
                return;
            }
        };
        let dialog = match remove_dialog::model(&status, &self.worktree_facts(&status.path)) {
            Ok(dialog) => dialog,
            Err(refusal) => {
                self.toast(&refusal_text(&refusal));
                return;
            }
        };
        let weak = Rc::downgrade(self);
        let root = status.path;
        remove_dialog::present(&self.window, &dialog, move |primary, delete_branch| {
            let Some(this) = weak.upgrade() else {
                return;
            };
            match primary {
                Primary::ShowTab => this.show_tab_in(&root),
                Primary::Remove => this.remove_worktree(&root, delete_branch),
            }
        });
    }

    /// What the window knows of the folder: the pull request of the tab in front, the tabs in it, an agent there.
    fn worktree_facts(&self, root: &Path) -> Facts {
        let tabs = tabs_in(self.pages(), root);
        let agent = tabs.iter().flat_map(|(_, page)| page.panes()).find_map(|pane| pane.agent_name());
        let pull_request = self.selected_page().and_then(|page| page.pull_request_number());
        Facts { pull_request, tabs: tabs.len(), agent }
    }

    /// Brings up the tab in the folder: the one in front when it is there, else the first.
    fn show_tab_in(&self, root: &Path) {
        let tabs = tabs_in(self.pages(), root);
        let selected = self.tab_view.selected_page();
        let tab = tabs.iter().find(|(tab, _)| Some(tab) == selected.as_ref()).or_else(|| tabs.first());
        if let Some((tab, _)) = tab {
            self.tab_view.set_selected_page(tab);
        }
    }

    /// Inspects the worktree again, as it may have changed while the dialog was open: the dialog comes back when it can
    /// no longer go. Else its tabs close and it goes.
    fn remove_worktree(self: &Rc<Self>, root: &Path, delete_branch: bool) {
        let status = worktree::inspect(root);
        let still_removable = status.as_ref().ok().and_then(|status| {
            remove_dialog::model(status, &self.worktree_facts(root)).ok().filter(|dialog| dialog.primary == Primary::Remove)
        });
        let (Ok(status), Some(_)) = (status, still_removable) else {
            self.ask_remove_worktree_at(root);
            return;
        };
        self.quiet_close.set(true);
        for (tab, _) in tabs_in(self.pages(), root) {
            self.tab_view.close_page(&tab);
        }
        self.quiet_close.set(false);
        match worktree::remove(&status, delete_branch) {
            Ok(None) => {}
            Ok(Some(said)) => {
                let branch = status.branch.unwrap_or_default();
                self.toast(&trf("工作樹已移除；分支「{0}」保留：{1}", &[&branch, &said]));
            }
            Err(error) => self.toast(&trf("無法移除工作樹：{0}", &[&error_text(&error)])),
        }
    }
}
