//! Text and icons drawn by the compositor itself, for the overview and the menu bar.

use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicI32, Ordering},
        Arc, Mutex, RwLock,
    },
};

use cairo::{Context, Format, ImageSurface};
use crate::stacks::Pixels;
use gio::{glib, prelude::*};
use smithay::{
    backend::{allocator::Fourcc, renderer::element::memory::MemoryRenderBuffer},
    utils::{Logical, Size, Transform},
};

/// The family Nekoland's own text is set in, the configuration's `font`, before the
/// families that fill in what it lacks: CJK first.
static FAMILY: RwLock<String> = RwLock::new(String::new());
const FALLBACK: &str = "Noto Sans CJK TC, Sans";
const ICON_THEMES: [&str; 4] = ["Nekoland", "Yaru", "Adwaita", "hicolor"];
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

/// While it is kept, pictures are drawn `times` as dense as the output: for what is shown
/// enlarged, which would be blurred at the output's own density.
pub struct Denser(i32);

impl Denser {
    pub fn new(times: i32) -> Self {
        let was = picture_scale();
        PICTURE_SCALE.store(was * times.max(1), Ordering::Relaxed);
        Self(was)
    }
}

impl Drop for Denser {
    fn drop(&mut self) {
        PICTURE_SCALE.store(self.0, Ordering::Relaxed);
    }
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
pub fn set_icon_theme(theme: &str) {
    *ICON_THEME.write().unwrap() = theme.to_string();
    forget_found_icons();
}

/// Has every icon looked for again, in the folders as they are now.
fn forget_found_icons() {
    FOUND_ICONS.lock().unwrap().clear();
    INDEXES.lock().unwrap().clear();
}

/// The icons in each folder they are looked for in, by name: each folder walked once, where
/// looking for each icon walked tens of thousands of folders for the grid's hundred or so.
// shortcut: an index being made as the icons are forgotten is kept, as it was; make one
// with a count of forgettings if that matters.
static INDEXES: Mutex<BTreeMap<PathBuf, Arc<HashMap<String, PathBuf>>>> = Mutex::new(BTreeMap::new());

/// Where icons were found, by name and the folder looked in first, until the theme changes:
/// a theme holds thousands of files, too many to walk for each icon drawn. None while one is
/// being looked for on its thread, then where it is, if anywhere.
static FOUND_ICONS: Mutex<BTreeMap<IconName, Option<Option<PathBuf>>>> = Mutex::new(BTreeMap::new());

/// An icon's name, and the folder it is looked for in first.
type IconName = (String, String);

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
    const PAD_X: i32 = 12;
    const PAD_Y: i32 = 8;

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
    /// The overview's badges, by application, title, icon size and scale, which it would
    /// otherwise draw anew for every window each time it lays them out.
    badges: HashMap<(String, String, i32, i32), Option<Picture>>,
}

impl Apps {
    pub fn info(&mut self, app_id: &str) -> &AppInfo {
        self.known
            .entry(app_id.to_string())
            .or_insert_with(|| lookup(&entries(), app_id))
    }

    /// The icon over the name, as it sits under a window in the overview.
    pub fn badge(&mut self, app_id: &str, fallback_name: &str, icon_size: i32) -> Option<Picture> {
        let key = (app_id.to_string(), fallback_name.to_string(), icon_size, picture_scale());
        if let Some(badge) = self.badges.get(&key) {
            return badge.clone();
        }
        // shortcut: forgets every badge once there are many, as titles that change pile up,
        // rather than the oldest.
        if self.badges.len() > 200 {
            self.badges.clear();
        }
        let waits = icon_waits();
        let badge = self.draw_badge(app_id, fallback_name, icon_size);
        if icon_waits() == waits {
            self.badges.insert(key, badge.clone());
        }
        badge
    }

