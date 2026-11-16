//! What the hover panel tells of a file, read on a thread, as FinderHover tells it on the
//! Mac: its kind and size, what its kind holds (a photo's camera and exposure, a video's
//! length and codec, an archive's files, a repository's branch, ...), its dates and
//! details, where it is, and the note kept with it.
//!
//! Formats with a common reader on Ubuntu are read with it when it is installed: exiftool
//! for photos, office documents and fonts, ffprobe for video and sound, pdfinfo, bsdtar for
//! archives and e-books, sqlite3, git and file. The rest are read here.

use std::{
    cell::Cell,
    ffi::{CString, OsStr},
    io::Read,
    os::unix::{
        ffi::OsStrExt,
        fs::{MetadataExt, PermissionsExt},
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::UNIX_EPOCH,
};

use gio::prelude::*;
use serde_json::Value;

mod samples;
pub use samples::{catalogue, plain_label, samples};

/// Where a file's note is kept, with the file, so that it goes where the file is copied or
/// moved: the freedesktop.org comment.
const NOTE_ATTRIBUTE: &str = "user.xdg.comment";
/// Where browsers note the page a file was downloaded from, and the page that led there.
const ORIGIN_ATTRIBUTE: &str = "user.xdg.origin.url";
const REFERRER_ATTRIBUTE: &str = "user.xdg.referrer.url";
const TAGS_ATTRIBUTE: &str = "user.xdg.tags";
/// Text larger than this is not read through for its lines and words, as FinderHover's
/// large-file protection has it; without it, up to `UNPROTECTED_LIMIT`, since the file is
/// read into the compositor's own memory.
const READ_LIMIT: u64 = 8 << 20;
const UNPROTECTED_LIMIT: u64 = 256 << 20;
/// The text a font's sample shows, and how large, in points.
const FONT_SAMPLE: &str = "Aa Bb Cc 123";
const FONT_SAMPLE_POINTS: i32 = 28;

thread_local! {
    // Each file is read on a thread of its own, which sets this before anything is read.
    static LIMIT: Cell<u64> = const { Cell::new(READ_LIMIT) };
}

fn read_limit() -> u64 {
    LIMIT.get()
}
/// How long a reader may take before it is given up on.
const TOOL_SECONDS: &str = "5";

pub type Rgb = (f64, f64, f64);

/// The colours of the kinds' key figures, the Mac's system colours.
pub const YELLOW: Rgb = (1.0, 0.80, 0.0);
pub const INDIGO: Rgb = (0.37, 0.36, 0.90);
pub const PINK: Rgb = (1.0, 0.18, 0.33);
pub const RED: Rgb = (1.0, 0.23, 0.19);
pub const BLUE: Rgb = (0.04, 0.52, 1.0);
pub const MINT: Rgb = (0.0, 0.78, 0.75);
pub const CYAN: Rgb = (0.20, 0.68, 0.90);
pub const ORANGE: Rgb = (1.0, 0.58, 0.0);
pub const BROWN: Rgb = (0.64, 0.52, 0.37);
pub const GREEN: Rgb = (0.20, 0.78, 0.35);
pub const PURPLE: Rgb = (0.69, 0.32, 0.87);

#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub label: String,
    pub value: String,
    /// Takes a whole row.
    pub wide: bool,
}

/// A figure as large as a kind's key figure, over its label.
#[derive(Clone, Debug, PartialEq)]
pub struct Spec {
    pub value: String,
    pub label: String,
    pub colored: bool,
}

/// How a part of a bar is filled.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shade {
    Accent,
    Fill,
    Track,
}

/// One piece of a kind's block, in the order it is drawn.
#[derive(Clone, Debug, PartialEq)]
pub enum Part {
    /// A camera and its lens, a song and its artist, a document and its author.
    Title {
        title: Option<String>,
        points: f64,
        accent: Option<String>,
        subtitle: Option<String>,
    },
    /// One headline figure, with its unit and a note at the other end; `small` for a
    /// repository's branch.
    Hero {
        value: Option<String>,
        unit: Option<String>,
        trailing: Option<String>,
        small: bool,
    },
    Specs(Vec<Spec>),
    Caption(String),
    /// Lines of code, comments and blank lines, side by side.
    Segments(Vec<(f64, Shade)>),
    /// Sizes as bars as long as each is of the largest: an archive's files and itself.
    Sizes(Vec<(String, u64, Shade)>),
    Fields(Vec<Field>, usize),
    /// A font's letters set in it: a PNG of them in black on clear, at `text::picture_scale`
    /// pixels to the point.
    Sample(Vec<u8>),
    /// Where a file was downloaded from: the site large, the rest of the address under it.
    Source {
        host: String,
        rest: String,
        address: String,
    },
}

/// What a file's kind holds, in the kind's colour.
#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    /// The kind, as `hover_hide` names it to leave the block out, and `name/label` a field.
    pub name: &'static str,
    pub color: Rgb,
    pub parts: Vec<Part>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Facts {
    pub path: PathBuf,
    pub name: String,
    /// Its kind, and its size or how many items it holds: `QuickTime 影片 · 1.2 GB`.
    pub details: String,
    pub kind: String,
    pub size: Option<String>,
    pub tags: Vec<String>,
    pub block: Option<Block>,
    pub source: Option<Block>,
    /// A picture's width and height, drawn as a frame of its proportions.
    pub frame: Option<(u64, u64)>,
    pub dates: Vec<Field>,
    pub modified: Option<String>,
    pub file: Vec<Field>,
    /// The folder it is in, as `~ › 文件`.
    pub folder: String,
    pub volume: String,
    pub available: Option<String>,
    /// How much of its volume is used, from 0 to 1.
    pub used: Option<f64>,
    /// Its note, or why it could not be read.
    pub note: Result<String, String>,
}

