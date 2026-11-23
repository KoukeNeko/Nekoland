//! `nekoland-preview`, 預覽程式: pictures and PDFs, one or several, in a window of their own.
//! A sidebar of thumbnails goes through the files, or a PDF's pages; the picture fits the
//! window or is zoomed (Ctrl+scroll, pinch, or the buttons) and dragged. A picture can be
//! turned or flipped and saved. A PDF's pages are drawn by poppler, one under another, at
//! whatever the zoom is; its text is selected by dragging over it and found with 搜尋.
//! Every edge is `EDGE` and every gap `GAP`, as in Files.

use std::{
    cell::{Cell, RefCell},
    collections::{hash_map::DefaultHasher, HashMap, HashSet},
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    process::Command,
    rc::Rc,
};

use adw::prelude::*;
use gtk::gdk_pixbuf::{Pixbuf, PixbufRotation};
use gtk::{gdk, gio, glib, pango};

mod alert;
#[path = "preview/annots.rs"]
mod annots;
mod browse;
#[path = "preview/document.rs"]
mod document;
#[path = "preview/edit.rs"]
mod edit;
#[path = "preview/imaging.rs"]
mod imaging;
#[path = "preview/inspector.rs"]
mod inspector;
#[path = "preview/markup.rs"]
mod markup;
#[path = "preview/menus.rs"]
mod menus;
#[path = "preview/pdfdoc.rs"]
mod pdfdoc;
#[path = "preview/prefs.rs"]
mod prefs;
#[path = "preview/subject.rs"]
mod subject;
#[path = "preview/printing.rs"]
mod printing;
#[path = "preview/settings.rs"]
mod settings;
#[path = "preview/shapes.rs"]
mod shapes;
#[path = "preview/view.rs"]
mod view;

use browse::{modified_label, EDGE, GAP};
use prefs::{Layout, Prefs};
use shapes::{Shape, Style, Tool};

const APP_ID: &str = "io.github.koukeneko.Nekoland.Preview";
const THUMB_SIZE: i32 = 160;
const MIN_SCALE: f64 = 0.05;
const MAX_SCALE: f64 = 16.0;
const ZOOM_STEP: f64 = 1.25;
/// How far two fingers go across the touchpad, in pixels, to turn a page.
const SWIPE_DISTANCE: f64 = 150.0;
/// What the Mac's Preview opens: PDFs and pictures, camera RAW and the less common ones too.
/// Which of them can be read depends on the image loaders installed.
const MIME_TYPES: [&str; 23] = [
    "application/pdf",
    "image/png",
    "image/jpeg",
    "image/gif",
    "image/bmp",
    "image/tiff",
    "image/webp",
    "image/heic",
    "image/heif",
    "image/avif",
    "image/jp2",
    "image/x-exr",
    "image/x-icon",
    "image/vnd.microsoft.icon",
    "image/vnd.adobe.photoshop",
    "image/x-dcraw",
    "image/x-adobe-dng",
    "image/x-tga",
    "image/vnd.radiance",
    "image/x-sgi",
    "image/x-xbitmap",
    "image/x-pict",
    "application/illustrator",
];

#[derive(Clone, Copy, PartialEq)]
enum Zoom {
    Fit,
    /// How many screen pixels one pixel of the picture takes.
    Scale(f64),
}

/// One thing the window can show: a picture, or one page of a PDF.
struct Item {
    path: PathBuf,
    /// 1-based, for a page of a PDF.
    page: Option<u32>,
    /// The picture as turned or flipped, until it is saved.
    edited: RefCell<Option<Pixbuf>>,
    /// What `edited` was before each change, newest last, for 還原.
    undo: RefCell<Vec<Option<Pixbuf>>>,
    /// What 還原 took back, newest last, for 重做.
    redo: RefCell<Vec<Option<Pixbuf>>>,
    /// A picture made from the clipboard or a copy of one, saved nowhere yet; `path` is then
    /// only the name it would have.
    untitled: bool,
    /// Where it is saved instead of `path`: a PNG, once its background is taken out of a
    /// format that has no clear pixels.
    save_to: RefCell<Option<PathBuf>>,
    /// The picture's dots to the inch, as 調整大小 sets them when it does not resample.
    dpi: Cell<Option<f64>>,
    /// The colour profile 指定描述檔 gave it, written into it when it is saved.
    profile: RefCell<Option<PathBuf>>,
}

impl Item {
    fn new(path: PathBuf, page: Option<u32>) -> Rc<Item> {
        Rc::new(Item {
            path,
            page,
            edited: RefCell::new(None),
            undo: RefCell::default(),
            redo: RefCell::default(),
            untitled: false,
            save_to: RefCell::new(None),
            dpi: Cell::new(None),
            profile: RefCell::new(None),
        })
    }

    /// A picture saved nowhere yet, showing `pixbuf`.
    fn untitled(name: &str, pixbuf: Pixbuf) -> Rc<Item> {
        Rc::new(Item {
            path: PathBuf::from(name),
            page: None,
            edited: RefCell::new(Some(pixbuf)),
            undo: RefCell::default(),
            redo: RefCell::default(),
            untitled: true,
            save_to: RefCell::new(None),
            dpi: Cell::new(None),
            profile: RefCell::new(None),
        })
    }

    /// A page of a copy of a PDF, saved nowhere yet: `path` is the copy, kept with the run's.
    fn copied(path: PathBuf, page: u32) -> Rc<Item> {
        Rc::new(Item {
            path,
            page: Some(page),
            edited: RefCell::new(None),
            undo: RefCell::default(),
            redo: RefCell::default(),
            untitled: true,
            save_to: RefCell::new(None),
            dpi: Cell::new(None),
            profile: RefCell::new(None),
        })
    }

    /// Where saving it writes.
    fn target(&self) -> PathBuf {
        self.save_to.borrow().clone().unwrap_or_else(|| self.path.clone())
    }

    fn is_page(&self) -> bool {
        self.page.is_some()
    }

    fn name(&self) -> String {
        self.path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

/// The PDF the window shows: its pages one under another, or one or two at a time, drawn by
/// poppler as the zoom asks.
struct Pdf {
    path: PathBuf,
    /// Read again when a change is taken back.
    document: RefCell<poppler::Document>,
    password: Option<String>,
    pages: Vec<gtk::DrawingArea>,
    /// Each page's size in points.
    sizes: Vec<(f64, f64)>,
    /// The screen pixels to a point the pages are laid out at; 0 before they are.
    scale: Cell<f64>,
    /// Pages left blank while out of view, to be drawn when scrolled to.
    undrawn: RefCell<HashSet<usize>>,
    layout: Cell<Layout>,
    /// The first page shown, a page at a time or two.
    spread: Cell<usize>,
    /// Whether it has annotations or form entries not yet saved.
    dirty: Cell<bool>,
    /// The PDF as it was before each change, newest last, and after each taken back.
    undo: RefCell<Vec<PathBuf>>,
    redo: RefCell<Vec<PathBuf>>,
}

impl Pdf {
    fn document(&self) -> poppler::Document {
        self.document.borrow().clone()
    }

    /// The pages laid out side by side, each row under the one before.
    fn rows(&self) -> Vec<Vec<usize>> {
        let count = self.sizes.len();
        match self.layout.get() {
            Layout::Continuous => (0..count).map(|index| vec![index]).collect(),
            Layout::Single => vec![vec![self.spread.get().min(count.saturating_sub(1))]],
            Layout::Two => {
                let first = self.spread.get();
                vec![(first..(first + 2).min(count)).collect()]
            }
        }
    }

    /// The first page of the spread page `index` is in, a page or two at a time.
    fn spread_of(&self, index: usize) -> usize {
        match self.layout.get() {
            Layout::Two => index - index % 2,
            _ => index,
        }
    }

    fn row_height(&self, row: &[usize], scale: f64) -> f64 {
        row.iter().map(|&index| self.height(index, scale)).fold(0.0, f64::max)
    }

    /// How far down the column page `index` starts at `scale`, with an `EDGE` around the pages
    /// and between them; past the last page, the column's whole height.
    fn top(&self, index: usize, scale: f64) -> f64 {
        let mut top = f64::from(EDGE);
        for row in self.rows() {
            if row.contains(&index) {
                return top;
            }
            top += self.row_height(&row, scale) + f64::from(EDGE);
        }
        if index >= self.sizes.len() {
            top
        } else {
            f64::from(EDGE)
        }
    }

    fn height(&self, index: usize, scale: f64) -> f64 {
        (self.sizes[index].1 * scale).round()
    }

    /// The page `y` down the column is on, or the one above the gap it is in.
    fn page_at(&self, y: f64, scale: f64) -> usize {
        let mut top = f64::from(EDGE);
        let rows = self.rows();
        for row in &rows {
            top += self.row_height(row, scale) + f64::from(EDGE);
            if y < top {
                return row[0];
            }
        }
        rows.last().map_or(0, |row| row[0])
    }
}

/// Text chosen in a PDF, from one place to another, each a page and a point on it from its
/// top left; by the character, or the word or line a double or triple click is on.
#[derive(Clone, Copy)]
struct Selection {
    from: (usize, f64, f64),
    to: (usize, f64, f64),
    style: poppler::SelectionStyle,
}

impl Selection {
    fn pages(&self) -> std::ops::RangeInclusive<usize> {
        self.from.0.min(self.to.0)..=self.from.0.max(self.to.0)
    }

    /// The part of page `index` chosen, as poppler takes it, when any is.
    fn on(&self, index: usize, size: (f64, f64)) -> Option<poppler::Rectangle> {
        let (first, last) = if self.from.0 <= self.to.0 { (self.from, self.to) } else { (self.to, self.from) };
        if !self.pages().contains(&index) {
            return None;
        }
        let (x1, y1) = if index == first.0 { (first.1, first.2) } else { (0.0, 0.0) };
        let (x2, y2) = if index == last.0 { (last.1, last.2) } else { size };
        Some(rectangle(x1, y1, x2, y2))
    }
}

fn rectangle(x1: f64, y1: f64, x2: f64, y2: f64) -> poppler::Rectangle {
    let mut rectangle = poppler::Rectangle::new();
    rectangle.set_x1(x1);
    rectangle.set_y1(y1);
    rectangle.set_x2(x2);
    rectangle.set_y2(y2);
    rectangle
}

/// The text `selection` takes in `document`, a line break between pages.
fn selected_text(document: &poppler::Document, selection: &Selection) -> String {
    selection
        .pages()
        .filter_map(|index| {
            let page = document.page(index as i32)?;
            let mut area = selection.on(index, page.size())?;
            Some(page.selected_text(selection.style, &mut area)?.to_string())
        })
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// A place on a page, its left, top, right and bottom from the page's top left, in points.
type Place = (f64, f64, f64, f64);

/// Where `text` is on `page`.
fn page_matches(page: &poppler::Page, text: &str) -> Vec<Place> {
    let height = page.size().1;
    // poppler measures what it finds up from the foot of the page.
    page.find_text_with_options(text, poppler::FindFlags::DEFAULT)
        .iter()
        .map(|found| (found.x1(), height - found.y2(), found.x2(), height - found.y1()))
        .collect()
}

/// Whether the file at `path` is locked, as 鎖定 leaves it: not writable.
fn is_locked(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|meta| meta.permissions().readonly())
}

/// Matches in reading order, each place once; by rank, the pages with the most come first.
fn order_matches(mut found: Vec<(usize, Place)>, by_rank: bool) -> Vec<(usize, Place)> {
    found.sort_by(|(a, (ax, ay, ..)), (b, (bx, by, ..))| a.cmp(b).then(ay.total_cmp(by)).then(ax.total_cmp(bx)));
    found.dedup_by(|a, b| a == b);
    if by_rank {
        let mut counts = std::collections::HashMap::new();
        for (page, _) in &found {
            *counts.entry(*page).or_insert(0usize) += 1;
        }
        // Stable, so a page keeps its matches together and in order, and ties keep page order.
        found.sort_by_key(|(page, _)| std::cmp::Reverse(counts[page]));
    }
    found
}

/// `page` drawn on white at `scale` pixels to the point.
fn page_texture(page: &poppler::Page, scale: f64) -> Option<gdk::Texture> {
    use gtk::cairo::{Context, Format, ImageSurface};
    let (width, height) = page.size();
    let (width, height) = (((width * scale).round() as i32).max(1), ((height * scale).round() as i32).max(1));
    let mut surface = ImageSurface::create(Format::ARgb32, width, height).ok()?;
    {
        let cr = Context::new(&surface).ok()?;
        cr.set_source_rgb(1.0, 1.0, 1.0);
        cr.paint().ok()?;
        cr.scale(scale, scale);
        page.render(&cr);
    }
    surface.flush();
    let stride = surface.stride() as usize;
    let bytes = glib::Bytes::from(&surface.data().ok()?[..]);
    // Cairo's ARGB32 is a word to a pixel, which is BGRA in memory on the little-endian
    // processors Nekoland runs on.
    let texture = gdk::MemoryTexture::new(width, height, gdk::MemoryFormat::B8g8r8a8Premultiplied, &bytes, stride);
    Some(texture.upcast())
}

/// Which list the sidebar shows, as 顯示方式 chooses.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Sidebar {
    Hidden,
    Thumbnails,
    Contents,
    Notes,
    Bookmarks,
    ContactSheet,
}

impl Sidebar {
    const ALL: [(Sidebar, &'static str); 6] = [
        (Sidebar::Hidden, "hidden"),
        (Sidebar::Thumbnails, "thumbnails"),
        (Sidebar::Contents, "contents"),
        (Sidebar::Notes, "notes"),
        (Sidebar::Bookmarks, "bookmarks"),
        (Sidebar::ContactSheet, "sheet"),
    ];

    fn key(self) -> &'static str {
        Sidebar::ALL.iter().find(|(mode, _)| *mode == self).map_or("thumbnails", |(_, key)| key)
    }

