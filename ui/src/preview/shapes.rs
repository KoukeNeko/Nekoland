//! The marks 標記 puts on a picture or a PDF: what each tool and shape is, the style it is drawn
//! in, and the shapes as lines through points, which a picture is painted with and a PDF keeps
//! as annotations.

use gtk::gdk_pixbuf::Pixbuf;
use gtk::{gdk, pango};

/// The tools of the markup bar.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    /// 文字選取: a PDF's text, chosen by dragging over it.
    Text,
    /// 矩形選取.
    Rectangle,
    /// 橢圓形選取, a picture's.
    Ellipse,
    /// 套索選取, a picture's.
    Lasso,
    /// 智慧型套索: a lasso that keeps to the edges near where it is drawn.
    SmartLasso,
    /// 即時 Alpha.
    InstantAlpha,
    /// 塗黑, a PDF's.
    Redact,
    /// 草圖.
    Sketch,
    /// Whichever shape is chosen.
    Shape,
    /// 文字: a box of text.
    TextBox,
    /// 附註, a PDF's.
    Note,
    /// The signature chosen.
    Signature,
    /// 表單填寫: text put anywhere on a page that has no fields to fill.
    FormText,
}

/// The tools by the target the `tool` action takes.
pub const TOOL_KEYS: [(Tool, &str); 13] = [
    (Tool::Text, "text"),
    (Tool::Rectangle, "rectangle"),
    (Tool::Ellipse, "ellipse"),
    (Tool::Lasso, "lasso"),
    (Tool::SmartLasso, "smart-lasso"),
    (Tool::InstantAlpha, "instant-alpha"),
    (Tool::Redact, "redact"),
    (Tool::Sketch, "sketch"),
    (Tool::Shape, "shape"),
    (Tool::TextBox, "textbox"),
    (Tool::Note, "note"),
    (Tool::Signature, "signature"),
    (Tool::FormText, "form"),
];

impl Tool {
    pub fn key(self) -> &'static str {
        TOOL_KEYS.iter().find(|(tool, _)| *tool == self).map_or("", |(_, key)| key)
    }

    pub fn from_key(key: &str) -> Option<Tool> {
        TOOL_KEYS.iter().find(|(_, name)| *name == key).map(|(tool, _)| *tool)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shape {
    Line,
    Arrow,
    Rectangle,
    RoundedRectangle,
    Ellipse,
    Bubble,
    Star,
    Polygon,
    Highlight,
    Loupe,
    Mask,
}

/// The shapes as the 形狀 menu lists them, with their names.
pub const SHAPES: [(Shape, &str); 11] = [
    (Shape::Line, "線條"),
    (Shape::Arrow, "箭頭"),
    (Shape::Rectangle, "矩形"),
    (Shape::RoundedRectangle, "圓角矩形"),
    (Shape::Ellipse, "橢圓形"),
    (Shape::Bubble, "對話框"),
    (Shape::Star, "星形"),
    (Shape::Polygon, "多邊形"),
    (Shape::Highlight, "標示重點"),
    (Shape::Loupe, "放大鏡"),
    (Shape::Mask, "遮罩"),
];

impl Shape {
    pub fn key(self) -> &'static str {
        match self {
            Shape::Line => "line",
            Shape::Arrow => "arrow",
            Shape::Rectangle => "rectangle",
            Shape::RoundedRectangle => "rounded",
            Shape::Ellipse => "ellipse",
            Shape::Bubble => "bubble",
            Shape::Star => "star",
            Shape::Polygon => "polygon",
            Shape::Highlight => "highlight",
            Shape::Loupe => "loupe",
            Shape::Mask => "mask",
        }
    }

    pub fn from_key(key: &str) -> Option<Shape> {
        SHAPES.iter().map(|(shape, _)| *shape).find(|shape| shape.key() == key)
    }

    /// Whether it is outlined and may be filled, rather than a line.
    pub fn closed(self) -> bool {
        !matches!(self, Shape::Line | Shape::Arrow)
    }
}

