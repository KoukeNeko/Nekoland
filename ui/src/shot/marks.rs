//! What is put on a screenshot: arrows, shapes, a pen's strokes, highlights, a spotlight,
//! text, numbered steps, a magnifier, censored parts, measurements and pasted pictures. Each
//! stays a mark of its own, chosen, moved and recoloured until the picture leaves 截圖, and is
//! drawn here over the picture, in its pixels.

use std::{cell::RefCell, collections::HashMap, f64::consts::TAU, rc::Rc};

use gtk::{cairo, pango};
use serde::{Deserialize, Serialize};

use super::img::{Area, Img};

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug)]
pub enum Kind {
    Arrow,
    Rect,
    Ellipse,
    Line,
    Pen,
    Highlight,
    Spotlight,
    Text,
    Step,
    Magnifier,
    Censor,
    Measure,
    Picture,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug)]
pub enum ArrowStyle {
    Straight,
    Thin,
    Curved,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug)]
pub enum Censor {
    Pixelate,
    Blur,
    Erase,
}

/// How a mark is drawn. The lengths are in the picture's pixels.
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Style {
    pub color: [f64; 4],
    pub width: f64,
    pub fill: bool,
    pub shadow: bool,
    pub sketchy: bool,
    pub arrow: ArrowStyle,
    pub censor: Censor,
    pub text_only: bool,
    /// A text's height, a step's and a magnifier's radius.
    pub size: f64,
    /// How dark a spotlight leaves the rest, 0 to 1.
    pub dim: f64,
    /// A censored part's square or blur, in points.
    pub strength: f64,
    /// How much a magnifier enlarges.
    pub zoom: f64,
    /// A pasted picture's.
    pub opacity: f64,
}

impl Style {
    /// The style of a picture taken at `scale` pixels a point.
    pub fn new(scale: f64, color: [f64; 4]) -> Style {
        Style {
            color,
            width: 3.0 * scale,
            fill: false,
            shadow: true,
            sketchy: false,
            arrow: ArrowStyle::Straight,
            censor: Censor::Pixelate,
            text_only: false,
            size: 18.0 * scale,
            dim: 0.55,
            strength: 8.0,
            zoom: 2.5,
            opacity: 1.0,
        }
    }
}

pub type Point = (f64, f64);

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Mark {
    pub kind: Kind,
    /// Two corners or ends for most; a pen's whole stroke; one point for a text, a step and a
    /// magnifier; an arrow's third point is where it bends through.
    pub pts: Vec<Point>,
    pub style: Style,
    /// A text's words, or the name of a pasted picture.
    pub text: String,
    /// What a censored part looks like, kept until it moves or the picture changes.
    #[serde(skip)]
    cache: RefCell<Option<(String, cairo::ImageSurface)>>,
}

impl Mark {
    pub fn new(kind: Kind, pts: Vec<Point>, style: Style) -> Mark {
        Mark {
            kind,
            pts,
            style,
            text: String::new(),
            cache: RefCell::new(None),
        }
    }

    pub fn area(&self) -> Area {
        let a = self.pts.first().copied().unwrap_or_default();
        let b = self.pts.get(1).copied().unwrap_or(a);
        Area::between(a, b)
    }

    /// Whether the mark is drawn from two points, which can each be dragged.
    pub fn has_ends(&self) -> bool {
        !matches!(self.kind, Kind::Pen | Kind::Text | Kind::Step | Kind::Magnifier)
    }

    /// Moves every point of it by `f`, as a crop, a turn or a resize moves the picture; a
    /// resize also scales its lengths by `scale`.
    pub fn map(&mut self, f: &dyn Fn(Point) -> Point, scale: f64) {
        for p in &mut self.pts {
            *p = f(*p);
        }
        if scale != 1.0 {
            self.style.width *= scale;
            self.style.size *= scale;
        }
        self.cache.replace(None);
    }

    pub fn shift(&mut self, dx: f64, dy: f64) {
        self.map(&|(x, y)| (x + dx, y + dy), 1.0);
    }

