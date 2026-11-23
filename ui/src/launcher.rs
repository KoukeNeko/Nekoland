//! `nekoland-launcher [--search=TEXT] [--open=VIEW]`: finds and does things, after
//! Raycast. It starts applications, by their names in any of the languages their desktop
//! entries give; works out arithmetic and units; runs the system's and the active window's
//! commands; opens files, quicklinks and web searches; and keeps the clipboard's history,
//! snippets and notes, with views of their own that `--open` can start in: clipboard, emoji,
//! files, snippets, quicklinks, notes or windows.
//!
//! `nekoland-launcher --remember-clipboard`, run by `wl-paste --watch`, adds what was copied
//! to the clipboard's history.

// The crate's other programs have modules of these names beside this file.
#[path = "launcher/apps.rs"]
mod apps;
#[path = "launcher/calc.rs"]
mod calc;
#[path = "launcher/commands.rs"]
mod commands;
#[path = "launcher/emoji.rs"]
mod emoji;
#[path = "launcher/files.rs"]
mod files;
#[path = "launcher/store.rs"]
mod store;
mod ipc;

use std::{
    cell::RefCell,
    collections::HashMap,
    io::Write,
    path::{Path, PathBuf},
    rc::Rc,
};

use gtk::{gio, glib, prelude::*};
use slint::{Image, ModelRc, VecModel};

use commands::{Command, Does, View, COMMANDS};
use ipc::msg;

slint::include_modules!();

/// How many matches the root lists before files and other searches.
const SHOWN: usize = 8;
const FILES_AT_ROOT: usize = 4;
/// How many a view lists; typing narrows them.
const VIEW_MOST: usize = 300;
const EMOJI_MOST: usize = 640;

// Where items go, in logical pixels: the list's inset, and each kind's height.
const WIDTH: f32 = 640.0;
const LIST_WITH_DETAIL: f32 = 340.0;
const INSET: f32 = 8.0;
const ROW: f32 = 36.0;
const ANSWER: f32 = 60.0;
const HEADER: f32 = 32.0;
const COLUMNS: usize = 8;
const CELL: f32 = 72.0;

/// What an item stands for, and so what its actions do.
#[derive(Clone)]
enum Target {
    Header,
    App(usize),
    Command(&'static Command),
    Answer(String),
    File(PathBuf),
    Copied(usize),
    Emoji(String),
    Snippet(usize),
    /// A quicklink, and what was typed for its `{query}`.
    Quicklink(usize, Option<String>),
    Note(usize),
    /// A window `windows` listed, by its place there and its application.
    Window(usize, String),
    Url(String),
    SearchMenus(String),
    SearchFiles(String),
    Create(Does),
}

#[derive(Clone)]
enum Action {
    Open,
    Copy(String),
    Paste(String),
    /// What the clipboard's history holds at this place, as it was copied.
    CopyCopied(usize),
    PasteCopied(usize),
    ShowInFiles(PathBuf),
    Trash(PathBuf),
    Edit,
    /// Asks first, then deletes.
    Delete,
    Forget(usize),
    /// Asks first, then forgets everything copied.
    ClearClipboard,
    Alias(String),
    Create(Does),
}

struct Found {
    kind: &'static str,
    title: String,
    subtitle: String,
    accessory: String,
    icon: Option<String>,
    glyph: String,
    part: Option<[f32; 4]>,
    target: Target,
}

impl Found {
    fn header(title: &str) -> Self {
        Self::new(title, Target::Header).kind("header")
    }

    fn new(title: impl Into<String>, target: Target) -> Self {
        Self {
            kind: "row",
            title: title.into(),
            subtitle: String::new(),
            accessory: String::new(),
            icon: None,
            glyph: String::new(),
            part: None,
            target,
        }
    }

    fn kind(mut self, kind: &'static str) -> Self {
        self.kind = kind;
        self
    }

    fn subtitle(mut self, subtitle: impl Into<String>) -> Self {
        self.subtitle = subtitle.into();
        self
    }

    fn accessory(mut self, accessory: impl Into<String>) -> Self {
        self.accessory = accessory.into();
        self
    }

    fn icon(mut self, icon: Option<String>) -> Self {
        self.icon = icon;
        self
    }

