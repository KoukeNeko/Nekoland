//! The kitty keyboard protocol, as Nekominal speaks it, over VTE, which does not: a program pushes the enhancements it
//! wants (`CSI > flags u`), and keys are sent as `CSI code[:shifted] ; modifiers[:event] ; text u` as the five flags
//! say. A key the flags leave alone is VTE's to send, as it always was. The flags are kept for the main screen and the
//! alternate one apart, as Nekominal keeps them.
//! <https://sw.kovidgoyal.net/kitty/keyboard-protocol/>

use gtk::{gdk, glib::translate::IntoGlib};

pub const DISAMBIGUATE: u32 = 0b1;
pub const REPORT_EVENTS: u32 = 0b10;
pub const REPORT_ALTERNATES: u32 = 0b100;
pub const REPORT_ALL_KEYS: u32 = 0b1000;
pub const REPORT_TEXT: u32 = 0b1_0000;
const ALL_FLAGS: u32 = 0b1_1111;
/// Programs push and forget to pop; the protocol lets a terminal drop the oldest beyond its limit.
const STACK_LIMIT: usize = 64;
/// The modifier bits of the protocol, sent as one more than their sum.
const SHIFT_BIT: u8 = 1;
const ALT_BIT: u8 = 2;
const CTRL_BIT: u8 = 4;
const SUPER_BIT: u8 = 8;
const CAPS_LOCK_BIT: u8 = 64;
/// kitty's numbers of the keys with no character, after `CSI`.
const ESCAPE_CODE: u32 = 27;
const ENTER_CODE: u32 = 13;
const TAB_CODE: u32 = 9;
const BACKSPACE_CODE: u32 = 127;
const F13_CODE: u32 = 57376;
const KEYPAD_DIGIT_CODE: u32 = 57399;
const FINAL_U: char = 'u';

/// What a program asks of the keyboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    /// `CSI > flags u`.
    Push(u32),
    /// `CSI < count u`.
    Pop(u32),
    /// `CSI = flags ; how u`.
    Set { flags: u32, how: SetHow },
    /// `CSI ? u`.
    Query,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetHow {
    Replace,
    Add,
    Remove,
}

/// The request in the parameters of a CSI that ends in `u`; None for a plain `CSI u`, which restores the cursor.
pub fn parse_request(parameters: &str) -> Option<Request> {
    let mut chars = parameters.chars();
    let kind = chars.next()?;
    let rest = chars.as_str();
    let mut numbers = rest.split(';').map(|number| number.parse::<u32>().ok());
    let mut next = || numbers.next().flatten();
    match kind {
        '>' => Some(Request::Push(next().unwrap_or(0))),
        '<' => Some(Request::Pop(next().unwrap_or(1).max(1))),
        '=' => {
            let flags = next().unwrap_or(0);
            let how = match next() {
                Some(2) => SetHow::Add,
                Some(3) => SetHow::Remove,
                _ => SetHow::Replace,
            };
            Some(Request::Set { flags, how })
        }
        '?' if rest.is_empty() => Some(Request::Query),
        _ => None,
    }
}

/// The enhancements pushed on each screen, the latest last; none pushed is the legacy keyboard.
#[derive(Debug, Default)]
pub struct Modes {
    main: Vec<u32>,
    alternate: Vec<u32>,
    on_alternate: bool,
}

impl Modes {
    pub fn flags(&self) -> u32 {
        self.stack().last().copied().unwrap_or(0)
    }

    fn stack(&self) -> &Vec<u32> {
        if self.on_alternate {
            &self.alternate
        } else {
            &self.main
        }
    }

    fn stack_mut(&mut self) -> &mut Vec<u32> {
        if self.on_alternate {
            &mut self.alternate
        } else {
            &mut self.main
        }
    }

    /// Carries a request out; a query gets the answer to send back.
    pub fn apply(&mut self, request: Request) -> Option<Vec<u8>> {
        match request {
            Request::Push(flags) => {
                let stack = self.stack_mut();
                if stack.len() >= STACK_LIMIT {
                    stack.remove(0);
                }
                stack.push(flags & ALL_FLAGS);
            }
            Request::Pop(count) => {
                let stack = self.stack_mut();
                let kept = stack.len().saturating_sub(count as usize);
                stack.truncate(kept);
            }
            Request::Set { flags, how } => {
                let flags = flags & ALL_FLAGS;
                let current = self.flags();
                let new = match how {
                    SetHow::Replace => flags,
                    SetHow::Add => current | flags,
                    SetHow::Remove => current & !flags,
                };
                let stack = self.stack_mut();
                match stack.last_mut() {
                    Some(top) => *top = new,
                    None => stack.push(new),
                }
            }
            Request::Query => return Some(format!("\x1b[?{}u", self.flags()).into_bytes()),
        }
        None
    }