    fn draw_badge(&mut self, app_id: &str, fallback_name: &str, icon_size: i32) -> Option<Picture> {
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
        let day = today();
        let svg = CALENDAR_TODAY.replace("{day}", &day.to_string());
        let key = (PathBuf::from(format!("{CALENDAR_ICON}@{day}")), None, size * scale);
        let surface = crate::stacks::pixels_surface(decoded_icon(key, Some(svg))?)?;
        surface.set_device_scale(f64::from(scale), f64::from(scale));
        return Some(surface);
    }
    let path = find_named_icon(name, extra_dir)?;
    let symbolic = name.ends_with("-symbolic") || path.to_string_lossy().contains("symbolic");
    let pixels = size * scale;
    // The file as it is now: a theme's change or a redrawn icon is another.
    let changed = std::fs::metadata(&path).and_then(|file| file.modified()).ok();
    let tinted = symbolic.then(|| [tint.0.to_bits(), tint.1.to_bits(), tint.2.to_bits()]);
    let kept_key = (path.clone(), changed, pixels, tinted);
    if pixels <= KEPT_ICON_PIXELS {
        if let Some(kept) = KEPT_ICONS.with_borrow(|kept| kept.get(&kept_key).and_then(copy_of)) {
            return Some(kept);
        }
    }
    let decoded = decoded_icon((path, changed, pixels), None)?;
    let surface = crate::stacks::tinted_pixels_surface(decoded, symbolic.then_some(tint))?;
    surface.set_device_scale(f64::from(scale), f64::from(scale));
    // A small one is kept as drawn: menus, the control center and notices ask for theirs
    // every time they are drawn.
    if pixels <= KEPT_ICON_PIXELS {
        KEPT_ICONS.with_borrow_mut(|kept| {
            // shortcut: forgets them all once there are many, rather than the oldest.
            if kept.len() >= 512 {
                kept.clear();
            }
            kept.extend(copy_of(&surface).map(|copy| (kept_key, copy)));
        });
    }
    Some(surface)
}

/// The largest icon kept once drawn, in pixels across: larger ones are kept by what asks
/// for them, the Dock and the applications' grid, as they draw them.
const KEPT_ICON_PIXELS: i32 = 128;

/// The most the decoded icons' pixels are let take before they are all forgotten.
const DECODED_ICON_BYTES: usize = 64 << 20;

type KeptIcon = (PathBuf, Option<std::time::SystemTime>, i32, Option<[u64; 3]>);
type DecodeKey = (PathBuf, Option<std::time::SystemTime>, i32);

thread_local! {
    static KEPT_ICONS: std::cell::RefCell<HashMap<KeptIcon, ImageSurface>> = std::cell::RefCell::default();
    static ICON_WAITS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Icons decoded on their thread, by file and pixels across: `None` while one is on its
/// way, then its pixels, if it could be decoded.
static DECODED: Mutex<BTreeMap<DecodeKey, Option<Option<Pixels>>>> = Mutex::new(BTreeMap::new());

/// What is pinged to draw the screen again once an icon is decoded.
static REDRAW: std::sync::OnceLock<smithay::reexports::calloop::ping::Ping> = std::sync::OnceLock::new();

pub fn redraw_on_decoded_icons(redraw: smithay::reexports::calloop::ping::Ping) {
    let _ = REDRAW.set(redraw);
}

/// Has the screen drawn again, from a thread that decoded something it shows.
pub fn redraw_soon() {
    if let Some(redraw) = REDRAW.get() {
        redraw.ping();
    }
}

/// How many times an icon or picture asked for on this thread was still being decoded: what
/// was drawn while this changed is not to be kept, as the decoding has the screen drawn again.
pub fn icon_waits() -> u64 {
    ICON_WAITS.get()
}

/// Counts a picture asked for that is still being read on its thread, as `icon_waits` tells.
pub fn picture_waited() {
    ICON_WAITS.set(ICON_WAITS.get() + 1);
}

/// An icon's pixels, decoded on a thread of their own, as a file's decoding is done by a
/// program started for it, which takes tens of milliseconds and may never end: on the
/// compositor's thread, it would stop the screen. None until it is done. The file's, or
/// that of the `svg` given in its place.
fn decoded_icon(key: DecodeKey, svg: Option<String>) -> Option<Pixels> {
    let mut decoded = DECODED.lock().unwrap();
    match decoded.get(&key) {
        Some(Some(pixels)) => return pixels.clone(),
        Some(None) => {
            ICON_WAITS.set(ICON_WAITS.get() + 1);
            return None;
        }
        None => {}
    }
    let jobs = icon_jobs()?;
    let bytes: usize = decoded.values().flatten().flatten().map(|(bytes, ..)| bytes.len()).sum();
    // shortcut: forgets them all once they take much memory, rather than the oldest.
    if bytes > DECODED_ICON_BYTES {
        decoded.clear();
    }
    decoded.insert(key.clone(), None);
    drop(decoded);
    if jobs.send((key, svg)).is_ok() {
        ICON_WAITS.set(ICON_WAITS.get() + 1);
    }
    None
}

type IconJob = (DecodeKey, Option<String>);

/// How many icons are decoded at once.
const ICON_DECODERS: usize = 6;

fn icon_jobs() -> Option<&'static std::sync::mpsc::Sender<IconJob>> {
    static JOBS: std::sync::OnceLock<Option<std::sync::mpsc::Sender<IconJob>>> = std::sync::OnceLock::new();
    JOBS.get_or_init(|| {
        let (send, jobs) = std::sync::mpsc::channel::<IconJob>();
        let jobs = std::sync::Arc::new(Mutex::new(jobs));
        let spawn = |jobs: std::sync::Arc<Mutex<std::sync::mpsc::Receiver<IconJob>>>| {
            std::thread::Builder::new()
                .name("nekoland-icons".into())
                .spawn(move || loop {
                    // Taken in a statement of its own, which lets go of the lock before decoding.
                    let job = jobs.lock().unwrap().recv();
                    let Ok((key, svg)) = job else { break };
                    let (path, _, pixels) = &key;
                    let read = match svg {
                        Some(svg) => gdk_pixbuf::Pixbuf::from_stream_at_scale(
                            &gio::MemoryInputStream::from_bytes(&glib::Bytes::from_owned(svg.into_bytes())),
                            *pixels,
                            *pixels,
                            true,
                            gio::Cancellable::NONE,
                        ),
                        None => gdk_pixbuf::Pixbuf::from_file_at_size(path, *pixels, *pixels),
                    };
                    let read = read
                        .inspect_err(|err| tracing::warn!("{}: {err}", path.display()))
                        .ok()
                        .map(|pixbuf| crate::stacks::pixbuf_pixels(&pixbuf));
                    DECODED.lock().unwrap().insert(key, Some(read));
                    redraw_soon();
                })
        };
        // Each file is decoded by a loader process started for it, mostly waiting: several at
        // once fill the Dock and the grid after login in a fraction of the time.
        let spawned = (0..ICON_DECODERS)
            .filter(|_| {
                spawn(jobs.clone())
                    .inspect_err(|err| tracing::warn!("icons are not decoded: {err}"))
                    .is_ok()
            })
            .count();
        (spawned > 0).then_some(send)
    })
    .as_ref()
}

