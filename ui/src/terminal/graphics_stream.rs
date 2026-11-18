//! Takes the picture sequences out of what a program writes, before the screen sees it: kitty's `APC G … ST` and
//! Sixel's `DCS … q … ST`, which VTE (built without Sixel) would drop, and XTSMGRAPHICS, which it does not answer.
//! Everything else goes on as it came, other `DCS` strings such as DECRQSS among it. The filter keeps its place
//! between reads, as a picture spans many.

use super::{kitty_graphics::MAX_TRANSFER, sixel};

const ESC: u8 = 0x1b;
/// CAN and SUB end a sequence where they stand.
const CANCEL: u8 = 0x18;
const SUBSTITUTE: u8 = 0x1a;
const APC: u8 = b'_';
const DCS: u8 = b'P';
const CSI: u8 = b'[';
const STRING_TERMINATOR: u8 = b'\\';
const KITTY_GRAPHICS: u8 = b'G';
const SIXEL_FINAL: u8 = b'q';
/// XTSMGRAPHICS: `CSI ? Pi ; Pa ; Pv S`.
const GRAPHICS_ATTRIBUTES_FINAL: u8 = b'S';
const PRIVATE_MARKER: u8 = b'?';
/// The longest DCS introducer kept to tell a Sixel from another string, and the longest CSI kept.
const INTRODUCER_LIMIT: usize = 64;

/// A piece of what a program wrote, in the order it wrote it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Piece {
    /// For the screen.
    Text(Vec<u8>),
    /// A kitty graphics command, without its `G`: the control keys, `;`, the payload.
    Kitty(Vec<u8>),
    /// A Sixel picture: its parameters, the `q` and the body.
    Sixel(Vec<u8>),
    /// An XTSMGRAPHICS question, its parameters after the `?`.
    GraphicsAttributes(String),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Mode {
    #[default]
    Ground,
    Escape,
    /// After `ESC _`: a `G` makes it a picture.
    ApcStart,
    Kitty,
    KittyEscape,
    /// In a DCS before its final byte, which tells a Sixel.
    DcsIntroducer,
    Sixel,
    SixelEscape,
    /// Another string, passed on to its end.
    PassString,
    PassStringEscape,
    /// A CSI, passed on and kept for XTSMGRAPHICS.
    Csi,
    /// A picture too large to keep, dropped to its end.
    Drop,
    DropEscape,
}

#[derive(Default)]
pub struct GraphicsFilter {
    mode: Mode,
    kept: Vec<u8>,
    pieces: Vec<Piece>,
}

impl GraphicsFilter {
    /// `bytes` in pieces: the text for the screen, and the pictures and questions between it.
    pub fn split(&mut self, bytes: &[u8]) -> Vec<Piece> {
        for &byte in bytes {
            self.mode = self.step(byte);
        }
        std::mem::take(&mut self.pieces)
    }

    fn step(&mut self, byte: u8) -> Mode {
        match self.mode {
            Mode::Ground => self.ground(byte),
            Mode::Escape => self.escape(byte),
            Mode::ApcStart => self.apc_start(byte),
            Mode::Kitty => self.collect(byte, Mode::Kitty, Mode::KittyEscape),
            Mode::KittyEscape => self.finish(byte, Piece::Kitty),
            Mode::DcsIntroducer => self.dcs_introducer(byte),
            Mode::Sixel => self.collect(byte, Mode::Sixel, Mode::SixelEscape),
            Mode::SixelEscape => self.finish(byte, Piece::Sixel),
            Mode::PassString => self.pass_string(byte),
            Mode::PassStringEscape => {
                self.text(&[byte]);
                if byte == STRING_TERMINATOR {
                    Mode::Ground
                } else {
                    Mode::PassString
                }
            }
            Mode::Csi => self.csi(byte),
            Mode::Drop => match byte {
                ESC => Mode::DropEscape,
                CANCEL | SUBSTITUTE => Mode::Ground,
                _ => Mode::Drop,
            },
            Mode::DropEscape if byte == STRING_TERMINATOR => Mode::Ground,
            Mode::DropEscape => Mode::Drop,
        }
    }

    fn ground(&mut self, byte: u8) -> Mode {
        if byte == ESC {
            return Mode::Escape;
        }
        self.text(&[byte]);
        Mode::Ground
    }

    /// The byte after an ESC: the strings and CSI this filter follows, or anything else passed on with its ESC.
    fn escape(&mut self, byte: u8) -> Mode {
        self.kept.clear();
        match byte {
            APC => Mode::ApcStart,
            DCS => Mode::DcsIntroducer,
            CSI => {
                self.text(&[ESC, CSI]);
                Mode::Csi
            }
            ESC => {
                self.text(&[ESC]);
                Mode::Escape
            }
            _ => {
                self.text(&[ESC, byte]);
                Mode::Ground
            }
        }
    }

    fn apc_start(&mut self, byte: u8) -> Mode {
        if byte == KITTY_GRAPHICS {
            return Mode::Kitty;
        }
        self.text(&[ESC, APC]);
        self.pass_string(byte)
    }

