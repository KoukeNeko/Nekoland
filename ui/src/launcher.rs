//! `nekoland-launcher [--search=TEXT]`: finds an application by its name, in any of the
//! languages its desktop entry gives, and starts it.

use std::{
    path::{Path, PathBuf},
    rc::Rc,
};

use slint::{Image, ModelRc, VecModel};

slint::include_modules!();

/// How many matches are listed.
const SHOWN: usize = 8;
const ICON_THEMES: [&str; 3] = ["Yaru", "Adwaita", "hicolor"];
/// What an application without an icon of its own shows.
const NO_ICON: &str = "application-x-executable";

/// What the Appearance settings say the launcher looks like: how solid its glass is, and
/// the icon theme looked in before `ICON_THEMES`.
struct Look {
    glass_opacity: f32,
    icon_theme: Option<String>,
}

impl Look {
    fn load() -> Self {
        let config: toml::Table = std::process::Command::new("nekoland")
            .args(["msg", "config"])
            .output()
            .inspect_err(|err| eprintln!("nekoland msg config: {err}"))
            .ok()
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .and_then(|text| {
                toml::from_str(&text)
                    .inspect_err(|err| eprintln!("nekoland msg config: {err}"))
                    .ok()
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
        Self {
            glass_opacity,
            icon_theme,
        }
    }

    fn icon_themes(&self) -> Vec<&str> {
        self.icon_theme
            .as_deref()
            .into_iter()
            .chain(ICON_THEMES)
            .collect()
    }
}

#[derive(Clone)]
struct App {
    name: String,
    detail: String,
    /// Everything it can be found by, in lower case.
    words: String,
    exec: String,
    icon: Option<PathBuf>,
}

fn data_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join(".local/share"))
        .into_iter()
        .collect();
    let system = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    dirs.extend(system.split(':').filter(|dir| !dir.is_empty()).map(PathBuf::from));
    dirs
}

/// The value of a key in a desktop entry's main group, in the user's language if it has one.
fn value(entry: &str, key: &str, languages: &[String]) -> Option<String> {
    let lines: Vec<&str> = entry
        .lines()
        .skip_while(|line| line.trim() != "[Desktop Entry]")
        .skip(1)
        .take_while(|line| !line.starts_with('['))
        .collect();
    let find = |key: &str| {
        lines.iter().find_map(|line| {
            let (name, value) = line.split_once('=')?;
            (name.trim() == key).then(|| value.trim().to_string())
        })
    };
    languages
        .iter()
        .find_map(|language| find(&format!("{key}[{language}]")))
        .or_else(|| find(key))
}

