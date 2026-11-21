//! The hover panel, after FinderHover on the Mac: resting the pointer on a file on the
//! desktop, in a folder opened from the Dock or in a Files or Nautilus window, shows beside
//! it, on glass, what the file is and holds, without opening it. Moving off the file puts
//! the panel away. Holding Alt keeps it where it is, to move into it and copy any value
//! with a click. A note typed in it is kept with the file.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::mpsc::{self, Receiver, TryRecvError},
    time::{Duration, Instant},
};

use cairo::{Context, Format, ImageSurface};
use gio::prelude::*;
use smithay::{
    backend::renderer::element::Id,
    input::keyboard::Keysym,
    reexports::wayland_server::{protocol::wl_surface::WlSurface, Resource},
    utils::{IsAlive, Logical, Point, Rectangle, Size},
    wayland::selection::{data_device::set_data_device_selection, SelectionTarget},
};

use crate::{
    appgrid::draw_candidates,
    config::HoverStyle,
    fileinfo::{self, Block, Facts, Field, Part, Rgb, Shade, Spec, GREEN, RED},
    glass::Tier,
    ime::Candidates,
    menubar::{self, Overlay, MENU_BAR_HEIGHT},
    nautilus,
    render::OUTLINE_COLOR,
    stacks::{self, Item},
    state::Nekoland,
    text::{self, Picture},
};

const PADDING: f64 = 12.0;
const RADIUS: i32 = 14;
const ICON: i32 = 36;
/// Room between sections, above and below the line between them.
const GAP: f64 = 8.0;
/// The tighter padding, room and icon of compact mode.
const COMPACT_PADDING: f64 = 8.0;
const COMPACT_GAP: f64 = 4.0;
const COMPACT_ICON: f64 = 28.0;
/// The text size the panel is laid out at; `hover_font_size` scales it.
const FONT_POINTS: f64 = 11.0;
/// From the output's edges.
const EDGE: i32 = 8;
/// A note grows with its lines up to this many, then is cut off.
const NOTE_LINES: i32 = 12;
const PLACEHOLDER: &str = "輸入備註⋯";
/// How long the pointer rests in a Nautilus window before Nautilus is asked what is under it.
const NAUTILUS_REST: Duration = Duration::from_millis(50);
/// What text copied out of the panel is offered as.
const TEXT_TYPES: [&str; 5] = [
    "text/plain;charset=utf-8",
    "text/plain",
    "UTF8_STRING",
    "STRING",
    "TEXT",
];

pub struct Hover {
    /// The file the pointer rests on, and when it last moved.
    resting: Option<(Item, Instant)>,
    /// The file being read on a thread, and where what is read comes back.
    reading: Option<(PathBuf, Receiver<Facts>)>,
    shown: Option<Shown>,
    pointer: Point<f64, Logical>,
    /// Alt is held over a panel: it stays, and its values are copied with a click.
    locked: bool,
    /// The value last copied, marked as copied.
    copied: Option<String>,
    pub candidates: Option<Candidates>,
    candidates_picture: Option<(Candidates, Picture)>,
    id: Id,
    candidates_id: Id,
    /// Since when the pointer has rested in a Nautilus window, not yet asked about.
    nautilus_rest: Option<Instant>,
    /// Which file is at a point in a Nautilus window, asked of a thread, and since when the
    /// pointer has rested there.
    asking: Option<(Point<f64, Logical>, Instant, Receiver<Option<nautilus::Answer>>)>,
    /// The file Nautilus showed under the pointer, and where on the screen its cell is.
    pointed: Option<(Rectangle<f64, Logical>, Item)>,
    nautilus: Option<nautilus::Asker>,
    /// What GTK calls each window on the bus, which Nautilus says the folders of.
    gtk_windows: HashMap<WlSurface, String>,
}

impl Default for Hover {
    fn default() -> Self {
        Self {
            resting: None,
            reading: None,
            shown: None,
            pointer: Point::default(),
            locked: false,
            copied: None,
            candidates: None,
            candidates_picture: None,
            id: Id::new(),
            candidates_id: Id::new(),
            nautilus_rest: None,
            asking: None,
            pointed: None,
            nautilus: None,
            gtk_windows: HashMap::new(),
        }
    }
}

struct Shown {
    item: Item,
    facts: Facts,
    /// The kind's icon until the picture the file is, or its thumbnail, is read.
    icon: Option<ImageSurface>,
    icon_read: Option<stacks::Reading>,
    /// Where the pointer was when the panel came.
    anchor: Point<i32, Logical>,
    picture: Option<Picture>,
    drawn: Option<Look>,
    /// What a click in the panel reaches, from its top left corner.
    spots: Vec<(Rectangle<f64, Logical>, Spot)>,
    note: Note,
}

#[derive(Clone, Debug, PartialEq)]
enum Spot {
    Copy(String),
    Note,
}

/// The file's note, and its field while it takes keys.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Note {
    text: String,
    /// Where typing goes, in bytes; none while the field does not take keys.
    caret: Option<usize>,
    pub preedit: String,
    pub ime_focused: bool,
    error: Option<String>,
}

/// What the panel was drawn with, to know when to draw it again.
#[derive(Clone, Debug, PartialEq)]
struct Look {
    dark: bool,
    style: HoverStyle,
    hidden: Vec<String>,
    locked: bool,
    /// Which of the values to copy the pointer is on.
    hot: Option<usize>,
    copied: Option<String>,
    note: Note,
    width: i32,
    font_scale: f64,
    compact: bool,
    glass: bool,
    opacity: f64,
}

impl Hover {
    /// A GTK window said what it is called on the bus.
    pub fn set_gtk_window(&mut self, surface: &WlSurface, path: String) {
        self.gtk_windows.retain(|surface, _| surface.alive());
        self.gtk_windows.insert(surface.clone(), path);
    }

    /// The note being typed, if one is.
    pub fn note_mut(&mut self) -> Option<&mut Note> {
        self.shown
            .as_mut()
            .map(|shown| &mut shown.note)
            .filter(|note| note.caret.is_some())
    }

    fn hide(&mut self) {
        self.shown = None;
        self.reading = None;
        self.candidates = None;
    }
}

impl Note {
    fn insert(&mut self, typed: &str) {
        let caret = self.caret.unwrap_or(self.text.len());
        self.text.insert_str(caret, typed);
        self.caret = Some(caret + typed.len());
    }

    /// A key that edits the note; false for one that does not.
    fn key(&mut self, sym: Keysym) -> bool {
        let Some(caret) = self.caret else {
            return false;
        };
        let previous = self.text[..caret]
            .char_indices()
            .next_back()
            .map_or(0, |(at, _)| at);
        let next = self.text[caret..]
            .chars()
            .next()
            .map_or(caret, |character| caret + character.len_utf8());
        let line_start = self.text[..caret].rfind('\n').map_or(0, |at| at + 1);
        let line_end = self.text[caret..]
            .find('\n')
            .map_or(self.text.len(), |at| caret + at);
        self.caret = Some(match sym {
            Keysym::BackSpace => {
                self.text.replace_range(previous..caret, "");
                previous
            }
            Keysym::Delete => {
                self.text.replace_range(caret..next, "");
                caret
            }
            Keysym::Left => previous,
            Keysym::Right => next,
            Keysym::Home => line_start,
            Keysym::End => line_end,
            Keysym::Up => 0,
            Keysym::Down => self.text.len(),
            Keysym::Return | Keysym::KP_Enter => {
                self.text.insert(caret, '\n');
                caret + 1
            }
            _ => return false,
        });
        true
    }
}

