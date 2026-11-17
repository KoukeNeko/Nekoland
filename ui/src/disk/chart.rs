//! The disk map: a folder as a sunburst, rings of what is in it around it, or as a treemap,
//! rectangles inside rectangles. Each is laid out once into shapes that are both drawn and
//! looked up under the pointer.

use std::f64::consts::TAU;

use gtk::{cairo, pango};

use crate::scan::{Kind, Tree};

/// How many rings a sunburst draws around its middle.
pub const RINGS: usize = 4;
/// The middle's share of the radius.
const MIDDLE: f64 = 0.24;
/// A wedge narrower than this, in radians, is not drawn.
const NARROWEST: f64 = 0.006;
/// Folders inside folders a treemap draws, past the one shown.
const TREEMAP_DEPTH: usize = 3;
/// A treemap folder's strip for its name, and its edge.
const HEADER: f64 = 16.0;
const INSET: f64 = 2.0;
/// A treemap tile smaller than this on a side has no tiles drawn in it.
const SMALLEST_FOLDER: f64 = 36.0;

/// What a shape stands for: an item of the scan, or the disk's free space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Item {
    Node(usize),
    Free,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Wedge {
    pub item: Item,
    /// 1 for the ring next to the middle.
    pub ring: usize,
    pub start: f64,
    pub end: f64,
    /// Which of the shown folder's children it is under, for its colour.
    pub hue: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Tile {
    pub item: Item,
    pub depth: usize,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub hue: usize,
}

/// The wedges of `focus`'s rings, its largest children first, clockwise from the top; with
/// `free`, the disk's free space is a wedge of its own after them.
pub fn sunburst(tree: &Tree, focus: usize, free: Option<u64>) -> Vec<Wedge> {
    let whole = tree.nodes[focus].size + free.unwrap_or(0);
    let mut wedges = Vec::new();
    if whole == 0 {
        return wedges;
    }
    let mut left: Vec<(usize, usize, f64, f64, usize)> = Vec::new();
    let mut at = 0.0;
    for (hue, child) in tree.children(focus).into_iter().enumerate() {
        let span = TAU * tree.nodes[child].size as f64 / whole as f64;
        left.push((child, 1, at, at + span, hue));
        at += span;
    }
    if let Some(free) = free.filter(|free| *free > 0) {
        wedges.push(Wedge { item: Item::Free, ring: 1, start: at, end: at + TAU * free as f64 / whole as f64, hue: 0 });
    }
    while let Some((id, ring, start, end, hue)) = left.pop() {
        if end - start < NARROWEST {
            continue;
        }
        wedges.push(Wedge { item: Item::Node(id), ring, start, end, hue });
        let node = &tree.nodes[id];
        if ring == RINGS || node.size == 0 {
            continue;
        }
        let mut at = start;
        for child in tree.children(id) {
            let span = (end - start) * tree.nodes[child].size as f64 / node.size as f64;
            left.push((child, ring + 1, at, at + span, hue));
            at += span;
        }
    }
    wedges
}

/// The radius each ring starts at, in a sunburst of `radius`, and how thick a ring is.
fn rings(radius: f64) -> (f64, f64) {
    let middle = radius * MIDDLE;
    (middle, (radius - middle) / RINGS as f64)
}

/// The wedge at `(x, y)` from the middle of a sunburst of `radius`, or None in the middle
/// or outside it; `Some(None)` is the middle.
pub fn wedge_at(wedges: &[Wedge], radius: f64, x: f64, y: f64) -> Option<Option<&Wedge>> {
    let distance = x.hypot(y);
    let (middle, thickness) = rings(radius);
    if distance <= middle {
        return Some(None);
    }
    if distance > radius {
        return None;
    }
    let ring = ((distance - middle) / thickness) as usize + 1;
    // Clockwise from the top, as the wedges are laid out.
    let angle = (x.atan2(-y) + TAU) % TAU;
    Some(wedges.iter().find(|wedge| wedge.ring == ring && wedge.start <= angle && angle < wedge.end))
}

/// The tiles of `focus` in a `width` by `height` treemap, each folder's inside it under a
/// strip for its name; with `free`, the disk's free space takes its share as a tile too.
pub fn treemap(tree: &Tree, focus: usize, width: f64, height: f64, free: Option<u64>) -> Vec<Tile> {
    let mut tiles = Vec::new();
    let mut items: Vec<(Item, u64, usize)> =
        tree.children(focus).into_iter().enumerate().map(|(hue, child)| (Item::Node(child), tree.nodes[child].size, hue)).collect();
    if let Some(free) = free.filter(|free| *free > 0) {
        items.push((Item::Free, free, 0));
    }
    let mut left = vec![(items, (0.0, 0.0, width, height), 1)];
    while let Some((items, (x, y, width, height), depth)) = left.pop() {
        for ((item, _, hue), (x, y, width, height)) in items.iter().zip(squarify(
            &items.iter().map(|(_, size, _)| *size as f64).collect::<Vec<_>>(),
            (x, y, width, height),
        )) {
            if width < 1.0 || height < 1.0 {
                continue;
            }
            tiles.push(Tile { item: *item, depth, x, y, width, height, hue: *hue });
            let Item::Node(id) = item else { continue };
            let node = &tree.nodes[*id];
            if depth == TREEMAP_DEPTH || node.kind != Kind::Folder || width.min(height) < SMALLEST_FOLDER {
                continue;
            }
            let inside = (x + INSET, y + HEADER, width - 2.0 * INSET, height - HEADER - INSET);
            if inside.2 < 1.0 || inside.3 < 1.0 {
                continue;
            }
            let children = tree.children(*id).into_iter().map(|child| (Item::Node(child), tree.nodes[child].size, *hue)).collect();
            left.push((children, inside, depth + 1));
        }
    }
    tiles
}

/// The deepest tile under `(x, y)`.
pub fn tile_at(tiles: &[Tile], x: f64, y: f64) -> Option<&Tile> {
    tiles
        .iter()
        .filter(|tile| tile.x <= x && x < tile.x + tile.width && tile.y <= y && y < tile.y + tile.height)
        .max_by_key(|tile| tile.depth)
}

/// Bruls, Huizing and van Wijk's squarified treemap: `sizes`, largest first, as rectangles
/// filling `area`, each row of them laid along its shorter side so that they stay near square.
fn squarify(sizes: &[f64], (x, y, width, height): (f64, f64, f64, f64)) -> Vec<(f64, f64, f64, f64)> {
    let total: f64 = sizes.iter().sum();
    let mut out = Vec::with_capacity(sizes.len());
    if total <= 0.0 || width <= 0.0 || height <= 0.0 {
        return sizes.iter().map(|_| (x, y, 0.0, 0.0)).collect();
    }
    let scale = width * height / total;
    let areas: Vec<f64> = sizes.iter().map(|size| size * scale).collect();
    let (mut x, mut y, mut width, mut height) = (x, y, width, height);
    let mut start = 0;
    while start < areas.len() {
        let side = width.min(height);
        let worst = |row: &[f64]| {
            let sum: f64 = row.iter().sum();
            let (most, least) = row.iter().fold((0.0_f64, f64::MAX), |(most, least), area| (most.max(*area), least.min(*area)));
            if sum <= 0.0 || least <= 0.0 {
                return f64::MAX;
            }
            (side * side * most / (sum * sum)).max(sum * sum / (side * side * least))
        };
        let mut end = start + 1;
        while end < areas.len() && worst(&areas[start..=end]) <= worst(&areas[start..end]) {
            end += 1;
        }
        let row = &areas[start..end];
        let sum: f64 = row.iter().sum();
        let thickness = if side > 0.0 { sum / side } else { 0.0 };
        let mut along = 0.0;
        for area in row {
            let length = if thickness > 0.0 { area / thickness } else { 0.0 };
            if width >= height {
                out.push((x, y + along, thickness, length));
            } else {
                out.push((x + along, y, length, thickness));
            }
            along += length;
        }
        if width >= height {
            x += thickness;
            width -= thickness;
        } else {
            y += thickness;
            height -= thickness;
        }
        start = end;
    }
    out
}

/// The colour of what is under the shown folder's `hue`th child, lighter further out.
pub fn colour(hue: usize, depth: usize, dark: bool) -> (f64, f64, f64) {
    // Apart enough that neighbours differ, starting from the accent's blue.
    const HUES: [f64; 8] = [212.0, 28.0, 140.0, 330.0, 48.0, 262.0, 174.0, 2.0];
    let lightness = if dark { 0.46 + 0.07 * depth as f64 } else { 0.50 + 0.08 * depth as f64 };
    hsl(HUES[hue % HUES.len()], 0.62, lightness.min(0.85))
}

pub fn free_colour(dark: bool) -> (f64, f64, f64) {
    if dark {
        (0.30, 0.30, 0.32)
    } else {
        (0.84, 0.84, 0.86)
    }
}

fn hsl(hue: f64, saturation: f64, lightness: f64) -> (f64, f64, f64) {
    let chroma = (1.0 - (2.0 * lightness - 1.0).abs()) * saturation;
    let part = chroma * (1.0 - ((hue / 60.0) % 2.0 - 1.0).abs());
    let (red, green, blue) = match (hue / 60.0) as u32 {
        0 => (chroma, part, 0.0),
        1 => (part, chroma, 0.0),
        2 => (0.0, chroma, part),
        3 => (0.0, part, chroma),
        4 => (part, 0.0, chroma),
        _ => (chroma, 0.0, part),
    };
    let lift = lightness - chroma / 2.0;
    (red + lift, green + lift, blue + lift)
}

/// What a chart is drawn with besides its shapes.
pub struct Look<'a> {
    pub dark: bool,
    pub hovered: Option<Item>,
    pub chosen: Option<Item>,
    /// The shown folder's name and size, written in a sunburst's middle.
    pub middle: (&'a str, &'a str),
    pub ink: gtk::gdk::RGBA,
}