/// The user's language, most specific first: zh_TW.UTF-8 gives zh_TW, then zh.
fn languages() -> Vec<String> {
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
fn strip_field_codes(exec: &str) -> String {
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

fn find_icon(dirs: &[PathBuf], themes: &[&str], icon: &str) -> Option<PathBuf> {
    let path = Path::new(icon);
    if path.is_absolute() {
        return path.is_file().then(|| path.to_path_buf());
    }
    for dir in dirs {
        for theme in themes {
            for size in ["scalable", "256x256", "128x128", "64x64", "48x48"] {
                for context in ["apps", "mimetypes"] {
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

/// Every application the user can start, once each, by name.
fn applications(themes: &[&str]) -> Vec<App> {
    let dirs = data_dirs();
    let languages = languages();
    let mut seen = std::collections::HashSet::new();
    let mut apps = Vec::new();
    for dir in &dirs {
        let Ok(entries) = std::fs::read_dir(dir.join("applications")) else {
            continue;
        };
        for path in entries.flatten().map(|entry| entry.path()) {
            if path.extension().and_then(|ext| ext.to_str()) != Some("desktop") {
                continue;
            }
            // The first directory to have an entry wins, as a user's own overrides the system's.
            let Some(id) = path.file_name().map(|name| name.to_string_lossy().to_string()) else {
                continue;
            };
            if !seen.insert(id) {
                continue;
            }
            let Ok(entry) = std::fs::read_to_string(&path) else {
                continue;
            };
            let get = |key: &str| value(&entry, key, &languages);
            let flag = |key: &str| get(key).is_some_and(|value| value == "true");
            if get("Type").as_deref() != Some("Application") || flag("NoDisplay") || flag("Hidden") {
                continue;
            }
            let (Some(name), Some(exec)) = (get("Name"), get("Exec")) else {
                continue;
            };
            // A generic name that only says the name again says nothing.
            let detail = get("GenericName")
                .filter(|generic| !generic.eq_ignore_ascii_case(&name))
                .or_else(|| get("Comment"))
                .unwrap_or_default();
            let words = [
                name.clone(),
                value(&entry, "Name", &[]).unwrap_or_default(),
                detail.clone(),
                get("Keywords").unwrap_or_default(),
                value(&entry, "Keywords", &[]).unwrap_or_default(),
                exec.split_whitespace().next().unwrap_or_default().to_string(),
            ]
            .join(" ")
            .to_lowercase();
            apps.push(App {
                name,
                detail,
                words,
                exec: strip_field_codes(&exec),
                icon: get("Icon").and_then(|icon| find_icon(&dirs, themes, &icon)),
            });
        }
    }
    apps.sort_by_key(|app| app.name.to_lowercase());
    apps
}

/// The applications for what was typed, none before anything is: names starting with it
/// first, then names with a word starting with it, then the rest that mention it anywhere.
fn matching<'a>(apps: &'a [App], query: &str) -> Vec<&'a App> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Vec::new();
    }
    let mut found: Vec<&App> = apps.iter().filter(|app| app.words.contains(&query)).collect();
    found.sort_by_key(|app| {
        let name = app.name.to_lowercase();
        if name.starts_with(&query) {
            0
        } else if name.split_whitespace().any(|word| word.starts_with(&query)) {
            1
        } else {
            2
        }
    });
    found.truncate(SHOWN);
    found
}

fn rows(apps: &[&App], no_icon: &Image) -> ModelRc<AppRow> {
    let rows: Vec<AppRow> = apps
        .iter()
        .map(|app| AppRow {
            name: app.name.clone().into(),
            detail: app.detail.clone().into(),
            icon: app
                .icon
                .as_deref()
                .and_then(|path| Image::load_from_path(path).ok())
                .unwrap_or_else(|| no_icon.clone()),
        })
        .collect();
    ModelRc::from(Rc::new(VecModel::from(rows)))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let look = Look::load();
    let themes = look.icon_themes();
    let apps = Rc::new(applications(&themes));
    let no_icon = find_icon(&data_dirs(), &themes, NO_ICON)
        .and_then(|path| Image::load_from_path(&path).ok())
        .unwrap_or_default();
    let search = std::env::args()
        .find_map(|argument| argument.strip_prefix("--search=").map(String::from))
        .unwrap_or_default();

    let launcher = Launcher::new()?;
    slint::set_xdg_app_id("nekoland-launcher")?;
    launcher.set_glass_opacity(look.glass_opacity);
    launcher.set_query(search.clone().into());
    launcher.set_apps(rows(&matching(&apps, &search), &no_icon));
    launcher.set_current(0);

    let weak = launcher.as_weak();
    let shown = apps.clone();
    launcher.on_edited(move |query| {
        if let Some(launcher) = weak.upgrade() {
            launcher.set_apps(rows(&matching(&shown, &query), &no_icon));
            launcher.set_current(0);
        }
    });

    let weak = launcher.as_weak();
    launcher.on_chosen(move |index| {
        let Some(launcher) = weak.upgrade() else {
            return;
        };
        let query = launcher.get_query();
        if let Some(app) = matching(&apps, &query).get(index as usize) {
            if let Err(err) = std::process::Command::new("sh").arg("-c").arg(&app.exec).spawn() {
                eprintln!("{}: {err}", app.exec);
            }
        }
        let _ = slint::quit_event_loop();
    });
    launcher.on_cancelled(|| {
        let _ = slint::quit_event_loop();
    });
    launcher.run()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(name: &str, words: &str) -> App {
        App {
            name: name.into(),
            detail: String::new(),
            words: format!("{} {}", name.to_lowercase(), words),
            exec: String::new(),
            icon: None,
        }
    }

    #[test]
    fn names_that_start_with_the_query_come_first() {
        let apps = [
            app("Text Editor", "gedit"),
            app("Terminal", "foot"),
            app("Files", "terminal"),
        ];
        let found: Vec<&str> = matching(&apps, "ter")
            .iter()
            .map(|app| app.name.as_str())
            .collect();
        assert_eq!(found, ["Terminal", "Files"]);
    }

    #[test]
    fn a_word_of_the_name_comes_before_the_rest() {
        let apps = [
            app("Files", "text"),
            app("Text Editor", "gedit"),
            app("GNOME Text", ""),
        ];
        let found: Vec<&str> = matching(&apps, "tex")
            .iter()
            .map(|app| app.name.as_str())
            .collect();
        assert_eq!(found, ["Text Editor", "GNOME Text", "Files"]);
        assert!(matching(&apps, " ").is_empty());
    }

    #[test]
    fn localized_values_win_over_the_default() {
        let entry = "[Desktop Entry]\nName=Files\nName[zh_TW]=檔案\n";
        assert_eq!(
            value(entry, "Name", &["zh_TW".into(), "zh".into()]).as_deref(),
            Some("檔案")
        );
        assert_eq!(value(entry, "Name", &[]).as_deref(), Some("Files"));
    }
}