/// The same picture on a surface of its own, which whoever has it may draw on.
pub(crate) fn copy_of(surface: &ImageSurface) -> Option<ImageSurface> {
    let copy = ImageSurface::create(Format::ARgb32, surface.width(), surface.height()).ok()?;
    let (x, y) = surface.device_scale();
    copy.set_device_scale(x, y);
    {
        let cr = Context::new(&copy).ok()?;
        cr.set_operator(cairo::Operator::Source);
        cr.set_source_surface(surface, 0.0, 0.0).ok()?;
        cr.paint().ok()?;
    }
    Some(copy)
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

/// The names GIO gives an icon, most specific first: a theme's names, or the path of a
/// file, as a desktop entry's `Icon=` may give one; `find_named_icon` takes either.
pub fn gio_icon_names(icon: gio::Icon) -> Vec<String> {
    if let Some(file) = icon.downcast_ref::<gio::FileIcon>() {
        return file.file().path().map(|path| path.display().to_string()).into_iter().collect();
    }
    icon.downcast::<gio::ThemedIcon>()
        .map(|icon| icon.names().iter().map(|name| name.to_string()).collect())
        .unwrap_or_default()
}

/// Any icon of that name, SVG or PNG, in any size or context of the themes. None until it
/// is found, as the themes' folders are walked on a thread of their own: tens of thousands
/// of files, which would hold up the screen for a tenth of a second or more.
fn find_named_icon(name: &str, extra_dir: &str) -> Option<PathBuf> {
    let key = (name.to_string(), extra_dir.to_string());
    let mut found = FOUND_ICONS.lock().unwrap();
    match found.get(&key) {
        // Looked for again should it be gone, as when a theme is uninstalled.
        Some(Some(Some(path))) if path.is_file() => return Some(path.clone()),
        Some(Some(None)) => return None,
        Some(None) => {
            ICON_WAITS.set(ICON_WAITS.get() + 1);
            return None;
        }
        _ => {}
    }
    let lookups = icon_lookups()?;
    found.insert(key.clone(), None);
    drop(found);
    let theme = ICON_THEME.read().unwrap().clone();
    if lookups.send((key, theme)).is_ok() {
        ICON_WAITS.set(ICON_WAITS.get() + 1);
    }
    None
}

fn icon_lookups() -> Option<&'static std::sync::mpsc::Sender<(IconName, String)>> {
    static LOOKUPS: std::sync::OnceLock<Option<std::sync::mpsc::Sender<(IconName, String)>>> =
        std::sync::OnceLock::new();
    LOOKUPS
        .get_or_init(|| {
            let (send, lookups) = std::sync::mpsc::channel::<(IconName, String)>();
            let spawned = std::thread::Builder::new().name("nekoland-icon-find".into()).spawn(move || {
                for ((name, extra_dir), theme) in lookups {
                    let found = look_for_icon(&name, &extra_dir);
                    // One looked for in a theme since replaced is looked for again.
                    if *ICON_THEME.read().unwrap() == theme {
                        FOUND_ICONS.lock().unwrap().insert((name, extra_dir), Some(found));
                    }
                    redraw_soon();
                }
            });
            spawned
                .inspect_err(|err| tracing::warn!("icons are not looked for: {err}"))
                .ok()
                .map(|_| send)
        })
        .as_ref()
}