    fn from_key(key: &str) -> Sidebar {
        Sidebar::ALL.iter().find(|(_, name)| *name == key).map_or(Sidebar::Thumbnails, |(mode, _)| *mode)
    }
}

/// Text marked on a PDF with 標示重點: how, and in which colour.
#[derive(Clone, Copy, PartialEq, Debug)]
struct Marking {
    kind: annots::Kind,
    colour: (f64, f64, f64),
}

/// The colours 標示重點 offers, as the Mac's has them.
const MARK_COLOURS: [((f64, f64, f64), &str); 5] = [
    ((1.0, 0.86, 0.2), "黃色"),
    ((0.45, 0.85, 0.35), "綠色"),
    ((0.35, 0.65, 1.0), "藍色"),
    ((1.0, 0.45, 0.65), "粉紅色"),
    ((0.7, 0.45, 0.95), "紫色"),
];

struct Win {
    app: adw::Application,
    window: adw::ApplicationWindow,
    split: adw::OverlaySplitView,
    sidebar_button: adw::SplitButton,
    title: adw::WindowTitle,
    stack: gtk::Stack,
    scroll: gtk::ScrolledWindow,
    picture: gtk::Picture,
    /// Over the picture, where marks and selections are drawn while they are made.
    mark_layer: gtk::DrawingArea,
    content: adw::ToolbarView,
    /// The markup bar, under the header, and the buttons that only pictures or PDFs have.
    markup_bar: gtk::Revealer,
    picture_tools: Vec<gtk::Widget>,
    pdf_tools: Vec<gtk::Widget>,
    highlight_button: adw::SplitButton,
    /// The tool the markup bar has chosen; none while it is hidden.
    tool: Cell<Option<Tool>>,
    shape: Cell<Shape>,
    /// The signature 簽名 puts, by its place in `prefs.signatures`.
    signature: Cell<Option<usize>>,
    signature_menu: gio::Menu,
    color: gtk::ColorDialogButton,
    fill: gtk::ColorDialogButton,
    filled: Cell<bool>,
    width: Cell<f64>,
    font: gtk::FontDialogButton,
    /// What is being drawn: the points the pointer went through, in the picture's pixels or
    /// the page's points, and for a PDF the page.
    stroke: RefCell<Vec<(f64, f64)>>,
    stroke_page: Cell<Option<usize>>,
    /// The part of a picture chosen, and its outline to draw, in the picture's pixels.
    image_mask: RefCell<Option<imaging::Mask>>,
    image_outline: RefCell<Vec<(f64, f64)>>,
    /// `image_mask` as drawn over the picture, made once when it is set.
    mask_overlay: RefCell<Option<Pixbuf>>,
    list: gtk::ListBox,
    sidebar_stack: gtk::Stack,
    contents: gtk::ListBox,
    notes: gtk::ListBox,
    bookmarks: gtk::ListBox,
    sheet: gtk::FlowBox,
    sidebar_mode: Cell<Sidebar>,
    zoom_button: gtk::Button,
    items: RefCell<Vec<Rc<Item>>>,
    thumbnails: RefCell<Vec<gtk::Picture>>,
    current: Cell<usize>,
    zoom: Cell<Zoom>,
    /// The picture shown; a PDF's pages are in `pdf` instead.
    shown: RefCell<Option<Pixbuf>>,
    syncing: Cell<bool>,
    /// Set once the window may close, its changes saved or let go.
    closing: Cell<bool>,
    pdf_scroll: gtk::ScrolledWindow,
    pdf_column: gtk::Box,
    pdf: RefCell<Option<Rc<Pdf>>>,
    selection: Cell<Option<Selection>>,
    /// Where to scroll once the column is laid out as tall as its pages: a page, how much of
    /// its height down it, and how many pixels more.
    pending: Cell<Option<(usize, f64, f64)>>,
    /// Set while the window scrolls the column itself, which does not choose a page again.
    jumping: Cell<bool>,
    /// Set while the zoom waits to fit the window's new size.
    fitting: Cell<bool>,
    find_box: gtk::Box,
    find_entry: gtk::SearchEntry,
    find_count: gtk::Label,
    /// What 搜尋 found: each a page and a place on it.
    matches: RefCell<Vec<(usize, Place)>>,
    match_at: Cell<Option<usize>>,
    /// Which search is the latest, so one still going stops when the text changes.
    finding: Cell<u32>,
    /// Whether text is being typed, which then takes the keys the window's actions would.
    typing: Cell<bool>,
    /// The passwords the PDFs that need one were opened with.
    passwords: RefCell<HashMap<PathBuf, String>>,
    prefs: Rc<RefCell<Prefs>>,
    /// How a PDF opened next is laid out.
    layout: Cell<Layout>,
    /// The places gone to before, for 上一步, and those gone back from, for 下一步: an item and
    /// how far down its page.
    back: RefCell<Vec<(usize, f64)>>,
    forward: RefCell<Vec<(usize, f64)>>,
    /// Text a PDF is marked with as it is chosen, while 標示重點 is on.
    marking: Cell<Option<Marking>>,
    last_marking: Cell<Marking>,
    /// The annotation chosen on a PDF, with its page.
    chosen: RefCell<Option<(usize, annots::Annotation)>>,
    /// A part of a page chosen with 矩形選取, from its top left in points.
    area: Cell<Option<(usize, Place)>>,
    /// Parts of pages to black out for good when the PDF is saved.
    redactions: RefCell<Vec<(usize, Place)>>,
    /// The magnifier: whether it is out, and where the pointer is over the view.
    loupe: Cell<bool>,
    loupe_at: Cell<Option<(f64, f64)>>,
    loupe_layer: gtk::DrawingArea,
    dark_pdf: Cell<bool>,
    /// Whether a picture's clear parts are shown on a checkerboard.
    checker: Cell<bool>,
    /// The slideshow's window, and its timer while it plays on by itself.
    slideshow: RefCell<Option<gtk::Window>>,
    playing: RefCell<Option<glib::SourceId>>,
    background_css: gtk::CssProvider,
    /// The picture as it was when 調整顏色 opened, which its sliders change a copy of.
    adjusting: RefCell<Option<Pixbuf>>,
}

/// In the temporary folder, where Ubuntu's AppArmor lets qpdf read and write PDFs, as it does
/// not in a hidden folder such as the cache; made there under a name no other user can guess
/// first, open to this user only.
fn scratch_dir() -> PathBuf {
    static DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        glib::mkdtemp(glib::tmp_dir().join("nekoland-preview-XXXXXX"))
            .unwrap_or_else(|| glib::user_cache_dir().join(format!("nekoland/preview-{}", std::process::id())))
    })
    .clone()
}

fn open_pdf(path: &Path, password: Option<&str>) -> Result<poppler::Document, glib::Error> {
    poppler::Document::from_file(&gio::File::for_path(path).uri(), password)
}

/// How many pages a PDF has.
fn page_count(path: &Path) -> Result<u32, String> {
    open_pdf(path, None)
        .map(|document| document.n_pages().max(0) as u32)
        .map_err(|err| err.to_string())
}

/// The programs that write the formats Pixbuf cannot, by extension: each is tried in turn,
/// with the PNG to read and the file to write.
const ENCODERS: [(&[&str], &[&str]); 3] = [
    (&["heic", "heif"], &["heif-enc"]),
    (&["jp2", "j2k"], &["opj_compress"]),
    (&["exr"], &["magick", "convert"]),
];

/// Whether `path` names a format only an encoder program writes.
fn needs_encoder(path: &Path) -> bool {
    let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    ENCODERS.iter().any(|(extensions, _)| extensions.contains(&extension.as_str()))
}

/// Writes the picture as HEIC, JPEG 2000 or OpenEXR: as a PNG first, then converted by
/// `heif-enc`, `opj_compress` or ImageMagick, whichever is installed.
fn export_with_encoder(pixbuf: &Pixbuf, path: &Path, quality: i32) -> Result<(), String> {
    let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    let Some((_, programs)) = ENCODERS.iter().find(|(extensions, _)| extensions.contains(&extension.as_str())) else {
        return Err(String::from("unknown format"));
    };
    let dir = scratch_dir();
    std::fs::create_dir_all(&dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    let png = dir.join("export.png");
    pixbuf.savev(&png, "png", &[]).map_err(|err| err.to_string())?;
    let mut missing = Vec::new();
    let mut outcome = Err(String::new());
    for program in programs.iter() {
        let mut command = Command::new(program);
        match *program {
            "heif-enc" => command.arg("-q").arg(quality.to_string()).arg("-o").arg(path).arg(&png),
            "opj_compress" => command.arg("-i").arg(&png).arg("-o").arg(path),
            _ => command.arg(&png).arg(path),
        };
        match command.output() {
            Ok(output) if output.status.success() => {
                outcome = Ok(());
                break;
            }
            Ok(output) => {
                outcome = Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
                break;
            }
            Err(_) => missing.push(*program),
        }
    }
    let _ = std::fs::remove_file(&png);
    if outcome == Err(String::new()) && !missing.is_empty() {
        return Err(format!("找不到 {}", missing.join("、")));
    }
    outcome
}

/// A PDF of one page that is the picture, at 72 pixels to the inch.
fn export_pdf(pixbuf: &Pixbuf, path: &Path) -> Result<(), String> {
    let (width, height) = (f64::from(pixbuf.width()), f64::from(pixbuf.height()));
    let surface = gtk::cairo::PdfSurface::new(width, height, path).map_err(|err| err.to_string())?;
    let cr = gtk::cairo::Context::new(&surface).map_err(|err| err.to_string())?;
    cr.set_source_pixbuf(pixbuf, 0.0, 0.0);
    cr.paint().map_err(|err| err.to_string())?;
    drop(cr);
    surface.finish();
    Ok(())
}

/// Writes the PDF at `path` again with the pages `plan` names, each the file and the number
/// of one page, in that order; the file is replaced whole or not at all.
fn rebuild_pdf(path: &Path, plan: &[(PathBuf, u32)]) -> Result<(), String> {
    let dir = scratch_dir().join("rebuild");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    let result = (|| {
        let run = |program: &str, args: Vec<std::ffi::OsString>| {
            let output = Command::new(program).args(args).output().map_err(|err| format!("{program}: {err}"))?;
            if output.status.success() {
                Ok(())
            } else {
                Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
            }
        };
        let mut sources: Vec<&PathBuf> = Vec::new();
        for (source, _) in plan {
            if !sources.contains(&source) {
                sources.push(source);
            }
        }
        for (index, source) in sources.iter().enumerate() {
            run("pdfseparate", vec![(*source).into(), dir.join(format!("{index}-%d.pdf")).into()])?;
        }
        let mut args: Vec<std::ffi::OsString> = plan
            .iter()
            .map(|(source, page)| {
                let index = sources.iter().position(|kept| *kept == source).unwrap_or(0);
                dir.join(format!("{index}-{page}.pdf")).into()
            })
            .collect();
        let joined = dir.join("joined.pdf");
        args.push(joined.clone().into());
        run("pdfunite", args)?;
        let kept = path.with_extension("pdf.new");
        std::fs::copy(&joined, &kept).map_err(|err| format!("{}: {err}", kept.display()))?;
        std::fs::rename(&kept, path).map_err(|err| format!("{}: {err}", path.display()))
    })();
    let _ = std::fs::remove_dir_all(&dir);
    result
}

/// The file type `Pixbuf::savev` writes for `path`, and its options.
fn save_format(path: &Path) -> Option<(&'static str, Vec<(&'static str, &'static str)>)> {
    match path.extension()?.to_str()?.to_lowercase().as_str() {
        "png" => Some(("png", vec![])),
        "jpg" | "jpeg" => Some(("jpeg", vec![("quality", "95")])),
        "tif" | "tiff" => Some(("tiff", vec![])),
        "bmp" => Some(("bmp", vec![])),
        _ => None,
    }
}

/// An Illustrator file is a PDF inside, so it is read as one, page by page; only a `.pdf` is
/// written again by the page actions.
fn opens_as_pdf(path: &Path) -> bool {
    is_pdf(path) || path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("ai"))
}

/// The ImageMagick decoder for `path`'s extension. ImageMagick picks a decoder by what a file
/// holds, so a file named `.hdr` could be an MVG or SVG script; naming the decoder keeps it to
/// the one the extension says, and only these are ever handed over.
fn magick_coder(path: &Path) -> Option<&'static str> {
    let extension = path.extension()?.to_str()?.to_lowercase();
    Some(match extension.as_str() {
        "hdr" => "hdr",
        "sgi" => "sgi",
        "exr" => "exr",
        "pict" | "pct" | "pic" => "pict",
        "heic" => "heic",
        "heif" => "heif",
        "jp2" | "j2k" | "jpx" => "jp2",
        "psd" => "psd",
        "ico" => "ico",
        "tga" => "tga",
        "xbm" => "xbm",
        "dng" => "dng",
        "cr3" => "cr3",
        "3fr" => "3fr",
        "fff" => "fff",
        "raw" => "dng",
        _ => return None,
    })
}

/// The picture in `path`: by gdk-pixbuf, or by ImageMagick as a PNG for the formats it
/// cannot read (HDR, SGI, PICT, OpenEXR, camera RAW, and the like) when it is installed.
fn load_picture(path: &Path) -> Result<Pixbuf, String> {
    let first = match Pixbuf::from_file(path) {
        Ok(pixbuf) => return Ok(pixbuf),
        Err(err) => err.to_string(),
    };
    let Some(coder) = magick_coder(path) else { return Err(first) };
    let dir = scratch_dir();
    std::fs::create_dir_all(&dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    let png = dir.join("convert.png");
    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);
    let png = png.with_file_name(format!("convert-{:x}.png", hasher.finish()));
    let converted = ["magick", "convert"].iter().any(|program| {
        // `[0]` takes the first frame of a file with several.
        let mut source = std::ffi::OsString::from(format!("{coder}:"));
        source.push(path.as_os_str());
        source.push("[0]");
        Command::new(program).arg(source).arg(&png).output().is_ok_and(|output| output.status.success())
    });
    let loaded = if converted { Pixbuf::from_file(&png).map_err(|err| err.to_string()) } else { Err(first) };
    let _ = std::fs::remove_file(&png);
    loaded
}

/// Reads a picture on a thread of its own, so a large one or ImageMagick's conversion does
/// not hold up the window. A Pixbuf cannot cross from a thread to the window's; its pixels can.
async fn read_off_thread(read: impl FnOnce() -> Result<Pixbuf, String> + Send + 'static) -> Result<Pixbuf, String> {
    let (bytes, alpha, width, height, stride) = gio::spawn_blocking(move || {
        read().map(|pixbuf| (pixbuf.read_pixel_bytes(), pixbuf.has_alpha(), pixbuf.width(), pixbuf.height(), pixbuf.rowstride()))
    })
    .await
    .unwrap_or_else(|panic| std::panic::resume_unwind(panic))?;
    Ok(Pixbuf::from_bytes(&bytes, gtk::gdk_pixbuf::Colorspace::Rgb, alpha, 8, width, height, stride))
}

fn is_pdf(path: &Path) -> bool {
    path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
}

thread_local! {
    /// Every window, kept until it is destroyed.
    static WINDOWS: RefCell<Vec<Rc<Win>>> = RefCell::default();
    /// The settings, shared by every window.
    static PREFS: Rc<RefCell<Prefs>> = Rc::new(RefCell::new(Prefs::load()));
    /// 打開最近使用的檔案's entries, which every window's menu bar shares.
    static RECENT: gio::Menu = gio::Menu::new();
}

/// A list for the sidebar, scrolled, saying `empty` while it has nothing.
fn sidebar_list(empty: &str) -> (gtk::ListBox, gtk::ScrolledWindow) {
    let list = gtk::ListBox::builder().css_classes(["navigation-sidebar"]).build();
    let placeholder = gtk::Label::builder()
        .label(empty)
        .css_classes(["dim-label"])
        .wrap(true)
        .margin_top(EDGE)
        .margin_start(EDGE)
        .margin_end(EDGE)
        .build();
    list.set_placeholder(Some(&placeholder));
    let scrolled = gtk::ScrolledWindow::builder().child(&list).hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).build();
    (list, scrolled)
}

