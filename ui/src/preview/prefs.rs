//! What 預覽程式 keeps between runs: the settings of its 設定 window, as the Mac's has them,
//! and what it remembers of the PDFs read, the page last shown and the bookmarks put in.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use gtk::glib;
use serde::{Deserialize, Serialize};

/// How a PDF's pages are laid out, as 顯示方式 chooses.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum Layout {
    #[default]
    Continuous,
    Single,
    Two,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    /// The window's background behind the pages and pictures, as CSS writes a colour; none
    /// keeps the theme's.
    pub background: Option<String>,
    /// Whether files opened together share a window, or each has its own.
    pub group_files: bool,
    /// Whether a picture's 100% is its size printed, rather than a pixel to a point.
    pub image_print_size: bool,
    /// Whether a PDF's 100% is its size printed, rather than a point to a pixel.
    pub pdf_print_size: bool,
    /// Whether a PDF opens at the page last shown.
    pub resume: bool,
    pub first_layout: Layout,
    /// Whether pages are numbered as the PDF labels them (i, ii, … then 1).
    pub logical_numbers: bool,
    /// Whether annotations carry `author`'s name.
    pub sign_annotations: bool,
    pub author: String,
    /// Per PDF, by its path: the page last shown, 0-based.
    pub last_page: HashMap<PathBuf, usize>,
    /// Per PDF, by its path: the pages bookmarked, 0-based, each with its name.
    pub bookmarks: HashMap<PathBuf, Vec<(usize, String)>>,
    /// The signatures made, each a picture in `signatures_dir` and what it is called.
    pub signatures: Vec<(String, String)>,
}

impl Default for Prefs {
    fn default() -> Prefs {
        Prefs {
            background: None,
            group_files: true,
            image_print_size: false,
            pdf_print_size: false,
            resume: true,
            first_layout: Layout::Continuous,
            logical_numbers: false,
            sign_annotations: true,
            author: glib::real_name().to_string_lossy().into_owned(),
            last_page: HashMap::new(),
            bookmarks: HashMap::new(),
            signatures: Vec::new(),
        }
    }
}

fn prefs_path() -> PathBuf {
    glib::user_config_dir().join("nekoland/preview.json")
}

/// Where the signatures made are kept, a PNG each.
pub fn signatures_dir() -> PathBuf {
    glib::user_data_dir().join("nekoland/preview-signatures")
}

impl Prefs {
    pub fn load() -> Prefs {
        std::fs::read(prefs_path())
            .ok()
            .and_then(|text| serde_json::from_slice(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let path = prefs_path();
        let written = std::fs::create_dir_all(path.parent().unwrap_or(Path::new("/")))
            .and_then(|()| std::fs::write(&path, serde_json::to_vec_pretty(self).unwrap_or_default()));
        if let Err(err) = written {
            eprintln!("{}: {err}", path.display());
        }
    }
}
