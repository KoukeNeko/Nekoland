//! Pictures in a pane, from the kitty graphics protocol and Sixel, as Nekominal shows them. VTE is built here without
//! Sixel and knows nothing of kitty's, so a picture is put on the screen as cells of its own: where it lands, the
//! screen is fed rows of a placeholder character whose colour names the placement and the row of the picture it holds.
//! VTE then keeps those cells as it keeps text, scrolling, clearing and swapping screens with them, and the painter
//! finds them in the rows in view and draws that part of the picture over them. A program that writes over a picture
//! writes over its cells, so the picture goes with them, as Nekominal's do.

use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    rc::Rc,
};

use base64::{engine::general_purpose::STANDARD, Engine};
use gtk::{gdk, glib, graphene, pango, prelude::*};
use unicode_width::UnicodeWidthChar;
use vte4::prelude::*;

use super::{
    graphics_stream::Piece,
    kitty_graphics::{self, Command, Decoded, Failure, LENIENT_BASE64},
    markup::html_to_markup,
    screen::Painter,
    screen_rows, sixel,
};

/// The character a picture's cells hold: kitty's own placeholder, which no font draws.
const PLACEHOLDER: char = '\u{10EEEE}';
/// A placeholder's colour is `tag << ROW_BITS | row`: 12 bits each, 24 in all.
const ROW_BITS: u32 = 12;
const ROW_MASK: u32 = (1 << ROW_BITS) - 1;
const TAG_LIMIT: u32 = 1 << 12;
const COLOUR_BITS: u32 = 8;
const COLOUR_MASK: u32 = (1 << COLOUR_BITS) - 1;
/// Pictures kept to be placed again (`a=p`), the oldest let go first.
const MOST_STORED: usize = 64;
/// What the placements on the screen may hold in pixels, the oldest let go first.
const MOST_PLACED_BYTES: usize = 256 * 1024 * 1024;
const BYTES_PER_PIXEL: usize = 4;
/// The decoded bytes a transmission in chunks may gather: what its base64 may be, decoded.
const MOST_GATHERED_BYTES: usize = kitty_graphics::MAX_TRANSFER / 4 * 3;
/// Ids the terminal gives pictures a program numbered (`I=`) or left unnamed start here, above those programs choose.
const FIRST_OWN_ID: u32 = 1 << 31;
const OK: u8 = 0;
const ACTION_QUERY: u8 = b'q';
const ACTION_TRANSMIT: u8 = b't';
const ACTION_TRANSMIT_AND_PLACE: u8 = b'T';
const ACTION_PLACE: u8 = b'p';
const ACTION_DELETE: u8 = b'd';
/// XTSMGRAPHICS: its items, the answers' status, and the colour registers a Sixel has.
const ITEM_COLOURS: &str = "1";
const ITEM_SIXEL_SIZE: &str = "2";
const STATUS_SUCCESS: u8 = 0;
const STATUS_UNKNOWN_ITEM: u8 = 1;
const SIXEL_COLOURS: u32 = 256;
/// DA1's answer and the attribute that says Sixel.
const DEVICE_ATTRIBUTES_START: &str = "\x1b[?";
const DEVICE_ATTRIBUTES_END: &str = "c";
const SIXEL_ATTRIBUTE: &str = "4";

/// The cells of the screen in device pixels, and how many to a row.
#[derive(Clone, Copy, Debug)]
pub struct Cells {
    pub width: u32,
    pub height: u32,
    pub scale: f32,
    pub columns: u32,
}

impl Cells {
    pub fn of(screen: &vte4::Terminal) -> Self {
        let scale = screen.scale_factor().max(1);
        Self {
            width: u32::try_from(screen.char_width() * i64::from(scale)).unwrap_or(1).max(1),
            height: u32::try_from(screen.char_height() * i64::from(scale)).unwrap_or(1).max(1),
            scale: scale as f32,
            columns: u32::try_from(screen.column_count()).unwrap_or(1).max(1),
        }
    }
}

