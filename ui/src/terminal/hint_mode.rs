//! Hint mode on a pane, as Nekominal's: each URL, path and hash on the screen gets a label drawn over its first cells;
//! typing a label copies what it marks, or with Shift opens it. Backspace takes the last letter back, Escape stops, and
//! any other key stops and goes on to the program.

use std::{cell::RefCell, rc::Rc};

use gtk::{gdk, gio, glib, prelude::*};
use vte4::prelude::*;

use super::{
    hints::{self, Hint, HintKind, ScreenRow},
    pane::Pane,
    preview, screen_rows,
};

const LABEL_BACKGROUND: gdk::RGBA = gdk::RGBA::new(0.98, 0.65, 0.14, 1.0);
const LABEL_FOREGROUND: gdk::RGBA = gdk::RGBA::BLACK;
/// Keys held for a label's letter, which do not stop hint mode.
const MODIFIER_KEYS: [gdk::Key; 8] = [
    gdk::Key::Shift_L,
    gdk::Key::Shift_R,
    gdk::Key::Control_L,
    gdk::Key::Control_R,
    gdk::Key::Alt_L,
    gdk::Key::Alt_R,
    gdk::Key::Super_L,
    gdk::Key::Caps_Lock,
];

/// The hints of the pane while hint mode is on, and what of a label has been typed.
#[derive(Default)]
struct Mode {
    hints: Vec<Hint>,
    typed: String,
}

impl Mode {
    /// The labels still to be picked, cut to what is left to type, at their cells.
    fn shown(&self) -> Vec<(usize, usize, String)> {
        self.hints
            .iter()
            .filter_map(|hint| hint.label.strip_prefix(self.typed.as_str()).map(|rest| (hint.row, hint.column, rest.to_string())))
            .collect()
    }
}

pub struct HintLayer {
    area: gtk::DrawingArea,
    mode: RefCell<Option<Mode>>,
}

impl HintLayer {
    /// The layer, to be put over a screen in an overlay.
    pub fn new() -> Rc<Self> {
        let area = gtk::DrawingArea::builder().can_target(false).can_focus(false).build();
        Rc::new(Self { area, mode: RefCell::default() })
    }

    pub fn widget(&self) -> &gtk::DrawingArea {
        &self.area
    }

    pub fn is_on(&self) -> bool {
        self.mode.borrow().is_some()
    }

    /// Draws the labels over the pane's screen, and takes the keys of the screen while hint mode is on.
    pub fn attach(self: &Rc<Self>, pane: &Rc<Pane>) {
        let screen = pane.screen.clone();
        let pane_screen = pane.screen.clone();
        let weak = Rc::downgrade(self);
        self.area.set_draw_func(glib::clone!(
            #[strong]
            weak,
            move |area, cairo, _, _| {
                if let Some(layer) = weak.upgrade() {
                    layer.draw(area, cairo, screen.upcast_ref());
                }
            }
        ));
        let keys = gtk::EventControllerKey::builder().propagation_phase(gtk::PropagationPhase::Capture).build();
        let pane = Rc::downgrade(pane);
        keys.connect_key_pressed(move |_, key, _, modifiers| {
            match (weak.upgrade(), pane.upgrade()) {
                (Some(layer), Some(pane)) if layer.is_on() => layer.take_key(&pane, key, modifiers),
                _ => glib::Propagation::Proceed,
            }
        });
        pane_screen.add_controller(keys);
    }

    /// Starts hint mode on what `pane` shows, or stops it when it is on.
    pub fn toggle(&self, pane: &Pane) {
        if self.is_on() {
            self.leave();
            return;
        }
        let found = hints::find(&visible_rows(pane.screen.upcast_ref()));
        if found.is_empty() {
            return;
        }
        self.mode.replace(Some(Mode { hints: found, typed: String::new() }));
        self.area.queue_draw();
    }

    fn leave(&self) {
        self.mode.take();
        self.area.queue_draw();
    }

