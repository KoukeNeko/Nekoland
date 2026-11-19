//! `nekoland-preview`, 預覽程式: pictures and PDFs, one or several, in a window of their own.
//! A sidebar of thumbnails goes through the files, or a PDF's pages; the picture fits the
//! window or is zoomed (Ctrl+scroll, pinch, or the buttons) and dragged. A picture can be
//! turned or flipped and saved. Every edge is `EDGE` and every gap `GAP`, as in Files.

use std::{
    cell::{Cell, RefCell},
    collections::hash_map::DefaultHasher,
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
/// The resolution a PDF page is drawn at; one PDF point is then `POINT_SCALE` of a pixel.
const PDF_DPI: u32 = 144;
const THUMB_DPI: u32 = 24;
const POINT_SCALE: f64 = 72.0 / PDF_DPI as f64;
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

    /// The scale that shows it at its real size.
    fn actual(&self) -> f64 {
        if self.is_page() {
            POINT_SCALE
        } else {
            1.0
        }
    }

    fn name(&self) -> String {
        self.path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
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
    shown: RefCell<Option<Pixbuf>>,
    /// Which showing is the latest, so a PDF page drawn late is not put over a newer one.
    generation: Cell<u32>,
    syncing: Cell<bool>,
    discard_edits: Cell<bool>,
    scratch: PathBuf,
}

/// In the user's own cache, not the shared temporary folder, where another user could have
/// made the folder first.
fn scratch_dir() -> PathBuf {
    glib::user_cache_dir().join(format!("nekoland/preview-{}", std::process::id()))
}

/// How many pages a PDF has.
fn page_count(path: &Path) -> Result<u32, String> {
    let output = Command::new("pdfinfo")
        .arg(path)
        .output()
        .map_err(|err| format!("pdfinfo: {err}"))?;
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| line.strip_prefix("Pages:")?.trim().parse().ok())
        .ok_or_else(|| String::from_utf8_lossy(&output.stderr).trim().to_owned())
}

