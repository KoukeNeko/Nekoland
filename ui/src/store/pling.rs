//! Icon and cursor themes from Pling (gnome-look.org and store.kde.org share it), through
//! its OCS API, which needs no account. A theme is installed by unpacking its archive into
//! the person's icon folder; nothing in the archive is run.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Command,
};

use gtk::{gdk, glib, prelude::Cast};

use crate::{ipc, sources, themes};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Look {
    Icons,
    Cursors,
}

impl Look {
    /// Pling's category: Full Icon Themes, Cursors.
    fn category(self) -> u32 {
        match self {
            Look::Icons => 132,
            Look::Cursors => 107,
        }
    }

    /// The compositor's setting, as Settings sets it.
    pub fn key(self) -> &'static str {
        match self {
            Look::Icons => "icon_theme",
            Look::Cursors => "cursor_theme",
        }
    }

    /// GNOME's, which GTK and others follow.
    fn gsetting(self) -> &'static str {
        match self {
            Look::Icons => "icon-theme",
            Look::Cursors => "cursor-theme",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Look::Icons => "圖示",
            Look::Cursors => "游標",
        }
    }

    fn holds(self, path: &Path) -> bool {
        match self {
            Look::Icons => themes::is_icon_theme(path),
            Look::Cursors => themes::is_cursor_theme(path),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct File {
    pub name: String,
    pub url: String,
    pub size: String,
}

#[derive(Clone, Debug)]
pub struct Theme {
    /// Pling's, or empty for a theme known only from this computer.
    pub id: String,
    pub look: Look,
    pub name: String,
    pub author: String,
    pub description: String,
    pub changed: String,
    pub previews: Vec<String>,
    pub files: Vec<File>,
    /// The folders it is installed as, by name: what can be applied.
    pub folders: Vec<String>,
    /// Where a theme known only from this computer is.
    pub path: Option<PathBuf>,
    pub system: bool,
}

const API: &str = "https://api.opendesktop.org/ocs/v1/content/data";
/// The archives `unpack` opens; Pling also lists links to sites and Windows packs.
const ARCHIVES: [&str; 8] = [
    ".tar.gz", ".tgz", ".tar.xz", ".txz", ".tar.bz2", ".tbz2", ".tar.zst", ".zip",
];

/// A JSON string or number as text, as OCS gives either for the same field.
fn text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

/// Pling's descriptions are HTML; their text, a paragraph a line.
pub fn plain(html: &str) -> String {
    let mut out = String::new();
    let mut rest = html;
    while let Some(at) = rest.find('<') {
        out.push_str(&rest[..at]);
        let Some(end) = rest[at..].find('>') else { break };
        let tag = rest[at + 1..at + end].to_ascii_lowercase();
        if tag.starts_with("br") || tag == "/p" || tag == "/li" || tag.starts_with("/h") {
            out.push('\n');
        }
        rest = &rest[at + end + 1..];
    }
    out.push_str(rest);
    let out = out
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&");
    out.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// "2024-06-18T…" as 2024年6月.
fn month(date: &str) -> String {
    let mut parts = date.split('-');
    match (parts.next(), parts.next().and_then(|m| m.parse::<u32>().ok())) {
        (Some(year), Some(month)) if year.len() == 4 => format!("{year}年{month}月"),
        _ => String::new(),
    }
}

/// The themes in an OCS answer, each with the archives it can be installed from.
pub fn parse(answer: &str) -> Vec<Theme> {
    let Ok(answer) = serde_json::from_str::<serde_json::Value>(answer) else {
        return Vec::new();
    };
    let Some(items) = answer["data"].as_array() else {
        return Vec::new();
    };
    let manifest = manifest();
    items
        .iter()
        .filter(|item| text(&item["ghns_excluded"]) != "1")
        .filter_map(|item| {
            let look = match text(&item["typeid"]).as_str() {
                "132" => Look::Icons,
                "107" => Look::Cursors,
                _ => return None,
            };
            let files: Vec<File> = (1..=7)
                .filter_map(|n| {
                    let name = text(&item[format!("downloadname{n}")]);
                    let url = text(&item[format!("downloadlink{n}")]);
                    let lower = name.to_lowercase();
                    let free = matches!(text(&item[format!("downloadprice{n}")]).as_str(), "" | "0");
                    (free
                        && !url.is_empty()
                        && ARCHIVES.iter().any(|ext| lower.ends_with(ext))
                        && !lower.contains("windows"))
                    .then(|| File {
                        size: text(&item[format!("downloadsize{n}")])
                            .parse::<u64>()
                            .map(|kb| sources::human_size(kb * 1024))
                            .unwrap_or_default(),
                        name,
                        url,
                    })
                })
                .collect();
            if files.is_empty() {
                return None;
            }
            let id = text(&item["id"]);
            let folders = installed_folders(&manifest, &id, look);
            Some(Theme {
                look,
                name: text(&item["name"]),
                author: text(&item["personid"]),
                description: plain(&text(&item["description"])),
                changed: month(&text(&item["changed"])),
                previews: (1..=3)
                    .map(|n| text(&item[format!("previewpic{n}")]))
                    .filter(|url| !url.is_empty())
                    .collect(),
                files,
                folders,
                path: None,
                system: false,
                id,
            })
        })
        .collect()
}

fn query(params: &str) -> Option<Vec<Theme>> {
    let url = format!("{API}?{params}&sortmode=down&pagesize=30&format=json");
    let answer = sources::fetch(&url)?;
    Some(parse(&String::from_utf8_lossy(&answer)))
}

/// The most downloaded; None when Pling cannot be reached.
pub fn popular(look: Look) -> Option<Vec<Theme>> {
    query(&format!("categories={}", look.category()))
}

pub fn search(words: &str) -> Vec<Theme> {
    let words = glib::Uri::escape_string(words, None, false);
    query(&format!(
        "categories={}x{}&search={words}",
        Look::Icons.category(),
        Look::Cursors.category()
    ))
    .unwrap_or_default()
}

/// The themes on this computer, each as its folder.
pub fn local(look: Look) -> Vec<Theme> {
    themes::installed(look == Look::Cursors)
        .into_iter()
        .map(|found| Theme {
            id: String::new(),
            look,
            system: found.system(),
            folders: vec![found.name.clone()],
            name: found.name,
            author: String::new(),
            description: String::new(),
            changed: String::new(),
            previews: Vec::new(),
            files: Vec::new(),
            path: Some(found.path),
        })
        .collect()
}

/// The theme the compositor is set to, empty for its default.
pub fn current(look: Look) -> String {
    setting(look.key())
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default()
}

/// The applications kept in the Dock, as the compositor has them.
pub fn dock() -> Vec<String> {
    setting("dock")
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|v| v.as_str().map(String::from))
        .collect()
}

fn setting(key: &str) -> Option<toml::Value> {
    let config: toml::Table = toml::from_str(&ipc::msg("config")?)
        .inspect_err(|err| eprintln!("config: {err}"))
        .ok()?;
    config.get(key).cloned()
}

/// Sets the theme for the compositor and for GNOME's settings together, as Settings does.
pub fn apply(look: Look, folder: &str) -> Result<(), String> {
    let reply = ipc::msg(&format!("set {} = {folder:?}", look.key())).ok_or("無法連線到 Nekoland")?;
    if !reply.trim().is_empty() {
        return Err(reply.trim().to_string());
    }
    let status = Command::new("gsettings")
        .args([
            "set",
            "org.gnome.desktop.interface",
            look.gsetting(),
            &format!("'{folder}'"),
        ])
        .status();
    if !status.is_ok_and(|s| s.success()) {
        eprintln!("gsettings set {} failed", look.gsetting());
    }
    Ok(())
}

// What Market installed, by Pling's id, so that it can be shown as installed and removed whole.

type Manifest = BTreeMap<String, Vec<String>>;

fn manifest_path() -> PathBuf {
    glib::user_data_dir().join("nekoland/market.json")
}

fn manifest() -> Manifest {
    std::fs::read_to_string(manifest_path())
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn save_manifest(manifest: &Manifest) -> Result<(), String> {
    let path = manifest_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|err| err.to_string())?;
    }
    let text = serde_json::to_string_pretty(manifest).map_err(|err| err.to_string())?;
    std::fs::write(path, text).map_err(|err| err.to_string())
}

/// The folders of `id` still there.
fn installed_folders(manifest: &Manifest, id: &str, look: Look) -> Vec<String> {
    let dir = themes::user_dir();
    manifest
        .get(id)
        .into_iter()
        .flatten()
        .filter(|name| look.holds(&dir.join(name)))
        .cloned()
        .collect()
}

/// Downloads and unpacks `file` into the cache, and gives the themes found in it.
pub fn fetch(theme: &Theme, file: &File) -> Result<Vec<PathBuf>, String> {
    let cache = glib::user_cache_dir().join("nekoland/market").join(&theme.id);
    let unpacked = cache.join(file.name.replace('/', "_") + ".d");
    let mut found = themes_in(&unpacked, theme.look);
    if found.is_empty() {
        let archive = cache.join(file.name.replace('/', "_"));
        std::fs::create_dir_all(&cache).map_err(|err| err.to_string())?;
        let ok = Command::new("curl")
            .args(["-fsSL", "--max-time", "600", "--max-filesize", "500000000", "-o"])
            .arg(&archive)
            .arg(&file.url)
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            return Err(format!("無法下載 {}", file.name));
        }
        let _ = std::fs::remove_dir_all(&unpacked);
        std::fs::create_dir_all(&unpacked).map_err(|err| err.to_string())?;
        let unpacked_ok = unpack(&archive, &unpacked);
        let _ = std::fs::remove_file(&archive);
        if !unpacked_ok {
            return Err(format!("無法解開 {}", file.name));
        }
        found = themes_in(&unpacked, theme.look);
    }
    if found.is_empty() {
        return Err(format!("{} 裡沒有{}主題", file.name, theme.look.name()));
    }
    Ok(found)
}