fn fill_shape(cr: &cairo::Context, item: Item, (red, green, blue): (f64, f64, f64), look: &Look) {
    let lift = if look.hovered == Some(item) { 0.12 } else { 0.0 };
    cr.set_source_rgb((red + lift).min(1.0), (green + lift).min(1.0), (blue + lift).min(1.0));
    // A failed fill leaves the shape blank, with nothing better to show.
    let _ = cr.fill_preserve();
    let edge = if look.dark { 0.12 } else { 1.0 };
    cr.set_source_rgb(edge, edge, edge);
    cr.set_line_width(if look.chosen == Some(item) { 3.0 } else { 1.0 });
    if look.chosen == Some(item) {
        let ink = &look.ink;
        cr.set_source_rgb(ink.red().into(), ink.green().into(), ink.blue().into());
    }
    let _ = cr.stroke();
}

fn shape_colour(item: Item, hue: usize, depth: usize, dark: bool) -> (f64, f64, f64) {
    match item {
        Item::Free => free_colour(dark),
        Item::Node(_) => colour(hue, depth, dark),
    }
}

pub fn draw_sunburst(cr: &cairo::Context, layout: &pango::Layout, wedges: &[Wedge], width: f64, height: f64, look: &Look) {
    let radius = width.min(height) / 2.0 - 8.0;
    if radius <= 0.0 {
        return;
    }
    let (middle, thickness) = rings(radius);
    let (cx, cy) = (width / 2.0, height / 2.0);
    for wedge in wedges {
        let inner = middle + thickness * (wedge.ring - 1) as f64;
        // Angles from the top, clockwise, as cairo's from the right.
        let (start, end) = (wedge.start - TAU / 4.0, wedge.end - TAU / 4.0);
        cr.new_path();
        cr.arc(cx, cy, inner + thickness, start, end);
        cr.arc_negative(cx, cy, inner, end, start);
        cr.close_path();
        fill_shape(cr, wedge.item, shape_colour(wedge.item, wedge.hue, wedge.ring, look.dark), look);
    }
    let ink = &look.ink;
    cr.set_source_rgba(ink.red().into(), ink.green().into(), ink.blue().into(), ink.alpha().into());
    let (name, size) = look.middle;
    layout.set_width(((middle * 1.7) * pango::SCALE as f64) as i32);
    layout.set_ellipsize(pango::EllipsizeMode::Middle);
    layout.set_alignment(pango::Alignment::Center);
    layout.set_markup(&format!(
        "{}\n<b>{}</b>",
        glib_escape(name),
        glib_escape(size)
    ));
    let (_, text_height) = layout.pixel_size();
    cr.move_to(cx - middle * 0.85, cy - text_height as f64 / 2.0);
    pangocairo::functions::show_layout(cr, layout);
}

