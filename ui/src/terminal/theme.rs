//! The screen's type and colours, as the settings say: Nekominal's colour schemes, its font, line height and cursor.

use gtk::{gdk, pango};
use vte4::prelude::*;

use std::cell::RefCell;

use super::{
    config::{self, Config, CursorShape, Profile, OPACITY_RANGE},
    scheme_import::OwnScheme,
};

/// The window's class, which the background's opacity is set on.
pub const WINDOW_CLASS: &str = "term-window";

thread_local! {
    /// The window's background at the opacity of the settings, replaced when they change.
    static OPACITY_CSS: gtk::CssProvider = gtk::CssProvider::new();
    /// The user's own schemes, each made once for every set of colours it has had: they live as long as the program, as
    /// the built-in ones do, and an import is rare.
    static OWN_SCHEMES: RefCell<Vec<&'static Scheme>> = RefCell::default();
}

pub struct Scheme {
    pub name: &'static str,
    pub foreground: &'static str,
    pub background: &'static str,
    pub cursor: &'static str,
    pub selection: &'static str,
    pub palette: [&'static str; 16],
}

/// Nekominal's schemes, in the order its settings show them; the first is the default.
pub const SCHEMES: [Scheme; 10] = [
    Scheme {
        name: "Tokyo Night",
        foreground: "#c0caf5",
        background: "#1a1b26",
        cursor: "#c0caf5",
        selection: "#33467c",
        palette: [
            "#15161e", "#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7", "#7dcfff", "#a9b1d6",
            "#414868", "#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7", "#7dcfff", "#c0caf5",
        ],
    },
    Scheme {
        name: "Campbell",
        foreground: "#cccccc",
        background: "#0c0c0c",
        cursor: "#cccccc",
        selection: "#ffffff40",
        palette: [
            "#0c0c0c", "#c50f1f", "#13a10e", "#c19c00", "#0037da", "#881798", "#3a96dd", "#cccccc",
            "#767676", "#e74856", "#16c60c", "#f9f1a5", "#3b78ff", "#b4009e", "#61d6d6", "#f2f2f2",
        ],
    },
    Scheme {
        name: "One Half Dark",
        foreground: "#dcdfe4",
        background: "#282c34",
        cursor: "#dcdfe4",
        selection: "#474e5d",
        palette: [
            "#282c34", "#e06c75", "#98c379", "#e5c07b", "#61afef", "#c678dd", "#56b6c2", "#dcdfe4",
            "#5a6374", "#e06c75", "#98c379", "#e5c07b", "#61afef", "#c678dd", "#56b6c2", "#dcdfe4",
        ],
    },
    Scheme {
        name: "Catppuccin Mocha",
        foreground: "#cdd6f4",
        background: "#1e1e2e",
        cursor: "#f5e0dc",
        selection: "#585b70",
        palette: [
            "#45475a", "#f38ba8", "#a6e3a1", "#f9e2af", "#89b4fa", "#f5c2e7", "#94e2d5", "#bac2de",
            "#585b70", "#f38ba8", "#a6e3a1", "#f9e2af", "#89b4fa", "#f5c2e7", "#94e2d5", "#a6adc8",
        ],
    },
    Scheme {
        name: "Dracula",
        foreground: "#f8f8f2",
        background: "#282a36",
        cursor: "#f8f8f2",
        selection: "#44475a",
        palette: [
            "#21222c", "#ff5555", "#50fa7b", "#f1fa8c", "#bd93f9", "#ff79c6", "#8be9fd", "#f8f8f2",
            "#6272a4", "#ff6e6e", "#69ff94", "#ffffa5", "#d6acff", "#ff92df", "#a4ffff", "#ffffff",
        ],
    },
    Scheme {
        name: "Nord",
        foreground: "#d8dee9",
        background: "#2e3440",
        cursor: "#d8dee9",
        selection: "#434c5e",
        palette: [
            "#3b4252", "#bf616a", "#a3be8c", "#ebcb8b", "#81a1c1", "#b48ead", "#88c0d0", "#e5e9f0",
            "#4c566a", "#bf616a", "#a3be8c", "#ebcb8b", "#81a1c1", "#b48ead", "#8fbcbb", "#eceff4",
        ],
    },
    Scheme {
        name: "Gruvbox Dark",
        foreground: "#ebdbb2",
        background: "#282828",
        cursor: "#ebdbb2",
        selection: "#504945",
        palette: [
            "#282828", "#cc241d", "#98971a", "#d79921", "#458588", "#b16286", "#689d6a", "#a89984",
            "#928374", "#fb4934", "#b8bb26", "#fabd2f", "#83a598", "#d3869b", "#8ec07c", "#ebdbb2",
        ],
    },
    Scheme {
        name: "Solarized Dark",
        foreground: "#839496",
        background: "#002b36",
        cursor: "#839496",
        selection: "#073642",
        palette: [
            "#073642", "#dc322f", "#859900", "#b58900", "#268bd2", "#d33682", "#2aa198", "#eee8d5",
            "#002b36", "#cb4b16", "#586e75", "#657b83", "#839496", "#6c71c4", "#93a1a1", "#fdf6e3",
        ],
    },
    Scheme {
        name: "One Half Light",
        foreground: "#383a42",
        background: "#fafafa",
        cursor: "#383a42",
        selection: "#bfceff",
        palette: [
            "#383a42", "#e45649", "#50a14f", "#c18401", "#0184bc", "#a626a4", "#0997b3", "#fafafa",
            "#4f525d", "#df6c75", "#98c379", "#e4c07a", "#61afef", "#c577dd", "#56b5c1", "#ffffff",
        ],
    },
    Scheme {
        name: "Solarized Light",
        foreground: "#657b83",
        background: "#fdf6e3",
        cursor: "#657b83",
        selection: "#eee8d5",
        palette: [
            "#073642", "#dc322f", "#859900", "#b58900", "#268bd2", "#d33682", "#2aa198", "#eee8d5",
            "#002b36", "#cb4b16", "#586e75", "#657b83", "#839496", "#6c71c4", "#93a1a1", "#fdf6e3",
        ],
    },
];

fn leak(text: &str) -> &'static str {
    Box::leak(text.to_owned().into_boxed_str())
}