    /// The rectangle it covers, for choosing it and its handles.
    pub fn bounds(&self) -> (f64, f64, f64, f64) {
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for &(x, y) in &self.pts {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
        match self.kind {
            Kind::Text => {
                let (w, h) = text_size(&self.text, self.style.size);
                (x0, y0, w, h)
            }
            Kind::Step | Kind::Magnifier => {
                let r = self.style.size;
                (x0 - r, y0 - r, r * 2.0, r * 2.0)
            }
            _ => {
                let pad = self.style.width;
                (x0 - pad, y0 - pad, x1 - x0 + pad * 2.0, y1 - y0 + pad * 2.0)
            }
        }
    }

    /// Whether a point, in the picture's pixels, is on it; `slack` widens the lines.
    pub fn hit(&self, p: Point, slack: f64) -> bool {
        match self.kind {
            Kind::Arrow | Kind::Line | Kind::Measure => {
                let path = arrow_points(self);
                path.windows(2)
                    .any(|w| segment_distance(p, w[0], w[1]) <= self.style.width + slack)
            }
            Kind::Pen => self
                .pts
                .windows(2)
                .any(|w| segment_distance(p, w[0], w[1]) <= self.style.width + slack),
            Kind::Rect | Kind::Ellipse if !self.style.fill => {
                let (x, y, w, h) = self.bounds();
                let inside =
                    p.0 >= x - slack && p.0 <= x + w + slack && p.1 >= y - slack && p.1 <= y + h + slack;
                let deep = p.0 > x + self.style.width * 2.0 + slack
                    && p.0 < x + w - self.style.width * 2.0 - slack
                    && p.1 > y + self.style.width * 2.0 + slack
                    && p.1 < y + h - self.style.width * 2.0 - slack;
                inside && !deep
            }
            _ => {
                let (x, y, w, h) = self.bounds();
                p.0 >= x - slack && p.0 <= x + w + slack && p.1 >= y - slack && p.1 <= y + h + slack
            }
        }
    }
}

/// A gradient a backdrop can be.
pub const GRADIENTS: [([f64; 3], [f64; 3]); 8] = [
    ([0.50, 0.35, 0.94], [0.21, 0.52, 0.89]),
    ([0.96, 0.65, 0.14], [0.90, 0.28, 0.30]),
    ([0.19, 0.64, 0.42], [0.21, 0.52, 0.89]),
    ([0.14, 0.12, 0.19], [0.37, 0.36, 0.39]),
    ([1.00, 0.85, 0.24], [0.96, 0.65, 0.14]),
    ([0.75, 0.75, 0.74], [0.98, 0.98, 0.98]),
    ([0.90, 0.28, 0.30], [0.56, 0.31, 0.78]),
    ([1.00, 1.00, 1.00], [1.00, 1.00, 1.00]),
];

/// The picture put on a gradient with room around it, rounded corners and a shadow.
#[derive(Clone, Copy, Serialize, Deserialize, Debug, PartialEq)]
pub struct Backdrop {
    pub gradient: usize,
    /// Each in points.
    pub padding: f64,
    pub radius: f64,
    pub shadow: f64,
}

impl Default for Backdrop {
    fn default() -> Self {
        Backdrop {
            gradient: 0,
            padding: 48.0,
            radius: 10.0,
            shadow: 24.0,
        }
    }
}

/// A screenshot and what is on it.
#[derive(Clone)]
pub struct Doc {
    pub base: Rc<Img>,
    pub marks: Vec<Mark>,
    pub backdrop: Option<Backdrop>,
    /// Pixels a point: 2 for a picture of a display drawn at twice its size.
    pub scale: f64,
    /// The first number the steps count from.
    pub step_start: i32,
    pub pictures: HashMap<String, Rc<Img>>,
}

impl Doc {
    pub fn new(base: Img, scale: f64) -> Doc {
        Doc {
            base: Rc::new(base),
            marks: Vec::new(),
            backdrop: None,
            scale,
            step_start: 1,
            pictures: HashMap::new(),
        }
    }

