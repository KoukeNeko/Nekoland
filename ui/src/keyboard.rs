//! `nekoland-keyboard`: the on-screen keyboard. Its keys go to the window in front through
//! the compositor, which never gives this window the keyboard; Shift, Ctrl, Alt and Super
//! pressed once hold for the next key, as sticky keys do.

use std::{cell::RefCell, rc::Rc};

use adw::prelude::*;
use gtk::{gdk, glib};

mod ipc;

const APP_ID: &str = "io.github.koukeneko.Nekoland.Keyboard";
/// How wide a key of one unit is, and the gap between keys.
const UNIT: i32 = 52;
const GAP: i32 = 6;

/// A key: what it shows, what it shows with Shift, its XKB name, and how many units wide.
type Key = (&'static str, &'static str, &'static str, f64);

const ROWS: [&[Key]; 5] = [
    &[
        ("Esc", "Esc", "Escape", 1.0),
        ("`", "~", "grave", 1.0),
        ("1", "!", "1", 1.0),
        ("2", "@", "2", 1.0),
        ("3", "#", "3", 1.0),
        ("4", "$", "4", 1.0),
        ("5", "%", "5", 1.0),
        ("6", "^", "6", 1.0),
        ("7", "&", "7", 1.0),
        ("8", "*", "8", 1.0),
        ("9", "(", "9", 1.0),
        ("0", ")", "0", 1.0),
        ("-", "_", "minus", 1.0),
        ("=", "+", "equal", 1.0),
        ("⌫", "⌫", "BackSpace", 2.0),
    ],
    &[
        ("Tab", "Tab", "Tab", 1.5),
        ("q", "Q", "q", 1.0),
        ("w", "W", "w", 1.0),
        ("e", "E", "e", 1.0),
        ("r", "R", "r", 1.0),
        ("t", "T", "t", 1.0),
        ("y", "Y", "y", 1.0),
        ("u", "U", "u", 1.0),
        ("i", "I", "i", 1.0),
        ("o", "O", "o", 1.0),
        ("p", "P", "p", 1.0),
        ("[", "{", "bracketleft", 1.0),
        ("]", "}", "bracketright", 1.0),
        ("\\", "|", "backslash", 2.5),
    ],
    &[
        ("Caps", "Caps", "Caps", 2.0),
        ("a", "A", "a", 1.0),
        ("s", "S", "s", 1.0),
        ("d", "D", "d", 1.0),
        ("f", "F", "f", 1.0),
        ("g", "G", "g", 1.0),
        ("h", "H", "h", 1.0),
        ("j", "J", "j", 1.0),
        ("k", "K", "k", 1.0),
        ("l", "L", "l", 1.0),
        (";", ":", "semicolon", 1.0),
        ("'", "\"", "apostrophe", 1.0),
        ("Enter", "Enter", "Return", 3.0),
    ],
    &[
        ("Shift", "Shift", "Shift", 2.5),
        ("z", "Z", "z", 1.0),
        ("x", "X", "x", 1.0),
        ("c", "C", "c", 1.0),
        ("v", "V", "v", 1.0),
        ("b", "B", "b", 1.0),
        ("n", "N", "n", 1.0),
        ("m", "M", "m", 1.0),
        (",", "<", "comma", 1.0),
        (".", ">", "period", 1.0),
        ("/", "?", "slash", 1.0),
        ("Shift", "Shift", "Shift", 1.5),
        ("↑", "↑", "Up", 1.0),
        ("Del", "Del", "Delete", 1.0),
    ],
    &[
        ("Ctrl", "Ctrl", "Ctrl", 1.5),
        ("Super", "Super", "Super", 1.5),
        ("Alt", "Alt", "Alt", 1.5),
        ("", "", "space", 8.5),
        ("←", "←", "Left", 1.0),
        ("↓", "↓", "Down", 1.0),
        ("→", "→", "Right", 1.0),
    ],
];

const MODIFIERS: [&str; 4] = ["Ctrl", "Alt", "Shift", "Super"];

const CSS: &str = "
.keyboard { padding: 10px; }
.keyboard button.key {
  min-height: 44px;
  padding: 0;
  border-radius: 8px;
  font-size: 16px;
  background: alpha(currentColor, .08);
}
.keyboard button.key:hover { background: alpha(currentColor, .14); }
.keyboard button.key:active { background: alpha(currentColor, .22); }
.keyboard button.key.word { font-size: 13px; }
.keyboard button.key.held { background: @window_fg_color; color: @window_bg_color; }
";

struct Board {
    /// The modifiers held for the next key.
    held: Vec<&'static str>,
    /// Caps Lock, kept here: letters are sent with Shift while it is on.
    caps: bool,
    /// Each key's button, with what it is.
    keys: Vec<(gtk::Button, Key)>,
}

impl Board {
    /// Shows the keys' Shift labels while Shift is held, and the held modifiers lit.
    fn relabel(&self) {
        let shift = self.held.contains(&"Shift");
        for (button, (label, shifted, name, _)) in &self.keys {
            let upper = shift != (self.caps && is_letter(name));
            button.set_label(if upper { shifted } else { label });
            if self.held.contains(name) || (*name == "Caps" && self.caps) {
                button.add_css_class("held");
            } else {
                button.remove_css_class("held");
            }
        }
    }
}

fn is_letter(name: &str) -> bool {
    name.len() == 1 && name.chars().all(|c| c.is_ascii_lowercase())
}

fn press(board: &Rc<RefCell<Board>>, name: &'static str) {
    let mut b = board.borrow_mut();
    if name == "Caps" {
        b.caps = !b.caps;
    } else if MODIFIERS.contains(&name) {
        match b.held.iter().position(|held| *held == name) {
            Some(at) => {
                b.held.remove(at);
            }
            None => b.held.push(name),
        }
    } else {
        let mut combo: Vec<&str> = b.held.clone();
        // Caps Lock turns Shift over for letters, as it does on a keyboard.
        if b.caps && is_letter(name) {
            match combo.iter().position(|held| *held == "Shift") {
                Some(at) => {
                    combo.remove(at);
                }
                None => combo.push("Shift"),
            }
        }
        combo.push(name);
        if ipc::msg(&format!("key {}", combo.join("+"))).is_none() {
            eprintln!("nekoland-keyboard: the compositor did not answer");
        }
        b.held.clear();
    }
    b.relabel();
}

fn open(app: &adw::Application) {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("螢幕鍵盤")
        .resizable(false)
        .build();
    let board = Rc::new(RefCell::new(Board {
        held: Vec::new(),
        caps: false,
        keys: Vec::new(),
    }));
    let rows = gtk::Box::new(gtk::Orientation::Vertical, GAP);
    rows.add_css_class("keyboard");
    for row in ROWS {
        let line = gtk::Box::new(gtk::Orientation::Horizontal, GAP);
        for key @ (label, _, name, units) in row.iter().copied() {
            let width = (units * f64::from(UNIT)) as i32 + ((units - 1.0).max(0.0) * f64::from(GAP)) as i32;
            let button = gtk::Button::builder().label(label).css_classes(["key"]).width_request(width).build();
            button.set_focusable(false);
            if label.chars().count() > 1 {
                button.add_css_class("word");
            }
            let board2 = board.clone();
            button.connect_clicked(move |_| press(&board2, name));
            board.borrow_mut().keys.push((button.clone(), key));
            line.append(&button);
        }
        rows.append(&line);
    }
    let header = adw::HeaderBar::builder().show_title(true).build();
    let view = adw::ToolbarView::new();
    view.add_top_bar(&header);
    view.set_content(Some(&rows));
    window.set_content(Some(&view));
    window.present();
}

fn main() -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_startup(|_| {
        let provider = gtk::CssProvider::new();
        provider.load_from_string(CSS);
        if let Some(display) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
        }
    });
    // Asked for again, it goes away, as the shortcut turns it off.
    app.connect_activate(|app| match app.active_window() {
        Some(window) => window.close(),
        None => open(app),
    });
    app.run()
}