/// Where an icon of that name is: the walk `find_named_icon` has done on its thread.
fn look_for_icon(name: &str, extra_dir: &str) -> Option<PathBuf> {
    let path = Path::new(name);
    if path.is_absolute() {
        // A path bypasses the icon theme, but the restyle of its file (snap apps give one)
        // is kept in ours under the file's name.
        let restyled = path.file_stem().and_then(|stem| stem.to_str()).and_then(|stem| {
            data_dirs().iter().find_map(|dir| indexed(&dir.join("icons/Nekoland")).get(stem).cloned())
        });
        return restyled.or_else(|| path.is_file().then(|| path.to_path_buf()));
    }
    // An application's own folder, as a status icon's, may have icons written to it at any
    // time, and is small: it is walked each time.
    if let Some(found) = Some(extra_dir).filter(|dir| !dir.is_empty()).and_then(|dir| find_file(Path::new(dir), name, 4)) {
        return Some(found);
    }
    let mut roots: Vec<PathBuf> = Vec::new();
    let themes = icon_themes();
    for dir in data_dirs() {
        roots.extend(themes.iter().map(|theme| dir.join("icons").join(theme)));
        roots.push(dir.join("pixmaps"));
    }
    roots.iter().find_map(|root| indexed(root).get(name).cloned())
}

/// The icons in `root` by name, the one `find_file` would find for each.
fn indexed(root: &Path) -> Arc<HashMap<String, PathBuf>> {
    if let Some(index) = INDEXES.lock().unwrap().get(root) {
        return index.clone();
    }
    let mut index = HashMap::new();
    index_folder(root, 4, &mut index);
    let index = Arc::new(index);
    INDEXES.lock().unwrap().insert(root.to_path_buf(), index.clone());
    index
}

/// Adds the icons in `dir` and the folders in it to `index`, those not in it yet, in the
/// order `find_file` looks in.
fn index_folder(dir: &Path, depth: usize, index: &mut HashMap<String, PathBuf>) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    let entries: Vec<PathBuf> = read.flatten().map(|entry| entry.path()).collect();
    for ext in ["svg", "png"] {
        for path in entries.iter().filter(|path| path.extension().and_then(|e| e.to_str()) == Some(ext)) {
            if let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) {
                index.entry(stem.to_string()).or_insert_with(|| path.clone());
            }
        }
    }
    if depth == 0 {
        return;
    }
    for sub in subfolders_in_order(&entries) {
        index_folder(sub, depth - 1, index);
    }
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
    subfolders_in_order(&entries)
        .into_iter()
        .find_map(|sub| find_file(sub, stem, depth - 1))
}

/// The folders among `entries` in the order icons are looked for in them: scalable ones
/// first, then the largest bitmaps, as a theme with no scalable icon of a name has it at
/// many sizes, and the first folder read may hold the 16-pixel one.
fn subfolders_in_order(entries: &[PathBuf]) -> Vec<&PathBuf> {
    let mut subdirs: Vec<&PathBuf> = entries.iter().filter(|path| path.is_dir()).collect();
    subdirs.sort_by_key(|path| {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        (
            !name.contains("scalable") && !name.contains("symbolic"),
            std::cmp::Reverse(bitmap_size(&name)),
        )
    });
    subdirs
}

/// The pixels across of the icons in a theme's folder named like `48x48` or `256x256@2x`;
/// 0 for any other name.
fn bitmap_size(folder: &str) -> u32 {
    let (size, scale) = folder.split_once('@').unwrap_or((folder, "1x"));
    let size: u32 = size.split_once('x').and_then(|(width, _)| width.parse().ok()).unwrap_or(0);
    size * scale.trim_end_matches('x').parse().unwrap_or(1)
}

