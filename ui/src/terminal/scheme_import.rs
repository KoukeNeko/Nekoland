//! Colour schemes brought in from other terminals, as Nekominal brings them: the `schemes` of a Windows Terminal
//! `settings.json` (comments and trailing commas allowed) that have a name not taken yet, and an iTerm2 preset
//! (`.itermcolors`), which takes the place of an imported scheme of its name so that importing it again takes its newer
//! colours. They are kept in the settings in Windows Terminal's own shape.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{config, theme};

const PALETTE_SIZE: usize = 16;
const SCHEMES_KEY: &str = "schemes";
const BYTE_ORDER_MARK: char = '\u{feff}';
/// Without a selection colour of its own, an iTerm2 preset selects in its foreground at this opacity.
const SELECTION_ALPHA: f64 = 0.25;
const ITERM_EXTENSION: &str = ".itermcolors";

/// Where schemes are brought in from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// The `schemes` of Windows Terminal's `settings.json`.
    WindowsTerminal,
    /// An iTerm2 preset, `.itermcolors`.
    Iterm,
}

impl Source {
    /// What the file dialog lists: the files of the kind, by their name.
    pub fn pattern(self) -> &'static str {
        match self {
            Source::WindowsTerminal => "*.json",
            Source::Iterm => "*.itermcolors",
        }
    }
}

/// Reads `path` and keeps its schemes in the settings; the names added.
pub fn import_file(path: &Path, source: Source) -> Result<Vec<String>, String> {
    let text = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
    match source {
        Source::WindowsTerminal => {
            let found = windows_terminal_schemes(&text)?;
            let mut added = Vec::new();
            config::update(|config| added = add_new(&mut config.schemes, found));
            Ok(added)
        }
        Source::Iterm => {
            let file_name = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
            let scheme = itermcolors_scheme(preset_name(file_name), &text)?;
            let name = scheme.name.clone();
            config::update(|config| replace_or_add(&mut config.schemes, scheme));
            Ok(vec![name])
        }
    }
}

/// A scheme of the user's own, in the fields Windows Terminal writes; each colour `#rrggbb`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnScheme {
    pub name: String,
    pub foreground: String,
    pub background: String,
    #[serde(default)]
    pub cursor_color: Option<String>,
    #[serde(default)]
    pub selection_background: Option<String>,
    pub black: String,
    pub red: String,
    pub green: String,
    pub yellow: String,
    pub blue: String,
    pub purple: String,
    pub cyan: String,
    pub white: String,
    pub bright_black: String,
    pub bright_red: String,
    pub bright_green: String,
    pub bright_yellow: String,
    pub bright_blue: String,
    pub bright_purple: String,
    pub bright_cyan: String,
    pub bright_white: String,
}

impl OwnScheme {
    pub fn palette(&self) -> [&str; PALETTE_SIZE] {
        [
            &self.black, &self.red, &self.green, &self.yellow, &self.blue, &self.purple, &self.cyan, &self.white,
            &self.bright_black, &self.bright_red, &self.bright_green, &self.bright_yellow, &self.bright_blue,
            &self.bright_purple, &self.bright_cyan, &self.bright_white,
        ]
    }

    pub fn cursor(&self) -> &str {
        self.cursor_color.as_deref().unwrap_or(&self.foreground)
    }

    pub fn selection(&self) -> &str {
        self.selection_background.as_deref().unwrap_or(&self.foreground)
    }

    fn is_valid(&self) -> bool {
        let optional = [self.cursor_color.as_deref(), self.selection_background.as_deref()];
        !self.name.trim().is_empty()
            && [self.foreground.as_str(), self.background.as_str()]
                .into_iter()
                .chain(self.palette())
                .chain(optional.into_iter().flatten())
                .all(is_hex_colour)
    }
}