fn has_colours_of(scheme: &Scheme, own: &OwnScheme) -> bool {
    scheme.name == own.name
        && scheme.foreground == own.foreground
        && scheme.background == own.background
        && scheme.cursor == own.cursor()
        && scheme.selection == own.selection()
        && scheme.palette == own.palette()
}

fn registered(own: &OwnScheme) -> &'static Scheme {
    OWN_SCHEMES.with_borrow_mut(|known| {
        if let Some(scheme) = known.iter().find(|scheme| has_colours_of(scheme, own)) {
            return *scheme;
        }
        let scheme: &'static Scheme = Box::leak(Box::new(Scheme {
            name: leak(&own.name),
            foreground: leak(&own.foreground),
            background: leak(&own.background),
            cursor: leak(own.cursor()),
            selection: leak(own.selection()),
            palette: own.palette().map(leak),
        }));
        known.push(scheme);
        scheme
    })
}

/// The built-in schemes, then the user's own in the order they were imported.
pub fn all_schemes() -> Vec<&'static Scheme> {
    let own = config::current().schemes;
    SCHEMES.iter().chain(own.iter().map(registered)).collect()
}

/// The scheme of that name, or the default when there is none: a name from an older or hand-edited file.
pub fn scheme_named(name: &str) -> &'static Scheme {
    all_schemes()
        .into_iter()
        .find(|scheme| scheme.name == name)
        .unwrap_or(&SCHEMES[0])
}

pub fn current_scheme() -> &'static Scheme {
    scheme_named(&config::current().scheme)
}

/// The colours a profile's screens take: its own scheme, or the window's.
pub fn scheme_of(profile: &Profile) -> &'static Scheme {
    match &profile.color_scheme {
        Some(name) => scheme_named(name),
        None => current_scheme(),
    }
}

pub fn colour(hex: &str) -> gdk::RGBA {
    gdk::RGBA::parse(hex).expect("the schemes hold valid colours")
}