    fn take_key(&self, pane: &Pane, key: gdk::Key, modifiers: gdk::ModifierType) -> glib::Propagation {
        if MODIFIER_KEYS.contains(&key) {
            return glib::Propagation::Stop;
        }
        match key {
            gdk::Key::Escape => {
                self.leave();
                return glib::Propagation::Stop;
            }
            gdk::Key::BackSpace => {
                if let Some(mode) = self.mode.borrow_mut().as_mut() {
                    mode.typed.pop();
                }
                self.area.queue_draw();
                return glib::Propagation::Stop;
            }
            _ => {}
        }
        let Some(letter) = key.to_lower().to_unicode().filter(char::is_ascii_lowercase) else {
            self.leave();
            return glib::Propagation::Proceed;
        };
        let picked = {
            let mut mode = self.mode.borrow_mut();
            let Some(mode) = mode.as_mut() else {
                return glib::Propagation::Proceed;
            };
            mode.typed.push(letter);
            let picked = mode.hints.iter().find(|hint| hint.label == mode.typed).cloned();
            if picked.is_none() && !mode.hints.iter().any(|hint| hint.label.starts_with(mode.typed.as_str())) {
                // A letter no label goes on with is ignored.
                mode.typed.pop();
            }
            picked
        };
        match picked {
            Some(hint) => {
                self.leave();
                use_hint(pane, &hint, modifiers.contains(gdk::ModifierType::SHIFT_MASK));
            }
            None => self.area.queue_draw(),
        }
        glib::Propagation::Stop
    }

    fn draw(&self, area: &gtk::DrawingArea, cairo: &gtk::cairo::Context, screen: &vte4::Terminal) {
        let Some(labels) = self.mode.borrow().as_ref().map(Mode::shown) else {
            return;
        };
        let (cell_width, cell_height) = (screen.char_width() as f64, screen.char_height() as f64);
        let (left, top) = screen_offset(screen);
        let font = screen_font(screen);
        for (row, column, label) in labels {
            let x = left + column as f64 * cell_width;
            let y = top + row as f64 * cell_height;
            let width = cell_width * label.chars().count() as f64;
            cairo.set_source_rgba(
                f64::from(LABEL_BACKGROUND.red()),
                f64::from(LABEL_BACKGROUND.green()),
                f64::from(LABEL_BACKGROUND.blue()),
                1.0,
            );
            cairo.rectangle(x, y, width, cell_height);
            if let Err(error) = cairo.fill() {
                eprintln!("nekoland-terminal: could not draw a hint: {error}");
                return;
            }
            let layout = area.create_pango_layout(Some(&label));
            layout.set_font_description(font.as_ref());
            cairo.set_source_rgba(
                f64::from(LABEL_FOREGROUND.red()),
                f64::from(LABEL_FOREGROUND.green()),
                f64::from(LABEL_FOREGROUND.blue()),
                1.0,
            );
            cairo.move_to(x, y);
            pangocairo::functions::show_layout(cairo, &layout);
        }
    }
}

/// The font the screen draws with, at its scale.
pub fn screen_font(screen: &vte4::Terminal) -> Option<gtk::pango::FontDescription> {
    screen.font().map(|font| {
        let mut font = font.clone();
        let size = f64::from(font.size()) * screen.font_scale();
        font.set_size(size as i32);
        font
    })
}

/// Where the first cell of the screen is in its widget: past its padding.
pub fn screen_offset(screen: &vte4::Terminal) -> (f64, f64) {
    #[allow(deprecated)]
    let padding = screen.style_context().padding();
    (f64::from(padding.left()), f64::from(padding.top()))
}

/// The rows the screen shows, each with whether it goes on in the row under it.
fn visible_rows(screen: &vte4::Terminal) -> Vec<ScreenRow> {
    let first = screen_rows::first_shown(screen);
    let columns = screen.column_count();
    (first..first + screen.row_count())
        .map(|row| {
            let (text, _) = screen.text_range_format(vte4::Format::Text, row, 0, row, columns);
            let text = text.map(|text| text.to_string()).unwrap_or_default();
            // VTE ends a row with a newline only where its line ends, not where it is wrapped.
            let wraps = !text.ends_with('\n') && text.chars().count() >= usize::try_from(columns).unwrap_or_default();
            ScreenRow { text: text.trim_end_matches('\n').to_string(), wraps }
        })
        .collect()
}

/// Copies a hint, or opens it: a URL in the browser, a path in the editor; a hash is only copied.
fn use_hint(pane: &Pane, hint: &Hint, open: bool) {
    if !open || hint.kind == HintKind::Hash {
        pane.screen.clipboard().set_text(&hint.text);
        return;
    }
    if hint.kind == HintKind::Url {
        let window = pane.screen.root().and_downcast::<gtk::Window>();
        let url = hint.text.clone();
        gtk::UriLauncher::new(&url).launch(window.as_ref(), gio::Cancellable::NONE, move |result| {
            if let Err(error) = result {
                eprintln!("nekoland-terminal: could not open {url}: {error}");
            }
        });
        return;
    }
    let line = preview::split_file_line(&hint.text).map(|(_, line)| line);
    let path = preview::resolve(hints::without_position(&hint.text), pane.directory().as_deref());
    preview::open_file(&pane.screen, &path, line);
}
