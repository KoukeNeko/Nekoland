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
mod browse;

use browse::{modified_label, EDGE, GAP};

const APP_ID: &str = "io.github.koukeneko.Nekoland.Preview";
const THUMB_SIZE: i32 = 160;
const MIN_SCALE: f64 = 0.05;
const MAX_SCALE: f64 = 16.0;
const ZOOM_STEP: f64 = 1.25;
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

#[derive(Clone, Copy, PartialEq, Debug)]
enum Tool {
    Pen,
    Highlighter,
    Rectangle,
    Ellipse,
    Arrow,
    Text,
}

const TOOLS: [(Tool, &str); 6] = [
    (Tool::Pen, "畫筆"),
    (Tool::Highlighter, "螢光筆"),
    (Tool::Rectangle, "矩形"),
    (Tool::Ellipse, "橢圓"),
    (Tool::Arrow, "箭頭"),
    (Tool::Text, "文字"),
];

/// `pixbuf` with one mark drawn over it, in the picture's own pixels: `points` are where the
/// pointer went, the first and last of them the corners of a shape.
fn paint(pixbuf: &Pixbuf, tool: Tool, color: gdk::RGBA, points: &[(f64, f64)], text: &str) -> Option<Pixbuf> {
    use gtk::cairo::{Context, Format, ImageSurface, LineCap, LineJoin};
    let (width, height) = (pixbuf.width(), pixbuf.height());
    let surface = ImageSurface::create(Format::ARgb32, width, height).ok()?;
    let cr = Context::new(&surface).ok()?;
    cr.set_source_pixbuf(pixbuf, 0.0, 0.0);
    cr.paint().ok()?;
    let line = (f64::from(width.max(height)) / 200.0).max(2.0);
    let (first, last) = (*points.first()?, *points.last()?);
    let alpha = if tool == Tool::Highlighter { 0.4 } else { f64::from(color.alpha()) };
    cr.set_source_rgba(f64::from(color.red()), f64::from(color.green()), f64::from(color.blue()), alpha);
    cr.set_line_cap(LineCap::Round);
    cr.set_line_join(LineJoin::Round);
    cr.set_line_width(if tool == Tool::Highlighter { line * 5.0 } else { line });
    match tool {
        Tool::Pen | Tool::Highlighter => {
            cr.move_to(first.0, first.1);
            for point in points {
                cr.line_to(point.0, point.1);
            }
            cr.stroke().ok()?;
        }
        Tool::Rectangle => {
            cr.rectangle(first.0, first.1, last.0 - first.0, last.1 - first.1);
            cr.stroke().ok()?;
        }
        Tool::Ellipse => {
            let (rx, ry) = (((last.0 - first.0) / 2.0).abs().max(0.5), ((last.1 - first.1) / 2.0).abs().max(0.5));
            cr.save().ok()?;
            cr.translate((first.0 + last.0) / 2.0, (first.1 + last.1) / 2.0);
            cr.scale(rx, ry);
            cr.arc(0.0, 0.0, 1.0, 0.0, std::f64::consts::TAU);
            cr.restore().ok()?;
            cr.stroke().ok()?;
        }
        Tool::Arrow => {
            let angle = (last.1 - first.1).atan2(last.0 - first.0);
            let head = line * 6.0;
            cr.move_to(first.0, first.1);
            cr.line_to(last.0, last.1);
            for turn in [2.6, -2.6] {
                cr.move_to(last.0, last.1);
                cr.line_to(last.0 + head * (angle + turn).cos(), last.1 + head * (angle + turn).sin());
            }
            cr.stroke().ok()?;
        }
        Tool::Text => {
            let layout = pangocairo::functions::create_layout(&cr);
            let mut font = pango::FontDescription::from_string("Sans");
            font.set_absolute_size(line * 9.0 * f64::from(pango::SCALE));
            layout.set_font_description(Some(&font));
            layout.set_text(text);
            cr.move_to(first.0, first.1);
            pangocairo::functions::show_layout(&cr, &layout);
        }
    }
    drop(cr);
    surface.flush();
    // Deprecated for a texture, which cannot be edited again as a Pixbuf is.
    #[allow(deprecated)]
    let painted = gdk::pixbuf_get_from_surface(&surface, 0, 0, width, height);
    painted
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
}

impl Item {
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

/// The PDF the window shows: its pages one under another, drawn by poppler as the zoom asks.
struct Pdf {
    path: PathBuf,
    document: poppler::Document,
    pages: Vec<gtk::DrawingArea>,
    /// Each page's size in points.
    sizes: Vec<(f64, f64)>,
    /// The screen pixels to a point the pages are laid out at; 0 before they are.
    scale: Cell<f64>,
    /// Pages left blank while out of view, to be drawn when scrolled to.
    undrawn: RefCell<HashSet<usize>>,
}

impl Pdf {
    /// How far down the column page `index` starts at `scale`, with an `EDGE` around the pages
    /// and between them; past the last page, the column's whole height.
    fn top(&self, index: usize, scale: f64) -> f64 {
        let above: f64 = self.sizes[..index].iter().map(|&(_, height)| (height * scale).round() + f64::from(EDGE)).sum();
        above + f64::from(EDGE)
    }

    fn height(&self, index: usize, scale: f64) -> f64 {
        (self.sizes[index].1 * scale).round()
    }

