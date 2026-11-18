//! Saved window layouts, as Nekominal's: the tabs of a window kept under a name, to be opened again in a window. They
//! live in a file of their own beside the settings, rewritten whenever one is saved, renamed or deleted.

use std::{collections::BTreeMap, fs, io, path::PathBuf};

use serde::{Deserialize, Serialize};

use super::{config, restore::SavedWindow};

const FILE: &str = "terminal-layouts.json";
const VERSION: u32 = 1;

/// The layouts by name, in the order of their names, as the palette and the settings list them.
pub type Layouts = BTreeMap<String, SavedWindow>;

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Document {
    version: u32,
    layouts: Layouts,
}

fn path() -> PathBuf {
    config::path().with_file_name(FILE)
}

/// The layouts saved; none when the file is missing or cannot be read, which is told.
pub fn load() -> Layouts {
    let text = match fs::read_to_string(path()) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Layouts::new(),
        Err(error) => {
            eprintln!("nekoland-terminal: could not read {}: {error}", path().display());
            return Layouts::new();
        }
    };
    parse(&text)
}

/// The layouts of a file, the ones without a name or a tab left out.
fn parse(text: &str) -> Layouts {
    match serde_json::from_str::<Document>(text.trim_start_matches('\u{feff}')) {
        Ok(document) => document
            .layouts
            .into_iter()
            .filter(|(name, tabs)| !name.trim().is_empty() && !tabs.is_empty())
            .collect(),
        Err(error) => {
            eprintln!("nekoland-terminal: the saved layouts are not valid: {error}");
            Layouts::new()
        }
    }
}

fn write(layouts: Layouts) -> io::Result<()> {
    let text = serde_json::to_string_pretty(&Document { version: VERSION, layouts }).map_err(io::Error::other)?;
    fs::write(path(), format!("{text}\n"))
}

/// Keeps `tabs` under `name`, in place of a layout of that name.
pub fn save(name: &str, tabs: SavedWindow) -> io::Result<()> {
    let mut layouts = load();
    layouts.insert(name.trim().to_string(), tabs);
    write(layouts)
}

/// Gives the layout `old` the name `new`; a layout already called `new` is replaced.
pub fn rename(old: &str, new: &str) -> io::Result<()> {
    let mut layouts = load();
    if let Some(tabs) = layouts.remove(old) {
        layouts.insert(new.trim().to_string(), tabs);
    }
    write(layouts)
}

pub fn delete(name: &str) -> io::Result<()> {
    let mut layouts = load();
    layouts.remove(name);
    write(layouts)
}

/// Whether a name can be given to a layout.
pub fn is_valid_name(name: &str) -> bool {
    !name.trim().is_empty()
}

#[cfg(test)]
mod tests {
    use super::super::restore::SavedTab;
    use super::*;

    #[test]
    fn layouts_read_back_without_the_ones_that_cannot_be_opened() {
        let tab = SavedTab { title: Some("build".into()), ..SavedTab::default() };
        let layouts = Layouts::from([
            ("agents".to_string(), vec![tab.clone()]),
            ("empty".to_string(), Vec::new()),
            (" ".to_string(), vec![tab.clone()]),
        ]);
        let text = serde_json::to_string(&Document { version: VERSION, layouts }).unwrap();
        assert_eq!(parse(&text), Layouts::from([("agents".to_string(), vec![tab])]));
        assert!(parse("not json").is_empty());
    }
}