/// Where the cursor goes once a picture is on the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CursorAfter {
    /// kitty's `C=1`.
    Stays,
    /// kitty's way: right of the picture on its last row.
    RightOfLastRow,
    /// xterm's way for a Sixel: the row below the picture, in the column it started at.
    BelowStart,
}

/// What a program calls a placement by: its picture's id, its own id, its depth. A Sixel's are all 0.
#[derive(Clone, Copy, Debug, Default)]
struct Named {
    image: u32,
    placement: u32,
    z: i32,
}

/// How large a placement is drawn and where in its first cell, in device pixels, and where the cursor goes after it.
struct Layout {
    size: (u32, u32),
    offset: (u32, u32),
    after: CursorAfter,
}

/// One picture on the screen: the part of it shown, ready to draw.
struct Placement {
    texture: gdk::Texture,
    named: Named,
    /// Logical pixels: where in its first cell the picture starts, and how large it is drawn.
    offset: (f32, f32),
    size: (f32, f32),
    bytes: usize,
}

/// What a piece became: bytes for the screen, and what to answer the program.
#[derive(Default)]
pub struct Output {
    pub screen: Vec<u8>,
    pub reply: Vec<u8>,
}

#[derive(Default)]
pub struct Images {
    stored: HashMap<u32, Rc<Decoded>>,
    stored_age: VecDeque<u32>,
    numbers: HashMap<u32, u32>,
    /// A transmission in chunks (`m=1`) until its last one.
    pending: Option<(Command, Vec<u8>)>,
    placements: HashMap<u32, Placement>,
    placement_age: VecDeque<u32>,
    placed_bytes: usize,
    last_tag: u32,
    last_own_id: u32,
}

impl Images {
    /// One piece of a program's output, carried out on `screen`.
    pub fn take(&mut self, piece: Piece, screen: &vte4::Terminal) -> Output {
        match piece {
            Piece::Text(text) => Output { screen: text, reply: Vec::new() },
            Piece::Kitty(sequence) => self.take_kitty(&sequence, screen),
            Piece::Sixel(data) => Output { screen: self.take_sixel(&data, Cells::of(screen)), reply: Vec::new() },
            Piece::GraphicsAttributes(parameters) => {
                Output { screen: Vec::new(), reply: graphics_attributes_answer(&parameters) }
            }
        }
    }

    fn take_kitty(&mut self, sequence: &[u8], screen: &vte4::Terminal) -> Output {
        let (control, payload) = match sequence.iter().position(|byte| *byte == b';') {
            Some(split) => (&sequence[..split], &sequence[split + 1..]),
            None => (sequence, &[][..]),
        };
        let Some(command) = Command::parse(control) else {
            return Output::default();
        };
        let Some((mut command, payload)) = self.gather(command, payload) else {
            return Output::default();
        };
        self.name_by_number(&mut command);
        let outcome = self.run_kitty(&command, &payload, screen);
        let reply = match command.action {
            ACTION_DELETE => None,
            _ => kitty_graphics::reply(&command, outcome.as_ref().map(|_| ())),
        };
        Output { screen: outcome.unwrap_or_default(), reply: reply.unwrap_or_default() }
    }

    /// The whole command once its last chunk is in: the first chunk's keys and every chunk's payload, as one base64.
    /// Each chunk is decoded on its own, as kitty decodes them: chafa pads every chunk, so the chunks joined are not
    /// one base64.
    fn gather(&mut self, command: Command, payload: &[u8]) -> Option<(Command, Vec<u8>)> {
        let (first, mut gathered) = match self.pending.take() {
            Some(pending) => pending,
            None if command.more => (command.clone(), Vec::new()),
            None => return Some((command, payload.to_vec())),
        };
        let chunk = LENIENT_BASE64.decode(payload).ok()?;
        if gathered.len() + chunk.len() > MOST_GATHERED_BYTES {
            return None;
        }
        gathered.extend(chunk);
        if command.more {
            self.pending = Some((first, gathered));
            return None;
        }
        Some((first, STANDARD.encode(gathered).into_bytes()))
    }

