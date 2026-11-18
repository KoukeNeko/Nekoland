//! The kitty graphics protocol (`APC G <control> ; <payload> ST`), as Nekominal speaks it: the control keys of a
//! command, the pixels a transmission carries and the reply a program waits for. What a command does to the pane
//! (keeping a picture, placing it, the part of it shown, deleting it) is the pane's.
//! <https://sw.kovidgoyal.net/kitty/graphics-protocol/>
//!
//! Every input is untrusted: sizes are checked before anything is allocated for them (`MAX_IMAGE_BYTES`), a
//! compressed payload is inflated only up to that size, and a file is read only when it is a regular file of a
//! bounded size. A temporary file (`t=t`) is deleted after it is read only under kitty's own rule: it is in a
//! temporary folder and its name says `tty-graphics-protocol`, so a program cannot have the terminal delete anything
//! else.

use std::{
    fs::File,
    io::{Cursor, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

use base64::{
    alphabet,
    engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig},
    Engine,
};
use flate2::read::ZlibDecoder;

/// The widest and tallest picture kept, in pixels.
pub const MAX_IMAGE_SIDE: u32 = 4096;
/// One picture's pixels at most: 4096 x 4096 RGBA.
pub const MAX_IMAGE_BYTES: usize = 64 * 1024 * 1024;
/// Room for the PNG or zlib framing around a picture at the size limit.
const FRAMING_ROOM: usize = 64 * 1024;
/// Base64 carries three bytes in four characters.
const BASE64_GROUP_BYTES: usize = 3;
const BASE64_GROUP_CHARS: usize = 4;
/// The most base64 one image may take while its chunks arrive: an image at the size limit, as base64, and its
/// framing.
pub const MAX_TRANSFER: usize = MAX_IMAGE_BYTES / BASE64_GROUP_BYTES * BASE64_GROUP_CHARS + FRAMING_ROOM;

/// What an absent key leaves in a `Command`; kitty reads it as the key's default.
const UNSET_LETTER: u8 = 0;
const UNSET_NUMBER: u32 = 0;
const KEY_SEPARATOR: u8 = b',';
const VALUE_SEPARATOR: u8 = b'=';
const FLAG_ON: u32 = 1;

const ACTION_TRANSMIT: u8 = b't';
const ACTION_DELETE: u8 = b'd';
const MEDIUM_DIRECT: u8 = b'd';
const MEDIUM_FILE: u8 = b'f';
const MEDIUM_TEMP_FILE: u8 = b't';
const MEDIUM_SHARED_MEMORY: u8 = b's';
const COMPRESSION_ZLIB: u8 = b'z';
const FORMAT_RGB: u32 = 24;
const FORMAT_RGBA: u32 = 32;
const FORMAT_PNG: u32 = 100;

const GRAY_ALPHA_CHANNELS: usize = 2;
const RGB_CHANNELS: usize = 3;
const RGBA_CHANNELS: usize = 4;
const OPAQUE: u8 = u8::MAX;

/// The errno names kitty replies with.
const INVALID: &str = "EINVAL";
const TOO_LARGE: &str = "EFBIG";
const NO_DATA: &str = "ENODATA";
const NOT_PERMITTED: &str = "EPERM";
const NOT_FOUND: &str = "ENOENT";
const BAD_FILE: &str = "EBADF";

/// `q=1` drops the OK replies, `q=2` every reply.
const QUIET_OK: u32 = 1;
const QUIET_ALL: u32 = 2;
const REPLY_START: &str = "\x1b_G";
const REPLY_END: &str = "\x1b\\";
const REPLY_OK: &str = "OK";

/// kitty's rule for a file the terminal may delete: in one of these folders (or `TMPDIR`), with this in its path.
const TEMP_FILE_MARKER: &str = "tty-graphics-protocol";
const KNOWN_TEMP_FOLDERS: [&str; 2] = ["/tmp", "/dev/shm"];

/// Programs pad their base64 or leave it out, and kitty takes both; chafa's padded chunks also leave bits set past
/// their last byte, which kitty ignores.
pub const LENIENT_BASE64: GeneralPurpose = GeneralPurpose::new(
    &alphabet::STANDARD,
    GeneralPurposeConfig::new()
        .with_decode_padding_mode(DecodePaddingMode::Indifferent)
        .with_decode_allow_trailing_bits(true),
);

/// One command's control keys. Absent keys are 0 (kitty's defaults: medium `d`, format 32, delete `a`), except
/// the action, which `parse` sets to `t`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Command {
    /// `a`
    pub action: u8,
    /// `q`: 1 drops the OK replies, 2 every reply.
    pub quiet: u32,
    /// `i`, `I`, `p`
    pub id: u32,
    pub number: u32,
    pub placement: u32,
    /// `f`, `t`, `o`
    pub format: u32,
    pub medium: u8,
    pub compression: u8,
    /// `s`, `v`: pixel size of raw data; `S`, `O`: bytes to read from a file and where.
    pub width: u32,
    pub height: u32,
    pub size: u32,
    pub offset: u32,
    /// `m=1`: more chunks follow.
    pub more: bool,
    /// `x`, `y`, `w`, `h`: the part of the image shown.
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    /// `X`, `Y`: pixel offset inside the first cell; `c`, `r`: cells to fit the image in.
    pub cell_x: u32,
    pub cell_y: u32,
    pub columns: u32,
    pub rows: u32,
    pub z: i32,
    /// `C=1`: the cursor stays.
    pub no_move: bool,
    /// `U=1`: a Unicode placeholder placement.
    pub placeholder: bool,
    /// `d`
    pub delete: u8,
}