impl Nekoland {
    /// The file under `pos` the panel may be for: one on the desktop with nothing over it
    /// and nothing going on there, one pointed at in a folder opened from the Dock, or the
    /// one Nautilus showed there.
    fn hover_target(&self, pos: Point<f64, Logical>) -> Option<Item> {
        if self.hover_blocked() {
            return None;
        }
        self.stack_hovered_item()
            .or_else(|| self.desktop_resting_item(pos))
            .or_else(|| {
                let (cell, item) = self.hover.pointed.as_ref()?;
                cell.contains(pos).then(|| item.clone())
            })
    }

    /// What to ask about `pos`, when a Nautilus or Files window is the topmost there, and
    /// where that window is.
    fn nautilus_question(
        &self,
        pos: Point<f64, Logical>,
    ) -> Option<(nautilus::Question, Point<f64, Logical>)> {
        if self.hover_blocked() || self.menu_bar_covers(pos) {
            return None;
        }
        let (window, _) = self.space.element_under(pos)?;
        // GTK gives places from the window's visible corner, inside its shadow.
        let location = self.space.element_location(window)?;
        let (title, app_id) = crate::windows::window_names(window);
        if app_id != nautilus::APP_ID && app_id != nautilus::FILES_APP_ID {
            return None;
        }
        let surface = window.toplevel()?.wl_surface();
        let pid = surface.client()?.get_credentials(&self.display_handle).ok()?.pid;
        let local = pos - location.to_f64();
        let question = nautilus::Question {
            app_id,
            pid: u32::try_from(pid).ok()?,
            title,
            window: self.hover.gtk_windows.get(surface).cloned(),
            point: (local.x.floor() as i32, local.y.floor() as i32),
        };
        Some((question, location.to_f64()))
    }

    /// Once the pointer has rested in a Nautilus window, asks which file is under it; the
    /// answer, for where the pointer still is, is the file it rests on.
    fn ask_nautilus(&mut self) {
        let pos = self.hover.pointer;
        if let Some((at, since, receiver)) = &self.hover.asking {
            let answer = match receiver.try_recv() {
                Ok(answer) => answer,
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => None,
            };
            let (at, since) = (*at, *since);
            self.hover.asking = None;
            if at != pos {
                return;
            }
            if let Some((item, (left, top, width, height))) = answer {
                let Some((_, window)) = self.nautilus_question(pos) else {
                    return;
                };
                let cell = Rectangle::new(
                    window + Point::from((f64::from(left), f64::from(top))),
                    Size::from((f64::from(width), f64::from(height))),
                );
                self.hover.pointed = Some((cell, item.clone()));
                if self.hover.resting.as_ref().map(|(resting, _)| &resting.path) != Some(&item.path) {
                    self.hover.resting = Some((item, since));
                }
            }
            return;
        }
        let Some(since) = self
            .hover
            .nautilus_rest
            .filter(|since| since.elapsed() >= NAUTILUS_REST)
        else {
            return;
        };
        self.hover.nautilus_rest = None;
        let Some((question, _)) = self.nautilus_question(pos) else {
            return;
        };
        let asker = self.hover.nautilus.get_or_insert_with(nautilus::start);
        let (sender, receiver) = mpsc::channel();
        if asker.send((question, sender)).is_ok() {
            self.hover.asking = Some((pos, since, receiver));
        }
    }

    fn hover_blocked(&self) -> bool {
        !self.config.hover_info || self.menu_bar.open.is_some() || self.overview.is_some()
    }

    /// Whether the panel goes when the pointer is on no file: always unless auto-hide is
    /// off, and then still when a menu or the overview comes.
    fn hover_leaves(&self) -> bool {
        self.config.hover_auto_hide || self.hover_blocked()
    }

    /// Where the panel is: beside the pointer, below and to the right of it unless the
    /// output's edge is nearer than the panel is big.
    fn hover_area(&self) -> Option<Rectangle<i32, Logical>> {
        let shown = self.hover.shown.as_ref()?;
        let size = shown.picture.as_ref()?.size;
        let output = self.output_area()?;
        let anchor = if self.config.hover_follow && !self.hover.locked && shown.note.caret.is_none() {
            self.hover.pointer.to_i32_round()
        } else {
            shown.anchor
        };
        let (dx, dy) = (self.config.hover_offset_x, self.config.hover_offset_y);
        let (left, top) = (output.loc.x + EDGE, output.loc.y + MENU_BAR_HEIGHT + EDGE);
        let (right, bottom) = (
            output.loc.x + output.size.w - EDGE,
            output.loc.y + output.size.h - EDGE,
        );
        let mut x = anchor.x + dx;
        if x + size.w > right {
            x = anchor.x - dx - size.w;
        }
        let mut y = anchor.y + dy;
        if y + size.h > bottom {
            y = anchor.y - dy - size.h;
        }
        Some(Rectangle::new(
            (x.min(right - size.w).max(left), y.min(bottom - size.h).max(top)).into(),
            size,
        ))
    }

    /// Whether the panel stays where it is whatever is under the pointer and takes the
    /// pointer itself: Alt is held or its note takes keys. Otherwise clicks go through it.
    fn hover_keeps(&self) -> bool {
        self.hover
            .shown
            .as_ref()
            .is_some_and(|shown| self.hover.locked || shown.note.caret.is_some())
    }

    pub fn hover_covers(&self, pos: Point<f64, Logical>) -> bool {
        self.hover_keeps() && self.hover_area().is_some_and(|area| area.to_f64().contains(pos))
    }

    /// The pointer moved: the panel waits for it to rest again, and goes when it leaves
    /// the panel's file.
    pub fn hover_motion(&mut self, pos: Point<f64, Logical>) {
        self.hover.pointer = pos;
        if self.hover_keeps() {
            return;
        }
        let target = self.hover_target(pos);
        let shown = self.hover.shown.as_ref().map(|shown| &shown.item.path);
        if shown.is_some() && shown == target.as_ref().map(|item| &item.path) {
            return;
        }
        if target.is_some() || self.hover_leaves() {
            self.hover.hide();
        }
        // Off the file Nautilus showed, Nautilus is asked again once the pointer rests.
        if target.is_none() {
            self.hover.pointed = None;
        }
        self.hover.nautilus_rest = target.is_none().then(Instant::now);
        self.hover.resting = target.map(|item| (item, Instant::now()));
    }

