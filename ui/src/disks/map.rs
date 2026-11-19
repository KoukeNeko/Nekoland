//! The map of a disk: its partitions and free space side by side, as wide as they are large,
//! each with how full it is. One is chosen by pressing it or with the arrow keys.

use std::{cell::{Cell, RefCell}, rc::Rc};

use adw::prelude::*;
use gtk::{glib, pango};

/// How tall the map is, with room around for the ring of the chosen one.
const HEIGHT: i32 = 124;
const PAD: f64 = 5.0;
const GAP: f64 = 3.0;
/// The narrowest a segment is drawn, and the narrowest that has room to say what it is.
const MIN_WIDTH: f64 = 22.0;
const LABELLED: f64 = 60.0;
const RADIUS: f64 = 10.0;

#[derive(Clone, PartialEq)]
pub struct Seg {
    pub name: String,
    pub size: String,
    /// What the segment counts for, in bytes.
    pub weight: f64,
    /// How full, 0 to 1, if that is known.
    pub used: Option<f64>,
    /// Which of the palette's colors, or None for free space.
    pub color: Option<usize>,
    pub tip: String,
}

/// The palette, for light and for dark.
const PALETTE: [[(f64, f64, f64); 6]; 2] = [
    [(0.427, 0.447, 0.482), (0.310, 0.482, 0.851), (0.690, 0.427, 0.078), (0.165, 0.498, 0.447), (0.541, 0.361, 0.761), (0.769, 0.216, 0.184)],
    [(0.604, 0.624, 0.659), (0.478, 0.612, 0.941), (0.851, 0.584, 0.227), (0.310, 0.710, 0.647), (0.706, 0.545, 0.902), (1.0, 0.420, 0.373)],
];

/// A color of the palette, for what a partition is drawn in.
pub fn palette(dark: bool, index: usize) -> (f64, f64, f64) {
    PALETTE[dark as usize][index % 6]
}

/// The x and the width of each segment in `width`, never narrower than a segment can be drawn.
pub fn spans(weights: &[f64], width: f64) -> Vec<(f64, f64)> {
    let count = weights.len();
    if count == 0 {
        return Vec::new();
    }
    let room = (width - GAP * (count as f64 - 1.0)).max(1.0);
    let total: f64 = weights.iter().sum::<f64>().max(1.0);
    let mut widths: Vec<f64> = weights.iter().map(|weight| weight / total * room).collect();
    // What the narrow ones are given is taken from the wide ones.
    for _ in 0..count {
        let narrow: f64 = widths.iter().filter(|width| **width < MIN_WIDTH).count() as f64;
        if narrow == 0.0 {
            break;
        }
        let wide: f64 = widths.iter().filter(|width| **width >= MIN_WIDTH).sum();
        let left = (room - narrow * MIN_WIDTH).max(0.0);
        let scale = if wide > 0.0 { left / wide } else { 1.0 };
        for width in &mut widths {
            *width = if *width < MIN_WIDTH { MIN_WIDTH } else { *width * scale };
        }
    }
    let mut x = 0.0;
    widths
        .into_iter()
        .map(|width| {
            let span = (x, width);
            x += width + GAP;
            span
        })
        .collect()
}

pub struct PartMap {
    pub area: gtk::DrawingArea,
    segs: RefCell<Vec<Seg>>,
    chosen: Cell<Option<usize>>,
    hovered: Cell<Option<usize>>,
    /// How far the bands have grown, 0 to 1.
    grown: Cell<f64>,
    on_choose: RefCell<Option<Box<dyn Fn(usize)>>>,
}

impl PartMap {
    pub fn new() -> Rc<PartMap> {
        let area = gtk::DrawingArea::builder()
            .hexpand(true)
            .content_height(HEIGHT)
            .focusable(true)
            .has_tooltip(true)
            .build();
        area.update_property(&[gtk::accessible::Property::Label("分割區")]);
        let map = Rc::new(PartMap {
            area,
            segs: RefCell::default(),
            chosen: Cell::new(None),
            hovered: Cell::new(None),
            grown: Cell::new(1.0),
            on_choose: RefCell::default(),
        });
        let drawn = map.clone();
        map.area.set_draw_func(move |area, cr, width, height| drawn.draw(area, cr, width, height));
        map.wire();
        map
    }

    pub fn connect_choose(&self, then: impl Fn(usize) + 'static) {
        self.on_choose.replace(Some(Box::new(then)));
    }

    /// Shows `segs` with `chosen` chosen; the bands of use grow in when `animate`.
    pub fn set(self: &Rc<Self>, segs: Vec<Seg>, chosen: Option<usize>, animate: bool) {
        let changed = *self.segs.borrow() != segs;
        self.chosen.set(chosen);
        self.hovered.set(None);
        if changed {
            self.segs.replace(segs);
        }
        if animate && changed {
            let this = self.clone();
            let target = adw::CallbackAnimationTarget::new(move |value| {
                this.grown.set(value);
                this.area.queue_draw();
            });
            let animation = adw::TimedAnimation::builder()
                .widget(&self.area)
                .value_from(0.0)
                .value_to(1.0)
                .duration(600)
                .easing(adw::Easing::EaseOutCubic)
                .target(&target)
                .build();
            animation.play();
        } else {
            self.grown.set(1.0);
        }
        self.area.queue_draw();
    }