impl Win {
    fn open(app: &adw::Application) -> Rc<Win> {
        let picture = gtk::Picture::builder()
            .content_fit(gtk::ContentFit::Fill)
            .can_shrink(true)
            .css_classes(["pv-sheet"])
            .build();
        let mark_layer = gtk::DrawingArea::builder().can_target(false).build();
        let canvas = gtk::Overlay::builder()
            .child(&picture)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .margin_top(EDGE)
            .margin_bottom(EDGE)
            .margin_start(EDGE)
            .margin_end(EDGE)
            .build();
        canvas.add_overlay(&mark_layer);
        let scroll = gtk::ScrolledWindow::builder()
            .child(&canvas)
            .hexpand(true)
            .vexpand(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .css_classes(["pv-canvas"])
            .build();
        let open_button = gtk::Button::builder()
            .label("打開⋯")
            .halign(gtk::Align::Center)
            .css_classes(["pill", "suggested-action"])
            .action_name("win.open")
            .build();
        let empty = adw::StatusPage::builder()
            .icon_name("image-x-generic-symbolic")
            .title("選擇要預覽的檔案")
            .child(&open_button)
            .build();
        // A PDF's pages in rows, an `EDGE` around and between them.
        let pdf_column = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(EDGE)
            .halign(gtk::Align::Center)
            .margin_top(EDGE)
            .margin_bottom(EDGE)
            .margin_start(EDGE)
            .margin_end(EDGE)
            .focusable(true)
            .build();
        pdf_column.set_cursor_from_name(Some("text"));
        // The column takes the focus when a page is pressed; scrolled to it, the view would jump
        // to its end mid-drag.
        let pdf_view = gtk::Viewport::builder().child(&pdf_column).scroll_to_focus(false).build();
        let pdf_scroll = gtk::ScrolledWindow::builder()
            .child(&pdf_view)
            .hexpand(true)
            .vexpand(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .css_classes(["pv-canvas"])
            .build();
        let sheet = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::Single)
            .activate_on_single_click(false)
            .homogeneous(true)
            .column_spacing(EDGE as u32)
            .row_spacing(EDGE as u32)
            .margin_top(EDGE)
            .margin_bottom(EDGE)
            .margin_start(EDGE)
            .margin_end(EDGE)
            .valign(gtk::Align::Start)
            .build();
        let sheet_scroll = gtk::ScrolledWindow::builder().child(&sheet).hscrollbar_policy(gtk::PolicyType::Never).css_classes(["pv-canvas"]).build();
        let stack = gtk::Stack::new();
        stack.add_named(&empty, Some("empty"));
        stack.add_named(&scroll, Some("view"));
        stack.add_named(&pdf_scroll, Some("pdf"));
        stack.add_named(&sheet_scroll, Some("sheet"));
        let loupe_layer = gtk::DrawingArea::builder().can_target(false).build();
        let view = gtk::Overlay::builder().child(&stack).build();
        view.add_overlay(&loupe_layer);

        let list = gtk::ListBox::builder()
            .css_classes(["navigation-sidebar"])
            .selection_mode(gtk::SelectionMode::Multiple)
            .build();
        let thumbnails_page = gtk::ScrolledWindow::builder().child(&list).hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).build();
        let (contents, contents_page) = sidebar_list("沒有目錄");
        let (notes, notes_page) = sidebar_list("沒有標示重點和附註");
        let (bookmarks, bookmarks_page) = sidebar_list("沒有書籤");
        let sidebar_stack = gtk::Stack::new();
        sidebar_stack.add_named(&thumbnails_page, Some(Sidebar::Thumbnails.key()));
        sidebar_stack.add_named(&contents_page, Some(Sidebar::Contents.key()));
        sidebar_stack.add_named(&notes_page, Some(Sidebar::Notes.key()));
        sidebar_stack.add_named(&bookmarks_page, Some(Sidebar::Bookmarks.key()));

        let title = adw::WindowTitle::new("預覽程式", "");
        let sidebar_button = adw::SplitButton::builder()
            .icon_name("sidebar-show-symbolic")
            .tooltip_text("側邊欄")
            .dropdown_tooltip("顯示方式")
            .menu_model(&menus::sidebar_menu())
            .action_name("win.toggle-sidebar")
            .build();
        let info = gtk::Button::builder()
            .icon_name("document-properties-symbolic")
            .tooltip_text("顯示檢閱器")
            .action_name("win.info")
            .build();
        let zoom_out = gtk::Button::builder().icon_name("zoom-out-symbolic").tooltip_text("縮小").action_name("win.zoom-out").build();
        let zoom_button = gtk::Button::builder().css_classes(["numeric"]).action_name("win.zoom-toggle").build();
        let zoom_in = gtk::Button::builder().icon_name("zoom-in-symbolic").tooltip_text("放大").action_name("win.zoom-in").build();
        let zoom_box = gtk::Box::builder().css_classes(["linked"]).build();
        zoom_box.append(&zoom_out);
        zoom_box.append(&zoom_button);
        zoom_box.append(&zoom_in);
        let highlight_menu = gio::Menu::new();
        let kinds = gio::Menu::new();
        kinds.append(Some("標示重點"), Some("win.mark::highlight"));
        kinds.append(Some("底線"), Some("win.mark::underline"));
        kinds.append(Some("刪除線"), Some("win.mark::strikeout"));
        highlight_menu.append_section(None, &kinds);
        let colours = gio::Menu::new();
        for (index, (_, name)) in MARK_COLOURS.iter().enumerate() {
            colours.append(Some(name), Some(&format!("win.mark-colour::{index}")));
        }
        highlight_menu.append_section(None, &colours);
        let highlight_button = adw::SplitButton::builder()
            .icon_name("format-text-rich-symbolic")
            .tooltip_text("標示重點")
            .dropdown_tooltip("標示重點樣式")
            .menu_model(&highlight_menu)
            .action_name("win.highlight")
            .build();
        let rotate_button = gtk::Button::builder()
            .icon_name("object-rotate-left-symbolic")
            .tooltip_text("向左旋轉")
            .action_name("win.rotate-left")
            .build();
        let markup_toggle = gtk::ToggleButton::builder()
            .icon_name("document-edit-symbolic")
            .tooltip_text("顯示標記工具列")
            .action_name("win.markup")
            .build();
        let find_entry = gtk::SearchEntry::builder().placeholder_text("搜尋").width_chars(14).build();
        let find_count = gtk::Label::builder().css_classes(["dim-label", "numeric"]).visible(false).build();
        let find_menu = gio::Menu::new();
        let words = gio::Menu::new();
        words.append(Some("完全符合"), Some("win.find-words::phrase"));
        words.append(Some("任何符合"), Some("win.find-words::any"));
        let order = gio::Menu::new();
        order.append(Some("搜尋等級"), Some("win.find-order::rank"));
        order.append(Some("頁面順序"), Some("win.find-order::page"));
        find_menu.append_section(None, &words);
        find_menu.append_section(None, &order);
        let find_options = gtk::MenuButton::builder().icon_name("view-more-symbolic").tooltip_text("搜尋選項").menu_model(&find_menu).css_classes(["flat"]).build();
        let find_box = gtk::Box::builder().spacing(GAP).visible(false).build();
        find_box.append(&find_count);
        find_box.append(&find_entry);
        find_box.append(&find_options);
        let header = adw::HeaderBar::builder().title_widget(&title).build();
        header.pack_start(&sidebar_button);
        header.pack_end(&find_box);
        header.pack_end(&markup_toggle);
        header.pack_end(&rotate_button);
        header.pack_end(&highlight_button);
        header.pack_end(&zoom_box);
        header.pack_end(&info);

        let bar = markup::build_bar();
        let content = adw::ToolbarView::new();
        content.add_top_bar(&header);
        // It slides down from under the toolbar and back up into it.
        let markup_bar = gtk::Revealer::builder()
            .transition_type(gtk::RevealerTransitionType::SlideDown)
            .child(&bar.bar)
            .build();
        content.add_top_bar(&markup_bar);
        content.set_content(Some(&view));
        let split = adw::OverlaySplitView::builder()
            .sidebar(&sidebar_stack)
            .content(&content)
            .min_sidebar_width(160.0)
            .max_sidebar_width(260.0)
            .sidebar_width_fraction(0.2)
            .show_sidebar(false)
            .build();
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("預覽程式")
            .default_width(960)
            .default_height(700)
            .content(&split)
            .build();
        let prefs = PREFS.with(Rc::clone);
        let layout = prefs.borrow().first_layout;
        let background_css = gtk::CssProvider::new();
        gtk::style_context_add_provider_for_display(&WidgetExt::display(&window), &background_css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1);

        let win = Rc::new(Win {
            app: app.clone(),
            window,
            split,
            sidebar_button,
            title,
            stack,
            scroll,
            picture,
            mark_layer,
            content,
            markup_bar,
            picture_tools: bar.picture_tools,
            pdf_tools: bar.pdf_tools,
            highlight_button,
            tool: Cell::new(None),
            shape: Cell::new(Shape::Rectangle),
            signature: Cell::new(None),
            signature_menu: bar.signature_menu,
            color: bar.color,
            fill: bar.fill,
            filled: Cell::new(false),
            width: Cell::new(shapes::WIDTHS[1]),
            font: bar.font,
            stroke: RefCell::default(),
            stroke_page: Cell::new(None),
            image_mask: RefCell::new(None),
            image_outline: RefCell::default(),
            mask_overlay: RefCell::new(None),
            list,
            sidebar_stack,
            contents,
            notes,
            bookmarks,
            sheet,
            sidebar_mode: Cell::new(Sidebar::Thumbnails),
            zoom_button,
            items: RefCell::new(Vec::new()),
            thumbnails: RefCell::default(),
            current: Cell::new(0),
            zoom: Cell::new(Zoom::Fit),
            shown: RefCell::new(None),
            syncing: Cell::new(false),
            closing: Cell::new(false),
            pdf_scroll,
            pdf_column,
            pdf: RefCell::default(),
            selection: Cell::new(None),
            pending: Cell::new(None),
            jumping: Cell::new(false),
            fitting: Cell::new(false),
            find_box,
            find_entry,
            find_count,
            matches: RefCell::default(),
            match_at: Cell::new(None),
            finding: Cell::new(0),
            typing: Cell::new(false),
            passwords: RefCell::default(),
            prefs,
            layout: Cell::new(layout),
            back: RefCell::default(),
            forward: RefCell::default(),
            marking: Cell::new(None),
            last_marking: Cell::new(Marking { kind: annots::Kind::Highlight, colour: MARK_COLOURS[0].0 }),
            chosen: RefCell::new(None),
            area: Cell::new(None),
            redactions: RefCell::default(),
            loupe: Cell::new(false),
            loupe_at: Cell::new(None),
            loupe_layer,
            dark_pdf: Cell::new(false),
            checker: Cell::new(false),
            slideshow: RefCell::new(None),
            playing: RefCell::new(None),
            background_css,
            adjusting: RefCell::new(None),
        });
        WINDOWS.with(|windows| windows.borrow_mut().push(win.clone()));
        let weak = Rc::downgrade(&win);
        win.window.connect_destroy(move |_| {
            if let Some(closing) = weak.upgrade() {
                WINDOWS.with(|windows| windows.borrow_mut().retain(|kept| !Rc::ptr_eq(kept, &closing)));
            }
        });
        win.add_actions();
        win.connect();
        win.connect_markup();
        win.apply_background();
        win.fill_signatures();
        win.show_nothing();
        win.window.present();
        win
    }

    /// Adds an action called `name` that runs `run`, given the target it is activated with when
    /// `parameter` says it takes one; with `state`, it keeps one, which `run` sets.
    fn action(
        self: &Rc<Self>,
        name: &str,
        parameter: Option<&glib::VariantTy>,
        state: Option<glib::Variant>,
        run: impl Fn(&Rc<Win>, Option<&glib::Variant>) + 'static,
    ) {
        let action = match state {
            Some(state) => gio::SimpleAction::new_stateful(name, parameter, &state),
            None => gio::SimpleAction::new(name, parameter),
        };
        let weak = Rc::downgrade(self);
        action.connect_activate(move |_, target| {
            if let Some(win) = weak.upgrade() {
                run(&win, target);
            }
        });
        self.window.add_action(&action);
    }

    fn set_action_state(&self, name: &str, state: glib::Variant) {
        if let Some(action) = self.window.lookup_action(name).and_downcast::<gio::SimpleAction>() {
            action.set_state(&state);
        }
    }

    fn add_actions(self: &Rc<Self>) {
        let plain: &[(&str, fn(&Rc<Win>))] = &[
            ("open", |win| win.choose_files()),
            ("new-from-clipboard", |win| win.new_from_clipboard()),
            ("close", |win| win.window.close()),
            ("close-selected", |win| win.close_selected()),
            ("save", |win| win.save()),
            ("duplicate", |win| win.duplicate()),
            ("save-as", |win| win.save_as()),
            ("rename", |win| win.rename()),
            ("move-to", |win| win.move_to()),
            ("revert-saved", |win| win.revert_saved()),
            ("browse-versions", |win| win.browse_versions()),
            ("export", |win| win.export()),
            ("export-pdf", |win| win.export_pdf()),
            ("enter-password", |win| win.enter_password()),
            ("edit-permissions", |win| win.edit_permissions()),
            ("import-scanner", |win| win.import_scanner()),
            ("print", |win| win.print()),
            ("undo", |win| win.undo()),
            ("redo", |win| win.redo()),
            ("cut", |win| win.cut()),
            ("copy", |win| win.copy()),
            ("copy-subject", |win| win.copy_subject()),
            ("paste", |win| win.paste()),
            ("delete", |win| win.delete()),
            ("select-all", |win| win.select_all()),
            ("select-none", |win| win.select_none()),
            ("invert-selection", |win| win.invert_selection()),
            ("insert-pdf", |win| win.insert_from_file()),
            ("insert-blank", |win| win.insert_blank()),
            ("insert-scanner", |win| win.insert_scanner()),
            ("delete-page", |win| win.delete_pages()),
            ("page-up", |win| win.move_page(-1)),
            ("page-down", |win| win.move_page(1)),
            ("trash", |win| win.trash()),
            ("find", |win| {
                win.find_entry.grab_focus();
            }),
            ("find-next", |win| win.step_match(1)),
            ("find-previous", |win| win.step_match(-1)),
            ("find-selection", |win| win.find_selection()),
            ("jump-selection", |win| win.jump_selection()),
            ("toggle-sidebar", |win| win.toggle_sidebar()),
            ("zoom-in", |win| win.zoom_by(ZOOM_STEP)),
            ("zoom-out", |win| win.zoom_by(1.0 / ZOOM_STEP)),
            ("zoom-fit", |win| win.set_zoom(Zoom::Fit)),
            ("zoom-actual", |win| win.set_actual()),
            ("zoom-toggle", |win| match win.zoom.get() {
                Zoom::Fit => win.set_actual(),
                Zoom::Scale(_) => win.set_zoom(Zoom::Fit),
            }),
            ("zoom-selection", |win| win.zoom_selection()),
            ("slideshow", |win| win.slideshow()),
            ("previous", |win| win.step(-1)),
            ("next", |win| win.step(1)),
            ("previous-document", |win| win.step_document(-1)),
            ("next-document", |win| win.step_document(1)),
            ("first-page", |win| win.go_to_end(false)),
            ("last-page", |win| win.go_to_end(true)),
            ("go-to-page", |win| win.ask_page()),
            ("back", |win| win.go_back()),
            ("forward", |win| win.go_forward()),
            ("info", |win| win.show_info()),
            ("adjust-color", |win| win.adjust_color()),
            ("resize", |win| win.resize()),
            ("assign-profile", |win| win.assign_profile()),
            ("describe", |win| win.describe()),
            ("bookmark", |win| win.add_bookmark()),
            ("rotate-left", |win| win.turn(false)),
            ("rotate-right", |win| win.turn(true)),
            ("flip", |win| win.flip(true)),
            ("flip-vertical", |win| win.flip(false)),
            ("crop", |win| win.crop()),
            ("remove-background", |win| win.remove_background()),
            ("highlight", |win| win.toggle_highlight()),
            ("manage-signatures", |win| win.make_signature()),
            ("minimize", |win| win.window.minimize()),
            ("cancel", |win| win.cancel()),
        ];
        for &(name, run) in plain {
            self.action(name, None, None, move |win, _| run(win));
        }
        let text = Some(glib::VariantTy::STRING);
        let with_text: &[(&str, fn(&Rc<Win>, &str))] = &[
            ("open-recent", |win, path| win.open_files(vec![PathBuf::from(path)])),
            ("screenshot", |win, kind| win.screenshot(kind)),
            ("mark", |win, kind| win.mark(kind)),
            ("mark-colour", |win, index| win.mark_colour(index.parse().unwrap_or(0))),
            ("annotate", |win, key| win.annotate(key)),
            ("signature", |win, index| win.choose_signature(index.parse().ok())),
        ];
        for &(name, run) in with_text {
            self.action(name, text, None, move |win, target| {
                if let Some(target) = target.and_then(|target| target.str()) {
                    run(win, target);
                }
            });
        }
        // Chosen one of several, as the menus' radio items show.
        let chosen: &[(&str, String, fn(&Rc<Win>, &str))] = &[
            ("sidebar", Sidebar::Thumbnails.key().to_owned(), |win, key| win.set_sidebar(Sidebar::from_key(key))),
            ("layout", layout_key(self.layout.get()).to_owned(), |win, key| win.set_layout(layout_from_key(key))),
            ("tool", String::new(), |win, key| win.set_tool(Tool::from_key(key))),
            ("line-width", format!("{}", self.width.get()), |win, width| win.set_width(width.parse().unwrap_or(2.0))),
            ("find-words", String::from("phrase"), |win, _| win.find()),
            ("find-order", String::from("page"), |win, _| win.find()),
        ];
        for (name, initial, run) in chosen {
            let run = *run;
            let name_owned = (*name).to_owned();
            self.action(name, text, Some(initial.to_variant()), move |win, target| {
                if let Some(key) = target.and_then(|target| target.str()) {
                    win.set_action_state(&name_owned, key.to_variant());
                    run(win, key);
                }
            });
        }
        // On or off, as the menus' check items show.
        let toggles: &[(&str, fn(&Rc<Win>, bool))] = &[
            ("markup", |win, on| win.set_markup(on)),
            ("hide-toolbar", |win, on| win.content.set_reveal_top_bars(!on)),
            ("fullscreen", |win, on| win.window.set_fullscreened(on)),
            ("loupe", |win, on| win.set_loupe(on)),
            ("dark-pdf", |win, on| win.set_dark_pdf(on)),
            ("image-background", |win, on| win.set_checker(on)),
            ("fill-shapes", |win, on| win.filled.set(on)),
            ("lock", |win, on| win.set_locked(on)),
        ];
        for &(name, run) in toggles {
            let name_owned = name.to_owned();
            self.action(name, None, Some(false.to_variant()), move |win, _| {
                let on = !win.toggled(&name_owned);
                win.set_action_state(&name_owned, on.to_variant());
                run(win, on);
            });
        }
    }