fn data_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let mut dirs: Vec<PathBuf> = home.iter().map(|home| home.join(".local/share")).collect();
    let system = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    dirs.extend(system.split(':').filter(|dir| !dir.is_empty()).map(PathBuf::from));
    dirs.push("/var/lib/snapd/desktop".into());
    dirs
}

/// Has the icons of applications installed while Nekoland runs drawn on its tile as the
/// others were: when a folder of desktop entries changes, the tool makes them again and
/// the compositor looks its icons up anew. The tool leaves a person who had its icons
/// removed without them.
pub fn watch_applications() {
    let changed = || -> Vec<Option<std::time::SystemTime>> {
        let (_, folders) = applications(&data_dirs());
        folders
            .iter()
            .map(|folder| std::fs::metadata(folder).and_then(|meta| meta.modified()).ok())
            .collect()
    };
    std::thread::spawn(move || {
        let mut seen = changed();
        loop {
            std::thread::sleep(std::time::Duration::from_secs(2));
            if changed() == seen {
                continue;
            }
            // A package puts its entry down before its icons: wait for the rest of it.
            std::thread::sleep(std::time::Duration::from_secs(5));
            // GIO, which lists the applications, watches these folders but not the ones
            // within them, where Wine keeps its programs' entries: a file come and gone in
            // the person's own has it look again.
            if let Some(folder) = data_dirs().first().map(|dir| dir.join("applications")) {
                let come = folder.join(".nekoland-changed");
                if std::fs::write(&come, "").is_ok() {
                    let _ = std::fs::remove_file(&come);
                }
            }
            seen = changed();
            *ENTRIES.write().unwrap() = Some(Arc::new(read_entries()));
            crate::appgrid::forget_listed();
            // Their icons, new to the themes' folders.
            forget_found_icons();
            match std::process::Command::new("nekoland-conform-icons").arg("--refresh").status() {
                Ok(status) if status.success() => {
                    if let Err(err) = crate::ipc::send("reload") {
                        tracing::warn!("reload after new icons: {err}");
                    }
                }
                Ok(status) => tracing::warn!("nekoland-conform-icons: {status}"),
                Err(err) => tracing::warn!("nekoland-conform-icons: {err}"),
            }
        }
    });
}

/// The applications' desktop entries, by the lowercased names of their files and the window
/// classes they claim, the first found of each kept.
#[derive(Default)]
struct Entries {
    by_name: HashMap<String, Arc<String>>,
    by_class: HashMap<String, Arc<String>>,
}

/// The desktop entries as last read, replaced when the applications change.
static ENTRIES: RwLock<Option<Arc<Entries>>> = RwLock::new(None);

/// The first reading of the desktop entries, begun as the compositor starts.
static FIRST_ENTRIES: Mutex<Option<std::thread::JoinHandle<Entries>>> = Mutex::new(None);

/// Reads the applications' desktop entries on a thread, for the first lookups to find read:
/// a hundred files or more, which at login, read cold, would hold up the screen.
pub fn read_applications() {
    match std::thread::Builder::new().name("nekoland-apps".into()).spawn(read_entries) {
        Ok(reading) => *FIRST_ENTRIES.lock().unwrap() = Some(reading),
        Err(err) => tracing::warn!("desktop entries are read when first asked for: {err}"),
    }
}

/// The desktop entries, waiting for their first reading if it is not done.
fn entries() -> Arc<Entries> {
    if let Some(entries) = ENTRIES.read().unwrap().clone() {
        return entries;
    }
    let reading = FIRST_ENTRIES.lock().unwrap().take();
    let read = reading.and_then(|reading| reading.join().ok()).unwrap_or_else(read_entries);
    let read = Arc::new(read);
    ENTRIES.write().unwrap().get_or_insert(read).clone()
}

fn read_entries() -> Entries {
    let mut entries = Entries::default();
    for path in applications(&data_dirs()).0 {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let text = Arc::new(text);
        let name = path.file_stem().and_then(|stem| stem.to_str()).unwrap_or_default().to_lowercase();
        entries.by_name.entry(name).or_insert_with(|| text.clone());
        let classes = text.lines().filter_map(|line| line.strip_prefix("StartupWMClass="));
        for class in classes {
            entries.by_class.entry(class.trim().to_lowercase()).or_insert_with(|| text.clone());
        }
    }
    entries
}

