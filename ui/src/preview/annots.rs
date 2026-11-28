//! A PDF's annotations, as Preview's markup puts them: text highlighted, underlined or struck
//! out, shapes, lines and arrows, sketches, text boxes, notes and signatures. They are kept as
//! PDF annotations, so they stay editable in the file once it is saved.
//!
//! Places here are in points from the page's top left, as the rest of the window has them;
//! poppler has them up from the page's foot.

use gtk::glib;
use gtk::prelude::Cast;
use gtk::glib::translate::{from_glib_full, from_glib_none, ToGlibPtr};
use poppler::ffi;
use poppler::prelude::*;
use std::ffi::{c_char, c_double, c_int, CString};

use super::Place;

/// What poppler 24.12 and later have that the bindings do not.
#[repr(C)]
struct FontDescription {
    font_name: *mut c_char,
    size_pt: c_double,
    stretch: c_int,
    weight: c_int,
    style: c_int,
}

#[repr(C)]
struct PopplerPath {
    _private: [u8; 0],
}

extern "C" {
    fn poppler_annot_free_text_new(doc: *mut ffi::PopplerDocument, rect: *mut ffi::PopplerRectangle) -> *mut ffi::PopplerAnnot;
    fn poppler_annot_free_text_set_font_desc(annot: *mut ffi::PopplerAnnot, desc: *mut FontDescription);
    fn poppler_annot_free_text_set_font_color(annot: *mut ffi::PopplerAnnot, color: *mut ffi::PopplerColor);
    fn poppler_font_description_new(name: *const c_char) -> *mut FontDescription;
    fn poppler_font_description_free(desc: *mut FontDescription);
    fn poppler_annot_get_border_width(annot: *mut ffi::PopplerAnnot, width: *mut c_double) -> glib::ffi::gboolean;
    fn poppler_annot_set_border_width(annot: *mut ffi::PopplerAnnot, width: c_double);
    fn poppler_annot_stamp_new(doc: *mut ffi::PopplerDocument, rect: *mut ffi::PopplerRectangle) -> *mut ffi::PopplerAnnot;
    fn poppler_annot_stamp_set_custom_image(
        annot: *mut ffi::PopplerAnnot,
        image: *mut gtk::cairo::ffi::cairo_surface_t,
        error: *mut *mut glib::ffi::GError,
    ) -> glib::ffi::gboolean;
}

/// Poppler's ink annotations, which an older poppler, as Debian 13's 25.03, lacks; they are
/// looked up as Preview runs, so that it builds and starts there, without sketches.
struct InkFunctions {
    new: unsafe extern "C" fn(*mut ffi::PopplerDocument, *mut ffi::PopplerRectangle) -> *mut ffi::PopplerAnnot,
    set_ink_list: unsafe extern "C" fn(*mut ffi::PopplerAnnot, *mut *mut PopplerPath, usize),
    get_ink_list: unsafe extern "C" fn(*mut ffi::PopplerAnnot, *mut usize) -> *mut *mut PopplerPath,
    path_new_from_array: unsafe extern "C" fn(*mut ffi::PopplerPoint, usize) -> *mut PopplerPath,
    path_get_points: unsafe extern "C" fn(*mut PopplerPath, *mut usize) -> *mut ffi::PopplerPoint,
    path_free: unsafe extern "C" fn(*mut PopplerPath),
}

fn ink_functions() -> Option<&'static InkFunctions> {
    static FUNCTIONS: std::sync::OnceLock<Option<InkFunctions>> = std::sync::OnceLock::new();
    FUNCTIONS.get_or_init(load_ink_functions).as_ref()
}