    /// Whether the action `name`, one that is on or off, is on.
    fn toggled(&self, name: &str) -> bool {
        self.window.lookup_action(name).and_then(|action| action.state()).and_then(|state| state.get::<bool>()).unwrap_or(false)
    }

    fn connect(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.list.connect_selected_rows_changed({
            let weak = weak.clone();
            move |list| {
                let Some(win) = weak.upgrade().filter(|win| !win.syncing.get()) else { return };
                let rows: Vec<usize> = list.selected_rows().iter().map(|row| row.index() as usize).collect();
                // One row clicked goes to it; rows added to the choice keep what is shown.
                match rows.as_slice() {
                    [only] => win.select(*only),
                    [first, ..] if !rows.contains(&win.current.get()) => win.select(*first),
                    _ => win.update_actions(),
                }
            }
        });
        self.window.connect_notify_local(Some("focus-widget"), {
            let weak = weak.clone();
            move |window, _| {
                let Some(win) = weak.upgrade() else { return };
                let typing = gtk::prelude::GtkWindowExt::focus(window).is_some_and(|widget| widget.is::<gtk::Text>() || widget.is::<gtk::TextView>());
                if win.typing.replace(typing) != typing {
                    win.update_actions();
                }
            }
        });
        self.window.connect_notify_local(Some("fullscreened"), {
            let weak = weak.clone();
            move |window, _| {
                if let Some(win) = weak.upgrade() {
                    win.set_action_state("fullscreen", window.is_fullscreen().to_variant());
                }
            }
        });

        // Fit follows the window's size.
        for adjustment in [
            self.scroll.hadjustment(),
            self.scroll.vadjustment(),
            self.pdf_scroll.hadjustment(),
            self.pdf_scroll.vadjustment(),
        ] {
            let weak = weak.clone();
            adjustment.connect_notify_local(Some("page-size"), move |_, _| {
                let Some(win) = weak.upgrade().filter(|win| win.zoom.get() == Zoom::Fit && !win.fitting.replace(true)) else {
                    return;
                };
                // The page size changes while the window is laid out, when sizes asked for
                // again are not laid out; they are asked for once that is over.
                glib::idle_add_local_once(move || {
                    win.fitting.set(false);
                    if win.zoom.get() == Zoom::Fit {
                        win.apply_zoom();
                    }
                });
            });
        }
        let pages = self.pdf_scroll.vadjustment();
        pages.connect_value_changed({
            let weak = weak.clone();
            move |_| {
                if let Some(win) = weak.upgrade() {
                    win.scrolled();
                }
            }
        });
        // The column has grown or shrunk to the pages it was given. This is said while the
        // window is laid out, when a scroll would not move what is shown, so it waits for
        // that to be over.
        pages.connect_changed({
            let weak = weak.clone();
            move |_| {
                let weak = weak.clone();
                glib::idle_add_local_once(move || {
                    if let Some(win) = weak.upgrade() {
                        win.apply_pending();
                    }
                });
            }
        });

        for scroll in [&self.scroll, &self.pdf_scroll] {
            let wheel = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
            wheel.connect_scroll({
                let weak = weak.clone();
                move |wheel, _, dy| {
                    let held = wheel.current_event_state().contains(gdk::ModifierType::CONTROL_MASK);
                    let Some(win) = weak.upgrade() else { return glib::Propagation::Proceed };
                    if held {
                        win.zoom_by(ZOOM_STEP.powf(-dy));
                        return glib::Propagation::Stop;
                    }
                    win.scrolled_past(dy)
                }
            });
            scroll.add_controller(wheel);

            // Two fingers swept across the touchpad turn to the next or last page or picture,
            // once the view can go no further that way.
            let swipe = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::HORIZONTAL);
            // Ahead of the scrolled window's own handling, which would take it as a scroll.
            swipe.set_propagation_phase(gtk::PropagationPhase::Capture);
            let swept = Rc::new(Cell::new(0.0_f64));
            swipe.connect_scroll({
                let (weak, swept, scroll) = (weak.clone(), swept.clone(), scroll.clone());
                move |swipe, dx, _| {
                    let Some(win) = weak.upgrade() else { return glib::Propagation::Proceed };                    // A wheel's tilt is not a swipe.
                    if swipe.unit() != gdk::ScrollUnit::Surface || dx == 0.0 {
                        return glib::Propagation::Proceed;
                    }
                    let room = scroll.hadjustment();
                    let at_edge = if dx > 0.0 { room.value() >= room.upper() - room.page_size() - 0.5 } else { room.value() <= room.lower() + 0.5 };
                    if !at_edge || swept.get().is_nan() {
                        return glib::Propagation::Proceed;
                    }
                    swept.set(swept.get() + dx);
                    if swept.get().abs() >= SWIPE_DISTANCE {
                        win.step(if swept.get() > 0.0 { 1 } else { -1 });
                        // One turn a swipe; NaN until the fingers lift.
                        swept.set(f64::NAN);
                    }
                    glib::Propagation::Stop
                }
            });
            swipe.connect_scroll_end({
                let swept = swept.clone();
                move |_| swept.set(0.0)
            });
            scroll.add_controller(swipe);

            let pinch = gtk::GestureZoom::new();
            let began_at = Rc::new(Cell::new(1.0));
            pinch.connect_begin({
                let (weak, began_at) = (weak.clone(), began_at.clone());
                move |_, _| {
                    if let Some(win) = weak.upgrade() {
                        began_at.set(win.scale());
                    }
                }
            });
            pinch.connect_scale_changed({
                let weak = weak.clone();
                move |_, factor| {
                    if let Some(win) = weak.upgrade() {
                        win.set_zoom(Zoom::Scale(began_at.get() * factor));
                    }
                }
            });
            scroll.add_controller(pinch);
        }

        self.find_entry.connect_search_changed({
            let weak = weak.clone();
            move |_| {
                if let Some(win) = weak.upgrade() {
                    win.find();
                }
            }
        });
        for signal in ["activate", "next-match"] {
            let weak = weak.clone();
            self.find_entry.connect_local(signal, false, move |_| {
                if let Some(win) = weak.upgrade() {
                    win.step_match(1);
                }
                None
            });
        }
        self.find_entry.connect_previous_match({
            let weak = weak.clone();
            move |_| {
                if let Some(win) = weak.upgrade() {
                    win.step_match(-1);
                }
            }
        });
        self.find_entry.connect_stop_search(|entry| entry.set_text(""));

        let pan = gtk::GestureDrag::new();
        let origin = Rc::new(Cell::new((0.0, 0.0)));
        pan.connect_drag_begin({
            let (weak, origin) = (weak.clone(), origin.clone());
            move |_, _, _| {
                if let Some(win) = weak.upgrade().filter(|win| win.tool.get().is_none()) {
                    origin.set((win.scroll.hadjustment().value(), win.scroll.vadjustment().value()));
                }
            }
        });
        pan.connect_drag_update({
            let weak = weak.clone();
            move |_, dx, dy| {
                if let Some(win) = weak.upgrade().filter(|win| win.tool.get().is_none()) {
                    win.scroll.hadjustment().set_value(origin.get().0 - dx);
                    win.scroll.vadjustment().set_value(origin.get().1 - dy);
                }
            }
        });
        self.scroll.add_controller(pan);
        self.drag_out();

        let double = gtk::GestureClick::new();
        double.connect_pressed({
            let weak = weak.clone();
            move |_, presses, _, _| {
                if let (2, Some(win)) = (presses, weak.upgrade().filter(|win| win.tool.get().is_none())) {
                    let _ = WidgetExt::activate_action(&win.window, "win.zoom-toggle", None);
                }
            }
        });
        self.scroll.add_controller(double);

        let drop = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
        drop.connect_drop({
            let weak = weak.clone();
            move |_, value, _, _| {
                let (Some(win), Ok(files)) = (weak.upgrade(), value.get::<gdk::FileList>()) else {
                    return false;
                };
                win.add_files(files.files().iter().filter_map(|file| file.path()).collect());
                true
            }
        });
        self.stack.add_controller(drop);
        self.connect_view();