    fn glyph(mut self, glyph: impl Into<String>) -> Self {
        self.glyph = glyph.into();
        self
    }
}

/// A form the launcher shows instead of its list.
#[derive(Clone)]
enum Form {
    Snippet(Option<usize>),
    Quicklink(Option<usize>),
    Note(Option<usize>),
    /// The alias of what has this key.
    Alias(String),
}

struct State {
    ui: slint::Weak<Launcher>,
    apps: Vec<apps::App>,
    themes: Vec<String>,
    icons: HashMap<String, Image>,
    no_icon: Image,
    languages: Vec<String>,
    store: store::Store,
    clipboard: Vec<store::Copied>,
    emoji: Option<Vec<emoji::Emoji>>,
    files: files::Index,
    files_ready: bool,
    windows: Vec<(String, String)>,
    view: View,
    /// What the root was searching when a view opened, for when it closes.
    root_query: String,
    found: Vec<Found>,
    form: Option<Form>,
    /// The deletion the action panel asks about.
    confirming: bool,
}

fn spawn(program: &str, arguments: &[&str]) {
    if let Err(err) = std::process::Command::new(program).args(arguments).spawn() {
        eprintln!("{program}: {err}");
    }
}

/// Puts text on the clipboard, waiting until it is there.
fn copy(text: &str) {
    copy_as(text.as_bytes(), None);
}

/// Puts what was copied back on the clipboard as it was copied: files, a picture or text.
fn copy_again(copied: &store::Copied) {
    if !copied.files.is_empty() {
        let uris: Vec<String> = copied.files.iter().map(|path| gio::File::for_path(path).uri().into()).collect();
        copy_as(uris.join("\r\n").as_bytes(), Some("text/uri-list"));
    } else if let (Some(picture), Some(kind)) = (&copied.picture, copied.picture_type()) {
        match std::fs::read(picture) {
            Ok(bytes) => copy_as(&bytes, Some(&kind)),
            Err(err) => eprintln!("{}: {err}", picture.display()),
        }
    } else {
        copy(&copied.text);
    }
}

/// Puts `bytes` on the clipboard as `kind`, or as text, waiting until they are there.
fn copy_as(bytes: &[u8], kind: Option<&str>) {
    let mut command = std::process::Command::new("wl-copy");
    if let Some(kind) = kind {
        command.args(["--type", kind]);
    }
    let child = command.stdin(std::process::Stdio::piped()).spawn();
    match child {
        Ok(mut child) => {
            if let Some(mut stdin) = child.stdin.take() {
                if let Err(err) = stdin.write_all(bytes) {
                    eprintln!("wl-copy: {err}");
                }
            }
            let _ = child.wait();
        }
        Err(err) => eprintln!("wl-copy: {err}"),
    }
}

/// Puts text on the clipboard and has the compositor paste it into the window under the
/// launcher once the launcher has closed.
fn paste(text: &str) {
    copy(text);
    msg("after-launcher paste");
}

/// How well what was typed finds something: 0 is best; None, not at all.
fn score(query: &str, name: &str, words: &str, alias: Option<&str>) -> Option<u8> {
    let name = name.to_lowercase();
    if let Some(alias) = alias.map(str::to_lowercase) {
        if alias == query {
            return Some(0);
        }
        if alias.starts_with(query) {
            return Some(1);
        }
    }
    if name.starts_with(query) {
        Some(2)
    } else if name.split_whitespace().any(|word| word.starts_with(query)) {
        Some(3)
    } else if name.contains(query) || words.to_lowercase().contains(query) {
        Some(4)
    } else {
        None
    }
}

/// How long ago, in words.
fn ago(at: i64) -> String {
    let seconds = (store::now() - at).max(0);
    match seconds {
        0..=59 => "剛剛".into(),
        60..=3599 => format!("{} 分鐘前", seconds / 60),
        3600..=86_399 => format!("{} 小時前", seconds / 3600),
        _ => format!("{} 天前", seconds / 86_400),
    }
}

/// What was copied is a link, a file's path or text, by its name and theme icons.
fn copied_kind(copied: &store::Copied) -> (&'static str, String) {
    if copied.picture.is_some() {
        return ("圖片", "image-x-generic".into());
    }
    if let Some(path) = copied.files.first() {
        return ("檔案", file_icon(path, path.is_dir()).join("\n"));
    }
    let text = copied.text.trim();
    if text.lines().count() == 1 {
        if text.starts_with("https://") || text.starts_with("http://") {
            return ("連結", "web-browser".into());
        }
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
        let path = text.strip_prefix("~/").map_or_else(|| PathBuf::from(text), |rest| home.join(rest));
        if path.is_absolute() && path.exists() {
            return ("檔案", file_icon(&path, path.is_dir()).join("\n"));
        }
    }
    ("文字", "text-x-generic".into())
}

/// Seconds since the epoch as a date and time.
fn date_time(at: i64) -> String {
    glib::DateTime::from_unix_local(at)
        .and_then(|time| time.format("%Y/%m/%d %H:%M"))
        .map(|text| text.to_string())
        .unwrap_or_default()
}

fn first_line(text: &str) -> String {
    text.trim().lines().next().unwrap_or_default().trim().to_string()
}

/// A path with the home folder as ~.
fn tilde(path: &Path) -> String {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    match path.strip_prefix(&home) {
        Ok(rest) if !home.as_os_str().is_empty() => format!("~/{}", rest.display()),
        _ => path.display().to_string(),
    }
}

/// The theme icons for a file, by its kind as its name tells it.
fn file_icon(path: &Path, folder: bool) -> Vec<String> {
    if folder {
        return vec!["folder".into()];
    }
    let (kind, _) = gio::content_type_guess(Some(path), &[]);
    gio::content_type_get_icon(&kind)
        .downcast::<gio::ThemedIcon>()
        .map(|icon| icon.names().iter().map(ToString::to_string).collect())
        .unwrap_or_default()
}

fn expand_placeholders(text: &str, clipboard: Option<&str>) -> String {
    let now = glib::DateTime::now_local().ok();
    let format = |pattern: &str| {
        now.as_ref().and_then(|now| now.format(pattern).ok()).map(|text| text.to_string()).unwrap_or_default()
    };
    text.replace("{date}", &format("%Y/%m/%d"))
        .replace("{time}", &format("%H:%M"))
        .replace("{clipboard}", clipboard.unwrap_or_default())
}

fn show_in_files(path: &Path) {
    let uri = gio::File::for_path(path).uri().to_string();
    let shown = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE).and_then(|bus| {
        bus.call_sync(
            Some("org.freedesktop.FileManager1"),
            "/org/freedesktop/FileManager1",
            "org.freedesktop.FileManager1",
            "ShowItems",
            Some(&(vec![uri], "").to_variant()),
            None,
            gio::DBusCallFlags::NONE,
            5000,
            gio::Cancellable::NONE,
        )
    });
    if let Err(err) = shown {
        eprintln!("show {}: {err}", path.display());
    }
}

fn open(target: &str) {
    spawn("gio", &["open", target]);
}

impl State {
    fn ui(&self) -> Launcher {
        self.ui.upgrade().expect("the launcher's window outlives its state")
    }

    fn image(&mut self, names: &[String]) -> Image {
        for name in names {
            if let Some(image) = self.icons.get(name) {
                return image.clone();
            }
            let themes: Vec<&str> = self.themes.iter().map(String::as_str).collect();
            let found = apps::find_icon(&apps::data_dirs(), &themes, name)
                .and_then(|path| Image::load_from_path(&path).ok());
            if let Some(image) = found {
                self.icons.insert(name.clone(), image.clone());
                return image;
            }
        }
        self.no_icon.clone()
    }

    /// An app's icon, looked for only once it is shown, so the window opens before every
    /// app's is found.
    fn app_image(&mut self, index: usize) -> Image {
        let Some(name) = self.apps[index].icon.clone() else {
            return self.no_icon.clone();
        };
        // Kept apart from `image`'s, as an app's icon not found is kept too.
        let key = format!("app:{name}");
        if let Some(image) = self.icons.get(&key) {
            return image.clone();
        }
        let themes: Vec<&str> = self.themes.iter().map(String::as_str).collect();
        let image = apps::find_icon(&apps::data_dirs(), &themes, &name)
            .and_then(|path| Image::load_from_path(&path).ok())
            .unwrap_or_else(|| self.no_icon.clone());
        self.icons.insert(key, image.clone());
        image
    }

