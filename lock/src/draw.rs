//! What the lock looks like: the wallpaper, blurred and a little nearer, under ink that
//! deepens where the words are; the time and the date; the user's name and a field for the
//! password. Colours are Nekoland's dark paper and ink, with its one red for a wrong password.

use std::f64::consts::PI;

use cairo::{Context, Format, ImageSurface, LinearGradient, RadialGradient};
use gdk_pixbuf::{InterpType, Pixbuf};

const PAPER: (f64, f64, f64) = (0x1b as f64 / 255.0, 0x1b as f64 / 255.0, 0x1e as f64 / 255.0);
const INK: (f64, f64, f64) = (0xf1 as f64 / 255.0, 0xef as f64 / 255.0, 0xea as f64 / 255.0);
const DANGER: (f64, f64, f64) = (1.0, 0x6b as f64 / 255.0, 0x5f as f64 / 255.0);
/// How much nearer the wallpaper comes while locked.
const NEARER: f64 = 1.06;
/// How far the bottom edge of the ink fades as it lifts.
pub const FEATHER: f64 = 160.0;
const FIELD_WIDTH: f64 = 248.0;
const FIELD_HEIGHT: f64 = 36.0;

/// What the configuration says the lock shows.
pub struct Look {
    pub wallpaper: Option<Pixbuf>,
    pub blur: bool,
    pub font: String,
    pub clock_24h: bool,
    pub name: String,
}

/// What is on the lock at a moment.
pub struct Moment<'a> {
    pub time: &'a str,
    pub date: &'a str,
    /// How many characters have been typed.
    pub typed: usize,
    pub checking: bool,
    pub caps_lock: bool,
    /// What went wrong with the last password, and how far into the shake it is, from 0 to 1.
    pub error: Option<(&'a str, f64)>,
}

/// The wallpaper covering `width` by `height` pixels, `nearer` times larger, blurred or not;
/// the paper alone without one.
pub fn backdrop(look: &Look, width: i32, height: i32, nearer: bool, blur: bool) -> Option<ImageSurface> {
    let surface = ImageSurface::create(Format::ARgb32, width, height).ok()?;
    let cr = Context::new(&surface).ok()?;
    cr.set_source_rgb(PAPER.0, PAPER.1, PAPER.2);
    cr.paint().ok()?;
    let Some(wallpaper) = &look.wallpaper else {
        return Some(surface);
    };
    let grow = if nearer { NEARER } else { 1.0 };
    let (w, h) = (f64::from(wallpaper.width()), f64::from(wallpaper.height()));
    let cover = (f64::from(width) / w).max(f64::from(height) / h) * grow;
    let (cw, ch) = ((w * cover).round() as i32, (h * cover).round() as i32);
    let scaled = if blur {
        // Down to a twentieth averages the picture; up again in two steps keeps it smooth.
        let small = wallpaper.scale_simple((cw / 20).max(1), (ch / 20).max(1), InterpType::Hyper)?;
        let middle = small.scale_simple((cw / 4).max(1), (ch / 4).max(1), InterpType::Bilinear)?;
        middle.scale_simple(cw, ch, InterpType::Bilinear)?
    } else {
        wallpaper.scale_simple(cw, ch, InterpType::Hyper)?
    };
    let picture = pixbuf_surface(&scaled)?;
    cr.set_source_surface(
        &picture,
        f64::from(width - cw) / 2.0,
        f64::from(height - ch) / 2.0,
    )
    .ok()?;
    cr.paint().ok()?;
    surface.flush();
    Some(surface)
}