/// What a mark is drawn in.
#[derive(Clone)]
pub struct Style {
    pub stroke: gdk::RGBA,
    pub fill: Option<gdk::RGBA>,
    /// The line's width, in points of a PDF or a picture's pixels at its size on screen.
    pub width: f64,
    pub font: pango::FontDescription,
}

/// The line widths 形狀樣式 offers.
pub const WIDTHS: [f64; 6] = [1.0, 2.0, 3.0, 5.0, 8.0, 12.0];

fn ellipse_points(cx: f64, cy: f64, rx: f64, ry: f64, from: f64, to: f64) -> Vec<(f64, f64)> {
    const STEPS: usize = 48;
    (0..=STEPS).map(|step| {
        let angle = from + (to - from) * step as f64 / STEPS as f64;
        (cx + rx * angle.cos(), cy + ry * angle.sin())
    })
    .collect()
}

/// The shape between two corners as lines through points, each line a list of them; a closed
/// shape's first line ends where it began.
pub fn outline(shape: Shape, from: (f64, f64), to: (f64, f64), width: f64) -> Vec<Vec<(f64, f64)>> {
    let (x1, y1, x2, y2) = (from.0.min(to.0), from.1.min(to.1), from.0.max(to.0), from.1.max(to.1));
    let (cx, cy, rx, ry) = ((x1 + x2) / 2.0, (y1 + y2) / 2.0, (x2 - x1) / 2.0, (y2 - y1) / 2.0);
    use std::f64::consts::{FRAC_PI_2, PI, TAU};
    match shape {
        Shape::Line => vec![vec![from, to]],
        Shape::Arrow => {
            let angle = (to.1 - from.1).atan2(to.0 - from.0);
            let head = (width * 4.0).max(10.0);
            let wing = |turn: f64| (to.0 + head * (angle + turn).cos(), to.1 + head * (angle + turn).sin());
            vec![vec![from, to], vec![wing(2.6), to, wing(-2.6)]]
        }
        Shape::Rectangle | Shape::Highlight | Shape::Mask => vec![vec![(x1, y1), (x2, y1), (x2, y2), (x1, y2), (x1, y1)]],
        Shape::RoundedRectangle => {
            let r = (rx.min(ry) * 0.3).min(24.0);
            let mut points = Vec::new();
            for (ccx, ccy, start) in [(x2 - r, y1 + r, -FRAC_PI_2), (x2 - r, y2 - r, 0.0), (x1 + r, y2 - r, FRAC_PI_2), (x1 + r, y1 + r, PI)] {
                points.extend(ellipse_points(ccx, ccy, r, r, start, start + FRAC_PI_2));
            }
            points.push(points[0]);
            vec![points]
        }
        Shape::Ellipse | Shape::Loupe => {
            let mut points = ellipse_points(cx, cy, rx, ry, 0.0, TAU);
            // Closed exactly, not where the sine of a full turn rounds to.
            points.pop();
            points.push(points[0]);
            vec![points]
        }
        Shape::Bubble => {
            // An ellipse over the top four fifths, its tail down to the bottom left.
            let body = y1 + (y2 - y1) * 0.8;
            let (bcy, bry) = ((y1 + body) / 2.0, (body - y1) / 2.0);
            let mut points = ellipse_points(cx, bcy, rx, bry, PI * 0.62, PI * 2.0 + PI * 0.48);
            points.push((x1 + (x2 - x1) * 0.2, y2));
            points.push(points[0]);
            vec![points]
        }
        Shape::Star => {
            let mut points: Vec<(f64, f64)> = (0..10)
                .map(|corner| {
                    let angle = -FRAC_PI_2 + PI * corner as f64 / 5.0;
                    let reach = if corner % 2 == 0 { 1.0 } else { 0.42 };
                    (cx + rx * reach * angle.cos(), cy + ry * reach * angle.sin())
                })
                .collect();
            points.push(points[0]);
            vec![points]
        }
        Shape::Polygon => {
            let mut points: Vec<(f64, f64)> = (0..6)
                .map(|corner| {
                    let angle = -FRAC_PI_2 + TAU * corner as f64 / 6.0;
                    (cx + rx * angle.cos(), cy + ry * angle.sin())
                })
                .collect();
            points.push(points[0]);
            vec![points]
        }
    }
}