fn load_ink_functions() -> Option<InkFunctions> {
    // Safety: each name is looked up in the poppler-glib this program links to, and has there
    // the signature given for it in poppler-annot.h.
    unsafe {
        Some(InkFunctions {
            new: std::mem::transmute(poppler_symbol(c"poppler_annot_ink_new")?),
            set_ink_list: std::mem::transmute(poppler_symbol(c"poppler_annot_ink_set_ink_list")?),
            get_ink_list: std::mem::transmute(poppler_symbol(c"poppler_annot_ink_get_ink_list")?),
            path_new_from_array: std::mem::transmute(poppler_symbol(c"poppler_path_new_from_array")?),
            path_get_points: std::mem::transmute(poppler_symbol(c"poppler_path_get_points")?),
            path_free: std::mem::transmute(poppler_symbol(c"poppler_path_free")?),
        })
    }
}

fn poppler_symbol(name: &std::ffi::CStr) -> Option<*mut std::ffi::c_void> {
    // Safety: dlsym only reads the name, which is NUL-terminated.
    let address = unsafe { libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr()) };
    (!address.is_null()).then_some(address)
}

/// The kinds the window draws, writes and lists.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Highlight,
    Underline,
    StrikeOut,
    Square,
    Circle,
    Line,
    Ink,
    FreeText,
    Note,
    Stamp,
    Other,
}

impl Kind {
    /// What the sidebar of highlights and notes calls it.
    pub fn label(self) -> &'static str {
        match self {
            Kind::Highlight => "標示重點",
            Kind::Underline => "底線",
            Kind::StrikeOut => "刪除線",
            Kind::Square => "矩形",
            Kind::Circle => "橢圓形",
            Kind::Line => "線條",
            Kind::Ink => "草圖",
            Kind::FreeText => "文字",
            Kind::Note => "附註",
            Kind::Stamp => "簽名",
            Kind::Other => "註解",
        }
    }

    /// Whether its box is its size, so dragging its corner resizes it.
    pub fn resizable(self) -> bool {
        matches!(self, Kind::Square | Kind::Circle | Kind::FreeText | Kind::Stamp)
    }

    /// Text marked over the page's own text, which is chosen with it rather than dragged.
    pub fn marks_text(self) -> bool {
        matches!(self, Kind::Highlight | Kind::Underline | Kind::StrikeOut)
    }
}

/// An annotation on a page, as the window lists it.
#[derive(Clone)]
pub struct Annotation {
    pub annot: poppler::Annot,
    pub kind: Kind,
    pub place: Place,
    pub contents: String,
    pub color: Option<(f64, f64, f64)>,
}

pub fn colour(red: f64, green: f64, blue: f64) -> ffi::PopplerColor {
    let channel = |value: f64| (value.clamp(0.0, 1.0) * 65535.0).round() as u16;
    ffi::PopplerColor { red: channel(red), green: channel(green), blue: channel(blue) }
}

/// A place from the page's top left as poppler has it, up from the foot.
fn up(place: Place, height: f64) -> ffi::PopplerRectangle {
    let (x1, y1, x2, y2) = place;
    ffi::PopplerRectangle { x1: x1.min(x2), y1: height - y1.max(y2), x2: x1.max(x2), y2: height - y1.min(y2) }
}

fn point(x: f64, y: f64, height: f64) -> ffi::PopplerPoint {
    ffi::PopplerPoint { x, y: height - y }
}

fn document_ptr(document: &poppler::Document) -> *mut ffi::PopplerDocument {
    document.to_glib_none().0
}

fn page_ptr(page: &poppler::Page) -> *mut ffi::PopplerPage {
    page.to_glib_none().0
}

fn annot_ptr(annot: &poppler::Annot) -> *mut ffi::PopplerAnnot {
    annot.to_glib_none().0
}

/// Puts an annotation made by `make` on `page` in `colour`, and hands it back.
fn place_annot(page: &poppler::Page, raw: *mut ffi::PopplerAnnot, colour_of: Option<(f64, f64, f64)>) -> Option<poppler::Annot> {
    if raw.is_null() {
        return None;
    }
    let annot: poppler::Annot = unsafe { from_glib_full(raw) };
    if let Some((red, green, blue)) = colour_of {
        let mut c = colour(red, green, blue);
        unsafe { ffi::poppler_annot_set_color(annot_ptr(&annot), &mut c) };
    }
    unsafe { ffi::poppler_page_add_annot(page_ptr(page), annot_ptr(&annot)) };
    Some(annot)
}

