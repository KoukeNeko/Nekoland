//! Sixel pictures (`DCS P1 ; P2 ; P3 q <sixels> ST`), decoded as Nekominal decodes them, into the same pixels the
//! kitty graphics protocol gives.
//!
//! What it reads: data characters `?`..`~` (six pixels of a column each), repeat `! Pn`, carriage return `$`, next
//! band `-`, raster attributes `" Pan ; Pad ; Ph ; Pv` (pixel aspect and size) and colour registers `# Pc` (select)
//! / `# Pc ; Pu ; Px ; Py ; Pz` (define in HLS, `Pu` 1, or RGB, `Pu` 2, both in percent). Anything else is skipped,
//! as a VT340 does. A register is resolved when a pixel is drawn, not when the picture ends: the encoders in use
//! define their colours first, and the pixels need no second pass. Every picture starts with the VT340's palette
//! (private registers, DECSET 1070's default).
//!
//! Output is untrusted: the picture is cut at `MAX_IMAGE_SIDE` pixels each way (pixels past it are dropped, not
//! stored), a repeat count, a register number or a parameter never grows anything past that, and the pixels a
//! picture may draw are capped.

use std::ops::Range;

use super::kitty_graphics::{Decoded, MAX_IMAGE_SIDE};

const MAX_SIDE: usize = MAX_IMAGE_SIDE as usize;
/// Colour registers of a picture (xterm's default for a VT340 is 16; it allows up to 1024).
const REGISTERS: usize = 1024;
/// Pixel writes one picture may make: a repeat count with a tall aspect draws hundreds of thousands of pixels for a
/// few bytes, so a picture that keeps drawing over itself is cut off here instead of taking seconds.
const DRAW_ALLOWANCE: usize = 2 * MAX_SIDE * MAX_SIDE;

const CHANNELS: usize = 4;
const OPAQUE: u8 = u8::MAX;
const CLEAR: [u8; CHANNELS] = [0; CHANNELS];
const BLACK: [u8; CHANNELS] = [0, 0, 0, OPAQUE];

/// The introducer: parameters, then the final `q`, with no intermediates (DECRQSS is `$q`, XTGETTCAP `+q`).
const INTRODUCER_FINAL: u8 = b'q';
const PARAMETER_SEPARATOR: u8 = b';';

const SIXEL_FIRST: u8 = b'?';
const SIXEL_LAST: u8 = b'~';
const REPEAT_INTRODUCER: u8 = b'!';
const RASTER_INTRODUCER: u8 = b'"';
const COLOR_INTRODUCER: u8 = b'#';
const CARRIAGE_RETURN: u8 = b'$';
const NEXT_BAND: u8 = b'-';
/// Pixels a data character covers, top to bottom.
const BAND_HEIGHT: usize = 6;
/// A data character not after a repeat draws once.
const NO_REPEAT: usize = 1;
/// The tallest or widest a sixel pixel is drawn, in pixels.
const MAX_ASPECT: usize = 16;

/// Parameters an introducer takes at most (`# Pc ; Pu ; Px ; Py ; Pz`).
const PARAMETER_SLOTS: usize = 5;
/// Raster attributes give the size only with all four parameters, and a colour is defined only with all five.
const RASTER_SIZE_PARAMETERS: usize = 4;
const COLOR_DEFINITION_PARAMETERS: usize = 5;
const COLOR_SPACE_HLS: u32 = 1;
const COLOR_SPACE_RGB: u32 = 2;
const DECIMAL_BASE: u32 = 10;

const PERCENT_MAX: u32 = 100;
const HUE_CIRCLE: f32 = 360.0;
/// DEC's hue circle starts at blue, where the usual one has it at 240 degrees.
const DEC_HUE_OFFSET: f32 = 240.0;
const ONE_SIXTH: f32 = 1.0 / 6.0;
const ONE_THIRD: f32 = 1.0 / 3.0;
const ONE_HALF: f32 = 0.5;
const TWO_THIRDS: f32 = 2.0 / 3.0;

/// The VT340's 16 start colours in percent, as xterm sets them; the other registers start black.
const VT340: [[u32; 3]; 16] = [
    [0, 0, 0],
    [20, 20, 80],
    [80, 13, 13],
    [20, 80, 20],
    [80, 20, 80],
    [20, 80, 80],
    [80, 80, 20],
    [53, 53, 53],
    [26, 26, 26],
    [33, 33, 60],
    [60, 26, 26],
    [33, 60, 33],
    [60, 33, 60],
    [33, 60, 60],
    [60, 60, 33],
    [80, 80, 80],
];