    /// Each frame: a file rested on long enough is read; what is read is shown; a panel
    /// whose file is no longer pointed at, or covered, goes; and it is drawn again when
    /// what it shows has changed.
    pub fn refresh_hover(&mut self) {
        let alt = self
            .seat
            .get_keyboard()
            .is_some_and(|keyboard| keyboard.modifier_state().alt);
        let was_locked = self.hover.locked;
        self.hover.locked = alt && self.hover.shown.is_some();
        self.ask_nautilus();
        let pos = self.hover.pointer;
        if !self.hover_keeps() {
            let target = self.hover_target(pos).map(|item| item.path);
            let shown = self.hover.shown.as_ref().map(|shown| shown.item.path.clone());
            if shown.is_some() && shown != target && (target.is_some() || self.hover_leaves()) {
                self.hover.hide();
            }
            if self.hover.resting.as_ref().map(|(item, _)| &item.path) != target.as_ref() {
                self.hover.resting = None;
            }
            if was_locked && !self.hover.locked {
                self.hover.copied = None;
            }
        }

        let delay = Duration::from_secs_f64(self.config.hover_delay.clamp(0.0, 5.0));
        if let Some((item, since)) = &self.hover.resting {
            let waiting = self.hover.reading.as_ref().map(|(path, _)| path) == Some(&item.path)
                || self.hover.shown.as_ref().map(|shown| &shown.item.path) == Some(&item.path);
            if !waiting && since.elapsed() >= delay {
                let (sender, receiver) = mpsc::channel();
                let (path, kind) = (item.path.clone(), item.kind.clone());
                let protect = self.config.hover_large_file_protection;
                std::thread::spawn(move || {
                    // The panel may have gone by the time the file is read.
                    let _ = sender.send(fileinfo::read(&path, &kind, protect));
                });
                self.hover.reading = Some((item.path.clone(), receiver));
            }
        }
        if let Some((_, receiver)) = &self.hover.reading {
            match receiver.try_recv() {
                Ok(facts) => {
                    self.hover.reading = None;
                    if let Some((item, _)) = self
                        .hover
                        .resting
                        .clone()
                        .filter(|(item, _)| item.path == facts.path)
                    {
                        let dark = self.menu_bar_dark();
                        let ink = menubar::colors(dark).ink;
                        self.hover.shown = Some(Shown {
                            icon: stacks::kind_icon(&item, ICON, ink),
                            icon_read: stacks::picture_path(&item).map(|path| {
                                let size = ICON * text::picture_scale();
                                stacks::read_on_a_thread(path, Size::from((size, size)))
                            }),
                            note: Note {
                                text: facts.note.clone().unwrap_or_default(),
                                error: facts.note.clone().err(),
                                ..Note::default()
                            },
                            item,
                            facts,
                            anchor: pos.to_i32_round(),
                            picture: None,
                            drawn: None,
                            spots: Vec::new(),
                        });
                    }
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => self.hover.reading = None,
            }
        }
        self.draw_hover();
    }

    /// How the panel is drawn as the settings say, with nothing held, hot or typed.
    fn hover_look(&self) -> Look {
        Look {
            dark: self.menu_bar_dark(),
            style: self.config.hover_style,
            hidden: self.config.hover_hide.clone(),
            locked: false,
            hot: None,
            copied: None,
            note: Note::default(),
            // FinderHover's ranges, whatever the file says.
            width: self.config.hover_width.clamp(300, 600),
            font_scale: self.config.hover_font_size.clamp(9.0, 14.0) / FONT_POINTS,
            compact: self.config.hover_compact,
            glass: self.config.hover_glass,
            opacity: self.config.hover_opacity.clamp(0.7, 1.0),
        }
    }

    /// `hover preview NAME`: the panel for the sample of the kind named, or of a plain file,
    /// drawn as it is now into a PNG for the settings. Answers where, and its width and
    /// height in points, by tabs.
    pub fn hover_preview(&self, name: &str) -> Option<String> {
        let mut samples = fileinfo::samples();
        let at = samples
            .iter()
            .position(|(facts, _)| facts.block.as_ref().is_some_and(|block| block.name == name))
            .unwrap_or(0);
        let (facts, kind) = samples.swap_remove(at);
        let look = self.hover_look();
        let ink = menubar::colors(look.dark).ink;
        let icon = gio::content_type_get_icon(kind)
            .downcast::<gio::ThemedIcon>()
            .ok()
            .and_then(|icon| {
                icon.names()
                    .iter()
                    .find_map(|name| text::named_icon(name, "", ICON, ink))
            });
        let (surface, _) = paint_panel(&facts, icon.as_ref(), &look)?;
        let path = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?).join("nekoland-hover-preview.png");
        let written = std::fs::File::create(&path)
            .map_err(|err| err.to_string())
            .and_then(|mut file| surface.write_to_png(&mut file).map_err(|err| err.to_string()));
        if let Err(err) = written {
            tracing::warn!("hover preview {}: {err}", path.display());
            return None;
        }
        let scale = text::picture_scale();
        Some(format!(
            "{}\t{}\t{}\n",
            path.display(),
            surface.width() / scale,
            surface.height() / scale
        ))
    }

    fn draw_hover(&mut self) {
        let look = self.hover_look();
        let area = self.hover_area();
        let hover = &mut self.hover;
        if hover.candidates.as_ref() != hover.candidates_picture.as_ref().map(|(drawn, _)| drawn) {
            hover.candidates_picture = hover
                .candidates
                .as_ref()
                .and_then(|candidates| Some((candidates.clone(), draw_candidates(candidates)?)));
        }
        let Some(shown) = hover.shown.as_mut() else {
            return;
        };
        let read = shown.icon_read.as_ref().and_then(|read| read.lock().ok()?.take());
        if let Some(pixels) = read {
            shown.icon_read = None;
            if let Some(icon) = pixels.and_then(stacks::pixels_surface) {
                shown.icon = Some(icon);
                shown.drawn = None;
            }
        }
        let local = area.map(|area| hover.pointer - area.loc.to_f64());
        let hot = local.filter(|_| hover.locked).and_then(|local| {
            shown
                .spots
                .iter()
                .filter(|(_, spot)| matches!(spot, Spot::Copy(_)))
                .position(|(rect, _)| rect.contains(local))
        });
        let look = Look {
            locked: hover.locked,
            hot,
            copied: hover.copied.clone(),
            note: shown.note.clone(),
            ..look
        };
        if shown.drawn.as_ref() == Some(&look) {
            return;
        }
        let drawn = draw_panel(&shown.facts, shown.icon.as_ref(), &look);
        shown.picture = drawn.as_ref().map(|(picture, _)| picture.clone());
        shown.spots = drawn.map(|(_, spots)| spots).unwrap_or_default();
        shown.drawn = Some(look);
    }

