//! Settings read once at startup from `$XDG_CONFIG_HOME/nekoland/config.toml`.

use std::{collections::HashMap, path::PathBuf, process::Command};

use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// The display mode to ask for, as `WIDTHxHEIGHT`; the display's preferred one otherwise.
    pub mode: Option<String>,
    /// Shell commands started once the compositor is up: the panel, the wallpaper, and so on.
    pub autostart: Vec<String>,
    pub terminal: String,
    pub launcher: String,
    /// What the launcher takes before the text to start searching with.
    pub launcher_search: String,
    pub volume_up: String,
    pub volume_down: String,
    pub volume_mute: String,
    pub brightness_up: String,
    pub brightness_down: String,
    /// Layer surfaces and windows drawn on glass, by the namespace or application ID they
    /// give, with the corner radius they draw themselves with.
    pub glass: HashMap<String, i32>,
    /// The applications kept in the Dock, by desktop entry name or application ID.
    pub dock: Vec<String>,
    /// The folders kept at the Dock's right end, before the Trash.
    pub stacks: Vec<Stack>,
}

/// A folder in the Dock, and how it shows what is in it, as the Mac's stacks do.
#[derive(Debug, Deserialize, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Stack {
    /// Where the folder is; `~` stands for the home folder.
    pub path: String,
    #[serde(default)]
    pub sort: Sort,
    #[serde(default)]
    pub show: Show,
    #[serde(default)]
    pub view: View,
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Sort {
    Name,
    #[default]
    Added,
    Modified,
    Created,
    Kind,
}

/// What the Dock shows for the folder: the folder's own icon, or its newest items piled up.
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Show {
    Folder,
    #[default]
    Stack,
}

/// How the folder's items open from the Dock.
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum View {
    #[default]
    Fan,
    Grid,
    List,
    Auto,
}

impl Sort {
    fn name(self) -> &'static str {
        match self {
            Sort::Name => "name",
            Sort::Added => "added",
            Sort::Modified => "modified",
            Sort::Created => "created",
            Sort::Kind => "kind",
        }
    }
}

impl Show {
    fn name(self) -> &'static str {
        match self {
            Show::Folder => "folder",
            Show::Stack => "stack",
        }
    }
}

impl View {
    fn name(self) -> &'static str {
        match self {
            View::Fan => "fan",
            View::Grid => "grid",
            View::List => "list",
            View::Auto => "auto",
        }
    }
}

impl Stack {
    pub fn new(path: String) -> Self {
        Self {
            path,
            sort: Sort::default(),
            show: Show::default(),
            view: View::default(),
        }
    }

    /// The folder's path with `~` and `$HOME` put in.
    pub fn folder(&self) -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_default();
        let path = match self.path.strip_prefix('~') {
            Some(rest) => format!("{home}{rest}"),
            None => self.path.replace("$HOME", &home),
        };
        PathBuf::from(path)
    }
}

/// The user's downloads folder, as `xdg-user-dirs` names it, written with `~`.
fn downloads() -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let named = std::fs::read_to_string(format!("{home}/.config/user-dirs.dirs"))
        .ok()
        .and_then(|text| {
            text.lines().find_map(|line| {
                let value = line.trim().strip_prefix("XDG_DOWNLOAD_DIR=")?;
                Some(value.trim_matches('"').replace("$HOME", "~"))
            })
        });
    named.unwrap_or_else(|| "~/Downloads".into())
}

impl Default for Config {
    fn default() -> Self {
        Self {
            mode: None,
            autostart: Vec::new(),
            terminal: "foot".into(),
            launcher: "nekoland-launcher".into(),
            launcher_search: "--search=".into(),
            volume_up: "wpctl set-volume -l 1.0 @DEFAULT_AUDIO_SINK@ 5%+".into(),
            volume_down: "wpctl set-volume @DEFAULT_AUDIO_SINK@ 5%-".into(),
            volume_mute: "wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle".into(),
            brightness_up: "brightnessctl set 5%+".into(),
            brightness_down: "brightnessctl set 5%-".into(),
            glass: HashMap::from([("nekoland-launcher".into(), 16)]),
            dock: [
                "org.gnome.Nautilus",
                "org.gnome.TextEditor",
                "org.gnome.Calculator",
                "foot",
            ]
            .map(String::from)
            .to_vec(),
            stacks: vec![Stack::new(downloads())],
        }
    }
}

impl Config {
    pub fn load() -> Self {
        let Some(path) = config_path() else {
            return Self::default();
        };
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(err) => {
                tracing::warn!("{}: {err}", path.display());
                return Self::default();
            }
        };
        toml::from_str(&text).unwrap_or_else(|err| {
            tracing::warn!("{}: {err}", path.display());
            Self::default()
        })
    }

    /// The size `mode` asks for.
    pub fn mode_size(&self) -> Option<(u16, u16)> {
        let (width, height) = self.mode.as_deref()?.split_once('x')?;
        Some((width.trim().parse().ok()?, height.trim().parse().ok()?))
    }
}

