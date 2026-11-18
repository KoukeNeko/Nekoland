//! One tab: its panes, split side by side or one above the other, the one in focus, and
//! what the tab shows of them: its name, its colour, and the state of the pane that needs
//! the user most.

use std::{
    cell::{Cell, RefCell},
    io,
    path::Path,
    rc::{Rc, Weak},
    sync::atomic::{AtomicU64, Ordering},
};

use gtk::{gdk, glib, prelude::*};

use super::{
    agent::Status,
    badges::{self, Chip},
    carry::{CarriedPane, Layout},
    github::PullRequest,
    jobs::Repository,
    pane::{Pane, PaneEvent, PaneOptions},
    shell_integration,
    usage::Usage,
    window,
};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// The tab colours, Nekoland's tag colours: their names in actions, and in the menu.
pub const TAB_COLOURS: [(&str, &str, &str); 7] = [
    ("red", "紅", "#d4493f"),
    ("orange", "橘", "#dc8436"),
    ("yellow", "黃", "#cfa92e"),
    ("green", "綠", "#4f9a61"),
    ("blue", "藍", "#4a7fc0"),
    ("purple", "紫", "#8e63b5"),
    ("gray", "灰", "#8e8c90"),
];

/// The tab colour of this name, as actions and saved tabs give it.
pub fn colour_named(name: &str) -> Option<&'static str> {
    TAB_COLOURS.iter().find(|(id, _, _)| *id == name).map(|(id, _, _)| *id)
}

/// A dot in a tab is drawn this many pixels across, for a crisp edge at twice the scale.
const DOT_TEXTURE_SIZE: usize = 32;
const DOT_RADIUS: f64 = 8.0;
const BYTES_PER_PIXEL: usize = 4;

pub struct Page {
    pub id: u64,
    /// The tab's child: one pane, or a split of them.
    pub root: gtk::Box,
    panes: RefCell<Vec<Rc<Pane>>>,
    focused: RefCell<Weak<Pane>>,
    pub custom_title: RefCell<Option<String>>,
    pub colour: Cell<Option<&'static str>>,
    pub tab: RefCell<Option<adw::TabPage>>,
    /// The pull request of the focused pane's branch, and the ports its panes listen on.
    pull_request: RefCell<Option<PullRequest>>,
    ports: RefCell<Vec<u16>>,
}

impl Page {
    /// A tab of one pane running `command`, or the profile's shell when it is empty.
    pub fn new(directory: Option<&Path>, profile: Option<&str>, command: &[String]) -> io::Result<Rc<Self>> {
        let page = Self::empty();
        let pane = page.spawn_pane(directory, profile, command)?;
        page.root.append(&pane.widget);
        Ok(page)
    }

    /// A tab of the panes the copy of this program before carried over, split as they were.
    pub fn adopted(layout: &Layout) -> io::Result<Rc<Self>> {
        let page = Self::empty();
        let widget = page.adopt_layout(layout)?;
        page.root.append(&widget);
        page.mark_focus();
        Ok(page)
    }

