//! The tabs open when the terminal last ran, kept for the next start when the settings ask for it, as Nekominal's
//! session restore: each window's tabs, where each one was, its name, its colour and its profile.

use std::{fs, io, path::PathBuf};

use serde::{Deserialize, Serialize};

use super::config;

const FILE: &str = "terminal-session.json";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SavedTab {
    pub directory: Option<PathBuf>,
    pub title: Option<String>,
    pub colour: Option<String>,
    pub profile: Option<String>,
}

pub type SavedWindow = Vec<SavedTab>;

fn path() -> PathBuf {
    config::path().with_file_name(FILE)
}

pub fn save(windows: &[SavedWindow]) {
    let write = || -> io::Result<()> {
        let text = serde_json::to_string_pretty(windows).map_err(io::Error::other)?;
        fs::write(path(), text)
    };
    if let Err(error) = write() {
        eprintln!("nekoland-terminal: could not save the session to {}: {error}", path().display());
    }
}

/// The windows saved, the ones without a tab left out; none when nothing was saved.
pub fn load() -> Vec<SavedWindow> {
    let text = match fs::read_to_string(path()) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Vec::new(),
        Err(error) => {
            eprintln!("nekoland-terminal: could not read {}: {error}", path().display());
            return Vec::new();
        }
    };
    parse(&text)
}

fn parse(text: &str) -> Vec<SavedWindow> {
    match serde_json::from_str::<Vec<SavedWindow>>(text) {
        Ok(windows) => windows.into_iter().filter(|tabs| !tabs.is_empty()).collect(),
        Err(error) => {
            eprintln!("nekoland-terminal: the saved session is not valid: {error}");
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_saved_session_reads_back_without_its_empty_windows() {
        let tab = SavedTab {
            directory: Some(PathBuf::from("/tmp")),
            title: Some("build".to_string()),
            colour: Some("blue".to_string()),
            profile: None,
        };
        let text = serde_json::to_string(&vec![vec![tab.clone()], Vec::new()]).unwrap();
        assert_eq!(parse(&text), vec![vec![tab]]);
        assert!(parse("not json").is_empty());
    }
}
