//! Several desktops to spread windows over, one shown at a time.

use smithay::{
    desktop::Window,
    utils::{Logical, Point},
};

use crate::{windows::is_managed, Nekoland};

#[derive(Default)]
pub struct Workspaces {
    pub active: usize,
    /// The windows of each desktop that is not shown, bottom of the stack first, with
    /// where they go back to. The entry of the active desktop is empty: its windows are on screen.
    parked: Vec<Vec<(Window, Point<i32, Logical>)>>,
}

impl Workspaces {
    pub fn new() -> Self {
        Self {
            active: 0,
            parked: vec![Vec::new()],
        }
    }

    pub fn count(&self) -> usize {
        self.parked.len()
    }

    /// Every window that is on a desktop other than the one shown.
    pub fn parked_windows(&self) -> impl Iterator<Item = &Window> {
        self.parked.iter().flatten().map(|(window, _)| window)
    }

    pub fn index_of(&self, window: &Window) -> Option<usize> {
        self.parked
            .iter()
            .position(|windows| windows.iter().any(|(parked, _)| parked == window))
    }

    pub fn forget(&mut self, window: &Window) {
        for windows in &mut self.parked {
            windows.retain(|(parked, _)| parked != window);
        }
    }
}

impl Nekoland {
    /// Every window there is: on screen, minimized, or on another desktop.
    pub fn all_windows(&self) -> impl Iterator<Item = &Window> {
        self.space
            .elements()
            .chain(self.minimized.iter().map(|(window, _)| window))
            .chain(self.workspaces.parked_windows())
    }

    pub fn add_workspace(&mut self) {
        self.workspaces.parked.push(Vec::new());
    }

    pub fn switch_workspace(&mut self, index: usize) {
        if index >= self.workspaces.count() || index == self.workspaces.active {
            return;
        }

        // X11 menus belong to nothing; they go away with the desktop they were opened on.
        let shown: Vec<(Window, Point<i32, Logical>)> = self
            .space
            .elements()
            .filter(|window| is_managed(window))
            .filter_map(|window| Some((window.clone(), self.space.element_location(window)?)))
            .collect();
        let all: Vec<Window> = self.space.elements().cloned().collect();
        all.iter().for_each(|window| self.space.unmap_elem(window));

        let previous = std::mem::replace(&mut self.workspaces.active, index);
        self.workspaces.parked[previous] = shown;
        for (window, location) in std::mem::take(&mut self.workspaces.parked[index]) {
            self.space.map_element(window, location, false);
        }

        if self.overview.is_some() {
            self.rebuild_overview();
        } else {
            self.focus_top_window();
        }
    }

    /// Where the windows of a desktop are on it, bottom of the stack first.
    pub fn desktop_windows(&self, index: usize) -> Vec<(Window, Point<i32, Logical>)> {
        if index == self.workspaces.active {
            return self
                .space
                .elements()
                .filter(|window| is_managed(window))
                .filter_map(|window| Some((window.clone(), self.space.element_location(window)?)))
                .collect();
        }
        self.workspaces.parked.get(index).cloned().unwrap_or_default()
    }

    /// Sends a window on screen to another desktop.
    pub fn move_to_workspace(&mut self, window: &Window, index: usize) {
        if index >= self.workspaces.count() || index == self.workspaces.active {
            return;
        }
        let Some(location) = self.space.element_location(window) else {
            return;
        };
        self.space.unmap_elem(window);
        self.workspaces.parked[index].push((window.clone(), location));
        // In the overview nothing has the keyboard, and giving it would leave the overview.
        if self.overview.is_none() {
            self.focus_top_window();
        }
    }

    /// Takes a desktop away; its windows go to the one before it, or the next for the first.
    pub fn remove_workspace(&mut self, index: usize) {
        let count = self.workspaces.count();
        if count <= 1 || index >= count {
            return;
        }
        let into = if index == 0 { 1 } else { index - 1 };
        if index == self.workspaces.active {
            self.switch_workspace(into);
        }
        let windows = std::mem::take(&mut self.workspaces.parked[index]);
        if into == self.workspaces.active {
            for (window, location) in windows {
                self.space.map_element(window, location, false);
            }
        } else {
            self.workspaces.parked[into].extend(windows);
        }
        self.workspaces.parked.remove(index);
        if self.workspaces.active > index {
            self.workspaces.active -= 1;
        }
        if self.overview.is_some() {
            self.rebuild_overview();
        }
    }

    /// The desktop to the left or to the right of the one shown, if there is one.
    pub fn switch_workspace_by(&mut self, step: isize) {
        if let Some(index) = self.workspaces.active.checked_add_signed(step) {
            self.switch_workspace(index);
        }
    }

    /// Shows a window wherever it is: on another desktop, or minimized.
    pub fn reveal_window(&mut self, window: &Window) {
        if let Some(index) = self.workspaces.index_of(window) {
            self.switch_workspace(index);
        }
        self.unminimize(window);
        self.focus_window(window);
    }
}