    /// The panel on the menus' glass, and the input method's candidates for its note under
    /// it, over everything.
    pub fn hover_overlays(&self) -> Vec<Overlay<'_>> {
        let Some(area) = self.hover_area() else {
            return Vec::new();
        };
        let Some(picture) = self.hover.shown.as_ref().and_then(|shown| shown.picture.as_ref()) else {
            return Vec::new();
        };
        let candidates = self
            .hover
            .candidates_picture
            .as_ref()
            .map(|(_, picture)| Overlay {
                id: &self.hover.candidates_id,
                rect: Rectangle::new(area.loc + Point::from((0, area.size.h + 4)), picture.size),
                glass: None,
                alpha: 1.0,
                fade: 1.0,
                picture,
            });
        candidates
            .into_iter()
            .chain([Overlay {
                id: &self.hover.id,
                rect: area,
                glass: self.config.hover_glass.then_some((Tier::Menu, RADIUS)),
                alpha: 1.0,
                fade: 1.0,
                picture,
            }])
            .collect()
    }

    /// The panel goes, as on a press elsewhere: the file it was about is gone from view.
    pub fn hover_dismiss(&mut self) {
        self.hover.hide();
        self.hover.resting = None;
    }

    /// A press: in the panel while it takes the pointer, a click on the note types in it,
    /// and with Alt held, one on a value copies it; anywhere else, the panel goes. Returns
    /// whether it was in the panel.
    pub fn hover_press(&mut self, pos: Point<f64, Logical>) -> bool {
        let area = self
            .hover_area()
            .filter(|area| self.hover_keeps() && area.to_f64().contains(pos));
        let Some(area) = area else {
            self.hover_dismiss();
            return false;
        };
        let local = pos - area.loc.to_f64();
        let locked = self.hover.locked;
        let Some(shown) = self.hover.shown.as_mut() else {
            return false;
        };
        let spot = shown
            .spots
            .iter()
            .find(|(rect, spot)| rect.contains(local) && (locked || *spot == Spot::Note))
            .map(|(_, spot)| spot.clone());
        match spot {
            Some(Spot::Note) => {
                if shown.note.caret.is_none() {
                    shown.note.caret = Some(shown.note.text.len());
                }
            }
            Some(Spot::Copy(value)) => {
                self.copy_text(&value);
                self.hover.copied = Some(value);
            }
            None => self.end_note(),
        }
        true
    }

    /// Puts `text` on the clipboard, for Wayland and X11 programs both.
    fn copy_text(&mut self, text: &str) {
        let mime_types: Vec<String> = TEXT_TYPES.map(String::from).into();
        if let Some(xwm) = self.xwm.as_mut() {
            if let Err(err) = xwm.new_selection(SelectionTarget::Clipboard, Some(mime_types.clone())) {
                tracing::warn!("offering copied text to X11 programs: {err}");
            }
        }
        // The clipboard's own data is a list of paths, written out as text for text types;
        // one "path" that is the text gives the text.
        set_data_device_selection(
            &self.display_handle,
            &self.seat,
            mime_types,
            Some(vec![PathBuf::from(text)]),
        );
    }

    /// The note stops taking keys.
    fn end_note(&mut self) {
        if let Some(note) = self.hover.note_mut() {
            note.caret = None;
            note.preedit.clear();
            note.ime_focused = false;
        }
        self.hover.candidates = None;
    }

    /// A key while the note takes them: Escape ends typing, Return starts a new line.
    /// Returns whether the note took it.
    pub fn hover_key(&mut self, sym: Keysym, typed: Option<char>) -> bool {
        let Some(note) = self.hover.note_mut() else {
            return false;
        };
        if sym == Keysym::Escape {
            self.end_note();
            return true;
        }
        if !note.key(sym) {
            match typed.filter(|character| !character.is_control()) {
                Some(character) => note.insert(&character.to_string()),
                None => return true,
            }
        }
        self.save_note();
        true
    }

    /// Text the input method put in the note.
    pub fn hover_note_commit(&mut self, text: &str) {
        if let Some(note) = self.hover.note_mut() {
            note.insert(text);
            self.save_note();
        }
    }

    // shortcut: the note is written on the compositor's thread at each key, a single
    // setxattr; move it to a thread if notes on network folders make typing wait.
    fn save_note(&mut self) {
        let Some(shown) = self.hover.shown.as_mut() else {
            return;
        };
        shown.note.error = fileinfo::save_note(&shown.item.path, &shown.note.text)
            .inspect_err(|err| tracing::warn!("{}: {err}", shown.item.path.display()))
            .err()
            .map(|_| "無法儲存備註。".to_string());
    }
}

/// The panel for `facts` as `look` says, and where its values and note are in it.
fn draw_panel(
    facts: &Facts,
    icon: Option<&ImageSurface>,
    look: &Look,
) -> Option<(Picture, Vec<(Rectangle<f64, Logical>, Spot)>)> {
    let (surface, spots) = paint_panel(facts, icon, look)?;
    Some((text::picture(surface)?, spots))
}

/// `facts` without what `hidden` leaves out: a kind's block by its name, one of its fields by
/// `name/label`, a date or file detail by `dates/label` or `file/label`.
fn without_hidden(facts: &Facts, hidden: &[String]) -> Facts {
    let hides = |group: &str, label: &str| {
        let label = fileinfo::plain_label(label);
        hidden
            .iter()
            .any(|name| name.strip_prefix(group).and_then(|rest| rest.strip_prefix('/')) == Some(label))
    };
    let fields = |group: &str, fields: &[Field]| -> Vec<Field> {
        fields
            .iter()
            .filter(|field| !hides(group, &field.label))
            .cloned()
            .collect()
    };
    let block = |block: &Option<Block>| -> Option<Block> {
        let block = block
            .as_ref()
            .filter(|block| !hidden.iter().any(|name| name == block.name))?;
        let parts: Vec<Part> = block
            .parts
            .iter()
            .filter_map(|part| match part {
                Part::Fields(list, columns) => {
                    let list = fields(block.name, list);
                    (!list.is_empty()).then_some(Part::Fields(list, *columns))
                }
                Part::Specs(specs) => {
                    let specs: Vec<Spec> = specs
                        .iter()
                        .filter(|spec| !hides(block.name, &spec.label))
                        .cloned()
                        .collect();
                    (!specs.is_empty()).then_some(Part::Specs(specs))
                }
                other => Some(other.clone()),
            })
            .collect();
        (!parts.is_empty()).then(|| Block {
            parts,
            ..block.clone()
        })
    };
    Facts {
        block: block(&facts.block),
        source: block(&facts.source),
        dates: fields("dates", &facts.dates),
        file: fields("file", &facts.file),
        ..facts.clone()
    }
}

/// The panel drawn into a surface, at `text::picture_scale` pixels to the point.
fn paint_panel(
    facts: &Facts,
    icon: Option<&ImageSurface>,
    look: &Look,
) -> Option<(ImageSurface, Vec<(Rectangle<f64, Logical>, Spot)>)> {
    let facts = &without_hidden(facts, &look.hidden);
    // Laid out once to learn how tall it is, then drawn.
    let probe = Context::new(ImageSurface::create(Format::ARgb32, 1, 1).ok()?).ok()?;
    let (width, height) = Pen::new(&probe, look, icon).panel(facts);
    let surface = text::canvas(width, height).ok()?;
    let cr = Context::new(&surface).ok()?;
    let (r, g, b, tint) = crate::glass::tint(look.dark, Tier::Menu);
    // Without glass behind it, the panel is its colour at the opacity chosen.
    let a = if look.glass { tint } else { look.opacity };
    text::rounded_rectangle(
        &cr,
        0.0,
        0.0,
        f64::from(width),
        f64::from(height),
        f64::from(RADIUS),
    );
    cr.set_source_rgba(r, g, b, a);
    let _ = cr.fill();
    let mut pen = Pen::new(&cr, look, icon);
    pen.panel(facts);
    let spots = std::mem::take(&mut pen.spots);
    drop(pen);
    drop(cr);
    Some((surface, spots))
}

