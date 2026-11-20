//! A picture as 截圖 works on it: premultiplied BGRA in memory, the way cairo keeps ARGB32,
//! and what reads, writes and changes its pixels: crops, turns, the censoring of a part, the
//! finding of a part's edges, and its colours in the forms designers copy.

use std::path::Path;

use gtk::gdk_pixbuf::{Colorspace, Pixbuf};
use gtk::prelude::*;
use gtk::{cairo, gdk, glib};

#[derive(Clone)]
pub struct Img {
    pub w: i32,
    pub h: i32,
    pub px: Vec<u8>,
}

/// A rectangle in a picture's pixels.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Area {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Area {
    pub fn new(x: i32, y: i32, w: i32, h: i32) -> Area {
        Area { x, y, w, h }
    }

    /// The rectangle between two corners, whichever way they were dragged.
    pub fn between(a: (f64, f64), b: (f64, f64)) -> Area {
        let (x0, x1) = (a.0.min(b.0), a.0.max(b.0));
        let (y0, y1) = (a.1.min(b.1), a.1.max(b.1));
        Area::new(
            x0.round() as i32,
            y0.round() as i32,
            (x1 - x0).round() as i32,
            (y1 - y0).round() as i32,
        )
    }

    /// The part of it inside a picture of the size given.
    pub fn clamp(self, w: i32, h: i32) -> Area {
        let x0 = self.x.clamp(0, w);
        let y0 = self.y.clamp(0, h);
        let x1 = (self.x + self.w).clamp(0, w);
        let y1 = (self.y + self.h).clamp(0, h);
        Area::new(x0, y0, x1 - x0, y1 - y0)
    }

    pub fn is_empty(self) -> bool {
        self.w <= 0 || self.h <= 0
    }
}