    fn empty() -> Rc<Self> {
        Rc::new(Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            root: gtk::Box::builder().css_classes(["term-page"]).build(),
            panes: RefCell::default(),
            focused: RefCell::default(),
            custom_title: RefCell::default(),
            colour: Cell::default(),
            tab: RefCell::default(),
            pull_request: RefCell::default(),
            ports: RefCell::default(),
        })
    }

    fn adopt_layout(self: &Rc<Self>, layout: &Layout) -> io::Result<gtk::Widget> {
        match layout {
            Layout::Pane(carried) => Ok(self.adopt_pane(carried)?.widget.clone().upcast()),
            Layout::Split { vertical, position, start, end } => {
                let orientation = if *vertical { gtk::Orientation::Vertical } else { gtk::Orientation::Horizontal };
                let split = new_split(orientation);
                split.set_start_child(Some(&self.adopt_layout(start)?));
                split.set_end_child(Some(&self.adopt_layout(end)?));
                split.set_position(*position);
                Ok(split.upcast())
            }
        }
    }

    /// The carried pane, or a new shell where it was when its pty could not be taken over.
    fn adopt_pane(self: &Rc<Self>, carried: &CarriedPane) -> io::Result<Rc<Pane>> {
        match Pane::adopt(carried, self.event_handler()) {
            Ok(pane) => {
                self.add_pane(&pane);
                Ok(pane)
            }
            Err(error) => {
                eprintln!("nekoland-terminal: pane {} could not be taken over, a new shell runs: {error}", carried.id);
                self.spawn_pane(carried.directory.as_deref(), Some(&carried.profile), &[])
            }
        }
    }

    /// Its splits and panes, for the copy of this program that takes over; `text_of` gives what a pane shows.
    pub fn carry(&self, text_of: &impl Fn(&Pane) -> String) -> io::Result<Layout> {
        let top = self.root.first_child().ok_or_else(|| io::Error::other("the tab has no pane"))?;
        self.carry_widget(&top, text_of)
    }

    fn carry_widget(&self, widget: &gtk::Widget, text_of: &impl Fn(&Pane) -> String) -> io::Result<Layout> {
        if let Some(split) = widget.downcast_ref::<gtk::Paned>() {
            let (Some(start), Some(end)) = (split.start_child(), split.end_child()) else {
                return Err(io::Error::other("a split lacks a side"));
            };
            return Ok(Layout::Split {
                vertical: split.orientation() == gtk::Orientation::Vertical,
                position: split.position(),
                start: Box::new(self.carry_widget(&start, text_of)?),
                end: Box::new(self.carry_widget(&end, text_of)?),
            });
        }
        let pane = self.panes.borrow().iter().find(|pane| pane.widget.upcast_ref::<gtk::Widget>() == widget).cloned();
        let pane = pane.ok_or_else(|| io::Error::other("a part of the tab is no pane"))?;
        Ok(Layout::Pane(pane.carry(text_of(&pane))?))
    }

    fn event_handler(self: &Rc<Self>) -> impl Fn(&Rc<Pane>, PaneEvent) + 'static {
        let weak = Rc::downgrade(self);
        move |pane, event| {
            if let Some(page) = weak.upgrade() {
                page.handle(pane, event);
            }
        }
    }

    fn spawn_pane(
        self: &Rc<Self>,
        directory: Option<&Path>,
        profile: Option<&str>,
        command: &[String],
    ) -> io::Result<Rc<Pane>> {
        let options = PaneOptions {
            directory,
            bash_integration: shell_integration::path(),
            profile,
            command,
        };
        let pane = Pane::spawn(&options, self.event_handler())?;
        self.add_pane(&pane);
        Ok(pane)
    }

    /// Follows the pane's focus, and makes it the focused one.
    fn add_pane(self: &Rc<Self>, pane: &Rc<Pane>) {
        let focus = gtk::EventControllerFocus::new();
        focus.connect_enter(glib::clone!(
            #[weak(rename_to = page)]
            self,
            #[weak]
            pane,
            move |_| page.focus_changed(&pane)
        ));
        pane.screen.add_controller(focus);
        self.panes.borrow_mut().push(pane.clone());
        self.focused.replace(Rc::downgrade(pane));
    }

    fn handle(self: &Rc<Self>, pane: &Rc<Pane>, event: PaneEvent) {
        match event {
            PaneEvent::Changed => {
                if window::is_watched(self) {
                    pane.acknowledge();
                }
                let status = pane.status();
                if pane.reported.replace(status) != status && status.is_news() {
                    window::notify(self, pane, status);
                }
                self.changed();
            }
            PaneEvent::Exited => self.remove_pane(pane),
            PaneEvent::Notify { title, body, happening } => window::notify_message(self, &title, &body, happening),
        }
    }

    fn focus_changed(&self, pane: &Rc<Pane>) {
        self.focused.replace(Rc::downgrade(pane));
        self.mark_focus();
        pane.acknowledge();
        self.changed();
    }

    /// The focused pane is outlined only when there is more than one to tell apart.
    fn mark_focus(&self) {
        let focused = self.focused_pane();
        let several = self.panes.borrow().len() > 1;
        for pane in self.panes.borrow().iter() {
            let is_focused = focused.as_ref().is_some_and(|focused| Rc::ptr_eq(focused, pane));
            if several && is_focused {
                pane.widget.add_css_class("focused");
            } else {
                pane.widget.remove_css_class("focused");
            }
        }
    }

    pub fn changed(&self) {
        self.refresh_tab();
        window::refresh_soon();
    }

    pub fn focused_pane(&self) -> Option<Rc<Pane>> {
        self.focused
            .borrow()
            .upgrade()
            .or_else(|| self.panes.borrow().first().cloned())
    }

    pub fn panes(&self) -> Vec<Rc<Pane>> {
        self.panes.borrow().clone()
    }

    /// Splits the focused pane, the new one opening where it is.
    pub fn split(self: &Rc<Self>, orientation: gtk::Orientation) -> io::Result<()> {
        let Some(old) = self.focused_pane() else {
            return Ok(());
        };
        self.split_pane(&old, orientation, None, &[]).map(drop)
    }

    /// Splits `old`, the new pane running `command` in `directory`, or the shell where `old` is.
    pub fn split_pane(
        self: &Rc<Self>,
        old: &Rc<Pane>,
        orientation: gtk::Orientation,
        directory: Option<&Path>,
        command: &[String],
    ) -> io::Result<Rc<Pane>> {
        let directory = directory.map(Path::to_path_buf).or_else(|| old.directory());
        let new = self.spawn_pane(directory.as_deref(), Some(old.profile()), command)?;
        let split = new_split(orientation);
        replace_child(old.widget.upcast_ref(), split.upcast_ref());
        split.set_start_child(Some(&old.widget));
        split.set_end_child(Some(&new.widget));
        self.mark_focus();
        new.screen.grab_focus();
        Ok(new)
    }

    /// Takes a pane whose shell has exited out of the tab; the tab closes with its last.
    fn remove_pane(self: &Rc<Self>, pane: &Rc<Pane>) {
        self.panes.borrow_mut().retain(|kept| !Rc::ptr_eq(kept, pane));
        match pane.widget.parent().and_downcast::<gtk::Paned>() {
            Some(split) => {
                let sibling = if split.start_child().as_ref() == Some(pane.widget.upcast_ref()) {
                    split.end_child()
                } else {
                    split.start_child()
                };
                split.set_start_child(gtk::Widget::NONE);
                split.set_end_child(gtk::Widget::NONE);
                if let Some(sibling) = sibling {
                    replace_child(split.upcast_ref(), &sibling);
                }
            }
            None => self.root.remove(&pane.widget),
        }
        if self.panes.borrow().is_empty() {
            window::close_emptied(self);
            return;
        }
        let next = self.panes.borrow().first().cloned();
        if let Some(next) = next {
            self.focused.replace(Rc::downgrade(&next));
            next.screen.grab_focus();
        }
        self.mark_focus();
        self.changed();
    }

    /// The pane that speaks for the tab.
    fn most_urgent_pane(&self) -> Option<Rc<Pane>> {
        self.panes
            .borrow()
            .iter()
            .max_by_key(|pane| pane.status().urgency())
            .cloned()
    }

    pub fn status(&self) -> Status {
        self.most_urgent_pane()
            .map_or(Status::Idle, |pane| pane.status())
    }

    pub fn title(&self) -> String {
        if let Some(title) = self.custom_title.borrow().clone() {
            return title;
        }
        self.focused_pane()
            .map(|pane| pane.label())
            .unwrap_or_default()
    }

    /// What the pane that speaks for the tab last said, when it said something.
    pub fn message(&self) -> Option<String> {
        self.most_urgent_pane()?.message()
    }

    /// Where the focused pane is: its repository and branch, or its folder.
    pub fn place(&self) -> String {
        self.focused_pane()
            .map(|pane| pane.place())
            .unwrap_or_default()
    }

    pub fn repository(&self) -> Option<Repository> {
        self.focused_pane()?.repository()
    }

    pub fn folder_label(&self) -> Option<String> {
        self.focused_pane()?.folder_label()
    }

    /// An agent runs in the pane in focus, so the bar shows its state.
    /// What Claude Code last said of its session in the focused pane.
    pub fn usage(&self) -> Option<Usage> {
        self.focused_pane()?.usage()
    }

    pub fn subagents(&self) -> Option<super::subagents::Subagents> {
        Some(self.focused_pane()?.subagents())
    }

    pub fn subagents_folder(&self) -> Option<std::path::PathBuf> {
        self.focused_pane()?.subagents_folder()
    }

    pub fn runs_agent(&self) -> bool {
        self.focused_pane()
            .is_some_and(|pane| pane.agent_name().is_some())
    }

    /// The profile the pane in focus was started from, for a tab opened like it.
    pub fn profile(&self) -> Option<String> {
        self.focused_pane().map(|pane| pane.profile().to_string())
    }

    pub fn shell_pids(&self) -> Vec<i32> {
        self.panes.borrow().iter().map(|pane| pane.shell_pid()).collect()
    }

    /// Keeps the tab's badges; true when they changed.
    pub fn set_badges(&self, pull_request: Option<PullRequest>, ports: Vec<u16>) -> bool {
        let changed = *self.pull_request.borrow() != pull_request || *self.ports.borrow() != ports;
        self.pull_request.replace(pull_request);
        self.ports.replace(ports);
        if changed {
            self.refresh_tab();
        }
        changed
    }

    pub fn pull_request_number(&self) -> Option<u64> {
        self.pull_request.borrow().as_ref().map(|pull_request| pull_request.number)
    }

    pub fn badges(&self) -> Vec<Chip> {
        badges::chips(self.pull_request.borrow().as_ref(), &self.ports.borrow())
    }

    pub fn pane_count(&self) -> usize {
        self.panes.borrow().len()
    }

    pub fn running_programs(&self) -> Vec<String> {
        self.panes
            .borrow()
            .iter()
            .filter_map(|pane| pane.running_program())
            .collect()
    }

    pub fn acknowledge(&self) {
        self.panes.borrow().iter().for_each(|pane| pane.acknowledge());
        self.refresh_tab();
    }

    /// Reads every pane's job and folder; true when any changed.
    pub fn poll(&self) -> bool {
        let changed = self
            .panes
            .borrow()
            .iter()
            .fold(false, |changed, pane| pane.poll() | changed);
        if changed {
            self.refresh_tab();
        }
        changed
    }

    /// The tab: its name, a spinner while it works, and a dot for waiting, done or failed,
    /// or else its colour.
    pub fn refresh_tab(&self) {
        let Some(tab) = self.tab.borrow().clone() else {
            return;
        };
        let status = self.status();
        tab.set_title(&self.title());
        let badges = badges::text(self.pull_request.borrow().as_ref(), &self.ports.borrow());
        let tooltip = [Some(self.place()), badges].into_iter().flatten().collect::<Vec<_>>().join("\n");
        tab.set_tooltip(&glib::markup_escape_text(&tooltip));
        tab.set_loading(status == Status::Working);
        tab.set_needs_attention(status == Status::Waiting);
        let dot = match status {
            Status::Waiting | Status::Done | Status::Failed => Some(status.colour()),
            Status::Working | Status::Idle => self.colour.get().and_then(colour_hex),
        };
        tab.set_indicator_icon(dot.map(dot_icon).as_ref());
    }
}