/// Draws the panel from the top down, noting where each value to copy is.
struct Pen<'a> {
    cr: &'a Context,
    look: &'a Look,
    icon: Option<&'a ImageSurface>,
    colors: menubar::Colors,
    width: f64,
    pad: f64,
    gap: f64,
    icon_side: f64,
    y: f64,
    spots: Vec<(Rectangle<f64, Logical>, Spot)>,
    /// How many values to copy have been drawn, to know the one the pointer is on.
    copyable: usize,
}

impl<'a> Pen<'a> {
    fn new(cr: &'a Context, look: &'a Look, icon: Option<&'a ImageSurface>) -> Self {
        let pad = if look.compact { COMPACT_PADDING } else { PADDING };
        Self {
            cr,
            look,
            icon,
            colors: menubar::colors(look.dark),
            width: f64::from(look.width),
            pad,
            gap: if look.compact { COMPACT_GAP } else { GAP },
            icon_side: if look.compact {
                COMPACT_ICON
            } else {
                f64::from(ICON)
            },
            y: pad,
            spots: Vec::new(),
            copyable: 0,
        }
    }

    fn inner(&self) -> f64 {
        self.width - self.pad * 2.0
    }

    fn shows(&self, section: &str) -> bool {
        !self.look.hidden.iter().any(|hidden| hidden == section)
    }

    fn layout(&self, font: &str, text: &str, width: f64) -> pango::Layout {
        let layout = text::new_layout(self.cr);
        let mut font = text::font(font);
        font.set_size((f64::from(font.size()) * self.look.font_scale).round() as i32);
        layout.set_font_description(Some(&font));
        layout.set_text(text);
        layout.set_width((width * f64::from(pango::SCALE)) as i32);
        layout.set_wrap(pango::WrapMode::WordChar);
        layout
    }

    fn ink(&self, (r, g, b): Rgb) {
        self.cr.set_source_rgb(r, g, b);
    }

    /// Text at `x` and `y`, `width` wide, returning its height; a value to copy when
    /// `copy` is given.
    fn text(&mut self, layout: &pango::Layout, x: f64, y: f64, color: Rgb, copy: Option<&str>) -> f64 {
        let (ink, logical) = layout.pixel_extents();
        let height = f64::from(logical.height());
        if let Some(value) = copy {
            let left = x + f64::from(ink.x().min(logical.x()));
            let width = f64::from(ink.width().max(1));
            self.spot(
                Rectangle::new((left - 3.0, y - 1.0).into(), (width + 6.0, height + 2.0).into()),
                value,
            );
        }
        self.ink(color);
        self.cr.move_to(x, y);
        pangocairo::functions::show_layout(self.cr, layout);
        height
    }

    /// A value to copy at `rect`: with Alt held, the one under the pointer is marked, and
    /// one copied has a check after it.
    fn spot(&mut self, rect: Rectangle<f64, Logical>, value: &str) {
        let hot = self.look.locked && self.look.hot == Some(self.copyable);
        self.copyable += 1;
        if hot {
            let (r, g, b) = self.colors.ink;
            self.cr.set_source_rgba(r, g, b, 0.10);
            text::rounded_rectangle(self.cr, rect.loc.x, rect.loc.y, rect.size.w, rect.size.h, 4.0);
            let _ = self.cr.fill();
        }
        let copied = self.look.copied.as_deref() == Some(value);
        let mark = match (copied, hot) {
            (true, _) => text::named_icon("object-select-symbolic", "", 12, GREEN),
            (false, true) => text::named_icon("edit-copy-symbolic", "", 12, self.colors.ink_3),
            _ => None,
        };
        if let Some(mark) = mark {
            let x = (rect.loc.x + rect.size.w + 2.0).min(self.width - self.pad - 12.0);
            let _ = self.cr.save();
            self.cr.translate(x, rect.loc.y + (rect.size.h - 12.0) / 2.0);
            stacks::paint_fitted(self.cr, &mark, 12.0, 1.0);
            let _ = self.cr.restore();
        }
        self.spots.push((rect, Spot::Copy(value.to_string())));
    }

    fn rule(&mut self) {
        self.y += self.gap;
        let (r, g, b) = self.colors.ink;
        self.cr.set_source_rgba(r, g, b, 0.12);
        self.cr.rectangle(self.pad, self.y, self.inner(), 1.0);
        let _ = self.cr.fill();
        self.y += 1.0 + self.gap;
    }

    /// Lays out and draws the whole panel, returning its size.
    fn panel(&mut self, facts: &Facts) -> (i32, i32) {
        if self.look.style == HoverStyle::Tooltip {
            return self.tooltip(facts);
        }
        self.header(facts);
        if self.shows("type") {
            if let Some(block) = &facts.block {
                self.rule();
                self.block(block);
            }
        }
        // A picture's frame goes beside the dates, or at the end of its block without them.
        let frame = facts.frame.filter(|_| self.shows("type"));
        let dates = self.shows("dates") && !facts.dates.is_empty();
        if let (Some(frame), false) = (frame, dates) {
            self.y += self.gap;
            let y = self.y;
            self.y += self.frame(frame, self.pad, y);
        }
        if self.shows("source") {
            if let Some(source) = &facts.source {
                self.rule();
                self.block(source);
            }
        }
        if dates {
            self.rule();
            match frame {
                Some(frame) => {
                    let y = self.y;
                    let drawn = self.frame(frame, self.pad, y);
                    let x = self.pad + 150.0;
                    let width = self.width - self.pad - x;
                    let mut bottom = y;
                    for field in &facts.dates {
                        bottom += self.field(field, x, bottom, width) + 8.0;
                    }
                    self.y = (y + drawn).max(bottom - 8.0);
                }
                None => self.fields(&facts.dates, 2),
            }
        }
        if self.shows("file") && !facts.file.is_empty() {
            self.rule();
            self.fields(&facts.file, 2);
        }
        if self.shows("location") {
            self.rule();
            self.location(facts);
        }
        if self.shows("notes") {
            self.rule();
            self.note();
        }
        (self.width as i32, (self.y + self.pad).ceil() as i32)
    }

    /// As Windows' tooltip says it: the name, then the kind, size and date, a line each.
    fn tooltip(&mut self, facts: &Facts) -> (i32, i32) {
        const PAD: f64 = 8.0;
        let lines: Vec<String> = [
            Some(format!("類型：{}", facts.kind)),
            facts.size.as_ref().map(|size| format!("大小：{size}")),
            facts.modified.as_ref().map(|date| format!("修改日期：{date}")),
        ]
        .into_iter()
        .flatten()
        .collect();
        let name = self.layout("Semibold 11", &facts.name, self.width - PAD * 2.0);
        let body = self.layout("10.5", &lines.join("\n"), self.width - PAD * 2.0);
        let width = name.pixel_size().0.max(body.pixel_size().0);
        let mut y = PAD;
        y += self.text(&name, PAD, y, self.colors.ink, None) + 2.0;
        y += self.text(&body, PAD, y, self.colors.ink_3, None);
        (width + PAD as i32 * 2, (y + PAD).ceil() as i32)
    }

