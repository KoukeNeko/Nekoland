//! Text and icons drawn by the compositor itself, for the overview and the menu bar.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use cairo::{Context, Format, ImageSurface};
use smithay::{
    backend::{allocator::Fourcc, renderer::element::memory::MemoryRenderBuffer},
    utils::{Logical, Size, Transform},
};

const FONT: &str = "Ubuntu, Noto Sans CJK TC, Sans";
const ICON_THEMES: [&str; 3] = ["Yaru", "Adwaita", "hicolor"];
// Largest first: icons are drawn smaller than this more often than larger.
const ICON_SIZES: [&str; 6] = ["256x256", "128x128", "96x96", "64x64", "48x48", "32x32"];

/// Something drawn once and shown many times.
#[derive(Clone)]
pub struct Picture {
    pub buffer: MemoryRenderBuffer,
    pub size: Size<i32, Logical>,
}

pub fn picture(mut surface: ImageSurface) -> Option<Picture> {
    let size = Size::from((surface.width(), surface.height()));
    surface.flush();
    let data = surface.data().ok()?;
    Some(Picture {
        // Cairo's ARGB32 is premultiplied and in native byte order, which is this format.
        buffer: MemoryRenderBuffer::from_slice(&data, Fourcc::Argb8888, size, 1, Transform::Normal, None),
        size: Size::from((size.w, size.h)),
    })
}

pub fn rounded_rectangle(cr: &Context, x: f64, y: f64, width: f64, height: f64, radius: f64) {
    use std::f64::consts::{FRAC_PI_2, PI};
    cr.new_sub_path();
    cr.arc(x + width - radius, y + radius, radius, -FRAC_PI_2, 0.0);
    cr.arc(x + width - radius, y + height - radius, radius, 0.0, FRAC_PI_2);
    cr.arc(x + radius, y + height - radius, radius, FRAC_PI_2, PI);
    cr.arc(x + radius, y + radius, radius, PI, 3.0 * FRAC_PI_2);
    cr.close_path();
}

fn layout(cr: &Context, text: &str, points: i32, bold: bool, max_width: i32) -> pango::Layout {
    let layout = pangocairo::functions::create_layout(cr);
    let weight = if bold { " Semi-Bold" } else { "" };
    let font = pango::FontDescription::from_string(&format!("{FONT}{weight} {points}"));
    layout.set_font_description(Some(&font));
    layout.set_text(text);
    layout.set_width(max_width * pango::SCALE);
    layout.set_ellipsize(pango::EllipsizeMode::End);
    layout.set_alignment(pango::Alignment::Center);
    layout
}

/// A line of white text with a soft shadow, as wide as it needs up to `max_width`,
/// on a rounded pill if `pill` says how opaque.
pub fn label(text: &str, points: i32, max_width: i32, pill: Option<f64>) -> Option<Picture> {
    picture(label_surface(text, points, max_width, pill)?)
}

fn label_surface(text: &str, points: i32, max_width: i32, pill: Option<f64>) -> Option<ImageSurface> {
    const PAD_X: i32 = 14;
    const PAD_Y: i32 = 6;

    // Measured on a surface of no size first; the real one needs the result.
    let probe = Context::new(ImageSurface::create(Format::ARgb32, 1, 1).ok()?).ok()?;
    let measured = layout(&probe, text, points, true, max_width);
    measured.set_width(-1);
    let (text_width, text_height) = measured.pixel_size();
    let text_width = text_width.min(max_width);

    let width = text_width + PAD_X * 2;
    let height = text_height + PAD_Y * 2;
    let surface = ImageSurface::create(Format::ARgb32, width, height).ok()?;
    {
        let cr = Context::new(&surface).ok()?;
        if let Some(opacity) = pill {
            rounded_rectangle(&cr, 0.0, 0.0, f64::from(width), f64::from(height), 9.0);
            cr.set_source_rgba(1.0, 1.0, 1.0, opacity);
            cr.fill().ok()?;
        }
        let text = layout(&cr, text, points, true, text_width);
        for (dx, dy, (r, g, b, a)) in [
            (0.0, 1.0, (0.0, 0.0, 0.0, 0.55)),
            (0.0, 0.0, (1.0, 1.0, 1.0, 1.0)),
        ] {
            cr.move_to(f64::from(PAD_X) + dx, f64::from(PAD_Y) + dy);
            cr.set_source_rgba(r, g, b, a);
            pangocairo::functions::show_layout(&cr, &text);
        }
    }
    Some(surface)
}

/// What a program is called and looks like, from its desktop entry.
#[derive(Clone, Default)]
pub struct AppInfo {
    pub name: Option<String>,
    icon: Option<PathBuf>,
}

/// Desktop entries and icons, looked up once per application.
#[derive(Default)]
pub struct Apps {
    known: HashMap<String, AppInfo>,
}