    /// A picture a program numbered (`I=`) gets an id of the terminal's on its transmission, which the number then
    /// names.
    fn name_by_number(&mut self, command: &mut Command) {
        if command.number == 0 || command.id != 0 {
            return;
        }
        command.id = match command.action {
            ACTION_TRANSMIT | ACTION_TRANSMIT_AND_PLACE => {
                let id = self.own_id();
                self.numbers.insert(command.number, id);
                id
            }
            _ => self.numbers.get(&command.number).copied().unwrap_or(0),
        };
    }

    fn own_id(&mut self) -> u32 {
        self.last_own_id = self.last_own_id.wrapping_add(1);
        FIRST_OWN_ID | self.last_own_id
    }

    fn run_kitty(&mut self, command: &Command, payload: &[u8], screen: &vte4::Terminal) -> Result<Vec<u8>, Failure> {
        let places = matches!(command.action, ACTION_TRANSMIT_AND_PLACE | ACTION_PLACE);
        if places && command.placeholder {
            return kitty_graphics::fail("EINVAL", "Unicode placeholders are not supported");
        }
        match command.action {
            ACTION_QUERY => kitty_graphics::decode(command, payload).map(|_| Vec::new()),
            ACTION_TRANSMIT => {
                let decoded = kitty_graphics::decode(command, payload)?;
                let id = if command.id == 0 { self.own_id() } else { command.id };
                self.store(id, decoded);
                Ok(Vec::new())
            }
            ACTION_TRANSMIT_AND_PLACE => {
                let decoded = kitty_graphics::decode(command, payload)?;
                let id = if command.id == 0 { self.own_id() } else { command.id };
                self.store(id, decoded);
                self.place_kitty(command, id, Cells::of(screen))
            }
            ACTION_PLACE => self.place_kitty(command, command.id, Cells::of(screen)),
            ACTION_DELETE => {
                self.delete(command, screen);
                Ok(Vec::new())
            }
            _ => kitty_graphics::fail("EINVAL", "unsupported action"),
        }
    }

    fn store(&mut self, id: u32, decoded: Decoded) {
        if self.stored.insert(id, Rc::new(decoded)).is_none() {
            self.stored_age.push_back(id);
        }
        while self.stored.len() > MOST_STORED {
            let Some(oldest) = self.stored_age.pop_front() else {
                break;
            };
            self.stored.remove(&oldest);
        }
    }

    fn place_kitty(&mut self, command: &Command, id: u32, cells: Cells) -> Result<Vec<u8>, Failure> {
        let Some(decoded) = self.stored.get(&id).cloned() else {
            return kitty_graphics::fail("ENOENT", "no such image");
        };
        let shown = Crop::of(command, &decoded);
        let size = scaled_size(&shown, command, cells);
        let offset = (command.cell_x.min(cells.width - 1), command.cell_y.min(cells.height - 1));
        let after = if command.no_move { CursorAfter::Stays } else { CursorAfter::RightOfLastRow };
        if command.placement != 0 {
            self.remove_placements(|placed| placed.named.image == id && placed.named.placement == command.placement);
        }
        let named = Named { image: id, placement: command.placement, z: command.z };
        Ok(self.put(named, &decoded, &shown, Layout { size, offset, after }, cells))
    }

    fn take_sixel(&mut self, data: &[u8], cells: Cells) -> Vec<u8> {
        let Some(decoded) = sixel::decode(data) else {
            return Vec::new();
        };
        let shown = Crop::whole(&decoded);
        let layout = Layout { size: (decoded.width, decoded.height), offset: (0, 0), after: CursorAfter::BelowStart };
        self.put(Named::default(), &decoded, &shown, layout, cells)
    }