    /// How far in from the picture's edge to the edge of what is drawn: the backdrop's room.
    pub fn margin(&self) -> f64 {
        self.backdrop.map_or(0.0, |b| b.padding * self.scale)
    }

    /// The size of what is drawn, the backdrop and all.
    pub fn canvas_size(&self) -> (i32, i32) {
        let m = self.margin().round() as i32;
        (self.base.w + m * 2, self.base.h + m * 2)
    }

    /// Everything drawn into a picture of its own, ready to copy or save; `without_pictures`
    /// leaves the pasted pictures off, for the frame before a before-and-after.
    pub fn flatten(&self, without_pictures: bool) -> Img {
        let (w, h) = self.canvas_size();
        let surface = cairo::ImageSurface::create(cairo::Format::ARgb32, w, h).expect("a surface");
        {
            let cr = cairo::Context::new(&surface).expect("a context");
            let base = self.base.surface();
            draw(&cr, self, &base, without_pictures);
        }
        Img::from_surface(surface)
    }
}

/// Draws the picture and its marks onto a context in the picture's pixels, the backdrop's
/// room around them.
pub fn draw(cr: &cairo::Context, doc: &Doc, base: &cairo::ImageSurface, without_pictures: bool) {
    let (w, h) = (doc.base.w as f64, doc.base.h as f64);
    cr.save().ok();
    if let Some(backdrop) = doc.backdrop {
        let m = doc.margin();
        let (a, b) = GRADIENTS[backdrop.gradient.min(GRADIENTS.len() - 1)];
        let gradient = cairo::LinearGradient::new(0.0, 0.0, w + m * 2.0, h + m * 2.0);
        gradient.add_color_stop_rgb(0.0, a[0], a[1], a[2]);
        gradient.add_color_stop_rgb(1.0, b[0], b[1], b[2]);
        let _ = cr.set_source(&gradient);
        let _ = cr.paint();
        cr.translate(m, m);
        let r = backdrop.radius * doc.scale;
        // A soft shadow from rings of falling darkness.
        let spread = backdrop.shadow * doc.scale;
        let steps = 12;
        for i in (1..=steps).rev() {
            let grow = spread * i as f64 / steps as f64;
            rounded(
                cr,
                -grow / 2.0,
                -grow / 4.0 + spread * 0.25,
                w + grow,
                h + grow,
                r + grow / 2.0,
            );
            cr.set_source_rgba(0.0, 0.0, 0.0, 0.5 / steps as f64);
            let _ = cr.fill();
        }
        rounded(cr, 0.0, 0.0, w, h, r);
        cr.clip();
    }
    let _ = cr.set_source_surface(base, 0.0, 0.0);
    let _ = cr.paint();

    let mut step = doc.step_start;
    let mut spotlight_done = false;
    for mark in &doc.marks {
        match mark.kind {
            Kind::Picture if without_pictures => {}
            Kind::Spotlight if spotlight_done => {}
            Kind::Spotlight => {
                spotlight_done = true;
                draw_spotlights(cr, doc, w, h);
            }
            Kind::Step => {
                draw_step(cr, mark, step);
                step += 1;
            }
            _ => draw_mark(cr, doc, mark),
        }
    }
    cr.restore().ok();
}

fn draw_spotlights(cr: &cairo::Context, doc: &Doc, w: f64, h: f64) {
    let lights: Vec<&Mark> = doc.marks.iter().filter(|m| m.kind == Kind::Spotlight).collect();
    cr.save().ok();
    cr.set_fill_rule(cairo::FillRule::EvenOdd);
    cr.rectangle(0.0, 0.0, w, h);
    for light in &lights {
        let a = light.area();
        let (x, y, aw, ah) = (a.x as f64, a.y as f64, a.w as f64, a.h as f64);
        if light.style.fill {
            cr.new_sub_path();
            ellipse(cr, x + aw / 2.0, y + ah / 2.0, aw / 2.0, ah / 2.0);
        } else {
            rounded(cr, x, y, aw, ah, 6.0 * doc.scale);
        }
    }
    cr.set_source_rgba(0.0, 0.0, 0.0, lights[0].style.dim);
    let _ = cr.fill();
    cr.restore().ok();
}

fn draw_step(cr: &cairo::Context, mark: &Mark, number: i32) {
    let (x, y) = mark.pts[0];
    let r = mark.style.size;
    let c = mark.style.color;
    cr.save().ok();
    if mark.style.shadow {
        cr.arc(x + r * 0.08, y + r * 0.14, r, 0.0, TAU);
        cr.set_source_rgba(0.0, 0.0, 0.0, 0.3);
        let _ = cr.fill();
    }
    cr.arc(x, y, r, 0.0, TAU);
    cr.set_source_rgba(c[0], c[1], c[2], c[3]);
    let _ = cr.fill_preserve();
    cr.set_source_rgb(1.0, 1.0, 1.0);
    cr.set_line_width(r * 0.12);
    let _ = cr.stroke();
    let layout = pangocairo::functions::create_layout(cr);
    let mut font = pango::FontDescription::from_string("Sans Bold");
    font.set_absolute_size(r * 1.1 * pango::SCALE as f64);
    layout.set_font_description(Some(&font));
    layout.set_text(&number.to_string());
    let (_, logical) = layout.pixel_extents();
    cr.move_to(
        x - logical.width() as f64 / 2.0,
        y - logical.height() as f64 / 2.0,
    );
    cr.set_source_rgb(1.0, 1.0, 1.0);
    pangocairo::functions::show_layout(cr, &layout);
    cr.restore().ok();
}

/// A text's size when drawn at `size` pixels high.
pub fn text_size(text: &str, size: f64) -> (f64, f64) {
    let surface = cairo::ImageSurface::create(cairo::Format::ARgb32, 1, 1).expect("a surface");
    let cr = cairo::Context::new(&surface).expect("a context");
    let layout = text_layout(&cr, text, size);
    let (_, logical) = layout.pixel_extents();
    let pad = size * 0.35;
    (
        logical.width() as f64 + pad * 2.0,
        logical.height() as f64 + pad * 2.0,
    )
}

fn text_layout(cr: &cairo::Context, text: &str, size: f64) -> pango::Layout {
    let layout = pangocairo::functions::create_layout(cr);
    let mut font = pango::FontDescription::from_string("Sans Bold");
    font.set_absolute_size(size * pango::SCALE as f64);
    layout.set_font_description(Some(&font));
    layout.set_text(if text.is_empty() { " " } else { text });
    layout
}

fn draw_text(cr: &cairo::Context, mark: &Mark) {
    let (x, y) = mark.pts[0];
    let size = mark.style.size;
    let c = mark.style.color;
    let pad = size * 0.35;
    let layout = text_layout(cr, &mark.text, size);
    let (w, h) = text_size(&mark.text, size);
    cr.save().ok();
    if mark.style.fill {
        // On a pill of its colour, in white.
        if mark.style.shadow {
            rounded(cr, x + size * 0.06, y + size * 0.12, w, h, size * 0.3);
            cr.set_source_rgba(0.0, 0.0, 0.0, 0.25);
            let _ = cr.fill();
        }
        rounded(cr, x, y, w, h, size * 0.3);
        cr.set_source_rgba(c[0], c[1], c[2], c[3]);
        let _ = cr.fill();
        cr.move_to(x + pad, y + pad);
        cr.set_source_rgb(1.0, 1.0, 1.0);
        pangocairo::functions::show_layout(cr, &layout);
    } else {
        // In its colour with a white edge, which reads over any picture.
        cr.move_to(x + pad, y + pad);
        pangocairo::functions::layout_path(cr, &layout);
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.95);
        cr.set_line_width(size * 0.18);
        cr.set_line_join(cairo::LineJoin::Round);
        let _ = cr.stroke_preserve();
        cr.set_source_rgba(c[0], c[1], c[2], c[3]);
        let _ = cr.fill();
    }
    cr.restore().ok();
}

