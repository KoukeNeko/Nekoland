//! Update and restart, as Nekominal's: every window, tab and split is carried over to the copy of the program installed
//! now, and the programs in the panes keep running (see `carry.rs`).

use std::{io, rc::Rc};

use gtk::prelude::*;
use vte4::prelude::*;

use super::{
    super::{
        carry::{self, CarriedTab, CarriedWindow},
        lang::trf,
        page::{colour_named, Page},
        pane::Pane,
        screen_rows,
    },
    all_pages, all_windows, drop_down, front_window, open_first, palette_items, Window,
};

/// How many lines of a pane's scrollback go over with it.
const CARRIED_LINES: usize = 2000;

/// What a pane shows, its scrollback with it, down to the cursor: what is written next goes where it would have.
fn pane_text(pane: &Pane) -> String {
    let mut lines = palette_items::screen_lines(pane.screen.upcast_ref());
    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    let (first_row, _) = screen_rows::kept(pane.screen.upcast_ref());
    let (column, row) = pane.screen.cursor_position();
    let cursor_line = usize::try_from(row - first_row).unwrap_or_default();
    // Below the text, as after a command's output: the blank lines down to it were trimmed with the others.
    if cursor_line >= lines.len() {
        lines.resize(cursor_line + 1, String::new());
        // So were the spaces before it, as after a prompt.
        if let Some(last) = lines.last_mut() {
            let length = last.chars().count();
            let column = usize::try_from(column).unwrap_or_default();
            last.extend(std::iter::repeat_n(' ', column.saturating_sub(length)));
        }
    }
    lines.drain(..lines.len().saturating_sub(CARRIED_LINES));
    lines.join("\n")
}

/// Becomes the copy of the program installed now, with every pane; only on a failure does it come back, and says so.
pub fn update_and_restart() {
    let carried: io::Result<Vec<CarriedWindow>> = all_windows().iter().map(|window| window.carry()).collect();
    let error = match carried {
        Ok(carried) => carry::restart(&carried),
        Err(error) => error,
    };
    for page in all_pages() {
        page.panes().iter().for_each(|pane| pane.keep());
    }
    eprintln!("nekoland-terminal: could not restart: {error}");
    if let Some(window) = front_window() {
        window.toast(&trf("無法重新啟動：{0}", &[&error]));
    }
}

/// Opens the windows the copy of the program before carried over, or the first window when none had a tab.
pub fn open_carried(app: &adw::Application, windows: &[CarriedWindow]) {
    let mut opened = false;
    for carried in windows.iter().filter(|carried| !carried.tabs.is_empty()) {
        let window = Window::new(app);
        window.drop_down.set(carried.drop_down);
        carried.tabs.iter().for_each(|tab| window.open_carried_tab(tab));
        if window.tab_view.n_pages() == 0 {
            window.open_tab(None, None, None);
        }
        if carried.visible {
            window.present_carried();
        }
        opened = true;
    }
    if !opened {
        open_first(app);
    }
}

impl Window {
    fn carry(&self) -> io::Result<CarriedWindow> {
        let selected = self.tab_view.selected_page();
        let tabs = self
            .pages()
            .into_iter()
            .map(|(tab, page)| {
                Ok(CarriedTab {
                    title: page.custom_title.borrow().clone(),
                    colour: page.colour.get().map(str::to_string),
                    selected: Some(&tab) == selected.as_ref(),
                    layout: page.carry(&pane_text)?,
                })
            })
            .collect::<io::Result<Vec<_>>>()?;
        Ok(CarriedWindow { tabs, drop_down: self.drop_down.get(), visible: self.window.is_visible() })
    }

    fn open_carried_tab(&self, carried: &CarriedTab) {
        let page = match Page::adopted(&carried.layout) {
            Ok(page) => self.add_page(page, None),
            Err(error) => {
                eprintln!("nekoland-terminal: a tab could not be taken over: {error}");
                return;
            }
        };
        page.custom_title.replace(carried.title.clone());
        page.colour.set(carried.colour.as_deref().and_then(colour_named));
        page.refresh_tab();
        if carried.selected {
            if let Some(tab) = page.tab.borrow().as_ref() {
                self.tab_view.set_selected_page(tab);
            }
        }
    }

    fn present_carried(self: &Rc<Self>) {
        if self.drop_down.get() {
            drop_down::show(self);
        } else {
            self.window.present();
        }
    }
}