/// Each line of the page's text inside `area`, as poppler would choose it.
pub fn text_lines(page: &poppler::Page, area: &poppler::Rectangle, style: poppler::SelectionStyle) -> Vec<Place> {
    // Measured at four pixels to the point, so the rounding to whole pixels stays small.
    const FINE: f64 = 4.0;
    let mut area = *area;
    let Some(region) = page.selected_region(FINE, style, &mut area) else { return Vec::new() };
    (0..region.num_rectangles())
        .map(|index| {
            let r = region.rectangle(index);
            let (x, y) = (f64::from(r.x()) / FINE, f64::from(r.y()) / FINE);
            (x, y, x + f64::from(r.width()) / FINE, y + f64::from(r.height()) / FINE)
        })
        .collect()
}

/// Highlights, underlines or strikes out the lines given.
pub fn mark_text(document: &poppler::Document, page: &poppler::Page, kind: Kind, lines: &[Place], colour_of: (f64, f64, f64)) -> Option<poppler::Annot> {
    let height = page.size().1;
    let first = lines.first()?;
    let bounds = lines.iter().fold(*first, |(a, b, c, d), &(x1, y1, x2, y2)| (a.min(x1), b.min(y1), c.max(x2), d.max(y2)));
    let mut rect = up(bounds, height);
    let quads: Vec<ffi::PopplerQuadrilateral> = lines
        .iter()
        .map(|&(x1, y1, x2, y2)| ffi::PopplerQuadrilateral {
            p1: point(x1, y1, height),
            p2: point(x2, y1, height),
            p3: point(x1, y2, height),
            p4: point(x2, y2, height),
        })
        .collect();
    let raw = unsafe {
        let array = glib::ffi::g_array_sized_new(
            glib::ffi::GFALSE,
            glib::ffi::GFALSE,
            std::mem::size_of::<ffi::PopplerQuadrilateral>() as u32,
            quads.len() as u32,
        );
        glib::ffi::g_array_append_vals(array, quads.as_ptr().cast(), quads.len() as u32);
        let make = match kind {
            Kind::Underline => ffi::poppler_annot_text_markup_new_underline,
            Kind::StrikeOut => ffi::poppler_annot_text_markup_new_strikeout,
            _ => ffi::poppler_annot_text_markup_new_highlight,
        };
        let raw = make(document_ptr(document), &mut rect, array);
        glib::ffi::g_array_unref(array);
        raw
    };
    place_annot(page, raw, Some(colour_of))
}

/// A rectangle or an ellipse inside `place`, its border `width` points wide, filled when
/// `fill` is given.
pub fn shape(
    document: &poppler::Document,
    page: &poppler::Page,
    ellipse: bool,
    place: Place,
    stroke: (f64, f64, f64),
    fill: Option<(f64, f64, f64)>,
    width: f64,
) -> Option<poppler::Annot> {
    let mut rect = up(place, page.size().1);
    let raw = unsafe {
        let raw = if ellipse {
            ffi::poppler_annot_circle_new(document_ptr(document), &mut rect)
        } else {
            ffi::poppler_annot_square_new(document_ptr(document), &mut rect)
        };
        if !raw.is_null() {
            poppler_annot_set_border_width(raw, width);
            if let Some((red, green, blue)) = fill {
                let mut inside = colour(red, green, blue);
                if ellipse {
                    ffi::poppler_annot_circle_set_interior_color(raw.cast(), &mut inside);
                } else {
                    ffi::poppler_annot_square_set_interior_color(raw.cast(), &mut inside);
                }
            }
        }
        raw
    };
    place_annot(page, raw, Some(stroke))
}

