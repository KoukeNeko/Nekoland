//! The keys of a pane while a program speaks the kitty keyboard protocol: the flags it pushed, which keys are held (so
//! that a key GTK sends again is a repeat), and an input method of its own for when every key is a code. Text an input
//! method composes is still sent as text, as the protocol has it; only a key the input method lets through, or types as
//! no more than its own character, is sent as a code.

use std::{
    cell::{Cell, RefCell},
    collections::HashSet,
    rc::Rc,
};

use gtk::{gdk, glib, pango, prelude::*};
use vte4::prelude::*;

use super::{
    hint_mode::{screen_font, screen_offset},
    kitty_keyboard::{self, KeyEvent, KeyInput, Modes, Outcome, Request, REPORT_ALL_KEYS},
    screen_rows,
};

const PREEDIT_CLASS: &str = "term-preedit";

/// What becomes of a key.
pub enum Action {
    Send(Vec<u8>),
    /// The input method took it.
    Swallow,
    /// VTE sends it, as without the protocol.
    Pass,
}

type TextHandler = Box<dyn Fn(&str)>;

pub struct KeyInputState {
    screen: vte4::Terminal,
    modes: RefCell<Modes>,
    held: RefCell<HashSet<u32>>,
    input_method: gtk::IMMulticontext,
    /// Whether the input method is focused: only while every key is a code, as VTE's own serves otherwise.
    input_method_on: Cell<bool>,
    /// While a key is in the input method, what it committed, to be told from a key typed as itself.
    filtering: Cell<bool>,
    committed: RefCell<String>,
    preedit: gtk::Label,
    on_text: RefCell<Option<TextHandler>>,
}

impl KeyInputState {
    pub fn new(screen: &impl IsA<vte4::Terminal>) -> Rc<Self> {
        let input_method = gtk::IMMulticontext::new();
        input_method.set_client_widget(Some(screen.as_ref()));
        let preedit = gtk::Label::builder()
            .css_classes([PREEDIT_CLASS])
            .halign(gtk::Align::Start)
            .valign(gtk::Align::Start)
            .can_target(false)
            .visible(false)
            .build();
        let state = Rc::new(Self {
            screen: screen.as_ref().clone(),
            modes: RefCell::default(),
            held: RefCell::default(),
            input_method,
            input_method_on: Cell::new(false),
            filtering: Cell::new(false),
            committed: RefCell::default(),
            preedit,
            on_text: RefCell::default(),
        });
        state.connect();
        state
    }

    /// The composition shown at the cursor while the input method is this one's.
    pub fn preedit_widget(&self) -> &gtk::Label {
        &self.preedit
    }

    /// Where text an input method commits on its own goes, as a candidate picked with the mouse.
    pub fn set_text_handler(&self, handler: impl Fn(&str) + 'static) {
        *self.on_text.borrow_mut() = Some(Box::new(handler));
    }

    fn connect(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.input_method.connect_commit(glib::clone!(
            #[strong]
            weak,
            move |_, text| {
                let Some(state) = weak.upgrade() else {
                    return;
                };
                if state.filtering.get() {
                    state.committed.borrow_mut().push_str(text);
                } else if let Some(handler) = state.on_text.borrow().as_ref() {
                    handler(text);
                }
            }
        ));
        self.input_method.connect_preedit_changed(glib::clone!(
            #[strong]
            weak,
            move |input_method| {
                if let Some(state) = weak.upgrade() {
                    let (text, _, _) = input_method.preedit_string();
                    state.show_preedit(&text);
                }
            }
        ));
        let focus = gtk::EventControllerFocus::new();
        focus.connect_enter(glib::clone!(
            #[strong]
            weak,
            move |_| {
                if let Some(state) = weak.upgrade() {
                    state.sync_input_method();
                }
            }
        ));
        focus.connect_leave(move |_| {
            if let Some(state) = weak.upgrade() {
                state.sync_input_method();
            }
        });
        self.screen.add_controller(focus);
    }

    pub fn flags(&self) -> u32 {
        self.modes.borrow().flags()
    }

    /// Carries a program's request out; a query gets its answer.
    pub fn request(&self, request: Request) -> Option<Vec<u8>> {
        let answer = self.modes.borrow_mut().apply(request);
        self.sync_input_method();
        answer
    }

    pub fn set_alternate(&self, on: bool) {
        self.modes.borrow_mut().set_alternate(on);
        self.sync_input_method();
    }

    pub fn clear(&self) {
        self.modes.borrow_mut().clear();
        self.sync_input_method();
    }

    /// Focuses this input method while every key is a code and the screen has the focus, and lets it go otherwise.
    fn sync_input_method(&self) {
        let wanted = self.flags() & REPORT_ALL_KEYS != 0 && self.screen.has_focus();
        if wanted == self.input_method_on.replace(wanted) {
            return;
        }
        if wanted {
            self.input_method.focus_in();
        } else {
            self.input_method.reset();
            self.input_method.focus_out();
            self.show_preedit("");
        }
    }