    /// Keeps a placement of `shown` laid out as `layout` says, and the bytes that put its cells on the screen.
    fn put(&mut self, named: Named, decoded: &Decoded, shown: &Crop, layout: Layout, cells: Cells) -> Vec<u8> {
        let Layout { size, offset, after } = layout;
        let columns = (offset.0 + size.0).div_ceil(cells.width).clamp(1, cells.columns);
        let rows = (offset.1 + size.1).div_ceil(cells.height).clamp(1, ROW_MASK);
        let placement = Placement {
            texture: shown.texture(decoded),
            named,
            offset: (offset.0 as f32 / cells.scale, offset.1 as f32 / cells.scale),
            size: (size.0 as f32 / cells.scale, size.1 as f32 / cells.scale),
            bytes: shown.width as usize * shown.height as usize * BYTES_PER_PIXEL,
        };
        let tag = self.next_tag();
        if let Some(replaced) = self.placements.remove(&tag) {
            self.placed_bytes -= replaced.bytes;
        }
        self.placed_bytes += placement.bytes;
        self.placements.insert(tag, placement);
        self.placement_age.push_back(tag);
        self.let_go_of_oldest();
        placeholder_bytes(tag, columns, rows, after)
    }

    fn next_tag(&mut self) -> u32 {
        self.last_tag = self.last_tag % (TAG_LIMIT - 1) + 1;
        self.last_tag
    }

    fn let_go_of_oldest(&mut self) {
        while self.placed_bytes > MOST_PLACED_BYTES && self.placements.len() > 1 {
            let Some(oldest) = self.placement_age.pop_front() else {
                break;
            };
            if let Some(placement) = self.placements.remove(&oldest) {
                self.placed_bytes -= placement.bytes;
            }
        }
    }

    fn remove_placements(&mut self, mut chosen: impl FnMut(&Placement) -> bool) {
        let tags: Vec<u32> =
            self.placements.iter().filter(|(_, placement)| chosen(placement)).map(|(tag, _)| *tag).collect();
        self.remove_tags(&tags);
    }

    fn remove_tags(&mut self, tags: &[u32]) {
        for tag in tags {
            if let Some(placement) = self.placements.remove(tag) {
                self.placed_bytes -= placement.bytes;
            }
        }
        self.placement_age.retain(|tag| self.placements.contains_key(tag));
    }

    /// `a=d`: the placements `d` picks go; an upper-case letter also lets go of the pictures themselves.
    fn delete(&mut self, command: &Command, screen: &vte4::Terminal) {
        let which = if command.delete == 0 { b'a' } else { command.delete };
        let frees = which.is_ascii_uppercase();
        match which.to_ascii_lowercase() {
            b'a' => self.remove_placements(|_| true),
            b'i' => {
                let (id, placement) = (command.id, command.placement);
                self.remove_placements(|placed| {
                    placed.named.image == id && (placement == 0 || placed.named.placement == placement)
                });
                self.free_if(frees, |stored| stored == id);
            }
            b'n' => {
                let id = self.numbers.get(&command.number).copied().unwrap_or(0);
                self.remove_placements(|placed| placed.named.image == id);
                self.free_if(frees, |stored| stored == id);
            }
            b'r' => {
                let range = command.x..=command.y;
                self.remove_placements(|placed| range.contains(&placed.named.image));
                self.free_if(frees, |stored| range.contains(&stored));
            }
            b'z' => self.remove_placements(|placed| placed.named.z == command.z),
            position => self.delete_at(position, command, screen),
        }
    }

    fn free_if(&mut self, frees: bool, chosen: impl Fn(u32) -> bool) {
        if frees {
            self.stored.retain(|id, _| !chosen(*id));
            self.stored_age.retain(|id| !chosen(*id));
        }
    }