/// A straight line from one point to another.
pub fn line(document: &poppler::Document, page: &poppler::Page, from: (f64, f64), to: (f64, f64), stroke: (f64, f64, f64), width: f64) -> Option<poppler::Annot> {
    let height = page.size().1;
    let pad = width.max(1.0) * 2.0;
    let mut rect = up((from.0.min(to.0) - pad, from.1.min(to.1) - pad, from.0.max(to.0) + pad, from.1.max(to.1) + pad), height);
    let (mut start, mut end) = (point(from.0, from.1, height), point(to.0, to.1, height));
    let raw = unsafe {
        let raw = ffi::poppler_annot_line_new(document_ptr(document), &mut rect, &mut start, &mut end);
        if !raw.is_null() {
            poppler_annot_set_border_width(raw, width);
        }
        raw
    };
    place_annot(page, raw, Some(stroke))
}

/// Strokes drawn by hand, or any shape made of lines, as one annotation.
pub fn ink(document: &poppler::Document, page: &poppler::Page, strokes: &[Vec<(f64, f64)>], stroke: (f64, f64, f64), width: f64) -> Option<poppler::Annot> {
    let height = page.size().1;
    let all: Vec<(f64, f64)> = strokes.iter().flatten().copied().collect();
    let first = *all.first()?;
    let pad = width.max(1.0) * 2.0;
    let (x1, y1, x2, y2) = all.iter().fold((first.0, first.1, first.0, first.1), |(a, b, c, d), &(x, y)| (a.min(x), b.min(y), c.max(x), d.max(y)));
    let mut rect = up((x1 - pad, y1 - pad, x2 + pad, y2 + pad), height);
    let functions = ink_functions()?;
    let raw = unsafe {
        let raw = (functions.new)(document_ptr(document), &mut rect);
        if !raw.is_null() {
            let mut paths: Vec<*mut PopplerPath> = strokes
                .iter()
                .filter(|stroke| !stroke.is_empty())
                .map(|stroke| {
                    let mut points: Vec<ffi::PopplerPoint> = stroke.iter().map(|&(x, y)| point(x, y, height)).collect();
                    (functions.path_new_from_array)(points.as_mut_ptr(), points.len())
                })
                .collect();
            poppler_annot_set_border_width(raw, width);
            (functions.set_ink_list)(raw, paths.as_mut_ptr(), paths.len());
            for path in paths {
                (functions.path_free)(path);
            }
        }
        raw
    };
    place_annot(page, raw, Some(stroke))
}

/// A text box in `place` with `text` in `font` at `size` points.
pub fn free_text(
    document: &poppler::Document,
    page: &poppler::Page,
    place: Place,
    text: &str,
    font: &str,
    size: f64,
    colour_of: (f64, f64, f64),
) -> Option<poppler::Annot> {
    let mut rect = up(place, page.size().1);
    let name = CString::new(font).ok()?;
    let raw = unsafe {
        let raw = poppler_annot_free_text_new(document_ptr(document), &mut rect);
        if !raw.is_null() {
            let desc = poppler_font_description_new(name.as_ptr());
            (*desc).size_pt = size;
            poppler_annot_free_text_set_font_desc(raw, desc);
            poppler_font_description_free(desc);
            let mut c = colour(colour_of.0, colour_of.1, colour_of.2);
            poppler_annot_free_text_set_font_color(raw, &mut c);
            poppler_annot_set_border_width(raw, 0.0);
        }
        raw
    };
    let annot = place_annot(page, raw, None)?;
    annot.set_contents(text);
    Some(annot)
}

/// A note, its icon at `at`, saying `text`.
pub fn note(document: &poppler::Document, page: &poppler::Page, at: (f64, f64), text: &str, colour_of: (f64, f64, f64)) -> Option<poppler::Annot> {
    const ICON: f64 = 20.0;
    let mut rect = up((at.0, at.1, at.0 + ICON, at.1 + ICON), page.size().1);
    let raw = unsafe { ffi::poppler_annot_text_new(document_ptr(document), &mut rect) };
    let annot = place_annot(page, raw, Some(colour_of))?;
    annot.set_contents(text);
    Some(annot)
}

