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
    /// Layer surfaces drawn on glass, by the namespace they give, with the corner radius
    /// they draw themselves with.
    pub glass: HashMap<String, i32>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            mode: None,
            autostart: Vec::new(),
            terminal: "foot".into(),
            launcher: "fuzzel".into(),
            launcher_search: "--search=".into(),
            volume_up: "wpctl set-volume -l 1.0 @DEFAULT_AUDIO_SINK@ 5%+".into(),
            volume_down: "wpctl set-volume @DEFAULT_AUDIO_SINK@ 5%-".into(),
            volume_mute: "wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle".into(),
            brightness_up: "brightnessctl set 5%+".into(),
            brightness_down: "brightnessctl set 5%-".into(),
            glass: HashMap::from([
                ("waybar".into(), 18),
                ("notifications".into(), 14),
                ("launcher".into(), 16),
            ]),
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
