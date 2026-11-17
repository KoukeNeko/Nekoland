//! The applications the user can start, from their desktop entries, and where icons are.

use std::path::{Path, PathBuf};

use gtk::{gio, prelude::*};

pub const ICON_THEMES: [&str; 4] = ["Nekoland", "Yaru", "Adwaita", "hicolor"];
/// What an application without an icon of its own shows.
pub const NO_ICON: &str = "application-x-executable";

/// What the Appearance settings say the launcher looks like: how solid its glass is, and
/// the icon theme looked in before `ICON_THEMES`.
pub struct Look {
    pub glass_opacity: f32,
    icon_theme: Option<String>,
}

impl Look {
    pub fn load() -> Self {
        let config: toml::Table = std::process::Command::new("nekoland")
            .args(["msg", "config"])
            .output()
            .inspect_err(|err| eprintln!("nekoland msg config: {err}"))
            .ok()
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .and_then(|text| {
                toml::from_str(&text).inspect_err(|err| eprintln!("nekoland msg config: {err}")).ok()
            })
            .unwrap_or_default();
        let glass_opacity = match config.get("glass_opacity") {
            Some(toml::Value::Integer(number)) => *number as f32,
            value => value.and_then(toml::Value::as_float).unwrap_or(0.8) as f32,
        };
        let icon_theme = config
            .get("icon_theme")
            .and_then(toml::Value::as_str)
            .filter(|theme| !theme.is_empty())
            .map(String::from);
        Self { glass_opacity, icon_theme }
    }

    pub fn icon_themes(&self) -> Vec<&str> {
        self.icon_theme.as_deref().into_iter().chain(ICON_THEMES).collect()
    }
}

#[derive(Clone)]
pub struct App {
    /// Its desktop entry's file name, which stays as its name is translated.
    pub id: String,
    pub name: String,
    pub detail: String,
    /// Everything it can be found by, in lower case.
    pub words: String,
    pub exec: String,
    pub icon: Option<PathBuf>,
}

pub fn data_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> =
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")).into_iter().collect();
    let system = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    dirs.extend(system.split(':').filter(|dir| !dir.is_empty()).map(PathBuf::from));
    dirs
}

/// The user's language, most specific first: zh_TW.UTF-8 gives zh_TW, then zh.
pub fn languages() -> Vec<String> {
    let lang = std::env::var("LC_ALL")
        .or_else(|_| std::env::var("LC_MESSAGES"))
        .or_else(|_| std::env::var("LANG"))
        .unwrap_or_default();
    let lang = lang.split('.').next().unwrap_or_default().to_string();
    let mut languages = vec![lang.clone()];
    if let Some((short, _)) = lang.split_once('_') {
        languages.push(short.to_string());
    }
    languages.retain(|language| !language.is_empty() && language != "C" && language != "POSIX");
    languages
}

/// A desktop entry's Exec without the %f, %U and the like a launcher would fill in.
pub fn strip_field_codes(exec: &str) -> String {
    let mut command = String::with_capacity(exec.len());
    let mut chars = exec.chars();
    while let Some(c) = chars.next() {
        match (c, chars.clone().next()) {
            ('%', Some('%')) => {
                command.push('%');
                chars.next();
            }
            ('%', Some(_)) => {
                chars.next();
            }
            _ => command.push(c),
        }
    }
    command.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn find_icon(dirs: &[PathBuf], themes: &[&str], icon: &str) -> Option<PathBuf> {
    let path = Path::new(icon);
    if path.is_absolute() {
        return path.is_file().then(|| path.to_path_buf());
    }
    for dir in dirs {
        for theme in themes {
            for size in ["scalable", "256x256", "128x128", "64x64", "48x48"] {
                // Apps first; the rest for the launcher's own commands.
                for context in ["apps", "mimetypes", "actions", "places", "status", "devices", "categories"] {
                    for ext in ["svg", "png"] {
                        let candidate = dir.join(format!("icons/{theme}/{size}/{context}/{icon}.{ext}"));
                        if candidate.is_file() {
                            return Some(candidate);
                        }
                    }
                }
            }
        }
        let candidate = dir.join(format!("pixmaps/{icon}.png"));
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Every application the user can start, once each, by name, as GIO lists them: the same
/// ones the app grid shows, those kept to other desktops left out.
pub fn applications(themes: &[&str]) -> Vec<App> {
    let dirs = data_dirs();
    let mut apps: Vec<App> = gio::AppInfo::all()
        .into_iter()
        .filter(|info| info.should_show())
        .filter_map(|info| {
            let entry = info.downcast::<gio::DesktopAppInfo>().ok()?;
            let id = entry.id()?.to_string();
            let name = entry.name().to_string();
            let exec = entry.commandline()?.to_string_lossy().into_owned();
            // A generic name that only says the name again says nothing.
            let detail = entry
                .generic_name()
                .map(String::from)
                .filter(|generic| !generic.eq_ignore_ascii_case(&name))
                .or_else(|| entry.description().map(String::from))
                .unwrap_or_default();
            // Found by its name and keywords in the user's language and in the entry's own.
            let words = [
                name.clone(),
                entry.string("Name").map(String::from).unwrap_or_default(),
                detail.clone(),
                entry.keywords().join(" "),
                entry.string("Keywords").map(String::from).unwrap_or_default(),
                exec.split_whitespace().next().unwrap_or_default().to_string(),
            ]
            .join(" ")
            .to_lowercase();
            let icon = entry
                .string("Icon")
                .and_then(|icon| find_icon(&dirs, themes, &icon));
            Some(App {
                id,
                name,
                detail,
                words,
                exec: strip_field_codes(&exec),
                icon,
            })
        })
        .collect();
    apps.sort_by_key(|app| app.name.to_lowercase());
    apps
}
