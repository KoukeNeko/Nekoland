//! What 預覽程式 does to a picture's pixels: the colour 調整顏色 sets, the parts the selection
//! tools and 即時 Alpha choose, and taking the background out.

use gtk::gdk_pixbuf::{Colorspace, Pixbuf};
use gtk::glib;

/// A picture's pixels, four bytes to one, red, green, blue and alpha, row after row.
#[derive(Clone)]
pub struct Rgba {
    pub width: usize,
    pub height: usize,
    pub bytes: Vec<u8>,
}

impl Rgba {
    pub fn from_pixbuf(pixbuf: &Pixbuf) -> Rgba {
        let (width, height) = (pixbuf.width() as usize, pixbuf.height() as usize);
        let (stride, channels) = (pixbuf.rowstride() as usize, pixbuf.n_channels() as usize);
        let source = pixbuf.read_pixel_bytes();
        let mut bytes = Vec::with_capacity(width * height * 4);
        for y in 0..height {
            for x in 0..width {
                let at = y * stride + x * channels;
                bytes.extend_from_slice(&source[at..at + 3]);
                bytes.push(if channels == 4 { source[at + 3] } else { 255 });
            }
        }
        Rgba { width, height, bytes }
    }

    pub fn to_pixbuf(&self) -> Pixbuf {
        Pixbuf::from_bytes(
            &glib::Bytes::from(&self.bytes[..]),
            Colorspace::Rgb,
            true,
            8,
            self.width as i32,
            self.height as i32,
            self.width as i32 * 4,
        )
    }

    pub fn pixel(&self, x: usize, y: usize) -> [u8; 4] {
        let at = (y * self.width + x) * 4;
        [self.bytes[at], self.bytes[at + 1], self.bytes[at + 2], self.bytes[at + 3]]
    }
}

/// What 調整顏色 sets, each slider at 0 leaving the picture as it is, from -1 to 1; the levels
/// are the black, middle and white points, from 0 to 1.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Adjust {
    pub exposure: f64,
    pub contrast: f64,
    pub highlights: f64,
    pub shadows: f64,
    pub saturation: f64,
    pub temperature: f64,
    pub tint: f64,
    /// From 0 to 1.
    pub sepia: f64,
    pub sharpness: f64,
    pub black: f64,
    pub middle: f64,
    pub white: f64,
}

impl Default for Adjust {
    fn default() -> Adjust {
        Adjust {
            exposure: 0.0,
            contrast: 0.0,
            highlights: 0.0,
            shadows: 0.0,
            saturation: 0.0,
            temperature: 0.0,
            tint: 0.0,
            sepia: 0.0,
            sharpness: 0.0,
            black: 0.0,
            middle: 0.5,
            white: 1.0,
        }
    }
}

/// How many pixels of each value each channel has, red, green and blue.
pub fn histogram(picture: &Rgba) -> [[u32; 256]; 3] {
    let mut counts = [[0u32; 256]; 3];
    for pixel in picture.bytes.chunks_exact(4) {
        for channel in 0..3 {
            counts[channel][pixel[channel] as usize] += 1;
        }
    }
    counts
}

/// The black and white points that clip the darkest and lightest half a percent, as 自動色階
/// sets them.
pub fn auto_levels(picture: &Rgba) -> (f64, f64) {
    let counts = histogram(picture);
    let total: u32 = (0..256).map(|v| counts[0][v] + counts[1][v] + counts[2][v]).sum();
    let clip = (f64::from(total) * 0.005) as u32;
    let mut seen = 0;
    let mut black = 0;
    for value in 0..256 {
        seen += counts[0][value] + counts[1][value] + counts[2][value];
        if seen > clip {
            black = value;
            break;
        }
    }
    seen = 0;
    let mut white = 255;
    for value in (0..256).rev() {
        seen += counts[0][value] + counts[1][value] + counts[2][value];
        if seen > clip {
            white = value;
            break;
        }
    }
    if white <= black {
        return (0.0, 1.0);
    }
    (black as f64 / 255.0, white as f64 / 255.0)
}