    fn at(&self, x: f64) -> Option<usize> {
        let weights: Vec<f64> = self.segs.borrow().iter().map(|seg| seg.weight).collect();
        let width = self.area.width() as f64 - 2.0 * PAD;
        spans(&weights, width).iter().position(|(start, span)| x - PAD >= *start && x - PAD <= start + span)
    }

    fn choose(&self, index: usize) {
        if let Some(then) = self.on_choose.borrow().as_ref() {
            then(index);
        }
    }

    fn wire(self: &Rc<Self>) {
        let click = gtk::GestureClick::new();
        let this = self.clone();
        click.connect_pressed(move |_, _, x, _| {
            if let Some(index) = this.at(x) {
                this.area.grab_focus();
                this.choose(index);
            }
        });
        self.area.add_controller(click);

        let motion = gtk::EventControllerMotion::new();
        let this = self.clone();
        motion.connect_motion(move |_, x, _| {
            let over = this.at(x);
            if over != this.hovered.get() {
                this.hovered.set(over);
                this.area.set_cursor_from_name(over.map(|_| "pointer"));
                this.area.queue_draw();
            }
        });
        let this = self.clone();
        motion.connect_leave(move |_| {
            this.hovered.set(None);
            this.area.set_cursor_from_name(None);
            this.area.queue_draw();
        });
        self.area.add_controller(motion);

        let keys = gtk::EventControllerKey::new();
        let this = self.clone();
        keys.connect_key_pressed(move |_, key, _, _| {
            let count = this.segs.borrow().len();
            let at = this.chosen.get().unwrap_or(0);
            let to = match key {
                gtk::gdk::Key::Left => at.checked_sub(1),
                gtk::gdk::Key::Right => (at + 1 < count).then_some(at + 1),
                gtk::gdk::Key::Home => Some(0),
                gtk::gdk::Key::End => count.checked_sub(1),
                _ => return glib::Propagation::Proceed,
            };
            if let Some(to) = to {
                this.choose(to);
            }
            glib::Propagation::Stop
        });
        self.area.add_controller(keys);

        let this = self.clone();
        self.area.connect_query_tooltip(move |_, x, _, _, tooltip| match this.at(x as f64) {
            Some(index) => {
                tooltip.set_text(Some(&this.segs.borrow()[index].tip));
                true
            }
            None => false,
        });
        let this = self.clone();
        adw::StyleManager::default().connect_dark_notify(move |_| this.area.queue_draw());
    }

    fn draw(&self, area: &gtk::DrawingArea, cr: &gtk::cairo::Context, width: i32, height: i32) {
        let dark = adw::StyleManager::default().is_dark();
        let (sheet, paper, ink, ink2, line, fill) = if dark {
            ((0.133, 0.133, 0.149), (0.106, 0.106, 0.118), (0.945, 0.937, 0.918), (0.663, 0.655, 0.671), (0.196, 0.196, 0.216), (0.173, 0.173, 0.192))
        } else {
            ((0.984, 0.980, 0.973), (0.957, 0.953, 0.941), (0.090, 0.090, 0.102), (0.365, 0.361, 0.380), (0.886, 0.878, 0.859), (0.918, 0.910, 0.890))
        };
        let segs = self.segs.borrow();
        let weights: Vec<f64> = segs.iter().map(|seg| seg.weight).collect();
        let spans = spans(&weights, width as f64 - 2.0 * PAD);
        let (top, tall) = (PAD, height as f64 - 2.0 * PAD);
        for (index, (seg, (start, span))) in segs.iter().zip(spans).enumerate() {
            let lift = if self.hovered.get() == Some(index) { -1.0 } else { 0.0 };
            let (x, y) = (PAD + start, top + lift);
            rounded(cr, x, y, span, tall, RADIUS);
            let color = seg.color.map(|color| PALETTE[dark as usize][color % 6]);
            match color {
                Some(c) => {
                    cr.set_source_rgb(mix(c.0, sheet.0, 0.18), mix(c.1, sheet.1, 0.18), mix(c.2, sheet.2, 0.18));
                    let _ = cr.fill_preserve();
                    cr.set_source_rgba(c.0, c.1, c.2, 0.30);
                }
                None => {
                    cr.set_source_rgb(fill.0, fill.1, fill.2);
                    let _ = cr.fill_preserve();
                    cr.set_source_rgb(line.0, line.1, line.2);
                }
            }
            cr.set_line_width(1.0);
            let _ = cr.stroke();
            if color.is_none() {
                hatch(cr, x, y, span, tall, line);
            }
            if span >= LABELLED {
                let name_color = if color.is_some() { ink } else { ink2 };
                text(area, cr, &seg.name, x + 13.0, y + 12.0, span - 26.0, true, name_color);
                text(area, cr, &seg.size, x + 13.0, y + 32.0, span - 26.0, false, ink2);
            }
            if let (Some(c), Some(used)) = (color, seg.used) {
                let band = span - 26.0;
                if band > 6.0 {
                    let by = y + tall - 19.0;
                    rounded(cr, x + 13.0, by, band, 6.0, 3.0);
                    cr.set_source_rgba(c.0, c.1, c.2, 0.25);
                    let _ = cr.fill();
                    let filled = (band * used.clamp(0.0, 1.0) * self.grown.get()).max(if used > 0.0 { 3.0 } else { 0.0 });
                    rounded(cr, x + 13.0, by, filled.min(band), 6.0, 3.0);
                    cr.set_source_rgb(c.0, c.1, c.2);
                    let _ = cr.fill();
                }
            }
            if self.chosen.get() == Some(index) {
                rounded(cr, x - 2.0, y - 2.0, span + 4.0, tall + 4.0, RADIUS + 2.0);
                cr.set_source_rgb(paper.0, paper.1, paper.2);
                cr.set_line_width(2.0);
                let _ = cr.stroke();
                rounded(cr, x - 3.5, y - 3.5, span + 7.0, tall + 7.0, RADIUS + 3.5);
                cr.set_source_rgb(ink.0, ink.1, ink.2);
                cr.set_line_width(2.0);
                if area.has_focus() {
                    cr.set_dash(&[6.0, 3.0], 0.0);
                }
                let _ = cr.stroke();
                cr.set_dash(&[], 0.0);
            }
        }
    }
}