pub fn rgb(colour: &gdk::RGBA) -> (f64, f64, f64) {
    (f64::from(colour.red()), f64::from(colour.green()), f64::from(colour.blue()))
}

fn trace(cr: &gtk::cairo::Context, lines: &[Vec<(f64, f64)>]) {
    for line in lines {
        let mut points = line.iter();
        if let Some(&(x, y)) = points.next() {
            cr.move_to(x, y);
            points.for_each(|&(x, y)| cr.line_to(x, y));
        }
    }
}

/// Draws `shape` between two corners with `style`, its line `width` wide, over what `cr`
/// shows; the magnifier enlarges what `under` draws, called with `cr` set up to draw it.
pub fn draw_shape(
    cr: &gtk::cairo::Context,
    shape: Shape,
    from: (f64, f64),
    to: (f64, f64),
    style: &Style,
    width: f64,
    under: &dyn Fn(&gtk::cairo::Context),
) {
    let lines = outline(shape, from, to, width);
    let (red, green, blue) = rgb(&style.stroke);
    cr.set_line_cap(gtk::cairo::LineCap::Round);
    cr.set_line_join(gtk::cairo::LineJoin::Round);
    cr.set_line_width(width);
    match shape {
        Shape::Highlight => {
            trace(cr, &lines);
            cr.set_source_rgba(1.0, 0.85, 0.0, 0.4);
            let _ = cr.fill();
        }
        Shape::Mask => {
            // All but the shape is dimmed.
            let (x1, y1, x2, y2) = cr.clip_extents().unwrap_or((0.0, 0.0, 0.0, 0.0));
            cr.rectangle(x1, y1, x2 - x1, y2 - y1);
            trace(cr, &lines);
            cr.set_fill_rule(gtk::cairo::FillRule::EvenOdd);
            cr.set_source_rgba(0.0, 0.0, 0.0, 0.5);
            let _ = cr.fill();
            cr.set_fill_rule(gtk::cairo::FillRule::Winding);
        }
        Shape::Loupe => {
            let (cx, cy) = ((from.0 + to.0) / 2.0, (from.1 + to.1) / 2.0);
            let _ = cr.save();
            trace(cr, &lines);
            cr.clip();
            cr.translate(cx, cy);
            cr.scale(2.0, 2.0);
            cr.translate(-cx, -cy);
            under(cr);
            let _ = cr.restore();
            trace(cr, &lines);
            cr.set_source_rgb(red, green, blue);
            let _ = cr.stroke();
        }
        _ => {
            if let (Some(fill), true) = (&style.fill, shape.closed()) {
                trace(cr, &lines);
                let (fr, fg, fb) = rgb(fill);
                cr.set_source_rgba(fr, fg, fb, f64::from(fill.alpha()));
                let _ = cr.fill();
            }
            trace(cr, &lines);
            cr.set_source_rgba(red, green, blue, f64::from(style.stroke.alpha()));
            let _ = cr.stroke();
        }
    }
}

/// Draws lines through `points` as a pen does.
pub fn draw_sketch(cr: &gtk::cairo::Context, points: &[(f64, f64)], style: &Style, width: f64) {
    let (red, green, blue) = rgb(&style.stroke);
    cr.set_source_rgba(red, green, blue, f64::from(style.stroke.alpha()));
    cr.set_line_cap(gtk::cairo::LineCap::Round);
    cr.set_line_join(gtk::cairo::LineJoin::Round);
    cr.set_line_width(width);
    trace(cr, &[points.to_vec()]);
    let _ = cr.stroke();
}

/// Draws `text` with its top left at `at`, in the style's font and colour.
pub fn draw_text(cr: &gtk::cairo::Context, at: (f64, f64), text: &str, style: &Style, size: f64) {
    let layout = pangocairo::functions::create_layout(cr);
    let mut font = style.font.clone();
    font.set_absolute_size(size * f64::from(pango::SCALE));
    layout.set_font_description(Some(&font));
    layout.set_text(text);
    let (red, green, blue) = rgb(&style.stroke);
    cr.set_source_rgba(red, green, blue, f64::from(style.stroke.alpha()));
    cr.move_to(at.0, at.1);
    pangocairo::functions::show_layout(cr, &layout);
}

