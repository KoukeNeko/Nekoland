//! Text and icons drawn by the compositor itself, for the overview and the menu bar.

use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicI32, Ordering},
        Mutex, RwLock,
    },
};

use cairo::{Context, Format, ImageSurface};
use gio::glib;
use smithay::{
    backend::{allocator::Fourcc, renderer::element::memory::MemoryRenderBuffer},
    utils::{Logical, Size, Transform},
};

/// The family Nekoland's own text is set in, the configuration's `font`, before the
/// families that fill in what it lacks: CJK first.
static FAMILY: RwLock<String> = RwLock::new(String::new());
const FALLBACK: &str = "Noto Sans CJK TC, Sans";
const ICON_THEMES: [&str; 3] = ["Yaru", "Adwaita", "hicolor"];
/// The configuration's `icon_theme`, looked in before `ICON_THEMES`.
static ICON_THEME: RwLock<String> = RwLock::new(String::new());

/// How many pixels a point of the compositor's own pictures is drawn with: the output's
/// scale, rounded up, so that a fractionally scaled output shrinks them rather than
/// enlarging them, and they stay as sharp as its clients.
static PICTURE_SCALE: AtomicI32 = AtomicI32::new(1);

// shortcut: one scale for all outputs, the last one set up, as the menu bar and Dock are on
// the first only; pictures drawn before a scale change keep their old density until redrawn.
pub fn set_picture_scale(scale: f64) {
    PICTURE_SCALE.store((scale.ceil() as i32).max(1), Ordering::Relaxed);
}

pub fn picture_scale() -> i32 {
    PICTURE_SCALE.load(Ordering::Relaxed)
}

/// A surface to draw a picture on, `width` by `height` points, with as many pixels to each
/// point as the outputs show; cairo, and the text on it, work in points.
pub fn canvas(width: i32, height: i32) -> Result<ImageSurface, cairo::Error> {
    let scale = picture_scale();
    let surface = ImageSurface::create(Format::ARgb32, width * scale, height * scale)?;
    surface.set_device_scale(f64::from(scale), f64::from(scale));
    Ok(surface)
}

/// A text layout for `cr`, drawn as the Mac draws text: unhinted outlines, grey
/// antialiasing and glyphs placed at fractions of a pixel, so that a word keeps its
/// shape and spacing at any size rather than being snapped to the pixel grid.
pub fn new_layout(cr: &Context) -> pango::Layout {
    let context = pangocairo::functions::create_context(cr);
    let mut options = cairo::FontOptions::new().expect("cairo font options");
    options.set_antialias(cairo::Antialias::Gray);
    options.set_hint_style(cairo::HintStyle::None);
    options.set_hint_metrics(cairo::HintMetrics::Off);
    pangocairo::functions::context_set_font_options(&context, Some(&options));
    context.set_round_glyph_positions(false);
    pango::Layout::new(&context)
}

/// Text that names no weight of its own is set at medium, where the Mac thickens every
/// outline: unhinted text with grey edges otherwise reads thin and frayed. CJK outlines
/// are thickened by FreeType instead; see `main`.
const TEXT_WEIGHT: pango::Weight = pango::Weight::Medium;

pub fn set_family(family: &str) {
    *FAMILY.write().unwrap() = format!("{family}, {FALLBACK}");
}

/// The font `description` names, in pango's words, in Nekoland's family if it names no
/// family, and at `TEXT_WEIGHT` if it names no weight.
// shortcut: icons already drawn keep the theme they were found in until drawn again.
pub fn set_icon_theme(theme: &str) {
    *ICON_THEME.write().unwrap() = theme.to_string();
    FOUND_ICONS.lock().unwrap().clear();
}

/// Where icons were found, by name and the folder looked in first, until the theme changes:
/// a theme holds thousands of files, too many to walk for each icon drawn.
static FOUND_ICONS: Mutex<BTreeMap<(String, String), PathBuf>> = Mutex::new(BTreeMap::new());

fn icon_themes() -> Vec<String> {
    let chosen = ICON_THEME.read().unwrap().clone();
    std::iter::once(chosen)
        .filter(|theme| !theme.is_empty())
        .chain(ICON_THEMES.map(String::from))
        .collect()
}

pub fn font(description: &str) -> pango::FontDescription {
    let mut font = pango::FontDescription::from_string(description);
    if font.family().is_none() {
        let family = FAMILY.read().unwrap();
        font.set_family(if family.is_empty() { FALLBACK } else { &family });
    }
    if font.weight() == pango::Weight::Normal {
        font.set_weight(TEXT_WEIGHT);
    }
    font
}

