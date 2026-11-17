//! What the launcher keeps between runs, in `$XDG_DATA_HOME/nekoland/`: snippets,
//! quicklinks, notes, the aliases given to what it finds and how often each was used, and
//! the clipboard's history, which `--remember-clipboard` adds to as things are copied.

use std::{collections::HashMap, path::PathBuf};

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
    pub text: String,
    /// Seconds since the epoch.
    pub at: i64,
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

pub fn clipboard() -> Vec<Copied> {
    read("clipboard.json")
}

pub fn set_clipboard(history: &[Copied]) {
    write("clipboard.json", &history);
}

/// `--remember-clipboard`, which `wl-paste --watch` runs with what was copied on stdin:
/// puts it first in the history, unless a password manager said it is a secret.
pub fn remember_clipboard() {
    let mut text = String::new();
    if let Err(err) = std::io::Read::read_to_string(&mut std::io::stdin(), &mut text) {
        // Not text: a picture, say, which the history does not keep.
        eprintln!("clipboard: {err}");
        return;
    }
    let secret = std::process::Command::new("wl-paste")
        .arg("--list-types")
        .output()
        .is_ok_and(|output| String::from_utf8_lossy(&output.stdout).contains("x-kde-passwordManagerHint"));
    if secret || text.trim().is_empty() || text.len() > CLIPBOARD_LONGEST {
        return;
    }
    let mut history = clipboard();
    history.retain(|copied| copied.text != text);
    history.insert(0, Copied { text, at: now() });
    history.truncate(CLIPBOARD_KEPT);
    set_clipboard(&history);
}