/// Draws one page of a PDF to a PNG in `dir`, unless it is there already.
fn render_page(path: &Path, page: u32, dpi: u32, dir: &Path) -> Result<PathBuf, String> {
    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);
    // A PDF written again must not show the pages it had.
    if let Ok(meta) = std::fs::metadata(path) {
        (meta.len(), meta.modified().ok()).hash(&mut hasher);
    }
    let prefix = dir.join(format!("{:x}-{page}-{dpi}", hasher.finish()));
    let png = prefix.with_extension("png");
    if png.exists() {
        return Ok(png);
    }
    std::fs::create_dir_all(dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    let output = Command::new("pdftoppm")
        .args(["-png", "-singlefile", "-r", &dpi.to_string(), "-f", &page.to_string(), "-l", &page.to_string()])
        .arg(path)
        .arg(&prefix)
        .output()
        .map_err(|err| format!("pdftoppm: {err}"))?;
    if output.status.success() && png.exists() {
        Ok(png)
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
    }
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
        let busy = gtk::Spinner::builder()
            .spinning(true)
            .width_request(32)
            .height_request(32)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .build();
        let stack = gtk::Stack::new();
        stack.add_named(&empty, Some("empty"));
        stack.add_named(&busy, Some("busy"));
        stack.add_named(&scroll, Some("view"));

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
        let header = adw::HeaderBar::builder().title_widget(&title).build();
        header.pack_start(&sidebar_toggle);
        header.pack_end(&menu_button);
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
            generation: Cell::new(0),
            syncing: Cell::new(false),
            discard_edits: Cell::new(false),
            scratch: scratch_dir(),
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
        for adjustment in [self.scroll.hadjustment(), self.scroll.vadjustment()] {
            let weak = weak.clone();
            adjustment.connect_notify_local(Some("page-size"), move |_, _| {
                if let Some(win) = weak.upgrade() {
                    if win.zoom.get() == Zoom::Fit {
                        win.apply_zoom();
                    }
                }
            });
        }

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
        self.scroll.add_controller(wheel);

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
        self.scroll.add_controller(pinch);

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
        for path in paths {
            if opens_as_pdf(&path) {
                match page_count(&path) {
                    Ok(pages) => items.extend((1..=pages).map(|page| {
                        Rc::new(Item { path: path.clone(), page: Some(page), edited: RefCell::new(None), undo: RefCell::default() })
                    })),
                    Err(err) => failed.push(format!("{}: {err}", path.display())),
                }
            } else {
                items.push(Rc::new(Item { path, page: None, edited: RefCell::new(None), undo: RefCell::default() }));
            }
        }
        if !failed.is_empty() {
            alert::notice(&self.window, "無法開啟", Some(&failed.join("\n")));
        }
        if items.is_empty() {
            return;
        }
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
        let scratch = self.scratch.clone();
        glib::spawn_future_local(async move {
            for (item, picture) in thumbnails {
                if weak.upgrade().is_none() {
                    return;
                }
                let file = match item.page {
                    Some(page) => {
                        let (path, scratch) = (item.path.clone(), scratch.clone());
                        gio::spawn_blocking(move || render_page(&path, page, THUMB_DPI, &scratch))
                            .await
                            .unwrap_or_else(|_| Err(String::new()))
                            .ok()
                    }
                    None => Some(item.path.clone()),
                };
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
        let generation = self.generation.get().wrapping_add(1);
        self.generation.set(generation);

        if let Some(edited) = item.edited.borrow().clone() {
            return self.show(&item, edited);
        }
        match item.page {
            None => match load_picture(&item.path) {
                Ok(pixbuf) => self.show(&item, pixbuf.apply_embedded_orientation().unwrap_or(pixbuf)),
                Err(err) => self.fail(&item, &err),
            },
            Some(page) => {
                self.stack.set_visible_child_name("busy");
                let weak = Rc::downgrade(self);
                let (path, scratch) = (item.path.clone(), self.scratch.clone());
                glib::spawn_future_local(async move {
                    let drawn = gio::spawn_blocking(move || render_page(&path, page, PDF_DPI, &scratch))
                        .await
                        .unwrap_or_else(|_| Err(String::new()));
                    let Some(win) = weak.upgrade() else { return };
                    if win.generation.get() != generation {
                        return;
                    }
                    match drawn.and_then(|png| Pixbuf::from_file(png).map_err(|err| err.to_string())) {
                        Ok(pixbuf) => win.show(&item, pixbuf),
                        Err(err) => win.fail(&item, &err),
                    }
                });
            }
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
            (Some(page), _) => format!("第 {page} 頁，共 {} 頁", self.items.borrow().len()),
            (None, Some(pixbuf)) => format!("{} × {}", pixbuf.width(), pixbuf.height()),
            (None, None) => String::new(),
        };
        self.title.set_subtitle(&subtitle);
        self.window.set_title(Some(&item.name()));
    }

    fn update_actions(&self) {
        let item = self.item().filter(|_| self.shown.borrow().is_some());
        let is_picture = item.as_ref().is_some_and(|item| !item.is_page());
        let is_page = item.as_ref().is_some_and(|item| item.is_page());
        let is_pdf_file = item.as_ref().is_some_and(|item| is_pdf(&item.path));
        let edited = item.as_ref().is_some_and(|item| item.edited.borrow().is_some());
        let has_items = self.items.borrow().len() > 1;
        for (name, on) in [
            ("save", is_picture && edited),
            ("save-as", is_picture),
            ("copy", item.is_some()),
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
            ("next", has_items),
            ("previous", has_items),
        ] {
            if let Some(action) = self.window.lookup_action(name).and_downcast::<gio::SimpleAction>() {
                action.set_enabled(on);
            }
        }
    }

    /// How many screen pixels each pixel of the picture takes now.
    fn scale(&self) -> f64 {
        let (Some(item), Some(pixbuf)) = (self.item(), self.shown.borrow().clone()) else {
            return 1.0;
        };
        match self.zoom.get() {
            Zoom::Scale(scale) => scale,
            Zoom::Fit => {
                let across = self.scroll.hadjustment().page_size() - f64::from(2 * EDGE);
                let down = self.scroll.vadjustment().page_size() - f64::from(2 * EDGE);
                // A picture smaller than the window stays as it is.
                (across / f64::from(pixbuf.width()))
                    .min(down / f64::from(pixbuf.height()))
                    .min(item.actual())
                    .max(MIN_SCALE)
            }
        }
    }

    fn apply_zoom(&self) {
        let (Some(item), Some(pixbuf)) = (self.item(), self.shown.borrow().clone()) else {
            return;
        };
        let scale = self.scale();
        let policy = if self.zoom.get() == Zoom::Fit { gtk::PolicyType::Never } else { gtk::PolicyType::Automatic };
        self.scroll.set_hscrollbar_policy(policy);
        self.scroll.set_vscrollbar_policy(policy);
        self.picture.set_size_request(
            ((f64::from(pixbuf.width()) * scale).round() as i32).max(1),
            ((f64::from(pixbuf.height()) * scale).round() as i32).max(1),
        );
        self.zoom_button.set_label(&format!("{}%", (scale / item.actual() * 100.0).round()));
        self.zoom_button.set_tooltip_text(Some(if self.zoom.get() == Zoom::Fit { "實際大小" } else { "適合視窗" }));
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

    fn set_actual(&self) {
        if let Some(item) = self.item() {
            self.set_zoom(Zoom::Scale(item.actual()));
        }
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

    fn copy(&self) {
        if let Some(pixbuf) = self.shown.borrow().as_ref() {
            self.window.clipboard().set_texture(&gdk::Texture::for_pixbuf(pixbuf));
        }
    }

    fn show_info(&self) {
        let (Some(item), Some(pixbuf)) = (self.item(), self.shown.borrow().clone()) else {
            return;
        };
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
        match item.page {
            Some(page) => rows.push(("頁數", format!("第 {page} 頁，共 {} 頁", self.items.borrow().len()))),
            None => rows.push(("尺寸", format!("{} × {} 像素", pixbuf.width(), pixbuf.height()))),
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