impl Apps {
    pub fn info(&mut self, app_id: &str) -> &AppInfo {
        self.known
            .entry(app_id.to_string())
            .or_insert_with(|| lookup(app_id))
    }

    /// The icon over the name, as it sits under a window in the overview.
    pub fn badge(&mut self, app_id: &str, fallback_name: &str, icon_size: i32) -> Option<Picture> {
        let info = self.info(app_id).clone();
        let name = info
            .name
            .as_deref()
            .filter(|name| !name.is_empty())
            .unwrap_or(fallback_name);
        let name = label_surface(name, 13, 360, None)?;

        const GAP: i32 = 4;
        let width = name.width().max(icon_size);
        let height = icon_size + GAP + name.height();
        let surface = ImageSurface::create(Format::ARgb32, width, height).ok()?;
        {
            let cr = Context::new(&surface).ok()?;
            let icon_x = f64::from(width - icon_size) / 2.0;
            let drawn = info
                .icon
                .as_deref()
                .and_then(|path| draw_icon(&cr, path, icon_x, 0.0, icon_size))
                .is_some();
            if !drawn {
                // No icon to be found: a plain tile in its place.
                let size = f64::from(icon_size);
                rounded_rectangle(&cr, icon_x, 0.0, size, size, size * 0.22);
                cr.set_source_rgba(0.35, 0.35, 0.38, 1.0);
                cr.fill().ok()?;
            }

            cr.set_source_surface(
                &name,
                f64::from(width - name.width()) / 2.0,
                f64::from(icon_size + GAP),
            )
            .ok()?;
            cr.paint().ok()?;
        }
        picture(surface)
    }
}

fn draw_icon(cr: &Context, path: &Path, x: f64, y: f64, size: i32) -> Option<()> {
    let icon = ImageSurface::create_from_png(&mut std::fs::File::open(path).ok()?).ok()?;
    let scale = f64::from(size) / f64::from(icon.width().max(icon.height()).max(1));
    cr.save().ok()?;
    cr.translate(x, y);
    cr.scale(scale, scale);
    cr.set_source_surface(&icon, 0.0, 0.0).ok()?;
    cr.paint().ok()?;
    cr.restore().ok()
}

/// An icon by name from the icon themes, or from `extra_dir` first, `size` pixels square.
/// Symbolic icons are drawn in `tint` instead of their own grey.
pub fn named_icon(name: &str, extra_dir: &str, size: i32, tint: (f64, f64, f64)) -> Option<ImageSurface> {
    let path = find_named_icon(name, extra_dir)?;
    let pixbuf = gdk_pixbuf::Pixbuf::from_file_at_size(&path, size, size).ok()?;
    let symbolic = name.ends_with("-symbolic") || path.to_string_lossy().contains("symbolic");
    pixbuf_surface(&pixbuf, symbolic.then_some(tint))
}

/// An icon a program drew itself: ARGB, in network byte order, not premultiplied.
pub fn pixmap_icon(width: i32, height: i32, argb: &[u8]) -> Option<ImageSurface> {
    let mut surface = ImageSurface::create(Format::ARgb32, width, height).ok()?;
    let stride = surface.stride() as usize;
    {
        let mut data = surface.data().ok()?;
        for (index, pixel) in argb.chunks_exact(4).enumerate() {
            let (y, x) = (index / width as usize, index % width as usize);
            let [a, r, g, b] = [pixel[0], pixel[1], pixel[2], pixel[3]];
            let premultiply = |channel: u8| (u16::from(channel) * u16::from(a) / 255) as u8;
            let native = u32::from_be_bytes([a, premultiply(r), premultiply(g), premultiply(b)]);
            let at = y * stride + x * 4;
            data[at..at + 4].copy_from_slice(&native.to_ne_bytes());
        }
    }
    Some(surface)
}