/// The desktop entry named after the application, or failing that the one that claims its
/// window class.
fn lookup(entries: &Entries, app_id: &str) -> AppInfo {
    let wanted = app_id.to_lowercase();
    let Some(entry) = entries.by_name.get(&wanted).or_else(|| entries.by_class.get(&wanted)) else {
        return AppInfo {
            // A Windows program of Wine's is named after its file, as "notepad.exe".
            name: app_id.to_lowercase().strip_suffix(".exe").map(str::to_string),
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

/// The desktop entries of the data folders and the folders they are in, those within
/// `applications` too, where Wine keeps the entries of the programs it installs.
fn applications(dirs: &[PathBuf]) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let (mut entries, mut folders) = (Vec::new(), Vec::new());
    let mut left: Vec<PathBuf> = dirs.iter().rev().map(|dir| dir.join("applications")).collect();
    while let Some(folder) = left.pop() {
        let Ok(inside) = std::fs::read_dir(&folder) else {
            continue;
        };
        folders.push(folder);
        for path in inside.flatten().map(|entry| entry.path()) {
            if path.extension().and_then(|ext| ext.to_str()) == Some("desktop") {
                entries.push(path);
            } else if path.is_dir() {
                left.push(path);
            }
        }
    }
    (entries, folders)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pango::prelude::*;

    #[test]
    fn an_index_finds_the_file_a_walk_would() {
        let root = std::env::temp_dir().join(format!("nekoland-icon-index-{}", std::process::id()));
        let files = [
            "top.png",
            "16x16/apps/top.svg",
            "16x16/apps/small.png",
            "256x256/apps/small.png",
            "scalable/apps/small.svg",
            "48x48/apps/both.png",
            "48x48/apps/both.svg",
            "symbolic/apps/deep.svg",
            "a/b/c/d/e/too-deep.svg",
        ];
        for file in files {
            let path = root.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, "").unwrap();
        }
        let mut index = HashMap::new();
        index_folder(&root, 4, &mut index);
        for name in ["top", "small", "both", "deep", "too-deep", "missing"] {
            assert_eq!(index.get(name).cloned(), find_file(&root, name, 4), "{name}");
        }
        assert_eq!(index["small"], root.join("scalable/apps/small.svg"));
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// A desktop entry's `Icon=` may be a file's path, which GIO gives as a file icon.
    #[test]
    fn a_file_icon_goes_by_its_path() {
        let icon = gio::FileIcon::new(&gio::File::for_path("/snap/blip/current/meta/gui/icon.png"));
        assert_eq!(gio_icon_names(icon.upcast()), ["/snap/blip/current/meta/gui/icon.png"]);
    }

    #[test]
    fn a_themed_icon_goes_by_its_names() {
        let icon = gio::ThemedIcon::from_names(&["org.gnome.Calculator", "accessories-calculator"]);
        // GIO adds their symbolic forms after them.
        assert!(gio_icon_names(icon.upcast()).starts_with(&["org.gnome.Calculator".into(), "accessories-calculator".into()]));
    }

    /// Calendar's icon is drawn from its template with the day written in ink on its page.
    #[test]
    fn bitmap_folders_are_sized_by_their_names() {
        assert_eq!(bitmap_size("256x256@2x"), 512);
        assert_eq!(bitmap_size("48x48"), 48);
        assert_eq!(bitmap_size("scalable"), 0);
    }

    #[test]
    fn icons_are_decoded_without_waiting_for_them() {
        let dir = std::env::temp_dir().join(format!("nekoland-decoded-icon-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("nekoland-decoded-icon-test.png");
        let drawn = ImageSurface::create(Format::ARgb32, 8, 8).unwrap();
        drawn.write_to_png(&mut std::fs::File::create(&path).unwrap()).unwrap();
        let name = path.to_str().unwrap();
        let waits = icon_waits();
        assert!(named_icon(name, "", 8, (0.0, 0.0, 0.0)).is_none());
        assert_eq!(icon_waits(), waits + 1);
        decoded(name, 8);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The icon once its thread has decoded it.
    fn decoded(name: &str, size: i32) -> ImageSurface {
        let asked = std::time::Instant::now();
        loop {
            if let Some(icon) = named_icon(name, "", size, (0.0, 0.0, 0.0)) {
                return icon;
            }
            assert!(asked.elapsed().as_secs() < 10, "{name} was never decoded");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[test]
    fn the_calendar_icon_shows_the_day() {
        let mut icon = decoded(CALENDAR_ICON, 128);
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