/// The picture with `adjust` applied.
pub fn adjust(picture: &Rgba, adjust: &Adjust) -> Rgba {
    let a = adjust;
    let gain = 2f64.powf(a.exposure * 2.0);
    let contrast = 1.0 + a.contrast * 0.9;
    let span = (a.white - a.black).max(1e-3);
    // The middle point is where the curve reaches half: a power that takes it there.
    let gamma = (0.5f64.ln() / a.middle.clamp(0.01, 0.99).ln()).clamp(0.1, 10.0);
    let mut out = picture.clone();
    for pixel in out.bytes.chunks_exact_mut(4) {
        let mut rgb = [f64::from(pixel[0]) / 255.0, f64::from(pixel[1]) / 255.0, f64::from(pixel[2]) / 255.0];
        for value in &mut rgb {
            *value = ((*value - a.black) / span).clamp(0.0, 1.0).powf(gamma);
            *value *= gain;
        }
        // Warmer adds red and takes blue; the tint goes from green to magenta.
        rgb[0] *= 1.0 + a.temperature * 0.3;
        rgb[2] *= 1.0 - a.temperature * 0.3;
        rgb[1] *= 1.0 - a.tint * 0.25;
        let luma = 0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2];
        // Shadows lift or deepen the dark tones, highlights the light ones.
        let lift = a.shadows * 0.5 * (1.0 - luma).powi(2) + a.highlights * 0.5 * luma.powi(2);
        for value in &mut rgb {
            *value += lift;
            *value = (*value - 0.5) * contrast + 0.5;
        }
        let luma = 0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2];
        let saturation = 1.0 + a.saturation;
        for value in &mut rgb {
            *value = luma + (*value - luma) * saturation;
        }
        if a.sepia > 0.0 {
            let sepia = [
                0.393 * rgb[0] + 0.769 * rgb[1] + 0.189 * rgb[2],
                0.349 * rgb[0] + 0.686 * rgb[1] + 0.168 * rgb[2],
                0.272 * rgb[0] + 0.534 * rgb[1] + 0.131 * rgb[2],
            ];
            for channel in 0..3 {
                rgb[channel] += (sepia[channel] - rgb[channel]) * a.sepia;
            }
        }
        for channel in 0..3 {
            pixel[channel] = (rgb[channel].clamp(0.0, 1.0) * 255.0).round() as u8;
        }
    }
    if a.sharpness.abs() > 1e-3 {
        out = sharpen(&out, a.sharpness);
    }
    out
}

/// Sharpened by the difference from a blur, or blurred when `amount` is below 0.
fn sharpen(picture: &Rgba, amount: f64) -> Rgba {
    let (width, height) = (picture.width, picture.height);
    let mut out = picture.clone();
    for y in 0..height {
        for x in 0..width {
            for channel in 0..3 {
                let mut sum = 0.0;
                let mut count = 0.0;
                for dy in -1i64..=1 {
                    for dx in -1i64..=1 {
                        let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                        if nx >= 0 && ny >= 0 && (nx as usize) < width && (ny as usize) < height {
                            sum += f64::from(picture.bytes[(ny as usize * width + nx as usize) * 4 + channel]);
                            count += 1.0;
                        }
                    }
                }
                let at = (y * width + x) * 4 + channel;
                let value = f64::from(picture.bytes[at]);
                let blurred = sum / count;
                out.bytes[at] = (value + (value - blurred) * amount * 2.0).clamp(0.0, 255.0).round() as u8;
            }
        }
    }
    out
}

/// Which pixels a selection takes, one to a pixel, row after row.
#[derive(Clone, PartialEq, Debug)]
pub struct Mask {
    pub width: usize,
    pub height: usize,
    pub inside: Vec<bool>,
}