impl Command {
    /// Parses the control keys (`a=T,f=100,i=7`). Unknown keys are skipped; a key with a value that is not one is
    /// an error, as in kitty.
    pub fn parse(control: &[u8]) -> Option<Self> {
        let mut command = Command::default();
        for pair in control
            .split(|byte| *byte == KEY_SEPARATOR)
            .filter(|pair| !pair.is_empty())
        {
            let [key, VALUE_SEPARATOR, value @ ..] = pair else {
                return None;
            };
            command.set_key(*key, value)?;
        }
        if command.action == UNSET_LETTER {
            command.action = ACTION_TRANSMIT;
        }
        Some(command)
    }

    /// Sets one key; None when its value is not of the key's kind.
    fn set_key(&mut self, key: u8, value: &[u8]) -> Option<()> {
        let number = || parse_number::<u32>(value);
        let letter = || parse_letter(value);
        match key {
            b'a' => self.action = letter()?,
            b'q' => self.quiet = number()?,
            b'i' => self.id = number()?,
            b'I' => self.number = number()?,
            b'p' => self.placement = number()?,
            b'f' => self.format = number()?,
            b't' => self.medium = letter()?,
            b'o' => self.compression = letter()?,
            b's' => self.width = number()?,
            b'v' => self.height = number()?,
            b'S' => self.size = number()?,
            b'O' => self.offset = number()?,
            b'm' => self.more = number()? == FLAG_ON,
            b'x' => self.x = number()?,
            b'y' => self.y = number()?,
            b'w' => self.w = number()?,
            b'h' => self.h = number()?,
            b'X' => self.cell_x = number()?,
            b'Y' => self.cell_y = number()?,
            b'c' => self.columns = number()?,
            b'r' => self.rows = number()?,
            b'z' => self.z = parse_number::<i32>(value)?,
            b'C' => self.no_move = number()? == FLAG_ON,
            b'U' => self.placeholder = number()? == FLAG_ON,
            b'd' => self.delete = letter()?,
            _ => {}
        }
        Some(())
    }

    /// True when the program named the image (`i` or `I`): kitty answers only those, so a program that did not ask
    /// for an answer never finds one in its input.
    fn names_an_image(&self) -> bool {
        self.id != UNSET_NUMBER || self.number != UNSET_NUMBER
    }
}

fn parse_number<T: std::str::FromStr>(value: &[u8]) -> Option<T> {
    std::str::from_utf8(value).ok()?.parse().ok()
}

fn parse_letter(value: &[u8]) -> Option<u8> {
    match value {
        [letter] => Some(*letter),
        _ => None,
    }
}