/// True when a DCS string (everything after `ESC P`) is a Sixel picture, not another request that ends its
/// introducer in `q`.
pub fn is_sixel(data: &[u8]) -> bool {
    split_introducer(data).is_some()
}

/// The picture of a DCS string: `data` is everything between `ESC P` and the string terminator. The pixels no sixel
/// set are left clear whatever P2 asks, so that the screen's own background shows through them, as Nekominal fills
/// them. None for another DCS or a picture with no pixels.
pub fn decode(data: &[u8]) -> Option<Decoded> {
    let (_, body) = split_introducer(data)?;
    let mut decoder = Decoder::new(DRAW_ALLOWANCE);
    body.iter().for_each(|byte| decoder.put(*byte));
    decoder.finish()
}

fn is_parameter_byte(byte: u8) -> bool {
    byte.is_ascii_digit() || byte == PARAMETER_SEPARATOR
}

/// The introducer's parameters and the sixels after its `q`.
fn split_introducer(data: &[u8]) -> Option<(&[u8], &[u8])> {
    let end = data.iter().position(|byte| !is_parameter_byte(*byte))?;
    (data[end] == INTRODUCER_FINAL).then(|| (&data[..end], &data[end + 1..]))
}

fn default_palette() -> Vec<[u8; CHANNELS]> {
    let mut palette = vec![BLACK; REGISTERS];
    for (slot, [red, green, blue]) in palette.iter_mut().zip(VT340) {
        *slot = [percent(red), percent(green), percent(blue), OPAQUE];
    }
    palette
}

/// A percentage to a channel, rounded.
fn percent(value: u32) -> u8 {
    let channel = u32::from(u8::MAX);
    ((value.min(PERCENT_MAX) * channel + PERCENT_MAX / 2) / PERCENT_MAX) as u8
}

fn fraction(value: u32) -> f32 {
    value.min(PERCENT_MAX) as f32 / PERCENT_MAX as f32
}

fn to_channel(value: f32) -> u8 {
    (value * f32::from(u8::MAX))
        .round()
        .clamp(0.0, f32::from(u8::MAX)) as u8
}

/// DEC HLS (hue 0 is blue, 120 red, 240 green; lightness and saturation in percent) to RGB.
fn hls_to_rgba(hue: u32, lightness: u32, saturation: u32) -> [u8; CHANNELS] {
    let hue = ((hue as f32 % HUE_CIRCLE) + DEC_HUE_OFFSET) % HUE_CIRCLE / HUE_CIRCLE;
    let (lightness, saturation) = (fraction(lightness), fraction(saturation));
    if saturation == 0.0 {
        let gray = to_channel(lightness);
        return [gray, gray, gray, OPAQUE];
    }
    let upper = if lightness < ONE_HALF {
        lightness * (1.0 + saturation)
    } else {
        lightness + saturation - lightness * saturation
    };
    let lower = 2.0 * lightness - upper;
    let channel = |position: f32| to_channel(hue_channel(lower, upper, position.rem_euclid(1.0)));
    [
        channel(hue + ONE_THIRD),
        channel(hue),
        channel(hue - ONE_THIRD),
        OPAQUE,
    ]
}

/// One channel of the usual HSL formula, `position` along the hue circle in 0..1.
fn hue_channel(lower: f32, upper: f32, position: f32) -> f32 {
    if position < ONE_SIXTH {
        lower + (upper - lower) * position / ONE_SIXTH
    } else if position < ONE_HALF {
        upper
    } else if position < TWO_THIRDS {
        lower + (upper - lower) * (TWO_THIRDS - position) / ONE_SIXTH
    } else {
        lower
    }
}

/// `(height, width)` of a sixel pixel from the raster attributes' `Pan : Pad`, the pixel's height to its width.
fn pixel_aspect(numerator: u32, denominator: u32) -> (usize, usize) {
    let (numerator, denominator) = (numerator.max(1) as usize, denominator.max(1) as usize);
    let rounded_ratio = |larger: usize, smaller: usize| ((larger + smaller / 2) / smaller).min(MAX_ASPECT);
    if numerator >= denominator {
        (rounded_ratio(numerator, denominator), 1)
    } else {
        (1, rounded_ratio(denominator, numerator))
    }
}