impl Mask {
    pub fn empty(width: usize, height: usize) -> Mask {
        Mask { width, height, inside: vec![false; width * height] }
    }

    pub fn is_empty(&self) -> bool {
        !self.inside.contains(&true)
    }

    pub fn rectangle(width: usize, height: usize, (x1, y1, x2, y2): (f64, f64, f64, f64)) -> Mask {
        let mut mask = Mask::empty(width, height);
        let (left, right) = (x1.min(x2).max(0.0) as usize, (x1.max(x2).ceil() as usize).min(width));
        let (top, bottom) = (y1.min(y2).max(0.0) as usize, (y1.max(y2).ceil() as usize).min(height));
        for y in top..bottom {
            for x in left..right {
                mask.inside[y * width + x] = true;
            }
        }
        mask
    }

    pub fn ellipse(width: usize, height: usize, (x1, y1, x2, y2): (f64, f64, f64, f64)) -> Mask {
        let mut mask = Mask::empty(width, height);
        let (cx, cy) = ((x1 + x2) / 2.0, (y1 + y2) / 2.0);
        let (rx, ry) = (((x2 - x1) / 2.0).abs().max(0.5), ((y2 - y1) / 2.0).abs().max(0.5));
        for y in 0..height {
            for x in 0..width {
                let (dx, dy) = ((x as f64 + 0.5 - cx) / rx, (y as f64 + 0.5 - cy) / ry);
                mask.inside[y * width + x] = dx * dx + dy * dy <= 1.0;
            }
        }
        mask
    }

    /// What a lasso drawn through `points` takes, closed by a straight line to where it began.
    pub fn lasso(width: usize, height: usize, points: &[(f64, f64)]) -> Mask {
        let mut mask = Mask::empty(width, height);
        if points.len() < 3 {
            return mask;
        }
        for y in 0..height {
            let fy = y as f64 + 0.5;
            // Where the outline crosses this row, left to right.
            let mut crossings: Vec<f64> = Vec::new();
            for index in 0..points.len() {
                let (a, b) = (points[index], points[(index + 1) % points.len()]);
                if (a.1 <= fy) != (b.1 <= fy) {
                    crossings.push(a.0 + (fy - a.1) / (b.1 - a.1) * (b.0 - a.0));
                }
            }
            crossings.sort_by(f64::total_cmp);
            for pair in crossings.chunks_exact(2) {
                let (from, to) = ((pair[0] - 0.5).ceil().max(0.0) as usize, ((pair[1] - 0.5).floor() as i64 + 1).clamp(0, width as i64) as usize);
                for x in from..to {
                    mask.inside[y * width + x] = true;
                }
            }
        }
        mask
    }

    pub fn invert(&self) -> Mask {
        Mask { width: self.width, height: self.height, inside: self.inside.iter().map(|inside| !inside).collect() }
    }

    /// The smallest box around what is inside: left, top, width and height.
    pub fn bounds(&self) -> Option<(usize, usize, usize, usize)> {
        let (mut left, mut top, mut right, mut bottom) = (usize::MAX, usize::MAX, 0, 0);
        for y in 0..self.height {
            for x in 0..self.width {
                if self.inside[y * self.width + x] {
                    left = left.min(x);
                    right = right.max(x);
                    top = top.min(y);
                    bottom = bottom.max(y);
                }
            }
        }
        (left != usize::MAX).then(|| (left, top, right - left + 1, bottom - top + 1))
    }
}

fn distance(a: [u8; 4], b: [u8; 4]) -> f64 {
    let d = |i: usize| f64::from(a[i]) - f64::from(b[i]);
    (d(0) * d(0) + d(1) * d(1) + d(2) * d(2)).sqrt() / (3.0f64.sqrt() * 255.0)
}