        self.window.connect_close_request({
            let weak = weak.clone();
            move |_| match weak.upgrade() {
                Some(win) if !win.closing.get() => {
                    win.close_window();
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });
    }

    /// What is shown dragged out as a file, with its picture for programs that take only
    /// that: from the title, as the Mac's proxy icon, and from the picture itself when it
    /// fits and there is nothing to pan.
    fn drag_out(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        let prepare = move |source: &gtk::DragSource, from_picture: bool| {
            let win = weak.upgrade()?;
            let (h, v) = (win.scroll.hadjustment(), win.scroll.vadjustment());
            let pans = h.upper() > h.page_size() + 1.0 || v.upper() > v.page_size() + 1.0;
            if from_picture && (pans || win.tool.get().is_some()) {
                return None;
            }
            let item = win.item()?;
            let picture = win.shown.borrow().as_ref().map(gdk::Texture::for_pixbuf);
            let mut kinds = Vec::new();
            if !item.untitled {
                let files = gdk::FileList::from_array(&[gio::File::for_path(&item.path)]);
                kinds.push(gdk::ContentProvider::for_value(&files.to_value()));
            }
            if let Some(picture) = &picture {
                kinds.push(gdk::ContentProvider::for_value(&picture.to_value()));
            }
            if kinds.is_empty() {
                return None;
            }
            let icon = win.shown.borrow().as_ref().and_then(|pixbuf| {
                let scale = 128.0 / f64::from(pixbuf.width().max(pixbuf.height()));
                let (w, h) = ((f64::from(pixbuf.width()) * scale) as i32, (f64::from(pixbuf.height()) * scale) as i32);
                pixbuf.scale_simple(w.max(1), h.max(1), gtk::gdk_pixbuf::InterpType::Bilinear)
            });
            match icon {
                Some(icon) => source.set_icon(Some(&gdk::Texture::for_pixbuf(&icon)), icon.width() / 2, icon.height() / 2),
                None => source.set_icon(Some(&gtk::WidgetPaintable::new(Some(&win.title))), 0, 0),
            }
            Some(gdk::ContentProvider::new_union(&kinds))
        };
        let from_title = gtk::DragSource::builder()
            .actions(gdk::DragAction::COPY)
            .propagation_phase(gtk::PropagationPhase::Capture)
            .build();
        let prepare = Rc::new(prepare);
        from_title.connect_prepare({
            let prepare = prepare.clone();
            move |source, _, _| prepare(source, false)
        });
        // Held, the title is the file's, not the header bar's: the bar would move the window
        // when the pointer leaps before the drag has seen it go far enough.
        let hold = gtk::GestureClick::builder()
            .propagation_phase(gtk::PropagationPhase::Capture)
            .build();
        hold.connect_pressed(|hold, _, _, _| {
            hold.set_state(gtk::EventSequenceState::Claimed);
        });
        self.title.add_controller(from_title.clone());
        self.title.add_controller(hold.clone());
        hold.group_with(&from_title);
        let from_picture = gtk::DragSource::builder().actions(gdk::DragAction::COPY).build();
        from_picture.connect_prepare(move |source, _, _| prepare(source, true));
        self.scroll.add_controller(from_picture);
    }

    fn item(&self) -> Option<Rc<Item>> {
        self.items.borrow().get(self.current.get()).cloned()
    }

    /// The rows chosen in the sidebar, the item shown among them.
    fn chosen_items(&self) -> Vec<usize> {
        let mut rows: Vec<usize> = self.list.selected_rows().iter().map(|row| row.index() as usize).collect();
        if !rows.contains(&self.current.get()) {
            rows = vec![self.current.get()];
        }
        rows.sort_unstable();
        rows
    }

    fn show_nothing(&self) {
        self.stack.set_visible_child_name("empty");
        self.title.set_title("預覽程式");
        self.title.set_subtitle("");
        self.window.set_title(Some("預覽程式"));
        self.zoom_button.set_label("");
        self.update_actions();
    }

    /// The items `paths` are, each picture and each page of each PDF, those that cannot be
    /// read said in `failed`, or the first PDF that needs a password.
    fn items_of(&self, paths: &[PathBuf], failed: &mut Vec<String>) -> Result<Vec<Rc<Item>>, PathBuf> {
        let mut items = Vec::new();
        for path in paths {
            if opens_as_pdf(path) {
                let password = self.passwords.borrow().get(path).cloned();
                match open_pdf(path, password.as_deref()) {
                    Ok(document) => items.extend((1..=document.n_pages().max(0) as u32).map(|page| Item::new(path.clone(), Some(page)))),
                    Err(err) if err.matches(poppler::Error::Encrypted) => return Err(path.clone()),
                    Err(err) => failed.push(format!("{}: {err}", path.display())),
                }
            } else {
                items.push(Item::new(path.clone(), None));
            }
        }
        Ok(items)
    }

    /// Replaces what is shown with `paths`: each picture, and each page of each PDF.
    fn open_files(self: &Rc<Self>, paths: Vec<PathBuf>) {
        let mut failed = Vec::new();
        let items = match self.items_of(&paths, &mut failed) {
            Ok(items) => items,
            // All of them are opened again once the password is in, or asked for again when wrong.
            Err(locked) => return self.ask_password(locked, paths, false),
        };
        if !failed.is_empty() {
            alert::notice(&self.window, "無法打開", Some(&failed.join("\n")));
        }
        if items.is_empty() {
            return;
        }
        for path in &paths {
            remember_recent(path);
        }
        let resume = items.first().filter(|item| item.is_page()).and_then(|item| {
            let prefs = self.prefs.borrow();
            prefs.resume.then(|| prefs.last_page.get(&item.path).copied()).flatten()
        });
        self.set_items(items, resume.unwrap_or(0));
    }

    /// Adds `paths` after what is shown, as files dropped on the window are.
    fn add_files(self: &Rc<Self>, paths: Vec<PathBuf>) {
        if self.items.borrow().is_empty() {
            return self.open_files(paths);
        }
        let mut failed = Vec::new();
        let added = match self.items_of(&paths, &mut failed) {
            Ok(items) => items,
            Err(locked) => return self.ask_password(locked, paths, true),
        };
        if !failed.is_empty() {
            alert::notice(&self.window, "無法打開", Some(&failed.join("\n")));
        }
        if added.is_empty() {
            return;
        }
        let first = self.items.borrow().len();
        let mut items = self.items.borrow().clone();
        items.extend(added);
        self.set_items(items, first);
    }

    /// Shows `items` in the sidebar, and the one at `index`.
    fn set_items(self: &Rc<Self>, items: Vec<Rc<Item>>, index: usize) {
        // A PDF written again is read again.
        let keep = self.pdf.borrow().clone().filter(|pdf| items.iter().any(|item| item.path == pdf.path) && !pdf.dirty.get());
        if keep.is_none() {
            *self.pdf.borrow_mut() = None;
        }
        self.selection.set(None);
        self.syncing.set(true);
        while let Some(row) = self.list.row_at_index(0) {
            self.list.remove(&row);
        }
        let mut thumbnails = Vec::new();
        for (at, item) in items.iter().enumerate() {
            let (row, thumbnail) = thumbnail_row(item, at);
            self.drag_rows(&row);
            self.list.append(&row);
            thumbnails.push((item.clone(), thumbnail));
        }
        *self.thumbnails.borrow_mut() = thumbnails.iter().map(|(_, picture)| picture.clone()).collect();
        self.syncing.set(false);
        if items.len() > 1 && self.sidebar_mode.get() == Sidebar::Thumbnails {
            self.split.set_show_sidebar(true);
        }
        // A lone picture has nothing for a sidebar to list.
        let listed = items.len() > 1 || items.iter().any(|item| item.is_page());
        self.sidebar_button.set_visible(listed);
        if !listed {
            self.split.set_show_sidebar(false);
        }
        *self.items.borrow_mut() = items;
        self.select(index.min(self.items.borrow().len().saturating_sub(1)));
        self.load_thumbnails(thumbnails);
        self.fill_sidebar();
    }

    /// Fills the sidebar's thumbnails one at a time, so the window stays free between them.
    fn load_thumbnails(self: &Rc<Self>, thumbnails: Vec<(Rc<Item>, gtk::Picture)>) {
        let weak = Rc::downgrade(self);
        let passwords = self.passwords.borrow().clone();
        let density = f64::from(self.window.scale_factor());
        let shown = self.pdf.borrow().clone();
        glib::spawn_future_local(async move {
            let mut documents: HashMap<PathBuf, Option<poppler::Document>> = HashMap::new();
            for (item, picture) in thumbnails {
                if weak.upgrade().is_none() {
                    return;
                }
                if let Some(number) = item.page {
                    let document = documents.entry(item.path.clone()).or_insert_with(|| match &shown {
                        // The PDF shown, with what was put on it since.
                        Some(pdf) if pdf.path == item.path => Some(pdf.document()),
                        _ => open_pdf(&item.path, passwords.get(&item.path).map(String::as_str)).ok(),
                    });
                    let page = document.as_ref().and_then(|document| document.page(number as i32 - 1));
                    // Drawn at the screen's density, shown at its size in the sidebar.
                    let texture = page.and_then(|page| {
                        let (width, height) = page.size();
                        page_texture(&page, f64::from(THUMB_SIZE) / width.max(height) * density)
                    });
                    if let Some(texture) = texture {
                        let size = |side: i32| ((f64::from(side) / density).round() as i32).max(1);
                        picture.set_size_request(size(texture.width()), size(texture.height()));
                        picture.set_paintable(Some(&texture));
                    }
                    glib::timeout_future(std::time::Duration::from_millis(1)).await;
                    continue;
                }
                let edited = item.edited.borrow().clone();
                let small = match edited {
                    Some(pixbuf) => thumbnail_of(&pixbuf),
                    None => {
                        let path = item.path.clone();
                        read_off_thread(move || {
                            let pixbuf = Pixbuf::from_file_at_scale(&path, THUMB_SIZE, THUMB_SIZE, true).or_else(|_| load_picture(&path))?;
                            thumbnail_of(&pixbuf).ok_or_else(String::new)
                        })
                        .await
                        .ok()
                    }
                };
                if let Some(small) = small {
                    picture.set_size_request(small.width(), small.height());
                    picture.set_paintable(Some(&gdk::Texture::for_pixbuf(&small)));
                }
                glib::timeout_future(std::time::Duration::from_millis(1)).await;
            }
        });
    }

    fn select(self: &Rc<Self>, index: usize) {
        let Some(item) = self.items.borrow().get(index).cloned() else {
            return;
        };
        let before = self.item();
        self.current.set(index);
        self.syncing.set(true);
        if let Some(row) = self.list.row_at_index(index as i32) {
            if !row.is_selected() || self.list.selected_rows().len() > 1 && before.is_none() {
                self.list.unselect_all();
                self.list.select_row(Some(&row));
            }
        }
        self.syncing.set(false);
        if before.as_ref().is_some_and(|before| before.path != item.path) {
            self.leave_item();
        }

        if let Some(edited) = item.edited.borrow().clone() {
            return self.show(&item, edited);
        }
        match item.page {
            None => {
                // Nothing is edited or saved from the picture before while this one is read.
                *self.shown.borrow_mut() = None;
                self.update_actions();
                let weak = Rc::downgrade(self);
                let path = item.path.clone();
                glib::spawn_future_local(async move {
                    let loaded = read_off_thread(move || {
                        load_picture(&path).map(|pixbuf| pixbuf.apply_embedded_orientation().unwrap_or(pixbuf))
                    })
                    .await;
                    let Some(win) = weak.upgrade() else { return };
                    // Another item was shown meanwhile, or this one was drawn on.
                    if !win.item().is_some_and(|shown| Rc::ptr_eq(&shown, &item)) || item.edited.borrow().is_some() {
                        return;
                    }
                    match loaded {
                        Ok(pixbuf) => win.show(&item, pixbuf),
                        Err(err) => win.fail(&item, &err),
                    }
                });
            }
            Some(page) => match self.show_pdf(&item.path) {
                Ok(()) => {
                    *self.shown.borrow_mut() = None;
                    self.show_tools();
                    self.apply_zoom();
                    self.update_title(&item);
                    self.update_actions();
                    self.go_page(page as usize - 1, None);
                }
                Err(err) => self.fail(&item, &err),
            },
        }
    }

    /// What is being done to the item shown ends as another is shown.
    fn leave_item(&self) {
        self.set_mask(None, Vec::new());
        self.stroke.take();
        self.chosen.take();
        self.area.set(None);
        self.mark_layer.queue_draw();
    }

    /// Asks for the password of the PDF at `path`, then opens `paths` again with it, or adds them
    /// after what is shown when `adding`.
    fn ask_password(self: &Rc<Self>, path: PathBuf, paths: Vec<PathBuf>, adding: bool) {
        let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        let weak = Rc::downgrade(self);
        let entry = alert::prompt(&self.window, &format!("輸入「{name}」的密碼"), "", -1, "打開", move |password| {
            if let Some(win) = weak.upgrade() {
                win.passwords.borrow_mut().insert(path, password);
                if adding {
                    win.add_files(paths);
                } else {
                    win.open_files(paths);
                }
            }
        });
        entry.set_visibility(false);
        entry.set_input_purpose(gtk::InputPurpose::Password);
    }

    /// Shows the PDF at `path` in the column of pages, read again only when it is not the one
    /// shown already.
    fn show_pdf(self: &Rc<Self>, path: &Path) -> Result<(), String> {
        if self.pdf.borrow().as_ref().is_some_and(|pdf| pdf.path == path) {
            self.stack.set_visible_child_name("pdf");
            return Ok(());
        }
        let password = self.passwords.borrow().get(path).cloned();
        let document = open_pdf(path, password.as_deref()).map_err(|err| err.to_string())?;
        let sizes: Vec<(f64, f64)> =
            (0..document.n_pages()).map(|index| document.page(index).map_or((1.0, 1.0), |page| page.size())).collect();
        let pages = (0..sizes.len())
            .map(|index| {
                let area = gtk::DrawingArea::builder().css_classes(["pv-sheet"]).valign(gtk::Align::Start).build();
                let weak = Rc::downgrade(self);
                area.set_draw_func(move |_, cr, _, _| {
                    if let Some(win) = weak.upgrade() {
                        win.draw_page(index, cr);
                    }
                });
                area
            })
            .collect();
        let pdf = Rc::new(Pdf {
            path: path.to_owned(),
            document: RefCell::new(document),
            password,
            pages,
            sizes,
            scale: Cell::new(0.0),
            undrawn: RefCell::default(),
            layout: Cell::new(self.layout.get()),
            spread: Cell::new(0),
            dirty: Cell::new(false),
            undo: RefCell::default(),
            redo: RefCell::default(),
        });
        self.lay_out_pages(&pdf);
        *self.pdf.borrow_mut() = Some(pdf);
        self.selection.set(None);
        self.pending.set(None);
        self.redactions.borrow_mut().clear();
        self.stack.set_visible_child_name("pdf");
        self.find();
        self.fill_sidebar();
        Ok(())
    }

    /// Draws page `index` with what is chosen, found and being drawn on it; a page out of view
    /// is left white until it is scrolled to.
    fn draw_page(&self, index: usize, cr: &gtk::cairo::Context) {
        let Some(pdf) = self.pdf.borrow().clone() else { return };
        cr.set_source_rgb(1.0, 1.0, 1.0);
        let _ = cr.paint();
        if pdf.scale.get() == 0.0 || !self.page_in_view(&pdf, index) {
            pdf.undrawn.borrow_mut().insert(index);
            return;
        }
        let Some(page) = pdf.document().page(index as i32) else { return };
        let scale = pdf.scale.get();
        cr.scale(scale, scale);
        page.render(cr);
        if self.dark_pdf.get() {
            // White paper turns dark and black text light, the pictures inverted with them.
            cr.set_operator(gtk::cairo::Operator::Difference);
            cr.set_source_rgb(1.0, 1.0, 1.0);
            let _ = cr.paint();
            cr.set_operator(gtk::cairo::Operator::Over);
        }
        if let Some(selection) = self.selection.get() {
            if let Some(mut area) = selection.on(index, pdf.sizes[index]) {
                let colour = |red: u16, green: u16, blue: u16| {
                    let mut colour = poppler::Color::new();
                    colour.set_red(red);
                    colour.set_green(green);
                    colour.set_blue(blue);
                    colour
                };
                // The accent's blue, with the text over it in white.
                let (mut glyph, mut background) = (colour(0xffff, 0xffff, 0xffff), colour(0x3535, 0x8484, 0xe4e4));
                let mut before = poppler::Rectangle::new();
                page.render_selection(cr, &mut area, &mut before, selection.style, &mut glyph, &mut background);
            }
        }
        let current = self.match_at.get();
        for (at, (_, (x1, y1, x2, y2))) in self.matches.borrow().iter().enumerate().filter(|(_, (on, _))| *on == index) {
            if Some(at) == current {
                cr.set_source_rgba(1.0, 0.6, 0.0, 0.5);
            } else {
                cr.set_source_rgba(1.0, 0.85, 0.0, 0.35);
            }
            cr.rectangle(*x1, *y1, x2 - x1, y2 - y1);
            let _ = cr.fill();
        }
        self.draw_page_marks(index, &page, cr);
    }

    /// Whether page `index` is in the column's view, or a screen away from it.
    fn page_in_view(&self, pdf: &Pdf, index: usize) -> bool {
        if pdf.layout.get() != Layout::Continuous {
            return pdf.rows().iter().any(|row| row.contains(&index));
        }
        let adjustment = self.pdf_scroll.vadjustment();
        let scale = pdf.scale.get();
        let (top, view) = (pdf.top(index, scale), adjustment.page_size());
        top + pdf.height(index, scale) >= adjustment.value() - view && top <= adjustment.value() + 2.0 * view
    }

    /// The column was scrolled: the pages come into view are drawn, and the page most of the
    /// view shows is the one chosen in the sidebar.
    fn scrolled(&self) {
        let Some(pdf) = self.pdf.borrow().clone() else { return };
        let come: Vec<usize> = pdf.undrawn.borrow().iter().copied().filter(|&index| self.page_in_view(&pdf, index)).collect();
        for index in come {
            pdf.undrawn.borrow_mut().remove(&index);
            pdf.pages[index].queue_draw();
        }
        // A page or two at a time, the pages shown are those chosen, not those scrolled to.
        if self.jumping.get() || pdf.scale.get() == 0.0 || pdf.layout.get() != Layout::Continuous {
            return;
        }
        let adjustment = self.pdf_scroll.vadjustment();
        let (from, to) = (adjustment.value(), adjustment.value() + adjustment.page_size());
        let scale = pdf.scale.get();
        let (mut top, mut most, mut shown) = (f64::from(EDGE), 0, 0.0);
        for index in 0..pdf.sizes.len() {
            let bottom = top + pdf.height(index, scale);
            let seen = bottom.min(to) - top.max(from);
            if seen > shown {
                (most, shown) = (index, seen);
            }
            if top > to {
                break;
            }
            top = bottom + f64::from(EDGE);
        }
        self.list_page(&pdf, most);
    }

    /// The index of the item that is page `page` of the PDF shown.
    fn item_of_page(&self, pdf: &Pdf, page: usize) -> Option<usize> {
        let first = self.items.borrow().iter().position(|item| item.path == pdf.path && item.page == Some(1))?;
        Some(first + page)
    }

    /// Selects the sidebar row of a page of the PDF shown, without scrolling to it.
    fn list_page(&self, pdf: &Pdf, page: usize) {
        let Some(index) = self.item_of_page(pdf, page) else { return };
        if index == self.current.get() {
            return;
        }
        self.current.set(index);
        self.syncing.set(true);
        if let Some(row) = self.list.row_at_index(index as i32) {
            self.list.unselect_all();
            self.list.select_row(Some(&row));
        }
        self.syncing.set(false);
        if let Some(item) = self.item() {
            self.update_title(&item);
        }
        self.update_actions();
    }

    /// Scrolls the column to where `pending` says, once it is as tall as its pages.
    fn apply_pending(&self) {
        let (Some((index, down, more)), Some(pdf)) = (self.pending.get(), self.pdf.borrow().clone()) else { return };
        let scale = pdf.scale.get();
        let adjustment = self.pdf_scroll.vadjustment();
        if scale == 0.0 || index >= pdf.sizes.len() || adjustment.upper() + 0.5 < pdf.top(pdf.sizes.len(), scale) {
            return;
        }
        self.pending.set(None);
        self.jumping.set(true);
        adjustment.set_value(pdf.top(index, scale) + down * pdf.height(index, scale) + more);
        self.jumping.set(false);
    }

    /// The place the column is scrolled to, as `pending` keeps it, to be kept through a zoom.
    fn scrolled_to(&self, pdf: &Pdf) -> (usize, f64, f64) {
        let scale = pdf.scale.get();
        let value = self.pdf_scroll.vadjustment().value();
        let index = pdf.page_at(value, scale);
        let into = value - pdf.top(index, scale);
        if into < 0.0 {
            (index, 0.0, into)
        } else {
            (index, into / pdf.height(index, scale).max(1.0), 0.0)
        }
    }

    /// Where `(x, y)` over the column is: a page, and a point on it in points. A point between
    /// pages is the end of the one above.
    fn hit(&self, x: f64, y: f64) -> Option<(usize, f64, f64)> {
        let (index, px, py) = self.hit_loose(x, y)?;
        let pdf = self.pdf.borrow().clone()?;
        let (width, height) = pdf.sizes[index];
        Some((index, px.clamp(0.0, width), py.clamp(0.0, height)))
    }

    /// Where `(x, y)` over the column is on the page in that row nearest it, in points, which
    /// may be past the page's edges.
    fn hit_loose(&self, x: f64, y: f64) -> Option<(usize, f64, f64)> {
        let pdf = self.pdf.borrow().clone()?;
        let scale = pdf.scale.get();
        if scale == 0.0 {
            return None;
        }
        let first = pdf.page_at(y, scale);
        let row = pdf.rows().into_iter().find(|row| row.contains(&first))?;
        let mut best: Option<(f64, (usize, f64, f64))> = None;
        for index in row {
            let point = self.pdf_column.compute_point(&pdf.pages[index], &gtk::graphene::Point::new(x as f32, y as f32))?;
            let (px, py) = (f64::from(point.x()) / scale, f64::from(point.y()) / scale);
            let width = pdf.sizes[index].0;
            let off = if px < 0.0 { -px } else if px > width { px - width } else { 0.0 };
            if best.as_ref().is_none_or(|(distance, _)| off < *distance) {
                best = Some((off, (index, px, py)));
            }
        }
        best.map(|(_, hit)| hit)
    }

    fn set_selection(&self, selection: Option<Selection>) {
        let before = self.selection.replace(selection);
        if let Some(pdf) = self.pdf.borrow().clone() {
            for index in before.iter().chain(selection.iter()).flat_map(Selection::pages) {
                if let Some(page) = pdf.pages.get(index) {
                    page.queue_draw();
                }
            }
        }
        self.update_actions();
    }

    /// Looks for 搜尋's text through the PDF shown, a page at a time so the window stays free;
    /// the first match from the page shown on is then scrolled to.
    fn find(self: &Rc<Self>) {
        let text = self.find_entry.text().to_string();
        let finding = self.finding.get().wrapping_add(1);
        self.finding.set(finding);
        self.matches.borrow_mut().clear();
        self.match_at.set(None);
        self.find_count.set_visible(false);
        self.redraw_pages();
        let Some(pdf) = self.pdf.borrow().clone() else { return };
        if text.is_empty() {
            return;
        }
        let chosen = |name: &str| self.window.action_state(name).and_then(|state| state.str().map(str::to_owned)).unwrap_or_default();
        // 任何符合 finds each word on its own; 完全符合, the whole of it.
        let terms: Vec<String> = match chosen("find-words").as_str() {
            "any" => text.split_whitespace().map(str::to_owned).collect(),
            _ => vec![text],
        };
        let by_rank = chosen("find-order") == "rank";
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let mut found = Vec::new();
            let document = pdf.document();
            for index in 0..pdf.sizes.len() {
                if weak.upgrade().is_none_or(|win| win.finding.get() != finding) {
                    return;
                }
                if let Some(page) = document.page(index as i32) {
                    for term in &terms {
                        found.extend(page_matches(&page, term).into_iter().map(|place| (index, place)));
                    }
                }
                glib::timeout_future(std::time::Duration::ZERO).await;
            }
            let found = order_matches(found, by_rank);
            let Some(win) = weak.upgrade().filter(|win| win.finding.get() == finding) else { return };
            let shown = win.item().and_then(|item| item.page).map_or(0, |page| page as usize - 1);
            // 搜尋等級 starts from the page with the most; 頁面順序, from the page shown.
            let at = if by_rank { None } else { found.iter().position(|(page, _)| *page >= shown) }.or((!found.is_empty()).then_some(0));
            *win.matches.borrow_mut() = found;
            win.match_at.set(at);
            win.show_match();
        });
    }

    fn step_match(&self, by: i64) {
        let count = self.matches.borrow().len() as i64;
        if count == 0 {
            return;
        }
        let at = self.match_at.get().map_or(0, |at| (at as i64 + by).rem_euclid(count));
        self.match_at.set(Some(at as usize));
        self.show_match();
    }

    /// Scrolls to the match chosen, a third of the way down the view, and says which it is.
    fn show_match(&self) {
        let count = self.matches.borrow().len();
        self.find_count.set_label(&match self.match_at.get() {
            Some(at) => format!("{} / {count}", at + 1),
            None => String::from("找不到"),
        });
        self.find_count.set_visible(!self.find_entry.text().is_empty());
        let found = self.match_at.get().and_then(|at| self.matches.borrow().get(at).copied());
        if let (Some((page, (_, y1, _, _))), Some(pdf)) = (found, self.pdf.borrow().clone()) {
            self.show_spread(&pdf, page);
            let view = self.pdf_scroll.vadjustment().page_size();
            self.pending.set(Some((page, y1 / pdf.sizes[page].1, -view / 3.0)));
            self.apply_pending();
            self.list_page(&pdf, page);
        }
        self.redraw_pages();
    }

    /// 使用所選範圍尋找: the text chosen is looked for.
    fn find_selection(self: &Rc<Self>) {
        let (Some(pdf), Some(selection)) = (self.pdf.borrow().clone(), self.selection.get()) else { return };
        let text = selected_text(&pdf.document(), &selection);
        if !text.trim().is_empty() {
            self.find_entry.set_text(text.trim());
        }
    }

    /// 跳到所選範圍: the text chosen is scrolled to.
    fn jump_selection(&self) {
        let (Some(pdf), Some(selection)) = (self.pdf.borrow().clone(), self.selection.get()) else { return };
        let (page, _, y) = selection.from;
        self.show_spread(&pdf, page);
        let view = self.pdf_scroll.vadjustment().page_size();
        self.pending.set(Some((page, y / pdf.sizes[page].1, -view / 3.0)));
        self.apply_pending();
    }

    fn redraw_pages(&self) {
        if let Some(pdf) = self.pdf.borrow().clone() {
            pdf.pages.iter().for_each(|page| page.queue_draw());
        }
    }

    fn fail(&self, item: &Item, reason: &str) {
        *self.shown.borrow_mut() = None;
        self.show_nothing();
        alert::notice(&self.window, "無法打開", Some(&format!("{}\n{reason}", item.path.display())));
    }

    fn show(self: &Rc<Self>, item: &Item, pixbuf: Pixbuf) {
        // The sidebar's thumbnail follows what was turned, cropped or drawn on.
        if let (Some(picture), true) = (self.thumbnails.borrow().get(self.current.get()), item.edited.borrow().is_some()) {
            if let Some(small) = thumbnail_of(&pixbuf) {
                picture.set_size_request(small.width(), small.height());
                picture.set_paintable(Some(&gdk::Texture::for_pixbuf(&small)));
            }
        }
        self.picture.set_paintable(Some(&gdk::Texture::for_pixbuf(&pixbuf)));
        *self.shown.borrow_mut() = Some(pixbuf);
        if self.sidebar_mode.get() != Sidebar::ContactSheet {
            self.stack.set_visible_child_name("view");
        }
        self.show_tools();
        self.apply_zoom();
        self.update_title(item);
        self.update_actions();
    }

    /// The markup bar's and header's buttons the item shown has.
    fn show_tools(self: &Rc<Self>) {
        let is_page = self.item().is_some_and(|item| item.is_page());
        for tool in &self.picture_tools {
            tool.set_visible(!is_page);
        }
        for tool in &self.pdf_tools {
            tool.set_visible(is_page);
        }
        self.highlight_button.set_visible(is_page);
        // A tool only the other kind has is put down.
        if let Some(tool) = self.tool.get() {
            let pdf_only = matches!(tool, Tool::Text | Tool::Redact | Tool::Note | Tool::FormText);
            let picture_only = matches!(tool, Tool::Ellipse | Tool::Lasso | Tool::SmartLasso | Tool::InstantAlpha);
            if (is_page && picture_only) || (!is_page && pdf_only) {
                self.set_tool(None);
            }
        }
    }

    fn step(self: &Rc<Self>, by: i32) {
        // A page or two at a time, the spread shown goes on.
        if let Some(pdf) = self.pdf.borrow().clone().filter(|_| self.item().is_some_and(|item| item.is_page())) {
            if pdf.layout.get() != Layout::Continuous {
                let step = if pdf.layout.get() == Layout::Two { 2 } else { 1 };
                let next = pdf.spread.get() as i64 + i64::from(by) * step;
                if (0..pdf.sizes.len() as i64).contains(&next) {
                    self.go_page(next as usize, None);
                    return;
                }
            }
        }
        let count = self.items.borrow().len() as i32;
        let next = self.current.get() as i32 + by;
        if (0..count).contains(&next) {
            self.select(next as usize);
        }
    }

    fn update_title(&self, item: &Item) {
        let locked = !item.untitled && is_locked(&item.path);
        self.set_action_state("lock", locked.to_variant());
        let edited = if self.is_edited(item) {
            " — 已編輯"
        } else if locked {
            " — 已鎖定"
        } else {
            ""
        };
        self.title.set_title(&format!("{}{edited}", item.name()));
        let subtitle = match (item.page, self.shown.borrow().as_ref()) {
            (Some(page), _) => {
                let label = self.pdf.borrow().as_ref().filter(|pdf| pdf.path == item.path).map(|pdf| {
                    pdfdoc::page_label(&pdf.document(), page as usize - 1, self.prefs.borrow().logical_numbers)
                });
                format!("第 {} 頁，共 {} 頁", label.unwrap_or_else(|| page.to_string()), self.pages_in(&item.path))
            }
            (None, Some(pixbuf)) => format!("{} × {}", pixbuf.width(), pixbuf.height()),
            (None, None) => String::new(),
        };
        self.title.set_subtitle(&subtitle);
        self.window.set_title(Some(&item.name()));
    }

    /// Whether `item` has changes not yet saved.
    fn is_edited(&self, item: &Item) -> bool {
        match item.page {
            None => item.edited.borrow().is_some(),
            Some(_) => self.pdf.borrow().as_ref().is_some_and(|pdf| pdf.path == item.path && pdf.dirty.get()),
        }
    }

    /// How many pages of the PDF at `path` the window has.
    fn pages_in(&self, path: &Path) -> usize {
        self.items.borrow().iter().filter(|item| item.is_page() && item.path == path).count()
    }

    fn update_actions(&self) {
        let item = self.item().filter(|item| match item.page {
            Some(_) => self.pdf.borrow().as_ref().is_some_and(|pdf| pdf.path == item.path),
            None => self.shown.borrow().is_some(),
        });
        let is_picture = item.as_ref().is_some_and(|item| !item.is_page());
        let is_page = item.as_ref().is_some_and(|item| item.is_page());
        let pdf = self.pdf.borrow().clone().filter(|_| is_page);
        // The page actions write the PDF itself, which is not an Illustrator file's.
        let is_pdf_file = item.as_ref().is_some_and(|item| is_pdf(&item.path) && !item.untitled);
        let edited = item.as_ref().is_some_and(|item| self.is_edited(item));
        let untitled = item.as_ref().is_some_and(|item| item.untitled);
        let several = self.items.borrow().len() > 1;
        let typing = self.typing.get();
        let has_mask = self.image_mask.borrow().as_ref().is_some_and(|mask| !mask.is_empty());
        let has_text = self.selection.get().is_some();
        let has_area = self.area.get().is_some();
        let has_chosen = self.chosen.borrow().is_some();
        let can_undo = match (&pdf, &item) {
            (Some(pdf), _) => !pdf.undo.borrow().is_empty(),
            (None, Some(item)) => !item.undo.borrow().is_empty(),
            _ => false,
        };
        let can_redo = match (&pdf, &item) {
            (Some(pdf), _) => !pdf.redo.borrow().is_empty(),
            (None, Some(item)) => !item.redo.borrow().is_empty(),
            _ => false,
        };
        let has_history = !self.back.borrow().is_empty();
        let has_future = !self.forward.borrow().is_empty();
        let paths: HashSet<PathBuf> = self.items.borrow().iter().map(|item| item.path.clone()).collect();
        self.find_box.set_visible(is_page);
        for (name, on) in [
            ("save", edited || untitled),
            ("duplicate", item.is_some()),
            ("save-as", item.is_some()),
            ("rename", item.is_some() && !untitled),
            ("move-to", item.is_some() && !untitled),
            ("revert-saved", edited && !untitled),
            ("browse-versions", item.is_some() && !untitled),
            ("export", item.is_some()),
            ("export-pdf", item.is_some()),
            ("enter-password", is_page),
            ("edit-permissions", is_page && is_pdf_file),
            ("print", item.is_some()),
            ("close-selected", item.is_some()),
            ("trash", item.is_some() && !untitled),
            ("undo", !typing && can_undo),
            ("redo", !typing && can_redo),
            ("cut", !typing && is_picture && has_mask),
            ("copy", !typing && (is_picture || has_text || has_area)),
            ("paste", !typing && item.is_some()),
            ("delete", !typing && (has_mask || has_chosen || (is_page && self.list.focus_child().is_some()))),
            ("select-all", !typing && item.is_some()),
            ("select-none", !typing && (has_mask || has_text || has_area)),
            ("invert-selection", !typing && is_picture && has_mask),
            ("insert-pdf", is_page && is_pdf_file),
            ("insert-blank", is_page && is_pdf_file),
            ("insert-scanner", is_page && is_pdf_file),
            ("delete-page", is_page && is_pdf_file && self.pages_in(&item.as_ref().map(|item| item.path.clone()).unwrap_or_default()) > 1),
            ("page-up", is_page && is_pdf_file && item.as_ref().and_then(|item| item.page).is_some_and(|page| page > 1)),
            (
                "page-down",
                is_page && is_pdf_file && item.as_ref().is_some_and(|item| item.page.is_some_and(|page| (page as usize) < self.pages_in(&item.path))),
            ),
            ("find", is_page),
            ("find-next", is_page),
            ("find-previous", is_page),
            ("find-selection", is_page && has_text),
            ("jump-selection", is_page && has_text),
            ("zoom-in", item.is_some()),
            ("zoom-out", item.is_some()),
            ("zoom-fit", item.is_some()),
            ("zoom-actual", item.is_some()),
            ("zoom-toggle", item.is_some()),
            ("zoom-selection", has_mask || has_area || has_text),
            ("slideshow", item.is_some()),
            ("previous", !typing && (several || is_page)),
            ("next", !typing && (several || is_page)),
            ("previous-document", paths.len() > 1),
            ("next-document", paths.len() > 1),
            ("first-page", !typing && is_page),
            ("last-page", !typing && is_page),
            ("go-to-page", is_page),
            ("back", has_history),
            ("forward", has_future),
            ("info", item.is_some()),
            ("adjust-color", is_picture),
            ("resize", is_picture),
            ("assign-profile", is_picture && !untitled),
            ("describe", is_picture && !untitled),
            ("bookmark", is_page && !untitled),
            ("rotate-left", is_picture || is_page && is_pdf_file),
            ("rotate-right", is_picture || is_page && is_pdf_file),
            ("flip", is_picture),
            ("flip-vertical", is_picture),
            ("crop", is_picture && has_mask || is_page && is_pdf_file && has_area),
            ("remove-background", is_picture),
            ("copy-subject", !typing && is_picture && PREFS.with(|prefs| prefs.borrow().copy_subject)),
            ("highlight", is_page),
            ("mark", is_page),
            ("mark-colour", is_page),
            ("markup", item.is_some()),
            ("loupe", !typing && item.is_some()),
            ("dark-pdf", true),
            ("image-background", true),
        ] {
            if let Some(action) = self.window.lookup_action(name).and_downcast::<gio::SimpleAction>() {
                action.set_enabled(on);
            }
        }
    }

    /// How many screen pixels a pixel of the picture, or a point of the PDF, takes now.
    fn scale(&self) -> f64 {
        let Some(item) = self.item() else { return 1.0 };
        if let Zoom::Scale(scale) = self.zoom.get() {
            return scale * self.actual_size(&item);
        }
        if item.is_page() {
            let Some(pdf) = self.pdf.borrow().clone() else { return 1.0 };
            // The widest row fills the window's width, or a page or two at a time, the whole
            // spread fits.
            let rows = pdf.rows();
            let widest = rows
                .iter()
                .map(|row| row.iter().map(|&index| pdf.sizes[index].0).sum::<f64>() + f64::from(EDGE) * (row.len() as f64 - 1.0).max(0.0))
                .fold(1.0, f64::max);
            let across = self.pdf_scroll.hadjustment().page_size() - f64::from(2 * EDGE);
            let mut scale = across / widest;
            if pdf.layout.get() != Layout::Continuous {
                let tallest = rows.iter().map(|row| row.iter().map(|&index| pdf.sizes[index].1).fold(1.0, f64::max)).fold(1.0, f64::max);
                let down = self.pdf_scroll.vadjustment().page_size() - f64::from(2 * EDGE);
                scale = scale.min(down / tallest);
            }
            return scale.clamp(MIN_SCALE, MAX_SCALE);
        }
        let Some(pixbuf) = self.shown.borrow().clone() else { return 1.0 };
        let across = self.scroll.hadjustment().page_size() - f64::from(2 * EDGE);
        let down = self.scroll.vadjustment().page_size() - f64::from(2 * EDGE);
        // A picture smaller than the window stays as it is.
        (across / f64::from(pixbuf.width())).min(down / f64::from(pixbuf.height())).clamp(MIN_SCALE, self.actual_size(&item))
    }

    /// The scale 實際大小 is: a pixel or a point to a screen pixel, or the size it prints at
    /// when 設定 says so, a screen being taken at 96 pixels to the inch.
    fn actual_size(&self, item: &Item) -> f64 {
        let prefs = self.prefs.borrow();
        match item.page {
            Some(_) if prefs.pdf_print_size => 96.0 / 72.0,
            None if prefs.image_print_size => 96.0 / item.dpi.get().unwrap_or(72.0),
            _ => 1.0,
        }
    }

    fn apply_zoom(&self) {
        let Some(item) = self.item() else { return };
        let scale = self.scale();
        let fit = self.zoom.get() == Zoom::Fit;
        self.zoom_button.set_label(&format!("{}%", (scale / self.actual_size(&item) * 100.0).round()));
        self.zoom_button.set_tooltip_text(Some(if fit { "實際大小" } else { "符合視窗大小" }));
        if item.is_page() {
            let Some(pdf) = self.pdf.borrow().clone() else { return };
            self.pdf_scroll.set_hscrollbar_policy(if fit { gtk::PolicyType::Never } else { gtk::PolicyType::Automatic });
            // Before the window has a width there is nothing to fit, and pages laid out tiny
            // would lose the place to scroll to.
            let no_width = fit && self.pdf_scroll.hadjustment().page_size() <= 0.0;
            if no_width || (pdf.scale.get() - scale).abs() < 1e-6 {
                return;
            }
            // The place read stays in view through the zoom.
            if pdf.scale.get() > 0.0 && self.pending.get().is_none() {
                self.pending.set(Some(self.scrolled_to(&pdf)));
            }
            pdf.scale.set(scale);
            for (page, &(width, height)) in pdf.pages.iter().zip(&pdf.sizes) {
                page.set_size_request(((width * scale).round() as i32).max(1), ((height * scale).round() as i32).max(1));
                page.queue_draw();
            }
            self.apply_pending();
            return;
        }
        let Some(pixbuf) = self.shown.borrow().clone() else { return };
        let policy = if fit { gtk::PolicyType::Never } else { gtk::PolicyType::Automatic };
        self.scroll.set_hscrollbar_policy(policy);
        self.scroll.set_vscrollbar_policy(policy);
        self.picture.set_size_request(
            ((f64::from(pixbuf.width()) * scale).round() as i32).max(1),
            ((f64::from(pixbuf.height()) * scale).round() as i32).max(1),
        );
        self.mark_layer.queue_draw();
    }

    fn set_zoom(&self, zoom: Zoom) {
        let actual = self.item().map_or(1.0, |item| self.actual_size(&item));
        self.zoom.set(match zoom {
            Zoom::Scale(scale) => Zoom::Scale(scale.clamp(MIN_SCALE / actual, MAX_SCALE / actual)),
            Zoom::Fit => Zoom::Fit,
        });
        self.apply_zoom();
    }

    fn zoom_by(&self, factor: f64) {
        let actual = self.item().map_or(1.0, |item| self.actual_size(&item));
        self.set_zoom(Zoom::Scale(self.scale() / actual * factor));
    }

    /// 實際大小: as 設定 says 100% is.
    fn set_actual(&self) {
        self.set_zoom(Zoom::Scale(1.0));
    }

    /// The text chosen in a PDF or the part of a page, or the picture or the part chosen.
    fn copy(&self) {
        let pdf = self.pdf.borrow().clone().filter(|_| self.item().is_some_and(|item| item.is_page()));
        if let Some(pdf) = pdf {
            if let Some(selection) = self.selection.get() {
                let text = selected_text(&pdf.document(), &selection);
                if !text.is_empty() {
                    self.window.clipboard().set_text(&text);
                }
            } else if let Some(texture) = self.area_texture(&pdf) {
                self.window.clipboard().set_texture(&texture);
            }
            return;
        }
        let Some(pixbuf) = self.shown.borrow().clone() else { return };
        let part = self.image_mask.borrow().as_ref().and_then(|mask| imaging::cut(&imaging::Rgba::from_pixbuf(&pixbuf), mask));
        let pixbuf = part.map_or(pixbuf, |part| part.to_pixbuf());
        self.window.clipboard().set_texture(&gdk::Texture::for_pixbuf(&pixbuf));
    }

    /// The part of a page chosen with 矩形選取, drawn at twice its size.
    fn area_texture(&self, pdf: &Pdf) -> Option<gdk::Texture> {
        let (index, (x1, y1, x2, y2)) = self.area.get()?;
        let page = pdf.document().page(index as i32)?;
        const SCALE: f64 = 2.0;
        let (width, height) = ((((x2 - x1) * SCALE).round() as i32).max(1), (((y2 - y1) * SCALE).round() as i32).max(1));
        let mut surface = gtk::cairo::ImageSurface::create(gtk::cairo::Format::ARgb32, width, height).ok()?;
        {
            let cr = gtk::cairo::Context::new(&surface).ok()?;
            cr.set_source_rgb(1.0, 1.0, 1.0);
            cr.paint().ok()?;
            cr.scale(SCALE, SCALE);
            cr.translate(-x1, -y1);
            page.render(&cr);
        }
        surface.flush();
        let stride = surface.stride() as usize;
        let bytes = glib::Bytes::from(&surface.data().ok()?[..]);
        Some(gdk::MemoryTexture::new(width, height, gdk::MemoryFormat::B8g8r8a8Premultiplied, &bytes, stride).upcast())
    }

    /// 全選: all of a PDF's text or all of a picture.
    fn select_all(&self) {
        if let Some(pixbuf) = self.shown.borrow().clone().filter(|_| self.item().is_some_and(|item| !item.is_page())) {
            let (width, height) = (pixbuf.width() as usize, pixbuf.height() as usize);
            let all = (0.0, 0.0, width as f64, height as f64);
            *self.image_mask.borrow_mut() = Some(imaging::Mask::rectangle(width, height, all));
            *self.image_outline.borrow_mut() = vec![(0.0, 0.0), (all.2, 0.0), (all.2, all.3), (0.0, all.3), (0.0, 0.0)];
            self.mark_layer.queue_draw();
            return self.update_actions();
        }
        let Some(pdf) = self.pdf.borrow().clone() else { return };
        let Some(&size) = pdf.sizes.last() else { return };
        let last = pdf.sizes.len() - 1;
        self.set_selection(Some(Selection { from: (0, 0.0, 0.0), to: (last, size.0, size.1), style: poppler::SelectionStyle::Glyph }));
    }

    fn select_none(&self) {
        self.set_mask(None, Vec::new());
        self.area.set(None);
        self.mark_layer.queue_draw();
        self.set_selection(None);
        self.redraw_pages();
    }

    /// Escape: ends what is under way, the slideshow, the magnifier, or what is chosen.
    fn cancel(self: &Rc<Self>) {
        if self.slideshow.borrow().is_some() {
            return self.end_slideshow();
        }
        if self.loupe.get() {
            self.set_action_state("loupe", false.to_variant());
            return self.set_loupe(false);
        }
        if self.window.is_fullscreen() {
            return self.window.unfullscreen();
        }
        if !self.stroke.borrow().is_empty() {
            self.stroke.take();
            self.mark_layer.queue_draw();
            return self.redraw_pages();
        }
        if self.chosen.take().is_some() {
            self.redraw_pages();
            return self.update_actions();
        }
        self.select_none();
    }
}