    /// A key pressed: a repeat when GTK sends it again while it is held.
    pub fn press(&self, controller: &gtk::EventControllerKey, key: gdk::Key, keycode: u32, modifiers: gdk::ModifierType) -> Action {
        let event = if self.held.borrow_mut().insert(keycode) { KeyEvent::Press } else { KeyEvent::Repeat };
        let flags = self.flags();
        if flags == 0 {
            return Action::Pass;
        }
        if self.input_method_on.get() {
            if let Some(action) = self.through_input_method(controller, key) {
                return action;
            }
        }
        self.encode(controller, key, keycode, modifiers, event, flags)
    }

    /// A key let go that is not the program's to hear of.
    pub fn forget(&self, keycode: u32) {
        self.held.borrow_mut().remove(&keycode);
    }

    pub fn release(&self,controller: &gtk::EventControllerKey, key: gdk::Key, keycode: u32, modifiers: gdk::ModifierType) -> Action {
        self.held.borrow_mut().remove(&keycode);
        let flags = self.flags();
        if flags == 0 {
            return Action::Pass;
        }
        if self.input_method_on.get() && self.filter(controller) && !self.preedit.text().is_empty() {
            return Action::Swallow;
        }
        match self.encode(controller, key, keycode, modifiers, KeyEvent::Release, flags) {
            Action::Pass => Action::Swallow,
            action => action,
        }
    }

    /// The input method's say on a key: composing it, or committing text other than the key's own character. None
    /// when it let the key through or typed it as itself, which is then a code.
    fn through_input_method(&self, controller: &gtk::EventControllerKey, key: gdk::Key) -> Option<Action> {
        self.place_input_method();
        self.committed.borrow_mut().clear();
        let taken = self.filter(controller);
        let committed = self.committed.take();
        if !taken {
            return None;
        }
        let own_character = key.to_unicode().map(String::from);
        if committed.is_empty() {
            return Some(Action::Swallow);
        }
        if Some(&committed) == own_character.as_ref() && self.preedit.text().is_empty() {
            return None;
        }
        Some(Action::Send(committed.into_bytes()))
    }

    fn filter(&self, controller: &gtk::EventControllerKey) -> bool {
        let Some(event) = controller.current_event() else {
            return false;
        };
        self.filtering.set(true);
        let taken = self.input_method.filter_keypress(&event);
        self.filtering.set(false);
        taken
    }

    fn encode(
        &self,
        controller: &gtk::EventControllerKey,
        key: gdk::Key,
        keycode: u32,
        modifiers: gdk::ModifierType,
        event: KeyEvent,
        flags: u32,
    ) -> Action {
        let base = unmodified_key(controller, keycode).unwrap_or(key);
        match kitty_keyboard::encode(&KeyInput { key, base, modifiers, event }, flags) {
            Outcome::Bytes(sequence) => Action::Send(sequence.into_bytes()),
            Outcome::Skip if event == KeyEvent::Release => Action::Swallow,
            Outcome::Skip | Outcome::Legacy => Action::Pass,
        }
    }

    /// Tells the input method where the cursor is, for its candidates.
    fn place_input_method(&self) {
        let (x, y, width, height) = self.cursor_cell();
        let (left, top) = screen_offset(&self.screen);
        self.input_method.set_cursor_location(&gdk::Rectangle::new(x - left as i32, y - top as i32, width, height));
    }

    fn cursor_cell(&self) -> (i32, i32, i32, i32) {
        let (column, row) = self.screen.cursor_position();
        let first = screen_rows::first_shown(&self.screen);
        let (left, top) = screen_offset(&self.screen);
        let (width, height) = (self.screen.char_width() as i32, self.screen.char_height() as i32);
        let x = left as i32 + column as i32 * width;
        let y = top as i32 + (row - first) as i32 * height;
        (x, y, width, height)
    }

    fn show_preedit(&self, text: &str) {
        self.preedit.set_visible(!text.is_empty());
        if text.is_empty() {
            self.preedit.set_text("");
            return;
        }
        let attributes = pango::AttrList::new();
        if let Some(font) = screen_font(&self.screen) {
            attributes.insert(pango::AttrFontDesc::new(&font));
        }
        attributes.insert(pango::AttrInt::new_underline(pango::Underline::Single));
        self.preedit.set_attributes(Some(&attributes));
        self.preedit.set_text(text);
        let (x, y, _, _) = self.cursor_cell();
        self.preedit.set_margin_start(x);
        self.preedit.set_margin_top(y);
    }
}

/// The key on the same key with no modifiers held: `a` for Shift+a, in the layout in use.
fn unmodified_key(controller: &gtk::EventControllerKey, keycode: u32) -> Option<gdk::Key> {
    let display = controller.widget()?.display();
    let group = controller.group() as i32;
    display.translate_key(keycode, gdk::ModifierType::empty(), group).map(|(key, ..)| key)
}

/// The composition's look: on the screen's own colours.
pub fn css() -> &'static str {
    ".term-preedit { background-color: @view_bg_color; color: @view_fg_color; }"
}