/// A premultiplied BGRA pixel's straight RGBA.
fn straight(p: &[u8]) -> [u8; 4] {
    let a = p[3];
    if a == 0 {
        return [0, 0, 0, 0];
    }
    let un = |c: u8| ((c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8;
    [un(p[2]), un(p[1]), un(p[0]), a]
}

/// A straight RGBA pixel as premultiplied BGRA.
fn premultiplied(rgba: [u8; 4]) -> [u8; 4] {
    let a = rgba[3] as u32;
    let pre = |c: u8| ((c as u32 * a + 127) / 255) as u8;
    [pre(rgba[2]), pre(rgba[1]), pre(rgba[0]), rgba[3]]
}

impl Img {
    pub fn new(w: i32, h: i32, rgba: [u8; 4]) -> Img {
        let pixel = premultiplied(rgba);
        Img {
            w,
            h,
            px: pixel.repeat((w.max(0) * h.max(0)) as usize),
        }
    }

    pub fn load(path: &Path) -> Result<Img, String> {
        let bytes = std::fs::read(path).map_err(|err| err.to_string())?;
        Img::from_bytes(bytes)
    }

    /// A picture from a PNG's or another format's bytes, as the clipboard gives them.
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Img, String> {
        // grim's and our own pictures, read here: gdk-pixbuf starts a sandboxed loader for
        // each, which is slow and, where the sandbox cannot start, never answers.
        if let Some(img) = Img::from_ppm(&bytes).or_else(|| Img::from_png(&bytes)) {
            return Ok(img);
        }
        // GTK reads JPEG and TIFF itself; only other formats go on to gdk-pixbuf.
        let texture =
            gdk::Texture::from_bytes(&glib::Bytes::from_owned(bytes)).map_err(|err| err.to_string())?;
        let mut downloader = gdk::TextureDownloader::new(&texture);
        downloader.set_format(gdk::MemoryFormat::B8g8r8a8Premultiplied);
        let (data, stride) = downloader.download_bytes();
        let (w, h) = (texture.width(), texture.height());
        let mut px = Vec::with_capacity((w * h * 4) as usize);
        for row in data.chunks(stride).take(h as usize) {
            px.extend_from_slice(&row[..(w * 4) as usize]);
        }
        Ok(Img { w, h, px })
    }

    /// A PNG of any colour type, as 8-bit samples.
    fn from_png(bytes: &[u8]) -> Option<Img> {
        let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
        decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
        let mut reader = decoder.read_info().ok()?;
        let mut buf = vec![0; reader.output_buffer_size()?];
        let frame = reader.next_frame(&mut buf).ok()?;
        let (w, h) = (
            i32::try_from(frame.width).ok()?,
            i32::try_from(frame.height).ok()?,
        );
        let channels = match frame.color_type {
            png::ColorType::Grayscale => 1,
            png::ColorType::GrayscaleAlpha => 2,
            png::ColorType::Rgb => 3,
            png::ColorType::Rgba => 4,
            png::ColorType::Indexed => return None,
        };
        let mut px = Vec::with_capacity(frame.width as usize * frame.height as usize * 4);
        for row in buf[..frame.buffer_size()].chunks_exact(frame.line_size) {
            for p in row[..frame.width as usize * channels].chunks_exact(channels) {
                let rgba = match channels {
                    1 => [p[0], p[0], p[0], 255],
                    2 => [p[0], p[0], p[0], p[1]],
                    3 => [p[0], p[1], p[2], 255],
                    _ => [p[0], p[1], p[2], p[3]],
                };
                px.extend_from_slice(&premultiplied(rgba));
            }
        }
        Some(Img { w, h, px })
    }

    /// A binary PPM (P6) of 8-bit samples, as `grim -t ppm` writes.
    fn from_ppm(bytes: &[u8]) -> Option<Img> {
        if !bytes.starts_with(b"P6") {
            return None;
        }
        // Three numbers after the magic, then one whitespace byte before the samples.
        let mut fields = Vec::new();
        let mut i = 2;
        while fields.len() < 3 {
            while bytes.get(i)?.is_ascii_whitespace() {
                i += 1;
            }
            if bytes[i] == b'#' {
                while *bytes.get(i)? != b'\n' {
                    i += 1;
                }
                continue;
            }
            let start = i;
            while bytes.get(i)?.is_ascii_digit() {
                i += 1;
            }
            fields.push(std::str::from_utf8(&bytes[start..i]).ok()?.parse::<i32>().ok()?);
        }
        let (w, h, max) = (fields[0], fields[1], fields[2]);
        let data = bytes.get(i + 1..)?;
        if max != 255 || w <= 0 || h <= 0 || data.len() < (w * h * 3) as usize {
            return None;
        }
        let mut px = Vec::with_capacity((w * h * 4) as usize);
        for p in data[..(w * h * 3) as usize].chunks_exact(3) {
            px.extend_from_slice(&[p[2], p[1], p[0], 255]);
        }
        Some(Img { w, h, px })
    }

    pub fn surface(&self) -> cairo::ImageSurface {
        cairo::ImageSurface::create_for_data(
            self.px.clone(),
            cairo::Format::ARgb32,
            self.w,
            self.h,
            self.w * 4,
        )
        .expect("an ARGB32 surface of a picture's own size")
    }

    /// What was drawn on a surface; it is given up, so that its pixels can be read.
    pub fn from_surface(surface: cairo::ImageSurface) -> Img {
        surface.flush();
        let (w, h, stride) = (surface.width(), surface.height(), surface.stride() as usize);
        let mut surface = surface;
        let data = surface.data().expect("a surface nothing else holds");
        let mut px = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h as usize {
            px.extend_from_slice(&data[y * stride..y * stride + w as usize * 4]);
        }
        Img { w, h, px }
    }

    pub fn rgba(&self, x: i32, y: i32) -> [u8; 4] {
        let x = x.clamp(0, self.w - 1);
        let y = y.clamp(0, self.h - 1);
        let i = ((y * self.w + x) * 4) as usize;
        straight(&self.px[i..i + 4])
    }

    pub fn set(&mut self, x: i32, y: i32, rgba: [u8; 4]) {
        let i = ((y * self.w + x) * 4) as usize;
        self.px[i..i + 4].copy_from_slice(&premultiplied(rgba));
    }

    /// Straight RGBA, row after row, as PNG and pixbufs keep it.
    pub fn straight_rgba(&self) -> Vec<u8> {
        self.px.chunks_exact(4).flat_map(straight).collect()
    }

    pub fn to_pixbuf(&self) -> Pixbuf {
        let bytes = glib::Bytes::from_owned(self.straight_rgba());
        Pixbuf::from_bytes(&bytes, Colorspace::Rgb, true, 8, self.w, self.h, self.w * 4)
    }

    pub fn png(&self) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut out, self.w as u32, self.h as u32);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            if let Ok(mut writer) = encoder.write_header() {
                if let Err(err) = writer.write_image_data(&self.straight_rgba()) {
                    eprintln!("png: {err}");
                }
            }
        }
        out
    }

    /// The picture as a file of the format its name ends in: PNG, or JPEG on white.
    pub fn encode(&self, format: &str) -> Result<Vec<u8>, String> {
        if format == "png" {
            return Ok(self.png());
        }
        let flat = self.on_white();
        let pixbuf = flat.to_pixbuf();
        let kind = if format == "jpg" { "jpeg" } else { format };
        pixbuf
            .save_to_bufferv(kind, &[("quality", "92")])
            .map_err(|err| err.to_string())
    }

    fn on_white(&self) -> Img {
        let mut flat = self.clone();
        for p in flat.px.chunks_exact_mut(4) {
            let a = p[3] as u32;
            for c in &mut p[..3] {
                *c = (*c as u32 + (255 - a)) as u8;
            }
            p[3] = 255;
        }
        flat
    }

    pub fn crop(&self, area: Area) -> Img {
        let area = area.clamp(self.w, self.h);
        let mut px = Vec::with_capacity((area.w * area.h * 4) as usize);
        for y in area.y..area.y + area.h {
            let start = ((y * self.w + area.x) * 4) as usize;
            px.extend_from_slice(&self.px[start..start + area.w as usize * 4]);
        }
        Img {
            w: area.w,
            h: area.h,
            px,
        }
    }

    /// Pastes another picture over this one, its top left corner at `x`, `y`.
    pub fn paste(&mut self, other: &Img, x: i32, y: i32) {
        for row in 0..other.h {
            let ty = y + row;
            if ty < 0 || ty >= self.h {
                continue;
            }
            for col in 0..other.w {
                let tx = x + col;
                if tx < 0 || tx >= self.w {
                    continue;
                }
                let s = ((row * other.w + col) * 4) as usize;
                let d = ((ty * self.w + tx) * 4) as usize;
                self.px[d..d + 4].copy_from_slice(&other.px[s..s + 4]);
            }
        }
    }

    pub fn scaled(&self, w: i32, h: i32) -> Img {
        let (w, h) = (w.max(1), h.max(1));
        let target = cairo::ImageSurface::create(cairo::Format::ARgb32, w, h).expect("a surface");
        {
            let cr = cairo::Context::new(&target).expect("a context");
            cr.scale(w as f64 / self.w as f64, h as f64 / self.h as f64);
            let _ = cr.set_source_surface(self.surface(), 0.0, 0.0);
            cr.source().set_filter(cairo::Filter::Good);
            let _ = cr.paint();
        }
        Img::from_surface(target)
    }

    /// Turned a quarter clockwise as many times as `quarters` says.
    pub fn turned(&self, quarters: i32) -> Img {
        let mut out = self.clone();
        for _ in 0..quarters.rem_euclid(4) {
            let src = out;
            let mut px = vec![0; src.px.len()];
            for y in 0..src.h {
                for x in 0..src.w {
                    let (nx, ny) = (src.h - 1 - y, x);
                    let s = ((y * src.w + x) * 4) as usize;
                    let d = ((ny * src.h + nx) * 4) as usize;
                    px[d..d + 4].copy_from_slice(&src.px[s..s + 4]);
                }
            }
            out = Img {
                w: src.h,
                h: src.w,
                px,
            };
        }
        out
    }

    pub fn flipped(&self, horizontal: bool) -> Img {
        let mut px = vec![0; self.px.len()];
        for y in 0..self.h {
            for x in 0..self.w {
                let (sx, sy) = if horizontal {
                    (self.w - 1 - x, y)
                } else {
                    (x, self.h - 1 - y)
                };
                let s = ((sy * self.w + sx) * 4) as usize;
                let d = ((y * self.w + x) * 4) as usize;
                px[d..d + 4].copy_from_slice(&self.px[s..s + 4]);
            }
        }
        Img {
            w: self.w,
            h: self.h,
            px,
        }
    }

    /// The average colour of a part, as `C` copies it.
    pub fn average(&self, area: Area) -> [u8; 4] {
        let area = area.clamp(self.w, self.h);
        let mut sum = [0u64; 3];
        let mut n = 0u64;
        for y in area.y..area.y + area.h {
            for x in area.x..area.x + area.w {
                let p = self.rgba(x, y);
                for c in 0..3 {
                    sum[c] += p[c] as u64;
                }
                n += 1;
            }
        }
        let n = n.max(1);
        [(sum[0] / n) as u8, (sum[1] / n) as u8, (sum[2] / n) as u8, 255]
    }

    /// The darkest pixel of the 20 by 20 around a point: a line of text's colour, without
    /// pointing at a stroke of it exactly.
    pub fn darkest_near(&self, x: i32, y: i32) -> [u8; 4] {
        let mut best = self.rgba(x, y);
        for yy in y - 10..y + 10 {
            for xx in x - 10..x + 10 {
                let p = self.rgba(xx, yy);
                if luminance(p) < luminance(best) {
                    best = p;
                }
            }
        }
        best
    }

    /// The selection shrunk to where its content starts: each side moves in while the line
    /// along it is all of one colour, the colour of its corner.
    pub fn trim(&self, area: Area) -> Area {
        let mut a = area.clamp(self.w, self.h);
        if a.is_empty() {
            return a;
        }
        let corner = self.rgba(a.x, a.y);
        let uniform = |img: &Img, points: &mut dyn Iterator<Item = (i32, i32)>| {
            let mut same = true;
            for (x, y) in points {
                if distance(corner, img.rgba(x, y)) >= 14.0 {
                    same = false;
                    break;
                }
            }
            same
        };
        while a.h > 1 && uniform(self, &mut (a.x..a.x + a.w).map(|x| (x, a.y))) {
            a.y += 1;
            a.h -= 1;
        }
        while a.h > 1 && uniform(self, &mut (a.x..a.x + a.w).map(|x| (x, a.y + a.h - 1))) {
            a.h -= 1;
        }
        while a.w > 1 && uniform(self, &mut (a.y..a.y + a.h).map(|y| (a.x, y))) {
            a.x += 1;
            a.w -= 1;
        }
        while a.w > 1 && uniform(self, &mut (a.y..a.y + a.h).map(|y| (a.x + a.w - 1, y))) {
            a.w -= 1;
        }
        a
    }

    /// The bounds of the patch of one colour a point is in, as ⌘-clicking chooses it.
    pub fn flood_bounds(&self, x: i32, y: i32) -> Area {
        let start = self.rgba(x, y);
        let mut seen = vec![false; (self.w * self.h) as usize];
        let mut stack = vec![(x, y)];
        let (mut x0, mut y0, mut x1, mut y1) = (x, y, x, y);
        let mut count = 0usize;
        while let Some((px, py)) = stack.pop() {
            if px < 0 || py < 0 || px >= self.w || py >= self.h {
                continue;
            }
            let i = (py * self.w + px) as usize;
            if seen[i] || distance(self.rgba(px, py), start) > 12.0 {
                continue;
            }
            seen[i] = true;
            count += 1;
            if count > 8_000_000 {
                break;
            }
            x0 = x0.min(px);
            y0 = y0.min(py);
            x1 = x1.max(px);
            y1 = y1.max(py);
            stack.extend([(px + 1, py), (px - 1, py), (px, py + 1), (px, py - 1)]);
        }
        Area::new(x0, y0, x1 - x0 + 1, y1 - y0 + 1)
    }

    /// How far the colour under a point goes each way along a line: the length of the
    /// stretch of one colour the point is in, from its first pixel to its last. `vertical`
    /// measures up and down, otherwise left and right.
    pub fn span(&self, x: i32, y: i32, vertical: bool) -> (i32, i32) {
        let start = self.rgba(x, y);
        let (limit, at) = if vertical { (self.h, y) } else { (self.w, x) };
        let same = |i: i32| {
            let p = if vertical {
                self.rgba(x, i)
            } else {
                self.rgba(i, y)
            };
            distance(p, start) < 24.0
        };
        let mut lo = at;
        while lo > 0 && same(lo - 1) {
            lo -= 1;
        }
        let mut hi = at;
        while hi < limit - 1 && same(hi + 1) {
            hi += 1;
        }
        (lo, hi + 1)
    }

    /// The colour most of a part's edge is: what lies behind its text.
    fn background(&self, area: Area) -> [u8; 4] {
        let mut counts: std::collections::HashMap<[u8; 3], usize> = std::collections::HashMap::new();
        let mut add = |p: [u8; 4]| *counts.entry([p[0] & 0xf8, p[1] & 0xf8, p[2] & 0xf8]).or_default() += 1;
        for x in area.x..area.x + area.w {
            add(self.rgba(x, area.y));
            add(self.rgba(x, area.y + area.h - 1));
        }
        for y in area.y..area.y + area.h {
            add(self.rgba(area.x, y));
            add(self.rgba(area.x + area.w - 1, y));
        }
        let key = counts
            .into_iter()
            .max_by_key(|(_, n)| *n)
            .map(|(k, _)| k)
            .unwrap_or([255; 3]);
        // The exact colour of a pixel in the most common bucket.
        for x in area.x..area.x + area.w {
            let p = self.rgba(x, area.y);
            if [p[0] & 0xf8, p[1] & 0xf8, p[2] & 0xf8] == key {
                return p;
            }
        }
        [key[0], key[1], key[2], 255]
    }

    /// Which pixels of a part are not its background: its text, widened by a pixel for the
    /// edges letters are smoothed with.
    fn text_mask(&self, area: Area) -> Vec<bool> {
        let bg = self.background(area);
        let (w, h) = (area.w as usize, area.h as usize);
        let mut mask = vec![false; w * h];
        for y in 0..h {
            for x in 0..w {
                mask[y * w + x] = distance(self.rgba(area.x + x as i32, area.y + y as i32), bg) > 40.0;
            }
        }
        let mut wide = mask.clone();
        for y in 0..h {
            for x in 0..w {
                if mask[y * w + x] {
                    for (dx, dy) in [
                        (-1i32, 0i32),
                        (1, 0),
                        (0, -1),
                        (0, 1),
                        (-1, -1),
                        (1, 1),
                        (-1, 1),
                        (1, -1),
                    ] {
                        let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                        if nx >= 0 && ny >= 0 && (nx as usize) < w && (ny as usize) < h {
                            wide[ny as usize * w + nx as usize] = true;
                        }
                    }
                }
            }
        }
        wide
    }

    /// A part in large squares of its average colours; `block` is a square's side.
    pub fn pixelate(&self, area: Area, block: i32, text_only: bool) -> Img {
        let area = area.clamp(self.w, self.h);
        let mut out = self.crop(area);
        let mask = text_only.then(|| self.text_mask(area));
        let block = block.max(2);
        for by in (0..area.h).step_by(block as usize) {
            for bx in (0..area.w).step_by(block as usize) {
                let bw = block.min(area.w - bx);
                let bh = block.min(area.h - by);
                if let Some(mask) = &mask {
                    let any = (by..by + bh).any(|y| (bx..bx + bw).any(|x| mask[(y * area.w + x) as usize]));
                    if !any {
                        continue;
                    }
                }
                let colour = self.average(Area::new(area.x + bx, area.y + by, bw, bh));
                for y in by..by + bh {
                    for x in bx..bx + bw {
                        out.set(x, y, colour);
                    }
                }
            }
        }
        out
    }

    /// A part blurred, three box blurs being nearly a Gaussian one.
    pub fn blur(&self, area: Area, radius: i32, text_only: bool) -> Img {
        let area = area.clamp(self.w, self.h);
        let pad = Area::new(
            area.x - radius * 3,
            area.y - radius * 3,
            area.w + radius * 6,
            area.h + radius * 6,
        )
        .clamp(self.w, self.h);
        let mut work = self.crop(pad);
        for _ in 0..3 {
            work = box_blur(&work, radius, true);
            work = box_blur(&work, radius, false);
        }
        let blurred = work.crop(Area::new(area.x - pad.x, area.y - pad.y, area.w, area.h));
        if !text_only {
            return blurred;
        }
        let mask = self.text_mask(area);
        let mut out = self.crop(area);
        for (i, on) in mask.iter().enumerate() {
            if *on {
                out.px[i * 4..i * 4 + 4].copy_from_slice(&blurred.px[i * 4..i * 4 + 4]);
            }
        }
        out
    }

    /// A part with what was in it taken away: filled in from its edges, or, for text alone,
    /// its letters given the colour behind them.
    pub fn erase(&self, area: Area, text_only: bool) -> Img {
        let area = area.clamp(self.w, self.h);
        let mut out = self.crop(area);
        if text_only {
            let bg = self.background(area);
            for (i, on) in self.text_mask(area).iter().enumerate() {
                if *on {
                    out.px[i * 4..i * 4 + 4].copy_from_slice(&premultiplied(bg));
                }
            }
            return out;
        }
        let (x0, y0, x1, y1) = (area.x - 1, area.y - 1, area.x + area.w, area.y + area.h);
        for y in 0..area.h {
            for x in 0..area.w {
                let (gx, gy) = (area.x + x, area.y + y);
                let left = self.rgba(x0, gy);
                let right = self.rgba(x1, gy);
                let top = self.rgba(gx, y0);
                let bottom = self.rgba(gx, y1);
                let fx = (x + 1) as f64 / (area.w + 1) as f64;
                let fy = (y + 1) as f64 / (area.h + 1) as f64;
                // Each side counts the more, the nearer the pixel is to it.
                let weights = [1.0 / fx, 1.0 / (1.0 - fx), 1.0 / fy, 1.0 / (1.0 - fy)];
                let total: f64 = weights.iter().sum();
                let mut colour = [0u8; 4];
                for c in 0..4 {
                    let v = (left[c] as f64 * weights[0]
                        + right[c] as f64 * weights[1]
                        + top[c] as f64 * weights[2]
                        + bottom[c] as f64 * weights[3])
                        / total;
                    colour[c] = v.round().clamp(0.0, 255.0) as u8;
                }
                out.set(x, y, colour);
            }
        }
        out
    }
}