/// A picture put on the page in `place`: a signature, or what the magnifier shows.
pub fn stamp(document: &poppler::Document, page: &poppler::Page, place: Place, picture: &gtk::cairo::ImageSurface) -> Option<poppler::Annot> {
    let mut rect = up(place, page.size().1);
    let raw = unsafe {
        let raw = poppler_annot_stamp_new(document_ptr(document), &mut rect);
        if !raw.is_null() {
            let mut error = std::ptr::null_mut();
            poppler_annot_stamp_set_custom_image(raw, picture.to_raw_none(), &mut error);
            if !error.is_null() {
                glib::ffi::g_error_free(error);
            }
        }
        raw
    };
    place_annot(page, raw, None)
}

fn kind_of(annot: &poppler::Annot) -> Kind {
    match unsafe { ffi::poppler_annot_get_annot_type(annot_ptr(annot)) } {
        ffi::POPPLER_ANNOT_HIGHLIGHT => Kind::Highlight,
        ffi::POPPLER_ANNOT_UNDERLINE => Kind::Underline,
        ffi::POPPLER_ANNOT_STRIKE_OUT => Kind::StrikeOut,
        ffi::POPPLER_ANNOT_SQUARE => Kind::Square,
        ffi::POPPLER_ANNOT_CIRCLE => Kind::Circle,
        ffi::POPPLER_ANNOT_LINE => Kind::Line,
        ffi::POPPLER_ANNOT_INK => Kind::Ink,
        ffi::POPPLER_ANNOT_FREE_TEXT => Kind::FreeText,
        ffi::POPPLER_ANNOT_TEXT => Kind::Note,
        ffi::POPPLER_ANNOT_STAMP => Kind::Stamp,
        _ => Kind::Other,
    }
}

/// The annotations on `page` a reader puts there, not its links or form fields, in the order
/// they are drawn.
pub fn on_page(page: &poppler::Page) -> Vec<Annotation> {
    let height = page.size().1;
    let mut found = Vec::new();
    unsafe {
        let list = ffi::poppler_page_get_annot_mapping(page_ptr(page));
        let mut node = list;
        while !node.is_null() {
            let mapping = (*node).data as *mut ffi::PopplerAnnotMapping;
            let annot: poppler::Annot = from_glib_none((*mapping).annot);
            let kind = kind_of(&annot);
            let skip = matches!(
                ffi::poppler_annot_get_annot_type((*mapping).annot),
                ffi::POPPLER_ANNOT_LINK | ffi::POPPLER_ANNOT_WIDGET | ffi::POPPLER_ANNOT_POPUP
            );
            if !skip {
                let area = (*mapping).area;
                let colour_of = {
                    let c = ffi::poppler_annot_get_color((*mapping).annot);
                    if c.is_null() {
                        None
                    } else {
                        let rgb = (f64::from((*c).red) / 65535.0, f64::from((*c).green) / 65535.0, f64::from((*c).blue) / 65535.0);
                        glib::ffi::g_free(c.cast());
                        Some(rgb)
                    }
                };
                found.push(Annotation {
                    contents: annot.contents().map(|text| text.to_string()).unwrap_or_default(),
                    annot,
                    kind,
                    place: (area.x1, height - area.y2, area.x2, height - area.y1),
                    color: colour_of,
                });
            }
            node = (*node).next;
        }
        ffi::poppler_page_free_annot_mapping(list);
    }
    found
}

pub fn remove(page: &poppler::Page, annot: &poppler::Annot) {
    unsafe { ffi::poppler_page_remove_annot(page_ptr(page), annot_ptr(annot)) };
}

pub fn set_colour(annot: &poppler::Annot, colour_of: (f64, f64, f64)) {
    let mut c = colour(colour_of.0, colour_of.1, colour_of.2);
    unsafe { ffi::poppler_annot_set_color(annot_ptr(annot), &mut c) };
}

