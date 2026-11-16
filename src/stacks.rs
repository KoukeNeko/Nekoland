//! The folders in the Dock, as the Mac's stacks: what is in a folder, in the order asked
//! for, the pictures of its items, and the fan and grid they open as over the Dock.

use std::{
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::UNIX_EPOCH,
};

use cairo::{Context, Format, ImageSurface};
use gio::prelude::*;
use smithay::utils::{Logical, Point, Rectangle, Size};

use crate::{
    config::Sort,
    dock::{ink, paint_pane},
    glass::Tier,
    text::{self, Picture},
};

/// Pictures with more pixels than this are not read: some formats are decoded whole before
/// they are scaled down, at four bytes a pixel.
const MAX_PICTURE_PIXELS: i64 = 200_000_000;

pub const FAN_ICON: f64 = 48.0;
/// How far apart the items of a fan are, how much it curves away, and how much each item
/// turns more than the one under it.
const FAN_STEP: f64 = 56.0;
const FAN_CURVE: f64 = 1.3;
const FAN_TURN: f64 = 0.024;
const PILL_HEIGHT: f64 = 26.0;
const PILL_GAP: f64 = 10.0;
/// How many items a fan shows at most, before the one that opens the folder.
pub const FAN_MAX: usize = 15;
/// Up to how many items Auto opens as a fan rather than a grid.
pub const AUTO_FAN: usize = 12;

pub const GRID_COLUMNS: usize = 5;
pub const GRID_ROWS: usize = 4;
pub const CELL_WIDTH: i32 = 92;
pub const CELL_HEIGHT: i32 = 96;
pub const GRID_ICON: i32 = 56;
const GRID_PADDING: i32 = 12;
const GRID_HEADER: i32 = 36;
pub const GRID_RADIUS: i32 = 14;

/// A file or folder in a stack.
#[derive(Clone, PartialEq, Debug)]
pub struct Item {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub kind: String,
    added: i64,
    modified: i64,
    created: i64,
}

/// What is in a folder, hidden files left out, in the order `sort` asks for: names A to Z,
/// kinds together, or newest first by the date chosen.
pub fn read_folder(folder: &Path, sort: Sort) -> Vec<Item> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut items: Vec<Item> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                return None;
            }
            let path = entry.path();
            // Through a link to what it points at.
            let meta = std::fs::metadata(&path).ok()?;
            let is_dir = meta.is_dir();
            let kind = if is_dir {
                "inode/directory".to_string()
            } else {
                // By its name, as file managers list folders: an empty sample of its content
                // would make GIO take it for an empty file.
                gio::File::for_path(&path)
                    .query_info(
                        "standard::fast-content-type",
                        gio::FileQueryInfoFlags::NONE,
                        gio::Cancellable::NONE,
                    )
                    .ok()
                    .and_then(|info| info.attribute_string("standard::fast-content-type"))
                    .map_or_else(|| "application/octet-stream".to_string(), |kind| kind.to_string())
            };
            let modified = meta.mtime();
            let created = meta
                .created()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map_or(modified, |since| since.as_secs() as i64);
            Some(Item {
                path,
                name,
                is_dir,
                kind,
                // Linux keeps no date a file was put in a folder; the last change to its
                // entry, as moving it there, comes closest.
                added: meta.ctime(),
                modified,
                created,
            })
        })
        .collect();
    sort_items(&mut items, sort);
    items
}

fn sort_items(items: &mut [Item], sort: Sort) {
    let by_name = |a: &Item, b: &Item| a.name.to_lowercase().cmp(&b.name.to_lowercase());
    match sort {
        Sort::Name => items.sort_by(by_name),
        Sort::Added => items.sort_by(|a, b| b.added.cmp(&a.added).then_with(|| by_name(a, b))),
        Sort::Modified => items.sort_by(|a, b| b.modified.cmp(&a.modified).then_with(|| by_name(a, b))),
        Sort::Created => items.sort_by(|a, b| b.created.cmp(&a.created).then_with(|| by_name(a, b))),
        Sort::Kind => items.sort_by(|a, b| a.kind.cmp(&b.kind).then_with(|| by_name(a, b))),
    }
}

/// A folder's name as shown: its last part.
pub fn folder_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// The names a file's icon goes by in the themes, most specific first, as GIO knows them:
/// a downloads folder has its own, a PDF its own.
pub fn icon_names(path: &Path) -> Vec<String> {
    gio::File::for_path(path)
        .query_info(
            "standard::icon",
            gio::FileQueryInfoFlags::NONE,
            gio::Cancellable::NONE,
        )
        .ok()
        .and_then(|info| info.icon())
        .and_then(|icon| icon.downcast::<gio::ThemedIcon>().ok())
        .map(|icon| icon.names().iter().map(|name| name.to_string()).collect())
        .unwrap_or_default()
}