/// The points an arrow, a line or a measurement runs through: straight, or curved through
/// its bend.
pub fn arrow_points(mark: &Mark) -> Vec<Point> {
    let a = mark.pts[0];
    let b = mark.pts.get(1).copied().unwrap_or(a);
    let bend = mark.pts.get(2).copied().or_else(|| {
        (mark.kind == Kind::Arrow && mark.style.arrow == ArrowStyle::Curved).then(|| {
            let (dx, dy) = (b.0 - a.0, b.1 - a.1);
            ((a.0 + b.0) / 2.0 - dy * 0.2, (a.1 + b.1) / 2.0 + dx * 0.2)
        })
    });
    match bend {
        // A quadratic curve through the bend at its middle.
        Some(m) => {
            let c = (2.0 * m.0 - (a.0 + b.0) / 2.0, 2.0 * m.1 - (a.1 + b.1) / 2.0);
            (0..=24)
                .map(|i| {
                    let t = i as f64 / 24.0;
                    let u = 1.0 - t;
                    (
                        u * u * a.0 + 2.0 * u * t * c.0 + t * t * b.0,
                        u * u * a.1 + 2.0 * u * t * c.1 + t * t * b.1,
                    )
                })
                .collect()
        }
        None => vec![a, b],
    }
}

/// A line made to look drawn by hand: it wavers a little, the same way each time.
fn sketched(points: &[Point], amount: f64) -> Vec<Point> {
    let mut out = Vec::new();
    for w in points.windows(2) {
        let (a, b) = (w[0], w[1]);
        let len = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
        let n = (len / 12.0).ceil().max(1.0) as usize;
        for i in 0..n {
            let t = i as f64 / n as f64;
            let (x, y) = (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
            let seed = (x * 12.9898 + y * 78.233).sin() * 43758.5453;
            let jitter = (seed - seed.floor() - 0.5) * amount;
            out.push((x + jitter, y - jitter));
        }
    }
    if let Some(last) = points.last() {
        out.push(*last);
    }
    out
}

fn trace(cr: &cairo::Context, points: &[Point]) {
    if let Some(first) = points.first() {
        cr.move_to(first.0, first.1);
        for p in &points[1..] {
            cr.line_to(p.0, p.1);
        }
    }
}

/// The outline of a rectangle or an ellipse as points, which the hand-drawn look wavers.
fn outline(kind: Kind, area: Area) -> Vec<Point> {
    let (x, y, w, h) = (area.x as f64, area.y as f64, area.w as f64, area.h as f64);
    match kind {
        Kind::Ellipse => (0..=64)
            .map(|i| {
                let t = i as f64 / 64.0 * TAU;
                (x + w / 2.0 + w / 2.0 * t.cos(), y + h / 2.0 + h / 2.0 * t.sin())
            })
            .collect(),
        _ => vec![(x, y), (x + w, y), (x + w, y + h), (x, y + h), (x, y)],
    }
}

fn set_colour(cr: &cairo::Context, c: [f64; 4]) {
    cr.set_source_rgba(c[0], c[1], c[2], c[3]);
}

/// Strokes a line of points in a mark's style: its shadow first, then once or, drawn by
/// hand, twice.
fn stroke_points(cr: &cairo::Context, mark: &Mark, points: &[Point], width: f64) {
    cr.set_line_cap(cairo::LineCap::Round);
    cr.set_line_join(cairo::LineJoin::Round);
    cr.set_line_width(width);
    let passes: Vec<Vec<Point>> = if mark.style.sketchy {
        vec![
            sketched(points, width * 0.9),
            sketched(
                &points.iter().map(|p| (p.0 + 0.7, p.1 + 0.4)).collect::<Vec<_>>(),
                width * 0.9,
            ),
        ]
    } else {
        vec![points.to_vec()]
    };
    if mark.style.shadow {
        let off = width * 0.35 + 1.0;
        for pass in &passes {
            trace(
                cr,
                &pass
                    .iter()
                    .map(|p| (p.0 + off * 0.5, p.1 + off))
                    .collect::<Vec<_>>(),
            );
        }
        cr.set_source_rgba(0.0, 0.0, 0.0, 0.28);
        let _ = cr.stroke();
    }
    for pass in &passes {
        trace(cr, pass);
    }
    set_colour(cr, mark.style.color);
    let _ = cr.stroke();
}

fn draw_arrow(cr: &cairo::Context, mark: &Mark) {
    let points = arrow_points(mark);
    let thin = mark.style.arrow == ArrowStyle::Thin;
    let width = if thin {
        (mark.style.width * 0.5).max(1.5)
    } else {
        mark.style.width
    };
    let n = points.len();
    let (tip, before) = (points[n - 1], points[n.saturating_sub(3).min(n - 2)]);
    let angle = (tip.1 - before.1).atan2(tip.0 - before.0);
    let head = (mark.style.width * 4.0).max(12.0);
    let wing = |turn: f64| {
        (
            tip.0 - head * (angle + turn).cos(),
            tip.1 - head * (angle + turn).sin(),
        )
    };
    let (left, right) = (wing(0.45), wing(-0.45));
    if thin {
        stroke_points(cr, mark, &points, width);
        stroke_points(cr, mark, &[left, tip, right], width);
        return;
    }
    // The shaft stops short of the tip, under the head.
    let back = head * 0.8;
    let mut shaft = points.clone();
    shaft[n - 1] = (tip.0 - back * angle.cos(), tip.1 - back * angle.sin());
    stroke_points(cr, mark, &shaft, width);
    let head_path = [
        tip,
        left,
        (
            tip.0 - head * 0.75 * angle.cos(),
            tip.1 - head * 0.75 * angle.sin(),
        ),
        right,
    ];
    if mark.style.shadow {
        let off = width * 0.35 + 1.0;
        trace(
            cr,
            &head_path
                .iter()
                .map(|p| (p.0 + off * 0.5, p.1 + off))
                .collect::<Vec<_>>(),
        );
        cr.close_path();
        cr.set_source_rgba(0.0, 0.0, 0.0, 0.28);
        let _ = cr.fill();
    }
    trace(cr, &head_path);
    cr.close_path();
    set_colour(cr, mark.style.color);
    let _ = cr.fill();
}

fn draw_measure(cr: &cairo::Context, doc: &Doc, mark: &Mark) {
    let a = mark.pts[0];
    let b = mark.pts.get(1).copied().unwrap_or(a);
    let len = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
    let angle = (b.1 - a.1).atan2(b.0 - a.0) + TAU / 4.0;
    let tick = (mark.style.width * 2.5).max(6.0);
    let (tx, ty) = (tick * angle.cos(), tick * angle.sin());
    let width = (mark.style.width * 0.5).max(1.5);
    let plain = Mark {
        style: Style {
            shadow: false,
            sketchy: false,
            ..mark.style.clone()
        },
        ..mark.clone()
    };
    stroke_points(cr, &plain, &[a, b], width);
    stroke_points(cr, &plain, &[(a.0 - tx, a.1 - ty), (a.0 + tx, a.1 + ty)], width);
    stroke_points(cr, &plain, &[(b.0 - tx, b.1 - ty), (b.0 + tx, b.1 + ty)], width);
    let label = format!("{} px", (len / doc.scale).round());
    chip(
        cr,
        &label,
        ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0),
        12.0 * doc.scale,
    );
}