    /// The deletes by cell: at the cursor (`c`), a cell (`p`, `q` with its depth), a column (`x`) or a row (`y`),
    /// cells counted from 1 on the live screen as kitty counts them.
    fn delete_at(&mut self, position: u8, command: &Command, screen: &vte4::Terminal) {
        let top = screen_rows::live_top(screen);
        let (cursor_column, cursor_row) = screen.cursor_position();
        let cursor = (usize::try_from(cursor_column).unwrap_or(0), cursor_row);
        let column = usize::try_from(command.x).unwrap_or(0).checked_sub(1);
        let row = i64::from(command.y).checked_sub(1).map(|row| top + row);
        let covers = |run: &Run, at_row: Option<i64>, at_column: Option<usize>| {
            at_row.is_none_or(|at_row| run.row == at_row) && at_column.is_none_or(|at| run.columns.contains(&at))
        };
        let runs = runs_in(screen, top, screen.row_count());
        let tags: Vec<u32> = runs
            .iter()
            .filter(|run| match position {
                b'c' => covers(run, Some(cursor.1), Some(cursor.0)),
                b'p' => row.is_some() && column.is_some() && covers(run, row, column),
                b'q' => {
                    row.is_some()
                        && column.is_some()
                        && covers(run, row, column)
                        && self.placements.get(&run.tag).is_some_and(|placed| placed.named.z == command.z)
                }
                b'x' => column.is_some() && covers(run, None, column),
                b'y' => row.is_some() && covers(run, row, None),
                _ => false,
            })
            .map(|run| run.tag)
            .collect();
        self.remove_tags(&tags);
    }
}

/// The part of a picture shown: kitty's `x, y, w, h`, kept inside the picture; 0 for `w` or `h` is the rest of it.
struct Crop {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

impl Crop {
    fn whole(decoded: &Decoded) -> Self {
        Self { x: 0, y: 0, width: decoded.width, height: decoded.height }
    }

    fn of(command: &Command, decoded: &Decoded) -> Self {
        let x = command.x.min(decoded.width.saturating_sub(1));
        let y = command.y.min(decoded.height.saturating_sub(1));
        let rest = (decoded.width - x, decoded.height - y);
        let width = if command.w == 0 { rest.0 } else { command.w.min(rest.0) };
        let height = if command.h == 0 { rest.1 } else { command.h.min(rest.1) };
        Self { x, y, width: width.max(1), height: height.max(1) }
    }