/// Ink over the backdrop: a little everywhere, more behind the time and the field. In
/// points, on a surface already scaled.
pub fn veil(cr: &Context, width: f64, height: f64) {
    cr.set_source_rgba(PAPER.0, PAPER.1, PAPER.2, 0.24);
    let _ = cr.paint();
    for (y, alpha) in [(0.34, 0.34), (0.66, 0.38)] {
        let (rx, ry) = (width * 0.34, height * 0.26);
        cr.save().ok();
        cr.translate(width / 2.0, height * y);
        cr.scale(rx, ry);
        let glow = RadialGradient::new(0.0, 0.0, 0.0, 0.0, 0.0, 1.0);
        glow.add_color_stop_rgba(0.0, PAPER.0, PAPER.1, PAPER.2, alpha);
        glow.add_color_stop_rgba(0.7, PAPER.0, PAPER.1, PAPER.2, 0.0);
        cr.set_source(&glow).ok();
        cr.arc(0.0, 0.0, 1.0, 0.0, 2.0 * PI);
        let _ = cr.fill();
        cr.restore().ok();
    }
}

fn text(cr: &Context, look: &Look, words: &str, size: f64, light: bool) -> pango::Layout {
    let layout = pangocairo::functions::create_layout(cr);
    // As Nekoland draws text: unhinted, with grey edges.
    if let Ok(mut options) = cairo::FontOptions::new() {
        options.set_antialias(cairo::Antialias::Gray);
        options.set_hint_style(cairo::HintStyle::None);
        options.set_hint_metrics(cairo::HintMetrics::Off);
        pangocairo::functions::context_set_font_options(&layout.context(), Some(&options));
        layout.context_changed();
    }
    let mut font = pango::FontDescription::from_string(&look.font);
    font.set_weight(if light {
        pango::Weight::Light
    } else {
        pango::Weight::Medium
    });
    font.set_absolute_size(size * f64::from(pango::SCALE));
    layout.set_font_description(Some(&font));
    layout.set_text(words);
    layout
}

/// Shows `layout` centred on `x`, its top at `y`; its height.
fn centred(cr: &Context, layout: &pango::Layout, x: f64, y: f64) -> f64 {
    let (_, logical) = layout.pixel_extents();
    cr.move_to(x - f64::from(logical.width()) / 2.0, y);
    pangocairo::functions::show_layout(cr, layout);
    f64::from(logical.height())
}

fn rounded(cr: &Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -PI / 2.0, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, PI / 2.0);
    cr.arc(x + r, y + h - r, r, PI / 2.0, PI);
    cr.arc(x + r, y + r, r, PI, 1.5 * PI);
    cr.close_path();
}

/// How far the field is off centre `t` into a shake: three swings, each smaller.
fn shake(t: f64) -> f64 {
    if t >= 1.0 {
        return 0.0;
    }
    -9.0 * (1.0 - t) * (t * 3.0 * 2.0 * PI).sin()
}