    fn header(&mut self, facts: &Facts) {
        let icon = self.icon.filter(|_| self.shows("icon"));
        let icon_side = if icon.is_some() { self.icon_side } else { 0.0 };
        let x = if icon.is_some() {
            self.pad + icon_side + 10.0
        } else {
            self.pad
        };
        let width = self.width - self.pad - x;
        if let Some(icon) = icon {
            let _ = self.cr.save();
            self.cr.translate(self.pad, self.y);
            stacks::paint_fitted(self.cr, icon, self.icon_side, 1.0);
            let _ = self.cr.restore();
        }
        let top = self.y;
        let mut y = top;
        let name = self.layout("Semibold 12.5", &facts.name, width);
        y += self.text(&name, x, y, self.colors.ink, Some(&facts.name)) + 2.0;
        let details = self.layout("10.5", &facts.details, width);
        y += self.text(&details, x, y, self.colors.ink_3, Some(&facts.details));
        if self.shows("tags") && !facts.tags.is_empty() {
            y += 3.0;
            let mut tag_x = x;
            for tag in &facts.tags {
                let layout = self.layout("10", tag, width);
                let (tag_width, tag_height) = layout.pixel_size();
                if tag_x + 11.0 + f64::from(tag_width) > self.width - self.pad && tag_x > x {
                    tag_x = x;
                    y += f64::from(tag_height) + 2.0;
                }
                let (r, g, b) = self.colors.ink_3;
                self.cr.set_source_rgb(r, g, b);
                self.cr.arc(
                    tag_x + 3.5,
                    y + f64::from(tag_height) / 2.0,
                    3.5,
                    0.0,
                    std::f64::consts::TAU,
                );
                let _ = self.cr.fill();
                self.text(&layout, tag_x + 11.0, y, self.colors.ink_3, None);
                tag_x += 11.0 + f64::from(tag_width) + 10.0;
            }
            y += self.layout("10", "標", width).pixel_size().1 as f64;
        }
        self.y = y.max(top + icon_side);
    }

    fn block(&mut self, block: &Block) {
        let mut first = true;
        for part in &block.parts {
            if !first {
                self.y += 8.0;
            }
            first = false;
            match part {
                Part::Title {
                    title,
                    points,
                    accent,
                    subtitle,
                } => {
                    let inner = self.inner();
                    let lines = [
                        title
                            .as_ref()
                            .map(|title| (format!("Semibold {points}"), title, self.colors.ink)),
                        accent
                            .as_ref()
                            .map(|accent| ("12".to_string(), accent, block.color)),
                        subtitle
                            .as_ref()
                            .map(|subtitle| ("11".to_string(), subtitle, self.colors.ink_3)),
                    ];
                    for (font, line, color) in lines.into_iter().flatten() {
                        let layout = self.layout(&font, line, inner);
                        self.y += self.text(&layout, self.pad, self.y, color, Some(line)) + 1.0;
                    }
                }
                Part::Hero {
                    value,
                    unit,
                    trailing,
                    small,
                } => self.hero(
                    value.as_deref(),
                    unit.as_deref(),
                    trailing.as_deref(),
                    *small,
                    block.color,
                ),
                Part::Specs(specs) => self.specs(specs, block.color),
                Part::Caption(caption) => {
                    let layout = self.layout("10.5", caption, self.inner());
                    self.y += self.text(&layout, self.pad, self.y, self.colors.ink_3, Some(caption));
                }
                Part::Sample(png) => {
                    // Drawn in the panel's ink through the sample's coverage, narrowed to fit.
                    if let Ok(sample) = ImageSurface::create_from_png(&mut png.as_slice()) {
                        let scale = f64::from(text::picture_scale());
                        let points = f64::from(sample.width()) / scale;
                        let fit = (self.inner() / points).min(1.0) / scale;
                        let _ = self.cr.save();
                        self.cr.translate(self.pad, self.y);
                        self.cr.scale(fit, fit);
                        self.ink(self.colors.ink);
                        let _ = self.cr.mask_surface(&sample, 0.0, 0.0);
                        let _ = self.cr.restore();
                        self.y += f64::from(sample.height()) * fit;
                    }
                }
                Part::Segments(segments) => self.segments(segments, block.color),
                Part::Sizes(sizes) => self.sizes(sizes, block.color),
                Part::Fields(fields, columns) => self.fields(fields, *columns),
                Part::Source { host, rest, address } => {
                    let inner = self.inner();
                    let label = self.layout("10", "下載自", inner);
                    self.y += self.text(&label, self.pad, self.y, self.colors.ink_3, None) + 2.0;
                    let layout = self.layout("Semibold 14", host, inner);
                    self.y += self.text(&layout, self.pad, self.y, block.color, Some(host)) + 1.0;
                    let layout = self.layout("10.5", rest, inner);
                    layout.set_height(-1);
                    layout.set_ellipsize(pango::EllipsizeMode::Middle);
                    self.y += self.text(&layout, self.pad, self.y, self.colors.ink_3, Some(address));
                }
            }
        }
    }

    /// One large figure with its unit after it and a note at the right, on one baseline.
    fn hero(
        &mut self,
        value: Option<&str>,
        unit: Option<&str>,
        trailing: Option<&str>,
        small: bool,
        color: Rgb,
    ) {
        let inner = self.inner();
        let big = if small { "Semibold 15" } else { "Semibold 26" };
        let value = value.map(|value| self.layout(big, value, inner));
        let baseline = |layout: &pango::Layout| f64::from(layout.baseline()) / f64::from(pango::SCALE);
        let top = self.y;
        let line = value.as_ref().map_or(0.0, baseline);
        let mut x = self.pad;
        let mut height = 0.0;
        if let Some(layout) = &value {
            layout.set_width(-1);
            let copied = layout.text().to_string();
            height = self.text(layout, x, top, color, Some(&copied));
            x += f64::from(layout.pixel_size().0) + 6.0;
        }
        if let Some(unit) = unit {
            let layout = self.layout("11.5", unit, inner);
            layout.set_width(-1);
            let y = top + (line - baseline(&layout)).max(0.0);
            height = f64::max(
                height,
                y - top + self.text(&layout, x, y, self.colors.ink_3, None),
            );
        }
        if let Some(trailing) = trailing {
            let layout = self.layout("11.5", trailing, inner);
            layout.set_width(-1);
            let width = f64::from(layout.pixel_size().0);
            let y = top + (line - baseline(&layout)).max(0.0);
            height = f64::max(
                height,
                y - top
                    + self.text(
                        &layout,
                        self.width - self.pad - width,
                        y,
                        self.colors.ink,
                        Some(trailing),
                    ),
            );
        }
        self.y += height;
    }