/// Unpacks with the system's tools, which keep every member inside `into`: tar refuses
/// names with `..` and drops a leading `/`, and Python's zipfile does the same.
fn unpack(archive: &Path, into: &Path) -> bool {
    let name = archive.to_string_lossy().to_lowercase();
    let mut command = if name.ends_with(".zip") {
        let mut python = Command::new("python3");
        python.args(["-I", "-m", "zipfile", "-e"]).arg(archive).arg(into);
        python
    } else {
        let mut tar = Command::new("tar");
        tar.args(["--no-same-owner", "--no-same-permissions", "-xf"])
            .arg(archive)
            .arg("-C")
            .arg(into);
        tar
    };
    command.status().is_ok_and(|s| s.success())
}

/// The theme folders under `root`, as deep as archives put them.
fn themes_in(root: &Path, look: Look) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut at = vec![(root.to_path_buf(), 0)];
    while let Some((dir, depth)) = at.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            // Symbolic links stay as the theme's own; a folder is never found through one.
            if !entry.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            if look.holds(&path) {
                found.push(path);
            } else if depth < 4 {
                at.push((path, depth + 1));
            }
        }
    }
    found.sort();
    found
}

/// Installs `file` of `theme`: its folders go to the person's icon folder, replacing what
/// an earlier install of the same theme put there, never another theme's.
pub fn install(theme: &Theme, file: &File) -> Result<Vec<String>, String> {
    let found = fetch(theme, file)?;
    let mut manifest = manifest();
    let ours = manifest.get(&theme.id).cloned().unwrap_or_default();
    let dir = themes::user_dir();
    std::fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
    let mut folders = Vec::new();
    for source in &found {
        let Some(name) = source.file_name().and_then(|n| n.to_str()).map(String::from) else {
            continue;
        };
        let target = dir.join(&name);
        if target.exists() {
            if !ours.contains(&name) {
                return Err(format!("已經有名為 {name} 的主題"));
            }
            std::fs::remove_dir_all(&target).map_err(|err| err.to_string())?;
        }
        // cp keeps the theme's own symbolic links, which icon themes are full of.
        let ok = Command::new("cp")
            .arg("-R")
            .arg("--no-dereference")
            .arg(source)
            .arg(&target)
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            return Err(format!("無法安裝 {name}"));
        }
        folders.push(name);
    }
    let mut kept = ours;
    kept.retain(|name| !folders.contains(name));
    kept.extend(folders.iter().cloned());
    manifest.insert(theme.id.clone(), kept);
    save_manifest(&manifest)?;
    let mut all = installed_folders(&manifest, &theme.id, theme.look);
    // The folders of this file first, the one the page applies.
    all.sort_by_key(|name| !folders.contains(name));
    Ok(all)
}

