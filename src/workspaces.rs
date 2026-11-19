//! Several desktops to spread windows over, one shown at a time.
//!
//! As on Windows, the desktops themselves are kept from one session to the next: how many
//! there are, in what order, what each was named and the wallpaper it has of its own. The
//! windows on them are not.

use serde::{Deserialize, Serialize};
use smithay::{
    desktop::Window,
    utils::{Logical, Point},
};

use crate::{config, windows::is_managed, Nekoland};

const SAVED: &str = "desktops.toml";

/// What is kept of the desktops: one name for each, empty for one not named, and as
/// many wallpapers, empty for one that has the wallpaper of them all.
#[derive(Default, Serialize, Deserialize)]
struct Saved {
    names: Vec<String>,
    #[serde(default)]
    wallpapers: Vec<String>,
}

pub struct Workspaces {
    pub active: usize,
    /// The windows of each desktop that is not shown, bottom of the stack first, with
    /// where they go back to. The entry of the active desktop is empty: its windows are on screen.
    parked: Vec<Vec<(Window, Point<i32, Logical>)>>,
    /// Each desktop's own name, if it was given one.
    names: Vec<Option<String>>,
    /// Each desktop's own wallpaper, if it was given one, as Windows lets each have.
    wallpapers: Vec<Option<String>>,
    /// The windows shown on every desktop, which stay on screen whichever is shown.
    sticky: Vec<Window>,
}

impl Workspaces {
    /// The desktops kept from the last session, or one.
    pub fn new() -> Self {
        let saved: Saved = config::config_file(SAVED)
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|text| {
                toml::from_str(&text)
                    .inspect_err(|err| tracing::warn!("{SAVED}: {err}"))
                    .ok()
            })
            .unwrap_or_default();
        let mut names: Vec<Option<String>> = (saved.names.into_iter())
            .map(|name| Some(name).filter(|name| !name.is_empty()))
            .collect();
        if names.is_empty() {
            names.push(None);
        }
        let mut wallpapers: Vec<Option<String>> = (saved.wallpapers.into_iter())
            .map(|path| Some(path).filter(|path| !path.is_empty()))
            .collect();
        wallpapers.resize(names.len(), None);
        Self {
            active: 0,
            parked: vec![Vec::new(); names.len()],
            names,
            wallpapers,
            sticky: Vec::new(),
        }
    }

    pub fn count(&self) -> usize {
        self.parked.len()
    }

    /// What a desktop is called: its own name, or its place.
    pub fn name(&self, index: usize) -> String {
        self.names
            .get(index)
            .cloned()
            .flatten()
            .unwrap_or_else(|| format!("桌面 {}", index + 1))
    }

    /// The wallpaper a desktop has of its own, if it has one.
    pub fn wallpaper(&self, index: usize) -> Option<&str> {
        self.wallpapers.get(index)?.as_deref()
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

    pub fn is_sticky(&self, window: &Window) -> bool {
        self.sticky.contains(window)
    }

    pub fn forget(&mut self, window: &Window) {
        for windows in &mut self.parked {
            windows.retain(|(parked, _)| parked != window);
        }
        self.sticky.retain(|sticky| sticky != window);
    }

    /// Puts the desktop at `from` at `to`, the others moving up or down a place, the one
    /// shown staying shown.
    fn reorder(&mut self, from: usize, to: usize) {
        let windows = self.parked.remove(from);
        self.parked.insert(to, windows);
        let name = self.names.remove(from);
        self.names.insert(to, name);
        let wallpaper = self.wallpapers.remove(from);
        self.wallpapers.insert(to, wallpaper);
        self.active = moved_index(self.active, from, to);
    }

    fn save(&self) {
        let saved = Saved {
            names: (self.names.iter())
                .map(|name| name.clone().unwrap_or_default())
                .collect(),
            wallpapers: (self.wallpapers.iter())
                .map(|path| path.clone().unwrap_or_default())
                .collect(),
        };
        let Some(path) = config::config_file(SAVED) else {
            return;
        };
        let written = toml::to_string(&saved)
            .map_err(std::io::Error::other)
            .and_then(|text| {
                std::fs::create_dir_all(path.parent().unwrap_or(&path))?;
                std::fs::write(&path, text)
            });
        if let Err(err) = written {
            tracing::warn!("{}: {err}", path.display());
        }
    }
}