/// Why a command failed: an errno name and a message, as kitty replies them (`ENOENT:no such image`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub code: &'static str,
    pub message: String,
}

impl Failure {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

pub fn fail<T>(code: &'static str, message: impl Into<String>) -> Result<T, Failure> {
    Err(Failure::new(code, message))
}

/// The pixels a transmission carried: RGBA8, straight alpha, row by row.
pub struct Decoded {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// The pixels of a complete transmission, the whole image: `payload` is the base64 of all its chunks. The part
/// shown (`x`, `y`, `w`, `h`) is left to the placement, which may show another part of the same image later.
pub fn decode(command: &Command, payload: &[u8]) -> Result<Decoded, Failure> {
    let transmitted = read_medium(command, payload)?;
    let bytes = decompress(command.compression, transmitted)?;
    decode_format(command, &bytes)
}

/// `ESC _ G i=… [,I=…] [,p=…] ; OK ESC \` or the error, for a command that ended with `outcome`. None for a
/// command that named no image, for a delete, for an OK under `q=1` and for anything under `q=2`. The keys are the
/// command's as they are: a transmission that gave only `I` is answered with the id the terminal chose, set in
/// `command.id` first.
pub fn reply(command: &Command, outcome: Result<(), &Failure>) -> Option<Vec<u8>> {
    if !command.names_an_image() || command.action == ACTION_DELETE {
        return None;
    }
    let message = match outcome {
        Ok(()) if command.quiet >= QUIET_OK => return None,
        Err(_) if command.quiet >= QUIET_ALL => return None,
        Ok(()) => REPLY_OK.to_owned(),
        Err(failure) => format!("{}:{}", failure.code, failure.message),
    };
    Some(format!("{REPLY_START}{};{message}{REPLY_END}", reply_keys(command)).into_bytes())
}

/// The keys a reply names the image by, the absent ones left out.
fn reply_keys(command: &Command) -> String {
    [("i", command.id), ("I", command.number), ("p", command.placement)]
        .iter()
        .filter(|(_, value)| *value != UNSET_NUMBER)
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join(",")
}

/// The bytes the payload stands for: itself, or the file it names.
fn read_medium(command: &Command, payload: &[u8]) -> Result<Vec<u8>, Failure> {
    match command.medium {
        UNSET_LETTER | MEDIUM_DIRECT => decode_base64(payload),
        MEDIUM_FILE | MEDIUM_TEMP_FILE => read_file(&decode_path(payload)?, command),
        MEDIUM_SHARED_MEMORY => fail(INVALID, "shared memory is not supported"),
        _ => fail(INVALID, "unknown transmission medium"),
    }
}

fn decode_base64(payload: &[u8]) -> Result<Vec<u8>, Failure> {
    // A program that wraps its base64 in lines still means the one string.
    let joined: Vec<u8> = payload
        .iter()
        .copied()
        .filter(|byte| !matches!(byte, b'\r' | b'\n'))
        .collect();
    LENIENT_BASE64
        .decode(joined)
        .or_else(|_| fail(INVALID, "bad base64 data"))
}

fn decode_path(payload: &[u8]) -> Result<String, Failure> {
    String::from_utf8(decode_base64(payload)?).or_else(|_| fail(INVALID, "the file name is not UTF-8"))
}

fn decompress(compression: u8, bytes: Vec<u8>) -> Result<Vec<u8>, Failure> {
    match compression {
        UNSET_LETTER => Ok(bytes),
        COMPRESSION_ZLIB => inflate_bounded(&bytes),
        _ => fail(INVALID, "unknown compression"),
    }
}

/// Inflates zlib data, refusing it as soon as it grows past `MAX_IMAGE_BYTES`: a few KiB can inflate to gigabytes.
fn inflate_bounded(bytes: &[u8]) -> Result<Vec<u8>, Failure> {
    const CORRUPT: &str = "the zlib data is corrupt or too large";
    // One byte past the limit tells data that is too large from data exactly at it.
    let limit = MAX_IMAGE_BYTES as u64 + 1;
    let mut inflated = Vec::new();
    ZlibDecoder::new(bytes)
        .take(limit)
        .read_to_end(&mut inflated)
        .or_else(|_| fail(INVALID, CORRUPT))?;
    if inflated.len() > MAX_IMAGE_BYTES {
        return fail(INVALID, CORRUPT);
    }
    Ok(inflated)
}

fn decode_format(command: &Command, bytes: &[u8]) -> Result<Decoded, Failure> {
    match command.format {
        UNSET_NUMBER | FORMAT_RGBA => decode_raw(command, bytes, RGBA_CHANNELS),
        FORMAT_RGB => decode_raw(command, bytes, RGB_CHANNELS),
        FORMAT_PNG => decode_png(bytes),
        _ => fail(INVALID, "unknown format"),
    }
}

fn rgba_length(width: u32, height: u32) -> usize {
    width as usize * height as usize * RGBA_CHANNELS
}

fn check_size(width: u32, height: u32) -> Result<(), Failure> {
    if width == 0 || height == 0 {
        return fail(INVALID, "the image has no size");
    }
    // The sides first: their product is computed only once both are known to be small.
    if width > MAX_IMAGE_SIDE || height > MAX_IMAGE_SIDE || rgba_length(width, height) > MAX_IMAGE_BYTES {
        return fail(
            TOO_LARGE,
            format!("the image is larger than {MAX_IMAGE_SIDE}x{MAX_IMAGE_SIDE}"),
        );
    }
    Ok(())
}

/// Raw pixels of `channels` bytes each, as large as `s` and `v` say; bytes past them are ignored, as in kitty.
fn decode_raw(command: &Command, bytes: &[u8], channels: usize) -> Result<Decoded, Failure> {
    let (width, height) = (command.width, command.height);
    check_size(width, height)?;
    let Some(pixels) = bytes.get(..width as usize * height as usize * channels) else {
        return fail(NO_DATA, "insufficient image data");
    };
    let rgba = if channels == RGBA_CHANNELS {
        pixels.to_vec()
    } else {
        rgb_to_rgba(pixels)
    };
    Ok(Decoded { width, height, rgba })
}

fn rgb_to_rgba(rgb: &[u8]) -> Vec<u8> {
    rgb.as_chunks::<RGB_CHANNELS>()
        .0
        .iter()
        .flat_map(|&[red, green, blue]| [red, green, blue, OPAQUE])
        .collect()
}

fn gray_alpha_to_rgba(gray_alpha: &[u8]) -> Vec<u8> {
    let pixels = gray_alpha.as_chunks::<GRAY_ALPHA_CHANNELS>().0;
    pixels
        .iter()
        .flat_map(|&[gray, alpha]| [gray, gray, gray, alpha])
        .collect()
}

fn gray_to_rgba(gray: &[u8]) -> Vec<u8> {
    gray.iter()
        .flat_map(|value| [*value, *value, *value, OPAQUE])
        .collect()
}

fn bad_png(error: png::DecodingError) -> Failure {
    Failure::new(INVALID, format!("bad PNG data: {error}"))
}

fn decode_png(bytes: &[u8]) -> Result<Decoded, Failure> {
    let mut decoder = png::Decoder::new_with_limits(
        Cursor::new(bytes),
        png::Limits {
            bytes: MAX_IMAGE_BYTES,
        },
    );
    // Palettes, low bit depths and 16-bit channels all come out as 8-bit channels.
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(bad_png)?;
    let (width, height) = (reader.info().width, reader.info().height);
    check_size(width, height)?;
    let buffer_size = reader
        .output_buffer_size()
        .ok_or_else(|| Failure::new(TOO_LARGE, "the PNG is too large"))?;
    let mut buffer = vec![0; buffer_size];
    let frame = reader.next_frame(&mut buffer).map_err(bad_png)?;
    buffer.truncate(frame.buffer_size());
    let rgba = png_to_rgba(frame.color_type, buffer)?;
    if rgba.len() != rgba_length(width, height) {
        return fail(INVALID, "bad PNG data");
    }
    Ok(Decoded { width, height, rgba })
}

fn png_to_rgba(color_type: png::ColorType, pixels: Vec<u8>) -> Result<Vec<u8>, Failure> {
    match color_type {
        png::ColorType::Rgba => Ok(pixels),
        png::ColorType::Rgb => Ok(rgb_to_rgba(&pixels)),
        png::ColorType::GrayscaleAlpha => Ok(gray_alpha_to_rgba(&pixels)),
        png::ColorType::Grayscale => Ok(gray_to_rgba(&pixels)),
        png::ColorType::Indexed => fail(INVALID, "unexpanded palette PNG"),
    }
}

/// The temporary folders as the file system names them, so a path through a link to one still counts.
fn temp_folders() -> Vec<PathBuf> {
    KNOWN_TEMP_FOLDERS
        .iter()
        .map(PathBuf::from)
        .chain([std::env::temp_dir()])
        .filter_map(|folder| folder.canonicalize().ok())
        .collect()
}

/// True for a file the protocol itself makes: kitty's rule, in a temporary folder with `tty-graphics-protocol` in
/// its path. Both are read from the file the path leads to, so neither `..` after the marker nor a link named
/// with it reaches another file.
fn is_protocol_temp_file(path: &Path) -> bool {
    let Ok(file) = path.canonicalize() else {
        return false;
    };
    file.to_string_lossy().contains(TEMP_FILE_MARKER)
        && temp_folders().iter().any(|folder| file.starts_with(folder))
}

/// Reads the bytes a `t=f` / `t=t` transmission names, and deletes a `t=t` file after. Only a file the protocol
/// itself makes is read. A program is output, possibly from another machine (ssh): with any path, the answers (no
/// such file, a folder, too large, read) would tell it which of the user's files exist and how big they are, so
/// every other path gets the same refusal before anything is looked at.
fn read_file(path: &str, command: &Command) -> Result<Vec<u8>, Failure> {
    let path = Path::new(path);
    if !path.is_absolute() || !is_protocol_temp_file(path) {
        return fail(NOT_PERMITTED, "only the protocol's own temporary files are read");
    }
    let length = length_to_read(path, command)?;
    let bytes = read_range(path, command.offset, length)?;
    if command.medium == MEDIUM_TEMP_FILE {
        remove_temp_file(path);
    }
    Ok(bytes)
}

/// The bytes to read: `S` of them from `O` on, or the rest of the file, refused past `MAX_TRANSFER`.
fn length_to_read(path: &Path, command: &Command) -> Result<u64, Failure> {
    let metadata =
        std::fs::metadata(path).or_else(|error| fail(NOT_FOUND, format!("cannot read the file: {error}")))?;
    // A pipe or a device would block the read or never end it.
    if !metadata.is_file() {
        return fail(BAD_FILE, "not a regular file");
    }
    let available = metadata.len().saturating_sub(u64::from(command.offset));
    let wanted = if command.size > 0 {
        u64::from(command.size).min(available)
    } else {
        available
    };
    if wanted > MAX_TRANSFER as u64 {
        return fail(TOO_LARGE, "the file is too large");
    }
    Ok(wanted)
}

fn read_range(path: &Path, offset: u32, length: u64) -> Result<Vec<u8>, Failure> {
    let mut file =
        File::open(path).or_else(|error| fail(NOT_FOUND, format!("cannot read the file: {error}")))?;
    file.seek(SeekFrom::Start(u64::from(offset)))
        .or_else(|error| fail(BAD_FILE, error.to_string()))?;
    let mut bytes = Vec::with_capacity(usize::try_from(length).unwrap_or(MAX_TRANSFER));
    file.take(length)
        .read_to_end(&mut bytes)
        .or_else(|error| fail(BAD_FILE, error.to_string()))?;
    Ok(bytes)
}

/// The picture was read, so a file left behind fails nothing: it is only told.
fn remove_temp_file(path: &Path) {
    if let Err(error) = std::fs::remove_file(path) {
        eprintln!("nekoland-terminal: could not delete {}: {error}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use base64::engine::general_purpose::STANDARD;
    use flate2::{write::ZlibEncoder, Compression};

    use super::*;

    fn b64(bytes: &[u8]) -> Vec<u8> {
        STANDARD.encode(bytes).into_bytes()
    }

    fn zlib(bytes: &[u8]) -> Vec<u8> {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(bytes).unwrap();
        encoder.finish().unwrap()
    }

    fn command(control: &str) -> Command {
        Command::parse(control.as_bytes()).unwrap()
    }

    fn failure_code(command: &Command, payload: &[u8]) -> &'static str {
        decode(command, payload).err().expect("a failure").code
    }

    fn reply_text(command: &Command, outcome: Result<(), &Failure>) -> Option<String> {
        reply(command, outcome).map(|bytes| String::from_utf8(bytes).unwrap())
    }

    #[test]
    fn control_keys_parse_with_kittys_defaults() {
        let parsed = command("a=T,f=24,s=2,v=1,i=7,p=3,z=-5,C=1,q=2,X=4");
        assert_eq!(
            (parsed.action, parsed.format, parsed.width, parsed.height),
            (b'T', 24, 2, 1)
        );
        assert_eq!(
            (
                parsed.id,
                parsed.placement,
                parsed.z,
                parsed.no_move,
                parsed.quiet,
                parsed.cell_x
            ),
            (7, 3, -5, true, 2, 4)
        );
        assert_eq!(command("").action, b't', "transmit is the default action");
        assert!(Command::parse(b"i=x").is_none());
        assert!(Command::parse(b"a=TT").is_none());
        assert!(Command::parse(b"i").is_none());
        assert_eq!(command("K=1,i=2").id, 2, "unknown keys are skipped");
    }

    #[test]
    fn raw_rgb_and_rgba_become_rgba() {
        let decoded = decode(&command("f=24,s=2,v=1"), &b64(&[1, 2, 3, 4, 5, 6])).unwrap();
        assert_eq!(decoded.rgba, [1, 2, 3, 255, 4, 5, 6, 255]);
        let rgba = command("f=32,s=1,v=1");
        assert_eq!(decode(&rgba, &b64(&[9, 8, 7, 6])).unwrap().rgba, [9, 8, 7, 6]);
        assert_eq!(failure_code(&rgba, &b64(&[9, 8])), "ENODATA");
        assert_eq!(
            failure_code(&command("f=32"), &b64(&[1, 2, 3, 4])),
            "EINVAL",
            "raw data needs s and v"
        );
    }

    #[test]
    fn the_source_rectangle_does_not_crop_the_decoded_image() {
        let decoded = decode(&command("f=32,s=2,v=1,x=1,w=1"), &b64(&[1, 2, 3, 4, 5, 6, 7, 8])).unwrap();
        assert_eq!((decoded.width, decoded.height), (2, 1));
    }

    #[test]
    fn zlib_and_png_payloads_decode() {
        let pixels = [10u8, 20, 30, 40, 50, 60, 70, 80];
        assert_eq!(
            decode(&command("f=32,s=2,v=1,o=z"), &b64(&zlib(&pixels)))
                .unwrap()
                .rgba,
            pixels
        );

        let mut png_bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut png_bytes, 2, 1);
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(png::BitDepth::Eight);
            encoder
                .write_header()
                .unwrap()
                .write_image_data(&[255, 0, 0, 0, 0, 255])
                .unwrap();
        }
        let decoded = decode(&command("f=100"), &b64(&png_bytes)).unwrap();
        assert_eq!((decoded.width, decoded.height), (2, 1));
        assert_eq!(decoded.rgba, [255, 0, 0, 255, 0, 0, 255, 255]);
    }