fn box_blur(img: &Img, radius: i32, horizontal: bool) -> Img {
    let mut out = img.clone();
    let (outer, inner) = if horizontal {
        (img.h, img.w)
    } else {
        (img.w, img.h)
    };
    let index = |o: i32, i: i32| -> usize {
        let (x, y) = if horizontal { (i, o) } else { (o, i) };
        ((y * img.w + x) * 4) as usize
    };
    for o in 0..outer {
        let mut sum = [0i64; 4];
        let mut n = 0i64;
        for i in -radius..=radius {
            if (0..inner).contains(&i) {
                let p = index(o, i);
                for c in 0..4 {
                    sum[c] += img.px[p + c] as i64;
                }
                n += 1;
            }
        }
        for i in 0..inner {
            let p = index(o, i);
            for c in 0..4 {
                out.px[p + c] = (sum[c] / n.max(1)) as u8;
            }
            let leaving = i - radius;
            let coming = i + radius + 1;
            if leaving >= 0 {
                let q = index(o, leaving);
                for c in 0..4 {
                    sum[c] -= img.px[q + c] as i64;
                }
                n -= 1;
            }
            if coming < inner {
                let q = index(o, coming);
                for c in 0..4 {
                    sum[c] += img.px[q + c] as i64;
                }
                n += 1;
            }
        }
    }
    out
}