/// `#rrggbb`, the one form Windows Terminal writes and the screen reads back.
fn is_hex_colour(text: &str) -> bool {
    text.strip_prefix('#').is_some_and(|hex| hex.len() == 6 && hex.chars().all(|digit| digit.is_ascii_hexdigit()))
}

/// Removes `//` and `/* */` comments outside of strings.
fn strip_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_string = false;
    while let Some(ch) = chars.next() {
        if in_string {
            out.push(ch);
            if ch == '\\' {
                out.extend(chars.next());
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match (ch, chars.peek()) {
            ('"', _) => {
                in_string = true;
                out.push(ch);
            }
            ('/', Some('/')) => {
                chars.by_ref().find(|&next| next == '\n');
                out.push('\n');
            }
            ('/', Some('*')) => {
                chars.next();
                let mut previous = ' ';
                chars.by_ref().find(|&next| std::mem::replace(&mut previous, next) == '*' && next == '/');
            }
            _ => out.push(ch),
        }
    }
    out
}

/// Removes the commas before a `}` or a `]`, outside of strings: Windows Terminal writes its file with them.
fn strip_trailing_commas(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut in_string = false;
    let mut index = 0;
    while index < chars.len() {
        let ch = chars[index];
        if in_string {
            if ch == '\\' {
                out.extend(chars.get(index..index + 2).unwrap_or(&chars[index..]));
                index += 2;
                continue;
            }
            in_string = ch != '"';
        } else if ch == '"' {
            in_string = true;
        } else if ch == ',' && matches!(chars[index + 1..].iter().find(|next| !next.is_whitespace()), Some('}' | ']')) {
            index += 1;
            continue;
        }
        out.push(ch);
        index += 1;
    }
    out
}

/// The schemes of a Windows Terminal `settings.json` that can be read; a scheme with a colour missing or not `#rrggbb`
/// is left out. What the parser said of a file that is not JSON.
pub fn windows_terminal_schemes(text: &str) -> Result<Vec<OwnScheme>, String> {
    let text = strip_trailing_commas(&strip_comments(text.trim_start_matches(BYTE_ORDER_MARK)));
    let document: serde_json::Value = serde_json::from_str(&text).map_err(|error| error.to_string())?;
    let schemes = document.get(SCHEMES_KEY).and_then(|schemes| schemes.as_array()).cloned().unwrap_or_default();
    Ok(schemes
        .into_iter()
        .filter_map(|scheme| serde_json::from_value::<OwnScheme>(scheme).ok())
        .filter(OwnScheme::is_valid)
        .collect())
}

/// One colour of a property list's `<dict>`: its three components, 0 to 1.
fn plist_colour(body: &str) -> Option<[f64; 3]> {
    let component = |name: &str| -> Option<f64> {
        let key = format!("<key>{name} Component</key>");
        let value = body[body.find(&key)? + key.len()..].trim_start();
        let number: f64 = value.get(value.find('>')? + 1..value.find("</")?)?.trim().parse().ok()?;
        number.is_finite().then(|| number.clamp(0.0, 1.0))
    };
    Some([component("Red")?, component("Green")?, component("Blue")?])
}

/// The colours of a property list by their keys: every `<key>` followed by a `<dict>` with the three components.
fn plist_colours(plist: &str) -> Vec<(String, [f64; 3])> {
    let mut colours = Vec::new();
    let mut rest = plist;
    while let Some(start) = rest.find("<key>") {
        let after = &rest[start + "<key>".len()..];
        let Some(end) = after.find("</key>") else { break };
        let key = after[..end].trim().to_string();
        rest = &after[end + "</key>".len()..];
        let Some(body) = rest.trim_start().strip_prefix("<dict>") else { continue };
        let Some(body_end) = body.find("</dict>") else { break };
        if let Some(colour) = plist_colour(&body[..body_end]) {
            colours.push((key, colour));
        }
        rest = &body[body_end + "</dict>".len()..];
    }
    colours
}

fn hex(colour: [f64; 3]) -> String {
    let byte = |component: f64| (component * f64::from(u8::MAX)).round() as u8;
    format!("#{:02x}{:02x}{:02x}", byte(colour[0]), byte(colour[1]), byte(colour[2]))
}

/// `over` laid on `under` at `alpha`: a colour the screen reads without an opacity of its own.
fn blend(over: [f64; 3], under: [f64; 3], alpha: f64) -> [f64; 3] {
    [0, 1, 2].map(|index| over[index] * alpha + under[index] * (1.0 - alpha))
}

/// The scheme of an iTerm2 preset named `name`; what is missing, when it is no preset.
pub fn itermcolors_scheme(name: &str, plist: &str) -> Result<OwnScheme, String> {
    let colours = plist_colours(plist);
    let get = |key: &str| colours.iter().find(|(known, _)| known == key).map(|(_, colour)| *colour);
    let required = |key: &str| get(key).ok_or_else(|| format!("no \"{key}\""));
    let mut ansi = Vec::with_capacity(PALETTE_SIZE);
    for index in 0..PALETTE_SIZE {
        ansi.push(hex(required(&format!("Ansi {index} Color"))?));
    }
    let foreground = required("Foreground Color")?;
    let background = required("Background Color")?;
    let selection = get("Selection Color").unwrap_or_else(|| blend(foreground, background, SELECTION_ALPHA));
    let [black, red, green, yellow, blue, purple, cyan, white, bright_black, bright_red, bright_green, bright_yellow, bright_blue, bright_purple, bright_cyan, bright_white] =
        <[String; PALETTE_SIZE]>::try_from(ansi).map_err(|_| "a palette of 16 colours".to_string())?;
    Ok(OwnScheme {
        name: name.trim().to_string(),
        foreground: hex(foreground),
        background: hex(background),
        cursor_color: Some(hex(get("Cursor Color").unwrap_or(foreground))),
        selection_background: Some(hex(selection)),
        black, red, green, yellow, blue, purple, cyan, white,
        bright_black, bright_red, bright_green, bright_yellow, bright_blue, bright_purple, bright_cyan, bright_white,
    })
}

/// A preset's name: its file's, without `.itermcolors`.
pub fn preset_name(file_name: &str) -> &str {
    file_name.strip_suffix(ITERM_EXTENSION).unwrap_or(file_name)
}

/// Adds the schemes whose names are not taken, built in or the user's own; the names added.
pub fn add_new(own: &mut Vec<OwnScheme>, found: Vec<OwnScheme>) -> Vec<String> {
    let mut added = Vec::new();
    for scheme in found {
        let taken = theme::SCHEMES.iter().any(|known| known.name == scheme.name)
            || own.iter().any(|known| known.name == scheme.name);
        if !taken {
            added.push(scheme.name.clone());
            own.push(scheme);
        }
    }
    added
}

/// Puts `scheme` in place of the user's own of its name, or after them.
pub fn replace_or_add(own: &mut Vec<OwnScheme>, scheme: OwnScheme) {
    match own.iter_mut().find(|known| known.name == scheme.name) {
        Some(known) => *known = scheme,
        None => own.push(scheme),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn windows_terminal_scheme(name: &str, background: &str) -> String {
        format!(
            r##"{{ "name": "{name}", "foreground": "#CCCCCC", "background": "{background}", "cursorColor": "#FFFFFF",
            "selectionBackground": "#FFFFFF", "black": "#0C0C0C", "red": "#C50F1F", "green": "#13A10E",
            "yellow": "#C19C00", "blue": "#0037DA", "purple": "#881798", "cyan": "#3A96DD", "white": "#CCCCCC",
            "brightBlack": "#767676", "brightRed": "#E74856", "brightGreen": "#16C60C", "brightYellow": "#F9F1A5",
            "brightBlue": "#3B78FF", "brightPurple": "#B4009E", "brightCyan": "#61D6D6", "brightWhite": "#F2F2F2", }}"##
        )
    }

    #[test]
    fn a_windows_terminal_file_with_comments_and_trailing_commas_is_read() {
        let text = format!(
            "\u{feff}{{\n  // Windows Terminal's own file\n  \"profiles\": {{ \"list\": [], }},\n  \"schemes\": [ {}, {}, /* a bad one */ {}, ],\n}}",
            windows_terminal_scheme("One // Half", "#282C34"),
            windows_terminal_scheme("Bad", "rgb(0,0,0)"),
            windows_terminal_scheme("Ocean", "#002B36"),
        );
        let names: Vec<String> = windows_terminal_schemes(&text).unwrap().into_iter().map(|scheme| scheme.name).collect();
        assert_eq!(names, ["One // Half", "Ocean"]);
        assert!(windows_terminal_schemes("not json").is_err());
    }

    #[test]
    fn only_names_not_taken_are_added() {
        let text = format!(
            r#"{{ "schemes": [ {}, {}, {} ] }}"#,
            windows_terminal_scheme("Campbell", "#0C0C0C"),
            windows_terminal_scheme("Mine", "#101010"),
            windows_terminal_scheme("Mine", "#202020"),
        );
        let mut own = Vec::new();
        assert_eq!(add_new(&mut own, windows_terminal_schemes(&text).unwrap()), ["Mine"]);
        assert_eq!(own[0].background, "#101010");
    }

    fn preset(entries: &[(&str, [f64; 3])]) -> String {
        let body: String = entries
            .iter()
            .map(|(key, [red, green, blue])| {
                format!(
                    "<key>{key}</key>\n<dict>\n<key>Blue Component</key>\n<real>{blue}</real>\n<key>Green Component</key>\n<real>{green}</real>\n<key>Red Component</key>\n<real>{red}</real>\n</dict>\n"
                )
            })
            .collect();
        format!("<?xml version=\"1.0\"?>\n<plist version=\"1.0\">\n<dict>\n{body}</dict>\n</plist>")
    }

    #[test]
    fn an_iterm_preset_gives_its_sixteen_colours_and_a_selection_of_its_own() {
        let ansi: Vec<String> = (0..PALETTE_SIZE).map(|index| format!("Ansi {index} Color")).collect();
        let mut entries: Vec<(&str, [f64; 3])> = ansi.iter().map(|key| (key.as_str(), [0.0, 0.0, 1.0])).collect();
        entries.extend([("Foreground Color", [1.0, 1.0, 1.0]), ("Background Color", [0.0, 0.0, 0.0])]);
        let scheme = itermcolors_scheme(" Deep Sea ", &preset(&entries)).unwrap();
        assert_eq!((scheme.name.as_str(), scheme.background.as_str()), ("Deep Sea", "#000000"));
        assert_eq!(scheme.bright_white, "#0000ff");
        assert_eq!(scheme.cursor(), "#ffffff");
        assert_eq!(scheme.selection(), "#404040");
        assert!(scheme.is_valid());
        entries.retain(|(key, _)| *key != "Ansi 7 Color");
        assert_eq!(itermcolors_scheme("x", &preset(&entries)), Err("no \"Ansi 7 Color\"".to_string()));
    }

    #[test]
    fn a_preset_imported_again_takes_its_newer_colours() {
        let mut own = Vec::new();
        let first = windows_terminal_schemes(&format!(r#"{{"schemes":[{}]}}"#, windows_terminal_scheme("A", "#111111"))).unwrap();
        let second = windows_terminal_schemes(&format!(r#"{{"schemes":[{}]}}"#, windows_terminal_scheme("A", "#222222"))).unwrap();
        replace_or_add(&mut own, first[0].clone());
        replace_or_add(&mut own, second[0].clone());
        assert_eq!((own.len(), own[0].background.as_str()), (1, "#222222"));
        assert_eq!(preset_name("Deep Sea.itermcolors"), "Deep Sea");
    }
}