/// Something drawn once and shown many times.
#[derive(Clone)]
pub struct Picture {
    pub buffer: MemoryRenderBuffer,
    pub size: Size<i32, Logical>,
}

pub fn picture(mut surface: ImageSurface) -> Option<Picture> {
    let pixels = Size::from((surface.width(), surface.height()));
    let scale = (surface.device_scale().0.round() as i32).max(1);
    surface.flush();
    let data = surface.data().ok()?;
    Some(Picture {
        // Cairo's ARGB32 is premultiplied and in native byte order, which is this format.
        buffer: MemoryRenderBuffer::from_slice(
            &data,
            Fourcc::Argb8888,
            pixels,
            scale,
            Transform::Normal,
            None,
        ),
        size: Size::from((pixels.w / scale, pixels.h / scale)),
    })
}

/// Corners as the Mac draws them since Big Sur: rather than a quarter circle, whose
/// curvature jumps from nothing to all of it where it meets the edge, a quarter of a
/// superellipse that starts further along the edge and bends gradually, passing through
/// the middle of the circle of the same radius. It runs `CORNER_EXTENT` radii along each
/// edge; `CORNER_EXPONENT` is its exponent, which the glass's shader draws it with.
pub const CORNER_EXTENT: f64 = 1.5;
pub const CORNER_EXPONENT: f64 = 3.19;
/// Two cubic curves to each half of a corner follow that superellipse within 0.002 of
/// the radius: from the edge, through `CORNER_HANDLE` and twice `CORNER_MIDDLE` along
/// it, to `CORNER_MIDDLE` in from both edges.
const CORNER_HANDLE: f64 = 1.036;
const CORNER_MIDDLE: f64 = 1.0 - std::f64::consts::FRAC_1_SQRT_2;

/// A rectangle with corners of `radius`, smooth as the Mac's; one too short for them has
/// round ends instead, as a capsule.
pub fn rounded_rectangle(cr: &Context, x: f64, y: f64, width: f64, height: f64, radius: f64) {
    use std::f64::consts::{FRAC_PI_2, PI};
    cr.new_sub_path();
    if radius * CORNER_EXTENT > width.min(height) / 2.0 {
        let radius = radius.min(width.min(height) / 2.0);
        cr.arc(x + width - radius, y + radius, radius, -FRAC_PI_2, 0.0);
        cr.arc(x + width - radius, y + height - radius, radius, 0.0, FRAC_PI_2);
        cr.arc(x + radius, y + height - radius, radius, FRAC_PI_2, PI);
        cr.arc(x + radius, y + radius, radius, PI, 3.0 * FRAC_PI_2);
        cr.close_path();
        return;
    }
    let (extent, handle) = (radius * CORNER_EXTENT, radius * CORNER_HANDLE);
    let (middle, near) = (radius * CORNER_MIDDLE, radius * CORNER_MIDDLE * 2.0);
    // Each corner from the edge before it to the edge after it, turning clockwise; `ax`
    // and `ay` step along the edge before and `bx` and `by` along the one after.
    let corner = |cx: f64, cy: f64, (ax, ay): (f64, f64), (bx, by): (f64, f64)| {
        let at = |along: f64, across: f64| (cx - ax * along + bx * across, cy - ay * along + by * across);
        let (p0, p1, p2, p3) = (
            at(extent, 0.0),
            at(handle, 0.0),
            at(near, 0.0),
            at(middle, middle),
        );
        let (q1, q2, q3) = (at(0.0, near), at(0.0, handle), at(0.0, extent));
        cr.line_to(p0.0, p0.1);
        cr.curve_to(p1.0, p1.1, p2.0, p2.1, p3.0, p3.1);
        cr.curve_to(q1.0, q1.1, q2.0, q2.1, q3.0, q3.1);
    };
    corner(x + width, y, (1.0, 0.0), (0.0, 1.0));
    corner(x + width, y + height, (0.0, 1.0), (-1.0, 0.0));
    corner(x, y + height, (-1.0, 0.0), (0.0, -1.0));
    corner(x, y, (0.0, -1.0), (1.0, 0.0));
    cr.close_path();
}

fn layout(cr: &Context, text: &str, points: i32, bold: bool, max_width: i32) -> pango::Layout {
    let layout = new_layout(cr);
    let weight = if bold { " Semi-Bold" } else { "" };
    let font = font(&format!("{weight} {points}"));
    layout.set_font_description(Some(&font));
    layout.set_text(text);
    layout.set_width(max_width * pango::SCALE);
    layout.set_ellipsize(pango::EllipsizeMode::End);
    layout.set_alignment(pango::Alignment::Center);
    layout
}