fn mix(color: f64, base: f64, share: f64) -> f64 {
    base + (color - base) * share
}

fn rounded(cr: &gtk::cairo::Context, x: f64, y: f64, width: f64, height: f64, radius: f64) {
    let radius = radius.min(width / 2.0).min(height / 2.0);
    cr.new_sub_path();
    cr.arc(x + width - radius, y + radius, radius, -std::f64::consts::FRAC_PI_2, 0.0);
    cr.arc(x + width - radius, y + height - radius, radius, 0.0, std::f64::consts::FRAC_PI_2);
    cr.arc(x + radius, y + height - radius, radius, std::f64::consts::FRAC_PI_2, std::f64::consts::PI);
    cr.arc(x + radius, y + radius, radius, std::f64::consts::PI, 3.0 * std::f64::consts::FRAC_PI_2);
    cr.close_path();
}

/// The stripes that mark free space.
fn hatch(cr: &gtk::cairo::Context, x: f64, y: f64, width: f64, height: f64, color: (f64, f64, f64)) {
    let _ = cr.save();
    rounded(cr, x, y, width, height, RADIUS);
    cr.clip();
    cr.set_source_rgba(color.0, color.1, color.2, 0.8);
    cr.set_line_width(6.0);
    let mut at = -height;
    while at < width {
        cr.move_to(x + at, y + height);
        cr.line_to(x + at + height, y);
        at += 12.0;
    }
    let _ = cr.stroke();
    let _ = cr.restore();
}

fn text(area: &gtk::DrawingArea, cr: &gtk::cairo::Context, text: &str, x: f64, y: f64, width: f64, bold: bool, color: (f64, f64, f64)) {
    let layout = area.create_pango_layout(Some(text));
    let mut font = pango::FontDescription::new();
    font.set_absolute_size(if bold { 13.0 } else { 12.0 } * pango::SCALE as f64);
    font.set_weight(if bold { pango::Weight::Semibold } else { pango::Weight::Normal });
    layout.set_font_description(Some(&font));
    layout.set_width((width * pango::SCALE as f64) as i32);
    layout.set_ellipsize(pango::EllipsizeMode::End);
    cr.set_source_rgb(color.0, color.1, color.2);
    cr.move_to(x, y);
    pangocairo::functions::show_layout(cr, &layout);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments_are_as_wide_as_they_are_large_and_fill_the_row() {
        let spans = spans(&[1.0, 3.0], 403.0);
        assert!((spans[0].1 - 100.0).abs() < 0.01 && (spans[1].1 - 300.0).abs() < 0.01);
        assert!((spans[1].0 + spans[1].1 - 403.0).abs() < 0.01);
    }

    #[test]
    fn a_tiny_partition_is_drawn_at_the_least_width_and_the_row_still_fits() {
        let spans = spans(&[0.001, 1.0, 1.0, 0.002], 500.0);
        assert!(spans[0].1 >= MIN_WIDTH - 0.01 && spans[3].1 >= MIN_WIDTH - 0.01);
        let last = spans.last().unwrap();
        assert!((last.0 + last.1 - 500.0).abs() < 0.01);
    }
}
