//! What the launcher keeps between runs, in `$XDG_DATA_HOME/nekoland/`: snippets,
//! quicklinks, notes, the aliases given to what it finds and how often each was used, and
//! the clipboard's history, which `--remember-clipboard` adds to as things are copied.

use std::{collections::HashMap, path::PathBuf};

use gtk::{gio, prelude::*};

use serde::{Deserialize, Serialize};

/// How many things copied the clipboard's history keeps.
const CLIPBOARD_KEPT: usize = 300;
/// Longer than any text worth finding again; a bigger copy is left out of the history.
const CLIPBOARD_LONGEST: usize = 256 << 10;

#[derive(Clone, Default, Deserialize, Serialize)]
pub struct Snippet {
    pub name: String,
    pub text: String,
    /// What it is also found by.
    #[serde(default)]
    pub keyword: String,
}

#[derive(Clone, Default, Deserialize, Serialize)]
pub struct Quicklink {
    pub name: String,
    /// Opened with what is typed after its keyword in place of `{query}`.
    pub url: String,
    #[serde(default)]
    pub keyword: String,
}

#[derive(Clone, Default, Deserialize, Serialize)]
pub struct Note {
    pub text: String,
    /// Seconds since the epoch.
    pub changed: i64,
}

#[derive(Default, Deserialize, Serialize)]
pub struct Store {
    #[serde(default)]
    pub snippets: Vec<Snippet>,
    /// None until the user has any but the ones it starts with.
    #[serde(default)]
    pub quicklinks: Option<Vec<Quicklink>>,
    #[serde(default)]
    pub notes: Vec<Note>,
    /// What each thing found is also called, by its key.
    #[serde(default)]
    pub aliases: HashMap<String, String>,
    /// How often each thing found was chosen, by its key, so the most used come first.
    #[serde(default)]
    pub uses: HashMap<String, u32>,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct Copied {
    /// What was copied as text; for files, their paths, and for a picture, what it is.
    pub text: String,
    /// Seconds since the epoch.
    pub at: i64,
    /// Files copied, as a file manager copies them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<PathBuf>,
    /// A picture copied, kept in a file of its own named for its type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub picture: Option<PathBuf>,
}

impl Copied {
    /// The type a picture copied was offered as, from its file's name.
    pub fn picture_type(&self) -> Option<String> {
        let kind = self.picture.as_ref()?.extension()?.to_str()?;
        Some(format!("image/{kind}"))
    }
}

fn data_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))?;
    Some(base.join("nekoland"))
}

fn read<T: for<'a> Deserialize<'a> + Default>(name: &str) -> T {
    let Some(path) = data_dir().map(|dir| dir.join(name)) else {
        return T::default();
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|err| {
            eprintln!("{}: {err}", path.display());
            T::default()
        }),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => T::default(),
        Err(err) => {
            eprintln!("{}: {err}", path.display());
            T::default()
        }
    }
}

/// Writes beside the file and renames it into place, so a crash leaves the old one whole.
fn write<T: Serialize>(name: &str, value: &T) {
    let Some(dir) = data_dir() else {
        return;
    };
    let path = dir.join(name);
    let partial = dir.join(format!("{name}.partial"));
    let written = std::fs::create_dir_all(&dir)
        .and_then(|()| serde_json::to_vec_pretty(value).map_err(std::io::Error::other))
        .and_then(|bytes| std::fs::write(&partial, bytes))
        .and_then(|()| std::fs::rename(&partial, &path));
    if let Err(err) = written {
        eprintln!("{}: {err}", path.display());
    }
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64)
}

impl Store {
    pub fn load() -> Self {
        read("launcher.json")
    }

    pub fn save(&self) {
        write("launcher.json", self);
    }

    pub fn quicklinks(&self) -> Vec<Quicklink> {
        self.quicklinks.clone().unwrap_or_else(|| {
            [
                ("Google 搜尋", "https://www.google.com/search?q={query}", "g"),
                ("YouTube", "https://www.youtube.com/results?search_query={query}", "yt"),
                ("維基百科", "https://zh.wikipedia.org/w/index.php?search={query}", "wiki"),
                ("Google 地圖", "https://www.google.com/maps/search/{query}", "map"),
                ("Google 翻譯", "https://translate.google.com/?sl=auto&tl=zh-TW&text={query}", "tr"),
                ("GitHub", "https://github.com/search?q={query}", "gh"),
            ]
            .map(|(name, url, keyword)| Quicklink {
                name: name.into(),
                url: url.into(),
                keyword: keyword.into(),
            })
            .to_vec()
        })
    }

    pub fn used(&mut self, key: &str) {
        *self.uses.entry(key.to_string()).or_default() += 1;
        self.save();
    }
}

/// What the history holds, without what is older than Settings keeps.
pub fn clipboard() -> Vec<Copied> {
    let mut history: Vec<Copied> = read("clipboard.json");
    if let Some(minutes) = kept_minutes() {
        history.retain(|copied| copied.at >= now() - minutes * 60);
    }
    history
}