/// Reads all the panel says of `path`, whose content type is `kind`; with `protect`, large
/// text is not read through. It runs the readers above, so it is called on a thread of
/// its own.
pub fn read(path: &Path, kind: &str, protect: bool) -> Facts {
    LIMIT.set(if protect { READ_LIMIT } else { UNPROTECTED_LIMIT });
    // A kind told by the name alone is unknown for a database or a program; their content
    // tells it.
    let sniffed = (kind == "application/octet-stream")
        .then(|| {
            gio::File::for_path(path)
                .query_info(
                    "standard::content-type",
                    gio::FileQueryInfoFlags::NONE,
                    gio::Cancellable::NONE,
                )
                .ok()?
                .content_type()
        })
        .flatten();
    let kind = sniffed.as_deref().unwrap_or(kind);
    let link = std::fs::symlink_metadata(path).ok();
    let meta = std::fs::metadata(path).ok().or_else(|| link.clone());
    let is_dir = meta.as_ref().is_some_and(|meta| meta.is_dir());
    let name = path.file_name().map_or_else(
        || path.to_string_lossy().into_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    let description = gio::content_type_get_description(kind).to_string();
    let size = meta
        .as_ref()
        .filter(|_| !is_dir)
        .map(|meta| size_text(meta.len()));
    let count = is_dir.then(|| visible_entries(path)).flatten();
    let details = [
        Some(description),
        count.map(|count| format!("{count} 個項目")),
        size.clone(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ");

    let (block, frame) = match &meta {
        Some(meta) => type_block(path, kind, meta),
        None => (None, None),
    };
    let modified = meta.as_ref().and_then(|meta| time_text(meta.mtime()));
    let mut dates = Vec::new();
    if let Some(meta) = &meta {
        let created = meta
            .created()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .and_then(|since| time_text(since.as_secs() as i64));
        push(&mut dates, "建立", created);
        push(&mut dates, "修改", modified.clone());
    }
    let opened = last_opened(path);
    push(&mut dates, "上次開啟", opened);
    if let Some(Field { wide, .. }) = dates.last_mut().filter(|field| field.label == "上次開啟") {
        *wide = true;
    }

    Facts {
        path: path.to_path_buf(),
        name,
        details,
        kind: gio::content_type_get_description(kind).to_string(),
        size,
        tags: attribute(path, TAGS_ATTRIBUTE)
            .ok()
            .flatten()
            .map(|tags| {
                tags.split(',')
                    .map(str::trim)
                    .filter(|tag| !tag.is_empty())
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default(),
        block,
        source: source_block(path),
        frame,
        dates,
        modified,
        file: file_fields(path, kind, link.as_ref(), meta.as_ref()),
        folder: path.parent().map(breadcrumb).unwrap_or_default(),
        volume: volume_name(path),
        available: None,
        used: None,
        note: attribute(path, NOTE_ATTRIBUTE)
            .map(Option::unwrap_or_default)
            .map_err(|_| "無法載入備註。".to_string()),
    }
    .with_space(path)
}

impl Facts {
    fn with_space(mut self, path: &Path) -> Self {
        let info = gio::File::for_path(path)
            .query_filesystem_info("filesystem::size,filesystem::free", gio::Cancellable::NONE);
        if let Ok(info) = info {
            let (size, free) = (
                info.attribute_uint64("filesystem::size"),
                info.attribute_uint64("filesystem::free"),
            );
            if size > 0 {
                self.available = Some(format!("可用 {}，共 {}", size_text(free), size_text(size)));
                self.used = Some(1.0 - free as f64 / size as f64);
            }
        }
        self
    }
}

/// Keeps `note` with the file, or takes the note away when it is empty.
pub fn save_note(path: &Path, note: &str) -> std::io::Result<()> {
    let file = CString::new(path.as_os_str().as_bytes())?;
    let name = CString::new(NOTE_ATTRIBUTE).expect("no NUL in the name");
    // SAFETY: both strings are NUL-terminated and outlive the calls; the value is a slice.
    let done = unsafe {
        if note.is_empty() {
            let removed = libc::removexattr(file.as_ptr(), name.as_ptr());
            if removed != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ENODATA) {
                return Ok(());
            }
            removed
        } else {
            libc::setxattr(file.as_ptr(), name.as_ptr(), note.as_ptr().cast(), note.len(), 0)
        }
    };
    if done == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// An extended attribute as text: none when the file has none of that name, or its file
/// system keeps none at all.
fn attribute(path: &Path, name: &str) -> std::io::Result<Option<String>> {
    let file = CString::new(path.as_os_str().as_bytes())?;
    let name = CString::new(name)?;
    let mut value = vec![0u8; 64 << 10];
    // SAFETY: the buffer is as long as said, and both strings are NUL-terminated.
    let read = unsafe {
        libc::getxattr(
            file.as_ptr(),
            name.as_ptr(),
            value.as_mut_ptr().cast(),
            value.len(),
        )
    };
    if read < 0 {
        let err = std::io::Error::last_os_error();
        return match err.raw_os_error() {
            Some(libc::ENODATA | libc::ENOTSUP) => Ok(None),
            _ => Err(err),
        };
    }
    value.truncate(read as usize);
    Ok(Some(String::from_utf8_lossy(&value).into_owned()))
}

/// How many extended attributes the file has.
fn attribute_count(path: &Path) -> usize {
    let Ok(file) = CString::new(path.as_os_str().as_bytes()) else {
        return 0;
    };
    let mut names = vec![0u8; 64 << 10];
    // SAFETY: the buffer is as long as said.
    let read = unsafe { libc::listxattr(file.as_ptr(), names.as_mut_ptr().cast(), names.len()) };
    if read <= 0 {
        return 0;
    }
    names[..read as usize]
        .split(|byte| *byte == 0)
        .filter(|name| !name.is_empty())
        .count()
}

fn push(fields: &mut Vec<Field>, label: &str, value: Option<String>) {
    if let Some(value) = value.filter(|value| !value.trim().is_empty()) {
        fields.push(Field {
            label: label.into(),
            value: value.trim().into(),
            wide: false,
        });
    }
}

fn push_wide(fields: &mut Vec<Field>, label: &str, value: Option<String>) {
    push(fields, label, value);
    if let Some(field) = fields.last_mut().filter(|field| field.label == label) {
        field.wide = true;
    }
}

fn spec(value: Option<String>, label: &str, colored: bool) -> Option<Spec> {
    Some(Spec {
        value: value.filter(|value| !value.trim().is_empty())?.trim().to_string(),
        label: label.into(),
        colored,
    })
}

/// The first spec in the kind's colour, the rest in ink, as FinderHover's documents.
fn specs_first_colored(specs: Vec<Option<Spec>>) -> Vec<Spec> {
    let mut specs: Vec<Spec> = specs.into_iter().flatten().collect();
    if let Some(first) = specs.first_mut() {
        first.colored = true;
    }
    specs
}

/// A block of its parts, or none when it has nothing to show.
fn block(name: &'static str, color: Rgb, parts: Vec<Option<Part>>) -> Option<Block> {
    let parts: Vec<Part> = parts.into_iter().flatten().collect();
    (!parts.is_empty()).then_some(Block { name, color, parts })
}

fn title(
    title: Option<String>,
    points: f64,
    accent: Option<String>,
    subtitle: Option<String>,
) -> Option<Part> {
    let some = |text: Option<String>| text.filter(|text| !text.trim().is_empty());
    let (title, accent, subtitle) = (some(title), some(accent), some(subtitle));
    (title.is_some() || accent.is_some() || subtitle.is_some()).then_some(Part::Title {
        title,
        points,
        accent,
        subtitle,
    })
}

fn specs(specs: Vec<Spec>) -> Option<Part> {
    (!specs.is_empty()).then_some(Part::Specs(specs))
}

fn fields(fields: Vec<Field>, columns: usize) -> Option<Part> {
    (!fields.is_empty()).then_some(Part::Fields(fields, columns))
}

fn hero(value: Option<String>, unit: Option<String>, trailing: Option<String>) -> Option<Part> {
    (value.is_some() || trailing.is_some()).then_some(Part::Hero {
        value,
        unit,
        trailing,
        small: false,
    })
}

fn yes_no(yes: bool) -> String {
    if yes { "是" } else { "否" }.into()
}

pub fn size_text(bytes: u64) -> String {
    gio::glib::format_size(bytes).to_string()
}

fn number(count: impl Into<u64>) -> String {
    let digits = count.into().to_string();
    let mut grouped = String::new();
    for (at, digit) in digits.chars().enumerate() {
        if at > 0 && (digits.len() - at) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

/// A moment as the panel words it: `2026/9/3 14:02`.
fn time_text(unix: i64) -> Option<String> {
    let time = gio::glib::DateTime::from_unix_local(unix).ok()?;
    time.format("%Y/%-m/%-d %H:%M").ok().map(String::from)
}

/// `2026:09:03 14:02:00`, `2026-09-03T14:02:00+08`, or `2026-09-03 14:02:00 +0800`, as
/// the panel words a moment, leaving the rest as it is.
fn date_text(text: &str) -> String {
    let digits: Vec<u32> = text
        .split(|character: char| !character.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .take(5)
        .filter_map(|part| part.parse().ok())
        .collect();
    match digits[..] {
        [year, month, day, hour, minute] if year > 1900 && (1..=12).contains(&month) => {
            format!("{year}/{month}/{day} {hour:02}:{minute:02}")
        }
        [year, month, day] if year > 1900 && (1..=12).contains(&month) => format!("{year}/{month}/{day}"),
        _ => text.trim().to_string(),
    }
}

/// `12:48`, or `1:02:03` past an hour.
fn duration_text(seconds: f64) -> String {
    let total = seconds.round() as u64;
    let (hours, minutes, seconds) = (total / 3600, total / 60 % 60, total % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

fn bitrate_text(bits: f64) -> String {
    if bits >= 1e6 {
        format!("{:.1} Mbps", bits / 1e6)
    } else {
        format!("{:.0} kbps", bits / 1e3)
    }
}

/// The folder as `~ › 文件 › 工作`.
fn breadcrumb(folder: &Path) -> String {
    let home = gio::glib::home_dir();
    let (start, rest) = match folder.strip_prefix(&home) {
        Ok(rest) => ("~".to_string(), rest),
        Err(_) => ("/".to_string(), folder.strip_prefix("/").unwrap_or(folder)),
    };
    std::iter::once(start)
        .chain(rest.iter().map(|part| part.to_string_lossy().into_owned()))
        .collect::<Vec<_>>()
        .join(" › ")
}

/// The volume the file is on and its file system: the computer's name for the root, as
/// Files names it, else the folder it is mounted on.
fn volume_name(path: &Path) -> String {
    let mounts = std::fs::read_to_string("/proc/self/mountinfo").unwrap_or_default();
    let mounted = mounts.lines().filter_map(|line| {
        let (before, after) = line.split_once(" - ")?;
        // The mount point, with spaces and the like written as octal escapes.
        let point = before
            .split(' ')
            .nth(4)?
            .replace("\\040", " ")
            .replace("\\011", "\t");
        let fs_type = after.split(' ').next()?.to_string();
        Some((PathBuf::from(point), fs_type))
    });
    let Some((point, fs_type)) = mounted
        .filter(|(point, _)| path.starts_with(point))
        .max_by_key(|(point, _)| point.as_os_str().len())
    else {
        return String::new();
    };
    let name = match point.file_name() {
        Some(name) => name.to_string_lossy().into_owned(),
        None => gio::glib::host_name().to_string(),
    };
    format!("{name} · {fs_type}")
}

fn visible_entries(folder: &Path) -> Option<usize> {
    Some(
        std::fs::read_dir(folder)
            .ok()?
            .flatten()
            .filter(|entry| !entry.file_name().as_bytes().starts_with(b"."))
            .count(),
    )
}

/// When the file was last opened through a program that notes recent files, as GTK's do,
/// and how many times.
fn last_opened(path: &Path) -> Option<String> {
    let recent = gio::glib::user_data_dir().join("recently-used.xbel");
    let text = std::fs::read_to_string(recent).ok()?;
    let uri = gio::File::for_path(path).uri();
    let start = text.find(&format!("href=\"{uri}\""))?;
    let bookmark = &text[start..start + text[start..].find("</bookmark>")?];
    let mut latest: Option<String> = None;
    let mut count = 0u64;
    for application in bookmark.split("<bookmark:application ").skip(1) {
        let attributes = attributes(application);
        if let Some(modified) = value_of(&attributes, "modified") {
            if latest.as_deref() < Some(modified) {
                latest = Some(modified.to_string());
            }
        }
        count += value_of(&attributes, "count")
            .and_then(|count| count.parse().ok())
            .unwrap_or(0);
    }
    let when = gio::glib::DateTime::from_iso8601(latest.as_deref()?, None)
        .ok()?
        .to_local()
        .ok()?;
    let when = when.format("%Y/%-m/%-d %H:%M").ok()?;
    Some(if count > 0 {
        format!("{when} · {count} 次")
    } else {
        when.to_string()
    })
}

fn file_fields(
    path: &Path,
    kind: &str,
    link: Option<&std::fs::Metadata>,
    meta: Option<&std::fs::Metadata>,
) -> Vec<Field> {
    let mut fields = Vec::new();
    if let Some(meta) = meta {
        push(&mut fields, "實際佔用", Some(size_text(meta.blocks() * 512)));
        push(&mut fields, "屬性修改日期", time_text(meta.ctime()));
    }
    push_wide(&mut fields, "類型識別碼", Some(kind.to_string()));
    if link.is_some_and(|link| link.file_type().is_symlink()) {
        push_wide(
            &mut fields,
            "連結目標",
            std::fs::read_link(path)
                .ok()
                .map(|target| target.to_string_lossy().into_owned()),
        );
    }
    if let Some(meta) = meta {
        if meta.nlink() > 1 && !meta.is_dir() {
            push(&mut fields, "硬連結數", Some(meta.nlink().to_string()));
        }
    }
    let attributes = attribute_count(path);
    if attributes > 0 {
        push(&mut fields, "擴展屬性", Some(attributes.to_string()));
    }
    if let Some(meta) = meta {
        let mode = meta.permissions().mode();
        let letters: String = (0..9)
            .map(|bit| {
                if mode & (0o400 >> bit) != 0 {
                    ['r', 'w', 'x'][bit % 3]
                } else {
                    '-'
                }
            })
            .collect();
        push(
            &mut fields,
            "模式",
            Some(format!("{letters} ({:o})", mode & 0o7777)),
        );
        push(&mut fields, "擁有者", Some(owner(meta.uid(), meta.gid())));
    }
    if path
        .file_name()
        .is_some_and(|name| name.as_bytes().starts_with(b"."))
    {
        push(&mut fields, "隱藏檔案", Some(yes_no(true)));
    }
    fields
}

fn owner(uid: u32, gid: u32) -> String {
    // SAFETY: getpwuid and getgrgid return static records or null; their names are copied
    // out before anything else could call them on this thread.
    let name = |pointer: *const libc::c_char| unsafe {
        std::ffi::CStr::from_ptr(pointer).to_string_lossy().into_owned()
    };
    let user = unsafe { libc::getpwuid(uid).as_ref() }.map_or(uid.to_string(), |record| name(record.pw_name));
    let group =
        unsafe { libc::getgrgid(gid).as_ref() }.map_or(gid.to_string(), |record| name(record.gr_name));
    format!("{user}:{group}")
}

/// Where the file was downloaded from, as browsers note it.
fn source_block(path: &Path) -> Option<Block> {
    let address = attribute(path, ORIGIN_ATTRIBUTE).ok().flatten()?;
    let (host, rest) = match address.split_once("://") {
        Some((_, after)) => match after.find('/') {
            Some(slash) => (after[..slash].to_string(), after[slash..].to_string()),
            None => (after.to_string(), String::new()),
        },
        None => (address.clone(), String::new()),
    };
    let mut referrer = Vec::new();
    push_wide(
        &mut referrer,
        "參照頁面",
        attribute(path, REFERRER_ATTRIBUTE).ok().flatten(),
    );
    block(
        "source",
        PURPLE,
        vec![Some(Part::Source { host, rest, address }), fields(referrer, 2)],
    )
}

/// The output of a reader, when it is installed and does not fail or take too long.
fn run(program: &str, args: &[&OsStr]) -> Option<String> {
    let output = Command::new("timeout")
        .arg(TOOL_SECONDS)
        .arg(program)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

fn first_bytes(path: &Path, count: usize) -> Vec<u8> {
    let mut start = Vec::new();
    if let Ok(file) = std::fs::File::open(path) {
        let _ = file.take(count as u64).read_to_end(&mut start);
    }
    start
}

/// The file as text, when it is small enough to read through.
fn text_of(path: &Path, meta: &std::fs::Metadata) -> Option<(String, &'static str)> {
    if meta.len() > read_limit() {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    if bytes.starts_with(&[0xff, 0xfe]) || bytes.starts_with(&[0xfe, 0xff]) {
        return Some((String::new(), "UTF-16"));
    }
    match String::from_utf8(bytes) {
        Ok(text) if text.is_ascii() => Some((text, "ASCII")),
        Ok(text) => Some((text, "UTF-8")),
        Err(err) => Some((String::from_utf8_lossy(err.as_bytes()).into_owned(), "非 UTF-8")),
    }
}

fn extension(path: &Path) -> String {
    path.extension()
        .map(|ext| ext.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

/// The block for what the file's kind holds, and a picture's width and height.
fn type_block(path: &Path, kind: &str, meta: &std::fs::Metadata) -> (Option<Block>, Option<(u64, u64)>) {
    if meta.is_dir() {
        return (xcode_block(path).or_else(|| git_block(path)), None);
    }
    let ext = extension(path);
    let ext = ext.as_str();
    let block = match kind {
        "image/svg+xml" | "image/svg+xml-compressed" => vector_block(path, meta, ext),
        "image/x-eps" | "application/postscript" => vector_block(path, meta, ext),
        "image/vnd.adobe.photoshop" => psd_block(path),
        _ if kind.starts_with("image/") => return photo_block(path),
        _ if kind.starts_with("video/") => media_block(path, true),
        _ if kind.starts_with("audio/") => media_block(path, false),
        "application/pdf" => pdf_block(path),
        "application/epub+zip" => ebook_block(path),
        "application/x-cd-image" | "application/x-qemu-disk" | "application/x-raw-disk-image" => {
            disk_block(path, meta, ext)
        }
        "application/x-desktop" => desktop_entry_block(path),
        "application/vnd.debian.binary-package" => deb_block(path),
        "application/vnd.sqlite3" | "application/x-sqlite3" => sqlite_block(path),
        "application/x-executable"
        | "application/x-sharedlib"
        | "application/x-pie-executable"
        | "application/x-object"
        | "application/vnd.appimage"
        | "application/x-iso9660-appimage" => executable_block(path),
        "text/markdown" | "text/x-markdown" => text_block(path, meta, markdown_block),
        "text/html" | "application/xhtml+xml" => text_block(path, meta, html_block),
        "application/x-subrip" | "text/vtt" | "text/x-ssa" => {
            text_block(path, meta, |text, encoding| subtitle_block(text, encoding, ext))
        }
        _ if kind.starts_with("font/") || kind.starts_with("application/x-font") => font_block(path),
        _ if kind.starts_with("model/") => model_block(path, meta, ext),
        _ if is_office(kind) => office_block(path),
        _ if is_archive(kind) => archive_block(path, meta, ext),
        _ => match ext {
            "md" | "markdown" => text_block(path, meta, markdown_block),
            "json" | "yaml" | "yml" | "toml" | "ini" | "conf" | "cfg" => {
                text_block(path, meta, |text, encoding| config_block(text, encoding, ext))
            }
            "srt" | "vtt" | "ass" | "ssa" => {
                text_block(path, meta, |text, encoding| subtitle_block(text, encoding, ext))
            }
            "obj" | "stl" | "gltf" | "glb" => model_block(path, meta, ext),
            "img" | "qcow2" | "iso" => disk_block(path, meta, ext),
            _ => match language(path) {
                Some(language) => {
                    text_block(path, meta, |text, encoding| code_block(text, encoding, &language))
                }
                None => None,
            },
        },
    };
    (block, None)
}

fn text_block(
    path: &Path,
    meta: &std::fs::Metadata,
    read: impl FnOnce(&str, &str) -> Option<Block>,
) -> Option<Block> {
    let (text, encoding) = text_of(path, meta)?;
    read(&text, encoding)
}

fn is_office(kind: &str) -> bool {
    kind.starts_with("application/vnd.openxmlformats-officedocument")
        || kind.starts_with("application/vnd.oasis.opendocument")
        || kind.starts_with("application/vnd.ms-")
        || kind == "application/msword"
        || kind == "application/rtf"
}

fn is_archive(kind: &str) -> bool {
    matches!(
        kind,
        "application/zip"
            | "application/x-7z-compressed"
            | "application/vnd.rar"
            | "application/x-rar"
            | "application/x-rar-compressed"
            | "application/x-tar"
            | "application/x-compressed-tar"
            | "application/x-bzip-compressed-tar"
            | "application/x-bzip2-compressed-tar"
            | "application/x-xz-compressed-tar"
            | "application/x-zstd-compressed-tar"
            | "application/x-lzma-compressed-tar"
            | "application/x-cpio"
            | "application/x-xar"
            | "application/vnd.ms-cab-compressed"
            | "application/java-archive"
    )
}

/// exiftool's tags of the file, by name.
fn exiftool(path: &Path) -> Option<serde_json::Map<String, Value>> {
    let output = run(
        "exiftool",
        &[
            "-j".as_ref(),
            "-api".as_ref(),
            "largefilesupport=1".as_ref(),
            path.as_os_str(),
        ],
    )?;
    let mut tags: Vec<serde_json::Map<String, Value>> = serde_json::from_str(&output).ok()?;
    tags.pop()
}

fn tag(tags: &serde_json::Map<String, Value>, names: &[&str]) -> Option<String> {
    // A font's names come in languages, as `FontFamily-en-US`; any of them will do.
    let named = |name: &str| {
        tags.get(name).or_else(|| {
            tags.iter()
                .find(|(key, _)| key.strip_prefix(name).is_some_and(|rest| rest.starts_with('-')))
                .map(|(_, value)| value)
        })
    };
    names.iter().find_map(|name| match named(name)? {
        Value::String(text) if !text.trim().is_empty() => Some(text.trim().to_string()),
        Value::Number(number) => Some(number.to_string()),
        Value::Array(items) => Some(
            items
                .iter()
                .map(|item| item.as_str().map_or_else(|| item.to_string(), String::from))
                .collect::<Vec<_>>()
                .join(", "),
        ),
        _ => None,
    })
}

fn tag_number(tags: &serde_json::Map<String, Value>, name: &str) -> Option<u64> {
    match tags.get(name)? {
        Value::Number(number) => number.as_u64(),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

/// A photo's camera, lens and exposure, when and where it was taken, and its pixels.
fn photo_block(path: &Path) -> (Option<Block>, Option<(u64, u64)>) {
    let tags = exiftool(path).unwrap_or_default();
    let frame = match (tag_number(&tags, "ImageWidth"), tag_number(&tags, "ImageHeight")) {
        (Some(width), Some(height)) if width > 0 && height > 0 => Some((width, height)),
        _ => gdk_pixbuf::Pixbuf::file_info(path)
            .map(|(_, width, height)| (width as u64, height as u64))
            .filter(|(width, height)| *width > 0 && *height > 0),
    };
    // `Canon EOS R5` names its maker already; `ILCE-7M4` does not, so `SONY` goes before it.
    let brand = tag(&tags, &["Make"]).filter(|make| !make.is_empty());
    let camera = tag(&tags, &["Model"]).map(|model| match brand {
        Some(make)
            if !model
                .to_lowercase()
                .contains(&make.split(' ').next().unwrap_or_default().to_lowercase()) =>
        {
            format!("{make} {model}")
        }
        _ => model,
    });
    let lens = tag(&tags, &["LensModel", "LensID", "Lens"]);
    let exposure = vec![
        spec(tag(&tags, &["FocalLength"]), "焦距", true),
        spec(tag(&tags, &["FNumber"]).map(|f| format!("ƒ/{f}")), "光圈", true),
        spec(
            tag(&tags, &["ExposureTime", "ShutterSpeed"]).map(|time| format!("{time} s")),
            "快門",
            true,
        ),
        spec(tag(&tags, &["ISO"]), "ISO", true),
    ];
    let mut details = Vec::new();
    push(
        &mut details,
        "拍攝",
        tag(&tags, &["DateTimeOriginal", "CreateDate"]).map(|date| date_text(&date)),
    );
    push(&mut details, "位置", tag(&tags, &["GPSPosition"]));
    push(&mut details, "創作工具", tag(&tags, &["CreatorTool", "Software"]));
    push(
        &mut details,
        "色彩描述檔",
        tag(&tags, &["ProfileDescription", "ICCProfileName"]),
    );
    push(
        &mut details,
        "色彩空間",
        tag(&tags, &["ColorSpace", "ColorSpaceData"]),
    );
    push(
        &mut details,
        "位元深度",
        tag(&tags, &["BitsPerSample", "BitDepth"]).map(|bits| {
            let bits = bits.split([' ', ',']).next().unwrap_or_default().to_string();
            format!("{bits}-bit")
        }),
    );
    push(
        &mut details,
        "HDR 增益圖",
        tags.contains_key("HDRGainMapVersion").then(|| yes_no(true)),
    );
    push_wide(&mut details, "標題", tag(&tags, &["Headline", "Title"]));
    push(
        &mut details,
        "創作者",
        tag(&tags, &["Creator", "Artist", "By-line"]),
    );
    push(
        &mut details,
        "版權",
        tag(&tags, &["Copyright", "Rights", "CopyrightNotice"]),
    );
    push(
        &mut details,
        "評分",
        tag_number(&tags, "Rating")
            .filter(|rating| *rating > 0)
            .map(|rating| {
                let rating = rating.min(5) as usize;
                "★".repeat(rating) + &"☆".repeat(5 - rating)
            }),
    );
    push_wide(&mut details, "關鍵字", tag(&tags, &["Keywords", "Subject"]));
    push_wide(
        &mut details,
        "描述",
        tag(&tags, &["Description", "ImageDescription", "Caption-Abstract"]),
    );
    let block = block(
        "照片",
        YELLOW,
        vec![
            title(camera, 13.0, None, lens),
            specs(exposure.into_iter().flatten().collect()),
            fields(details, 2),
        ],
    );
    (block, frame)
}

/// A video's length, picture, codec and tracks, or a song's title, artist and sound.
fn media_block(path: &Path, video: bool) -> Option<Block> {
    let output = run(
        "ffprobe",
        &[
            "-v",
            "quiet",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
            "-show_chapters",
        ]
        .map(OsStr::new)
        .into_iter()
        .chain([path.as_os_str()])
        .collect::<Vec<_>>(),
    )?;
    let probe: Value = serde_json::from_str(&output).ok()?;
    let format = &probe["format"];
    let streams = probe["streams"].as_array().cloned().unwrap_or_default();
    let of_type = |kind: &str| -> Vec<&Value> {
        streams
            .iter()
            .filter(|stream| stream["codec_type"] == kind)
            // A song's cover is a picture stream too.
            .filter(|stream| stream["disposition"]["attached_pic"] != 1)
            .collect()
    };
    let float = |value: &Value| {
        value
            .as_str()
            .and_then(|text| text.parse::<f64>().ok())
            .or(value.as_f64())
    };
    let duration = float(&format["duration"]).filter(|seconds| *seconds > 0.0);
    let bitrate = float(&format["bit_rate"]).filter(|bits| *bits > 0.0);
    let tags = |name: &str| -> Option<String> {
        format["tags"].as_object()?.iter().find_map(|(key, value)| {
            key.eq_ignore_ascii_case(name)
                .then(|| value.as_str().map(str::trim).map(String::from))
                .flatten()
        })
    };

    if !video {
        let audio = of_type("audio");
        let sound = audio.first()?;
        let channels = sound["channels"].as_u64().map(|channels| match channels {
            1 => "單聲道".to_string(),
            2 => "立體聲".to_string(),
            _ => sound["channel_layout"]
                .as_str()
                .map_or(channels.to_string(), String::from),
        });
        let rate = float(&sound["sample_rate"]).map(|rate| format!("{} kHz", rate / 1000.0));
        let album = [
            tags("album"),
            tags("date").map(|date| date.chars().take(4).collect()),
            tags("genre"),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");
        let mut more = Vec::new();
        push(&mut more, "專輯演出者", tags("album_artist"));
        return block(
            "音訊",
            PINK,
            vec![
                title(tags("title"), 16.0, tags("artist"), Some(album)),
                specs(
                    [
                        spec(duration.map(duration_text), "時長", false),
                        spec(bitrate.map(bitrate_text), "位元率", false),
                        spec(rate, "取樣率", false),
                        spec(channels, "聲道", false),
                    ]
                    .into_iter()
                    .flatten()
                    .collect(),
                ),
                fields(more, 2),
            ],
        );
    }

    let pictures = of_type("video");
    let picture = pictures.first()?;
    let transfer = picture["color_transfer"].as_str().unwrap_or_default();
    let dolby = picture["side_data_list"].as_array().is_some_and(|data| {
        data.iter().any(|side| {
            side["side_data_type"]
                .as_str()
                .is_some_and(|kind| kind.contains("DOVI"))
        })
    });
    let hdr = if dolby {
        Some("Dolby Vision")
    } else {
        match transfer {
            "smpte2084" => Some("HDR10"),
            "arib-std-b67" => Some("HLG"),
            _ => None,
        }
    };
    let resolution = match (picture["width"].as_u64(), picture["height"].as_u64()) {
        (Some(width), Some(height)) => Some(format!("{width}×{height}")),
        _ => None,
    };
    let rate = picture["avg_frame_rate"].as_str().and_then(|rate| {
        let (top, bottom) = rate.split_once('/')?;
        let (top, bottom): (f64, f64) = (top.parse().ok()?, bottom.parse().ok()?);
        (bottom > 0.0 && top > 0.0).then(|| {
            let fps = top / bottom;
            if (fps - fps.round()).abs() < 0.005 {
                format!("{fps:.0} fps")
            } else {
                format!("{fps:.2} fps")
            }
        })
    });
    let codec = picture["codec_name"].as_str().map(|codec| {
        match codec {
            "hevc" => "HEVC",
            "h264" => "H.264",
            "av1" => "AV1",
            "vp9" => "VP9",
            "vp8" => "VP8",
            "prores" => "ProRes",
            "mpeg4" => "MPEG-4",
            "mpeg2video" => "MPEG-2",
            other => return other.to_uppercase(),
        }
        .to_string()
    });
    let count = |kind: &str, unit: &str| {
        let count = streams
            .iter()
            .filter(|stream| stream["codec_type"] == kind)
            .count();
        (count > 0).then(|| format!("{count} 個{unit}"))
    };
    let chapters = probe["chapters"].as_array().map_or(0, Vec::len);
    let container = format["format_long_name"]
        .as_str()
        .map(|name| name.split(" / ").next().unwrap_or(name).to_string());
    let tracks = [
        container,
        Some(format!("{} 個視訊軌", pictures.len())),
        count("audio", "音訊軌"),
        count("subtitle", "字幕軌"),
        (chapters > 0).then(|| format!("{chapters} 個章節")),
        count("attachment", "附件"),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ");
    let mut more = Vec::new();
    let primaries = picture["color_primaries"]
        .as_str()
        .map(|primaries| match primaries {
            "bt2020" => "BT.2020".to_string(),
            "bt709" => "BT.709".to_string(),
            "smpte432" => "Display P3".to_string(),
            other => other.to_uppercase(),
        });
    push(&mut more, "色域", primaries);
    push(
        &mut more,
        "轉換函數",
        (!transfer.is_empty()).then(|| transfer.to_uppercase()),
    );
    block(
        "影片",
        INDIGO,
        vec![
            (duration.is_some() || hdr.is_some()).then(|| Part::Hero {
                value: duration.map(duration_text),
                unit: duration.map(|_| "時長".to_string()),
                trailing: hdr.map(String::from),
                small: false,
            }),
            specs(
                [
                    spec(resolution, "解析度", false),
                    spec(rate, "影格率", false),
                    spec(codec, "編碼", false),
                    spec(bitrate.map(bitrate_text), "位元率", false),
                ]
                .into_iter()
                .flatten()
                .collect(),
            ),
            (!tracks.is_empty()).then_some(Part::Caption(tracks)),
            fields(more, 2),
        ],
    )
}

fn pdf_block(path: &Path) -> Option<Block> {
    let output = run("pdfinfo", &["-isodates".as_ref(), path.as_os_str()])?;
    let info = |key: &str| -> Option<String> {
        output.lines().find_map(|line| {
            let value = line.strip_prefix(key)?.strip_prefix(':')?.trim();
            (!value.is_empty()).then(|| value.to_string())
        })
    };
    let page_size = info("Page size").map(|size| {
        // `595.276 x 841.89 pts (A4)`: the paper's name when it has one, else millimetres.
        match (size.find('('), size.rfind(')')) {
            (Some(open), Some(close)) if open < close => size[open + 1..close].to_string(),
            _ => {
                let numbers: Vec<f64> = size
                    .split_whitespace()
                    .filter_map(|part| part.parse().ok())
                    .collect();
                match numbers[..] {
                    [width, height, ..] => {
                        format!("{:.0} × {:.0} mm", width / 72.0 * 25.4, height / 72.0 * 25.4)
                    }
                    _ => size,
                }
            }
        }
    });
    let mut more = Vec::new();
    push_wide(&mut more, "主旨", info("Subject"));
    push_wide(&mut more, "關鍵字", info("Keywords"));
    push(&mut more, "建立者", info("Creator"));
    push(&mut more, "製作者", info("Producer"));
    push(
        &mut more,
        "文件建立",
        info("CreationDate").map(|date| date_text(&date)),
    );
    push(
        &mut more,
        "文件修改",
        info("ModDate").map(|date| date_text(&date)),
    );
    block(
        "PDF",
        RED,
        vec![
            title(info("Title"), 15.0, None, info("Author")),
            specs(specs_first_colored(vec![
                spec(
                    info("Pages").map(|pages| number(pages.parse::<u64>().unwrap_or_default())),
                    "頁數",
                    false,
                ),
                spec(page_size, "頁面大小", false),
                spec(info("PDF version"), "PDF 版本", false),
                spec(
                    info("Encrypted").map(|yes| yes_no(yes.starts_with("yes"))),
                    "已加密",
                    false,
                ),
            ])),
            fields(more, 2),
        ],
    )
}

fn office_block(path: &Path) -> Option<Block> {
    let tags = exiftool(path)?;
    let mut more = Vec::new();
    push_wide(&mut more, "主旨", tag(&tags, &["Subject"]));
    push_wide(&mut more, "關鍵字", tag(&tags, &["Keywords"]));
    push_wide(&mut more, "註解", tag(&tags, &["Description", "Comments"]));
    push(
        &mut more,
        "文件建立",
        tag(&tags, &["CreateDate", "CreationDate"]).map(|date| date_text(&date)),
    );
    push(
        &mut more,
        "文件修改",
        tag(&tags, &["ModifyDate", "Date"]).map(|date| date_text(&date)),
    );
    push(
        &mut more,
        "修改者",
        tag(&tags, &["LastModifiedBy", "InitialCreator"]),
    );
    push(&mut more, "公司", tag(&tags, &["Company"]));
    push(&mut more, "類別", tag(&tags, &["Category"]));
    block(
        "Office 文件",
        BLUE,
        vec![
            title(
                tag(&tags, &["Title"]),
                15.0,
                None,
                tag(&tags, &["Creator", "Author"]),
            ),
            specs(specs_first_colored(vec![
                spec(
                    tag(&tags, &["Pages", "Document-statisticPage-count"]),
                    "頁數",
                    false,
                ),
                spec(
                    tag(&tags, &["Words", "Document-statisticWord-count"]),
                    "字數",
                    false,
                ),
                spec(tag(&tags, &["Document-statisticTable-count"]), "工作表數", false),
                spec(tag(&tags, &["Slides"]), "投影片數", false),
            ])),
            fields(more, 2),
        ],
    )
}

fn font_block(path: &Path) -> Option<Block> {
    let tags = exiftool(path).unwrap_or_default();
    let mut more = Vec::new();
    push(&mut more, "名稱", tag(&tags, &["FontName"]));
    push(
        &mut more,
        "版本",
        tag(&tags, &["NameTableVersion", "FontVersion"]),
    );
    push(&mut more, "字形數", glyph_count(path).map(number));
    push(&mut more, "設計者", tag(&tags, &["Designer"]));
    push_wide(&mut more, "版權", tag(&tags, &["Copyright"]));
    block(
        "字型",
        GREEN,
        vec![
            title(
                tag(&tags, &["FontFamily"]),
                15.0,
                tag(&tags, &["FontSubfamily"]),
                None,
            ),
            font_sample(path).map(Part::Sample),
            fields(more, 2),
        ],
    )
}

/// The font's sample, drawn by HarfBuzz's hb-view: Pango cannot set text in a font that is
/// not installed without adding it to every font Nekoland draws with.
fn font_sample(path: &Path) -> Option<Vec<u8>> {
    let size = FONT_SAMPLE_POINTS * crate::text::picture_scale();
    let output = Command::new("timeout")
        .arg(TOOL_SECONDS)
        .arg("hb-view")
        .arg(format!("--font-size={size}"))
        .args(["--margin=0", "--background=#00000000", "--foreground=#000000"])
        .arg("--output-format=png")
        .arg(path)
        .arg(FONT_SAMPLE)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    (output.status.success() && !output.stdout.is_empty()).then_some(output.stdout)
}

/// How many glyphs a TrueType or OpenType font has, from its `maxp` table.
fn glyph_count(path: &Path) -> Option<u64> {
    let start = first_bytes(path, 64 << 10);
    let u16_at = |at: usize| Some(u16::from_be_bytes(start.get(at..at + 2)?.try_into().ok()?));
    let u32_at = |at: usize| Some(u32::from_be_bytes(start.get(at..at + 4)?.try_into().ok()?));
    let tables = u16_at(4)? as usize;
    (0..tables).find_map(|table| {
        let record = 12 + table * 16;
        (start.get(record..record + 4)? == b"maxp")
            .then(|| u16_at(u32_at(record + 8)? as usize + 4))
            .flatten()
            .map(u64::from)
    })
}

/// What an archive holds, read without taking it apart.
fn archive_block(path: &Path, meta: &std::fs::Metadata, ext: &str) -> Option<Block> {
    let output = Command::new("timeout")
        .arg(TOOL_SECONDS)
        .args(["bsdtar", "-tvf"])
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .ok()?;
    let errors = String::from_utf8_lossy(&output.stderr).to_lowercase();
    let start = first_bytes(path, 8);
    // A zip's first file says in its flags whether it is encrypted.
    let zip_encrypted = start.starts_with(b"PK\x03\x04") && start.get(6).is_some_and(|flags| flags & 1 == 1);
    let encrypted = zip_encrypted || errors.contains("encrypt") || errors.contains("passphrase");
    let listing = String::from_utf8_lossy(&output.stdout);
    let files: Vec<u64> = listing
        .lines()
        .filter(|line| !line.starts_with('d'))
        .filter_map(|line| line.split_whitespace().nth(4)?.parse().ok())
        .collect();
    if files.is_empty() && !encrypted {
        return None;
    }
    let uncompressed: u64 = files.iter().sum();
    let format = match ext {
        "gz" | "tgz" => "tar.gz".to_string(),
        "bz2" | "tbz2" => "tar.bz2".to_string(),
        "xz" | "txz" => "tar.xz".to_string(),
        "zst" => "tar.zst".to_string(),
        other => other.to_uppercase(),
    };
    let mut more = Vec::new();
    push(&mut more, "格式", Some(format));
    if uncompressed > 0 {
        let saved = 1.0 - meta.len() as f64 / uncompressed as f64;
        push(
            &mut more,
            "壓縮率",
            Some(format!("{:.0}%", saved.max(0.0) * 100.0)),
        );
    }
    block(
        "壓縮檔",
        BROWN,
        vec![
            hero(
                (!files.is_empty()).then(|| number(files.len() as u64)),
                (!files.is_empty()).then(|| "個檔案".to_string()),
                encrypted.then(|| "已加密".to_string()),
            ),
            (uncompressed > 0).then(|| {
                Part::Sizes(vec![
                    ("未壓縮".into(), uncompressed, Shade::Fill),
                    ("壓縮檔".into(), meta.len(), Shade::Accent),
                ])
            }),
            fields(more, 3),
        ],
    )
}

/// An e-book's title, author and publisher, from the package document inside it.
fn ebook_block(path: &Path) -> Option<Block> {
    let member = |name: &str| run("bsdtar", &["-xOf".as_ref(), path.as_os_str(), name.as_ref()]);
    let container = member("META-INF/container.xml")?;
    let at = container.find("full-path=\"")? + "full-path=\"".len();
    let package = member(&container[at..at + container[at..].find('"')?])?;
    let element = |name: &str| -> Option<String> {
        let open = package.find(&format!("<dc:{name}"))?;
        let start = open + package[open..].find('>')? + 1;
        let end = start + package[start..].find(&format!("</dc:{name}>"))?;
        Some(strip_tags(&package[start..end]))
    };
    let isbn = package
        .match_indices("<dc:identifier")
        .filter_map(|(open, _)| {
            let start = open + package[open..].find('>')? + 1;
            let value = &package[start..start + package[start..].find('<')?];
            let digits: String = value.chars().filter(char::is_ascii_alphanumeric).collect();
            (digits.len() == 13 || digits.len() == 10)
                .then(|| value.trim().trim_start_matches("urn:isbn:").to_string())
        })
        .next();
    let mut more = Vec::new();
    push_wide(&mut more, "書名", element("title"));
    push(&mut more, "作者", element("creator"));
    push(&mut more, "出版社", element("publisher"));
    push(
        &mut more,
        "出版日期",
        element("date").map(|date| date_text(&date)),
    );
    push(&mut more, "ISBN", isbn);
    push(&mut more, "語言", element("language"));
    push_wide(&mut more, "內容簡介", element("description"));
    block("電子書", BROWN, vec![fields(more, 2)])
}

fn strip_tags(text: &str) -> String {
    let mut plain = String::new();
    let mut inside = false;
    for character in text.chars() {
        match character {
            '<' => inside = true,
            '>' => inside = false,
            _ if !inside => plain.push(character),
            _ => {}
        }
    }
    plain
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// A CD or DVD image's volume, a virtual machine's disk, or a raw disk's partitions.
fn disk_block(path: &Path, meta: &std::fs::Metadata, ext: &str) -> Option<Block> {
    let mut specs = Vec::new();
    let mut sizes = Vec::new();
    let mut more = Vec::new();
    let start = first_bytes(path, 34 << 10);
    if start.get(32769..32774) == Some(b"CD001") {
        specs.push(spec(Some("ISO 9660".into()), "格式", false));
        let volume = String::from_utf8_lossy(&start[32808..32840]).trim().to_string();
        let blocks = u32::from_le_bytes(start[32848..32852].try_into().ok()?);
        sizes.push(("總大小".into(), u64::from(blocks) * 2048, Shade::Fill));
        push(&mut more, "磁碟區名稱", Some(volume));
        push(
            &mut more,
            "可開機",
            Some(yes_no(
                start.get(34823..34828) == Some(b"CD001") && start.get(34816) == Some(&0),
            )),
        );
    } else if start.starts_with(b"QFI\xfb") {
        specs.push(spec(Some("qcow2".into()), "格式", false));
        let virtual_size = u64::from_be_bytes(start.get(24..32)?.try_into().ok()?);
        sizes.push(("總大小".into(), virtual_size, Shade::Fill));
        sizes.push(("壓縮大小".into(), meta.blocks() * 512, Shade::Track));
        // Byte 32 on starts the encryption method, 0 for none.
        let encryption = u32::from_be_bytes(start.get(32..36)?.try_into().ok()?);
        push(&mut more, "加密狀態", Some(yes_no(encryption != 0)));
    } else if ext == "img" {
        specs.push(spec(Some("Raw".into()), "格式", false));
        let scheme = if start.get(512..520) == Some(b"EFI PART") {
            Some("GPT")
        } else if start.get(510..512) == Some(&[0x55, 0xaa]) {
            Some("MBR")
        } else {
            None
        };
        specs.push(spec(scheme.map(String::from), "分割區配置", false));
        sizes.push(("總大小".into(), meta.len(), Shade::Fill));
    } else {
        return None;
    }
    block(
        "磁碟映像",
        CYAN,
        vec![
            self::specs(specs.into_iter().flatten().collect()),
            (!sizes.is_empty()).then_some(Part::Sizes(sizes)),
            fields(more, 3),
        ],
    )
}

/// An application's desktop entry: its name, what it runs and what kind it is.
fn desktop_entry_block(path: &Path) -> Option<Block> {
    let text = std::fs::read_to_string(path).ok()?;
    let entry = text.split("[Desktop Entry]").nth(1)?.split("\n[").next()?;
    let language = std::env::var("LANG").unwrap_or_default();
    let language = language.split('.').next().unwrap_or_default();
    let key = |name: &str| -> Option<String> {
        let plain = format!("{name}=");
        let local = format!("{name}[{language}]=");
        let line = |prefix: &str| {
            entry
                .lines()
                .find_map(|line| line.strip_prefix(prefix).map(str::to_string))
        };
        line(&local).or_else(|| line(&plain))
    };
    let mut more = Vec::new();
    push_wide(
        &mut more,
        "識別碼",
        path.file_stem().map(|stem| stem.to_string_lossy().into_owned()),
    );
    push_wide(&mut more, "指令", key("Exec"));
    push(
        &mut more,
        "類別",
        key("Categories").map(|categories| categories.trim_end_matches(';').replace(';', ", ")),
    );
    push(
        &mut more,
        "版本",
        key("X-AppImage-Version").or_else(|| key("Version")),
    );
    block(
        "應用程式",
        BLUE,
        vec![title(key("Name"), 15.0, None, key("Comment")), fields(more, 2)],
    )
}

/// A Debian package's name, version and what it is for.
fn deb_block(path: &Path) -> Option<Block> {
    let output = run("dpkg-deb", &["-f".as_ref(), path.as_os_str()])?;
    let field = |name: &str| -> Option<String> {
        output.lines().find_map(|line| {
            line.strip_prefix(name)?
                .strip_prefix(':')
                .map(|value| value.trim().to_string())
        })
    };
    let mut more = Vec::new();
    push_wide(&mut more, "套件", field("Package"));
    push(&mut more, "架構", field("Architecture"));
    push(
        &mut more,
        "安裝大小",
        field("Installed-Size")
            .and_then(|kilobytes| kilobytes.parse::<u64>().ok())
            .map(|kilobytes| size_text(kilobytes * 1024)),
    );
    push_wide(&mut more, "維護者", field("Maintainer"));
    push_wide(&mut more, "描述", field("Description"));
    block(
        "Debian 套件",
        BLUE,
        vec![hero(field("Version"), None, field("Section")), fields(more, 2)],
    )
}

/// An executable's architecture, type and how it is linked, as `file` tells.
fn executable_block(path: &Path) -> Option<Block> {
    let output = run("file", &["-b".as_ref(), path.as_os_str()])?;
    let parts: Vec<&str> = output.trim().split(", ").collect();
    let class = parts.first()?.strip_prefix("ELF ")?;
    let architecture = parts.get(1).map(|arch| arch.to_string());
    let linking = parts
        .iter()
        .find(|part| part.ends_with("linked"))
        .map(|part| match *part {
            "dynamically linked" => "動態".to_string(),
            "statically linked" => "靜態".to_string(),
            other => other.to_string(),
        });
    let kernel = parts
        .iter()
        .find_map(|part| part.strip_prefix("for GNU/Linux "))
        .map(|version| format!("Linux {version}"));
    let mut more = Vec::new();
    push(&mut more, "架構", architecture);
    push(&mut more, "檔案類型", Some(class.to_string()));
    push(&mut more, "連結", linking);
    push(&mut more, "最低核心", kernel);
    push(
        &mut more,
        "符號",
        parts.iter().find(|part| part.ends_with("stripped")).map(|part| {
            if part.starts_with("not") {
                "保留"
            } else {
                "已移除"
            }
            .to_string()
        }),
    );
    push_wide(
        &mut more,
        "載入器",
        parts
            .iter()
            .find_map(|part| part.strip_prefix("interpreter "))
            .map(String::from),
    );
    push_wide(
        &mut more,
        "BuildID",
        parts
            .iter()
            .find_map(|part| part.split_once("]=").map(|(_, id)| id.to_string())),
    );
    block("執行檔", BLUE, vec![fields(more, 2)])
}

/// A SQLite database's tables and rows, read without writing to it.
fn sqlite_block(path: &Path) -> Option<Block> {
    let start = first_bytes(path, 100);
    if !start.starts_with(b"SQLite format 3\0") || start.len() < 100 {
        return None;
    }
    let u32_at = |at: usize| u32::from_be_bytes(start[at..at + 4].try_into().expect("4 bytes"));
    let page_size = match u16::from_be_bytes([start[16], start[17]]) {
        1 => 65536,
        size => u32::from(size),
    };
    let encoding = match u32_at(56) {
        2 => "UTF-16le",
        3 => "UTF-16be",
        _ => "UTF-8",
    };
    let sqlite = |query: &str| {
        run(
            "sqlite3",
            &[
                "-readonly".as_ref(),
                "-safe".as_ref(),
                "-batch".as_ref(),
                path.as_os_str(),
                query.as_ref(),
            ],
        )
    };
    let mut counts = std::collections::HashMap::new();
    let mut tables = Vec::new();
    if let Some(listing) = sqlite("SELECT type, name FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'") {
        for line in listing.lines() {
            if let Some((kind, name)) = line.split_once('|') {
                *counts.entry(kind.to_string()).or_insert(0u64) += 1;
                if kind == "table" {
                    tables.push(name.to_string());
                }
            }
        }
    }
    let rows = (!tables.is_empty() && tables.len() <= 200)
        .then(|| {
            let sum = tables
                .iter()
                .map(|table| format!("(SELECT count(*) FROM \"{}\")", table.replace('"', "\"\"")))
                .collect::<Vec<_>>()
                .join("+");
            sqlite(&format!("SELECT {sum}"))?.trim().parse::<u64>().ok()
        })
        .flatten();
    let count = |kind: &str| counts.get(kind).copied();
    let mut more = Vec::new();
    push(&mut more, "資料表數", count("table").map(number));
    push(&mut more, "索引數", count("index").map(number));
    push(&mut more, "觸發器數", count("trigger").map(number));
    push(&mut more, "視圖數", count("view").map(number));
    push(&mut more, "總列數", rows.map(number));
    push(&mut more, "架構版本", Some(u32_at(40).to_string()));
    push(&mut more, "頁面大小", Some(format!("{page_size} bytes")));
    push(&mut more, "編碼", Some(encoding.to_string()));
    block("SQLite", BLUE, vec![fields(more, 2)])
}

/// An Xcode project's targets, Swift version and deployment target, read from its
/// `project.pbxproj`, or a workspace's projects, as FinderHover reads them.
fn xcode_block(path: &Path) -> Option<Block> {
    let (file, workspace) = match extension(path).as_str() {
        "xcodeproj" => ("project.pbxproj", false),
        "xcworkspace" => ("contents.xcworkspacedata", true),
        _ => return None,
    };
    let file = path.join(file);
    let (text, _) = text_of(&file, &std::fs::metadata(&file).ok()?)?;
    let count = |marker: &str| text.matches(marker).count();
    let mut more = Vec::new();
    if workspace {
        push(&mut more, "Target 數", Some(number(count("<FileRef") as u64)));
        return block("Xcode 專案", BLUE, vec![fields(more, 2)]);
    }
    // `KEY = value;`, the first one, unquoted.
    let setting = |key: &str| {
        let rest = text.split(&format!("{key} = ")).nth(1)?;
        let value = rest.split(';').next()?.trim().trim_matches('"');
        (!value.is_empty()).then(|| value.to_string())
    };
    let deployment = [
        ("IPHONEOS", "iOS"),
        ("MACOSX", "macOS"),
        ("TVOS", "tvOS"),
        ("WATCHOS", "watchOS"),
    ]
    .into_iter()
    .find_map(|(key, platform)| {
        Some(format!(
            "{platform} {}",
            setting(&format!("{key}_DEPLOYMENT_TARGET"))?
        ))
    });
    let has = |product: &str| text.contains(&format!("productType = \"com.apple.product-type.{product}\""));
    push(
        &mut more,
        "Target 數",
        Some(number(count("isa = PBXNativeTarget") as u64)),
    );
    push(&mut more, "Swift 版本", setting("SWIFT_VERSION"));
    push(&mut more, "部署目標", deployment);
    let configurations = count("isa = XCBuildConfiguration");
    push(
        &mut more,
        "配置數",
        (configurations > 0).then(|| number(configurations as u64)),
    );
    push(&mut more, "組織名稱", setting("ORGANIZATIONNAME"));
    push(
        &mut more,
        "單元測試",
        has("bundle.unit-test").then(|| "是".to_string()),
    );
    push(
        &mut more,
        "UI 測試",
        has("bundle.ui-testing").then(|| "是".to_string()),
    );
    block("Xcode 專案", BLUE, vec![fields(more, 2)])
}

/// A 3D model's vertices, faces, meshes and materials.
fn model_block(path: &Path, meta: &std::fs::Metadata, ext: &str) -> Option<Block> {
    let mut more = Vec::new();
    match ext {
        "obj" => {
            let (text, _) = text_of(path, meta)?;
            let mut materials = std::collections::HashSet::new();
            let (mut vertices, mut faces, mut meshes) = (0u64, 0u64, 0u64);
            let (mut low, mut high) = ([f64::MAX; 3], [f64::MIN; 3]);
            for line in text.lines() {
                let mut words = line.split_whitespace();
                match words.next() {
                    Some("v") => {
                        vertices += 1;
                        for (axis, word) in words.take(3).enumerate() {
                            if let Ok(value) = word.parse::<f64>() {
                                low[axis] = low[axis].min(value);
                                high[axis] = high[axis].max(value);
                            }
                        }
                    }
                    Some("f") => faces += 1,
                    Some("o" | "g") => meshes += 1,
                    Some("usemtl") => {
                        materials.insert(words.next().unwrap_or_default().to_string());
                    }
                    _ => {}
                }
            }
            push(&mut more, "格式", Some("Wavefront OBJ".into()));
            push(&mut more, "頂點數", Some(number(vertices)));
            push(&mut more, "面數", Some(number(faces)));
            push(&mut more, "網格數", Some(number(meshes.max(1))));
            push(&mut more, "材質數", Some(number(materials.len() as u64)));
            if vertices > 0 {
                let size: Vec<String> = (0..3)
                    .map(|axis| format!("{:.2}", high[axis] - low[axis]))
                    .collect();
                push_wide(&mut more, "邊界框", Some(size.join(" × ")));
            }
        }
        "stl" => {
            let start = first_bytes(path, 84);
            let ascii = start.starts_with(b"solid") && meta.len() as usize != 84 + 50 * triangles(&start);
            let faces = if ascii {
                let (text, _) = text_of(path, meta)?;
                text.matches("facet normal").count() as u64
            } else {
                triangles(&start) as u64
            };
            push(
                &mut more,
                "格式",
                Some(if ascii { "STL (ASCII)" } else { "STL" }.into()),
            );
            push(&mut more, "頂點數", Some(number(faces * 3)));
            push(&mut more, "面數", Some(number(faces)));
        }
        "gltf" | "glb" => {
            let bytes = (meta.len() <= (read_limit() * 8).min(UNPROTECTED_LIMIT))
                .then(|| std::fs::read(path).ok())
                .flatten()?;
            let json: Value = if ext == "glb" {
                let length = u32::from_le_bytes(bytes.get(12..16)?.try_into().ok()?) as usize;
                serde_json::from_slice(bytes.get(20..20 + length)?).ok()?
            } else {
                serde_json::from_slice(&bytes).ok()?
            };
            let count = |key: &str| json[key].as_array().map_or(0, Vec::len) as u64;
            let accessors = json["accessors"].as_array().cloned().unwrap_or_default();
            let vertices: u64 = json["meshes"]
                .as_array()
                .into_iter()
                .flatten()
                .flat_map(|mesh| mesh["primitives"].as_array().cloned().unwrap_or_default())
                .filter_map(|primitive| {
                    accessors.get(primitive["attributes"]["POSITION"].as_u64()? as usize)?["count"].as_u64()
                })
                .sum();
            push(
                &mut more,
                "格式",
                Some(if ext == "glb" { "glTF (二進位)" } else { "glTF" }.into()),
            );
            push(&mut more, "頂點數", Some(number(vertices)));
            push(&mut more, "網格數", Some(number(count("meshes"))));
            push(&mut more, "材質數", Some(number(count("materials"))));
            push(
                &mut more,
                "動畫數",
                (count("animations") > 0).then(|| number(count("animations"))),
            );
            push(&mut more, "骨架", Some(yes_no(count("skins") > 0)));
        }
        _ => return None,
    }
    block("3D 模型", ORANGE, vec![fields(more, 2)])
}

fn triangles(start: &[u8]) -> usize {
    start.get(80..84).map_or(0, |count| {
        u32::from_le_bytes(count.try_into().expect("4 bytes")) as usize
    })
}

/// An SVG's or EPS's size, its maker and how much it draws.
fn vector_block(path: &Path, meta: &std::fs::Metadata, ext: &str) -> Option<Block> {
    let (text, _) = text_of(path, meta)?;
    let mut more = Vec::new();
    if ext == "eps" || ext == "ps" || text.starts_with("%!") {
        let comment = |name: &str| {
            text.lines()
                .take(200)
                .find_map(|line| line.strip_prefix(name).map(|value| value.trim().to_string()))
        };
        push(&mut more, "格式", Some("EPS".into()));
        push(
            &mut more,
            "尺寸",
            comment("%%BoundingBox:").map(|bounds| {
                let numbers: Vec<f64> = bounds
                    .split_whitespace()
                    .filter_map(|word| word.parse().ok())
                    .collect();
                match numbers[..] {
                    [left, bottom, right, top] => format!("{:.0} × {:.0} pt", right - left, top - bottom),
                    _ => bounds,
                }
            }),
        );
        push(&mut more, "建立者", comment("%%Creator:"));
        push(
            &mut more,
            "版本",
            text.lines()
                .next()
                .and_then(|line| line.strip_prefix("%!"))
                .map(String::from),
        );
    } else {
        let open = text.find("<svg")?;
        let tag = &text[open..open + text[open..].find('>')?];
        let attributes = attributes(tag);
        let attribute = |name: &str| value_of(&attributes, name).map(String::from);
        push(&mut more, "格式", Some("SVG".into()));
        push(
            &mut more,
            "尺寸",
            attribute("width")
                .zip(attribute("height"))
                .map(|(width, height)| format!("{width} × {height}")),
        );
        push(&mut more, "視圖框", attribute("viewBox"));
        push(
            &mut more,
            "元素數",
            Some(number(
                text.match_indices('<')
                    .filter(|(at, _)| {
                        text[at + 1..].starts_with(|character: char| character.is_ascii_alphabetic())
                    })
                    .count() as u64,
            )),
        );
        let creator = attribute("inkscape:version")
            .map(|version| {
                format!(
                    "Inkscape {}",
                    version.split_whitespace().next().unwrap_or_default()
                )
            })
            .or_else(|| {
                let at = text.find("Generator: ")? + "Generator: ".len();
                Some(text[at..].split([',', '-']).next()?.trim().to_string())
            });
        push(&mut more, "建立者", creator);
        push(&mut more, "版本", attribute("version"));
    }
    block("向量圖形", ORANGE, vec![fields(more, 2)])
}

/// A Photoshop document's size, colour, depth and layers, from its header.
fn psd_block(path: &Path) -> Option<Block> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut header = [0u8; 26];
    file.read_exact(&mut header).ok()?;
    if &header[..4] != b"8BPS" {
        return None;
    }
    let u16_at = |bytes: &[u8], at: usize| u16::from_be_bytes([bytes[at], bytes[at + 1]]);
    let u32_at = |bytes: &[u8], at: usize| u32::from_be_bytes(bytes[at..at + 4].try_into().expect("4 bytes"));
    let (height, width) = (u32_at(&header, 14), u32_at(&header, 18));
    let depth = u16_at(&header, 22);
    let mode = match u16_at(&header, 24) {
        0 => "點陣圖",
        1 => "灰階",
        2 => "索引色",
        3 => "RGB",
        4 => "CMYK",
        7 => "多重色版",
        8 => "雙色調",
        9 => "Lab",
        _ => "其他",
    };
    // Past the colour table and the image resources, the layers' section starts with its
    // length, then the layer list's, then how many layers; a negative count says the first
    // alpha channel is the merged picture's transparency.
    let skip = |file: &mut std::fs::File| -> Option<()> {
        let mut length = [0u8; 4];
        file.read_exact(&mut length).ok()?;
        std::io::Seek::seek(
            file,
            std::io::SeekFrom::Current(i64::from(u32::from_be_bytes(length))),
        )
        .ok()?;
        Some(())
    };
    let layers = (|| {
        skip(&mut file)?;
        skip(&mut file)?;
        let mut section = [0u8; 10];
        file.read_exact(&mut section).ok()?;
        Some(i16::from_be_bytes([section[8], section[9]]))
    })();
    let mut more = Vec::new();
    push(&mut more, "尺寸", Some(format!("{width} × {height}")));
    push(
        &mut more,
        "圖層數",
        layers.map(|layers| layers.unsigned_abs().to_string()),
    );
    push(&mut more, "色彩模式", Some(mode.into()));
    push(&mut more, "位元深度", Some(format!("{depth} bit")));
    push(&mut more, "透明度", layers.map(|layers| yes_no(layers < 0)));
    block("PSD", BLUE, vec![fields(more, 2)])
}

fn markdown_block(text: &str, _encoding: &str) -> Option<Block> {
    let (frontmatter, body) = frontmatter(text);
    let front_title = frontmatter.as_ref().and_then(|(_, front)| {
        front.lines().find_map(|line| {
            let value = line
                .trim()
                .strip_prefix("title")?
                .trim_start()
                .strip_prefix([':', '='])?;
            Some(value.trim().trim_matches(['"', '\'', ',']).to_string())
        })
    });
    let (mut headings, mut fences, mut links, mut images) = (0u64, 0u64, 0u64, 0u64);
    let mut first_heading = None;
    let mut in_code = false;
    for line in body.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fences += 1;
            in_code = !in_code;
            continue;
        }
        if in_code {
            continue;
        }
        if let Some(heading) = trimmed.strip_prefix('#') {
            let heading = heading.trim_start_matches('#');
            if heading.starts_with(' ') {
                headings += 1;
                if first_heading.is_none() && !trimmed.starts_with("##") {
                    first_heading = Some(heading.trim().to_string());
                }
            }
        }
        images += line.matches("![").count() as u64;
        links += line.matches("](").count() as u64;
    }
    let format = frontmatter.as_ref().map(|(format, _)| *format);
    block(
        "Markdown",
        MINT,
        vec![
            title(
                front_title.or(first_heading),
                15.0,
                None,
                Some(match format {
                    Some(format) => format!("{format} frontmatter"),
                    None => "沒有 frontmatter".into(),
                }),
            ),
            specs(specs_first_colored(vec![
                spec(Some(number(word_count(body))), "字數", false),
                spec(Some(number(headings)), "標題數", false),
                spec(Some(number(links.saturating_sub(images))), "連結數", false),
                spec(Some(number(images)), "圖片數", false),
                spec(Some(number(fences / 2)), "程式碼區塊", false),
            ])),
        ],
    )
}

/// The frontmatter at the start of a Markdown file, its format and text, and the rest.
fn frontmatter(text: &str) -> (Option<(&'static str, &str)>, &str) {
    for (fence, format) in [("---", "YAML"), ("+++", "TOML")] {
        if let Some(rest) = text.strip_prefix(fence).and_then(|rest| rest.strip_prefix('\n')) {
            if let Some(end) = rest.find(&format!("\n{fence}")) {
                let after = &rest[end + 1 + fence.len()..];
                return (Some((format, &rest[..end])), after);
            }
        }
    }
    if text.starts_with('{') {
        if let Some(end) = text.find("\n}") {
            return (Some(("JSON", &text[..end + 2])), &text[end + 2..]);
        }
    }
    (None, text)
}

/// Words as a word processor counts them: runs of letters, and each CJK character.
fn word_count(text: &str) -> u64 {
    let mut count = 0;
    let mut in_word = false;
    for character in text.chars() {
        let cjk = matches!(u32::from(character), 0x3040..=0x30ff | 0x3400..=0x4dbf | 0x4e00..=0x9fff | 0xac00..=0xd7af | 0xf900..=0xfaff);
        if cjk {
            count += 1;
            in_word = false;
        } else if character.is_alphanumeric() {
            if !in_word {
                count += 1;
            }
            in_word = true;
        } else {
            in_word = false;
        }
    }
    count
}

/// A tag's attributes, as name and value.
fn attributes(tag: &str) -> Vec<(String, String)> {
    let mut found = Vec::new();
    let mut rest = tag;
    while let Some(equals) = rest.find('=') {
        let name = rest[..equals]
            .split_whitespace()
            .last()
            .unwrap_or_default()
            .to_string();
        let after = rest[equals + 1..].trim_start();
        let (value, next) = match after.chars().next() {
            Some(quote @ ('"' | '\'')) => match after[1..].find(quote) {
                Some(end) => (&after[1..end + 1], &after[end + 2..]),
                None => (&after[1..], ""),
            },
            _ => {
                let end = after
                    .find(|character: char| character.is_whitespace() || character == '>')
                    .unwrap_or(after.len());
                (&after[..end], &after[end..])
            }
        };
        found.push((name.to_lowercase(), value.to_string()));
        rest = next;
    }
    found
}

fn value_of<'a>(attributes: &'a [(String, String)], name: &str) -> Option<&'a str> {
    attributes
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

fn html_block(text: &str, _encoding: &str) -> Option<Block> {
    let lower = text.to_lowercase();
    let title = lower.find("<title").and_then(|open| {
        let start = open + lower[open..].find('>')? + 1;
        let end = start + lower[start..].find("</title")?;
        Some(strip_tags(&text[start..end]))
    });
    let mut metas: Vec<Vec<(String, String)>> = Vec::new();
    for (open, _) in lower.match_indices("<meta") {
        if let Some(end) = text[open..].find('>') {
            metas.push(attributes(&text[open + 5..open + end]));
        }
    }
    let meta = |name: &str| -> Option<String> {
        metas.iter().find_map(|attributes| {
            let named = value_of(attributes, "name").or_else(|| value_of(attributes, "property"))?;
            named
                .eq_ignore_ascii_case(name)
                .then(|| value_of(attributes, "content").map(String::from))
                .flatten()
        })
    };
    let charset = metas
        .iter()
        .find_map(|attributes| value_of(attributes, "charset").map(String::from))
        .or_else(|| {
            metas.iter().find_map(|attributes| {
                let content = value_of(attributes, "content")?.to_lowercase();
                Some(content.split("charset=").nth(1)?.trim().to_uppercase())
            })
        });
    let language = lower.find("<html").and_then(|open| {
        let end = open + lower[open..].find('>')?;
        value_of(&attributes(&text[open + 5..end]), "lang").map(String::from)
    });
    let mut more = Vec::new();
    push_wide(&mut more, "標題", title);
    push_wide(&mut more, "描述", meta("description"));
    push(&mut more, "字元集", charset);
    push(&mut more, "語言", language);
    push_wide(&mut more, "OG 標題", meta("og:title"));
    push_wide(&mut more, "OG 描述", meta("og:description"));
    push_wide(&mut more, "OG 圖片", meta("og:image"));
    push(&mut more, "Twitter Card", meta("twitter:card"));
    push_wide(&mut more, "關鍵字", meta("keywords"));
    push(&mut more, "作者", meta("author"));
    block("HTML", ORANGE, vec![fields(more, 2)])
}

fn subtitle_block(text: &str, encoding: &str, ext: &str) -> Option<Block> {
    let ass = matches!(ext, "ass" | "ssa") || text.contains("[Script Info]");
    let (entries, last_end, formatted) = if ass {
        let dialogue: Vec<&str> = text
            .lines()
            .filter(|line| line.starts_with("Dialogue:"))
            .collect();
        let end = dialogue
            .last()
            .and_then(|line| line.split(',').nth(2))
            .map(str::to_string);
        (dialogue.len(), end, true)
    } else {
        let cues: Vec<&str> = text.lines().filter(|line| line.contains("-->")).collect();
        let end = cues
            .last()
            .and_then(|line| line.split("-->").nth(1))
            .and_then(|end| end.split_whitespace().next())
            .map(str::to_string);
        let formatted = ["<i>", "<b>", "<u>", "<font", "{\\"]
            .iter()
            .any(|mark| text.contains(mark));
        (cues.len(), end, formatted)
    };
    let duration = last_end.and_then(|end| {
        let parts: Vec<f64> = end
            .replace(',', ".")
            .split(':')
            .filter_map(|part| part.trim().parse().ok())
            .collect();
        match parts[..] {
            [hours, minutes, seconds] => Some(hours * 3600.0 + minutes * 60.0 + seconds),
            [minutes, seconds] => Some(minutes * 60.0 + seconds),
            _ => None,
        }
    });
    let mut more = Vec::new();
    push(
        &mut more,
        "格式",
        Some(
            match ext {
                "vtt" => "WebVTT",
                "ass" => "ASS",
                "ssa" => "SSA",
                _ if ass => "ASS",
                _ => "SubRip",
            }
            .into(),
        ),
    );
    push(&mut more, "編碼", Some(encoding.into()));
    push(&mut more, "條目數", Some(number(entries as u64)));
    push(&mut more, "時長", duration.map(duration_text));
    push(
        &mut more,
        "語言",
        text.lines().take(20).find_map(|line| {
            line.strip_prefix("Language:")
                .map(|language| language.trim().to_string())
        }),
    );
    push(&mut more, "格式化", Some(yes_no(formatted)));
    block("字幕", GREEN, vec![fields(more, 2)])
}

fn config_block(text: &str, encoding: &str, ext: &str) -> Option<Block> {
    fn json_shape(value: &Value) -> (u64, u64) {
        match value {
            Value::Object(map) => map.values().fold((map.len() as u64, 1), |(keys, depth), value| {
                let (inner_keys, inner_depth) = json_shape(value);
                (keys + inner_keys, depth.max(inner_depth + 1))
            }),
            Value::Array(items) => items.iter().fold((0, 1), |(keys, depth), value| {
                let (inner_keys, inner_depth) = json_shape(value);
                (keys + inner_keys, depth.max(inner_depth + 1))
            }),
            _ => (0, 0),
        }
    }
    fn toml_shape(value: &toml::Value) -> (u64, u64) {
        match value {
            toml::Value::Table(table) => {
                table
                    .values()
                    .fold((table.len() as u64, 1), |(keys, depth), value| {
                        let (inner_keys, inner_depth) = toml_shape(value);
                        (keys + inner_keys, depth.max(inner_depth + 1))
                    })
            }
            toml::Value::Array(items) => items.iter().fold((0, 1), |(keys, depth), value| {
                let (inner_keys, inner_depth) = toml_shape(value);
                (keys + inner_keys, depth.max(inner_depth + 1))
            }),
            _ => (0, 0),
        }
    }
    let commented = |marks: &[&str]| {
        text.lines()
            .any(|line| marks.iter().any(|mark| line.trim_start().starts_with(mark)))
    };
    let (format, valid, shape, comments) = match ext {
        "json" => {
            let parsed: Result<Value, _> = serde_json::from_str(text);
            (
                "JSON",
                Some(parsed.is_ok()),
                parsed.ok().map(|value| json_shape(&value)),
                false,
            )
        }
        "toml" => {
            let parsed: Result<toml::Value, _> = text.parse();
            (
                "TOML",
                Some(parsed.is_ok()),
                parsed.ok().map(|value| toml_shape(&value)),
                commented(&["#"]),
            )
        }
        "yaml" | "yml" => {
            // shortcut: YAML is counted line by line, not parsed; add a parser if its
            // keys or validity turn out to matter.
            let keys = text
                .lines()
                .filter(|line| {
                    let line = line.trim_start().trim_start_matches("- ");
                    !line.starts_with('#')
                        && line
                            .split_once(':')
                            .is_some_and(|(key, _)| !key.is_empty() && !key.contains(' '))
                })
                .count() as u64;
            let depth = text
                .lines()
                .filter(|line| !line.trim().is_empty() && !line.trim_start().starts_with('#'))
                .map(|line| (line.len() - line.trim_start().len()) as u64 / 2 + 1)
                .max()
                .unwrap_or(0);
            ("YAML", None, Some((keys, depth)), commented(&["#"]))
        }
        _ => {
            let keys = text
                .lines()
                .filter(|line| !line.trim_start().starts_with(['#', ';']) && line.contains('='))
                .count() as u64;
            let sections = text.lines().any(|line| line.trim_start().starts_with('['));
            (
                "INI",
                None,
                Some((keys, 1 + u64::from(sections))),
                commented(&["#", ";"]),
            )
        }
    };
    let mut more = Vec::new();
    push(&mut more, "格式", Some(format.into()));
    push(&mut more, "有效", valid.map(yes_no));
    push(&mut more, "鍵值數", shape.map(|(keys, _)| number(keys)));
    push(&mut more, "深度", shape.map(|(_, depth)| depth.to_string()));
    push(&mut more, "註解", (format != "JSON").then(|| yes_no(comments)));
    push(&mut more, "編碼", Some(encoding.into()));
    block("設定檔", ORANGE, vec![fields(more, 2)])
}

/// The language a source file is in, by its name, with how it starts a comment line and
/// a comment block.
struct Language {
    name: &'static str,
    line: &'static [&'static str],
    block: Option<(&'static str, &'static str)>,
}

fn language(path: &Path) -> Option<Language> {
    const C: Option<(&str, &str)> = Some(("/*", "*/"));
    let file_name = path.file_name()?.to_string_lossy().to_lowercase();
    let ext = extension(path);
    let (name, line, block): (&str, &[&str], _) = match (file_name.as_str(), ext.as_str()) {
        ("makefile" | "gnumakefile", _) | (_, "mk") => ("Makefile", &["#"], None),
        ("dockerfile", _) | (_, "dockerfile") => ("Dockerfile", &["#"], None),
        ("cmakelists.txt", _) | (_, "cmake") => ("CMake", &["#"], None),
        (_, "rs") => ("Rust", &["//"], C),
        (_, "c" | "h") => ("C", &["//"], C),
        (_, "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx") => ("C++", &["//"], C),
        (_, "cs") => ("C#", &["//"], C),
        (_, "java") => ("Java", &["//"], C),
        (_, "kt" | "kts") => ("Kotlin", &["//"], C),
        (_, "swift") => ("Swift", &["//"], C),
        (_, "go") => ("Go", &["//"], C),
        (_, "js" | "mjs" | "cjs") => ("JavaScript", &["//"], C),
        (_, "jsx") => ("JSX", &["//"], C),
        (_, "ts" | "mts" | "cts") => ("TypeScript", &["//"], C),
        (_, "tsx") => ("TSX", &["//"], C),
        (_, "py" | "pyw") => ("Python", &["#"], None),
        (_, "rb") => ("Ruby", &["#"], None),
        (_, "php") => ("PHP", &["//", "#"], C),
        (_, "sh" | "bash" | "zsh" | "ksh") => ("Shell", &["#"], None),
        (_, "fish") => ("Fish", &["#"], None),
        (_, "pl" | "pm") => ("Perl", &["#"], None),
        (_, "lua") => ("Lua", &["--"], Some(("--[[", "]]"))),
        (_, "r") => ("R", &["#"], None),
        (_, "scala" | "sc") => ("Scala", &["//"], C),
        (_, "dart") => ("Dart", &["//"], C),
        (_, "m" | "mm") => ("Objective-C", &["//"], C),
        (_, "sql") => ("SQL", &["--"], C),
        (_, "hs") => ("Haskell", &["--"], Some(("{-", "-}"))),
        (_, "ex" | "exs") => ("Elixir", &["#"], None),
        (_, "erl" | "hrl") => ("Erlang", &["%"], None),
        (_, "clj" | "cljs" | "edn") => ("Clojure", &[";"], None),
        (_, "el" | "lisp" | "scm") => ("Lisp", &[";"], None),
        (_, "vim") => ("Vim script", &["\""], None),
        (_, "css") => ("CSS", &[], C),
        (_, "scss" | "sass" | "less") => ("SCSS", &["//"], C),
        (_, "vue") => ("Vue", &["//"], Some(("<!--", "-->"))),
        (_, "svelte") => ("Svelte", &["//"], Some(("<!--", "-->"))),
        (_, "zig") => ("Zig", &["//"], None),
        (_, "nim") => ("Nim", &["#"], Some(("#[", "]#"))),
        (_, "ml" | "mli") => ("OCaml", &[], Some(("(*", "*)"))),
        (_, "fs" | "fsx") => ("F#", &["//"], Some(("(*", "*)"))),
        (_, "jl") => ("Julia", &["#"], Some(("#=", "=#"))),
        (_, "ps1" | "psm1") => ("PowerShell", &["#"], Some(("<#", "#>"))),
        (_, "groovy" | "gradle") => ("Groovy", &["//"], C),
        (_, "asm" | "s") => ("Assembly", &[";", "#"], None),
        (_, "v" | "sv") => ("Verilog", &["//"], C),
        (_, "vhd" | "vhdl") => ("VHDL", &["--"], None),
        (_, "tex" | "sty") => ("LaTeX", &["%"], None),
        (_, "gd") => ("GDScript", &["#"], None),
        (_, "proto") => ("Protocol Buffers", &["//"], C),
        (_, "graphql" | "gql") => ("GraphQL", &["#"], None),
        (_, "xml" | "xsd" | "xsl") => ("XML", &[], Some(("<!--", "-->"))),
        (_, "slint") => ("Slint", &["//"], C),
        (_, "glsl" | "vert" | "frag" | "wgsl") => ("Shader", &["//"], C),
        _ => return None,
    };
    Some(Language { name, line, block })
}

/// Lines of code, comments and blank lines, with the language and encoding.
fn code_block(text: &str, encoding: &str, language: &Language) -> Option<Block> {
    let (mut code, mut comments, mut blank) = (0u64, 0u64, 0u64);
    let mut in_block: Option<&str> = None;
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(end) = in_block {
            comments += 1;
            if trimmed.contains(end) {
                in_block = None;
            }
        } else if trimmed.is_empty() {
            blank += 1;
        } else if language.line.iter().any(|mark| trimmed.starts_with(mark)) {
            comments += 1;
        } else if let Some((start, end)) = language.block.filter(|(start, _)| trimmed.starts_with(start)) {
            comments += 1;
            if !trimmed[start.len()..].contains(end) {
                in_block = Some(end);
            }
        } else {
            code += 1;
        }
    }
    let total = code + comments + blank;
    block(
        "程式碼",
        CYAN,
        vec![
            Some(Part::Hero {
                value: Some(number(total)),
                unit: Some("行".into()),
                trailing: Some(format!("{} · {encoding}", language.name)),
                small: false,
            }),
            (total > 0).then(|| {
                Part::Segments(vec![
                    (code as f64, Shade::Accent),
                    (comments as f64, Shade::Fill),
                    (blank as f64, Shade::Track),
                ])
            }),
            fields(
                vec![
                    Field {
                        label: "程式碼".into(),
                        value: number(code),
                        wide: false,
                    },
                    Field {
                        label: "註解".into(),
                        value: number(comments),
                        wide: false,
                    },
                    Field {
                        label: "空白".into(),
                        value: number(blank),
                        wide: false,
                    },
                ],
                3,
            ),
        ],
    )
}

/// A read-only git command in `folder`, with what would start programs off and no lock
/// taken.
fn git(folder: &Path, args: &[&str]) -> Option<String> {
    let mut all: Vec<&OsStr> = [
        "--no-optional-locks",
        "-c",
        "core.fsmonitor=false",
        "-c",
        "log.showSignature=false",
        "-C",
    ]
    .map(OsStr::new)
    .to_vec();
    all.push(folder.as_os_str());
    all.extend(args.iter().map(OsStr::new));
    run("git", &all)
}

/// A repository's branch, whether it has uncommitted changes, its commits and remote.
fn git_block(folder: &Path) -> Option<Block> {
    if !folder.join(".git").exists() {
        return None;
    }
    // The repository's own settings are someone else's: they can name programs `status`
    // starts, as a file system monitor or a filter. Without them, status is not asked.
    let config = git(folder, &["config", "--list", "--show-scope", "--includes"]);
    let entries: Vec<(String, String, String)> = config
        .as_deref()
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let (scope, pair) = line.split_once('\t')?;
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            Some((scope.to_string(), key.to_lowercase(), value.to_string()))
        })
        .collect();
    let starts_programs = config.is_none()
        || entries.iter().any(|(scope, key, _)| {
            (scope == "local" || scope == "worktree")
                && (key.starts_with("filter.") || key == "core.fsmonitor")
        });
    let remote = entries
        .iter()
        .rev()
        .find(|(_, key, _)| key == "remote.origin.url")
        .map(|(_, _, value)| value.clone());
    let (mut branch, mut changed) = (None, None);
    if let Some(status) = (!starts_programs)
        .then(|| {
            git(
                folder,
                &["status", "--porcelain=v2", "--branch", "--ignore-submodules"],
            )
        })
        .flatten()
    {
        branch = status
            .lines()
            .find_map(|line| line.strip_prefix("# branch.head "))
            .filter(|head| *head != "(detached)")
            .map(String::from);
        changed = Some(status.lines().any(|line| !line.starts_with('#')));
    }
    branch = branch.or_else(|| {
        git(folder, &["branch", "--show-current"])
            .map(|branch| branch.trim().to_string())
            .filter(|branch| !branch.is_empty())
    });
    let refs = git(folder, &["for-each-ref", "--format=%(refname)"]).unwrap_or_default();
    let branches = refs
        .lines()
        .filter(|name| name.starts_with("refs/heads/") || name.starts_with("refs/remotes/"))
        .count();
    let tags = refs.lines().filter(|name| name.starts_with("refs/tags/")).count();
    let commits =
        git(folder, &["rev-list", "--count", "HEAD"]).and_then(|count| count.trim().parse::<u64>().ok());
    let last = git(folder, &["log", "-1", "--format=%ci|%s"]).and_then(|log| {
        let (date, message) = log.trim().split_once('|')?;
        Some(Field {
            label: format!("最後提交 · {}", date_text(date)),
            value: message.to_string(),
            wide: true,
        })
    });
    let mut remote_field = Vec::new();
    push_wide(&mut remote_field, "遠端", remote);
    block(
        "Git",
        ORANGE,
        vec![
            (branch.is_some() || changed.is_some()).then(|| Part::Hero {
                value: branch,
                unit: None,
                trailing: changed.map(|changed| {
                    if changed {
                        "有未提交的變更"
                    } else {
                        "沒有未提交的變更"
                    }
                    .into()
                }),
                small: true,
            }),
            last.map(|last| Part::Fields(vec![last], 1)),
            specs(
                [
                    spec(commits.map(number), "提交數", false),
                    spec(Some(number(branches as u64)), "分支數", false),
                    spec(Some(number(tags as u64)), "標籤數", false),
                ]
                .into_iter()
                .flatten()
                .collect(),
            ),
            fields(remote_field, 1),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_lines_are_told_apart_from_comments_and_blank_ones() {
        let rust = language(Path::new("main.rs")).unwrap();
        let block = code_block("// a\nfn main() {}\n\n/* b\n c */\nlet x = 1;\n", "UTF-8", &rust).unwrap();
        let fields = block.parts.iter().find_map(|part| match part {
            Part::Fields(fields, 3) => Some(fields.clone()),
            _ => None,
        });
        let values: Vec<String> = fields.unwrap().into_iter().map(|field| field.value).collect();
        assert_eq!(values, ["2", "3", "1"]);
    }

    #[test]
    fn markdown_takes_its_title_from_frontmatter_before_its_heading() {
        let block = markdown_block(
            "---\ntitle: \"行程\"\n---\n# 別的\n\n見 [這裡](x) 與 ![圖](y)\n",
            "UTF-8",
        )
        .unwrap();
        let Part::Title { title, subtitle, .. } = &block.parts[0] else {
            panic!("a title first");
        };
        assert_eq!(title.as_deref(), Some("行程"));
        assert_eq!(subtitle.as_deref(), Some("YAML frontmatter"));
        let Part::Specs(specs) = &block.parts[1] else {
            panic!("figures second");
        };
        let links = specs.iter().find(|spec| spec.label == "連結數").unwrap();
        assert_eq!(links.value, "1");
    }

    #[test]
    fn words_count_each_cjk_character_and_each_run_of_letters() {
        assert_eq!(word_count("新潟 fireworks, 2026"), 4);
    }

    #[test]
    fn dates_read_in_exif_and_iso_forms() {
        assert_eq!(date_text("2026:09:03 14:02:00"), "2026/9/3 14:02");
        assert_eq!(date_text("2026-09-03T14:02:00+08"), "2026/9/3 14:02");
        assert_eq!(duration_text(768.0), "12:48");
        assert_eq!(number(1_234_567u64), "1,234,567");
    }

    #[test]
    fn attributes_take_quoted_and_bare_values() {
        let found = attributes(r#" name="description" content='a "b"' data=x>"#);
        assert_eq!(value_of(&found, "content"), Some("a \"b\""));
        assert_eq!(value_of(&found, "data"), Some("x"));
    }

    #[test]
    fn json_config_counts_nested_keys_and_depth() {
        let block = config_block(r#"{"a": {"b": 1, "c": [ {"d": 2} ]}}"#, "ASCII", "json").unwrap();
        let Part::Fields(fields, _) = &block.parts[0] else {
            panic!("fields");
        };
        let value = |label: &str| {
            fields
                .iter()
                .find(|field| field.label == label)
                .unwrap()
                .value
                .clone()
        };
        assert_eq!(value("鍵值數"), "4");
        assert_eq!(value("深度"), "4");
        assert_eq!(value("有效"), "是");
    }

    #[test]
    fn xcode_project_lists_targets_and_settings() {
        let project = std::env::temp_dir().join(format!("nekoland-{}.xcodeproj", std::process::id()));
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(
            project.join("project.pbxproj"),
            "isa = PBXNativeTarget; productType = \"com.apple.product-type.bundle.unit-test\";\n\
             isa = PBXNativeTarget;\n isa = XCBuildConfiguration;\n\
             SWIFT_VERSION = 5.0;\n MACOSX_DEPLOYMENT_TARGET = \"14.0\";\n",
        )
        .unwrap();
        let block = xcode_block(&project);
        std::fs::remove_dir_all(&project).unwrap();
        let block = block.unwrap();
        let Part::Fields(fields, _) = &block.parts[0] else {
            panic!("fields");
        };
        let shown: Vec<(&str, &str)> = fields
            .iter()
            .map(|field| (field.label.as_str(), field.value.as_str()))
            .collect();
        assert_eq!(
            shown,
            [
                ("Target 數", "2"),
                ("Swift 版本", "5.0"),
                ("部署目標", "macOS 14.0"),
                ("配置數", "1"),
                ("單元測試", "是"),
            ]
        );
    }

    #[test]
    fn tags_in_a_language_are_found_by_their_plain_name() {
        let tags: serde_json::Map<String, Value> =
            serde_json::from_str(r#"{"FontFamily-en-US": "Ubuntu", "FontNameID": "x", "Title": " Plain "}"#)
                .unwrap();
        assert_eq!(tag(&tags, &["FontFamily"]).as_deref(), Some("Ubuntu"));
        assert_eq!(tag(&tags, &["FontName"]), None);
        assert_eq!(tag(&tags, &["Title"]).as_deref(), Some("Plain"));
    }
}