    fn uses(&self, key: &str) -> u32 {
        self.store.uses.get(key).copied().unwrap_or_default()
    }

    fn alias(&self, key: &str) -> Option<&str> {
        self.store.aliases.get(key).map(String::as_str)
    }

    fn command_found(command: &'static Command) -> Found {
        let mut found = Found::new(command.title, Target::Command(command)).accessory("指令");
        if command.icon.is_empty() {
            found.part = command.part;
        } else {
            found.icon = Some(command.icon.into());
        }
        found
    }

    /// What the root finds for what was typed.
    fn root(&self, query: &str) -> Vec<Found> {
        let typed = query.trim().to_lowercase();
        if typed.is_empty() {
            return Vec::new();
        }
        let mut found = Vec::new();
        if let Some((result, read)) = calc::answer(query) {
            found.push(
                Found::new(result.clone(), Target::Answer(result))
                    .subtitle(read)
                    .glyph("=")
                    .accessory("計算機"),
            );
        }

        let links = self.store.quicklinks();
        // `g cats`: a quicklink's keyword, and what to look up.
        if let Some((keyword, rest)) = query.trim_start().split_once(' ') {
            if let Some((index, link)) = links
                .iter()
                .enumerate()
                .find(|(_, link)| !link.keyword.is_empty() && link.keyword.eq_ignore_ascii_case(keyword))
            {
                if !rest.trim().is_empty() {
                    found.push(
                        Found::new(
                            format!("{}：{}", link.name, rest.trim()),
                            Target::Quicklink(index, Some(rest.trim().into())),
                        )
                        .icon(Some("web-browser".into()))
                        .accessory("快速連結"),
                    );
                }
            }
        }

        let mut ranked: Vec<(u8, u32, Found)> = Vec::new();
        for (index, app) in self.apps.iter().enumerate() {
            let key = format!("app:{}", app.id);
            if let Some(score) = score(&typed, &app.name, &app.words, self.alias(&key)) {
                let found =
                    Found::new(&app.name, Target::App(index)).subtitle(&app.detail).accessory("應用程式");
                ranked.push((score, self.uses(&key), found));
            }
        }
        for command in COMMANDS {
            let key = format!("cmd:{}", command.id);
            if let Some(score) = score(&typed, command.title, command.words, self.alias(&key)) {
                ranked.push((score, self.uses(&key), Self::command_found(command)));
            }
        }
        for (index, link) in links.iter().enumerate() {
            let key = format!("link:{}", link.name);
            let alias =
                self.alias(&key).or(Some(link.keyword.as_str()).filter(|keyword| !keyword.is_empty()));
            if let Some(score) = score(&typed, &link.name, &link.url, alias) {
                let found = Found::new(&link.name, Target::Quicklink(index, None))
                    .icon(Some("web-browser".into()))
                    .accessory("快速連結");
                ranked.push((score, self.uses(&key), found));
            }
        }
        ranked.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)).then_with(|| a.2.title.cmp(&b.2.title)));
        found.extend(ranked.into_iter().take(SHOWN).map(|(_, _, found)| found));

        if typed.chars().count() >= 2 {
            let files = files::matching(&self.files, &typed, FILES_AT_ROOT);
            if !files.is_empty() {
                found.push(Found::header("檔案"));
                for (path, folder) in files {
                    found.push(self.file_found(path, folder));
                }
            }
        }

        let query = query.trim();
        found.push(Found::header("其他搜尋"));
        found.push(
            Found::new(
                format!("用 Google 搜尋「{query}」"),
                Target::Url(format!(
                    "https://www.google.com/search?q={}",
                    glib::Uri::escape_string(query, None, false)
                )),
            )
            .icon(Some("web-browser".into())),
        );
        found.push(
            Found::new(format!("搜尋檔案「{query}」"), Target::SearchFiles(query.into()))
                .icon(Some("folder".into())),
        );
        found.push(
            Found::new(format!("搜尋選單項目「{query}」"), Target::SearchMenus(query.into()))
                .icon(Some("help-browser".into())),
        );

        if let Some(first) = found.first_mut().filter(|first| !matches!(first.target, Target::Header)) {
            first.kind = "answer";
        }
        found
    }

    fn file_found(&self, path: PathBuf, folder: bool) -> Found {
        let name = path.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default();
        let place = path.parent().map(tilde).unwrap_or_default();
        let mut found = Found::new(name, Target::File(path.clone())).subtitle(place);
        found.icon = Some(file_icon(&path, folder).join("\n"));
        found
    }

    /// What a view lists for what was typed.
    fn view_items(&mut self, query: &str) -> Vec<Found> {
        let typed = query.trim().to_lowercase();
        let matches = |text: &str| typed.is_empty() || text.to_lowercase().contains(&typed);
        match self.view {
            View::Root => self.root(query),
            View::Clipboard => self
                .clipboard
                .iter()
                .enumerate()
                .filter(|(_, copied)| matches(&copied.text))
                .take(VIEW_MOST)
                .map(|(index, copied)| {
                    Found::new(first_line(&copied.text), Target::Copied(index))
                        .icon(Some(copied_kind(copied).1))
                })
                .collect(),
            View::Emoji => {
                let languages = self.languages.clone();
                let emoji = self.emoji.get_or_insert_with(|| emoji::load(&languages));
                emoji
                    .iter()
                    .filter(|emoji| {
                        typed.is_empty()
                            || emoji.words.iter().any(|word| word.to_lowercase().contains(&typed))
                    })
                    .take(EMOJI_MOST)
                    .map(|emoji| {
                        let name = emoji.words.first().cloned().unwrap_or_default();
                        Found::new(name, Target::Emoji(emoji.glyph.clone())).glyph(&emoji.glyph).kind("cell")
                    })
                    .collect()
            }
            View::Files => {
                if typed.is_empty() {
                    return Vec::new();
                }
                files::matching(&self.files, &typed, VIEW_MOST)
                    .into_iter()
                    .map(|(path, folder)| self.file_found(path, folder))
                    .collect()
            }
            View::Snippets => {
                let mut found: Vec<Found> = self
                    .store
                    .snippets
                    .iter()
                    .enumerate()
                    .filter(|(_, snippet)| {
                        matches(&snippet.name) || matches(&snippet.text) || matches(&snippet.keyword)
                    })
                    .map(|(index, snippet)| {
                        Found::new(&snippet.name, Target::Snippet(index))
                            .subtitle(first_line(&snippet.text))
                            .accessory(&snippet.keyword)
                            .icon(Some("text-editor".into()))
                    })
                    .collect();
                if found.is_empty() && typed.is_empty() {
                    found.push(
                        Found::new("建立文字片段", Target::Create(Does::NewSnippet))
                            .icon(Some("text-editor".into())),
                    );
                }
                found
            }
            View::Quicklinks => self
                .store
                .quicklinks()
                .iter()
                .enumerate()
                .filter(|(_, link)| matches(&link.name) || matches(&link.url) || matches(&link.keyword))
                .map(|(index, link)| {
                    Found::new(&link.name, Target::Quicklink(index, None))
                        .subtitle(&link.url)
                        .accessory(&link.keyword)
                        .icon(Some("web-browser".into()))
                })
                .collect(),
            View::Notes => {
                let mut found: Vec<Found> = self
                    .store
                    .notes
                    .iter()
                    .enumerate()
                    .filter(|(_, note)| matches(&note.text))
                    .map(|(index, note)| {
                        Found::new(first_line(&note.text), Target::Note(index))
                            .accessory(ago(note.changed))
                            .icon(Some("x-office-document".into()))
                    })
                    .collect();
                if found.is_empty() && typed.is_empty() {
                    found.push(
                        Found::new("新增筆記", Target::Create(Does::NewNote))
                            .icon(Some("x-office-document".into())),
                    );
                }
                found
            }
            View::Windows => self
                .windows
                .iter()
                .enumerate()
                .filter(|(_, (app_id, title))| matches(title) || matches(app_id))
                .map(|(index, (app_id, title))| {
                    let app = self.app_for(app_id);
                    let name =
                        app.map(|index| self.apps[index].name.clone()).unwrap_or_else(|| app_id.clone());
                    let mut found = Found::new(
                        if title.is_empty() { name.clone() } else { title.clone() },
                        Target::Window(index, app_id.clone()),
                    )
                    .subtitle(name);
                    // An application's own icon, drawn in `items`.
                    found.icon = app.map(|index| format!("app:{index}"));
                    found
                })
                .collect(),
        }
    }

    /// The application a window belongs to, by its application ID.
    fn app_for(&self, app_id: &str) -> Option<usize> {
        let wanted = app_id.to_lowercase();
        self.apps.iter().position(|app| {
            let id = app.id.trim_end_matches(".desktop").to_lowercase();
            id == wanted
                || id.ends_with(&format!(".{wanted}"))
                || app.exec.split_whitespace().next().is_some_and(|exec| exec.ends_with(&wanted))
        })
    }

    /// Lays the found items out, and gives them to the window.
    fn show(&mut self) {
        let ui = self.ui();
        if let Some(form) = self.form.clone() {
            return self.show_form(&ui, &form);
        }
        if ui.get_form() {
            ui.set_form(false);
            ui.invoke_focus_field();
        }
        let query = ui.get_query().to_string();
        self.found = self.view_items(&query);
        let detail = self.lists_beside_detail();
        let width = if detail { LIST_WITH_DETAIL } else { WIDTH } - 2.0 * INSET;
        let cell = width / COLUMNS as f32;

        let mut items = Vec::with_capacity(self.found.len());
        let (mut y, mut column) = (INSET, 0);
        for index in 0..self.found.len() {
            let (kind, height) = match self.found[index].kind {
                "header" => ("header", HEADER),
                "answer" => ("answer", ANSWER),
                "cell" => ("cell", CELL),
                _ => ("row", ROW),
            };
            let x = if kind == "cell" { INSET + column as f32 * cell } else { INSET };
            let icon = match self.found[index].icon.clone() {
                Some(app) if app.starts_with("app:") => {
                    app[4..].parse().map_or(self.no_icon.clone(), |app| self.app_image(app))
                }
                Some(names) => self.image(&names.lines().map(String::from).collect::<Vec<_>>()),
                None => match self.found[index].target {
                    Target::App(app) => self.app_image(app),
                    _ => Image::default(),
                },
            };
            let found = &self.found[index];
            let part = found.part.unwrap_or_default();
            items.push(Item {
                kind: kind.into(),
                title: found.title.clone().into(),
                subtitle: found.subtitle.clone().into(),
                accessory: found.accessory.clone().into(),
                icon,
                glyph: found.glyph.clone().into(),
                framed: found.part.is_some(),
                part_x: part[0],
                part_y: part[1],
                part_w: part[2] - part[0],
                part_h: part[3] - part[1],
                x,
                y,
                width: if kind == "cell" { cell } else { width },
                height,
            });
            if kind == "cell" {
                column += 1;
                if column == COLUMNS || index + 1 == self.found.len() {
                    column = 0;
                    y += CELL;
                }
            } else {
                y += height + if kind == "answer" { 4.0 } else { 0.0 };
            }
        }
        ui.set_items(ModelRc::from(Rc::new(VecModel::from(items))));
        ui.set_content_height(y + INSET);
        ui.set_fixed(self.view != View::Root);
        let current =
            self.found.iter().position(|found| !matches!(found.target, Target::Header)).unwrap_or(0);
        ui.set_current(current as i32);
        self.chose(current);
    }

    fn show_form(&mut self, ui: &Launcher, form: &Form) {
        ui.set_form(true);
        ui.set_form_error("".into());
        ui.set_actions(ModelRc::default());
        let (title, name, text, key, multiline) = match form {
            Form::Snippet(index) => {
                let snippet =
                    index.and_then(|index| self.store.snippets.get(index)).cloned().unwrap_or_default();
                ui.set_name_field(snippet.name.into());
                ui.set_text_field(snippet.text.into());
                ui.set_key_field(snippet.keyword.into());
                (
                    if index.is_some() { "編輯文字片段" } else { "建立文字片段" },
                    "名稱",
                    "內容",
                    "關鍵字",
                    true,
                )
            }
            Form::Quicklink(index) => {
                let link =
                    index.and_then(|index| self.store.quicklinks().get(index).cloned()).unwrap_or_default();
                ui.set_name_field(link.name.into());
                ui.set_text_field(link.url.into());
                ui.set_key_field(link.keyword.into());
                (
                    if index.is_some() { "編輯快速連結" } else { "建立快速連結" },
                    "名稱",
                    "網址",
                    "關鍵字",
                    false,
                )
            }
            Form::Note(index) => {
                let note = index.and_then(|index| self.store.notes.get(index)).cloned().unwrap_or_default();
                ui.set_name_field("".into());
                ui.set_text_field(note.text.into());
                ui.set_key_field("".into());
                (if index.is_some() { "編輯筆記" } else { "新增筆記" }, "", "內容", "", true)
            }
            Form::Alias(key) => {
                ui.set_name_field(self.alias(key).unwrap_or_default().into());
                ui.set_text_field("".into());
                ui.set_key_field("".into());
                ("設定別名", "別名", "", "", false)
            }
        };
        ui.set_form_title(title.into());
        ui.set_name_label(name.into());
        ui.set_text_label(text.into());
        ui.set_key_label(key.into());
        ui.set_text_multiline(multiline);
    }

    /// The item in hand changed: shows what it holds and what can be done to it.
    /// Whether the list leaves room for what its items hold: not when it holds only a row
    /// to make the first of them.
    fn lists_beside_detail(&self) -> bool {
        self.found
            .iter()
            .any(|found| matches!(found.target, Target::Copied(_) | Target::Snippet(_) | Target::Note(_)))
    }

    fn chose(&mut self, index: usize) {
        let ui = self.ui();
        self.confirming = false;
        ui.set_actions_open(false);
        let facts = match self.found.get(index).map(|found| &found.target) {
            Some(Target::Copied(copied)) => self.clipboard.get(*copied).map_or_else(Vec::new, |copied| {
                let fact = |label: &str, value: String| Fact { label: label.into(), value: value.into() };
                let mut facts = vec![fact("類型", copied_kind(copied).0.into())];
                if copied.files.len() > 1 {
                    facts.push(fact("項目", calc::format(copied.files.len() as f64)));
                } else if copied.files.is_empty() && copied.picture.is_none() {
                    facts.push(fact("字元", calc::format(copied.text.chars().count() as f64)));
                }
                facts.push(fact("複製時間", date_time(copied.at)));
                facts
            }),
            _ => Vec::new(),
        };
        ui.set_facts(ModelRc::from(Rc::new(VecModel::from(facts))));
        let picture = match self.found.get(index).map(|found| &found.target) {
            Some(Target::Copied(copied)) => self.clipboard.get(*copied).and_then(|copied| copied.picture.as_deref()),
            _ => None,
        };
        ui.set_detail_picture(picture.and_then(|path| slint::Image::load_from_path(path).ok()).unwrap_or_default());
        let detail = match self.found.get(index).map(|found| &found.target) {
            Some(Target::Copied(copied)) => self.clipboard.get(*copied).map(|copied| copied.text.clone()),
            Some(Target::Snippet(snippet)) => {
                self.store.snippets.get(*snippet).map(|snippet| snippet.text.clone())
            }
            Some(Target::Note(note)) => self.store.notes.get(*note).map(|note| note.text.clone()),
            Some(Target::Emoji(_)) => self.found.get(index).map(|found| found.title.clone()),
            _ => None,
        };
        // An emoji's name is said in the footer, where the grid has no room for it.
        if self.view == View::Emoji {
            ui.set_detail("".into());
            ui.set_status(detail.unwrap_or_default().into());
        } else {
            ui.set_status("".into());
            ui.set_detail(detail.unwrap_or_default().into());
            if self.lists_beside_detail() && ui.get_detail().is_empty() {
                // Keeps the list as narrow as it was laid out.
                ui.set_detail(" ".into());
            }
        }
        let actions = self.actions(index);
        ui.set_primary(actions.first().map(|(title, _, _)| title.as_str()).unwrap_or_default().into());
        let acts: Vec<Act> = actions
            .iter()
            .map(|(title, keys, _)| Act { title: title.into(), keys: (*keys).into(), danger: false })
            .collect();
        ui.set_actions(ModelRc::from(Rc::new(VecModel::from(acts))));
    }

    /// What can be done to an item, the first by Return and the second by Ctrl+Return.
    fn actions(&self, index: usize) -> Vec<(String, &'static str, Action)> {
        let Some(found) = self.found.get(index) else {
            return Vec::new();
        };
        let named = |title: &str| title.to_string();
        let alias = |key: String| (named("設定別名"), "", Action::Alias(key));
        match &found.target {
            Target::Header => Vec::new(),
            Target::App(app) => {
                vec![(named("開啟"), "↵", Action::Open), alias(format!("app:{}", self.apps[*app].id))]
            }
            Target::Command(command) => {
                let title = match command.does {
                    Does::Open(_) | Does::SearchMenus => "開啟",
                    _ => "執行",
                };
                vec![(named(title), "↵", Action::Open), alias(format!("cmd:{}", command.id))]
            }
            Target::Answer(result) => vec![
                (named("複製答案"), "↵", Action::Copy(result.clone())),
                (named("貼上答案"), "Ctrl ↵", Action::Paste(result.clone())),
            ],
            Target::File(path) => vec![
                (named("開啟"), "↵", Action::Open),
                (named("在「檔案」中顯示"), "Ctrl ↵", Action::ShowInFiles(path.clone())),
                (named("複製路徑"), "", Action::Copy(path.display().to_string())),
                (named("移到垃圾桶"), "", Action::Trash(path.clone())),
            ],
            Target::Copied(copied) => {
                vec![
                    (named("貼上"), "↵", Action::PasteCopied(*copied)),
                    (named("複製"), "Ctrl ↵", Action::CopyCopied(*copied)),
                    (named("從紀錄中移除"), "", Action::Forget(*copied)),
                    (named("清除剪貼簿紀錄"), "", Action::ClearClipboard),
                ]
            }
            Target::Emoji(glyph) => vec![
                (named("貼上"), "↵", Action::Paste(glyph.clone())),
                (named("複製"), "Ctrl ↵", Action::Copy(glyph.clone())),
            ],
            Target::Snippet(snippet) => {
                let text =
                    self.store.snippets.get(*snippet).map(|snippet| snippet.text.clone()).unwrap_or_default();
                let text =
                    expand_placeholders(&text, self.clipboard.iter().find(|copied| copied.files.is_empty() && copied.picture.is_none()).map(|copied| copied.text.as_str()));
                vec![
                    (named("貼上"), "↵", Action::Paste(text.clone())),
                    (named("複製"), "Ctrl ↵", Action::Copy(text)),
                    (named("編輯"), "", Action::Edit),
                    (named("建立文字片段"), "", Action::Create(Does::NewSnippet)),
                    (named("刪除"), "", Action::Delete),
                ]
            }
            Target::Quicklink(link, query) => {
                let mut actions = vec![(named("開啟"), "↵", Action::Open)];
                if query.is_none() {
                    let name =
                        self.store.quicklinks().get(*link).map(|link| link.name.clone()).unwrap_or_default();
                    actions.push(alias(format!("link:{name}")));
                    actions.push((named("編輯"), "", Action::Edit));
                    actions.push((named("建立快速連結"), "", Action::Create(Does::NewQuicklink)));
                    actions.push((named("刪除"), "", Action::Delete));
                }
                actions
            }
            Target::Note(note) => {
                let text = self.store.notes.get(*note).map(|note| note.text.clone()).unwrap_or_default();
                vec![
                    (named("編輯"), "↵", Action::Edit),
                    (named("複製"), "Ctrl ↵", Action::Copy(text)),
                    (named("新增筆記"), "", Action::Create(Does::NewNote)),
                    (named("刪除"), "", Action::Delete),
                ]
            }
            Target::Window(..) => vec![(named("切換"), "↵", Action::Open)],
            Target::Url(_) | Target::SearchFiles(_) | Target::SearchMenus(_) => {
                vec![(named("開啟"), "↵", Action::Open)]
            }
            Target::Create(does) => vec![(named("建立"), "↵", Action::Create(*does))],
        }
    }

    fn quit(&self) {
        let _ = slint::quit_event_loop();
    }

    /// Opens one of the launcher's own views.
    fn open_view(&mut self, view: View) {
        let ui = self.ui();
        if self.view == View::Root {
            self.root_query = ui.get_query().to_string();
        }
        self.view = view;
        self.form = None;
        if view == View::Windows {
            self.windows = msg("windows")
                .unwrap_or_default()
                .lines()
                .filter_map(|line| line.split_once('\t'))
                .map(|(app_id, title)| (app_id.to_string(), title.to_string()))
                .collect();
        }
        if view == View::Clipboard {
            self.clipboard = store::clipboard();
        }
        let title = commands::opening(view).map(|command| command.title).unwrap_or_default();
        ui.set_view_title(title.into());
        ui.set_placeholder(
            match view {
                View::Emoji => "搜尋表情符號",
                View::Files => "檔案名稱",
                _ => "搜尋",
            }
            .into(),
        );
        ui.set_query("".into());
        self.show();
    }

    fn back(&mut self) {
        let ui = self.ui();
        if self.form.take().is_some() {
            return self.show();
        }
        self.view = View::Root;
        ui.set_view_title("".into());
        ui.set_placeholder("搜尋".into());
        ui.set_query(self.root_query.clone().into());
        self.show();
    }

    fn open_form(&mut self, form: Form) {
        self.form = Some(form);
        self.show();
    }

    fn run_command(&mut self, command: &'static Command) {
        self.store.used(&format!("cmd:{}", command.id));
        match command.does {
            Does::Run(arguments) => {
                spawn(arguments[0], &arguments[1..]);
                self.quit();
            }
            Does::Message(message) => {
                msg(message);
                self.quit();
            }
            Does::Window(action) => {
                msg(&format!("after-launcher window {action}"));
                self.quit();
            }
            Does::Open(view) => self.open_view(view),
            Does::DarkMode => {
                let dark = std::process::Command::new("gsettings")
                    .args(["get", "org.gnome.desktop.interface", "color-scheme"])
                    .output()
                    .is_ok_and(|output| String::from_utf8_lossy(&output.stdout).contains("dark"));
                let scheme = if dark { "default" } else { "prefer-dark" };
                spawn("gsettings", &["set", "org.gnome.desktop.interface", "color-scheme", scheme]);
                self.quit();
            }
            Does::ClearClipboard => {
                self.clipboard = store::clipboard();
                self.ask("清除剪貼簿紀錄");
            }
            Does::SearchMenus => {
                msg("after-launcher menu-search ");
                self.quit();
            }
            Does::NewSnippet => self.open_form(Form::Snippet(None)),
            Does::NewQuicklink => self.open_form(Form::Quicklink(None)),
            Does::NewNote => self.open_form(Form::Note(None)),
        }
    }

    /// Asks in the action panel before doing what cannot be undone: deleting the item in
    /// hand, or for anything else, forgetting what was copied.
    fn ask(&mut self, title: &str) {
        let ui = self.ui();
        self.confirming = true;
        let danger = Act { title: title.into(), keys: "↵".into(), danger: true };
        let cancel = Act { title: "取消".into(), keys: "Esc".into(), danger: false };
        ui.set_actions(ModelRc::from(Rc::new(VecModel::from(vec![danger, cancel]))));
        ui.set_action_current(0);
        ui.set_actions_open(true);
    }

    /// Does one of the item in hand's actions.
    fn act(&mut self, index: usize) {
        let ui = self.ui();
        let current = ui.get_current() as usize;
        if self.confirming {
            self.confirming = false;
            ui.set_actions_open(false);
            if index != 0 {
                return self.chose(current);
            }
            let deleting = self.found.get(current).map(|found| found.target.clone()).filter(|target| {
                matches!(target, Target::Snippet(_) | Target::Quicklink(..) | Target::Note(_))
            });
            match deleting {
                Some(target) => self.delete(&target),
                None => {
                    store::set_clipboard(&[]);
                    self.clipboard.clear();
                }
            }
            return self.show();
        }
        let Some((_, _, action)) = self.actions(current).into_iter().nth(index) else {
            return;
        };
        ui.set_actions_open(false);
        let target = self.found[current].target.clone();
        match action {
            Action::Open => self.open_target(&target),
            Action::Copy(text) => {
                copy(&text);
                self.quit();
            }
            Action::Paste(text) => {
                self.count(&target);
                paste(&text);
                self.quit();
            }
            Action::CopyCopied(index) => {
                if let Some(copied) = self.clipboard.get(index) {
                    copy_again(copied);
                }
                self.quit();
            }
            Action::PasteCopied(index) => {
                if let Some(copied) = self.clipboard.get(index) {
                    copy_again(copied);
                    msg("after-launcher paste");
                }
                self.quit();
            }
            Action::ShowInFiles(path) => {
                show_in_files(&path);
                self.quit();
            }
            Action::Trash(path) => {
                if let Err(err) = gio::File::for_path(&path).trash(gio::Cancellable::NONE) {
                    eprintln!("{}: {err}", path.display());
                }
                self.show();
            }
            Action::Edit => match target {
                Target::Snippet(index) => self.open_form(Form::Snippet(Some(index))),
                Target::Quicklink(index, _) => self.open_form(Form::Quicklink(Some(index))),
                Target::Note(index) => self.open_form(Form::Note(Some(index))),
                _ => {}
            },
            Action::Delete => {
                let title = format!("刪除「{}」", self.found[current].title);
                self.ask(&title);
            }
            Action::Forget(index) => {
                self.clipboard = store::clipboard();
                if index < self.clipboard.len() {
                    self.clipboard.remove(index);
                    store::set_clipboard(&self.clipboard);
                }
                self.show();
            }
            Action::ClearClipboard => self.ask("清除剪貼簿紀錄"),
            Action::Alias(key) => self.open_form(Form::Alias(key)),
            Action::Create(does) => match does {
                Does::NewSnippet => self.open_form(Form::Snippet(None)),
                Does::NewQuicklink => self.open_form(Form::Quicklink(None)),
                Does::NewNote => self.open_form(Form::Note(None)),
                _ => {}
            },
        }
    }

    fn delete(&mut self, target: &Target) {
        match *target {
            Target::Snippet(index) if index < self.store.snippets.len() => {
                self.store.snippets.remove(index);
            }
            Target::Quicklink(index, _) => {
                let mut links = self.store.quicklinks();
                if index < links.len() {
                    links.remove(index);
                }
                self.store.quicklinks = Some(links);
            }
            Target::Note(index) if index < self.store.notes.len() => {
                self.store.notes.remove(index);
            }
            _ => return,
        }
        self.store.save();
    }

    /// Counts a use of what can be found again, so it comes first next time.
    fn count(&mut self, target: &Target) {
        let key = match target {
            Target::App(index) => format!("app:{}", self.apps[*index].id),
            Target::Quicklink(index, _) => {
                format!(
                    "link:{}",
                    self.store.quicklinks().get(*index).map(|link| link.name.clone()).unwrap_or_default()
                )
            }
            _ => return,
        };
        self.store.used(&key);
    }

    /// The first action of most things: opening it.
    fn open_target(&mut self, target: &Target) {
        match target {
            Target::App(index) => {
                self.count(target);
                let exec = self.apps[*index].exec.clone();
                if let Err(err) = std::process::Command::new("sh").arg("-c").arg(&exec).spawn() {
                    eprintln!("{exec}: {err}");
                }
                self.quit();
            }
            Target::Command(command) => self.run_command(command),
            Target::File(path) => {
                open(&path.display().to_string());
                self.quit();
            }
            Target::Quicklink(index, query) => {
                let Some(link) = self.store.quicklinks().get(*index).cloned() else {
                    return;
                };
                match query {
                    Some(query) => {
                        self.count(target);
                        let escaped = glib::Uri::escape_string(query, None, false);
                        open(&link.url.replace("{query}", &escaped));
                        self.quit();
                    }
                    // One that takes a query waits for it: its keyword is typed for it.
                    None if link.url.contains("{query}") => {
                        let ui = self.ui();
                        if self.view != View::Root {
                            self.back();
                        }
                        let keyword =
                            if link.keyword.is_empty() { link.name.clone() } else { link.keyword.clone() };
                        ui.set_query(format!("{keyword} ").into());
                        self.show();
                    }
                    None => {
                        self.count(target);
                        open(&link.url);
                        self.quit();
                    }
                }
            }
            Target::Window(index, app_id) => {
                msg(&format!("after-launcher focus {index} {app_id}"));
                self.quit();
            }
            Target::Url(url) => {
                open(url);
                self.quit();
            }
            Target::SearchMenus(query) => {
                msg(&format!("after-launcher menu-search {query}"));
                self.quit();
            }
            Target::SearchFiles(query) => {
                let query = query.clone();
                self.open_view(View::Files);
                self.ui().set_query(query.into());
                self.show();
            }
            _ => {}
        }
    }

    /// Return or a click on an item: its first action.
    fn activate(&mut self, index: usize) {
        self.ui().set_current(index as i32);
        if self.found.get(index).is_some_and(|found| !matches!(found.target, Target::Header)) {
            self.act(0);
        }
    }

    /// Moves the item in hand: by rows in a list, and in a grid by columns too.
    fn step(&mut self, columns: i32, rows: i32) {
        let ui = self.ui();
        let count = self.found.len() as i32;
        if count == 0 {
            return;
        }
        let current = ui.get_current();
        let grid = self.found.iter().any(|found| found.kind == "cell");
        let mut next = if grid { current + columns + rows * COLUMNS as i32 } else { current + rows };
        next = next.clamp(0, count - 1);
        // Headers are passed over.
        let direction = if rows < 0 || columns < 0 { -1 } else { 1 };
        while next >= 0 && next < count && matches!(self.found[next as usize].target, Target::Header) {
            next += direction;
        }
        if next < 0 || next >= count {
            return;
        }
        ui.set_current(next);
        self.chose(next as usize);
    }

    fn save_form(&mut self) {
        let ui = self.ui();
        let Some(form) = self.form.clone() else {
            return;
        };
        let name = ui.get_name_field().trim().to_string();
        let text = ui.get_text_field().to_string();
        let key = ui.get_key_field().trim().to_string();
        let missing = |what: &str| ui.set_form_error(format!("請輸入{what}").into());
        match form {
            Form::Snippet(index) => {
                if name.is_empty() {
                    return missing("名稱");
                }
                if text.trim().is_empty() {
                    return missing("內容");
                }
                let snippet = store::Snippet { name, text, keyword: key };
                match index {
                    Some(index) if index < self.store.snippets.len() => self.store.snippets[index] = snippet,
                    _ => self.store.snippets.push(snippet),
                }
                self.view = View::Snippets;
            }
            Form::Quicklink(index) => {
                if name.is_empty() {
                    return missing("名稱");
                }
                let url = text.trim().to_string();
                if !url.contains("://") {
                    return ui.set_form_error("網址要以 https:// 這類開頭".into());
                }
                let mut links = self.store.quicklinks();
                let link = store::Quicklink { name, url, keyword: key };
                match index {
                    Some(index) if index < links.len() => links[index] = link,
                    _ => links.push(link),
                }
                self.store.quicklinks = Some(links);
                self.view = View::Quicklinks;
            }
            Form::Note(index) => {
                if text.trim().is_empty() {
                    return missing("內容");
                }
                let note = store::Note { text, changed: store::now() };
                match index {
                    Some(index) if index < self.store.notes.len() => {
                        self.store.notes.remove(index);
                    }
                    _ => {}
                }
                // The latest first.
                self.store.notes.insert(0, note);
                self.view = View::Notes;
            }
            Form::Alias(key) => {
                if name.is_empty() {
                    self.store.aliases.remove(&key);
                } else {
                    self.store.aliases.insert(key, name);
                }
            }
        }
        self.store.save();
        self.form = None;
        let view = self.view;
        if view == View::Root {
            self.back();
        } else {
            let title = commands::opening(view).map(|command| command.title).unwrap_or_default();
            ui.set_view_title(title.into());
            ui.set_query("".into());
            self.show();
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments.iter().any(|argument| argument == "--remember-clipboard") {
        store::remember_clipboard();
        return Ok(());
    }
    let search = arguments
        .iter()
        .find_map(|argument| argument.strip_prefix("--search="))
        .unwrap_or_default()
        .to_string();
    let open_view =
        arguments.iter().find_map(|argument| argument.strip_prefix("--open=")).and_then(|view| match view {
            "clipboard" => Some(View::Clipboard),
            "emoji" => Some(View::Emoji),
            "files" => Some(View::Files),
            "snippets" => Some(View::Snippets),
            "quicklinks" => Some(View::Quicklinks),
            "notes" => Some(View::Notes),
            "windows" => Some(View::Windows),
            other => {
                eprintln!("--open: no view {other:?}");
                None
            }
        });

    let look = apps::Look::load();
    let themes: Vec<String> = look.icon_themes().into_iter().map(String::from).collect();
    let theme_names: Vec<&str> = themes.iter().map(String::as_str).collect();
    let apps = apps::applications();
    let no_icon = apps::find_icon(&apps::data_dirs(), &theme_names, apps::NO_ICON)
        .and_then(|path| Image::load_from_path(&path).ok())
        .unwrap_or_default();

    let launcher = Launcher::new()?;
    slint::set_xdg_app_id("nekoland-launcher")?;
    launcher.set_glass_opacity(look.glass_opacity);

    let weak = launcher.as_weak();
    let files = files::index(move || {
        let _ = weak.upgrade_in_event_loop(|launcher| launcher.invoke_refresh());
    });
    let state = Rc::new(RefCell::new(State {
        ui: launcher.as_weak(),
        apps,
        themes,
        icons: HashMap::new(),
        no_icon,
        languages: apps::languages(),
        store: store::Store::load(),
        clipboard: Vec::new(),
        emoji: None,
        files,
        files_ready: false,
        windows: Vec::new(),
        view: View::Root,
        root_query: String::new(),
        found: Vec::new(),
        form: None,
        confirming: false,
    }));

    macro_rules! on {
        ($connect:ident, |$state:ident $(, $argument:ident)*| $body:expr) => {{
            let state = state.clone();
            launcher.$connect(move |$($argument),*| {
                let mut $state = state.borrow_mut();
                $body
            });
        }};
    }
    on!(on_edited, |state, _query| state.show());
    on!(on_move, |state, columns, rows| state.step(columns, rows));
    on!(on_activate, |state, index| state.activate(index as usize));
    on!(on_act, |state, index| state.act(index as usize));
    on!(on_chose, |state, index| state.chose(index as usize));
    on!(on_back, |state| state.back());
    on!(on_save_form, |state| state.save_form());
    on!(on_refresh, |state| {
        state.files_ready = true;
        // Files found while something was typed show up under it.
        if state.form.is_none() && matches!(state.view, View::Root | View::Files) {
            let current = state.ui().get_current();
            state.show();
            if current < state.found.len() as i32 {
                state.ui().set_current(current);
            }
        }
    });
    launcher.on_cancelled(|| {
        let _ = slint::quit_event_loop();
    });

    launcher.set_query(search.into());
    match open_view {
        Some(view) => state.borrow_mut().open_view(view),
        None => state.borrow_mut().show(),
    }
    launcher.run()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_that_start_with_the_query_come_first() {
        assert_eq!(score("ter", "Terminal", "foot", None), Some(2));
        assert_eq!(score("ter", "GNOME Terminal", "", None), Some(3));
        assert_eq!(score("ter", "Files", "terminal", None), Some(4));
        assert_eq!(score("ter", "Files", "", None), None);
        assert_eq!(score("ff", "Firefox", "", Some("ff")), Some(0));
    }

    #[test]
    fn snippets_fill_in_what_they_hold() {
        let text = expand_placeholders("{clipboard}!", Some("hi"));
        assert_eq!(text, "hi!");
        assert!(!expand_placeholders("{date}", None).contains('{'));
    }
}