    /// Figures side by side, each over its label.
    fn specs(&mut self, specs: &[Spec], color: Rgb) {
        let column = (self.inner() - 4.0 * (specs.len() as f64 - 1.0)) / specs.len() as f64;
        let mut tallest: f64 = 0.0;
        for (index, spec) in specs.iter().enumerate() {
            let x = self.pad + index as f64 * (column + 4.0);
            // Text too wide for its column is set smaller, and at the smallest on two lines.
            let fitted = |text: &str, sizes: &[f64]| {
                let set = |size: f64, lines: i32| {
                    let layout = self.layout(&format!("Semibold {size}"), text, column);
                    layout.set_alignment(pango::Alignment::Center);
                    layout.set_height(-lines);
                    layout.set_ellipsize(pango::EllipsizeMode::End);
                    layout
                };
                let smallest = sizes[sizes.len() - 1];
                sizes
                    .iter()
                    .map(|size| set(*size, 1))
                    .find(|layout| !layout.is_ellipsized())
                    .unwrap_or_else(|| set(smallest, 2))
            };
            let value = fitted(&spec.value, &[15.0, 13.0, 11.0]);
            let label = fitted(&spec.label, &[8.5, 7.5]);
            let ink = if spec.colored { color } else { self.colors.ink };
            let mut y = self.y;
            y += self.text(&value, x, y, ink, Some(&spec.value)) + 1.0;
            y += self.text(&label, x, y, self.colors.ink_3, None);
            tallest = tallest.max(y - self.y);
        }
        self.y += tallest;
    }

    fn bar(&self, x: f64, y: f64, width: f64, height: f64, (r, g, b, a): (f64, f64, f64, f64)) {
        if width <= 0.0 {
            return;
        }
        self.cr.set_source_rgba(r, g, b, a);
        text::rounded_rectangle(self.cr, x, y, width, height, (height / 2.0).min(2.0));
        let _ = self.cr.fill();
    }

    fn shade(&self, shade: Shade, color: Rgb) -> (f64, f64, f64, f64) {
        let (r, g, b) = self.colors.ink;
        match shade {
            Shade::Accent => (color.0, color.1, color.2, 1.0),
            Shade::Fill => (r, g, b, 0.35),
            Shade::Track => (r, g, b, 0.12),
        }
    }

    fn segments(&mut self, segments: &[(f64, Shade)], color: Rgb) {
        let total: f64 = segments.iter().map(|(amount, _)| amount).sum();
        let room = self.inner() - 2.0 * (segments.len() as f64 - 1.0);
        let mut x = self.pad;
        for (amount, shade) in segments {
            let width = (room * amount / total.max(1.0)).max(1.0);
            self.bar(x, self.y, width, 4.0, self.shade(*shade, color));
            x += width + 2.0;
        }
        self.y += 4.0;
    }

    /// Sizes as bars as long as each is of the largest, between its label and its size.
    fn sizes(&mut self, sizes: &[(String, u64, Shade)], color: Rgb) {
        let largest = sizes.iter().map(|(_, size, _)| *size).max().unwrap_or(0).max(1) as f64;
        let inner = self.inner();
        for (index, (label, size, shade)) in sizes.iter().enumerate() {
            if index > 0 {
                self.y += 6.0;
            }
            let label = self.layout("10.5", label, 84.0);
            label.set_height(-1);
            label.set_ellipsize(pango::EllipsizeMode::End);
            let height = self.text(&label, self.pad, self.y, self.colors.ink_3, None);
            let text = fileinfo::size_text(*size);
            let figure = self.layout("Medium 11", &text, 70.0);
            figure.set_alignment(pango::Alignment::Right);
            self.text(
                &figure,
                self.pad + inner - 70.0,
                self.y,
                self.colors.ink,
                Some(&text),
            );
            let (bar_x, bar_width) = (self.pad + 94.0, inner - 94.0 - 80.0);
            let middle = self.y + height / 2.0 - 1.5;
            self.bar(bar_x, middle, bar_width, 3.0, self.shade(Shade::Track, color));
            self.bar(
                bar_x,
                middle,
                bar_width * *size as f64 / largest,
                3.0,
                self.shade(*shade, color),
            );
            self.y += height;
        }
    }

    /// A label over its value, `width` wide, returning its height.
    fn field(&mut self, field: &Field, x: f64, y: f64, width: f64) -> f64 {
        let label = self.layout("10", &field.label, width);
        let mut height = self.text(&label, x, y, self.colors.ink_3, None) + 2.0;
        let value = self.layout("11.5", &field.value, width);
        height += self.text(&value, x, y + height, self.colors.ink, Some(&field.value));
        height
    }

    /// Fields `columns` to a row, those too long for a column a row each.
    fn fields(&mut self, fields: &[Field], columns: usize) {
        let inner = self.inner();
        let column = (inner - 12.0 * (columns as f64 - 1.0)) / columns as f64;
        let mut rows: Vec<Vec<&Field>> = Vec::new();
        let mut row = Vec::new();
        for field in fields {
            if field.wide {
                if !row.is_empty() {
                    rows.push(std::mem::take(&mut row));
                }
                rows.push(vec![field]);
                continue;
            }
            row.push(field);
            if row.len() == columns {
                rows.push(std::mem::take(&mut row));
            }
        }
        if !row.is_empty() {
            rows.push(row);
        }
        for (index, row) in rows.iter().enumerate() {
            if index > 0 {
                self.y += 10.0;
            }
            let mut tallest: f64 = 0.0;
            for (at, field) in row.iter().enumerate() {
                let (x, width) = if field.wide {
                    (self.pad, inner)
                } else {
                    (self.pad + at as f64 * (column + 12.0), column)
                };
                tallest = tallest.max(self.field(field, x, self.y, width));
            }
            self.y += tallest;
        }
    }

    /// A picture's proportions as a frame, its width over it and its height beside it,
    /// returning how tall it is.
    fn frame(&mut self, (width, height): (u64, u64), x: f64, y: f64) -> f64 {
        let scale = f64::min(104.0 / width as f64, 78.0 / height as f64);
        let (box_w, box_h) = (
            (width as f64 * scale).max(36.0),
            (height as f64 * scale).max(24.0),
        );
        let box_x = x + 32.0;
        let measure = |pen: &Self, text: &str| {
            let layout = pen.layout("Semibold 10", text, 200.0);
            layout.set_width(-1);
            layout
        };
        let (r, g, b) = self.colors.ink_3;
        let top = measure(self, &width.to_string());
        let (top_w, top_h) = top.pixel_size();
        let (top_w, top_h) = (f64::from(top_w), f64::from(top_h));
        self.cr.set_source_rgba(r, g, b, 0.6);
        self.cr
            .rectangle(box_x, y + top_h / 2.0, (box_w - top_w) / 2.0 - 4.0, 1.0);
        self.cr.rectangle(
            box_x + (box_w + top_w) / 2.0 + 4.0,
            y + top_h / 2.0,
            (box_w - top_w) / 2.0 - 4.0,
            1.0,
        );
        let _ = self.cr.fill();
        self.text(&top, box_x + (box_w - top_w) / 2.0, y, self.colors.ink, None);
        let box_y = y + top_h + 4.0;
        let side = measure(self, &height.to_string());
        let (side_w, side_h) = side.pixel_size();
        let (side_w, side_h) = (f64::from(side_w), f64::from(side_h));
        self.cr.set_source_rgba(r, g, b, 0.6);
        self.cr
            .rectangle(x + 14.0, box_y, 1.0, (box_h - side_h) / 2.0 - 3.0);
        self.cr.rectangle(
            x + 14.0,
            box_y + (box_h + side_h) / 2.0 + 3.0,
            1.0,
            (box_h - side_h) / 2.0 - 3.0,
        );
        let _ = self.cr.fill();
        self.text(
            &side,
            x + 14.5 - side_w / 2.0,
            box_y + (box_h - side_h) / 2.0,
            self.colors.ink,
            None,
        );
        self.cr.set_source_rgba(r, g, b, 0.6);
        text::rounded_rectangle(self.cr, box_x + 0.5, box_y + 0.5, box_w - 1.0, box_h - 1.0, 2.0);
        self.cr.set_line_width(1.0);
        let _ = self.cr.stroke();
        let megapixels = format!("{:.1} MP", width as f64 * height as f64 / 1e6);
        let inside = self.layout("10", &megapixels, box_w);
        inside.set_alignment(pango::Alignment::Center);
        let inside_h = f64::from(inside.pixel_size().1);
        self.text(
            &inside,
            box_x,
            box_y + (box_h - inside_h) / 2.0,
            self.colors.ink_3,
            None,
        );
        let dimensions = format!("{width} × {height}");
        self.spot(
            Rectangle::new((x, y).into(), (box_x + box_w - x, box_y + box_h - y).into()),
            &dimensions,
        );
        box_y + box_h - y
    }