/// A dark rounded label centred on a point, as sizes are shown on the picture.
pub fn chip(cr: &cairo::Context, label: &str, at: Point, size: f64) {
    let layout = pangocairo::functions::create_layout(cr);
    let mut font = pango::FontDescription::from_string("Sans Bold");
    font.set_absolute_size(size * pango::SCALE as f64);
    layout.set_font_description(Some(&font));
    layout.set_text(label);
    let (_, logical) = layout.pixel_extents();
    let (w, h) = (
        logical.width() as f64 + size,
        logical.height() as f64 + size * 0.5,
    );
    cr.save().ok();
    rounded(cr, at.0 - w / 2.0, at.1 - h / 2.0, w, h, h * 0.3);
    cr.set_source_rgba(0.14, 0.12, 0.19, 0.85);
    let _ = cr.fill();
    cr.move_to(
        at.0 - logical.width() as f64 / 2.0,
        at.1 - logical.height() as f64 / 2.0,
    );
    cr.set_source_rgb(1.0, 1.0, 1.0);
    pangocairo::functions::show_layout(cr, &layout);
    cr.restore().ok();
}

fn draw_magnifier(cr: &cairo::Context, doc: &Doc, mark: &Mark) {
    let (x, y) = mark.pts[0];
    let r = mark.style.size;
    cr.save().ok();
    if mark.style.shadow {
        cr.arc(x + r * 0.04, y + r * 0.08, r, 0.0, TAU);
        cr.set_source_rgba(0.0, 0.0, 0.0, 0.3);
        let _ = cr.fill();
    }
    cr.arc(x, y, r, 0.0, TAU);
    cr.clip();
    cr.translate(x, y);
    cr.scale(mark.style.zoom, mark.style.zoom);
    cr.translate(-x, -y);
    let _ = cr.set_source_surface(doc.base.surface(), 0.0, 0.0);
    cr.source().set_filter(cairo::Filter::Nearest);
    let _ = cr.paint();
    cr.restore().ok();
    cr.save().ok();
    cr.arc(x, y, r, 0.0, TAU);
    set_colour(cr, mark.style.color);
    cr.set_line_width(mark.style.width.max(2.0));
    let _ = cr.stroke();
    cr.restore().ok();
}