    /// The alternate screen comes or goes; it starts with no enhancements.
    pub fn set_alternate(&mut self, on: bool) {
        if on && !self.on_alternate {
            self.alternate.clear();
        }
        self.on_alternate = on;
    }

    /// The program that pushed is gone: the shell is at its prompt again.
    pub fn clear(&mut self) {
        self.main.clear();
        self.alternate.clear();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyEvent {
    Press,
    Repeat,
    Release,
}

/// A key as GTK gives it.
#[derive(Clone, Copy, Debug)]
pub struct KeyInput {
    /// The key with the modifiers applied: `A` for Shift+a.
    pub key: gdk::Key,
    /// The same key with none: `a`.
    pub base: gdk::Key,
    pub modifiers: gdk::ModifierType,
    pub event: KeyEvent,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Bytes(String),
    /// Not reported at all.
    Skip,
    /// The flags leave it as it was: VTE sends it.
    Legacy,
}

/// What kind of key it is, for the protocol.
enum Kind {
    Modifier(u32, u8),
    Lock(u32),
    Enter,
    Tab,
    Backspace,
    Escape,
    /// `CSI number ; modifiers final`, as the legacy keyboard has them.
    Letter(u32, char),
    /// Only ever `CSI number u`.
    Numbered(u32),
    /// A key that types its character; a keypad one is reported by its own number.
    Text(char, Option<u32>),
    Other,
}

fn kind_of(key: gdk::Key) -> Kind {
    use gdk::Key as K;
    let modifier = |code, bit| Kind::Modifier(code, bit);
    match key {
        K::Shift_L => modifier(57441, SHIFT_BIT),
        K::Control_L => modifier(57442, CTRL_BIT),
        K::Alt_L => modifier(57443, ALT_BIT),
        K::Super_L => modifier(57444, SUPER_BIT),
        K::Shift_R => modifier(57447, SHIFT_BIT),
        K::Control_R => modifier(57448, CTRL_BIT),
        K::Alt_R => modifier(57449, ALT_BIT),
        K::Super_R => modifier(57450, SUPER_BIT),
        K::ISO_Level3_Shift => modifier(57453, 0),
        K::ISO_Level5_Shift => modifier(57454, 0),
        K::Caps_Lock => Kind::Lock(57358),
        K::Scroll_Lock => Kind::Lock(57359),
        K::Num_Lock => Kind::Lock(57360),
        K::Return | K::KP_Enter => Kind::Enter,
        K::Tab | K::ISO_Left_Tab | K::KP_Tab => Kind::Tab,
        K::BackSpace => Kind::Backspace,
        K::Escape => Kind::Escape,
        K::Up => Kind::Letter(1, 'A'),
        K::Down => Kind::Letter(1, 'B'),
        K::Right => Kind::Letter(1, 'C'),
        K::Left => Kind::Letter(1, 'D'),
        K::Home => Kind::Letter(1, 'H'),
        K::End => Kind::Letter(1, 'F'),
        K::KP_Begin => Kind::Letter(1, 'E'),
        K::Insert => Kind::Letter(2, '~'),
        K::Delete => Kind::Letter(3, '~'),
        K::Page_Up => Kind::Letter(5, '~'),
        K::Page_Down => Kind::Letter(6, '~'),
        K::F1 => Kind::Letter(1, 'P'),
        K::F2 => Kind::Letter(1, 'Q'),
        K::F3 => Kind::Letter(13, '~'),
        K::F4 => Kind::Letter(1, 'S'),
        K::F5 => Kind::Letter(15, '~'),
        K::F6 => Kind::Letter(17, '~'),
        K::F7 => Kind::Letter(18, '~'),
        K::F8 => Kind::Letter(19, '~'),
        K::F9 => Kind::Letter(20, '~'),
        K::F10 => Kind::Letter(21, '~'),
        K::F11 => Kind::Letter(23, '~'),
        K::F12 => Kind::Letter(24, '~'),
        K::KP_Left => Kind::Numbered(57417),
        K::KP_Right => Kind::Numbered(57418),
        K::KP_Up => Kind::Numbered(57419),
        K::KP_Down => Kind::Numbered(57420),
        K::KP_Page_Up => Kind::Numbered(57421),
        K::KP_Page_Down => Kind::Numbered(57422),
        K::KP_Home => Kind::Numbered(57423),
        K::KP_End => Kind::Numbered(57424),
        K::KP_Insert => Kind::Numbered(57425),
        K::KP_Delete => Kind::Numbered(57426),
        K::Print => Kind::Numbered(57361),
        K::Pause => Kind::Numbered(57362),
        K::Menu => Kind::Numbered(57363),
        K::AudioPlay => Kind::Numbered(57428),
        K::AudioPause => Kind::Numbered(57429),
        K::AudioStop => Kind::Numbered(57432),
        K::AudioForward => Kind::Numbered(57433),
        K::AudioRewind => Kind::Numbered(57434),
        K::AudioNext => Kind::Numbered(57435),
        K::AudioPrev => Kind::Numbered(57436),
        K::AudioRecord => Kind::Numbered(57437),
        K::AudioLowerVolume => Kind::Numbered(57438),
        K::AudioRaiseVolume => Kind::Numbered(57439),
        K::AudioMute => Kind::Numbered(57440),
        K::KP_Decimal => Kind::Text('.', Some(57409)),
        K::KP_Divide => Kind::Text('/', Some(57410)),
        K::KP_Multiply => Kind::Text('*', Some(57411)),
        K::KP_Subtract => Kind::Text('-', Some(57412)),
        K::KP_Add => Kind::Text('+', Some(57413)),
        K::KP_Equal => Kind::Text('=', Some(57415)),
        K::KP_Separator => Kind::Text(',', Some(57416)),
        _ => kind_of_numbered_or_text(key),
    }
}

/// The keys whose numbers follow from where they are in a range: F13 to F35 and the keypad's digits; else a key that
/// types a character.
fn kind_of_numbered_or_text(key: gdk::Key) -> Kind {
    let value = key.into_glib();
    let f13 = gdk::Key::F13.into_glib();
    let f35 = gdk::Key::F35.into_glib();
    let keypad_zero = gdk::Key::KP_0.into_glib();
    let keypad_nine = gdk::Key::KP_9.into_glib();
    if (f13..=f35).contains(&value) {
        return Kind::Numbered(F13_CODE + value - f13);
    }
    if (keypad_zero..=keypad_nine).contains(&value) {
        let digit = value - keypad_zero;
        let char = char::from_digit(digit, 10).unwrap_or('0');
        return Kind::Text(char, Some(KEYPAD_DIGIT_CODE + digit));
    }
    match key.to_unicode().filter(|char| !char.is_control()) {
        Some(char) => Kind::Text(lower(char), None),
        None => Kind::Other,
    }
}

fn lower(char: char) -> char {
    let mut lower = char.to_lowercase();
    match (lower.next(), lower.next()) {
        (Some(single), None) => single,
        _ => char,
    }
}

/// What Shift does to a key GTK did not apply it to: upper case for letters, the US layout for ASCII symbols, as
/// Nekominal falls back.
fn shift_fallback(char: char) -> char {
    const US_SHIFTED: [(char, char); 21] = [
        ('`', '~'), ('1', '!'), ('2', '@'), ('3', '#'), ('4', '$'), ('5', '%'), ('6', '^'), ('7', '&'), ('8', '*'),
        ('9', '('), ('0', ')'), ('-', '_'), ('=', '+'), ('[', '{'), (']', '}'), ('\\', '|'), (';', ':'), ('\'', '"'),
        (',', '<'), ('.', '>'), ('/', '?'),
    ];
    if char.is_alphabetic() {
        let mut upper = char.to_uppercase();
        return match (upper.next(), upper.next()) {
            (Some(single), None) => single,
            _ => char,
        };
    }
    US_SHIFTED.iter().find(|(plain, _)| *plain == char).map_or(char, |(_, shifted)| *shifted)
}

/// One `CSI code[:shifted] [; mods[:event]] [; text] final` sequence, laid out as kitty's own `serialize()`.
struct Csi {
    code: u32,
    shifted: Option<u32>,
    /// Modifier bits without the `+ 1`.
    mods: u8,
    /// 1 press, 2 repeat, 3 release.
    event: u8,
    text: Vec<u32>,
    final_char: char,
}

impl Csi {
    fn serialize(&self) -> String {
        let second = self.mods != 0 || self.event != 1;
        let third = !self.text.is_empty();
        let mut out = String::from("\x1b[");
        // The number is left out only when it is 1 and nothing follows it: `CSI A`.
        if self.code != 1 || self.shifted.is_some() || second || third {
            out += &self.code.to_string();
        }
        if let Some(shifted) = self.shifted {
            out += &format!(":{shifted}");
        }
        if second || third {
            out.push(';');
            if second {
                out += &(u32::from(self.mods) + 1).to_string();
                if self.event != 1 {
                    out += &format!(":{}", self.event);
                }
            }
        }
        if third {
            let text: Vec<String> = self.text.iter().map(u32::to_string).collect();
            out += &format!(";{}", text.join(":"));
        }
        out.push(self.final_char);
        out
    }
}

/// The event as the flags see it.
struct Context {
    flags: u32,
    event: u8,
    /// The modifiers held, a modifier key counting itself while it is down.
    held: u8,
    locks: u8,
    release: bool,
}

impl Context {
    fn has(&self, flag: u32) -> bool {
        self.flags & flag != 0
    }

    /// Any modifier but Shift: the key is not plain typing.
    fn chord(&self) -> bool {
        self.held & !SHIFT_BIT != 0
    }

    fn csi(&self, code: u32, final_char: char) -> Csi {
        // Lock modifiers are told only when every key is.
        let locks = if self.has(REPORT_ALL_KEYS) { self.locks } else { 0 };
        Csi { code, shifted: None, mods: self.held | locks, event: self.event, text: Vec::new(), final_char }
    }

    fn bytes(&self, code: u32, final_char: char) -> Outcome {
        Outcome::Bytes(self.csi(code, final_char).serialize())
    }
}

/// What a key sends under `flags`.
pub fn encode(input: &KeyInput, flags: u32) -> Outcome {
    if flags == 0 {
        return Outcome::Legacy;
    }
    let report_events = flags & REPORT_EVENTS != 0;
    // Releases are told only when asked for; a repeat is a press otherwise.
    if input.event == KeyEvent::Release && !report_events {
        return Outcome::Skip;
    }
    let event = match (report_events, input.event) {
        (true, KeyEvent::Repeat) => 2,
        (true, KeyEvent::Release) => 3,
        _ => 1,
    };
    let kind = kind_of(input.base);
    let mut held = modifier_bits(input.modifiers);
    if let Kind::Modifier(_, bit) = kind {
        // GTK gives the state before the key: a modifier pressed counts itself, one released no longer does.
        held = if input.event == KeyEvent::Release { held & !bit } else { held | bit };
    }
    let locks = if input.modifiers.contains(gdk::ModifierType::LOCK_MASK) { CAPS_LOCK_BIT } else { 0 };
    let context = Context { flags, event, held, locks, release: input.event == KeyEvent::Release };
    match kind {
        Kind::Modifier(code, _) | Kind::Lock(code) => modifier_key(&context, input.event, code),
        Kind::Enter => c0_key(&context, ENTER_CODE),
        Kind::Tab => c0_key(&context, TAB_CODE),
        Kind::Backspace => c0_key(&context, BACKSPACE_CODE),
        Kind::Escape | Kind::Numbered(_) if !(context.release || context.has(DISAMBIGUATE | REPORT_ALL_KEYS)) => {
            Outcome::Legacy
        }
        Kind::Escape => context.bytes(ESCAPE_CODE, FINAL_U),
        Kind::Numbered(code) => context.bytes(code, FINAL_U),
        // Any flag switches these to the CSI form, with the event when asked for.
        Kind::Letter(code, final_char) => context.bytes(code, final_char),
        Kind::Text(char, keypad) => text_key(&context, input, char, keypad),
        Kind::Other => Outcome::Legacy,
    }
}

/// Modifier and lock keys are told only when every key is; their repeats are noise.
fn modifier_key(context: &Context, event: KeyEvent, code: u32) -> Outcome {
    if !context.has(REPORT_ALL_KEYS) || event == KeyEvent::Repeat {
        return Outcome::Skip;
    }
    context.bytes(code, FINAL_U)
}

/// Enter, Tab and Backspace stay legacy bytes until modified, so a shell survives a program that died with the flags on.
fn c0_key(context: &Context, code: u32) -> Outcome {
    if context.has(REPORT_ALL_KEYS) {
        return context.bytes(code, FINAL_U);
    }
    if context.release {
        return Outcome::Skip;
    }
    // Shift counts: Shift+Enter is the point.
    if context.has(DISAMBIGUATE) && context.held != 0 {
        return context.bytes(code, FINAL_U);
    }
    Outcome::Legacy
}

/// A key that types a character: plain and Shift-only typing is text, anything else a code.
fn text_key(context: &Context, input: &KeyInput, char: char, keypad: Option<u32>) -> Outcome {
    let escape = context.has(REPORT_ALL_KEYS) || (context.has(DISAMBIGUATE) && context.chord()) || context.release;
    if !escape {
        return Outcome::Legacy;
    }
    let shift = context.held & SHIFT_BIT != 0;
    let typed = input.key.to_unicode().filter(|typed| !typed.is_control());
    // A key that comes with Shift held but not applied, as a virtual keyboard sends it, types its Shift layer.
    let typed = match typed {
        Some(typed) if shift && keypad.is_none() && lower(typed) == char => Some(shift_fallback(char)),
        other => other,
    };
    let mut csi = context.csi(keypad.unwrap_or(char as u32), FINAL_U);
    if context.has(REPORT_ALTERNATES) && keypad.is_none() && shift {
        csi.shifted = typed.filter(|typed| *typed != char).map(u32::from);
    }
    if context.has(REPORT_TEXT) && !context.release && !context.chord() {
        csi.text = typed.map(u32::from).into_iter().collect();
    }
    Outcome::Bytes(csi.serialize())
}

fn modifier_bits(modifiers: gdk::ModifierType) -> u8 {
    [
        (gdk::ModifierType::SHIFT_MASK, SHIFT_BIT),
        (gdk::ModifierType::ALT_MASK, ALT_BIT),
        (gdk::ModifierType::CONTROL_MASK, CTRL_BIT),
        (gdk::ModifierType::SUPER_MASK, SUPER_BIT),
    ]
    .into_iter()
    .filter(|(mask, _)| modifiers.contains(*mask))
    .map(|(_, bit)| bit)
    .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: gdk::ModifierType = gdk::ModifierType::empty();
    const SHIFT: gdk::ModifierType = gdk::ModifierType::SHIFT_MASK;
    const CTRL: gdk::ModifierType = gdk::ModifierType::CONTROL_MASK;

    fn press(key: gdk::Key, base: gdk::Key, modifiers: gdk::ModifierType) -> KeyInput {
        KeyInput { key, base, modifiers, event: KeyEvent::Press }
    }

    fn sent(input: KeyInput, flags: u32) -> Outcome {
        encode(&input, flags)
    }

    fn bytes(text: &str) -> Outcome {
        Outcome::Bytes(text.to_owned())
    }

    #[test]
    fn requests_are_read_and_a_plain_restore_is_not_one() {
        assert_eq!(parse_request(">1"), Some(Request::Push(1)));
        assert_eq!(parse_request("<"), Some(Request::Pop(1)));
        assert_eq!(parse_request("<3"), Some(Request::Pop(3)));
        assert_eq!(parse_request("=1;2"), Some(Request::Set { flags: 1, how: SetHow::Add }));
        assert_eq!(parse_request("?"), Some(Request::Query));
        assert_eq!(parse_request(""), None);
    }

    #[test]
    fn push_pop_and_set_flow_into_the_flags() {
        let mut modes = Modes::default();
        modes.apply(Request::Push(31));
        assert_eq!(modes.apply(Request::Query), Some(b"\x1b[?31u".to_vec()));
        modes.apply(Request::Set { flags: 8, how: SetHow::Replace });
        modes.apply(Request::Set { flags: 2, how: SetHow::Add });
        assert_eq!(modes.flags(), REPORT_ALL_KEYS | REPORT_EVENTS);
        modes.apply(Request::Set { flags: 8, how: SetHow::Remove });
        assert_eq!(modes.flags(), REPORT_EVENTS);
        modes.apply(Request::Pop(1));
        assert_eq!(modes.flags(), 0);
    }

    #[test]
    fn the_flags_are_per_screen() {
        let mut modes = Modes::default();
        modes.apply(Request::Push(1));
        modes.set_alternate(true);
        assert_eq!(modes.flags(), 0);
        modes.apply(Request::Push(8));
        assert_eq!(modes.flags(), REPORT_ALL_KEYS);
        modes.set_alternate(false);
        assert_eq!(modes.flags(), DISAMBIGUATE);
    }

    #[test]
    fn disambiguation_codes_the_ambiguous_keys_only() {
        assert_eq!(sent(press(gdk::Key::Return, gdk::Key::Return, SHIFT), 1), bytes("\x1b[13;2u"));
        assert_eq!(sent(press(gdk::Key::Escape, gdk::Key::Escape, NONE), 1), bytes("\x1b[27u"));
        assert_eq!(sent(press(gdk::Key::i, gdk::Key::i, CTRL), 1), bytes("\x1b[105;5u"));
        assert_eq!(sent(press(gdk::Key::C, gdk::Key::c, CTRL | SHIFT), 1), bytes("\x1b[99;6u"));
        assert_eq!(sent(press(gdk::Key::Return, gdk::Key::Return, NONE), 1), Outcome::Legacy);
        assert_eq!(sent(press(gdk::Key::A, gdk::Key::a, SHIFT), 1), Outcome::Legacy);
        assert_eq!(sent(press(gdk::Key::Up, gdk::Key::Up, NONE), 1), bytes("\x1b[A"));
        assert_eq!(sent(press(gdk::Key::Escape, gdk::Key::Escape, NONE), 0), Outcome::Legacy);
    }

    #[test]
    fn every_key_is_a_code_with_its_text_when_asked() {
        let flags = REPORT_ALL_KEYS | REPORT_TEXT | REPORT_ALTERNATES;
        assert_eq!(sent(press(gdk::Key::a, gdk::Key::a, NONE), flags), bytes("\x1b[97;;97u"));
        assert_eq!(sent(press(gdk::Key::A, gdk::Key::a, SHIFT), flags), bytes("\x1b[97:65;2;65u"));
        assert_eq!(sent(press(gdk::Key::a, gdk::Key::a, SHIFT), flags), bytes("\x1b[97:65;2;65u"));
        assert_eq!(sent(press(gdk::Key::_1, gdk::Key::_1, SHIFT), flags), bytes("\x1b[49:33;2;33u"));
        assert_eq!(sent(press(gdk::Key::Return, gdk::Key::Return, NONE), flags), bytes("\x1b[13u"));
        assert_eq!(sent(press(gdk::Key::Shift_L, gdk::Key::Shift_L, NONE), flags), bytes("\x1b[57441;2u"));
        assert_eq!(sent(press(gdk::Key::KP_5, gdk::Key::KP_5, NONE), flags), bytes("\x1b[57404;;53u"));
        assert_eq!(sent(press(gdk::Key::F13, gdk::Key::F13, NONE), flags), bytes("\x1b[57376u"));
    }

    #[test]
    fn events_are_told_and_releases_only_then() {
        let release = KeyInput { event: KeyEvent::Release, ..press(gdk::Key::a, gdk::Key::a, NONE) };
        assert_eq!(sent(release, DISAMBIGUATE), Outcome::Skip);
        assert_eq!(sent(release, REPORT_EVENTS), bytes("\x1b[97;1:3u"));
        let repeat = KeyInput { event: KeyEvent::Repeat, ..press(gdk::Key::Up, gdk::Key::Up, NONE) };
        assert_eq!(sent(repeat, REPORT_EVENTS), bytes("\x1b[1;1:2A"));
        let enter_up = KeyInput { event: KeyEvent::Release, ..press(gdk::Key::Return, gdk::Key::Return, NONE) };
        assert_eq!(sent(enter_up, REPORT_EVENTS), Outcome::Skip);
        let shift_up = KeyInput { event: KeyEvent::Release, ..press(gdk::Key::Shift_L, gdk::Key::Shift_L, SHIFT) };
        assert_eq!(sent(shift_up, REPORT_EVENTS | REPORT_ALL_KEYS), bytes("\x1b[57441;1:3u"));
    }
}