    /// The folder it is in, its volume and how full that is.
    fn location(&mut self, facts: &Facts) {
        let inner = self.inner();
        let path = facts.path.to_string_lossy();
        let folder = self.layout("10.5", &facts.folder, inner);
        self.y += self.text(&folder, self.pad, self.y, self.colors.ink_3, Some(&path)) + 6.0;
        let mut height: f64 = 0.0;
        if !facts.volume.is_empty() {
            let volume = self.layout("10.5", &facts.volume, inner);
            volume.set_width(-1);
            height = self.text(&volume, self.pad, self.y, self.colors.ink_3, Some(&facts.volume));
        }
        if let Some(available) = &facts.available {
            let layout = self.layout("10.5", available, inner);
            layout.set_width(-1);
            let width = f64::from(layout.pixel_size().0);
            height = height.max(self.text(
                &layout,
                self.pad + inner - width,
                self.y,
                self.colors.ink_3,
                Some(available),
            ));
        }
        self.y += height;
        if let Some(used) = facts.used {
            self.y += 6.0;
            let ink = self.colors.ink;
            self.bar(self.pad, self.y, inner, 3.0, (ink.0, ink.1, ink.2, 0.12));
            self.bar(
                self.pad,
                self.y,
                inner * used.clamp(0.0, 1.0),
                3.0,
                (ink.0, ink.1, ink.2, 0.35),
            );
            self.y += 3.0;
        }
    }

    /// The note's field: its text, or what to type there, and the caret while it takes
    /// keys, with what the input method composes underlined.
    fn note(&mut self) {
        let note = &self.look.note;
        let typing = note.caret.is_some();
        let inner = self.inner();
        let mut shown = note.text.clone();
        let caret = note.caret.unwrap_or(shown.len());
        shown.insert_str(caret, &note.preedit);
        let empty = shown.is_empty();
        let layout = self.layout("10.5", if empty { PLACEHOLDER } else { &shown }, inner - 18.0);
        layout.set_height(-NOTE_LINES);
        layout.set_ellipsize(pango::EllipsizeMode::End);
        if !note.preedit.is_empty() {
            let attributes = pango::AttrList::new();
            let mut underline = pango::AttrInt::new_underline(pango::Underline::Single);
            underline.set_start_index(caret as u32);
            underline.set_end_index((caret + note.preedit.len()) as u32);
            attributes.insert(underline);
            layout.set_attributes(Some(&attributes));
        }
        let text_height = f64::from(layout.pixel_size().1);
        let (x, y, w, h) = (self.pad, self.y, inner, text_height + 10.0);
        let (r, g, b) = self.colors.ink;
        if typing {
            let [or, og, ob, _] = OUTLINE_COLOR.map(f64::from);
            let (sr, sg, sb) = self.colors.sheet;
            self.cr.set_source_rgba(sr, sg, sb, 0.85);
            text::rounded_rectangle(self.cr, x, y, w, h, 6.0);
            let _ = self.cr.fill();
            self.cr.set_source_rgba(or, og, ob, 0.7);
        } else {
            self.cr.set_source_rgba(r, g, b, 0.12);
        }
        text::rounded_rectangle(self.cr, x + 0.5, y + 0.5, w - 1.0, h - 1.0, 6.0);
        self.cr.set_line_width(1.0);
        let _ = self.cr.stroke();
        let color = if empty { self.colors.ink_3 } else { self.colors.ink };
        self.text(&layout, x + 9.0, y + 5.0, color, None);
        if typing {
            let at = if empty { 0 } else { caret + note.preedit.len() };
            let (strong, _) = layout.cursor_pos(at as i32);
            let scale = f64::from(pango::SCALE);
            self.cr.set_source_rgb(r, g, b);
            self.cr.rectangle(
                x + 9.0 + f64::from(strong.x()) / scale,
                y + 5.0 + f64::from(strong.y()) / scale,
                1.5,
                f64::from(strong.height()) / scale,
            );
            let _ = self.cr.fill();
        }
        self.spots
            .push((Rectangle::new((x, y).into(), (w, h).into()), Spot::Note));
        self.y += h;
        if let Some(error) = &note.error {
            let layout = self.layout("10", error, inner);
            self.y += 4.0 + self.text(&layout, self.pad, self.y + 4.0, RED, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typing(text: &str) -> Note {
        Note {
            text: text.into(),
            caret: Some(text.len()),
            ..Note::default()
        }
    }

    #[test]
    fn a_note_takes_keys_on_its_lines() {
        let mut note = typing("新潟");
        note.insert("花火");
        assert!(note.key(Keysym::Return));
        note.insert("8 月");
        assert!(note.key(Keysym::BackSpace));
        assert_eq!(note.text, "新潟花火\n8 ");
        assert!(note.key(Keysym::Home));
        assert_eq!(note.caret, Some("新潟花火\n".len()));
        assert!(note.key(Keysym::Left));
        assert!(note.key(Keysym::Delete));
        assert_eq!(note.text, "新潟花火8 ");
        assert!(!note.key(Keysym::a));
    }

    #[test]
    fn hidden_kinds_and_fields_are_left_out() {
        let (photo, _) = fileinfo::samples().swap_remove(1);
        let hidden = ["照片/焦距", "照片/拍攝", "file/模式"].map(String::from);
        let kept = without_hidden(&photo, &hidden);
        let labels: Vec<&str> = kept
            .block
            .iter()
            .flat_map(|block| &block.parts)
            .flat_map(|part| match part {
                Part::Specs(specs) => specs.iter().map(|spec| spec.label.as_str()).collect(),
                Part::Fields(fields, _) => fields.iter().map(|field| field.label.as_str()).collect(),
                _ => Vec::new(),
            })
            .collect();
        assert!(labels.contains(&"光圈") && labels.contains(&"位置"));
        assert!(!labels.contains(&"焦距") && !labels.contains(&"拍攝"));
        assert!(kept.file.iter().all(|field| field.label != "模式"));
        assert!(without_hidden(&photo, &["照片".into()]).block.is_none());
    }

    #[test]
    fn a_note_not_being_typed_takes_no_keys() {
        let mut note = Note {
            text: "x".into(),
            ..Note::default()
        };
        assert!(!note.key(Keysym::BackSpace));
        assert_eq!(note.text, "x");
    }
}