/// Where the desktop at `index` ends up when the one at `from` is put at `to`.
pub fn moved_index(index: usize, from: usize, to: usize) -> usize {
    if index == from {
        to
    } else if from < index && index <= to {
        index - 1
    } else if to <= index && index < from {
        index + 1
    } else {
        index
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

    /// The windows the Dock shows: those of every desktop, or, set to, only the ones on the
    /// desktop shown and minimized.
    pub fn dock_windows(&self) -> Vec<Window> {
        if self.config.dock_this_desktop {
            (self.space.elements())
                .chain(self.minimized.iter().map(|(window, _)| window))
                .cloned()
                .collect()
        } else {
            self.all_windows().cloned().collect()
        }
    }

    pub fn add_workspace(&mut self) {
        self.workspaces.parked.push(Vec::new());
        self.workspaces.names.push(None);
        self.workspaces.wallpapers.push(None);
        self.workspaces.save();
    }

    /// A new desktop at the end, shown, as Windows' Win+Ctrl+D makes one.
    pub fn add_and_show_workspace(&mut self) {
        self.add_workspace();
        self.switch_workspace(self.workspaces.count() - 1);
    }

    pub fn switch_workspace(&mut self, index: usize) {
        self.show_workspace(index, true);
    }

    /// Shows another desktop: sliding past the one shown, or, picked in the overview, at
    /// once, as the overview closing over it is the way there.
    pub fn show_workspace(&mut self, index: usize, sliding: bool) {
        if index >= self.workspaces.count() || index == self.workspaces.active {
            return;
        }

        // X11 menus belong to nothing; they go away with the desktop they were opened on.
        // The windows shown on every desktop stay where they are.
        let shown: Vec<(Window, Point<i32, Logical>)> = self
            .space
            .elements()
            .filter(|window| is_managed(window) && !self.workspaces.is_sticky(window))
            .filter_map(|window| Some((window.clone(), self.space.element_location(window)?)))
            .collect();
        let leaving: Vec<Window> = self
            .space
            .elements()
            .filter(|window| !self.workspaces.is_sticky(window))
            .cloned()
            .collect();
        leaving.iter().for_each(|window| self.space.unmap_elem(window));

        let wallpaper = self.desktop_wallpaper(self.workspaces.active);
        let previous = std::mem::replace(&mut self.workspaces.active, index);
        if self.desktop_wallpaper(index) != wallpaper {
            self.show_wallpaper();
        }
        if sliding {
            self.motion_slide(shown.clone(), index > previous);
        }
        self.workspaces.parked[previous] = shown;
        for (window, location) in std::mem::take(&mut self.workspaces.parked[index]) {
            self.space.map_element(window, location, false);
        }
        // Those on every desktop stay in front of the ones that came.
        for window in self.workspaces.sticky.clone() {
            self.space.raise_element(&window, false);
        }

        if self.overview.is_some() {
            self.rebuild_overview();
        } else {
            self.focus_top_window();
        }
    }

    /// Where the windows of a desktop are on it, bottom of the stack first: those shown on
    /// every desktop too.
    pub fn desktop_windows(&self, index: usize) -> Vec<(Window, Point<i32, Logical>)> {
        let on_screen = |keep: &dyn Fn(&Window) -> bool| -> Vec<(Window, Point<i32, Logical>)> {
            self.space
                .elements()
                .filter(|window| is_managed(window) && keep(window))
                .filter_map(|window| Some((window.clone(), self.space.element_location(window)?)))
                .collect()
        };
        if index == self.workspaces.active {
            return on_screen(&|_| true);
        }
        let mut windows = self.workspaces.parked.get(index).cloned().unwrap_or_default();
        windows.extend(on_screen(&|window| self.workspaces.is_sticky(window)));
        windows
    }

    /// Sends a window on screen to another desktop; one shown on every desktop is then
    /// on that one alone.
    pub fn move_to_workspace(&mut self, window: &Window, index: usize) {
        self.workspaces.sticky.retain(|sticky| sticky != window);
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

    /// Sends a window to a new desktop at the end.
    pub fn move_to_new_workspace(&mut self, window: &Window) {
        let index = self.workspaces.count();
        self.add_workspace();
        self.move_to_workspace(window, index);
    }

    /// Takes the focused window to the desktop to the left or right and goes there with it.
    pub fn carry_window_by(&mut self, window: &Window, step: isize) {
        let Some(index) = self
            .workspaces
            .active
            .checked_add_signed(step)
            .filter(|&index| index < self.workspaces.count())
        else {
            return;
        };
        self.move_to_workspace(window, index);
        self.switch_workspace(index);
        self.focus_window(window);
    }

    /// Shows a window on every desktop, or again on the one shown alone.
    pub fn toggle_sticky(&mut self, window: &Window) {
        if self.workspaces.is_sticky(window) {
            self.workspaces.sticky.retain(|sticky| sticky != window);
        } else if self.space.element_location(window).is_some() {
            self.workspaces.sticky.push(window.clone());
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
        self.workspaces.names.remove(index);
        self.workspaces.wallpapers.remove(index);
        if self.workspaces.active > index {
            self.workspaces.active -= 1;
        }
        self.workspaces.save();
        if self.overview.is_some() {
            self.rebuild_overview();
        }
    }

    /// Puts a desktop at another place along the row.
    pub fn move_workspace(&mut self, from: usize, to: usize) {
        let count = self.workspaces.count();
        if from >= count || to >= count || from == to {
            return;
        }
        self.workspaces.reorder(from, to);
        self.workspaces.save();
    }

    /// Names a desktop; with nothing but spaces, it goes back to being named by its place.
    pub fn rename_workspace(&mut self, index: usize, name: &str) {
        let Some(slot) = self.workspaces.names.get_mut(index) else {
            return;
        };
        let name = name.trim();
        *slot = (!name.is_empty()).then(|| name.to_string());
        self.workspaces.save();
    }

    /// The wallpaper a desktop shows: its own, or the one of them all.
    pub fn desktop_wallpaper(&self, index: usize) -> Option<String> {
        (self.workspaces.wallpaper(index).map(str::to_string)).or_else(|| self.config.wallpaper.clone())
    }

    /// Gives a desktop a wallpaper of its own, or, with none, the one of them all.
    pub fn set_workspace_wallpaper(&mut self, index: usize, path: Option<String>) {
        let shown = self.desktop_wallpaper(self.workspaces.active);
        let Some(slot) = self.workspaces.wallpapers.get_mut(index) else {
            return;
        };
        *slot = path;
        self.workspaces.save();
        if self.desktop_wallpaper(self.workspaces.active) != shown {
            self.show_wallpaper();
        }
    }

    /// Starts drawing the wallpaper of the desktop shown. The one before stays until the
    /// new one has drawn, so that nothing under it shows in between.
    pub fn show_wallpaper(&mut self) {
        self.stop_old_wallpaper();
        self.old_wallpaper = self.wallpaper.take();
        let path = self.desktop_wallpaper(self.workspaces.active);
        self.wallpaper = config::start_wallpaper(path.as_deref(), self.config.wallpaper_mode);
        if self.wallpaper.is_none() {
            self.stop_old_wallpaper();
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

#[cfg(test)]
mod tests {
    use super::moved_index;

    #[test]
    fn a_desktop_moved_takes_the_others_along() {
        // Moving the second to the fourth: the third and fourth move down a place.
        let moved: Vec<usize> = (0..5).map(|index| moved_index(index, 1, 3)).collect();
        assert_eq!(moved, [0, 3, 1, 2, 4]);
        // And back again.
        let moved: Vec<usize> = (0..5).map(|index| moved_index(index, 3, 1)).collect();
        assert_eq!(moved, [0, 2, 3, 1, 4]);
    }
}