/// The divider between two parts of a tab.
fn new_split(orientation: gtk::Orientation) -> gtk::Paned {
    gtk::Paned::builder()
        .orientation(orientation)
        .wide_handle(false)
        .shrink_start_child(false)
        .shrink_end_child(false)
        .build()
}

/// Puts `new` where `old` is in the tab's layout, taking `old` out.
fn replace_child(old: &gtk::Widget, new: &gtk::Widget) {
    let Some(parent) = old.parent() else {
        return;
    };
    if let Some(split) = parent.downcast_ref::<gtk::Paned>() {
        if split.start_child().as_ref() == Some(old) {
            split.set_start_child(Some(new));
        } else {
            split.set_end_child(Some(new));
        }
    } else if let Some(container) = parent.downcast_ref::<gtk::Box>() {
        container.insert_child_after(new, Some(old));
        container.remove(old);
    }
}

pub fn colour_hex(name: &str) -> Option<&'static str> {
    TAB_COLOURS
        .iter()
        .find(|(id, _, _)| *id == name)
        .map(|(_, _, hex)| *hex)
}

/// A round dot of `hex`, as an icon for a tab.
fn dot_icon(hex: &str) -> gdk::Texture {
    let colour = gdk::RGBA::parse(hex).expect("the dot colours are valid");
    let channels = [colour.red(), colour.green(), colour.blue()].map(|channel| f64::from(channel) * 255.0);
    let centre = DOT_TEXTURE_SIZE as f64 / 2.0;
    let mut pixels = Vec::with_capacity(DOT_TEXTURE_SIZE * DOT_TEXTURE_SIZE * BYTES_PER_PIXEL);
    for row in 0..DOT_TEXTURE_SIZE {
        for column in 0..DOT_TEXTURE_SIZE {
            let distance = (column as f64 + 0.5 - centre).hypot(row as f64 + 0.5 - centre);
            // One pixel of edge, blended, so that the dot is round at any scale.
            let coverage = (DOT_RADIUS + 0.5 - distance).clamp(0.0, 1.0);
            pixels.extend(channels.map(|channel| (channel * coverage) as u8));
            pixels.push((coverage * 255.0) as u8);
        }
    }
    gdk::MemoryTexture::new(
        DOT_TEXTURE_SIZE as i32,
        DOT_TEXTURE_SIZE as i32,
        gdk::MemoryFormat::R8g8b8a8Premultiplied,
        &glib::Bytes::from_owned(pixels),
        DOT_TEXTURE_SIZE * BYTES_PER_PIXEL,
    )
    .upcast()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_colour_is_found_by_its_name() {
        assert_eq!(colour_hex("blue"), Some("#4a7fc0"));
    }
}