/// A line of white text with a soft shadow, as wide as it needs up to `max_width`,
/// on a rounded pill if `pill` says how opaque.
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
    let surface = canvas(width, height).ok()?;
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
    /// The icon's name in the themes, for drawing it at any size, SVG included.
    pub icon_name: Option<String>,
    /// The command line that starts it, without the desktop entry's field codes.
    pub exec: Option<String>,
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
        let name_size = (name.width() / picture_scale(), name.height() / picture_scale());
        let width = name_size.0.max(icon_size);
        let height = icon_size + GAP + name_size.1;
        let surface = canvas(width, height).ok()?;
        {
            let cr = Context::new(&surface).ok()?;
            let icon_x = f64::from(width - icon_size) / 2.0;
            // By name first, which finds SVG icons too; a symbolic one is drawn white.
            let drawn = info
                .icon_name
                .as_deref()
                .and_then(|name| named_icon(name, "", icon_size, (1.0, 1.0, 1.0)))
                .and_then(|icon| {
                    let (w, h) = points(&icon);
                    let scale = f64::from(icon_size) / w.max(h).max(1.0);
                    cr.save().ok()?;
                    cr.translate(icon_x, 0.0);
                    cr.scale(scale, scale);
                    cr.set_source_surface(&icon, 0.0, 0.0).ok()?;
                    cr.paint().ok()?;
                    cr.restore().ok()
                })
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
                f64::from(width - name_size.0) / 2.0,
                f64::from(icon_size + GAP),
            )
            .ok()?;
            cr.paint().ok()?;
        }
        picture(surface)
    }
}

/// Calendar's icon name; the shell draws it with today's day of the month on it, where the
/// installed icon, for everyone else, can only show a month.
pub const CALENDAR_ICON: &str = "io.github.koukeneko.Nekoland.Calendar";
const CALENDAR_TODAY: &str = include_str!("calendar-today.svg");

/// The day of the month it is here.
pub fn today() -> i32 {
    glib::DateTime::now_local().map_or(1, |now| now.day_of_month())
}

/// An icon by name from the icon themes, or from `extra_dir` first, `size` pixels square.
/// Symbolic icons are drawn in `tint` instead of their own grey. It is `size` points
/// square, with as many pixels as a canvas has.
pub fn named_icon(name: &str, extra_dir: &str, size: i32, tint: (f64, f64, f64)) -> Option<ImageSurface> {
    let scale = picture_scale();
    if name == CALENDAR_ICON {
        let svg = CALENDAR_TODAY.replace("{day}", &today().to_string());
        let stream = gio::MemoryInputStream::from_bytes(&glib::Bytes::from_owned(svg.into_bytes()));
        let pixbuf = gdk_pixbuf::Pixbuf::from_stream_at_scale(
            &stream,
            size * scale,
            size * scale,
            true,
            gio::Cancellable::NONE,
        )
        .ok()?;
        let surface = pixbuf_surface(&pixbuf, None)?;
        surface.set_device_scale(f64::from(scale), f64::from(scale));
        return Some(surface);
    }
    let path = find_named_icon(name, extra_dir)?;
    let pixbuf = gdk_pixbuf::Pixbuf::from_file_at_size(&path, size * scale, size * scale).ok()?;
    let symbolic = name.ends_with("-symbolic") || path.to_string_lossy().contains("symbolic");
    let surface = pixbuf_surface(&pixbuf, symbolic.then_some(tint))?;
    surface.set_device_scale(f64::from(scale), f64::from(scale));
    Some(surface)
}

/// How big a surface is drawn, in points: its pixels over its device scale.
pub fn points(surface: &ImageSurface) -> (f64, f64) {
    let (x, y) = surface.device_scale();
    (f64::from(surface.width()) / x, f64::from(surface.height()) / y)
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

pub(crate) fn pixbuf_surface(
    pixbuf: &gdk_pixbuf::Pixbuf,
    tint: Option<(f64, f64, f64)>,
) -> Option<ImageSurface> {
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
    let key = (name.to_string(), extra_dir.to_string());
    if let Some(found) = FOUND_ICONS
        .lock()
        .unwrap()
        .get(&key)
        .filter(|found| found.is_file())
    {
        return Some(found.clone());
    }
    let mut roots: Vec<PathBuf> = Vec::new();
    if !extra_dir.is_empty() {
        roots.push(extra_dir.into());
    }
    let themes = icon_themes();
    for dir in data_dirs() {
        roots.extend(themes.iter().map(|theme| dir.join("icons").join(theme)));
        roots.push(dir.join("pixmaps"));
    }
    // shortcut: a name not found is looked for again each time, walking the theme folders;
    // remember those too if names without icons turn out to be asked for often.
    let found = roots.iter().find_map(|root| find_file(root, name, 4))?;
    FOUND_ICONS.lock().unwrap().insert(key, found.clone());
    Some(found)
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
            icon_name: Some(app_id.to_lowercase()),
            exec: None,
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
        icon_name: value("Icon"),
        exec: value("Exec").map(|exec| strip_field_codes(&exec)),
    }
}