    /// The page `y` down the column is on, or the one above the gap it is in.
    fn page_at(&self, y: f64, scale: f64) -> usize {
        let mut top = f64::from(EDGE);
        for index in 0..self.sizes.len() {
            top += self.height(index, scale) + f64::from(EDGE);
            if y < top {
                return index;
            }
        }
        self.sizes.len().saturating_sub(1)
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

struct Win {
    window: adw::ApplicationWindow,
    split: adw::OverlaySplitView,
    title: adw::WindowTitle,
    stack: gtk::Stack,
    scroll: gtk::ScrolledWindow,
    picture: gtk::Picture,
    crop_layer: gtk::DrawingArea,
    content: adw::ToolbarView,
    /// The part to keep while cropping, in the picture's own widget coordinates.
    crop: Cell<Option<(f64, f64, f64, f64)>>,
    cropping: Cell<bool>,
    /// The mark being drawn, in the picture's pixels, and the tool that marks.
    tool: Cell<Option<Tool>>,
    /// The tool the bar shows chosen, which marking starts with.
    picked: Cell<Tool>,
    stroke: RefCell<Vec<(f64, f64)>>,
    color: gtk::ColorDialogButton,
    crop_bar: gtk::ActionBar,
    markup_bar: gtk::ActionBar,
    list: gtk::ListBox,
    zoom_button: gtk::Button,
    items: RefCell<Vec<Rc<Item>>>,
    thumbnails: RefCell<Vec<gtk::Picture>>,
    current: Cell<usize>,
    zoom: Cell<Zoom>,
    /// The picture shown; a PDF's pages are in `pdf` instead.
    shown: RefCell<Option<Pixbuf>>,
    syncing: Cell<bool>,
    discard_edits: Cell<bool>,
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
    /// Whether 搜尋 has the keyboard, which then takes the keys the window's actions would.
    typing: Cell<bool>,
    /// The passwords the PDFs that need one were opened with.
    passwords: RefCell<HashMap<PathBuf, String>>,
}

/// In the user's own cache, not the shared temporary folder, where another user could have
/// made the folder first.
fn scratch_dir() -> PathBuf {
    glib::user_cache_dir().join(format!("nekoland/preview-{}", std::process::id()))
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
fn export_with_encoder(pixbuf: &Pixbuf, path: &Path) -> Result<(), String> {
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
            "heif-enc" => command.args(["-q", "90", "-o"]).arg(path).arg(&png),
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

trait TapValue {
    fn tap_value(self, value: i32) -> Self;
}

impl TapValue for gtk::SpinButton {
    fn tap_value(self, value: i32) -> Self {
        self.set_value(f64::from(value));
        self
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

fn is_pdf(path: &Path) -> bool {
    path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
}

impl Win {
    fn open(app: &adw::Application) -> Rc<Win> {
        let picture = gtk::Picture::builder()
            .content_fit(gtk::ContentFit::Fill)
            .can_shrink(true)
            .css_classes(["pv-sheet"])
            .build();
        // Over the picture, where the part to keep is drawn while cropping.
        let crop_layer = gtk::DrawingArea::builder().can_target(false).build();
        let canvas = gtk::Overlay::builder()
            .child(&picture)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .margin_top(EDGE)
            .margin_bottom(EDGE)
            .margin_start(EDGE)
            .margin_end(EDGE)
            .build();
        canvas.add_overlay(&crop_layer);
        let scroll = gtk::ScrolledWindow::builder()
            .child(&canvas)
            .hexpand(true)
            .vexpand(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .build();
        let open_button = gtk::Button::builder()
            .label("開啟…")
            .halign(gtk::Align::Center)
            .css_classes(["pill", "suggested-action"])
            .action_name("win.open")
            .build();
        let empty = adw::StatusPage::builder()
            .icon_name("image-x-generic-symbolic")
            .title("選擇要預覽的檔案")
            .child(&open_button)
            .build();
        // A PDF's pages, an `EDGE` around and between them.
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
        let pdf_scroll = gtk::ScrolledWindow::builder()
            .child(&pdf_column)
            .hexpand(true)
            .vexpand(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .build();
        let stack = gtk::Stack::new();
        stack.add_named(&empty, Some("empty"));
        stack.add_named(&scroll, Some("view"));
        stack.add_named(&pdf_scroll, Some("pdf"));

        let list = gtk::ListBox::builder()
            .css_classes(["navigation-sidebar"])
            .build();
        let sidebar = gtk::ScrolledWindow::builder()
            .child(&list)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .build();

        let title = adw::WindowTitle::new("預覽程式", "");
        let sidebar_toggle = gtk::ToggleButton::builder()
            .icon_name("sidebar-show-symbolic")
            .tooltip_text("側邊欄")
            .build();
        let zoom_out = gtk::Button::builder()
            .icon_name("zoom-out-symbolic")
            .tooltip_text("縮小")
            .action_name("win.zoom-out")
            .build();
        let zoom_button = gtk::Button::builder().css_classes(["numeric"]).action_name("win.zoom-toggle").build();
        let zoom_in = gtk::Button::builder()
            .icon_name("zoom-in-symbolic")
            .tooltip_text("放大")
            .action_name("win.zoom-in")
            .build();
        let zoom_box = gtk::Box::builder().css_classes(["linked"]).build();
        zoom_box.append(&zoom_out);
        zoom_box.append(&zoom_button);
        zoom_box.append(&zoom_in);
        let rotate = gtk::Button::builder()
            .icon_name("object-rotate-left-symbolic")
            .tooltip_text("向左旋轉")
            .action_name("win.rotate-left")
            .build();
        let menu_model = gio::Menu::new();
        let file = gio::Menu::new();
        file.append(Some("開啟…"), Some("win.open"));
        file.append(Some("儲存"), Some("win.save"));
        file.append(Some("另存新檔…"), Some("win.save-as"));
        file.append(Some("匯出為 PDF…"), Some("win.export-pdf"));
        menu_model.append_section(None, &file);
        let edit = gio::Menu::new();
        edit.append(Some("拷貝"), Some("win.copy"));
        edit.append(Some("全選"), Some("win.select-all"));
        edit.append(Some("尋找"), Some("win.find"));
        edit.append(Some("向左旋轉"), Some("win.rotate-left"));
        edit.append(Some("向右旋轉"), Some("win.rotate-right"));
        edit.append(Some("水平翻轉"), Some("win.flip"));
        edit.append(Some("標註"), Some("win.markup"));
        edit.append(Some("還原"), Some("win.undo"));
        edit.append(Some("裁切"), Some("win.crop"));
        edit.append(Some("調整大小…"), Some("win.resize"));
        let pages = gio::Menu::new();
        pages.append(Some("上移此頁"), Some("win.page-up"));
        pages.append(Some("下移此頁"), Some("win.page-down"));
        pages.append(Some("插入 PDF…"), Some("win.insert-pdf"));
        pages.append(Some("刪除此頁"), Some("win.delete-page"));
        menu_model.append_section(None, &edit);
        menu_model.append_section(None, &pages);
        let view = gio::Menu::new();
        view.append(Some("實際大小"), Some("win.zoom-actual"));
        view.append(Some("適合視窗"), Some("win.zoom-fit"));
        view.append(Some("取得資訊"), Some("win.info"));
        menu_model.append_section(None, &view);
        let menu_button = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .menu_model(&menu_model)
            .build();
        let find_entry = gtk::SearchEntry::builder().placeholder_text("搜尋").width_chars(14).build();
        let find_count = gtk::Label::builder().css_classes(["dim-label", "numeric"]).visible(false).build();
        let find_box = gtk::Box::builder().spacing(GAP).visible(false).build();
        find_box.append(&find_count);
        find_box.append(&find_entry);
        let header = adw::HeaderBar::builder().title_widget(&title).build();
        header.pack_start(&sidebar_toggle);
        header.pack_end(&menu_button);
        header.pack_end(&find_box);
        header.pack_end(&rotate);
        header.pack_end(&zoom_box);

        let cancel_crop = gtk::Button::builder().label("取消").action_name("win.crop-cancel").build();
        let apply_crop = gtk::Button::builder()
            .label("套用")
            .css_classes(["suggested-action"])
            .action_name("win.crop-apply")
            .build();
        let crop_bar = gtk::ActionBar::builder().css_classes(["pv-bar"]).build();
        crop_bar.pack_end(&apply_crop);
        crop_bar.pack_end(&cancel_crop);
        let color = gtk::ColorDialogButton::builder()
            .dialog(&gtk::ColorDialog::builder().with_alpha(false).build())
            .rgba(&gdk::RGBA::new(0.85, 0.15, 0.12, 1.0))
            .tooltip_text("顏色")
            .build();
        let tools = gtk::Box::builder().css_classes(["linked"]).build();
        let mut first_tool: Option<gtk::ToggleButton> = None;
        let tool_buttons: Vec<(Tool, gtk::ToggleButton)> = TOOLS
            .iter()
            .map(|&(tool, label)| {
                let button = gtk::ToggleButton::builder().label(label).active(first_tool.is_none()).build();
                button.set_group(first_tool.as_ref());
                first_tool.get_or_insert_with(|| button.clone());
                tools.append(&button);
                (tool, button)
            })
            .collect();
        let done_marking = gtk::Button::builder()
            .label("完成")
            .css_classes(["suggested-action"])
            .action_name("win.markup-done")
            .build();
        let markup_bar = gtk::ActionBar::builder().css_classes(["pv-bar"]).build();
        markup_bar.pack_start(&tools);
        markup_bar.pack_start(&color);
        markup_bar.pack_end(&done_marking);
        markup_bar.set_visible(false);
        crop_bar.set_visible(false);
        let content = adw::ToolbarView::new();
        content.add_top_bar(&header);
        content.add_bottom_bar(&crop_bar);
        content.add_bottom_bar(&markup_bar);
        content.set_reveal_bottom_bars(false);
        content.set_content(Some(&stack));
        let split = adw::OverlaySplitView::builder()
            .sidebar(&sidebar)
            .content(&content)
            .min_sidebar_width(160.0)
            .max_sidebar_width(240.0)
            .sidebar_width_fraction(0.2)
            .show_sidebar(false)
            .build();
        sidebar_toggle
            .bind_property("active", &split, "show-sidebar")
            .bidirectional()
            .sync_create()
            .build();
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("預覽程式")
            .default_width(960)
            .default_height(700)
            .content(&split)
            .build();

        let win = Rc::new(Win {
            window,
            split,
            title,
            stack,
            scroll,
            picture,
            crop_layer,
            content,
            crop: Cell::new(None),
            cropping: Cell::new(false),
            tool: Cell::new(None),
            picked: Cell::new(Tool::Pen),
            stroke: RefCell::default(),
            color,
            crop_bar,
            markup_bar,
            list,
            zoom_button,
            items: RefCell::new(Vec::new()),
            thumbnails: RefCell::default(),
            current: Cell::new(0),
            zoom: Cell::new(Zoom::Fit),
            shown: RefCell::new(None),
            syncing: Cell::new(false),
            discard_edits: Cell::new(false),
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
        });
        for (tool, button) in tool_buttons {
            let weak = Rc::downgrade(&win);
            button.connect_toggled(move |button| {
                if let (true, Some(win)) = (button.is_active(), weak.upgrade()) {
                    win.picked.set(tool);
                    if win.tool.get().is_some() {
                        win.tool.set(Some(tool));
                    }
                }
            });
        }
        win.add_actions();
        win.connect();
        win.show_nothing();
        win.window.present();
        win
    }

    fn add_actions(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        let add = |name: &str, run: fn(&Rc<Win>)| {
            let action = gio::SimpleAction::new(name, None);
            let weak = weak.clone();
            action.connect_activate(move |_, _| {
                if let Some(win) = weak.upgrade() {
                    run(&win);
                }
            });
            self.window.add_action(&action);
        };
        add("open", |win| win.choose_files());
        add("save", |win| win.save());
        add("save-as", |win| win.save_as());
        add("copy", |win| win.copy());
        add("rotate-left", |win| win.turn(PixbufRotation::Counterclockwise));
        add("rotate-right", |win| win.turn(PixbufRotation::Clockwise));
        add("flip", |win| win.flip());
        add("info", |win| win.show_info());
        add("zoom-in", |win| win.zoom_by(ZOOM_STEP));
        add("zoom-out", |win| win.zoom_by(1.0 / ZOOM_STEP));
        add("zoom-fit", |win| win.set_zoom(Zoom::Fit));
        add("zoom-actual", |win| win.set_actual());
        add("zoom-toggle", |win| match win.zoom.get() {
            Zoom::Fit => win.set_actual(),
            Zoom::Scale(_) => win.set_zoom(Zoom::Fit),
        });
        add("markup", |win| win.start_markup());
        add("markup-done", |win| win.finish_markup());
        add("undo", |win| win.undo());
        add("export-pdf", |win| win.save_as_type("pdf"));
        add("page-up", |win| win.move_page(-1));
        add("page-down", |win| win.move_page(1));
        add("insert-pdf", |win| win.insert_pdf());
        add("crop", |win| win.start_crop());
        add("crop-apply", |win| win.finish_crop(true));
        add("crop-cancel", |win| win.finish_crop(false));
        add("resize", |win| win.resize());
        add("delete-page", |win| win.delete_page());
        add("next", |win| win.step(1));
        add("previous", |win| win.step(-1));
        add("find", |win| {
            win.find_entry.grab_focus();
        });
        add("find-next", |win| win.step_match(1));
        add("find-previous", |win| win.step_match(-1));
        add("select-all", |win| win.select_all());
    }

    fn connect(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.list.connect_row_selected({
            let weak = weak.clone();
            move |_, row| {
                let Some(win) = weak.upgrade() else { return };
                if let (false, Some(row)) = (win.syncing.get(), row) {
                    win.select(row.index() as usize);
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
                    match (held, weak.upgrade()) {
                        (true, Some(win)) => {
                            win.zoom_by(ZOOM_STEP.powf(-dy));
                            glib::Propagation::Stop
                        }
                        _ => glib::Propagation::Proceed,
                    }
                }
            });
            scroll.add_controller(wheel);

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

        // Dragging over a PDF's pages chooses its text, a double click a word and a triple
        // click a line.
        let choose = gtk::GestureDrag::new();
        let anchor = Rc::new(Cell::new(None));
        choose.connect_drag_begin({
            let (weak, anchor) = (weak.clone(), anchor.clone());
            move |_, x, y| {
                if let Some(win) = weak.upgrade() {
                    anchor.set(win.hit(x, y));
                }
            }
        });
        choose.connect_drag_update({
            let weak = weak.clone();
            move |drag, dx, dy| {
                // Not the jitter of a click.
                if dx.hypot(dy) < 3.0 {
                    return;
                }
                let (Some(win), Some(from), Some((x, y))) = (weak.upgrade(), anchor.get(), drag.start_point()) else { return };
                if let Some(to) = win.hit(x + dx, y + dy) {
                    win.set_selection(Some(Selection { from, to, style: poppler::SelectionStyle::Glyph }));
                }
            }
        });
        self.pdf_column.add_controller(choose);
        let clicks = gtk::GestureClick::new();
        clicks.connect_pressed({
            let weak = weak.clone();
            move |_, presses, x, y| {
                let Some(win) = weak.upgrade() else { return };
                win.pdf_column.grab_focus();
                let style = match presses {
                    1 => return win.set_selection(None),
                    2 => poppler::SelectionStyle::Word,
                    _ => poppler::SelectionStyle::Line,
                };
                if let Some(at) = win.hit(x, y) {
                    win.set_selection(Some(Selection { from: at, to: at, style }));
                }
            }
        });
        self.pdf_column.add_controller(clicks);

        self.find_entry.connect_search_changed({
            let weak = weak.clone();
            move |_| {
                if let Some(win) = weak.upgrade() {
                    win.find();
                }
            }
        });
        self.find_entry.connect_activate({
            let weak = weak.clone();
            move |_| {
                if let Some(win) = weak.upgrade() {
                    win.step_match(1);
                }
            }
        });
        self.find_entry.connect_next_match({
            let weak = weak.clone();
            move |_| {
                if let Some(win) = weak.upgrade() {
                    win.step_match(1);
                }
            }
        });
        self.find_entry.connect_previous_match({
            let weak = weak.clone();
            move |_| {
                if let Some(win) = weak.upgrade() {
                    win.step_match(-1);
                }
            }
        });
        self.find_entry.connect_stop_search(|entry| entry.set_text(""));
        let focus = gtk::EventControllerFocus::new();
        for (signal_enters, typing) in [(true, true), (false, false)] {
            let weak = weak.clone();
            let then = move |_: &gtk::EventControllerFocus| {
                if let Some(win) = weak.upgrade() {
                    win.typing.set(typing);
                    win.update_actions();
                }
            };
            if signal_enters {
                focus.connect_enter(then);
            } else {
                focus.connect_leave(then);
            }
        }
        self.find_entry.add_controller(focus);

        let pan = gtk::GestureDrag::new();
        let origin = Rc::new(Cell::new((0.0, 0.0)));
        pan.connect_drag_begin({
            let (weak, origin) = (weak.clone(), origin.clone());
            move |_, _, _| {
                if let Some(win) = weak.upgrade().filter(|win| !win.editing()) {
                    origin.set((win.scroll.hadjustment().value(), win.scroll.vadjustment().value()));
                }
            }
        });
        pan.connect_drag_update({
            let weak = weak.clone();
            move |_, dx, dy| {
                if let Some(win) = weak.upgrade().filter(|win| !win.editing()) {
                    win.scroll.hadjustment().set_value(origin.get().0 - dx);
                    win.scroll.vadjustment().set_value(origin.get().1 - dy);
                }
            }
        });
        self.scroll.add_controller(pan);

        self.crop_layer.set_draw_func({
            let weak = weak.clone();
            move |_, cr, width, height| {
                let Some(win) = weak.upgrade() else { return };
                if let Some(tool) = win.tool.get() {
                    let scale = win.scale();
                    let color = win.color.rgba();
                    cr.set_source_rgba(
                        f64::from(color.red()),
                        f64::from(color.green()),
                        f64::from(color.blue()),
                        if tool == Tool::Highlighter { 0.4 } else { 1.0 },
                    );
                    cr.set_line_cap(gtk::cairo::LineCap::Round);
                    cr.set_line_width(2.0 * if tool == Tool::Highlighter { 5.0 } else { 1.0 });
                    let stroke = win.stroke.borrow();
                    let mut points = stroke.iter().map(|(x, y)| (x * scale, y * scale));
                    if let (Some(first), Some(last)) = (points.next(), stroke.last().map(|(x, y)| (x * scale, y * scale))) {
                        match tool {
                            Tool::Pen | Tool::Highlighter => {
                                cr.move_to(first.0, first.1);
                                points.for_each(|(x, y)| cr.line_to(x, y));
                            }
                            Tool::Rectangle | Tool::Text => cr.rectangle(first.0, first.1, last.0 - first.0, last.1 - first.1),
                            Tool::Ellipse | Tool::Arrow => {
                                cr.move_to(first.0, first.1);
                                cr.line_to(last.0, last.1);
                            }
                        }
                        let _ = cr.stroke();
                    }
                    return;
                }
                let Some((x0, y0, x1, y1)) = win.crop.get() else { return };
                let (width, height) = (f64::from(width), f64::from(height));
                cr.set_source_rgba(0.0, 0.0, 0.0, 0.5);
                for (x, y, w, h) in [
                    (0.0, 0.0, width, y0),
                    (0.0, y1, width, height - y1),
                    (0.0, y0, x0, y1 - y0),
                    (x1, y0, width - x1, y1 - y0),
                ] {
                    cr.rectangle(x, y, w, h);
                }
                let _ = cr.fill();
                cr.set_source_rgb(1.0, 1.0, 1.0);
                cr.set_line_width(1.0);
                cr.rectangle(x0 + 0.5, y0 + 0.5, x1 - x0 - 1.0, y1 - y0 - 1.0);
                let _ = cr.stroke();
            }
        });
        let select = gtk::GestureDrag::new();
        let from = Rc::new(Cell::new((0.0, 0.0)));
        select.connect_drag_begin({
            let (weak, from) = (weak.clone(), from.clone());
            move |_, x, y| {
                if let Some(win) = weak.upgrade().filter(|win| win.tool.get().is_some()) {
                    let scale = win.scale();
                    *win.stroke.borrow_mut() = vec![(x / scale, y / scale)];
                    from.set((x, y));
                    return;
                }
                if let Some(win) = weak.upgrade().filter(|win| win.cropping.get()) {
                    from.set((x, y));
                    win.crop.set(None);
                    win.crop_layer.queue_draw();
                }
            }
        });
        select.connect_drag_update({
            let weak = weak.clone();
            let from = from.clone();
            move |_, dx, dy| {
                if let Some(win) = weak.upgrade().filter(|win| win.tool.get().is_some()) {
                    let scale = win.scale();
                    let (x, y) = from.get();
                    win.stroke.borrow_mut().push(((x + dx) / scale, (y + dy) / scale));
                    win.crop_layer.queue_draw();
                    return;
                }
                let Some(win) = weak.upgrade().filter(|win| win.cropping.get()) else { return };
                let (width, height) = (f64::from(win.crop_layer.width()), f64::from(win.crop_layer.height()));
                let (x, y) = from.get();
                let (x1, y1) = ((x + dx).clamp(0.0, width), (y + dy).clamp(0.0, height));
                let (x0, y0) = (x.clamp(0.0, width), y.clamp(0.0, height));
                win.crop.set(Some((x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1))));
                win.crop_layer.queue_draw();
            }
        });
        select.connect_drag_end({
            let weak = weak.clone();
            move |_, _, _| {
                if let Some(win) = weak.upgrade() {
                    win.finish_mark();
                }
            }
        });
        self.picture.add_controller(select);

        let double = gtk::GestureClick::new();
        double.connect_pressed({
            let weak = weak.clone();
            move |_, presses, _, _| {
                if let (2, Some(win)) = (presses, weak.upgrade().filter(|win| !win.editing())) {
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
                win.open_files(files.files().iter().filter_map(|file| file.path()).collect());
                true
            }
        });
        self.window.add_controller(drop);

        self.window.connect_close_request({
            let weak = weak.clone();
            move |window| {
                let Some(win) = weak.upgrade() else {
                    return glib::Propagation::Proceed;
                };
                let edited = win.items.borrow().iter().filter(|item| item.edited.borrow().is_some()).count();
                if edited == 0 || win.discard_edits.get() {
                    return glib::Propagation::Proceed;
                }
                let window = window.clone();
                alert::confirm(
                    &window.clone(),
                    &format!("捨棄 {edited} 個檔案的變更？"),
                    Some("變更尚未儲存。"),
                    "捨棄",
                    move || {
                        win.discard_edits.set(true);
                        window.close();
                    },
                );
                glib::Propagation::Stop
            }
        });
    }

    fn item(&self) -> Option<Rc<Item>> {
        self.items.borrow().get(self.current.get()).cloned()
    }

    fn show_nothing(&self) {
        self.stack.set_visible_child_name("empty");
        self.title.set_title("預覽程式");
        self.title.set_subtitle("");
        self.zoom_button.set_label("");
        self.update_actions();
    }

    /// Replaces what is shown with `paths`: each picture, and each page of each PDF.
    fn open_files(self: &Rc<Self>, paths: Vec<PathBuf>) {
        let mut items = Vec::new();
        let mut failed = Vec::new();
        let mut locked = None;
        for path in &paths {
            if opens_as_pdf(path) {
                let password = self.passwords.borrow().get(path).cloned();
                match open_pdf(path, password.as_deref()) {
                    Ok(document) => items.extend((1..=document.n_pages().max(0) as u32).map(|page| {
                        Rc::new(Item { path: path.clone(), page: Some(page), edited: RefCell::new(None), undo: RefCell::default() })
                    })),
                    Err(err) if err.matches(poppler::Error::Encrypted) => {
                        locked.get_or_insert_with(|| path.clone());
                    }
                    Err(err) => failed.push(format!("{}: {err}", path.display())),
                }
            } else {
                items.push(Rc::new(Item { path: path.clone(), page: None, edited: RefCell::new(None), undo: RefCell::default() }));
            }
        }
        // All of them are opened again once the password is in, or asked for again when wrong.
        if let Some(path) = locked {
            return self.ask_password(path, paths);
        }
        if !failed.is_empty() {
            alert::notice(&self.window, "無法開啟", Some(&failed.join("\n")));
        }
        if items.is_empty() {
            return;
        }
        // A PDF written again is read again.
        *self.pdf.borrow_mut() = None;
        self.selection.set(None);
        self.syncing.set(true);
        while let Some(row) = self.list.row_at_index(0) {
            self.list.remove(&row);
        }
        let mut thumbnails = Vec::new();
        for (index, item) in items.iter().enumerate() {
            let (row, thumbnail) = thumbnail_row(item, index);
            self.list.append(&row);
            thumbnails.push((item.clone(), thumbnail));
        }
        *self.thumbnails.borrow_mut() = thumbnails.iter().map(|(_, picture)| picture.clone()).collect();
        self.syncing.set(false);
        self.split.set_show_sidebar(items.len() > 1);
        *self.items.borrow_mut() = items;
        self.select(0);
        self.load_thumbnails(thumbnails);
    }

    /// Fills the sidebar's thumbnails one at a time, so the window stays free between them.
    fn load_thumbnails(self: &Rc<Self>, thumbnails: Vec<(Rc<Item>, gtk::Picture)>) {
        let weak = Rc::downgrade(self);
        let passwords = self.passwords.borrow().clone();
        let density = f64::from(self.window.scale_factor());
        glib::spawn_future_local(async move {
            let mut documents: HashMap<PathBuf, Option<poppler::Document>> = HashMap::new();
            for (item, picture) in thumbnails {
                if weak.upgrade().is_none() {
                    return;
                }
                if let Some(number) = item.page {
                    let document = documents
                        .entry(item.path.clone())
                        .or_insert_with(|| open_pdf(&item.path, passwords.get(&item.path).map(String::as_str)).ok());
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
                let file = Some(item.path.clone());
                let pixbuf = file.and_then(|file| {
                    Pixbuf::from_file_at_scale(&file, THUMB_SIZE, THUMB_SIZE, true)
                        .ok()
                        .or_else(|| {
                            let whole = load_picture(&file).ok()?;
                            let ratio = f64::from(THUMB_SIZE) / f64::from(whole.width().max(whole.height()));
                            whole.scale_simple(
                                ((f64::from(whole.width()) * ratio) as i32).max(1),
                                ((f64::from(whole.height()) * ratio) as i32).max(1),
                                gtk::gdk_pixbuf::InterpType::Bilinear,
                            )
                        })
                });
                if let Some(pixbuf) = pixbuf {
                    picture.set_size_request(pixbuf.width(), pixbuf.height());
                    picture.set_paintable(Some(&gdk::Texture::for_pixbuf(&pixbuf)));
                }
                glib::timeout_future(std::time::Duration::from_millis(1)).await;
            }
        });
    }

    fn select(self: &Rc<Self>, index: usize) {
        let Some(item) = self.items.borrow().get(index).cloned() else {
            return;
        };
        self.current.set(index);
        self.syncing.set(true);
        if let Some(row) = self.list.row_at_index(index as i32) {
            self.list.select_row(Some(&row));
        }
        self.syncing.set(false);

        if let Some(edited) = item.edited.borrow().clone() {
            return self.show(&item, edited);
        }
        match item.page {
            None => match load_picture(&item.path) {
                Ok(pixbuf) => self.show(&item, pixbuf.apply_embedded_orientation().unwrap_or(pixbuf)),
                Err(err) => self.fail(&item, &err),
            },
            Some(page) => match self.show_pdf(&item.path) {
                Ok(()) => {
                    *self.shown.borrow_mut() = None;
                    self.apply_zoom();
                    self.update_title(&item);
                    self.update_actions();
                    // Just above the page, with the gap over it in view.
                    self.pending.set(Some((page as usize - 1, 0.0, -f64::from(EDGE))));
                    self.apply_pending();
                }
                Err(err) => self.fail(&item, &err),
            },
        }
    }

    /// Asks for the password of the PDF at `path`, then opens `paths` again with it.
    fn ask_password(self: &Rc<Self>, path: PathBuf, paths: Vec<PathBuf>) {
        let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        let weak = Rc::downgrade(self);
        let entry = alert::prompt(&self.window, &format!("輸入「{name}」的密碼"), "", -1, "開啟", move |password| {
            if let Some(win) = weak.upgrade() {
                win.passwords.borrow_mut().insert(path, password);
                win.open_files(paths);
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
        while let Some(child) = self.pdf_column.first_child() {
            self.pdf_column.remove(&child);
        }
        let sizes: Vec<(f64, f64)> =
            (0..document.n_pages()).map(|index| document.page(index).map_or((1.0, 1.0), |page| page.size())).collect();
        let pages = (0..sizes.len())
            .map(|index| {
                let area = gtk::DrawingArea::builder().css_classes(["pv-sheet"]).halign(gtk::Align::Center).build();
                let weak = Rc::downgrade(self);
                area.set_draw_func(move |_, cr, _, _| {
                    if let Some(win) = weak.upgrade() {
                        win.draw_page(index, cr);
                    }
                });
                self.pdf_column.append(&area);
                area
            })
            .collect();
        *self.pdf.borrow_mut() = Some(Rc::new(Pdf {
            path: path.to_owned(),
            document,
            pages,
            sizes,
            scale: Cell::new(0.0),
            undrawn: RefCell::default(),
        }));
        self.selection.set(None);
        self.pending.set(None);
        self.stack.set_visible_child_name("pdf");
        self.find();
        Ok(())
    }

    /// Draws page `index` with what is chosen and found on it; a page out of view is left
    /// white until it is scrolled to.
    fn draw_page(&self, index: usize, cr: &gtk::cairo::Context) {
        let Some(pdf) = self.pdf.borrow().clone() else { return };
        cr.set_source_rgb(1.0, 1.0, 1.0);
        let _ = cr.paint();
        if pdf.scale.get() == 0.0 || !self.page_in_view(&pdf, index) {
            pdf.undrawn.borrow_mut().insert(index);
            return;
        }
        let Some(page) = pdf.document.page(index as i32) else { return };
        let scale = pdf.scale.get();
        cr.scale(scale, scale);
        page.render(cr);
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
    }

    /// Whether page `index` is in the column's view, or a screen away from it.
    fn page_in_view(&self, pdf: &Pdf, index: usize) -> bool {
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
        if self.jumping.get() || pdf.scale.get() == 0.0 {
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

    /// Selects the sidebar row of a page of the PDF shown, without scrolling to it.
    fn list_page(&self, pdf: &Pdf, page: usize) {
        let Some(first) = self.items.borrow().iter().position(|item| item.path == pdf.path && item.page == Some(1)) else {
            return;
        };
        let index = first + page;
        if index == self.current.get() {
            return;
        }
        self.current.set(index);
        self.syncing.set(true);
        if let Some(row) = self.list.row_at_index(index as i32) {
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
        let pdf = self.pdf.borrow().clone()?;
        let scale = pdf.scale.get();
        let index = pdf.page_at(y, scale);
        let point = self.pdf_column.compute_point(&pdf.pages[index], &gtk::graphene::Point::new(x as f32, y as f32))?;
        let (width, height) = pdf.sizes[index];
        Some((index, (f64::from(point.x()) / scale).clamp(0.0, width), (f64::from(point.y()) / scale).clamp(0.0, height)))
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

    fn select_all(&self) {
        let Some(pdf) = self.pdf.borrow().clone() else { return };
        let Some(&size) = pdf.sizes.last() else { return };
        let last = pdf.sizes.len() - 1;
        self.set_selection(Some(Selection { from: (0, 0.0, 0.0), to: (last, size.0, size.1), style: poppler::SelectionStyle::Glyph }));
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
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let mut found = Vec::new();
            for index in 0..pdf.sizes.len() {
                if weak.upgrade().is_none_or(|win| win.finding.get() != finding) {
                    return;
                }
                if let Some(page) = pdf.document.page(index as i32) {
                    found.extend(page_matches(&page, &text).into_iter().map(|place| (index, place)));
                }
                glib::timeout_future(std::time::Duration::ZERO).await;
            }
            let Some(win) = weak.upgrade().filter(|win| win.finding.get() == finding) else { return };
            let shown = win.item().and_then(|item| item.page).map_or(0, |page| page as usize - 1);
            let at = found.iter().position(|(page, _)| *page >= shown).or((!found.is_empty()).then_some(0));
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
            let view = self.pdf_scroll.vadjustment().page_size();
            self.pending.set(Some((page, y1 / pdf.sizes[page].1, -view / 3.0)));
            self.apply_pending();
            self.list_page(&pdf, page);
        }
        self.redraw_pages();
    }

    fn redraw_pages(&self) {
        if let Some(pdf) = self.pdf.borrow().clone() {
            pdf.pages.iter().for_each(|page| page.queue_draw());
        }
    }

    fn fail(&self, item: &Item, reason: &str) {
        *self.shown.borrow_mut() = None;
        self.show_nothing();
        alert::notice(&self.window, "無法開啟", Some(&format!("{}\n{reason}", item.path.display())));
    }

    fn show(&self, item: &Item, pixbuf: Pixbuf) {
        // The sidebar's thumbnail follows what was turned, cropped or drawn on.
        if let (Some(picture), true) = (self.thumbnails.borrow().get(self.current.get()), item.edited.borrow().is_some()) {
            let ratio = f64::from(THUMB_SIZE) / f64::from(pixbuf.width().max(pixbuf.height()));
            let size = |side: i32| ((f64::from(side) * ratio).round() as i32).max(1);
            if let Some(small) = pixbuf.scale_simple(size(pixbuf.width()), size(pixbuf.height()), gtk::gdk_pixbuf::InterpType::Bilinear) {
                picture.set_size_request(small.width(), small.height());
                picture.set_paintable(Some(&gdk::Texture::for_pixbuf(&small)));
            }
        }
        self.picture.set_paintable(Some(&gdk::Texture::for_pixbuf(&pixbuf)));
        *self.shown.borrow_mut() = Some(pixbuf);
        self.stack.set_visible_child_name("view");
        self.apply_zoom();
        self.update_title(item);
        self.update_actions();
    }

    fn step(self: &Rc<Self>, by: i32) {
        let count = self.items.borrow().len() as i32;
        let next = self.current.get() as i32 + by;
        if (0..count).contains(&next) {
            self.select(next as usize);
        }
    }

    fn update_title(&self, item: &Item) {
        let edited = if item.edited.borrow().is_some() { " — 已編輯" } else { "" };
        self.title.set_title(&format!("{}{edited}", item.name()));
        let subtitle = match (item.page, self.shown.borrow().as_ref()) {
            (Some(page), _) => format!("第 {page} 頁，共 {} 頁", self.pages_in(&item.path)),
            (None, Some(pixbuf)) => format!("{} × {}", pixbuf.width(), pixbuf.height()),
            (None, None) => String::new(),
        };
        self.title.set_subtitle(&subtitle);
        self.window.set_title(Some(&item.name()));
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
        // pdfseparate, which rewrites the pages, cannot open a PDF that needs a password.
        let is_pdf_file = item
            .as_ref()
            .is_some_and(|item| is_pdf(&item.path) && !self.passwords.borrow().contains_key(&item.path));
        let edited = item.as_ref().is_some_and(|item| item.edited.borrow().is_some());
        let has_items = self.items.borrow().len() > 1;
        let typing = self.typing.get();
        self.find_box.set_visible(is_page);
        for (name, on) in [
            ("save", is_picture && edited),
            ("save-as", is_picture),
            ("copy", !typing && (is_picture || is_page && self.selection.get().is_some())),
            ("select-all", !typing && is_page),
            ("find", is_page),
            ("find-next", is_page),
            ("find-previous", is_page),
            ("rotate-left", is_picture),
            ("rotate-right", is_picture),
            ("flip", is_picture),
            ("crop", is_picture && !self.editing()),
            ("markup", is_picture && !self.editing()),
            ("markup-done", self.tool.get().is_some()),
            ("undo", item.as_ref().is_some_and(|item| !item.undo.borrow().is_empty())),
            ("export-pdf", is_picture),
            ("page-up", is_page && is_pdf_file && self.current.get() > 0),
            ("page-down", is_page && is_pdf_file && self.current.get() + 1 < self.items.borrow().len()),
            ("insert-pdf", is_page && is_pdf_file),
            ("crop-apply", self.cropping.get()),
            ("crop-cancel", self.cropping.get()),
            ("resize", is_picture),
            ("delete-page", is_page && is_pdf_file && has_items),
            ("info", item.is_some()),
            ("zoom-in", item.is_some()),
            ("zoom-out", item.is_some()),
            ("zoom-fit", item.is_some()),
            ("zoom-actual", item.is_some()),
            ("zoom-toggle", item.is_some()),
            ("next", has_items && !typing),
            ("previous", has_items && !typing),
        ] {
            if let Some(action) = self.window.lookup_action(name).and_downcast::<gio::SimpleAction>() {
                action.set_enabled(on);
            }
        }
    }

    /// How many screen pixels each pixel of the picture takes now.
    /// How many screen pixels a pixel of the picture, or a point of the PDF, takes now.
    fn scale(&self) -> f64 {
        let Some(item) = self.item() else { return 1.0 };
        if let Zoom::Scale(scale) = self.zoom.get() {
            return scale;
        }
        if item.is_page() {
            let Some(pdf) = self.pdf.borrow().clone() else { return 1.0 };
            // A PDF's widest page fills the window's width, to be read scrolling down.
            let widest = pdf.sizes.iter().map(|&(width, _)| width).fold(1.0, f64::max);
            let across = self.pdf_scroll.hadjustment().page_size() - f64::from(2 * EDGE);
            return (across / widest).clamp(MIN_SCALE, MAX_SCALE);
        }
        let Some(pixbuf) = self.shown.borrow().clone() else { return 1.0 };
        let across = self.scroll.hadjustment().page_size() - f64::from(2 * EDGE);
        let down = self.scroll.vadjustment().page_size() - f64::from(2 * EDGE);
        // A picture smaller than the window stays as it is.
        (across / f64::from(pixbuf.width())).min(down / f64::from(pixbuf.height())).clamp(MIN_SCALE, 1.0)
    }

    fn apply_zoom(&self) {
        let Some(item) = self.item() else { return };
        let scale = self.scale();
        let fit = self.zoom.get() == Zoom::Fit;
        self.zoom_button.set_label(&format!("{}%", (scale * 100.0).round()));
        self.zoom_button.set_tooltip_text(Some(if fit { "實際大小" } else { "適合視窗" }));
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
    }

    fn set_zoom(&self, zoom: Zoom) {
        self.zoom.set(match zoom {
            Zoom::Scale(scale) => Zoom::Scale(scale.clamp(MIN_SCALE, MAX_SCALE)),
            Zoom::Fit => Zoom::Fit,
        });
        self.apply_zoom();
    }

    fn zoom_by(&self, factor: f64) {
        self.set_zoom(Zoom::Scale(self.scale() * factor));
    }

    /// A pixel of the picture, or a point of the PDF, to a pixel of the screen.
    fn set_actual(&self) {
        self.set_zoom(Zoom::Scale(1.0));
    }

    fn choose_files(self: &Rc<Self>) {
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("圖片與 PDF"));
        for mime in MIME_TYPES {
            filter.add_mime_type(mime);
        }
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        let dialog = gtk::FileDialog::builder()
            .title("開啟")
            .filters(&filters)
            .default_filter(&filter)
            .build();
        let weak = Rc::downgrade(self);
        dialog.open_multiple(Some(&self.window), gio::Cancellable::NONE, move |chosen| {
            // Closing the dialog without choosing is an error that needs no telling.
            let (Some(win), Ok(files)) = (weak.upgrade(), chosen) else { return };
            let paths = files
                .iter::<gio::File>()
                .filter_map(Result::ok)
                .filter_map(|file| file.path())
                .collect();
            win.open_files(paths);
        });
    }

    fn turn(self: &Rc<Self>, rotation: PixbufRotation) {
        self.edit(|pixbuf| pixbuf.rotate_simple(rotation));
    }

    fn flip(self: &Rc<Self>) {
        self.edit(|pixbuf| pixbuf.flip(true));
    }

    fn edit(self: &Rc<Self>, change: impl FnOnce(&Pixbuf) -> Option<Pixbuf>) {
        let (Some(item), Some(pixbuf)) = (self.item(), self.shown.borrow().clone()) else {
            return;
        };
        let Some(changed) = change(&pixbuf) else {
            return alert::notice(&self.window, "無法編輯", Some("記憶體不足。"));
        };
        let before = item.edited.borrow_mut().replace(changed.clone());
        item.undo.borrow_mut().push(before);
        self.show(&item, changed);
    }

    /// Whether a crop or a markup is in progress, which the pointer then draws instead of
    /// moving the picture.
    fn editing(&self) -> bool {
        self.cropping.get() || self.tool.get().is_some()
    }

    fn undo(self: &Rc<Self>) {
        let Some(item) = self.item() else { return };
        let Some(before) = item.undo.borrow_mut().pop() else { return };
        let reload = before.is_none();
        *item.edited.borrow_mut() = before.clone();
        match before {
            Some(pixbuf) => self.show(&item, pixbuf),
            None if reload => self.select(self.current.get()),
            None => {}
        }
    }

    fn start_markup(self: &Rc<Self>) {
        self.tool.set(Some(self.picked.get()));
        self.stroke.take();
        self.picture.set_cursor_from_name(Some("crosshair"));
        self.crop_bar.set_visible(false);
        self.markup_bar.set_visible(true);
        self.content.set_reveal_bottom_bars(true);
        self.update_actions();
    }

    fn finish_markup(self: &Rc<Self>) {
        self.tool.set(None);
        self.stroke.take();
        self.picture.set_cursor(None);
        self.markup_bar.set_visible(false);
        self.content.set_reveal_bottom_bars(false);
        self.crop_layer.queue_draw();
        self.update_actions();
    }

    /// The pointer was let go: the mark drawn is put on the picture, a text first asked for.
    fn finish_mark(self: &Rc<Self>) {
        let Some(tool) = self.tool.get() else { return };
        let points = self.stroke.take();
        self.crop_layer.queue_draw();
        if points.is_empty() {
            return;
        }
        let color = self.color.rgba();
        if tool != Tool::Text {
            self.edit(|pixbuf| paint(pixbuf, tool, color, &points, ""));
            return;
        }
        let weak = Rc::downgrade(self);
        alert::prompt(&self.window, "文字", "", -1, "加入", move |text| {
            if let Some(win) = weak.upgrade() {
                win.edit(|pixbuf| paint(pixbuf, Tool::Text, color, &points[..1], &text));
            }
        });
    }

    fn save(self: &Rc<Self>) {
        if let Some(item) = self.item() {
            self.write(&item, item.path.clone());
        }
    }

    fn save_as(self: &Rc<Self>) {
        self.save_as_type("");
    }

    /// 另存新檔, with the name's extension `extension` when that is not empty.
    fn save_as_type(self: &Rc<Self>, extension: &str) {
        let Some(item) = self.item() else { return };
        let name = match extension {
            "" => item.name(),
            _ => Path::new(&item.name()).with_extension(extension).to_string_lossy().into_owned(),
        };
        let dialog = gtk::FileDialog::builder().title("另存新檔").initial_name(name).build();
        let weak = Rc::downgrade(self);
        dialog.save(Some(&self.window), gio::Cancellable::NONE, move |chosen| {
            let (Some(win), Ok(file)) = (weak.upgrade(), chosen) else { return };
            if let Some(path) = file.path() {
                win.write(&item, path);
            }
        });
    }

    /// Writes the picture as shown to `path`, in the format its extension names.
    fn write(self: &Rc<Self>, item: &Rc<Item>, path: PathBuf) {
        let Some(pixbuf) = self.shown.borrow().clone() else { return };
        if needs_encoder(&path) {
            return match export_with_encoder(&pixbuf, &path) {
                Ok(()) => {}
                Err(err) => alert::notice(&self.window, "無法儲存", Some(&format!("{}\n{err}", path.display()))),
            };
        }
        if is_pdf(&path) {
            return match export_pdf(&pixbuf, &path) {
                Ok(()) => {}
                Err(err) => alert::notice(&self.window, "無法儲存", Some(&format!("{}\n{err}", path.display()))),
            };
        }
        let Some((format, options)) = save_format(&path) else {
            return alert::notice(&self.window, "無法儲存", Some("可儲存為 PNG、JPEG、TIFF、BMP、PDF、HEIC、JPEG 2000 或 OpenEXR。"));
        };
        let options: Vec<(&str, &str)> = options;
        match pixbuf.savev(&path, format, &options) {
            Ok(()) if path == item.path => {
                *item.edited.borrow_mut() = None;
                self.update_title(item);
                self.update_actions();
            }
            Ok(()) => {}
            Err(err) => alert::notice(&self.window, "無法儲存", Some(&format!("{}\n{err}", path.display()))),
        }
    }

    fn start_crop(self: &Rc<Self>) {
        self.cropping.set(true);
        self.crop.set(None);
        self.picture.set_can_target(true);
        self.picture.set_cursor_from_name(Some("crosshair"));
        self.markup_bar.set_visible(false);
        self.crop_bar.set_visible(true);
        self.content.set_reveal_bottom_bars(true);
        self.update_actions();
    }

    /// Leaves cropping, keeping the part chosen when `keep`.
    fn finish_crop(self: &Rc<Self>, keep: bool) {
        let chosen = self.crop.take();
        self.cropping.set(false);
        self.picture.set_cursor(None);
        self.content.set_reveal_bottom_bars(false);
        self.crop_layer.queue_draw();
        self.update_actions();
        let (true, Some((x0, y0, x1, y1))) = (keep, chosen) else { return };
        let scale = self.scale();
        let (x, y) = ((x0 / scale).round() as i32, (y0 / scale).round() as i32);
        let (width, height) = (((x1 - x0) / scale).round() as i32, ((y1 - y0) / scale).round() as i32);
        self.edit(|pixbuf| {
            let (width, height) = (width.min(pixbuf.width() - x), height.min(pixbuf.height() - y));
            (width >= 1 && height >= 1).then(|| pixbuf.new_subpixbuf(x, y, width, height).copy()).flatten()
        });
    }

    fn resize(self: &Rc<Self>) {
        let Some(pixbuf) = self.shown.borrow().clone() else { return };
        let (width, height) = (pixbuf.width(), pixbuf.height());
        let spin = |value: i32| gtk::SpinButton::with_range(1.0, 32768.0, 1.0).tap_value(value);
        let (across, down) = (spin(width), spin(height));
        let keep = gtk::CheckButton::builder().label("維持比例").active(true).build();
        let fields = gtk::Box::builder().spacing(GAP).halign(gtk::Align::Center).build();
        fields.append(&across);
        fields.append(&gtk::Label::new(Some("×")));
        fields.append(&down);
        let column = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(GAP).build();
        column.append(&fields);
        column.append(&keep);
        let ratio = f64::from(width) / f64::from(height);
        let settling = Rc::new(Cell::new(false));
        for (this, other, to_other) in [
            (&across, &down, (|value: f64, ratio: f64| value / ratio) as fn(f64, f64) -> f64),
            (&down, &across, |value, ratio| value * ratio),
        ] {
            let (other, keep, settling) = (other.clone(), keep.clone(), settling.clone());
            this.connect_value_changed(move |this| {
                if keep.is_active() && !settling.replace(true) {
                    other.set_value(to_other(this.value(), ratio).round().max(1.0));
                    settling.set(false);
                }
            });
        }
        let alert = adw::AlertDialog::new(Some("調整大小"), None);
        alert.set_extra_child(Some(&column));
        alert.add_responses(&[("cancel", "取消"), ("do", "套用")]);
        alert.set_response_appearance("do", adw::ResponseAppearance::Suggested);
        alert.set_default_response(Some("do"));
        alert.set_close_response("cancel");
        let weak = Rc::downgrade(self);
        alert.choose(&self.window, gio::Cancellable::NONE, move |response| {
            let Some(win) = weak.upgrade().filter(|_| response == "do") else { return };
            let (width, height) = (across.value_as_int(), down.value_as_int());
            win.edit(|pixbuf| pixbuf.scale_simple(width, height, gtk::gdk_pixbuf::InterpType::Hyper));
        });
    }

    /// Writes the PDF again with the pages `plan` names, then shows it at page `then`.
    fn rewrite_pdf(self: &Rc<Self>, path: PathBuf, plan: Vec<(PathBuf, u32)>, then: u32) {
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = gio::spawn_blocking({
                let path = path.clone();
                move || rebuild_pdf(&path, &plan)
            })
            .await
            .unwrap_or_else(|_| Err(String::new()));
            let Some(win) = weak.upgrade() else { return };
            match result {
                Ok(()) => {
                    win.open_files(vec![path]);
                    let last = win.items.borrow().len().saturating_sub(1);
                    win.select((then.saturating_sub(1) as usize).min(last));
                }
                Err(err) => alert::notice(&win.window, "無法寫入", Some(&err)),
            }
        });
    }

    /// The file's pages as a plan, each as it is now.
    fn pages_of(&self, path: &Path) -> Vec<(PathBuf, u32)> {
        self.items
            .borrow()
            .iter()
            .filter(|item| item.path == path)
            .filter_map(|item| Some((item.path.clone(), item.page?)))
            .collect()
    }

    /// Takes the page out of its PDF, which is written again without it.
    fn delete_page(self: &Rc<Self>) {
        let Some(item) = self.item() else { return };
        let Some(page) = item.page else { return };
        let weak = Rc::downgrade(self);
        alert::confirm(
            &self.window,
            &format!("刪除第 {page} 頁？"),
            Some(&format!("{} 會在沒有這一頁的情況下重新寫入。", item.name())),
            "刪除",
            move || {
                let Some(win) = weak.upgrade() else { return };
                let mut plan = win.pages_of(&item.path);
                plan.retain(|(_, kept)| *kept != page);
                win.rewrite_pdf(item.path.clone(), plan, page.saturating_sub(1).max(1));
            },
        );
    }

    /// Swaps the page shown with the one `by` places on, in the PDF itself.
    fn move_page(self: &Rc<Self>, by: i32) {
        let Some(item) = self.item() else { return };
        let Some(page) = item.page else { return };
        let mut plan = self.pages_of(&item.path);
        let (from, to) = (page as usize - 1, page as i64 + i64::from(by) - 1);
        if to < 0 || to as usize >= plan.len() {
            return;
        }
        plan.swap(from, to as usize);
        self.rewrite_pdf(item.path.clone(), plan, to as u32 + 1);
    }

    /// Puts every page of the PDFs chosen after the page shown.
    fn insert_pdf(self: &Rc<Self>) {
        let Some(item) = self.item() else { return };
        let Some(page) = item.page else { return };
        let filter = gtk::FileFilter::new();
        filter.add_mime_type("application/pdf");
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        let dialog = gtk::FileDialog::builder().title("插入").filters(&filters).build();
        let weak = Rc::downgrade(self);
        dialog.open_multiple(Some(&self.window), gio::Cancellable::NONE, move |chosen| {
            let (Some(win), Ok(files)) = (weak.upgrade(), chosen) else { return };
            let mut plan = win.pages_of(&item.path);
            let mut inserted = 0;
            for file in files.iter::<gio::File>().filter_map(Result::ok).filter_map(|file| file.path()) {
                match page_count(&file) {
                    Ok(pages) => {
                        let at = page as usize + inserted;
                        plan.splice(at..at, (1..=pages).map(|n| (file.clone(), n)));
                        inserted += pages as usize;
                    }
                    Err(err) => return alert::notice(&win.window, "無法插入", Some(&err)),
                }
            }
            win.rewrite_pdf(item.path.clone(), plan, page + 1);
        });
    }

    /// The text chosen in a PDF, or the picture.
    fn copy(&self) {
        let pdf = self.pdf.borrow().clone().filter(|_| self.item().is_some_and(|item| item.is_page()));
        if let Some(pdf) = pdf {
            let text = self.selection.get().map(|selection| selected_text(&pdf.document, &selection)).unwrap_or_default();
            if !text.is_empty() {
                self.window.clipboard().set_text(&text);
            }
            return;
        }
        if let Some(pixbuf) = self.shown.borrow().as_ref() {
            self.window.clipboard().set_texture(&gdk::Texture::for_pixbuf(pixbuf));
        }
    }

    fn show_info(&self) {
        let Some(item) = self.item() else { return };
        let file = gio::File::for_path(&item.path);
        let info = file
            .query_info("standard::*,time::modified", gio::FileQueryInfoFlags::NONE, gio::Cancellable::NONE)
            .ok();
        let kind = info
            .as_ref()
            .and_then(|info| info.content_type())
            .map(|content_type| gio::content_type_get_description(&content_type).to_string())
            .unwrap_or_default();
        let modified = info
            .as_ref()
            .and_then(|info| info.modification_date_time())
            .and_then(|when| Some(modified_label(&when.to_local().ok()?, &glib::DateTime::now_local().ok()?)))
            .unwrap_or_default();
        let size = info.as_ref().map(|info| glib::format_size(info.size() as u64).to_string()).unwrap_or_default();
        let mut rows = vec![("名稱", item.name()), ("種類", kind)];
        match (item.page, self.shown.borrow().as_ref()) {
            (Some(page), _) => rows.push(("頁數", format!("第 {page} 頁，共 {} 頁", self.pages_in(&item.path)))),
            (None, Some(pixbuf)) => rows.push(("尺寸", format!("{} × {} 像素", pixbuf.width(), pixbuf.height()))),
            (None, None) => {}
        }
        rows.push(("大小", size));
        rows.push(("修改日期", modified));
        rows.push(("位置", item.path.parent().map(|dir| dir.display().to_string()).unwrap_or_default()));

        let group = adw::PreferencesGroup::new();
        for (title, value) in rows.into_iter().filter(|(_, value)| !value.is_empty()) {
            let row = adw::ActionRow::builder()
                .title(title)
                .subtitle(glib::markup_escape_text(&value))
                .subtitle_selectable(true)
                .css_classes(["property"])
                .build();
            group.add(&row);
        }
        let page = adw::PreferencesPage::new();
        page.add(&group);
        let view = adw::ToolbarView::new();
        view.add_top_bar(&adw::HeaderBar::new());
        view.set_content(Some(&page));
        let dialog = adw::Dialog::builder()
            .title("取得資訊")
            .content_width(400)
            .content_height(420)
            .child(&view)
            .build();
        dialog.present(Some(&self.window));
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
    // The bars at the foot take the same edge and gap as the rest.
    "
.pv-bar > revealer > box { padding: 8px 12px; }
.pv-sheet { box-shadow: 0 0 0 1px alpha(currentColor, .12), 0 1px 4px alpha(black, .2); }
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
        for (action, keys) in [
            ("win.open", &["<Control>o"][..]),
            ("win.save", &["<Control>s"]),
            ("win.save-as", &["<Control><Shift>s"]),
            ("win.copy", &["<Control>c"]),
            ("win.rotate-left", &["<Control>l"]),
            ("win.rotate-right", &["<Control>r"]),
            ("win.info", &["<Control>i"]),
            ("win.zoom-in", &["<Control>plus", "<Control>equal"]),
            ("win.zoom-out", &["<Control>minus"]),
            ("win.zoom-actual", &["<Control>0"]),
            ("win.zoom-fit", &["<Control>9"]),
            ("win.crop", &["<Control>k"]),
            ("win.markup", &["<Control>m"]),
            ("win.undo", &["<Control>z"]),
            ("win.crop-apply", &["Return"]),
            ("win.crop-cancel", &["Escape"]),
            ("win.next", &["Right", "Page_Down"]),
            ("win.previous", &["Left", "Page_Up"]),
            ("win.find", &["<Control>f"]),
            ("win.find-next", &["<Control>g"]),
            ("win.find-previous", &["<Control><Shift>g"]),
            ("win.select-all", &["<Control>a"]),
        ] {
            app.set_accels_for_action(action, keys);
        }
    });
    app.connect_shutdown(|_| {
        // Only the pages this run drew.
        let _ = std::fs::remove_dir_all(scratch_dir());
    });
    let windows: Rc<RefCell<Vec<Rc<Win>>>> = Rc::default();
    // A window is kept alive until it is destroyed.
    let keep = {
        let windows = windows.clone();
        move |win: Rc<Win>| {
            let (all, closing) = (windows.clone(), Rc::downgrade(&win));
            win.window.connect_destroy(move |_| {
                if let Some(closing) = closing.upgrade() {
                    all.borrow_mut().retain(|kept| !Rc::ptr_eq(kept, &closing));
                }
            });
            windows.borrow_mut().push(win);
        }
    };
    let on_activate = keep.clone();
    app.connect_activate(move |app| match app.active_window() {
        Some(window) => window.present(),
        None => on_activate(Win::open(app)),
    });
    app.connect_open(move |app, files, _| {
        let win = Win::open(app);
        win.open_files(files.iter().filter_map(|file| file.path()).collect());
        keep(win);
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
        let marked = paint(&blank, Tool::Rectangle, red, &[(10.0, 10.0), (60.0, 60.0)], "").unwrap();
        assert_eq!(pixel(&marked, 10, 30), [255, 0, 0]);
        assert_eq!(pixel(&marked, 30, 30), [255, 255, 255]);
        assert_eq!(pixel(&marked, 90, 90), [255, 255, 255]);
    }

    #[test]
    fn text_is_drawn_where_it_was_put() {
        let blank = Pixbuf::new(gtk::gdk_pixbuf::Colorspace::Rgb, false, 8, 200, 100).unwrap();
        blank.fill(0xffff_ffff);
        let black = gdk::RGBA::new(0.0, 0.0, 0.0, 1.0);
        let marked = paint(&blank, Tool::Text, black, &[(10.0, 10.0)], "MMM").unwrap();
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
            export_with_encoder(&blank, &path).unwrap_or_else(|err| panic!("{extension}: {err}"));
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
            document: open_pdf(&path, None).unwrap(),
            pages: Vec::new(),
            sizes: vec![(200.0, 100.0), (200.0, 300.0), (200.0, 100.0)],
            scale: Cell::new(2.0),
            undrawn: RefCell::default(),
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
}