fn define_color(space: u32, first: u32, second: u32, third: u32) -> Option<[u8; CHANNELS]> {
    match space {
        COLOR_SPACE_HLS => Some(hls_to_rgba(first, second, third)),
        COLOR_SPACE_RGB => Some([percent(first), percent(second), percent(third), OPAQUE]),
        _ => None,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Data,
    Repeat,
    Raster,
    Color,
}

struct Decoder {
    palette: Vec<[u8; CHANNELS]>,
    color: [u8; CHANNELS],
    /// One row of pixels per picture row, each as long as the rightmost pixel set in it.
    rows: Vec<Vec<[u8; CHANNELS]>>,
    column: usize,
    /// Top pixel row of the current band of six.
    band_top: usize,
    /// Each sixel pixel is drawn `pixel_width` x `pixel_height` pixels, as the raster attributes say.
    pixel_width: usize,
    pixel_height: usize,
    /// Size the raster attributes declared (the picture is at least that big).
    declared: (usize, usize),
    state: State,
    parameters: [u32; PARAMETER_SLOTS],
    parameter_count: usize,
    allowance: usize,
    written: usize,
}

impl Decoder {
    fn new(allowance: usize) -> Self {
        let palette = default_palette();
        let color = palette[0];
        Self {
            palette,
            color,
            rows: Vec::new(),
            column: 0,
            band_top: 0,
            pixel_width: 1,
            pixel_height: 1,
            declared: (0, 0),
            state: State::Data,
            parameters: [0; PARAMETER_SLOTS],
            parameter_count: 0,
            allowance,
            written: 0,
        }
    }

    fn put(&mut self, byte: u8) {
        if self.state != State::Data && is_parameter_byte(byte) {
            self.put_parameter(byte);
            return;
        }
        let repeat = self.end_parameters();
        self.put_command(byte, repeat);
    }

    fn put_parameter(&mut self, byte: u8) {
        self.parameter_count = self.parameter_count.max(1);
        if byte == PARAMETER_SEPARATOR {
            self.parameter_count = (self.parameter_count + 1).min(PARAMETER_SLOTS);
            self.parameters[self.parameter_count - 1] = 0;
        } else {
            let slot = &mut self.parameters[self.parameter_count - 1];
            *slot = slot
                .saturating_mul(DECIMAL_BASE)
                .saturating_add(u32::from(byte - b'0'));
        }
    }

    fn put_command(&mut self, byte: u8, repeat: usize) {
        match byte {
            SIXEL_FIRST..=SIXEL_LAST => self.draw_sixel(byte - SIXEL_FIRST, repeat),
            REPEAT_INTRODUCER => self.start(State::Repeat),
            RASTER_INTRODUCER => self.start(State::Raster),
            COLOR_INTRODUCER => self.start(State::Color),
            CARRIAGE_RETURN => self.column = 0,
            NEXT_BAND => self.next_band(),
            _ => {}
        }
    }

    fn start(&mut self, state: State) {
        self.state = state;
        self.parameters = [0; PARAMETER_SLOTS];
        self.parameter_count = 0;
    }

    fn next_band(&mut self) {
        self.column = 0;
        self.band_top = self
            .band_top
            .saturating_add(BAND_HEIGHT * self.pixel_height)
            .min(MAX_SIDE);
    }

    /// Applies the parameters of a repeat, raster or colour introducer that just ended; returns the repeat count for
    /// the data character that follows.
    fn end_parameters(&mut self) -> usize {
        match std::mem::replace(&mut self.state, State::Data) {
            State::Data => NO_REPEAT,
            State::Repeat => (self.parameters[0] as usize).clamp(NO_REPEAT, MAX_SIDE),
            State::Raster => {
                self.apply_raster();
                NO_REPEAT
            }
            State::Color => {
                self.apply_color();
                NO_REPEAT
            }
        }
    }

    fn apply_raster(&mut self) {
        let [numerator, denominator, width, height, _] = self.parameters;
        (self.pixel_height, self.pixel_width) = pixel_aspect(numerator, denominator);
        if self.parameter_count >= RASTER_SIZE_PARAMETERS {
            self.declared = ((width as usize).min(MAX_SIDE), (height as usize).min(MAX_SIDE));
        }
    }

    fn apply_color(&mut self) {
        let [register, space, first, second, third] = self.parameters;
        let Some(register) = usize::try_from(register)
            .ok()
            .filter(|register| *register < REGISTERS)
        else {
            return;
        };
        if self.parameter_count >= COLOR_DEFINITION_PARAMETERS {
            if let Some(color) = define_color(space, first, second, third) {
                self.palette[register] = color;
            }
        }
        self.color = self.palette[register];
    }

    fn draw_sixel(&mut self, bits: u8, repeat: usize) {
        let start = self.column;
        let end = start
            .saturating_add(repeat.saturating_mul(self.pixel_width))
            .min(MAX_SIDE);
        self.column = end;
        if bits == 0 || start >= end {
            return;
        }
        let writes = (end - start) * bits.count_ones() as usize * self.pixel_height;
        if !self.spend(writes) {
            return;
        }
        for bit in (0..BAND_HEIGHT).filter(|bit| bits & (1 << bit) != 0) {
            for sub_row in 0..self.pixel_height {
                let row = self.band_top + bit * self.pixel_height + sub_row;
                if row >= MAX_SIDE {
                    return;
                }
                self.fill_run(row, start..end);
            }
        }
    }

    /// True when `writes` more pixel writes fit the allowance, and counts them.
    fn spend(&mut self, writes: usize) -> bool {
        let written = self.written.saturating_add(writes);
        if written > self.allowance {
            return false;
        }
        self.written = written;
        true
    }

    fn fill_run(&mut self, row: usize, columns: Range<usize>) {
        if self.rows.len() <= row {
            self.rows.resize_with(row + 1, Vec::new);
        }
        let pixels = &mut self.rows[row];
        if pixels.len() < columns.end {
            pixels.resize(columns.end, CLEAR);
        }
        pixels[columns].fill(self.color);
    }

    /// The picture; None when it has no pixels at all.
    fn finish(mut self) -> Option<Decoded> {
        self.end_parameters();
        let width = self
            .rows
            .iter()
            .map(Vec::len)
            .max()
            .unwrap_or(0)
            .max(self.declared.0);
        let height = self.rows.len().max(self.declared.1);
        if width == 0 || height == 0 {
            return None;
        }
        let mut rgba = vec![0; width * height * CHANNELS];
        for (index, pixels) in self.rows.iter().enumerate() {
            let at = index * width * CHANNELS;
            rgba[at..at + pixels.len() * CHANNELS].copy_from_slice(pixels.as_flattened());
        }
        Some(Decoded {
            width: width as u32,
            height: height as u32,
            rgba,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: [u8; 4] = [255, 0, 0, 255];
    const GREEN: [u8; 4] = [0, 255, 0, 255];

    fn picture(data: &[u8]) -> Decoded {
        decode(data).expect("a picture")
    }

    fn pixel(image: &Decoded, column: u32, row: u32) -> [u8; 4] {
        let at = ((row * image.width + column) * 4) as usize;
        image.rgba[at..at + 4].try_into().unwrap()
    }

    fn feed(decoder: &mut Decoder, bytes: &[u8]) {
        bytes.iter().for_each(|byte| decoder.put(*byte));
    }

    #[test]
    fn a_sixel_string_is_told_from_other_dcs_requests() {
        assert!(is_sixel(b"q"));
        assert!(is_sixel(b"0;1;0q#1~"));
        assert!(!is_sixel(b"$qm"), "DECRQSS");
        assert!(!is_sixel(b"+q544e"), "XTGETTCAP");
        assert!(!is_sixel(b"1$q"));
        assert!(!is_sixel(b"0;1"));
        assert!(!is_sixel(b""));
        assert!(decode(b"$qm").is_none());
    }

    #[test]
    fn a_two_by_seven_golden_picture_with_rgb_registers_bands_and_carriage_return() {
        // Band 1: red column of six, then (after `$`) green in the second column's top three rows. Band 2: a red
        // pixel at the top of the second column.
        let image = picture(b"0;1q#1;2;100;0;0#2;2;0;100;0#1~$?#2F-#1?@");
        assert_eq!(
            (image.width, image.height),
            (2, 7),
            "as tall as the lowest pixel set"
        );
        for row in 0..6 {
            assert_eq!(pixel(&image, 0, row), RED, "row {row}");
        }
        assert_eq!(pixel(&image, 1, 0), GREEN);
        assert_eq!(pixel(&image, 1, 2), GREEN);
        assert_eq!(pixel(&image, 1, 3), CLEAR);
        assert_eq!(pixel(&image, 1, 6), RED, "second band starts at row 6");
        assert_eq!(pixel(&image, 0, 6), CLEAR);
    }

    #[test]
    fn repeat_fills_a_run_and_raster_attributes_set_size_and_aspect() {
        let image = picture(b"q\"2;1;10;4#1;2;0;100;0!3~");
        // Declared 10 x 4, but the pixels are twice as tall as wide, so the band covers 12 rows.
        assert_eq!((image.width, image.height), (10, 12));
        assert_eq!(pixel(&image, 2, 11), GREEN);
        assert_eq!(
            pixel(&image, 3, 0),
            CLEAR,
            "past the run, inside the declared width: left for the screen's background"
        );
    }

    #[test]
    fn unset_pixels_are_left_clear_whatever_p2_asks() {
        let body = b"\"1;1;2;1#1;2;100;0;0@";
        let with_p2 = |p2: &str| picture(&[format!("0;{p2}q").as_bytes(), body].concat());
        for p2 in ["0", "1", "2"] {
            assert_eq!(pixel(&with_p2(p2), 1, 0), CLEAR, "P2 {p2}");
            assert_eq!(pixel(&with_p2(p2), 0, 0), RED, "P2 {p2}");
        }
    }

    #[test]
    fn hls_colours_follow_the_dec_hue_circle() {
        assert_eq!(hls_to_rgba(120, 50, 100), RED, "hue 120 is red");
        assert_eq!(hls_to_rgba(240, 50, 100), GREEN);
        assert_eq!(hls_to_rgba(0, 50, 100), [0, 0, 255, 255], "hue 0 is blue");
        assert_eq!(hls_to_rgba(0, 100, 0), [255, 255, 255, 255]);
        assert_eq!(pixel(&picture(b"0;1q#5;1;120;50;100#5@"), 0, 0), RED);
    }

    #[test]
    fn default_registers_are_the_vt340_colours() {
        let image = picture(b"0;1q#2@");
        assert_eq!(
            pixel(&image, 0, 0),
            [204, 33, 33, 255],
            "register 2 is the VT340's red (80, 13, 13 %)"
        );
    }

    #[test]
    fn oversized_input_is_cut_at_the_limit_instead_of_growing() {
        let mut data = b"0;1q#1;2;100;0;0!999999999~".to_vec();
        data.extend(std::iter::repeat_n(b'-', 2000));
        data.extend_from_slice(b"~\"1;1;99999;99999");
        let image = picture(&data);
        assert_eq!(
            (image.width as usize, image.height as usize),
            (MAX_SIDE, MAX_SIDE)
        );
    }

    #[test]
    fn a_picture_stops_drawing_past_its_allowance_and_keeps_what_fit() {
        // "16;1: each sixel pixel 16 tall; !4096~: 4096 columns of six of them, about 400 000 pixels for 7 bytes.
        let flood: Vec<u8> = b"\"16;1".iter().copied().chain(b"!4096~$".repeat(1000)).collect();
        let mut decoder = Decoder::new(1_000_000);
        feed(&mut decoder, &flood);
        assert!(decoder.written <= 1_000_000, "{}", decoder.written);
        assert!(decoder.finish().is_some(), "what fit is the picture");
    }

    #[test]
    fn a_string_without_pixels_is_no_picture() {
        assert!(decode(b"q").is_none());
        assert!(decode(b"0;1q#1;2;1;2;3$-").is_none());
    }

    #[test]
    fn garbage_never_panics() {
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        // Mostly the sixel alphabet, with digits and introducers mixed in.
        let alphabet = b"0123456789;#!\"$-?~@ABCDxyz\x00\xff";
        for _ in 0..200 {
            let data: Vec<u8> = b"q"
                .iter()
                .copied()
                .chain((0..2000).map(|_| {
                    seed ^= seed << 13;
                    seed ^= seed >> 7;
                    seed ^= seed << 17;
                    alphabet[(seed % alphabet.len() as u64) as usize]
                }))
                .collect();
            let _ = decode(&data);
            let _ = decode(&data[1..]);
        }
    }
}