/// The pixels joined to those at `seeds` whose colour is within `tolerance`, from 0 to 1, of
/// the colour of the seed they are reached from.
pub fn flood(picture: &Rgba, seeds: &[(usize, usize)], tolerance: f64) -> Mask {
    let (width, height) = (picture.width, picture.height);
    let mut mask = Mask::empty(width, height);
    let mut queue: Vec<(usize, usize, [u8; 4])> = Vec::new();
    for &(x, y) in seeds {
        if x < width && y < height && !mask.inside[y * width + x] {
            mask.inside[y * width + x] = true;
            queue.push((x, y, picture.pixel(x, y)));
        }
    }
    while let Some((x, y, seed)) = queue.pop() {
        let neighbours = [
            (x.wrapping_sub(1), y),
            (x + 1, y),
            (x, y.wrapping_sub(1)),
            (x, y + 1),
        ];
        for (nx, ny) in neighbours {
            if nx >= width || ny >= height || mask.inside[ny * width + nx] {
                continue;
            }
            if distance(picture.pixel(nx, ny), seed) <= tolerance {
                mask.inside[ny * width + nx] = true;
                queue.push((nx, ny, seed));
            }
        }
    }
    mask
}

/// 即時 Alpha: what is joined to the pixel pressed and like it, the more alike the shorter
/// the drag from it, as `drag` pixels long on screen.
pub fn instant_alpha(picture: &Rgba, at: (usize, usize), drag: f64) -> Mask {
    flood(picture, &[at], (drag / 400.0).clamp(0.0, 1.0))
}

/// 移除背景: what reaches the picture's edges in colours like those along them.
pub fn background(picture: &Rgba) -> Mask {
    let (width, height) = (picture.width, picture.height);
    let mut seeds = Vec::new();
    for x in 0..width {
        seeds.push((x, 0));
        seeds.push((x, height - 1));
    }
    for y in 0..height {
        seeds.push((0, y));
        seeds.push((width - 1, y));
    }
    flood(picture, &seeds, 0.12)
}

/// The picture with what `mask` takes made clear.
pub fn clear(picture: &Rgba, mask: &Mask) -> Rgba {
    let mut out = picture.clone();
    for (pixel, inside) in out.bytes.chunks_exact_mut(4).zip(&mask.inside) {
        if *inside {
            pixel[3] = 0;
        }
    }
    out
}

/// What `mask` takes of the picture, cut to its bounds, the rest of them made clear.
pub fn cut(picture: &Rgba, mask: &Mask) -> Option<Rgba> {
    let (left, top, width, height) = mask.bounds()?;
    let mut bytes = Vec::with_capacity(width * height * 4);
    for y in top..top + height {
        for x in left..left + width {
            let mut pixel = picture.pixel(x, y);
            if !mask.inside[y * picture.width + x] {
                pixel[3] = 0;
            }
            bytes.extend_from_slice(&pixel);
        }
    }
    Some(Rgba { width, height, bytes })
}

/// Units the 調整大小 window measures in, and how many of a pixel at `dpi`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Unit {
    Pixels,
    Percent,
    Inches,
    Centimetres,
    Millimetres,
    Points,
}

pub const UNITS: [(Unit, &str); 6] = [
    (Unit::Pixels, "像素"),
    (Unit::Percent, "百分比"),
    (Unit::Inches, "英寸"),
    (Unit::Centimetres, "公分"),
    (Unit::Millimetres, "公釐"),
    (Unit::Points, "點"),
];

/// `pixels` in `unit`, a picture `original` pixels on that side printed at `dpi`.
pub fn from_pixels(pixels: f64, unit: Unit, dpi: f64, original: f64) -> f64 {
    match unit {
        Unit::Pixels => pixels,
        Unit::Percent => pixels / original * 100.0,
        Unit::Inches => pixels / dpi,
        Unit::Centimetres => pixels / dpi * 2.54,
        Unit::Millimetres => pixels / dpi * 25.4,
        Unit::Points => pixels / dpi * 72.0,
    }
}