fn draw_censor(cr: &cairo::Context, doc: &Doc, mark: &Mark) {
    let area = mark.area().clamp(doc.base.w, doc.base.h);
    if area.is_empty() {
        return;
    }
    let key = format!(
        "{:?} {:?} {} {} {:p}",
        area,
        mark.style.censor,
        mark.style.text_only,
        mark.style.strength,
        Rc::as_ptr(&doc.base)
    );
    let mut cache = mark.cache.borrow_mut();
    if cache.as_ref().is_none_or(|(k, _)| *k != key) {
        let strength = (mark.style.strength * doc.scale).round().max(2.0) as i32;
        let patch = match mark.style.censor {
            Censor::Pixelate => doc.base.pixelate(area, strength, mark.style.text_only),
            Censor::Blur => doc.base.blur(area, (strength / 2).max(1), mark.style.text_only),
            Censor::Erase => doc.base.erase(area, mark.style.text_only),
        };
        *cache = Some((key, patch.surface()));
    }
    if let Some((_, surface)) = cache.as_ref() {
        let _ = cr.set_source_surface(surface, area.x as f64, area.y as f64);
        let _ = cr.paint();
    }
}

fn draw_mark(cr: &cairo::Context, doc: &Doc, mark: &Mark) {
    cr.save().ok();
    match mark.kind {
        Kind::Arrow => draw_arrow(cr, mark),
        Kind::Line => stroke_points(cr, mark, &arrow_points(mark), mark.style.width),
        Kind::Pen => stroke_points(cr, mark, &mark.pts, mark.style.width),
        Kind::Rect | Kind::Ellipse => {
            let area = mark.area();
            if mark.style.fill {
                let c = mark.style.color;
                if mark.kind == Kind::Ellipse {
                    ellipse(
                        cr,
                        area.x as f64 + area.w as f64 / 2.0,
                        area.y as f64 + area.h as f64 / 2.0,
                        area.w as f64 / 2.0,
                        area.h as f64 / 2.0,
                    );
                } else {
                    rounded(
                        cr,
                        area.x as f64,
                        area.y as f64,
                        area.w as f64,
                        area.h as f64,
                        mark.style.width,
                    );
                }
                cr.set_source_rgba(c[0], c[1], c[2], c[3] * 0.25);
                let _ = cr.fill();
            }
            if mark.kind == Kind::Rect && !mark.style.sketchy {
                let w = mark.style.width;
                if mark.style.shadow {
                    let off = w * 0.35 + 1.0;
                    rounded(
                        cr,
                        area.x as f64 + off * 0.5,
                        area.y as f64 + off,
                        area.w as f64,
                        area.h as f64,
                        w,
                    );
                    cr.set_source_rgba(0.0, 0.0, 0.0, 0.28);
                    cr.set_line_width(w);
                    let _ = cr.stroke();
                }
                rounded(cr, area.x as f64, area.y as f64, area.w as f64, area.h as f64, w);
                set_colour(cr, mark.style.color);
                cr.set_line_width(w);
                let _ = cr.stroke();
            } else {
                stroke_points(cr, mark, &outline(mark.kind, area), mark.style.width);
            }
        }
        Kind::Highlight => {
            let a = mark.area();
            cr.set_operator(cairo::Operator::Multiply);
            rounded(
                cr,
                a.x as f64,
                a.y as f64,
                a.w as f64,
                a.h as f64,
                3.0 * doc.scale,
            );
            set_colour(cr, mark.style.color);
            let _ = cr.fill();
        }
        Kind::Text => draw_text(cr, mark),
        Kind::Magnifier => draw_magnifier(cr, doc, mark),
        Kind::Censor => draw_censor(cr, doc, mark),
        Kind::Measure => draw_measure(cr, doc, mark),
        Kind::Picture => {
            if let Some(picture) = doc.pictures.get(&mark.text) {
                let a = mark.area();
                if a.w > 0 && a.h > 0 {
                    cr.translate(a.x as f64, a.y as f64);
                    cr.scale(a.w as f64 / picture.w as f64, a.h as f64 / picture.h as f64);
                    let _ = cr.set_source_surface(picture.surface(), 0.0, 0.0);
                    let _ = cr.paint_with_alpha(mark.style.opacity);
                }
            }
        }
        Kind::Spotlight | Kind::Step => {}
    }
    cr.restore().ok();
}

