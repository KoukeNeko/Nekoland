//! 截圖's preferences, in shot.toml: read by 截圖 and by Settings.

use std::{io::Write, path::PathBuf};

use gtk::glib;
use serde::{Deserialize, Serialize};

/// What happens once a picture is taken.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug)]
#[serde(rename_all = "lowercase")]
pub enum After {
    /// 截圖's window opens on it.
    Editor,
    /// A notice shows it; clicking that opens the window.
    Thumbnail,
    /// Nothing is shown.
    Nothing,
}

#[derive(Clone, Default, Serialize, Deserialize, Debug)]
#[serde(default)]
pub struct S3 {
    pub endpoint: String,
    pub bucket: String,
    pub region: String,
    pub access_key: String,
    pub secret_key: String,
    /// Where an uploaded file is read from, when not the endpoint itself.
    pub public_url: String,
}

impl S3 {
    pub fn is_set(&self) -> bool {
        !(self.endpoint.is_empty()
            || self.bucket.is_empty()
            || self.access_key.is_empty()
            || self.secret_key.is_empty())
    }
}

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(default)]
pub struct Prefs {
    pub after: After,
    pub auto_copy: bool,
    pub auto_save: bool,
    /// Where 儲存 puts pictures; empty for Pictures' 截圖 folder.
    pub folder: String,
    pub template: String,
    pub format: String,
    /// Pictures of a display drawn at twice its size are saved at its size in points.
    pub downscale: bool,
    pub delay: u32,
    /// The colour marks start in: an index into the swatches.
    pub color: usize,
    /// 加入截圖 puts the new one under the picture rather than beside it.
    pub combine_below: bool,
    pub s3: S3,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            // As the Mac's: a thumbnail, and the picture saved; the clipboard only with Control.
            after: After::Thumbnail,
            auto_copy: false,
            auto_save: true,
            folder: String::new(),
            template: "截圖 {年}-{月}-{日} {時}.{分}.{秒}".into(),
            format: "png".into(),
            downscale: false,
            delay: 5,
            color: 0,
            combine_below: false,
            s3: S3::default(),
        }
    }
}

fn config_path() -> PathBuf {
    glib::user_config_dir().join("nekoland/shot.toml")
}

impl Prefs {
    pub fn load() -> Prefs {
        std::fs::read_to_string(config_path())
            .ok()
            .and_then(|text| {
                toml::from_str(&text)
                    .map_err(|err| eprintln!("shot.toml: {err}"))
                    .ok()
            })
            .unwrap_or_default()
    }

    /// Kept readable by its owner alone, as it can hold the bucket's secret key.
    pub fn save(&self) {
        let path = config_path();
        let text = match toml::to_string(self) {
            Ok(text) => text,
            Err(err) => return eprintln!("shot.toml: {err}"),
        };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        use std::os::unix::fs::OpenOptionsExt;
        let written = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)
            .and_then(|mut file| file.write_all(text.as_bytes()));
        if let Err(err) = written {
            eprintln!("{}: {err}", path.display());
        }
    }

    pub fn folder(&self) -> PathBuf {
        if self.folder.is_empty() {
            let pictures =
                glib::user_special_dir(glib::UserDirectory::Pictures).unwrap_or_else(glib::home_dir);
            pictures.join("截圖")
        } else {
            PathBuf::from(&self.folder)
        }
    }
}