    #[test]
    fn sizes_past_the_limit_are_refused_before_anything_is_allocated() {
        assert_eq!(
            failure_code(&command("f=32,s=100000,v=100000"), &b64(&[0; 4])),
            "EFBIG"
        );
        // A zlib bomb: a few KiB that inflate past the limit.
        let bomb = zlib(&vec![0; MAX_IMAGE_BYTES + 1]);
        assert_eq!(
            failure_code(&command("f=32,s=4096,v=4096,o=z"), &b64(&bomb)),
            "EINVAL"
        );
    }

    #[test]
    fn only_the_protocols_own_temp_files_are_read_and_every_other_path_is_refused_alike() {
        let folder = std::env::temp_dir().join(format!("nekoland-kitty-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let other = folder.join("pixels.rgba");
        std::fs::write(&other, [1, 2, 3, 4]).unwrap();
        let mine = folder.join("tty-graphics-protocol-0.rgba");
        std::fs::write(&mine, [1, 2, 3, 4]).unwrap();
        let temp = folder.join("tty-graphics-protocol-1.rgba");
        std::fs::write(&temp, [5, 6, 7, 8]).unwrap();
        let link = folder.join("tty-graphics-protocol-link.rgba");
        std::os::unix::fs::symlink(&other, &link).unwrap();
        let path = |file: &Path| b64(file.to_str().unwrap().as_bytes());

        let file = command("f=32,s=1,v=1,t=f");
        assert_eq!(decode(&file, &path(&mine)).unwrap().rgba, [1, 2, 3, 4]);
        assert!(mine.exists(), "a file transmission leaves the file");
        let as_temp = command("f=32,s=1,v=1,t=t");
        assert_eq!(decode(&as_temp, &path(&temp)).unwrap().rgba, [5, 6, 7, 8]);
        assert!(
            !temp.exists(),
            "the protocol's own temp file is deleted after the read"
        );

        // A file that exists, one that does not, a folder, a relative path: one answer, nothing to tell apart. And
        // paths that name the marker on their way to another file: `..` after it, and a link named with it.
        let around = folder
            .join("tty-graphics-protocol")
            .join("..")
            .join("pixels.rgba");
        let refused = [
            path(&other),
            path(&folder.join("missing")),
            path(&folder),
            b64(b"relative.png"),
            path(&around),
            path(&link),
        ];
        for payload in refused {
            assert_eq!(failure_code(&file, &payload), "EPERM");
        }
        assert_eq!(failure_code(&as_temp, &path(&link)), "EPERM");
        assert!(
            other.exists() && link.exists(),
            "not deleted by a path that only names the marker"
        );
        assert_eq!(
            failure_code(&command("t=s,f=32,s=1,v=1"), &b64(b"name")),
            "EINVAL"
        );
        std::fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn replies_name_the_image_and_follow_the_quiet_levels() {
        let failure = Failure::new("ENOENT", "no such image");
        assert_eq!(
            reply_text(&command("a=T,i=31"), Ok(())).as_deref(),
            Some("\x1b_Gi=31;OK\x1b\\")
        );
        assert_eq!(
            reply_text(&command("a=T,i=1,q=1"), Ok(())),
            None,
            "q=1 drops the OK"
        );
        assert_eq!(
            reply_text(&command("a=p,i=99,q=1"), Err(&failure)).as_deref(),
            Some("\x1b_Gi=99;ENOENT:no such image\x1b\\"),
            "but keeps the errors"
        );
        assert_eq!(reply_text(&command("a=p,i=99,q=2"), Err(&failure)), None);
        assert_eq!(
            reply_text(&command("a=T"), Ok(())),
            None,
            "an anonymous picture is not answered"
        );
        assert_eq!(
            reply_text(&command("a=d,d=i,i=1"), Ok(())),
            None,
            "deletes are not answered"
        );
        let numbered = Command {
            id: 5,
            ..command("a=p,I=13,p=2")
        };
        assert_eq!(
            reply_text(&numbered, Ok(())).as_deref(),
            Some("\x1b_Gi=5,I=13,p=2;OK\x1b\\")
        );
        assert_eq!(
            reply_text(&command("a=t,I=13"), Ok(())).as_deref(),
            Some("\x1b_GI=13;OK\x1b\\")
        );
    }

    #[test]
    fn garbage_payloads_fail_without_panicking() {
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        let mut next_byte = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed as u8
        };
        for control in ["f=100", "f=32,s=3,v=3,o=z", "f=24,s=2,v=2"] {
            let parsed = command(control);
            for length in 0..64 {
                let bytes: Vec<u8> = (0..length * 7).map(|_| next_byte()).collect();
                let _ = decode(&parsed, &b64(&bytes));
                let _ = decode(&parsed, &bytes);
                let _ = Command::parse(&bytes);
            }
        }
    }
}