/// An item's thumbnail, made by a file manager, or else its kind's icon. A picture itself is
/// only read on a thread, with `read_on_a_thread`.
pub fn item_icon(item: &Item, size: i32, tint: (f64, f64, f64)) -> Option<ImageSurface> {
    let scaled = size * text::picture_scale();
    picture_path(item)
        .filter(|path| *path != item.path)
        .and_then(|path| gdk_pixbuf::Pixbuf::from_file_at_scale(path, scaled, scaled, true).ok())
        .and_then(|pixbuf| text::pixbuf_surface(&pixbuf, None))
        .or_else(|| kind_icon(item, size, tint))
}

/// The picture an item's icon is made from: the thumbnail a file manager made of it, or the
/// picture itself when it is one.
pub fn picture_path(item: &Item) -> Option<PathBuf> {
    if item.is_dir {
        return None;
    }
    let thumbnail = gio::File::for_path(&item.path)
        .query_info(
            "thumbnail::path",
            gio::FileQueryInfoFlags::NONE,
            gio::Cancellable::NONE,
        )
        .ok()
        .and_then(|info| info.attribute_byte_string("thumbnail::path"))
        .map(|path| PathBuf::from(path.as_str()));
    thumbnail.or_else(|| item.kind.starts_with("image/").then(|| item.path.clone()))
}

/// The icon of an item's kind in the theme.
pub fn kind_icon(item: &Item, size: i32, tint: (f64, f64, f64)) -> Option<ImageSurface> {
    icon_names(&item.path)
        .iter()
        .find_map(|name| text::named_icon(name, "", size, tint))
}

/// A picture's pixels as gdk-pixbuf gives them, which, unlike its pixbuf, may cross threads:
/// the bytes, whether they have alpha, width, height and row stride.
pub type Pixels = (gio::glib::Bytes, bool, i32, i32, i32);

/// Pixels read on a thread: `None` until it is done, then what it read, if anything.
pub type Reading = Arc<Mutex<Option<Option<Pixels>>>>;

/// Reads the picture at `path`, at most `pixels` big, on a thread.
pub fn read_on_a_thread(path: PathBuf, pixels: Size<i32, Logical>) -> Reading {
    let read = Arc::new(Mutex::new(None));
    let into = read.clone();
    std::thread::spawn(move || {
        // Learning the size may read the whole file, hence here and not on the caller's thread.
        let fits = gdk_pixbuf::Pixbuf::file_info(&path)
            .is_some_and(|(_, w, h)| i64::from(w) * i64::from(h) <= MAX_PICTURE_PIXELS);
        let found = fits
            .then(|| gdk_pixbuf::Pixbuf::from_file_at_scale(&path, pixels.w, pixels.h, true))
            .and_then(|read| {
                read.inspect_err(|err| tracing::warn!("{}: {err}", path.display()))
                    .ok()
            })
            .map(|pixbuf| {
                let stride = pixbuf.rowstride();
                (
                    pixbuf.read_pixel_bytes(),
                    pixbuf.has_alpha(),
                    pixbuf.width(),
                    pixbuf.height(),
                    stride,
                )
            });
        if let Ok(mut read) = into.lock() {
            *read = Some(found);
        }
    });
    read
}

/// A picture's surface from pixels read on a thread.
pub fn pixels_surface((bytes, alpha, width, height, stride): Pixels) -> Option<ImageSurface> {
    let pixbuf = gdk_pixbuf::Pixbuf::from_bytes(
        &bytes,
        gdk_pixbuf::Colorspace::Rgb,
        alpha,
        8,
        width,
        height,
        stride,
    );
    text::pixbuf_surface(&pixbuf, None)
}

/// Draws `icon` fitted to a square of `size` at the origin, in the middle of it.
pub fn paint_fitted(cr: &Context, icon: &ImageSurface, size: f64, alpha: f64) {
    let (w, h) = text::points(icon);
    let scale = size / w.max(h).max(1.0);
    let _ = cr.save();
    cr.translate((size - w * scale) / 2.0, (size - h * scale) / 2.0);
    cr.scale(scale, scale);
    let _ = cr.set_source_surface(icon, 0.0, 0.0);
    cr.source().set_filter(cairo::Filter::Good);
    let _ = cr.paint_with_alpha(alpha);
    let _ = cr.restore();
}