pub fn draw_treemap(cr: &cairo::Context, layout: &pango::Layout, tree: &Tree, tiles: &[Tile], look: &Look) {
    layout.set_alignment(pango::Alignment::Left);
    layout.set_ellipsize(pango::EllipsizeMode::End);
    for tile in tiles {
        cr.new_path();
        cr.rectangle(tile.x + 0.5, tile.y + 0.5, (tile.width - 1.0).max(0.0), (tile.height - 1.0).max(0.0));
        let (red, green, blue) = shape_colour(tile.item, tile.hue, tile.depth, look.dark);
        fill_shape(cr, tile.item, (red, green, blue), look);
        let name = match tile.item {
            Item::Node(id) => tree.nodes[id].name.as_str(),
            Item::Free => "可用空間",
        };
        if tile.width < 48.0 || tile.height < HEADER {
            continue;
        }
        // Dark text on light tiles and light on dark, by how bright the tile is.
        let shade = if 0.299 * red + 0.587 * green + 0.114 * blue > 0.6 { 0.1 } else { 1.0 };
        cr.set_source_rgb(shade, shade, shade);
        layout.set_width(((tile.width - 8.0) * pango::SCALE as f64) as i32);
        layout.set_text(name);
        cr.move_to(tile.x + 4.0, tile.y + 1.0);
        pangocairo::functions::show_layout(cr, layout);
    }
}