/// The words and the field, in points, on a surface `width` by `height` points.
pub fn words(cr: &Context, look: &Look, moment: &Moment, width: f64, height: f64) {
    let cx = width / 2.0;

    let clock = text(cr, look, moment.time, (width * 0.095).min(116.0), true);
    let (_, clock_extent) = clock.pixel_extents();
    let clock_top = height * 0.35 - f64::from(clock_extent.height()) / 2.0;
    cr.set_source_rgb(INK.0, INK.1, INK.2);
    let clock_height = centred(cr, &clock, cx, clock_top);
    cr.set_source_rgba(INK.0, INK.1, INK.2, 0.72);
    centred(
        cr,
        &text(cr, look, moment.date, 17.0, false),
        cx,
        clock_top + clock_height + 6.0,
    );

    let field_top = height * 0.67 - FIELD_HEIGHT / 2.0;
    cr.set_source_rgb(INK.0, INK.1, INK.2);
    let name = text(cr, look, &look.name, 15.0, false);
    let (_, name_extent) = name.pixel_extents();
    centred(cr, &name, cx, field_top - 18.0 - f64::from(name_extent.height()));

    let x = cx - FIELD_WIDTH / 2.0 + moment.error.map_or(0.0, |(_, t)| shake(t));
    rounded(cr, x, field_top, FIELD_WIDTH, FIELD_HEIGHT, FIELD_HEIGHT / 2.0);
    cr.set_source_rgba(INK.0, INK.1, INK.2, 0.2);
    let _ = cr.fill_preserve();
    match moment.error {
        Some(_) => {
            cr.set_source_rgb(DANGER.0, DANGER.1, DANGER.2);
            cr.set_line_width(1.5);
        }
        None => {
            cr.set_source_rgba(1.0, 1.0, 1.0, 0.4);
            cr.set_line_width(1.0);
        }
    }
    let _ = cr.stroke();

    let middle = field_top + FIELD_HEIGHT / 2.0;
    if moment.typed == 0 && !moment.checking {
        cr.set_source_rgba(INK.0, INK.1, INK.2, 0.5);
        let hint = text(cr, look, "密碼", 14.0, false);
        let (_, extent) = hint.pixel_extents();
        cr.move_to(x + 16.0, middle - f64::from(extent.height()) / 2.0);
        pangocairo::functions::show_layout(cr, &hint);
    } else {
        // The last dots that fit, as a field scrolls to what is being typed.
        let room = ((FIELD_WIDTH - 36.0) / 12.0) as usize;
        let dots = moment.typed.min(room);
        cr.set_source_rgba(INK.0, INK.1, INK.2, if moment.checking { 0.5 } else { 1.0 });
        for i in 0..dots {
            cr.arc(x + 20.0 + 12.0 * i as f64, middle, 3.5, 0.0, 2.0 * PI);
            let _ = cr.fill();
        }
    }

    let note = match moment.error {
        Some((why, _)) => Some((why, DANGER, 1.0)),
        None if moment.caps_lock => Some(("大寫鎖定", INK, 0.72)),
        None => None,
    };
    if let Some((words, colour, alpha)) = note {
        cr.set_source_rgba(colour.0, colour.1, colour.2, alpha);
        centred(
            cr,
            &text(cr, look, words, 13.0, false),
            cx,
            field_top + FIELD_HEIGHT + 10.0,
        );
    }
}

/// The locked layer lifted `lifted` of the way, from 0 to 1, over `under`; its bottom edge
/// fades, so it reads as a sheet taken off rather than a curtain. In pixels.
pub fn lift(cr: &Context, under: &ImageSurface, layer: &ImageSurface, lifted: f64, scale: f64) {
    let _ = cr.set_source_surface(under, 0.0, 0.0);
    let _ = cr.paint();
    let height = f64::from(layer.height());
    let feather = FEATHER * scale;
    let eased = 1.0 - (1.0 - lifted).powi(3);
    let offset = -eased * (height + feather);
    let fade = LinearGradient::new(0.0, offset + height - feather, 0.0, offset + height);
    fade.add_color_stop_rgba(0.0, 0.0, 0.0, 0.0, 1.0);
    fade.add_color_stop_rgba(1.0, 0.0, 0.0, 0.0, 0.0);
    let _ = cr.set_source_surface(layer, 0.0, offset);
    let _ = cr.mask(&fade);
}

/// A picture as Cairo draws it: premultiplied, in native byte order.
fn pixbuf_surface(pixbuf: &Pixbuf) -> Option<ImageSurface> {
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
                let premultiply = |channel: u8| (u16::from(channel) * u16::from(a) / 255) as u8;
                let native = u32::from_be_bytes([
                    a,
                    premultiply(pixel[0]),
                    premultiply(pixel[1]),
                    premultiply(pixel[2]),
                ]);
                let at = y * stride + x * 4;
                data[at..at + 4].copy_from_slice(&native.to_ne_bytes());
            }
        }
    }
    Some(surface)
}

#[cfg(test)]
mod tests {
    use super::shake;

    #[test]
    fn the_shake_settles_where_it_started() {
        assert_eq!(shake(0.0), 0.0);
        assert_eq!(shake(1.0), 0.0);
        assert!(shake(0.08).abs() > 5.0);
        assert!(shake(0.9).abs() < 1.0);
    }
}