/// Removes a theme's folders from the person's icon folders.
pub fn remove(theme: &Theme) -> Result<(), String> {
    let dirs = [themes::user_dir(), glib::home_dir().join(".icons")];
    for name in &theme.folders {
        let Some(path) = dirs.iter().map(|dir| dir.join(name)).find(|path| path.is_dir()) else {
            continue;
        };
        std::fs::remove_dir_all(&path).map_err(|err| format!("無法移除 {name}：{err}"))?;
    }
    let mut manifest = manifest();
    if manifest.remove(&theme.id).is_some() {
        save_manifest(&manifest)?;
    }
    Ok(())
}

// Cursor pictures, for trying a theme on before applying it.

/// The cursors tried on, by their names in the theme, each with its older X11 names.
pub const SHAPES: [(&str, &[&str]); 6] = [
    ("一般", &["default", "left_ptr", "arrow"]),
    ("連結", &["pointer", "hand2", "hand1", "pointing_hand"]),
    ("文字", &["text", "xterm", "ibeam"]),
    ("等待", &["wait", "watch", "progress", "left_ptr_watch"]),
    ("移動", &["move", "fleur", "all-scroll", "size_all"]),
    (
        "調整大小",
        &["ew-resize", "sb_h_double_arrow", "size_hor", "col-resize"],
    ),
];