    fn texture(&self, decoded: &Decoded) -> gdk::Texture {
        let row_bytes = self.width as usize * BYTES_PER_PIXEL;
        let mut pixels = Vec::with_capacity(row_bytes * self.height as usize);
        for row in self.y..self.y + self.height {
            let start = (row as usize * decoded.width as usize + self.x as usize) * BYTES_PER_PIXEL;
            pixels.extend_from_slice(&decoded.rgba[start..start + row_bytes]);
        }
        gdk::MemoryTexture::new(
            self.width as i32,
            self.height as i32,
            gdk::MemoryFormat::R8g8b8a8,
            &glib::Bytes::from_owned(pixels),
            row_bytes,
        )
        .upcast()
    }
}

/// The size a placement is drawn at, in device pixels: scaled into `c` x `r` cells, one of them alone keeping the
/// picture's aspect, or as large as the picture.
fn scaled_size(shown: &Crop, command: &Command, cells: Cells) -> (u32, u32) {
    let fit_width = command.columns * cells.width;
    let fit_height = command.rows * cells.height;
    let aspect = |side: u32, along: u32, across: u32| (u64::from(side) * u64::from(across) / u64::from(along.max(1))) as u32;
    match (command.columns, command.rows) {
        (0, 0) => (shown.width, shown.height),
        (_, 0) => (fit_width, aspect(fit_width, shown.width, shown.height)),
        (0, _) => (aspect(fit_height, shown.height, shown.width), fit_height),
        _ => (fit_width, fit_height),
    }
}

/// The bytes that put a placement's cells on the screen at the cursor. Room for its rows is made first, scrolling as
/// text would; the cells are written with autowrap off so that a picture wider than the rest of the row stays on its
/// row; the cursor and the program's colours are put back before the cursor moves as `after` says.
fn placeholder_bytes(tag: u32, columns: u32, rows: u32, after: CursorAfter) -> Vec<u8> {
    let below = rows - 1;
    let mut bytes = Vec::new();
    if below > 0 {
        bytes.extend("\x1bD".repeat(below as usize).as_bytes());
        bytes.extend(format!("\x1b[{below}A").as_bytes());
    }
    bytes.extend(b"\x1b7\x1b[?7l");
    let cells = PLACEHOLDER.to_string().repeat(columns as usize);
    for row in 0..rows {
        if row > 0 {
            bytes.extend(format!("\x1b[{columns}D\x1b[B").as_bytes());
        }
        let colour = tag << ROW_BITS | row;
        let (red, green, blue) = (colour >> (2 * COLOUR_BITS), colour >> COLOUR_BITS & COLOUR_MASK, colour & COLOUR_MASK);
        // Concealed, so that VTE draws nothing for them: its box for a character no font has spills past the cell.
        bytes.extend(format!("\x1b[38;2;{red};{green};{blue};8m{cells}").as_bytes());
    }
    bytes.extend(b"\x1b[?7h\x1b8");
    let down = if below > 0 { format!("\x1b[{below}B") } else { String::new() };
    match after {
        CursorAfter::Stays => {}
        CursorAfter::RightOfLastRow => bytes.extend(format!("{down}\x1b[{columns}C").as_bytes()),
        CursorAfter::BelowStart => bytes.extend(format!("{down}\x1bD").as_bytes()),
    }
    bytes
}

/// The answer to XTSMGRAPHICS: what a Sixel may hold. Setting an item is taken and changes nothing, as in Nekominal.
fn graphics_attributes_answer(parameters: &str) -> Vec<u8> {
    let item = parameters.split(';').next().unwrap_or_default();
    let answer = match item {
        ITEM_COLOURS => format!("{STATUS_SUCCESS};{SIXEL_COLOURS}"),
        ITEM_SIXEL_SIZE => format!("{STATUS_SUCCESS};{0};{0}", kitty_graphics::MAX_IMAGE_SIDE),
        _ => format!("{STATUS_UNKNOWN_ITEM};{OK}"),
    };
    format!("\x1b[?{item};{answer}S").into_bytes()
}

/// VTE's answer to DA1 (`CSI ? 65 ; 1 ; 9 c`) with Sixel among the attributes, as programs look there for it; None
/// for anything else the screen sends.
pub fn with_sixel_attribute(answer: &str) -> Option<String> {
    let attributes = answer.strip_prefix(DEVICE_ATTRIBUTES_START)?.strip_suffix(DEVICE_ATTRIBUTES_END)?;
    let mut list: Vec<&str> = attributes.split(';').collect();
    let well_formed = list.iter().all(|attribute| !attribute.is_empty() && attribute.bytes().all(|byte| byte.is_ascii_digit()));
    if !well_formed || list.contains(&SIXEL_ATTRIBUTE) {
        return None;
    }
    // The first number is the conformance level; the attributes follow it.
    list.insert(1, SIXEL_ATTRIBUTE);
    Some(format!("{DEVICE_ATTRIBUTES_START}{}{DEVICE_ATTRIBUTES_END}", list.join(";")))
}

/// A row's run of a placement's cells: the row in the buffer, its columns, and which row of which placement it holds.
struct Run {
    row: i64,
    columns: std::ops::Range<usize>,
    tag: u32,
    picture_row: u32,
}

/// The placement cells of `count` rows from `first`.
fn runs_in(screen: &vte4::Terminal, first: i64, count: i64) -> Vec<Run> {
    (first..first + count).flat_map(|row| row_runs(screen, row)).collect()
}

fn row_runs(screen: &vte4::Terminal, row: i64) -> Vec<Run> {
    let end = screen.column_count();
    let (text, _) = screen.text_range_format(vte4::Format::Text, row, 0, row, end);
    if !text.is_some_and(|text| text.contains(PLACEHOLDER)) {
        return Vec::new();
    }
    let (html, _) = screen.text_range_format(vte4::Format::Html, row, 0, row, end);
    let Some(Ok((attributes, text, _))) = html.map(|html| pango::parse_markup(&html_to_markup(&html), '\0')) else {
        return Vec::new();
    };
    let colours = foreground_ranges(&attributes);
    let mut runs: Vec<Run> = Vec::new();
    let mut column = 0;
    for (index, char) in text.char_indices() {
        let width = char.width().unwrap_or(0);
        if char == PLACEHOLDER {
            if let Some(colour) = colours.iter().find(|(range, _)| range.contains(&index)).map(|(_, colour)| *colour) {
                extend_runs(&mut runs, row, column, colour);
            }
        }
        column += width;
    }
    runs
}

/// A placeholder cell joins the run left of it when it holds the same row of the same placement.
fn extend_runs(runs: &mut Vec<Run>, row: i64, column: usize, colour: u32) {
    let (tag, picture_row) = (colour >> ROW_BITS, colour & ROW_MASK);
    match runs.last_mut() {
        Some(last) if last.columns.end == column && last.tag == tag && last.picture_row == picture_row => {
            last.columns.end = column + 1;
        }
        _ => runs.push(Run { row, columns: column..column + 1, tag, picture_row }),
    }
}

/// The byte ranges of the markup's text and the 24-bit foreground each has.
fn foreground_ranges(attributes: &pango::AttrList) -> Vec<(std::ops::Range<usize>, u32)> {
    attributes
        .attributes()
        .into_iter()
        .filter(|attribute| attribute.type_() == pango::AttrType::Foreground)
        .filter_map(|attribute| {
            let range = attribute.start_index() as usize..attribute.end_index() as usize;
            let colour = attribute.downcast::<pango::AttrColor>().ok()?.color();
            let to_byte = |channel: u16| u32::from(channel >> COLOUR_BITS);
            let packed = to_byte(colour.red()) << (2 * COLOUR_BITS) | to_byte(colour.green()) << COLOUR_BITS | to_byte(colour.blue());
            Some((range, packed))
        })
        .collect()
}

/// The painter that draws the pictures in `images` over their cells.
pub fn painter(images: Rc<RefCell<Images>>) -> Painter {
    Box::new(move |screen, snapshot, draw_screen| paint(&images.borrow(), screen, snapshot, draw_screen))
}

fn paint(images: &Images, screen: &vte4::Terminal, snapshot: &gtk::Snapshot, draw_screen: &dyn Fn(&gtk::Snapshot)) {
    if images.placements.is_empty() {
        draw_screen(snapshot);
        return;
    }
    let first = screen_rows::first_shown(screen);
    let runs = runs_in(screen, first, screen.row_count());
    if runs.is_empty() {
        draw_screen(snapshot);
        return;
    }
    let (cell_width, cell_height) = (screen.char_width() as f32, screen.char_height() as f32);
    let area = |run: &Run| {
        graphene::Rect::new(
            run.columns.start as f32 * cell_width,
            (run.row - first) as f32 * cell_height,
            run.columns.len() as f32 * cell_width,
            cell_height,
        )
    };
    snapshot.push_mask(gtk::gsk::MaskMode::InvertedAlpha);
    for run in &runs {
        snapshot.append_color(&gdk::RGBA::BLACK, &area(run));
    }
    snapshot.pop();
    draw_screen(snapshot);
    snapshot.pop();
    let background = screen.color_background_for_draw();
    for run in &runs {
        let cells = area(run);
        snapshot.append_color(&background, &cells);
        let Some(placement) = images.placements.get(&run.tag) else {
            continue;
        };
        let picture = graphene::Rect::new(
            cells.x() + placement.offset.0,
            cells.y() - run.picture_row as f32 * cell_height + placement.offset.1,
            placement.size.0,
            placement.size.1,
        );
        snapshot.push_clip(&cells);
        snapshot.append_texture(&placement.texture, &picture);
        snapshot.pop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholder_rows_name_their_placement_and_row() {
        let bytes = String::from_utf8(placeholder_bytes(3, 2, 2, CursorAfter::RightOfLastRow)).unwrap();
        let first = 3 << ROW_BITS;
        assert!(bytes.starts_with("\x1bD\x1b[1A\x1b7\x1b[?7l"));
        assert!(bytes.contains(&format!("\x1b[38;2;0;{};0;8m\u{10EEEE}\u{10EEEE}", first >> COLOUR_BITS)));
        assert!(bytes.contains(&format!("\x1b[2D\x1b[B\x1b[38;2;0;{};1;8m", first >> COLOUR_BITS)));
        assert!(bytes.ends_with("\x1b[?7h\x1b8\x1b[1B\x1b[2C"));
    }

    #[test]
    fn a_sixel_leaves_the_cursor_below_where_it_started() {
        let bytes = String::from_utf8(placeholder_bytes(1, 4, 1, CursorAfter::BelowStart)).unwrap();
        assert!(bytes.ends_with("\x1b8\x1bD"));
        let stays = String::from_utf8(placeholder_bytes(1, 4, 3, CursorAfter::Stays)).unwrap();
        assert!(stays.ends_with("\x1b8"));
    }

    #[test]
    fn a_run_keeps_cells_of_one_placement_row() {
        let mut runs = Vec::new();
        extend_runs(&mut runs, 0, 4, 5 << ROW_BITS | 2);
        extend_runs(&mut runs, 0, 5, 5 << ROW_BITS | 2);
        extend_runs(&mut runs, 0, 6, 6 << ROW_BITS);
        assert_eq!(runs.len(), 2);
        assert_eq!((runs[0].columns.clone(), runs[0].tag, runs[0].picture_row), (4..6, 5, 2));
    }

    #[test]
    fn chunks_padded_one_by_one_are_gathered_as_chafa_sends_them() {
        let (width, height) = (4u32, 2u32);
        let pixels = vec![200u8; (width * height) as usize * BYTES_PER_PIXEL];
        let (head, tail) = pixels.split_at(10);
        let mut images = Images::default();
        let first = Command::parse(format!("a=T,f=32,s={width},v={height},m=1").as_bytes()).unwrap();
        assert!(images.gather(first, b"").is_none());
        let middle = Command::parse(b"m=1").unwrap();
        assert!(images.gather(middle.clone(), STANDARD.encode(head).as_bytes()).is_none());
        assert!(images.gather(middle, STANDARD.encode(tail).as_bytes()).is_none());
        let (command, payload) = images.gather(Command::parse(b"m=0").unwrap(), b"").expect("the last chunk");
        let decoded = kitty_graphics::decode(&command, &payload).expect("the picture");
        assert_eq!((decoded.width, decoded.height, decoded.rgba), (width, height, pixels));
    }

    #[test]
    fn a_chunk_with_bits_past_its_last_byte_is_taken() {
        let mut images = Images::default();
        let first = Command::parse(b"a=T,f=24,s=1,v=1,m=1").unwrap();
        assert!(images.gather(first, b"").is_none());
        // `AQJ=` is 0x01 0x02 with a bit set past the second byte, as chafa pads.
        assert!(images.gather(Command::parse(b"m=1").unwrap(), b"AQJ=").is_none());
        let (_, payload) = images.gather(Command::parse(b"m=0").unwrap(), b"Aw==").expect("the last chunk");
        assert_eq!(STANDARD.decode(payload).unwrap(), [1, 2, 3]);
    }

    #[test]
    fn device_attributes_say_sixel_and_nothing_else_is_touched() {
        assert_eq!(with_sixel_attribute("\x1b[?65;1;9c").as_deref(), Some("\x1b[?65;4;1;9c"));
        assert_eq!(with_sixel_attribute("\x1b[?65;4;1c"), None);
        assert_eq!(with_sixel_attribute("c"), None);
        assert_eq!(with_sixel_attribute("\x1b[>65;8400;1c"), None);
    }

    #[test]
    fn xtsmgraphics_tells_the_colours_and_the_largest_sixel() {
        assert_eq!(graphics_attributes_answer("1;1;0"), b"\x1b[?1;0;256S");
        assert_eq!(graphics_attributes_answer("3;1;0"), b"\x1b[?3;1;0S");
    }
}