pub fn rounded(cr: &cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -TAU / 4.0, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, TAU / 4.0);
    cr.arc(x + r, y + h - r, r, TAU / 4.0, TAU / 2.0);
    cr.arc(x + r, y + r, r, TAU / 2.0, TAU * 0.75);
    cr.close_path();
}

fn ellipse(cr: &cairo::Context, cx: f64, cy: f64, rx: f64, ry: f64) {
    if rx <= 0.0 || ry <= 0.0 {
        return;
    }
    cr.save().ok();
    cr.translate(cx, cy);
    cr.scale(rx, ry);
    cr.arc(0.0, 0.0, 1.0, 0.0, TAU);
    cr.restore().ok();
}

pub fn segment_distance(p: Point, a: Point, b: Point) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len = dx * dx + dy * dy;
    let t = if len == 0.0 {
        0.0
    } else {
        (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len).clamp(0.0, 1.0)
    };
    let (x, y) = (a.0 + dx * t, a.1 + dy * t);
    ((p.0 - x).powi(2) + (p.1 - y).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_is_hit_near_it_and_not_away_from_it() {
        let mark = Mark::new(
            Kind::Line,
            vec![(0.0, 0.0), (100.0, 0.0)],
            Style::new(1.0, [1.0, 0.0, 0.0, 1.0]),
        );
        assert!(mark.hit((50.0, 4.0), 2.0));
        assert!(!mark.hit((50.0, 30.0), 2.0));
    }

    #[test]
    fn a_hollow_rectangle_is_chosen_by_its_edge() {
        let mark = Mark::new(
            Kind::Rect,
            vec![(0.0, 0.0), (100.0, 100.0)],
            Style::new(1.0, [1.0, 0.0, 0.0, 1.0]),
        );
        assert!(mark.hit((1.0, 50.0), 2.0));
        assert!(!mark.hit((50.0, 50.0), 2.0));
    }

    #[test]
    fn steps_count_from_where_they_start() {
        let mut doc = Doc::new(Img::new(40, 40, [255, 255, 255, 255]), 1.0);
        doc.step_start = 3;
        for x in [10.0, 30.0] {
            doc.marks.push(Mark::new(
                Kind::Step,
                vec![(x, 20.0)],
                Style::new(1.0, [1.0, 0.0, 0.0, 1.0]),
            ));
        }
        let flat = doc.flatten(false);
        assert_eq!((flat.w, flat.h), (40, 40));
        assert_ne!(flat.rgba(10, 20), [255, 255, 255, 255]);
    }
}