/// A desktop entry's Exec without the %f, %U and the like a launcher would fill in.
fn strip_field_codes(exec: &str) -> String {
    let mut command = String::with_capacity(exec.len());
    let mut chars = exec.chars();
    while let Some(c) = chars.next() {
        match (c, chars.clone().next()) {
            ('%', Some('%')) => {
                command.push('%');
                chars.next();
            }
            ('%', Some(_)) => {
                chars.next();
            }
            _ => command.push(c),
        }
    }
    command.split_whitespace().collect::<Vec<_>>().join(" ")
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

#[cfg(test)]
mod tests {
    use super::*;
    use pango::prelude::*;

    /// Calendar's icon is drawn from its template with the day written in ink on its page.
    #[test]
    fn the_calendar_icon_shows_the_day() {
        let mut icon = named_icon(CALENDAR_ICON, "", 128, (0.0, 0.0, 0.0)).expect("the template draws");
        let stride = icon.stride() as usize;
        let data = icon.data().unwrap();
        // Where the number sits, on the ruled page under the coils: some of it is
        // near-black ink, which nothing else on the page is.
        let inked = (55..100)
            .flat_map(|y| (40..100).map(move |x| y * stride + x * 4))
            .filter(|&at| data[at] < 70 && data[at + 1] < 70 && data[at + 2] < 70 && data[at + 3] > 200)
            .count();
        assert!(inked > 100, "only {inked} ink pixels where the day goes");
    }

    /// The curves cairo draws a corner with keep to the superellipse the glass is cut to.
    #[test]
    fn corners_follow_the_superellipse_the_glass_is_cut_to() {
        let (extent, n) = (CORNER_EXTENT, CORNER_EXPONENT);
        let bezier = |t: f64| {
            let p = [
                (extent, 0.0),
                (CORNER_HANDLE, 0.0),
                (CORNER_MIDDLE * 2.0, 0.0),
                (CORNER_MIDDLE, CORNER_MIDDLE),
            ];
            let w = [
                (1.0 - t).powi(3),
                3.0 * (1.0 - t).powi(2) * t,
                3.0 * (1.0 - t) * t * t,
                t.powi(3),
            ];
            (0..4).fold((0.0, 0.0), |(x, y), i| (x + w[i] * p[i].0, y + w[i] * p[i].1))
        };
        let curve: Vec<(f64, f64)> = (0..=400).map(|i| bezier(f64::from(i) / 400.0)).collect();
        let last = 2f64.powf(-1.0 / n);
        for i in 0..=50 {
            let u = last * f64::from(i) / 50.0;
            let v = (1.0 - u.powf(n)).powf(1.0 / n);
            let (x, y) = (extent - extent * u, extent - extent * v);
            let gap = curve
                .iter()
                .map(|(cx, cy)| (cx - x).hypot(cy - y))
                .fold(f64::MAX, f64::min);
            assert!(gap < 0.003, "{gap} off at {u}");
        }
    }

    #[test]
    fn text_that_names_no_weight_is_set_at_medium() {
        let surface = ImageSurface::create(Format::ARgb32, 1, 1).unwrap();
        let cr = Context::new(&surface).unwrap();
        let weight_of = |font: &str| {
            let layout = new_layout(&cr);
            layout.set_font_description(Some(&super::font(font)));
            layout.set_text("Nekoland");
            let run = layout.line_readonly(0).unwrap().runs()[0].clone();
            run.item().analysis().font().describe().weight()
        };
        assert_eq!(weight_of("Ubuntu Sans 11"), TEXT_WEIGHT);
        assert_eq!(weight_of("Ubuntu Sans Bold 11"), pango::Weight::Bold);
    }

    #[test]
    fn text_is_in_nekolands_family_unless_it_names_one() {
        set_family("DejaVu Serif");
        assert_eq!(font("Bold 11").family().as_deref(), Some("DejaVu Serif, Noto Sans CJK TC, Sans"));
        assert_eq!(font("Ubuntu Sans Mono 9").family().as_deref(), Some("Ubuntu Sans Mono"));
        set_family("Ubuntu Sans");
    }

    #[test]
    fn field_codes_are_left_out_of_commands() {
        assert_eq!(
            strip_field_codes("nautilus --new-window %U"),
            "nautilus --new-window"
        );
        assert_eq!(strip_field_codes("printf 100%%"), "printf 100%");
    }
}