/// One frame of a cursor, as Xcursor keeps it: premultiplied RGBA.
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub xhot: u32,
    pub yhot: u32,
    pub pixels: Vec<u8>,
}

/// Each of `SHAPES`, when the theme has it.
pub type Shapes = Vec<Option<Frame>>;

impl Frame {
    pub fn texture(&self) -> gdk::Texture {
        gdk::MemoryTexture::new(
            self.width as i32,
            self.height as i32,
            gdk::MemoryFormat::R8g8b8a8Premultiplied,
            &glib::Bytes::from(&self.pixels),
            self.width as usize * 4,
        )
        .upcast()
    }
}

/// Each shape's frame nearest `size` pixels in the theme at `path`.
// shortcut: a shape the theme leaves to the theme it inherits is missing; follow Inherits= if themes need it.
pub fn shapes(path: &Path, size: u32) -> Shapes {
    SHAPES
        .iter()
        .map(|(_, names)| {
            let bytes = names
                .iter()
                .find_map(|name| std::fs::read(path.join("cursors").join(name)).ok())?;
            let images = xcursor::parser::parse_xcursor(&bytes)?;
            let image = images
                .into_iter()
                .min_by_key(|image| (i64::from(image.size) - i64::from(size)).abs())?;
            Some(Frame {
                width: image.width,
                height: image.height,
                xhot: image.xhot,
                yhot: image.yhot,
                pixels: image.pixels_rgba,
            })
        })
        .collect()
}

/// Where an installed folder of a theme is.
pub fn folder_path(name: &str) -> Option<PathBuf> {
    themes::dirs()
        .into_iter()
        .map(|dir| dir.join(name))
        .find(|path| path.is_dir())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptions_lose_their_html() {
        assert_eq!(
            plain("<p>Light &amp; dark<br />cursors.</p><p><a href=\"x\">Site</a></p>"),
            "Light & dark\ncursors.\nSite"
        );
    }

    #[test]
    fn items_keep_only_archives_they_can_install() {
        let answer = r#"{"data": [
            {"id": 1, "typeid": "107", "name": "Ice", "personid": "kaiz", "changed": "2024-06-18T00:00:00",
             "downloadname1": "Ice-Windows.zip", "downloadlink1": "https://x/1", "downloadsize1": "10",
             "downloadname2": "Ice.tar.xz", "downloadlink2": "https://x/2", "downloadsize2": 2048, "downloadprice2": "0",
             "downloadname3": "Ice", "downloadlink3": "https://github.com/x"},
            {"id": 2, "typeid": "107", "name": "Link only", "downloadname1": "site", "downloadlink1": "https://x"},
            {"id": 3, "typeid": "132", "name": "Hidden", "ghns_excluded": "1",
             "downloadname1": "a.tar.gz", "downloadlink1": "https://x/3"}
        ]}"#;
        let found = parse(answer);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].look, Look::Cursors);
        assert_eq!(found[0].changed, "2024年6月");
        assert_eq!(
            found[0].files,
            vec![File {
                name: "Ice.tar.xz".into(),
                url: "https://x/2".into(),
                size: "2 MB".into()
            }]
        );
    }
}