/// `pixbuf` the size of a thumbnail.
fn thumbnail_of(pixbuf: &Pixbuf) -> Option<Pixbuf> {
    let ratio = f64::from(THUMB_SIZE) / f64::from(pixbuf.width().max(pixbuf.height()));
    let size = |side: i32| ((f64::from(side) * ratio).round() as i32).max(1);
    pixbuf.scale_simple(size(pixbuf.width()), size(pixbuf.height()), gtk::gdk_pixbuf::InterpType::Bilinear)
}

fn layout_key(layout: Layout) -> &'static str {
    match layout {
        Layout::Continuous => "continuous",
        Layout::Single => "single",
        Layout::Two => "two",
    }
}

fn layout_from_key(key: &str) -> Layout {
    match key {
        "single" => Layout::Single,
        "two" => Layout::Two,
        _ => Layout::Continuous,
    }
}

/// Puts `path` first in 打開最近使用的檔案, and in the desktop's own list of recent files.
fn remember_recent(path: &Path) {
    let uri = gio::File::for_path(path).uri();
    gtk::RecentManager::default().add_item(&uri);
    RECENT.with(|recent| {
        let target = path.to_string_lossy();
        for index in (0..recent.n_items()).rev() {
            let same = recent
                .item_attribute_value(index, "target", Some(glib::VariantTy::STRING))
                .and_then(|value| value.str().map(str::to_owned))
                .is_some_and(|value| value == target);
            if same {
                recent.remove(index);
            }
        }
        let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        let item = gio::MenuItem::new(Some(&name), None);
        item.set_action_and_target_value(Some("win.open-recent"), Some(&target.to_variant()));
        recent.prepend_item(&item);
        while recent.n_items() > 10 {
            recent.remove(recent.n_items() - 1);
        }
    });
}