pub fn to_pixels(value: f64, unit: Unit, dpi: f64, original: f64) -> f64 {
    match unit {
        Unit::Pixels => value,
        Unit::Percent => value / 100.0 * original,
        Unit::Inches => value * dpi,
        Unit::Centimetres => value / 2.54 * dpi,
        Unit::Millimetres => value / 25.4 * dpi,
        Unit::Points => value / 72.0 * dpi,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture(width: usize, height: usize, at: impl Fn(usize, usize) -> [u8; 4]) -> Rgba {
        let mut bytes = Vec::new();
        for y in 0..height {
            for x in 0..width {
                bytes.extend_from_slice(&at(x, y));
            }
        }
        Rgba { width, height, bytes }
    }

    #[test]
    fn settings_at_rest_leave_the_picture_as_it_is() {
        let original = picture(4, 4, |x, y| [(x * 60) as u8, (y * 60) as u8, 128, 255]);
        assert!(adjust(&original, &Adjust::default()).bytes == original.bytes);
    }

    #[test]
    fn saturation_all_the_way_down_makes_it_grey_and_exposure_lightens() {
        let original = picture(2, 1, |x, _| if x == 0 { [200, 40, 40, 255] } else { [40, 40, 200, 255] });
        let grey = adjust(&original, &Adjust { saturation: -1.0, ..Adjust::default() });
        for pixel in grey.bytes.chunks_exact(4) {
            assert!(pixel[0].abs_diff(pixel[1]) <= 1 && pixel[1].abs_diff(pixel[2]) <= 1);
        }
        let lighter = adjust(&original, &Adjust { exposure: 0.5, ..Adjust::default() });
        assert!(lighter.bytes[0] > original.bytes[0]);
    }

    #[test]
    fn instant_alpha_takes_the_joined_colour_and_not_what_is_across_a_line() {
        // White, a black line down the middle, white again.
        let original = picture(9, 3, |x, _| if x == 4 { [0, 0, 0, 255] } else { [250, 250, 250, 255] });
        let mask = instant_alpha(&original, (0, 0), 20.0);
        assert!(mask.inside[0] && mask.inside[3]);
        assert!(!mask.inside[4] && !mask.inside[5]);
        let cleared = clear(&original, &mask);
        assert_eq!(cleared.bytes[3], 0);
        assert_eq!(cleared.bytes[4 * 4 + 3], 255);
    }

    #[test]
    fn the_background_is_what_reaches_the_edges() {
        // A dark square on light ground.
        let original = picture(8, 8, |x, y| if (2..6).contains(&x) && (2..6).contains(&y) { [20, 20, 20, 255] } else { [240, 240, 240, 255] });
        let mask = background(&original);
        assert!(mask.inside[0]);
        assert!(!mask.inside[3 * 8 + 3]);
        assert_eq!(mask.invert().bounds(), Some((2, 2, 4, 4)));
    }

    #[test]
    fn a_lasso_takes_what_it_goes_round() {
        let mask = Mask::lasso(10, 10, &[(1.0, 1.0), (9.0, 1.0), (9.0, 9.0), (1.0, 9.0)]);
        assert!(mask.inside[5 * 10 + 5]);
        assert!(!mask.inside[0]);
        assert_eq!(mask.bounds(), Some((1, 1, 8, 8)));
        let ellipse = Mask::ellipse(10, 10, (0.0, 0.0, 10.0, 10.0));
        assert!(ellipse.inside[5 * 10 + 5] && !ellipse.inside[0]);
    }

    #[test]
    fn sizes_go_between_units_and_back() {
        for (unit, _) in UNITS {
            let value = from_pixels(300.0, unit, 150.0, 600.0);
            assert!((to_pixels(value, unit, 150.0, 600.0) - 300.0).abs() < 1e-9);
        }
        assert!((from_pixels(300.0, Unit::Inches, 150.0, 600.0) - 2.0).abs() < 1e-9);
    }
}