/// Keeps `history`, and lets go of the pictures it no longer holds.
pub fn set_clipboard(history: &[Copied]) {
    write("clipboard.json", &history);
    let Some(folder) = pictures_dir() else { return };
    let Ok(entries) = std::fs::read_dir(&folder) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if !history.iter().any(|copied| copied.picture.as_ref() == Some(&path)) {
            if let Err(err) = std::fs::remove_file(&path) {
                eprintln!("{}: {err}", path.display());
            }
        }
    }
}

fn pictures_dir() -> Option<PathBuf> {
    data_dir().map(|dir| dir.join("clipboard"))
}

/// How many minutes the history keeps what was copied, as Settings sets it in the
/// compositor's configuration: None without end, and 0 for nothing at all.
fn kept_minutes() -> Option<i64> {
    let output = std::process::Command::new("nekoland").args(["msg", "config"]).output().ok()?;
    let config: toml::Table = toml::from_str(&String::from_utf8_lossy(&output.stdout)).ok()?;
    config.get("clipboard_history")?.as_integer().filter(|minutes| *minutes >= 0)
}

fn wl_paste(arguments: &[&str]) -> Option<Vec<u8>> {
    let output = std::process::Command::new("wl-paste").args(arguments).output();
    match output {
        Ok(output) if output.status.success() => Some(output.stdout),
        Ok(_) => None,
        Err(err) => {
            eprintln!("wl-paste: {err}");
            None
        }
    }
}

/// `--remember-clipboard`, which `wl-paste --watch` runs when something is copied: puts
/// it first in the history in the most useful of the kinds it was copied as, files, text
/// or a picture, unless a password manager said it is a secret.
pub fn remember_clipboard() {
    // What wl-paste hands over is asked for again below in the kind kept; read, so that
    // wl-paste is not cut off writing it.
    if let Err(err) = std::io::copy(&mut std::io::stdin(), &mut std::io::sink()) {
        eprintln!("clipboard: {err}");
    }
    let minutes = kept_minutes();
    if minutes == Some(0) {
        return;
    }
    let types = String::from_utf8_lossy(&wl_paste(&["--list-types"]).unwrap_or_default()).into_owned();
    let types: Vec<&str> = types.lines().collect();
    if types.contains(&"x-kde-passwordManagerHint") {
        return;
    }
    let at = now();
    let files: Vec<PathBuf> = types
        .contains(&"text/uri-list")
        .then(|| wl_paste(&["--no-newline", "--type", "text/uri-list"]))
        .flatten()
        .map(|list| {
            String::from_utf8_lossy(&list)
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty() && !line.starts_with('#'))
                .filter_map(|uri| gio::File::for_uri(uri).path())
                .collect()
        })
        .unwrap_or_default();
    let is_text = |kind: &&str| kind.starts_with("text/plain") || ["UTF8_STRING", "STRING", "TEXT"].contains(kind);
    let copied = if !files.is_empty() {
        let text = files.iter().map(|path| path.display().to_string()).collect::<Vec<_>>().join("\n");
        Copied { text, at, files, picture: None }
    } else if types.iter().any(is_text) {
        let Some(text) = wl_paste(&["--no-newline", "--type", "text"]) else { return };
        let text = String::from_utf8_lossy(&text).into_owned();
        if text.trim().is_empty() || text.len() > CLIPBOARD_LONGEST {
            return;
        }
        Copied { text, at, files: Vec::new(), picture: None }
    } else if let Some(kind) = types.iter().find_map(|kind| kind.strip_prefix("image/")) {
        let Some(picture) = keep_picture(kind) else { return };
        let text = match gtk::gdk_pixbuf::Pixbuf::file_info(&picture) {
            Some((_, width, height)) => format!("圖片 {width} × {height}"),
            None => "圖片".into(),
        };
        Copied { text, at, files: Vec::new(), picture: Some(picture) }
    } else {
        return;
    };
    let mut history = clipboard();
    history.retain(|old| (&old.text, &old.files, &old.picture) != (&copied.text, &copied.files, &copied.picture));
    history.insert(0, copied);
    history.truncate(CLIPBOARD_KEPT);
    set_clipboard(&history);
}

/// The picture on the clipboard, offered as `image/{kind}`, kept in a file named for what
/// it holds, so that the same picture copied again is kept once.
fn keep_picture(kind: &str) -> Option<PathBuf> {
    use std::hash::{Hash, Hasher};
    if !kind.chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c)) {
        return None;
    }
    let bytes = wl_paste(&["--type", &format!("image/{kind}")])?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    let folder = pictures_dir()?;
    let path = folder.join(format!("{:016x}.{kind}", hasher.finish()));
    let written = std::fs::create_dir_all(&folder).and_then(|()| std::fs::write(&path, &bytes));
    if let Err(err) = written {
        eprintln!("{}: {err}", path.display());
        return None;
    }
    Some(path)
}