fn pixbuf_surface(pixbuf: &gdk_pixbuf::Pixbuf, tint: Option<(f64, f64, f64)>) -> Option<ImageSurface> {
    let (width, height) = (pixbuf.width(), pixbuf.height());
    let channels = pixbuf.n_channels() as usize;
    let rowstride = pixbuf.rowstride() as usize;
    let bytes = pixbuf.read_pixel_bytes();
    let mut surface = ImageSurface::create(Format::ARgb32, width, height).ok()?;
    let stride = surface.stride() as usize;
    {
        let mut data = surface.data().ok()?;
        for y in 0..height as usize {
            for x in 0..width as usize {
                let pixel = &bytes[y * rowstride + x * channels..];
                let a = if channels == 4 { pixel[3] } else { 255 };
                let (r, g, b) = match tint {
                    Some((r, g, b)) => ((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8),
                    None => (pixel[0], pixel[1], pixel[2]),
                };
                let premultiply = |channel: u8| (u16::from(channel) * u16::from(a) / 255) as u8;
                let native = u32::from_be_bytes([a, premultiply(r), premultiply(g), premultiply(b)]);
                let at = y * stride + x * 4;
                data[at..at + 4].copy_from_slice(&native.to_ne_bytes());
            }
        }
    }
    Some(surface)
}

/// Any icon of that name, SVG or PNG, in any size or context of the themes.
fn find_named_icon(name: &str, extra_dir: &str) -> Option<PathBuf> {
    let path = Path::new(name);
    if path.is_absolute() {
        return path.is_file().then(|| path.to_path_buf());
    }
    let mut roots: Vec<PathBuf> = Vec::new();
    if !extra_dir.is_empty() {
        roots.push(extra_dir.into());
    }
    for dir in data_dirs() {
        roots.extend(ICON_THEMES.iter().map(|theme| dir.join("icons").join(theme)));
        roots.push(dir.join("pixmaps"));
    }
    // shortcut: walks the theme folders on each lookup; callers keep what they draw.
    roots.iter().find_map(|root| find_file(root, name, 4))
}

fn find_file(dir: &Path, stem: &str, depth: usize) -> Option<PathBuf> {
    let entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .collect();
    // Scalable first, so that a symbolic SVG beats a small bitmap.
    let found = ["svg", "png"].iter().find_map(|ext| {
        entries
            .iter()
            .find(|path| {
                path.file_stem().and_then(|s| s.to_str()) == Some(stem)
                    && path.extension().and_then(|e| e.to_str()) == Some(ext)
            })
            .cloned()
    });
    if found.is_some() || depth == 0 {
        return found;
    }
    let mut subdirs: Vec<&PathBuf> = entries.iter().filter(|path| path.is_dir()).collect();
    subdirs.sort_by_key(|path| {
        !path.to_string_lossy().contains("scalable") && !path.to_string_lossy().contains("symbolic")
    });
    subdirs
        .into_iter()
        .find_map(|sub| find_file(sub, stem, depth - 1))
}

fn data_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let mut dirs: Vec<PathBuf> = home.iter().map(|home| home.join(".local/share")).collect();
    let system = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    dirs.extend(system.split(':').filter(|dir| !dir.is_empty()).map(PathBuf::from));
    dirs.push("/var/lib/snapd/desktop".into());
    dirs
}

fn lookup(app_id: &str) -> AppInfo {
    let dirs = data_dirs();
    let Some(entry) = desktop_entry(&dirs, app_id) else {
        return AppInfo {
            name: None,
            icon: find_icon(&dirs, &app_id.to_lowercase()),
        };
    };
    let value = |key: &str| {
        entry
            .lines()
            .skip_while(|line| line.trim() != "[Desktop Entry]")
            .skip(1)
            .take_while(|line| !line.starts_with('['))
            .find_map(|line| {
                line.strip_prefix(key)?
                    .strip_prefix('=')
                    .map(|value| value.trim().to_string())
            })
    };
    AppInfo {
        name: value("Name"),
        icon: value("Icon").and_then(|icon| find_icon(&dirs, &icon)),
    }
}

/// The desktop entry named after the application, or failing that the one that claims its window class.
fn desktop_entry(dirs: &[PathBuf], app_id: &str) -> Option<String> {
    let wanted = app_id.to_lowercase();
    let mut by_class = None;
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir.join("applications")) else {
            continue;
        };
        for path in entries.flatten().map(|entry| entry.path()) {
            if path.extension().and_then(|ext| ext.to_str()) != Some("desktop") {
                continue;
            }
            let stem = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or_default()
                .to_lowercase();
            if stem == wanted {
                return std::fs::read_to_string(path).ok();
            }
            if by_class.is_none() {
                if let Ok(text) = std::fs::read_to_string(&path) {
                    let claims = text
                        .lines()
                        .filter_map(|line| line.strip_prefix("StartupWMClass="))
                        .any(|class| class.trim().to_lowercase() == wanted);
                    if claims {
                        by_class = Some(text);
                    }
                }
            }
        }
    }
    by_class
}

// shortcut: PNG icons only; an application that ships just an SVG gets the plain tile.
fn find_icon(dirs: &[PathBuf], icon: &str) -> Option<PathBuf> {
    let path = Path::new(icon);
    if path.is_absolute() {
        return (path.extension().and_then(|ext| ext.to_str()) == Some("png") && path.is_file())
            .then(|| path.to_path_buf());
    }
    for dir in dirs {
        for theme in ICON_THEMES {
            for size in ICON_SIZES {
                let candidate = dir.join(format!("icons/{theme}/{size}/apps/{icon}.png"));
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
        let candidate = dir.join(format!("pixmaps/{icon}.png"));
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}