fn layout(cr: &Context, text: &str, points: f64) -> pango::Layout {
    let layout = text::new_layout(cr);
    layout.set_font_description(Some(&text::font(&points.to_string())));
    layout.set_text(text);
    layout
}

fn text_width(text: &str, points: f64) -> f64 {
    let Ok(surface) = ImageSurface::create(Format::ARgb32, 1, 1) else {
        return 0.0;
    };
    let Ok(cr) = Context::new(&surface) else {
        return 0.0;
    };
    f64::from(layout(&cr, text, points).pixel_size().0)
}

/// `1301` as `1,301`.
pub fn count_text(count: usize) -> String {
    let digits = count.to_string();
    let mut out = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// Where each item of a fan goes from the bottom middle of where it starts: rising, curving
/// right a little more each time, as the Mac's does, and turning with it. The last place is
/// for the line that opens the folder.
pub fn fan_places(count: usize) -> Vec<(f64, f64, f64)> {
    (0..count)
        .map(|index| {
            let i = index as f64;
            (
                FAN_CURVE * i * i,
                -FAN_ICON / 2.0 - i * FAN_STEP,
                FAN_TURN * i,
            )
        })
        .collect()
}

/// A fan laid out: where it is drawn from, and where each item and its name are.
pub struct Fan {
    pub places: Vec<(f64, f64, f64)>,
    /// How wide each name is, with the pill around it.
    pub widths: Vec<f64>,
    /// Where the picture's top left is, from the anchor.
    pub origin: Point<f64, Logical>,
    pub size: Size<i32, Logical>,
}

impl Fan {
    /// `labels` bottom first; the last one is the line that opens the folder.
    pub fn new(labels: &[String]) -> Self {
        let places = fan_places(labels.len());
        let widths: Vec<f64> = labels
            .iter()
            .map(|label| text_width(label, 10.0) + 28.0)
            .collect();
        let (mut min, mut max) = ((f64::MAX, f64::MAX), (f64::MIN, f64::MIN));
        for ((x, y, angle), width) in places.iter().zip(&widths) {
            let half = FAN_ICON / 2.0;
            for (cx, cy) in [
                (-(half + PILL_GAP + width), -half),
                (half, -half),
                (-(half + PILL_GAP + width), half),
                (half, half),
            ] {
                let (sin, cos) = angle.sin_cos();
                let (px, py) = (x + cx * cos - cy * sin, y + cx * sin + cy * cos);
                min = (min.0.min(px), min.1.min(py));
                max = (max.0.max(px), max.1.max(py));
            }
        }
        let pad = 12.0;
        let origin = Point::from((min.0 - pad, min.1 - pad));
        let size = Size::from((
            (max.0 - min.0 + pad * 2.0).ceil() as i32,
            (max.1 - min.1 + pad * 2.0).ceil() as i32,
        ));
        Self {
            places,
            widths,
            origin,
            size,
        }
    }

    /// Which item, or the last line, a point from the anchor is on.
    pub fn hit(&self, point: Point<f64, Logical>) -> Option<usize> {
        let half = FAN_ICON / 2.0;
        self.places
            .iter()
            .zip(&self.widths)
            .position(|((x, y, angle), width)| {
                let (dx, dy) = (point.x - x, point.y - y);
                let (sin, cos) = (-angle).sin_cos();
                let (lx, ly) = (dx * cos - dy * sin, dx * sin + dy * cos);
                lx >= -(half + PILL_GAP + width) && lx <= half && ly.abs() <= half
            })
    }

    /// The fan, `progress` of the way out of the stack, with the pointed-at item's name lit.
    pub fn draw(
        &self,
        labels: &[String],
        icons: &[Option<ImageSurface>],
        progress: f64,
        hovered: Option<usize>,
        dark: bool,
    ) -> Option<Picture> {
        let surface = text::canvas(self.size.w.max(1), self.size.h.max(1)).ok()?;
        {
            let cr = Context::new(&surface).ok()?;
            let (r, g, b) = ink(dark);
            let half = FAN_ICON / 2.0;
            let start = (0.0, -half);
            for (index, (((x, y, angle), width), label)) in
                self.places.iter().zip(&self.widths).zip(labels).enumerate()
            {
                // On its way out of the stack, each a little behind the one under it.
                let t = ((progress * (1.0 + index as f64 * 0.04)) - index as f64 * 0.02).clamp(0.0, 1.0);
                let (cx, cy) = (start.0 + (x - start.0) * t, start.1 + (y - start.1) * t);
                cr.save().ok()?;
                cr.translate(cx - self.origin.x, cy - self.origin.y);
                cr.rotate(angle * t);
                let lit = hovered == Some(index);
                let pill_x = -(half + PILL_GAP + width);
                text::rounded_rectangle(
                    &cr,
                    pill_x,
                    -PILL_HEIGHT / 2.0,
                    *width,
                    PILL_HEIGHT,
                    PILL_HEIGHT / 2.0,
                );
                let fill = match (lit, dark) {
                    (true, _) => (r, g, b, 0.95),
                    (false, true) => (0.13, 0.13, 0.15, 0.88),
                    (false, false) => (0.98, 0.98, 0.97, 0.92),
                };
                cr.set_source_rgba(fill.0, fill.1, fill.2, fill.3 * t);
                cr.fill_preserve().ok()?;
                cr.set_source_rgba(1.0, 1.0, 1.0, if dark { 0.14 * t } else { 0.5 * t });
                cr.set_line_width(1.0);
                cr.stroke().ok()?;
                let text = layout(&cr, label, 10.0);
                let (_, text_height) = text.pixel_size();
                let color = if lit { ink(!dark) } else { (r, g, b) };
                cr.set_source_rgba(color.0, color.1, color.2, t);
                cr.move_to(pill_x + 14.0, -f64::from(text_height) / 2.0);
                pangocairo::functions::show_layout(&cr, &text);
                if let Some(Some(icon)) = icons.get(index) {
                    cr.translate(-half, -half);
                    paint_fitted(&cr, icon, FAN_ICON, t);
                }
                cr.restore().ok()?;
            }
        }
        text::picture(surface)
    }
}

/// A grid laid out: how many columns, how many rows show at once, and its size.
pub struct Grid {
    pub columns: usize,
    pub rows: usize,
    pub size: Size<i32, Logical>,
}

impl Grid {
    pub fn new(count: usize) -> Self {
        let columns = count.clamp(1, GRID_COLUMNS);
        let rows = count.div_ceil(columns).clamp(1, GRID_ROWS);
        let size = Size::from((
            GRID_PADDING * 2 + columns as i32 * CELL_WIDTH,
            GRID_PADDING * 2 + GRID_HEADER + rows as i32 * CELL_HEIGHT,
        ));
        Self { columns, rows, size }
    }

    /// Where the `index`th shown cell is, from the grid's top left.
    pub fn cell(&self, index: usize) -> Rectangle<i32, Logical> {
        let (row, column) = (index / self.columns, index % self.columns);
        Rectangle::new(
            (
                GRID_PADDING + column as i32 * CELL_WIDTH,
                GRID_PADDING + GRID_HEADER + row as i32 * CELL_HEIGHT,
            )
                .into(),
            (CELL_WIDTH, CELL_HEIGHT).into(),
        )
    }

    /// Where the button that opens the folder is, from the grid's top left.
    pub fn button(&self, label: &str) -> Rectangle<i32, Logical> {
        let width = text_width(label, 9.5).ceil() as i32 + 20;
        Rectangle::new(
            (self.size.w - GRID_PADDING - width, GRID_PADDING + 4).into(),
            (width, GRID_HEADER - 12).into(),
        )
    }

    /// The grid: the folder's name and the button to open it, then a page of items, the
    /// pointed-at one shaded.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &self,
        title: &str,
        button: &str,
        cells: &[(String, Option<ImageSurface>)],
        hovered: Option<usize>,
        button_hovered: bool,
        dark: bool,
    ) -> Option<Picture> {
        let surface = text::canvas(self.size.w, self.size.h).ok()?;
        {
            let cr = Context::new(&surface).ok()?;
            paint_pane(
                &cr,
                f64::from(self.size.w),
                f64::from(self.size.h),
                f64::from(GRID_RADIUS),
                dark,
                Tier::Menu,
            );
            let (r, g, b) = ink(dark);
            let heading = layout(&cr, title, 11.0);
            heading.set_font_description(Some(&text::font("Bold 11")));
            heading.set_width((self.size.w - GRID_PADDING * 2 - 140) * pango::SCALE);
            heading.set_ellipsize(pango::EllipsizeMode::End);
            cr.set_source_rgb(r, g, b);
            cr.move_to(f64::from(GRID_PADDING + 6), f64::from(GRID_PADDING + 8));
            pangocairo::functions::show_layout(&cr, &heading);

            let rect = self.button(button);
            text::rounded_rectangle(
                &cr,
                f64::from(rect.loc.x),
                f64::from(rect.loc.y),
                f64::from(rect.size.w),
                f64::from(rect.size.h),
                f64::from(rect.size.h) / 2.0,
            );
            cr.set_source_rgba(r, g, b, if button_hovered { 0.18 } else { 0.08 });
            cr.fill().ok()?;
            let label = layout(&cr, button, 9.5);
            let (label_width, label_height) = label.pixel_size();
            cr.set_source_rgb(r, g, b);
            cr.move_to(
                f64::from(rect.loc.x) + f64::from(rect.size.w - label_width) / 2.0,
                f64::from(rect.loc.y) + f64::from(rect.size.h - label_height) / 2.0,
            );
            pangocairo::functions::show_layout(&cr, &label);

            for (index, (name, icon)) in cells.iter().enumerate() {
                let cell = self.cell(index);
                let (x, y) = (f64::from(cell.loc.x), f64::from(cell.loc.y));
                if hovered == Some(index) {
                    text::rounded_rectangle(
                        &cr,
                        x + 2.0,
                        y,
                        f64::from(CELL_WIDTH) - 4.0,
                        f64::from(CELL_HEIGHT) - 4.0,
                        8.0,
                    );
                    cr.set_source_rgba(r, g, b, 0.10);
                    cr.fill().ok()?;
                }
                if let Some(icon) = icon {
                    cr.save().ok()?;
                    cr.translate(x + f64::from(CELL_WIDTH - GRID_ICON) / 2.0, y + 6.0);
                    paint_fitted(&cr, icon, f64::from(GRID_ICON), 1.0);
                    cr.restore().ok()?;
                }
                let text = layout(&cr, name, 8.5);
                text.set_width((CELL_WIDTH - 8) * pango::SCALE);
                text.set_wrap(pango::WrapMode::WordChar);
                text.set_ellipsize(pango::EllipsizeMode::End);
                text.set_height(-2);
                text.set_alignment(pango::Alignment::Center);
                cr.set_source_rgb(r, g, b);
                cr.move_to(x + 4.0, y + f64::from(GRID_ICON) + 10.0);
                pangocairo::functions::show_layout(&cr, &text);
            }
        }
        text::picture(surface)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(name: &str, kind: &str, added: i64) -> Item {
        Item {
            path: PathBuf::from(name),
            name: name.into(),
            is_dir: false,
            kind: kind.into(),
            added,
            modified: added,
            created: added,
        }
    }

    #[test]
    fn items_are_sorted_as_asked() {
        let mut items = vec![
            item("b.png", "image/png", 2),
            item("A.zip", "application/zip", 1),
            item("c.png", "image/png", 3),
        ];
        sort_items(&mut items, Sort::Added);
        assert_eq!(
            items.iter().map(|item| item.name.as_str()).collect::<Vec<_>>(),
            ["c.png", "b.png", "A.zip"]
        );
        sort_items(&mut items, Sort::Name);
        assert_eq!(
            items.iter().map(|item| item.name.as_str()).collect::<Vec<_>>(),
            ["A.zip", "b.png", "c.png"]
        );
        sort_items(&mut items, Sort::Kind);
        assert_eq!(items[0].name, "A.zip");
    }

    #[test]
    fn counts_are_grouped_by_thousands() {
        assert_eq!(count_text(7), "7");
        assert_eq!(count_text(1301), "1,301");
        assert_eq!(count_text(1234567), "1,234,567");
    }

    #[test]
    fn a_fan_finds_the_item_under_a_point() {
        let fan = Fan::new(&["one".into(), "two".into()]);
        let (x, y, _) = fan.places[1];
        assert_eq!(fan.hit(Point::from((x, y))), Some(1));
        // On the second item's name, to the left of its icon.
        assert_eq!(fan.hit(Point::from((x - FAN_ICON, y))), Some(1));
        assert_eq!(fan.hit(Point::from((x + 100.0, y))), None);
    }

    #[test]
    fn a_grid_pages_at_most_its_rows() {
        let grid = Grid::new(30);
        assert_eq!((grid.columns, grid.rows), (GRID_COLUMNS, GRID_ROWS));
        let small = Grid::new(3);
        assert_eq!((small.columns, small.rows), (3, 1));
        assert_eq!(small.cell(2).loc.x, GRID_PADDING + 2 * CELL_WIDTH);
    }
}