fn glib_escape(text: &str) -> String {
    gtk::glib::markup_escape_text(text).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::{Node, ROOT};

    fn node(name: &str, kind: Kind, size: u64, parent: Option<usize>, children: &[usize]) -> Node {
        Node { name: name.into(), kind, size, files: 0, modified: 0, parent, children: children.to_vec() }
    }

    /// root 100: a 75 (x 50, y 25), z 25.
    fn tree() -> Tree {
        Tree {
            root: "/r".into(),
            scanned: 0,
            nodes: vec![
                node("/r", Kind::Folder, 100, None, &[1, 4]),
                node("a", Kind::Folder, 75, Some(0), &[2, 3]),
                node("x", Kind::File, 50, Some(1), &[]),
                node("y", Kind::File, 25, Some(1), &[]),
                node("z", Kind::File, 25, Some(0), &[]),
            ],
            denied: 0,
            free: None,
        }
    }

    #[test]
    fn wedges_share_the_circle_by_size() {
        let wedges = sunburst(&tree(), ROOT, None);
        let find = |id| wedges.iter().find(|wedge| wedge.item == Item::Node(id)).unwrap();
        assert!((find(1).end - TAU * 0.75).abs() < 1e-9);
        assert_eq!((find(2).ring, find(2).hue), (2, 0));
        assert!((find(3).end - find(1).end).abs() < 1e-9);
        assert_eq!(find(4).hue, 1);
        // In the first ring, the top right is in a; the top left, past three quarters, in z;
        // the second ring there, in x.
        assert_eq!(wedge_at(&wedges, 100.0, 30.0, -10.0).unwrap().unwrap().item, Item::Node(1));
        assert_eq!(wedge_at(&wedges, 100.0, -30.0, -10.0).unwrap().unwrap().item, Item::Node(4));
        assert_eq!(wedge_at(&wedges, 100.0, 50.0, -10.0).unwrap().unwrap().item, Item::Node(2));
        assert_eq!(wedge_at(&wedges, 100.0, 1.0, 1.0), Some(None));
        assert_eq!(wedge_at(&wedges, 100.0, 200.0, 0.0), None);
    }

    #[test]
    fn free_space_takes_its_share() {
        let wedges = sunburst(&tree(), ROOT, Some(100));
        let free = wedges.iter().find(|wedge| wedge.item == Item::Free).unwrap();
        assert!((free.end - free.start - TAU / 2.0).abs() < 1e-9);
    }

    #[test]
    fn tiles_fill_the_map_by_size() {
        let tiles = treemap(&tree(), ROOT, 400.0, 200.0, None);
        let top: f64 = tiles.iter().filter(|tile| tile.depth == 1).map(|tile| tile.width * tile.height).sum();
        assert!((top - 80_000.0).abs() < 1e-6);
        let a = tiles.iter().find(|tile| tile.item == Item::Node(1)).unwrap();
        assert!((a.width * a.height - 60_000.0).abs() < 1e-6);
        assert_eq!(tile_at(&tiles, a.x + 10.0, a.y + HEADER + 5.0).unwrap().depth, 2);
        assert_eq!(tile_at(&tiles, a.x + 10.0, a.y + 2.0).unwrap().item, Item::Node(1));
    }

    #[test]
    fn squares_stay_near_square() {
        let rects = squarify(&[6.0, 6.0, 4.0, 3.0, 2.0, 2.0, 1.0], (0.0, 0.0, 6.0, 4.0));
        let area: f64 = rects.iter().map(|(_, _, width, height)| width * height).sum();
        assert!((area - 24.0).abs() < 1e-9);
        assert!(rects.iter().all(|(_, _, width, height)| width.max(*height) / width.min(*height) < 4.0));
    }
}