    /// A picture's bytes up to its ESC, or the whole picture dropped when it grows past what is kept.
    fn collect(&mut self, byte: u8, inside: Mode, escape: Mode) -> Mode {
        match byte {
            ESC => escape,
            CANCEL | SUBSTITUTE => Mode::Ground,
            _ if self.kept.len() >= MAX_TRANSFER => {
                self.kept.clear();
                Mode::Drop
            }
            _ => {
                self.kept.push(byte);
                inside
            }
        }
    }

    /// The byte after an ESC inside a picture: ST ends it; anything else cuts it off and starts a new sequence.
    fn finish(&mut self, byte: u8, piece: fn(Vec<u8>) -> Piece) -> Mode {
        let kept = std::mem::take(&mut self.kept);
        if byte == STRING_TERMINATOR {
            self.pieces.push(piece(kept));
            return Mode::Ground;
        }
        self.escape(byte)
    }

    fn dcs_introducer(&mut self, byte: u8) -> Mode {
        if byte == SIXEL_FINAL {
            self.kept.push(byte);
            if sixel::is_sixel(&self.kept) {
                return Mode::Sixel;
            }
            self.kept.pop();
        }
        if (0x40..=0x7e).contains(&byte) || self.kept.len() >= INTRODUCER_LIMIT {
            let introducer = std::mem::take(&mut self.kept);
            self.text(&[ESC, DCS]);
            self.text(&introducer);
            self.text(&[byte]);
            return Mode::PassString;
        }
        self.kept.push(byte);
        Mode::DcsIntroducer
    }

    fn pass_string(&mut self, byte: u8) -> Mode {
        self.text(&[byte]);
        match byte {
            ESC => Mode::PassStringEscape,
            CANCEL | SUBSTITUTE => Mode::Ground,
            _ => Mode::PassString,
        }
    }

    /// A CSI goes on to the screen whole; XTSMGRAPHICS is also told apart, as VTE leaves it unanswered.
    fn csi(&mut self, byte: u8) -> Mode {
        // An ESC cuts the CSI off and starts the next sequence, as on the screen.
        if byte == ESC {
            self.kept.clear();
            return Mode::Escape;
        }
        self.text(&[byte]);
        if matches!(byte, CANCEL | SUBSTITUTE) {
            self.kept.clear();
            return Mode::Ground;
        }
        if !(0x40..=0x7e).contains(&byte) {
            if self.kept.len() < INTRODUCER_LIMIT {
                self.kept.push(byte);
            }
            return Mode::Csi;
        }
        if byte == GRAPHICS_ATTRIBUTES_FINAL {
            if let Some(parameters) = self.kept.strip_prefix(&[PRIVATE_MARKER]) {
                let parameters = String::from_utf8_lossy(parameters).into_owned();
                self.pieces.push(Piece::GraphicsAttributes(parameters));
            }
        }
        self.kept.clear();
        Mode::Ground
    }

    fn text(&mut self, bytes: &[u8]) {
        match self.pieces.last_mut() {
            Some(Piece::Text(text)) => text.extend_from_slice(bytes),
            _ => self.pieces.push(Piece::Text(bytes.to_vec())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(chunks: &[&[u8]]) -> Vec<Piece> {
        let mut filter = GraphicsFilter::default();
        chunks.iter().flat_map(|chunk| filter.split(chunk)).collect()
    }

    #[test]
    fn a_kitty_command_is_taken_out_of_the_text() {
        let pieces = split(&[b"ab\x1b_Ga=T,f=100;AAAA\x1b\\cd"]);
        assert_eq!(
            pieces,
            [
                Piece::Text(b"ab".to_vec()),
                Piece::Kitty(b"a=T,f=100;AAAA".to_vec()),
                Piece::Text(b"cd".to_vec())
            ]
        );
    }

    #[test]
    fn a_picture_split_across_reads_is_kept_whole() {
        let pieces = split(&[b"\x1bPq#0;2;0;0;0", b"~~\x1b", b"\\x"]);
        assert_eq!(pieces, [Piece::Sixel(b"q#0;2;0;0;0~~".to_vec()), Piece::Text(b"x".to_vec())]);
    }

    #[test]
    fn other_strings_and_sequences_pass_on_unchanged() {
        let request_status = b"\x1bP$qm\x1b\\\x1b[1;31mred\x1b_other\x1b\\\x1b]0;title\x07";
        let pieces = split(&[request_status]);
        assert_eq!(pieces, [Piece::Text(request_status.to_vec())]);
    }

    #[test]
    fn xtsmgraphics_is_told_apart_and_still_passed_on() {
        let pieces = split(&[b"\x1b[?1;1;0S"]);
        assert_eq!(pieces, [Piece::Text(b"\x1b[?1;1;0S".to_vec()), Piece::GraphicsAttributes("1;1;0".into())]);
    }

    #[test]
    fn a_sixel_with_parameters_is_a_picture() {
        let pieces = split(&[b"\x1bP0;1;0q\"1;1;2;2#1~\x1b\\"]);
        assert_eq!(pieces, [Piece::Sixel(b"0;1;0q\"1;1;2;2#1~".to_vec())]);
    }
}