/// The files the desktop lists as used lately that 預覽程式 opens, newest first.
fn fill_recent() {
    let mut items: Vec<gtk::RecentInfo> = gtk::RecentManager::default()
        .items()
        .into_iter()
        .filter(|info| MIME_TYPES.contains(&info.mime_type().as_str()) && info.exists())
        .collect();
    items.sort_by_key(|info| std::cmp::Reverse(info.modified().to_unix()));
    let paths: Vec<PathBuf> = items.iter().filter_map(|info| gio::File::for_uri(&info.uri()).path()).take(10).collect();
    for path in paths.iter().rev() {
        remember_recent(path);
    }
}

/// A sidebar row: the thumbnail, filled in when it is drawn, and a name or a page number.
fn thumbnail_row(item: &Item, index: usize) -> (gtk::ListBoxRow, gtk::Picture) {
    let picture = gtk::Picture::builder()
        .content_fit(gtk::ContentFit::Fill)
        .can_shrink(true)
        .width_request(THUMB_SIZE * 3 / 4)
        .height_request(THUMB_SIZE * 3 / 4)
        .halign(gtk::Align::Center)
        .css_classes(["pv-sheet"])
        .build();
    let label = gtk::Label::builder()
        .label(match item.page {
            Some(page) => page.to_string(),
            None => item.name(),
        })
        .ellipsize(pango::EllipsizeMode::Middle)
        .css_classes(["caption"])
        .build();
    let column = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(GAP)
        .margin_top(GAP)
        .margin_bottom(GAP)
        .margin_start(GAP)
        .margin_end(GAP)
        .build();
    column.append(&picture);
    column.append(&label);
    let row = gtk::ListBoxRow::builder().child(&column).build();
    row.update_property(&[gtk::accessible::Property::Label(&format!("{}", index + 1))]);
    (row, picture)
}

fn css() -> &'static str {
    "