pub fn border_width(annot: &poppler::Annot) -> f64 {
    let mut width = 1.0;
    unsafe { poppler_annot_get_border_width(annot_ptr(annot), &mut width) };
    width
}

pub fn set_border_width(annot: &poppler::Annot, width: f64) {
    unsafe { poppler_annot_set_border_width(annot_ptr(annot), width) };
}

/// Puts `author`'s name on a markup annotation; the others have no place for it.
pub fn set_author(annot: &poppler::Annot, author: &str) {
    if let Some(markup) = annot.downcast_ref::<poppler::AnnotMarkup>() {
        markup.set_label(Some(author));
    }
}

/// Where an annotation is, from the page's top left.
pub fn place_of(page: &poppler::Page, annot: &poppler::Annot) -> Place {
    let mut rect = ffi::PopplerRectangle { x1: 0.0, y1: 0.0, x2: 0.0, y2: 0.0 };
    unsafe { ffi::poppler_annot_get_rectangle(annot_ptr(annot), &mut rect) };
    let height = page.size().1;
    (rect.x1, height - rect.y2, rect.x2, height - rect.y1)
}

/// The strokes of an ink annotation, from the page's top left.
pub fn ink_strokes(page: &poppler::Page, annot: &poppler::Annot) -> Vec<Vec<(f64, f64)>> {
    let height = page.size().1;
    let mut strokes = Vec::new();
    let Some(functions) = ink_functions() else {
        return strokes;
    };
    unsafe {
        let mut count = 0;
        let paths = (functions.get_ink_list)(annot_ptr(annot), &mut count);
        if paths.is_null() {
            return strokes;
        }
        for index in 0..count {
            let path = *paths.add(index);
            let mut n = 0;
            let points = (functions.path_get_points)(path, &mut n);
            strokes.push((0..n).map(|i| ((*points.add(i)).x, height - (*points.add(i)).y)).collect());
            (functions.path_free)(path);
        }
        glib::ffi::g_free(paths.cast());
    }
    strokes
}

/// Gives an annotation the box `place`, as dragging its corner does.
pub fn set_place(page: &poppler::Page, annot: &poppler::Annot, place: Place) {
    let mut rect = up(place, page.size().1);
    unsafe { ffi::poppler_annot_set_rectangle(annot_ptr(annot), &mut rect) };
}

/// Moves an annotation by `dx`, `dy` points. A line or a sketch is made again where it goes,
/// since its points are its own; the new one is handed back in its place.
pub fn shift(document: &poppler::Document, page: &poppler::Page, annotation: &Annotation, dx: f64, dy: f64) -> Option<poppler::Annot> {
    let (x1, y1, x2, y2) = place_of(page, &annotation.annot);
    let moved = (x1 + dx, y1 + dy, x2 + dx, y2 + dy);
    let stroke = annotation.color.unwrap_or((0.0, 0.0, 0.0));
    let width = border_width(&annotation.annot);
    match annotation.kind {
        Kind::Ink => {
            let strokes: Vec<Vec<(f64, f64)>> = ink_strokes(page, &annotation.annot)
                .into_iter()
                .map(|stroke| stroke.into_iter().map(|(x, y)| (x + dx, y + dy)).collect())
                .collect();
            let made = ink(document, page, &strokes, stroke, width)?;
            remove(page, &annotation.annot);
            Some(made)
        }
        Kind::Line => {
            // A line's ends are the corners of its box, the padding taken off.
            let pad = width.max(1.0) * 2.0;
            let (ax, ay, bx, by) = (x1 + pad, y1 + pad, x2 - pad, y2 - pad);
            let made = line(document, page, (ax + dx, ay + dy), (bx + dx, by + dy), stroke, width)?;
            remove(page, &annotation.annot);
            Some(made)
        }
        _ => {
            let mut rect = up(moved, page.size().1);
            unsafe { ffi::poppler_annot_set_rectangle(annot_ptr(&annotation.annot), &mut rect) };
            Some(annotation.annot.clone())
        }
    }
}