pub fn distance(a: [u8; 4], b: [u8; 4]) -> f64 {
    let d = |i: usize| a[i] as f64 - b[i] as f64;
    (d(0) * d(0) + d(1) * d(1) + d(2) * d(2)).sqrt()
}

fn luminance(p: [u8; 4]) -> f64 {
    0.2126 * p[0] as f64 + 0.7152 * p[1] as f64 + 0.0722 * p[2] as f64
}

pub fn hex(p: [u8; 4]) -> String {
    format!("#{:02X}{:02X}{:02X}", p[0], p[1], p[2])
}

fn linear(c: u8) -> f64 {
    let c = c as f64 / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// A colour as OKLCH: lightness, chroma and hue in degrees.
pub fn oklch(p: [u8; 4]) -> (f64, f64, f64) {
    let (r, g, b) = (linear(p[0]), linear(p[1]), linear(p[2]));
    let l = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b).cbrt();
    let m = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b).cbrt();
    let s = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b).cbrt();
    let ll = 0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s;
    let a = 1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s;
    let bb = 0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s;
    let hue = bb.atan2(a).to_degrees().rem_euclid(360.0);
    (ll, (a * a + bb * bb).sqrt(), hue)
}

pub fn hsl(p: [u8; 4]) -> (f64, f64, f64) {
    let (r, g, b) = (p[0] as f64 / 255.0, p[1] as f64 / 255.0, p[2] as f64 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    if max == min {
        return (0.0, 0.0, l * 100.0);
    }
    let d = max - min;
    let s = if l > 0.5 {
        d / (2.0 - max - min)
    } else {
        d / (max + min)
    };
    let h = if max == r {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    (h * 60.0, s * 100.0, l * 100.0)
}

/// APCA's lightness contrast of text of one colour on a background of another.
pub fn apca(text: [u8; 4], background: [u8; 4]) -> f64 {
    let y = |p: [u8; 4]| {
        let c = |v: u8| (v as f64 / 255.0).powf(2.4);
        let y = 0.2126729 * c(p[0]) + 0.7151522 * c(p[1]) + 0.0721750 * c(p[2]);
        if y < 0.022 {
            y + (0.022 - y).powf(1.414)
        } else {
            y
        }
    };
    let (yt, yb) = (y(text), y(background));
    let c = if yb > yt {
        (yb.powf(0.56) - yt.powf(0.57)) * 1.14
    } else {
        (yb.powf(0.65) - yt.powf(0.62)) * 1.14
    };
    let c = if c.abs() < 0.1 {
        0.0
    } else if c > 0.0 {
        c - 0.027
    } else {
        c + 0.027
    };
    c * 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_crop_keeps_its_pixels_and_a_turn_round_comes_back() {
        let mut img = Img::new(3, 2, [255, 255, 255, 255]);
        img.set(2, 1, [10, 20, 30, 255]);
        assert_eq!(img.crop(Area::new(1, 1, 2, 1)).rgba(1, 0), [10, 20, 30, 255]);
        let turned = img.turned(1);
        assert_eq!((turned.w, turned.h), (2, 3));
        assert_eq!(turned.rgba(0, 2), [10, 20, 30, 255]);
        assert_eq!(img.turned(4).px, img.px);
    }

    #[test]
    fn trimming_stops_at_the_content() {
        let mut img = Img::new(10, 10, [255, 255, 255, 255]);
        for y in 3..6 {
            for x in 4..8 {
                img.set(x, y, [0, 0, 0, 255]);
            }
        }
        assert_eq!(img.trim(Area::new(0, 0, 10, 10)), Area::new(4, 3, 4, 3));
        assert_eq!(img.flood_bounds(5, 4), Area::new(4, 3, 4, 3));
        assert_eq!(img.span(5, 4, false), (4, 8));
    }

    #[test]
    fn grims_ppm_is_read_without_gdk_pixbuf() {
        let mut ppm = b"P6\n# grim\n2 1\n255\n".to_vec();
        ppm.extend_from_slice(&[255, 0, 0, 10, 20, 30]);
        let img = Img::from_ppm(&ppm).expect("a PPM");
        assert_eq!((img.w, img.h), (2, 1));
        assert_eq!(img.rgba(0, 0), [255, 0, 0, 255]);
        assert_eq!(img.rgba(1, 0), [10, 20, 30, 255]);
        assert!(Img::from_ppm(b"P6 2 1 255 ").is_none());
    }

    #[test]
    fn a_png_is_read_without_gdk_pixbuf() {
        let mut img = Img::new(3, 2, [10, 20, 30, 255]);
        img.set(2, 1, [200, 100, 0, 128]);
        let read = Img::from_png(&img.png()).expect("a PNG");
        assert_eq!((read.w, read.h), (3, 2));
        assert_eq!(read.rgba(0, 0), [10, 20, 30, 255]);
        let [r, g, b, a] = read.rgba(2, 1);
        assert_eq!(a, 128);
        assert!(r.abs_diff(200) <= 1 && g.abs_diff(100) <= 1 && b == 0);
        // A PNG with one channel, as some apps put on the clipboard.
        let mut gray = Vec::new();
        let mut enc = png::Encoder::new(&mut gray, 2, 1);
        enc.set_color(png::ColorType::Grayscale);
        enc.write_header().unwrap().write_image_data(&[0, 255]).unwrap();
        let read = Img::from_png(&gray).expect("a grey PNG");
        assert_eq!(read.rgba(1, 0), [255, 255, 255, 255]);
        assert!(Img::from_png(b"not a png").is_none());
    }

    #[test]
    fn a_jpeg_is_read_without_gdk_pixbuf() {
        let img = Img::from_bytes(include_bytes!("red.jpg").to_vec()).expect("a JPEG");
        assert_eq!((img.w, img.h), (8, 8));
        let [r, g, b, a] = img.rgba(4, 4);
        assert!(r.abs_diff(200) <= 8 && g.abs_diff(40) <= 8 && b.abs_diff(40) <= 8 && a == 255);
    }

    #[test]
    fn erasing_text_leaves_the_background() {
        let mut img = Img::new(20, 10, [240, 240, 240, 255]);
        img.set(10, 5, [0, 0, 0, 255]);
        let erased = img.erase(Area::new(2, 2, 16, 6), true);
        assert_eq!(erased.rgba(8, 3), [240, 240, 240, 255]);
    }

    #[test]
    fn colours_read_as_designers_write_them() {
        assert_eq!(hex([53, 132, 228, 255]), "#3584E4");
        let (l, _, _) = oklch([255, 255, 255, 255]);
        assert!((l - 1.0).abs() < 0.001);
        assert!(apca([0, 0, 0, 255], [255, 255, 255, 255]) > 100.0);
    }
}