fn config_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| Some(PathBuf::from(std::env::var_os("HOME")?).join(".config")))?;
    Some(base.join("nekoland/config.toml"))
}

/// A file beside the configuration, for what Nekoland keeps between sessions.
pub fn config_file(name: &str) -> Option<PathBuf> {
    config_path().map(|path| path.with_file_name(name))
}

/// Writes the Dock's kept applications back to the configuration file.
pub fn save_dock(dock: &[String]) {
    let quoted: Vec<String> = dock.iter().map(|app_id| format!("{app_id:?}")).collect();
    save_setting("dock", &format!("dock = [{}]", quoted.join(", ")));
}

/// Writes the Dock's folders back to the configuration file.
pub fn save_stacks(stacks: &[Stack]) {
    save_setting("stacks", &stacks_line(stacks));
}

fn stacks_line(stacks: &[Stack]) -> String {
    let tables: Vec<String> = stacks
        .iter()
        .map(|stack| {
            format!(
                "{{ path = {:?}, sort = {:?}, show = {:?}, view = {:?} }}",
                stack.path,
                stack.sort.name(),
                stack.show.name(),
                stack.view.name()
            )
        })
        .collect();
    format!("stacks = [{}]", tables.join(", "))
}

/// Puts `line` in the configuration file in place of the setting `key`, or at its end.
// shortcut: edits the file as text, so a setting written across several lines other than
// one item per line is left as it is; a TOML writer if that comes up.
fn save_setting(key: &str, line: &str) {
    let Some(path) = config_path() else {
        return;
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => {
            tracing::warn!("{}: {err}", path.display());
            return;
        }
    };
    if let Err(err) = std::fs::create_dir_all(path.parent().unwrap_or(&path))
        .and_then(|_| std::fs::write(&path, with_setting(&text, key, line)))
    {
        tracing::warn!("{}: {err}", path.display());
    }
}

/// The configuration text with its `key = [...]` setting, on one line or several, replaced.
fn with_setting(text: &str, key: &str, line: &str) -> String {
    let mut out = Vec::new();
    let mut lines = text.lines();
    let mut replaced = false;
    while let Some(current) = lines.next() {
        let name = current.split('=').next().unwrap_or_default().trim();
        if name == key && !replaced {
            // An array that goes on past this line ends at the line closing it.
            let mut rest = current.to_string();
            while !rest.trim_end().ends_with(']') {
                match lines.next() {
                    Some(next) => rest = next.to_string(),
                    None => break,
                }
            }
            out.push(line.to_string());
            replaced = true;
        } else {
            out.push(current.to_string());
        }
    }
    if !replaced {
        out.push(line.to_string());
    }
    out.join("\n") + "\n"
}

/// Text quoted for a shell, as one word.
pub fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// Starts a shell command as a client of this compositor.
pub fn spawn(command: &str) {
    if let Err(err) = Command::new("sh").arg("-c").arg(command).spawn() {
        tracing::warn!("{command}: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dock_line_is_replaced_where_it_is() {
        let text = "terminal = \"foot\"\ndock = [\n    \"a\",\n    \"b\",\n]\nmode = \"1920x1080\"\n";
        assert_eq!(
            with_setting(text, "dock", "dock = [\"b\"]"),
            "terminal = \"foot\"\ndock = [\"b\"]\nmode = \"1920x1080\"\n"
        );
        assert_eq!(
            with_setting("terminal = \"foot\"\n", "dock", "dock = []"),
            "terminal = \"foot\"\ndock = []\n"
        );
    }

    #[test]
    fn stacks_are_written_back_as_they_are_read() {
        let stack = Stack {
            path: "~/下載".into(),
            sort: Sort::Kind,
            show: Show::Folder,
            view: View::Grid,
        };
        let stacks = vec![stack.clone(), Stack::new("~/Downloads".into())];
        let config: Config = toml::from_str(&stacks_line(&stacks)).unwrap();
        assert_eq!(config.stacks, stacks);
    }

    #[test]
    fn mode_size_parses_width_and_height() {
        let config = Config {
            mode: Some("1920x1080".into()),
            ..Config::default()
        };
        assert_eq!(config.mode_size(), Some((1920, 1080)));
    }

    #[test]
    fn mode_size_rejects_malformed_values() {
        for mode in ["1920", "x1080", "widexhigh"] {
            let config = Config {
                mode: Some(mode.into()),
                ..Config::default()
            };
            assert_eq!(config.mode_size(), None, "{mode}");
        }
    }

    #[test]
    fn glass_names_layers_with_their_corner_radius() {
        let config: Config = toml::from_str("[glass]\npanel = 0\ndock = 18").unwrap();
        assert_eq!(
            config.glass,
            HashMap::from([("panel".into(), 0), ("dock".into(), 18)])
        );
    }

    #[test]
    fn unknown_keys_are_rejected() {
        assert!(toml::from_str::<Config>("moed = \"1x1\"").is_err());
    }
}