.pv-sheet { box-shadow: 0 0 0 1px alpha(currentColor, .12), 0 1px 4px alpha(black, .2); }
.pv-checker {
  background-color: white;
  background-image: linear-gradient(45deg, #ccc 25%, transparent 25%, transparent 75%, #ccc 75%),
                    linear-gradient(45deg, #ccc 25%, transparent 25%, transparent 75%, #ccc 75%);
  background-size: 16px 16px;
  background-position: 0 0, 8px 8px;
}
.pv-slideshow { background: black; }
.pv-chosen { background: alpha(@accent_bg_color, .25); border-radius: 6px; }
"
}

fn main() -> glib::ExitCode {
    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_OPEN)
        .build();
    app.connect_startup(|app| {
        let provider = gtk::CssProvider::new();
        provider.load_from_string(css());
        if let Some(display) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
        }
        fill_recent();
        RECENT.with(|recent| app.set_menubar(Some(&menus::menubar(recent))));
        menus::bind_keys(app);
        settings::add_app_actions(app);
    });
    app.connect_shutdown(|_| {
        // Only the pages this run drew.
        let _ = std::fs::remove_dir_all(scratch_dir());
    });
    app.connect_activate(|app| match app.active_window() {
        Some(window) => window.present(),
        None => {
            Win::open(app);
        }
    });
    app.connect_open(|app, files, _| {
        let paths: Vec<PathBuf> = files.iter().filter_map(|file| file.path()).collect();
        // Files opened together share a window, or each has its own, as 設定 says.
        if PREFS.with(|prefs| prefs.borrow().group_files) {
            Win::open(app).open_files(paths);
        } else {
            for path in paths {
                Win::open(app).open_files(vec![path]);
            }
        }
    });
    app.run()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(pixbuf: &Pixbuf, x: i32, y: i32) -> [u8; 3] {
        let at = (y * pixbuf.rowstride() + x * pixbuf.n_channels()) as usize;
        let bytes = pixbuf.read_pixel_bytes();
        [bytes[at], bytes[at + 1], bytes[at + 2]]
    }

    #[test]
    fn matches_read_in_order_or_by_the_pages_with_most() {
        let at = |page, y| (page, (0.0, y, 1.0, y + 1.0));
        // Two words found apart, one place found by both.
        let found = vec![at(2, 5.0), at(0, 9.0), at(2, 1.0), at(1, 3.0), at(2, 5.0), at(1, 1.0)];
        assert_eq!(order_matches(found.clone(), false), vec![at(0, 9.0), at(1, 1.0), at(1, 3.0), at(2, 1.0), at(2, 5.0)]);
        assert_eq!(order_matches(found, true), vec![at(1, 1.0), at(1, 3.0), at(2, 1.0), at(2, 5.0), at(0, 9.0)]);
    }

    #[test]
    fn formats_gdk_pixbuf_cannot_read_come_through_imagemagick() {
        if Command::new("magick").arg("-version").output().is_err() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("nekoland-preview-load-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // A script that only says it is a picture is not read as one.
        let script = dir.join("script.hdr");
        std::fs::write(&script, "push graphic-context\nviewbox 0 0 20 10\nfill 'url(https://127.0.0.1/x)'\npop graphic-context\n").unwrap();
        assert!(load_picture(&script).is_err());
        for extension in ["hdr", "sgi", "exr", "pict"] {
            let path = dir.join(format!("picture.{extension}"));
            let made = Command::new("magick").args(["-size", "20x10", "xc:red"]).arg(&path).output().unwrap();
            if !made.status.success() {
                continue;
            }
            let pixbuf = load_picture(&path).unwrap_or_else(|err| panic!("{extension}: {err}"));
            assert_eq!((pixbuf.width(), pixbuf.height()), (20, 10), "{extension}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_rectangle_is_drawn_in_the_colour_and_leaves_the_rest() {
        let blank = Pixbuf::new(gtk::gdk_pixbuf::Colorspace::Rgb, false, 8, 100, 100).unwrap();
        blank.fill(0xffff_ffff);
        let red = gdk::RGBA::new(1.0, 0.0, 0.0, 1.0);
        let style = Style { stroke: red, fill: None, width: 3.0, font: pango::FontDescription::from_string("Sans 12") };
        let marked = shapes::paint(&blank, Tool::Rectangle, Shape::Rectangle, &style, &[(10.0, 10.0), (60.0, 60.0)], "").unwrap();
        assert_eq!(pixel(&marked, 10, 30), [255, 0, 0]);
        assert_eq!(pixel(&marked, 30, 30), [255, 255, 255]);
        assert_eq!(pixel(&marked, 90, 90), [255, 255, 255]);
    }

    #[test]
    fn text_is_drawn_where_it_was_put() {
        let blank = Pixbuf::new(gtk::gdk_pixbuf::Colorspace::Rgb, false, 8, 200, 100).unwrap();
        blank.fill(0xffff_ffff);
        let black = gdk::RGBA::new(0.0, 0.0, 0.0, 1.0);
        let style = Style { stroke: black, fill: None, width: 2.0, font: pango::FontDescription::from_string("Sans 20") };
        let marked = shapes::paint(&blank, Tool::TextBox, Shape::Rectangle, &style, &[(10.0, 10.0)], "MMM").unwrap();
        let dark = (10..60).flat_map(|x| (10..40).map(move |y| (x, y))).filter(|&(x, y)| pixel(&marked, x, y)[0] < 128).count();
        assert!(dark > 20, "{dark}");
        assert_eq!(pixel(&marked, 190, 90), [255, 255, 255]);
    }

    #[test]
    fn a_picture_is_exported_in_the_formats_encoders_write() {
        let blank = Pixbuf::new(gtk::gdk_pixbuf::Colorspace::Rgb, false, 8, 32, 32).unwrap();
        blank.fill(0x3366_99ff);
        let dir = std::env::temp_dir().join(format!("nekoland-preview-export-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (extension, program) in [("jp2", "opj_compress"), ("heic", "heif-enc"), ("exr", "magick")] {
            if Command::new(program).arg("-version").output().is_err() && Command::new(program).arg("-h").output().is_err() {
                continue;
            }
            let path = dir.join(format!("picture.{extension}"));
            export_with_encoder(&blank, &path, 90).unwrap_or_else(|err| panic!("{extension}: {err}"));
            assert!(std::fs::metadata(&path).unwrap().len() > 0, "{extension}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A PDF of 200 × 100 point pages, each with its text near the top left.
    fn text_pdf(path: &Path, pages: &[&str]) {
        let surface = gtk::cairo::PdfSurface::new(200.0, 100.0, path).unwrap();
        let cr = gtk::cairo::Context::new(&surface).unwrap();
        for text in pages {
            let layout = pangocairo::functions::create_layout(&cr);
            layout.set_font_description(Some(&pango::FontDescription::from_string("Sans 12")));
            layout.set_text(text);
            cr.move_to(10.0, 10.0);
            pangocairo::functions::show_layout(&cr, &layout);
            cr.show_page().unwrap();
        }
        drop(cr);
        surface.finish();
    }

    #[test]
    fn text_is_found_where_it_is_on_its_page_and_chosen_across_pages() {
        let dir = std::env::temp_dir().join(format!("nekoland-preview-text-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("text.pdf");
        text_pdf(&path, &["alpha", "beta gamma"]);
        let document = open_pdf(&path, None).unwrap();
        assert_eq!(page_count(&path), Ok(2));

        let (first, second) = (document.page(0).unwrap(), document.page(1).unwrap());
        assert!(page_matches(&first, "gamma").is_empty());
        let found = page_matches(&second, "GAMMA");
        assert_eq!(found.len(), 1);
        let (x1, y1, x2, y2) = found[0];
        // After "beta ", near the top of the page, measured down from it.
        assert!(x1 > 30.0 && x2 > x1 && x2 < 200.0, "{found:?}");
        assert!(y1 >= 5.0 && y2 > y1 && y2 < 50.0, "{found:?}");

        let all = Selection { from: (0, 0.0, 0.0), to: (1, 200.0, 100.0), style: poppler::SelectionStyle::Glyph };
        let text = selected_text(&document, &all);
        assert!(text.contains("alpha") && text.contains("beta gamma") && text.find("alpha") < text.find("beta"), "{text:?}");
        // Chosen backwards, from the end of the second page to the top of the first, it is the same.
        let backwards = Selection { from: all.to, to: all.from, style: all.style };
        assert_eq!(selected_text(&document, &backwards), text);
        let word = Selection { from: (1, x1 + 1.0, y1 + 1.0), to: (1, x1 + 1.0, y1 + 1.0), style: poppler::SelectionStyle::Word };
        assert_eq!(selected_text(&document, &word).trim(), "gamma");

        let texture = page_texture(&first, 2.0).unwrap();
        assert_eq!((texture.width(), texture.height()), (400, 200));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_column_of_pages_is_measured_from_its_top() {
        let dir = std::env::temp_dir().join(format!("nekoland-preview-column-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("column.pdf");
        text_pdf(&path, &["one", "two", "three"]);
        let pdf = Pdf {
            path: path.clone(),
            document: RefCell::new(open_pdf(&path, None).unwrap()),
            password: None,
            pages: Vec::new(),
            sizes: vec![(200.0, 100.0), (200.0, 300.0), (200.0, 100.0)],
            scale: Cell::new(2.0),
            undrawn: RefCell::default(),
            layout: Cell::new(Layout::Continuous),
            spread: Cell::new(0),
            dirty: Cell::new(false),
            undo: RefCell::default(),
            redo: RefCell::default(),
        };
        let edge = f64::from(EDGE);
        assert_eq!(pdf.top(0, 2.0), edge);
        assert_eq!(pdf.top(1, 2.0), edge + 200.0 + edge);
        assert_eq!(pdf.top(3, 2.0), edge + 200.0 + edge + 600.0 + edge + 200.0 + edge);
        assert_eq!(pdf.page_at(0.0, 2.0), 0);
        // The gap under a page goes with it.
        assert_eq!(pdf.page_at(edge + 200.0 + 1.0, 2.0), 0);
        assert_eq!(pdf.page_at(pdf.top(1, 2.0) + 1.0, 2.0), 1);
        assert_eq!(pdf.page_at(1e9, 2.0), 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn pages_are_rewritten_in_the_order_of_the_plan() {
        if ["pdfseparate", "pdfunite", "pdfinfo"].iter().any(|tool| Command::new(tool).arg("-v").output().is_err()) {
            return;
        }
        let dir = std::env::temp_dir().join(format!("nekoland-preview-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("pages.pdf");
        let surface = gtk::cairo::PdfSurface::new(100.0, 100.0, &path).unwrap();
        let cr = gtk::cairo::Context::new(&surface).unwrap();
        for width in [100.0, 200.0, 300.0] {
            surface.set_size(width, 100.0).unwrap();
            cr.show_page().unwrap();
        }
        drop(cr);
        surface.finish();
        assert_eq!(page_count(&path), Ok(3));
        let plan = vec![(path.clone(), 3), (path.clone(), 1)];
        rebuild_pdf(&path, &plan).unwrap();
        assert_eq!(page_count(&path), Ok(2));
        let sizes = Command::new("pdfinfo").args(["-f", "1", "-l", "2"]).arg(&path).output().unwrap();
        let sizes = String::from_utf8_lossy(&sizes.stdout).into_owned();
        assert!(sizes.contains("Page    1 size:  300 x 100"), "{sizes}");
        assert!(sizes.contains("Page    2 size:  100 x 100"), "{sizes}");
        // Without the first page, then with the other file's pages put after it.
        rebuild_pdf(&path, &[(path.clone(), 2)]).unwrap();
        assert_eq!(page_count(&path), Ok(1));
        let other = dir.join("other.pdf");
        std::fs::copy(&path, &other).unwrap();
        rebuild_pdf(&path, &[(path.clone(), 1), (other.clone(), 1), (path.clone(), 1)]).unwrap();
        assert_eq!(page_count(&path), Ok(3));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_grey_pdf_keeps_its_drawing_and_loses_its_colour() {
        if Command::new("gs").arg("--version").output().is_err() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("nekoland-preview-grey-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (red, grey) = (dir.join("red.pdf"), dir.join("100% grey.pdf"));
        let surface = gtk::cairo::PdfSurface::new(100.0, 100.0, &red).unwrap();
        let cr = gtk::cairo::Context::new(&surface).unwrap();
        cr.set_source_rgb(1.0, 0.0, 0.0);
        cr.paint().unwrap();
        drop(cr);
        surface.finish();
        pdfdoc::grayscale(&red, &grey).unwrap();
        let page = poppler::Document::from_file(&glib::filename_to_uri(&grey, None).unwrap(), None).unwrap().page(0).unwrap();
        let mut drawn = gtk::cairo::ImageSurface::create(gtk::cairo::Format::Rgb24, 10, 10).unwrap();
        let cr = gtk::cairo::Context::new(&drawn).unwrap();
        cr.scale(0.1, 0.1);
        page.render(&cr);
        drop(cr);
        let data = drawn.data().unwrap();
        let (b, g, r) = (data[0], data[1], data[2]);
        assert!(r == g && g == b && r < 200, "{r} {g} {b}");
        // Still drawn, not turned into a picture.
        if let Ok(images) = Command::new("pdfimages").arg("-list").arg(&grey).output() {
            assert_eq!(String::from_utf8_lossy(&images.stdout).lines().count(), 2);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_protected_pdf_asks_for_its_password_and_keeps_its_permissions() {
        if !pdfdoc::has_qpdf() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("nekoland-preview-protect-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("open.pdf");
        text_pdf(&path, &["one", "two"]);
        let locked = dir.join("locked.pdf");
        let allowed = pdfdoc::Permissions { print: false, copy: true, assemble: true, annotate: true, forms: true };
        pdfdoc::protect(&path, None, &locked, "secret", "owner pass", allowed).unwrap();
        assert!(open_pdf(&locked, None).is_err());
        let document = open_pdf(&locked, Some("secret")).unwrap();
        assert!(!pdfdoc::permissions(&document).print);
        assert!(pdfdoc::permissions(&document).copy);
        // A password-protected file's pages are reordered with its password.
        pdfdoc::rebuild(&locked, &[(locked.clone(), Some("secret".into()), 2), (locked.clone(), Some("secret".into()), 1)]).unwrap();
        assert!(open_pdf(&locked, None).is_err(), "reordering kept the password");
        let document = open_pdf(&locked, Some("secret")).unwrap();
        assert!(document.page(0).unwrap().text().unwrap().contains("two"));
        assert!(!pdfdoc::permissions(&document).print);
        // An open file stays open.
        pdfdoc::rebuild(&path, &[(path.clone(), None, 2), (path.clone(), None, 1)]).unwrap();
        assert!(open_pdf(&path, None).is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_cropped_page_shows_only_the_part_chosen() {
        if !pdfdoc::has_qpdf() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("nekoland-preview-crop-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("crop.pdf");
        text_pdf(&path, &["one", "two"]);
        pdfdoc::crop(&path, None, &[2], (10.0, 20.0, 110.0, 70.0)).unwrap();
        let document = open_pdf(&path, None).unwrap();
        let (width, height) = document.page(1).unwrap().size();
        assert!((width - 100.0).abs() < 0.5 && (height - 50.0).abs() < 0.5, "{width} × {height}");
        assert_eq!(document.page(0).unwrap().size(), (200.0, 100.0));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_resized_rectangle_keeps_its_new_box_once_saved() {
        let dir = std::env::temp_dir().join(format!("nekoland-preview-resize-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("marked.pdf");
        text_pdf(&path, &["one"]);
        let document = open_pdf(&path, None).unwrap();
        let page = document.page(0).unwrap();
        let square = annots::shape(&document, &page, false, (20.0, 20.0, 60.0, 50.0), (1.0, 0.0, 0.0), None, 2.0).unwrap();
        annots::set_place(&page, &square, (20.0, 20.0, 150.0, 90.0));
        let saved = dir.join("saved.pdf");
        document.save(&gio::File::for_path(&saved).uri()).unwrap();
        let again = open_pdf(&saved, None).unwrap();
        let found = annots::on_page(&again.page(0).unwrap());
        let square = found.iter().find(|annotation| annotation.kind == annots::Kind::Square).expect("the rectangle");
        let (x1, y1, x2, y2) = square.place;
        for (got, wanted) in [(x1, 20.0), (y1, 20.0), (x2, 150.0), (y2, 90.0)] {
            assert!((got - wanted).abs() < 0.5, "{:?}", square.place);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