/// How wide a line `style` asks for is in a picture's own pixels: as wide on screen as on a
/// PDF when the picture fits a window, so a large photo gets lines that can be seen.
pub fn picture_width(pixbuf: &Pixbuf, style: &Style) -> f64 {
    style.width * (f64::from(pixbuf.width().max(pixbuf.height())) / 800.0).max(1.0)
}

/// `pixbuf` with a mark painted on, in its own pixels: a sketch through `points`, a shape
/// between the first and last of them, or `text` at the first; `None` when out of memory.
pub fn paint(pixbuf: &Pixbuf, tool: Tool, shape: Shape, style: &Style, points: &[(f64, f64)], text: &str) -> Option<Pixbuf> {
    use gtk::cairo::{Context, Format, ImageSurface};
    use gtk::prelude::GdkCairoContextExt;
    let (width, height) = (pixbuf.width(), pixbuf.height());
    let surface = ImageSurface::create(Format::ARgb32, width, height).ok()?;
    let cr = Context::new(&surface).ok()?;
    cr.set_source_pixbuf(pixbuf, 0.0, 0.0);
    cr.paint().ok()?;
    let line = picture_width(pixbuf, style);
    let (first, last) = (*points.first()?, *points.last()?);
    match tool {
        Tool::Sketch => draw_sketch(&cr, points, style, line),
        Tool::TextBox | Tool::FormText => draw_text(&cr, first, text, style, line * 6.0 + style.font.size() as f64 / f64::from(pango::SCALE)),
        _ => {
            let under = |cr: &Context| {
                cr.set_source_pixbuf(pixbuf, 0.0, 0.0);
                let _ = cr.paint();
            };
            draw_shape(&cr, shape, first, last, style, line, &under);
        }
    }
    drop(cr);
    surface.flush();
    // Deprecated for a texture, which cannot be edited again as a Pixbuf is.
    #[allow(deprecated)]
    let painted = gdk::pixbuf_get_from_surface(&surface, 0, 0, width, height);
    painted
}

/// `pixbuf` with `stamp`, a signature, drawn into the box between two corners.
pub fn paint_picture(pixbuf: &Pixbuf, stamp: &Pixbuf, from: (f64, f64), to: (f64, f64)) -> Option<Pixbuf> {
    use gtk::cairo::{Context, Format, ImageSurface};
    use gtk::prelude::GdkCairoContextExt;
    let (width, height) = (pixbuf.width(), pixbuf.height());
    let surface = ImageSurface::create(Format::ARgb32, width, height).ok()?;
    let cr = Context::new(&surface).ok()?;
    cr.set_source_pixbuf(pixbuf, 0.0, 0.0);
    cr.paint().ok()?;
    let (x1, y1) = (from.0.min(to.0), from.1.min(to.1));
    let box_width = (to.0 - from.0).abs().max(1.0);
    let scale = box_width / f64::from(stamp.width());
    cr.translate(x1, y1);
    cr.scale(scale, scale);
    cr.set_source_pixbuf(stamp, 0.0, 0.0);
    cr.paint().ok()?;
    drop(cr);
    surface.flush();
    #[allow(deprecated)]
    let painted = gdk::pixbuf_get_from_surface(&surface, 0, 0, width, height);
    painted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closed_shapes_end_where_they_begin_and_an_arrow_has_a_head() {
        for (shape, _) in SHAPES {
            let lines = outline(shape, (10.0, 10.0), (110.0, 60.0), 2.0);
            if shape.closed() {
                let line = &lines[0];
                assert_eq!(line.first(), line.last(), "{shape:?}");
            }
        }
        assert_eq!(outline(Shape::Arrow, (0.0, 0.0), (100.0, 0.0), 2.0).len(), 2);
        let star = &outline(Shape::Star, (0.0, 0.0), (100.0, 100.0), 1.0)[0];
        assert_eq!(star.len(), 11);
    }
}