/// Applies the settings to a screen, the profile's own colours and size over the window's; called again on every
/// screen when they change.
pub fn set_up(terminal: &impl IsA<vte4::Terminal>, profile: &Profile) {
    let terminal = terminal.as_ref();
    let config = config::current();
    terminal.set_font(Some(&font(&config, profile.font_size.unwrap_or(config.font_size))));
    terminal.set_cell_height_scale(config.line_height);
    terminal.set_scrollback_lines(i64::from(config.scrollback));
    terminal.set_cursor_shape(match config.cursor_shape {
        CursorShape::Block => vte4::CursorShape::Block,
        CursorShape::Bar => vte4::CursorShape::Ibeam,
        CursorShape::Underline => vte4::CursorShape::Underline,
    });
    terminal.set_cursor_blink_mode(if config.cursor_blink {
        vte4::CursorBlinkMode::On
    } else {
        vte4::CursorBlinkMode::Off
    });
    // VTE blinks at GTK's pace, a whole cycle; the setting is how long the cursor stays on, or off, as Nekominal's.
    gtk::Settings::for_display(&terminal.display())
        .set_gtk_cursor_blink_time((config.cursor_blink_rate * 2) as i32);
    // The bell is read as the pane wanting the user, not played.
    terminal.set_audible_bell(false);
    terminal.set_allow_hyperlink(true);
    terminal.set_mouse_autohide(true);
    terminal.set_scroll_on_keystroke(true);
    terminal.set_scroll_on_output(false);
    apply_colours(terminal, scheme_of(profile), opacity(&config));
}

/// Lets what is behind the window show through its background as the settings say; the screens' own backgrounds take
/// the same opacity in [`set_up`].
pub fn show_opacity(config: &Config) {
    let opacity = opacity(config);
    OPACITY_CSS.with(|provider| {
        provider.load_from_string(&format!(
            "window.{WINDOW_CLASS} {{ background-color: alpha(@window_bg_color, {opacity:.2}); }}"
        ));
        if let Some(display) = gdk::Display::default() {
            // A display keeps a provider once, however often it is added.
            gtk::style_context_add_provider_for_display(&display, provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
        }
    });
}

/// The opacity of the settings, a hand-edited one kept readable.
fn opacity(config: &Config) -> f64 {
    config.opacity.clamp(OPACITY_RANGE.0, OPACITY_RANGE.1)
}

fn font(config: &Config, size: f64) -> pango::FontDescription {
    let mut font = pango::FontDescription::new();
    font.set_family(&config.font_family);
    font.set_size((size * f64::from(pango::SCALE)).round() as i32);
    font
}

fn apply_colours(terminal: &vte4::Terminal, scheme: &Scheme, opacity: f64) {
    let palette: Vec<gdk::RGBA> = scheme.palette.iter().map(|hex| colour(hex)).collect();
    let palette: Vec<&gdk::RGBA> = palette.iter().collect();
    let mut background = colour(scheme.background);
    background.set_alpha(opacity as f32);
    terminal.set_colors(Some(&colour(scheme.foreground)), Some(&background), &palette);
    terminal.set_color_cursor(Some(&colour(scheme.cursor)));
    terminal.set_color_cursor_foreground(Some(&colour(scheme.background)));
    terminal.set_color_highlight(Some(&colour(scheme.selection)));
    terminal.set_color_highlight_foreground(Some(&colour(scheme.foreground)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_scheme_colour_parses() {
        for scheme in &SCHEMES {
            let colours = [scheme.foreground, scheme.background, scheme.cursor, scheme.selection];
            for hex in colours.iter().chain(&scheme.palette) {
                assert!(gdk::RGBA::parse(*hex).is_ok(), "{} has {hex}", scheme.name);
            }
        }
    }

    #[test]
    fn an_unknown_scheme_falls_back_to_the_default() {
        assert_eq!(scheme_named("Dracula").name, "Dracula");
        assert_eq!(scheme_named("gone").name, SCHEMES[0].name);
    }
}
