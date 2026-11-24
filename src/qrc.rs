//! QQ Music's word-timed lyrics (QRC): hex of a zlib stream, masked by Triple DES with a
//! key QQ fixed, in a DES that is the standard one but for two entries of its S-boxes, for
//! where its key schedule takes half the bits of each key from, and for the bytes of each
//! four of a block, and of the key, being taken in the opposite order.
//! None of it is secret, as the key and the tables are in every player that reads them.

use std::io::Read;

use crate::lyrics::Line;

const KEY: &[u8; 24] = b"!@#)(*$%123ZXC!@!@#)(NHL";

const IP: [u8; 64] = [
    58, 50, 42, 34, 26, 18, 10, 2, 60, 52, 44, 36, 28, 20, 12, 4, 62, 54, 46, 38, 30, 22, 14, 6, 64, 56, 48,
    40, 32, 24, 16, 8, 57, 49, 41, 33, 25, 17, 9, 1, 59, 51, 43, 35, 27, 19, 11, 3, 61, 53, 45, 37, 29, 21,
    13, 5, 63, 55, 47, 39, 31, 23, 15, 7,
];
const FP: [u8; 64] = [
    40, 8, 48, 16, 56, 24, 64, 32, 39, 7, 47, 15, 55, 23, 63, 31, 38, 6, 46, 14, 54, 22, 62, 30, 37, 5, 45,
    13, 53, 21, 61, 29, 36, 4, 44, 12, 52, 20, 60, 28, 35, 3, 43, 11, 51, 19, 59, 27, 34, 2, 42, 10, 50, 18,
    58, 26, 33, 1, 41, 9, 49, 17, 57, 25,
];
const E: [u8; 48] = [
    32, 1, 2, 3, 4, 5, 4, 5, 6, 7, 8, 9, 8, 9, 10, 11, 12, 13, 12, 13, 14, 15, 16, 17, 16, 17, 18, 19, 20,
    21, 20, 21, 22, 23, 24, 25, 24, 25, 26, 27, 28, 29, 28, 29, 30, 31, 32, 1,
];
const P: [u8; 32] = [
    16, 7, 20, 21, 29, 12, 28, 17, 1, 15, 23, 26, 5, 18, 31, 10, 2, 8, 24, 14, 32, 27, 3, 9, 19, 13, 30, 6,
    22, 11, 4, 25,
];
const PC1: [u8; 56] = [
    57, 49, 41, 33, 25, 17, 9, 1, 58, 50, 42, 34, 26, 18, 10, 2, 59, 51, 43, 35, 27, 19, 11, 3, 60, 52, 44,
    36, 63, 55, 47, 39, 31, 23, 15, 7, 62, 54, 46, 38, 30, 22, 14, 6, 61, 53, 45, 37, 29, 21, 13, 5, 28, 20,
    12, 4,
];
const PC2: [u8; 48] = [
    14, 17, 11, 24, 1, 5, 3, 28, 15, 6, 21, 10, 23, 19, 12, 4, 26, 8, 16, 7, 27, 20, 13, 2, 41, 52, 31, 37,
    47, 55, 30, 40, 51, 45, 33, 48, 44, 49, 39, 56, 34, 53, 46, 42, 50, 36, 29, 32,
];
const SHIFTS: [u8; 16] = [1, 1, 2, 2, 2, 2, 2, 2, 1, 2, 2, 2, 2, 2, 2, 1];

/// The eight S-boxes, each as four rows of sixteen, with QQ's two changes.
const SBOX: [[u8; 64]; 8] = [
    [
        14, 4, 13, 1, 2, 15, 11, 8, 3, 10, 6, 12, 5, 9, 0, 7, 0, 15, 7, 4, 14, 2, 13, 1, 10, 6, 12, 11, 9, 5,
        3, 8, 4, 1, 14, 8, 13, 6, 2, 11, 15, 12, 9, 7, 3, 10, 5, 0, 15, 12, 8, 2, 4, 9, 1, 7, 5, 11, 3, 14,
        10, 0, 6, 13,
    ],
    [
        15, 1, 8, 14, 6, 11, 3, 4, 9, 7, 2, 13, 12, 0, 5, 10, 3, 13, 4, 7, 15, 2, 8, 15, 12, 0, 1, 10, 6, 9,
        11, 5, 0, 14, 7, 11, 10, 4, 13, 1, 5, 8, 12, 6, 9, 3, 2, 15, 13, 8, 10, 1, 3, 15, 4, 2, 11, 6, 7, 12,
        0, 5, 14, 9,
    ],
    [
        10, 0, 9, 14, 6, 3, 15, 5, 1, 13, 12, 7, 11, 4, 2, 8, 13, 7, 0, 9, 3, 4, 6, 10, 2, 8, 5, 14, 12, 11,
        15, 1, 13, 6, 4, 9, 8, 15, 3, 0, 11, 1, 2, 12, 5, 10, 14, 7, 1, 10, 13, 0, 6, 9, 8, 7, 4, 15, 14, 3,
        11, 5, 2, 12,
    ],
    [
        7, 13, 14, 3, 0, 6, 9, 10, 1, 2, 8, 5, 11, 12, 4, 15, 13, 8, 11, 5, 6, 15, 0, 3, 4, 7, 2, 12, 1, 10,
        14, 9, 10, 6, 9, 0, 12, 11, 7, 13, 15, 1, 3, 14, 5, 2, 8, 4, 3, 15, 0, 6, 10, 10, 13, 8, 9, 4, 5, 11,
        12, 7, 2, 14,
    ],
    [
        2, 12, 4, 1, 7, 10, 11, 6, 8, 5, 3, 15, 13, 0, 14, 9, 14, 11, 2, 12, 4, 7, 13, 1, 5, 0, 15, 10, 3, 9,
        8, 6, 4, 2, 1, 11, 10, 13, 7, 8, 15, 9, 12, 5, 6, 3, 0, 14, 11, 8, 12, 7, 1, 14, 2, 13, 6, 15, 0, 9,
        10, 4, 5, 3,
    ],
    [
        12, 1, 10, 15, 9, 2, 6, 8, 0, 13, 3, 4, 14, 7, 5, 11, 10, 15, 4, 2, 7, 12, 9, 5, 6, 1, 13, 14, 0, 11,
        3, 8, 9, 14, 15, 5, 2, 8, 12, 3, 7, 0, 4, 10, 1, 13, 11, 6, 4, 3, 2, 12, 9, 5, 15, 10, 11, 14, 1, 7,
        6, 0, 8, 13,
    ],
    [
        4, 11, 2, 14, 15, 0, 8, 13, 3, 12, 9, 7, 5, 10, 6, 1, 13, 0, 11, 7, 4, 9, 1, 10, 14, 3, 5, 12, 2, 15,
        8, 6, 1, 4, 11, 13, 12, 3, 7, 14, 10, 15, 6, 8, 0, 5, 9, 2, 6, 11, 13, 8, 1, 4, 10, 7, 9, 5, 0, 15,
        14, 2, 3, 12,
    ],
    [
        13, 2, 8, 4, 6, 15, 11, 1, 10, 9, 3, 14, 5, 0, 12, 7, 1, 15, 13, 8, 10, 3, 7, 4, 12, 5, 6, 11, 0, 14,
        9, 2, 7, 11, 4, 1, 9, 12, 14, 2, 0, 6, 10, 13, 15, 3, 5, 8, 2, 1, 14, 7, 4, 10, 8, 13, 15, 12, 9, 0,
        3, 5, 6, 11,
    ],
];

/// Bits of `value`, an `from`-bit number, picked by a table of 1-based positions from its top.
fn permute(value: u64, table: &[u8], from: u32) -> u64 {
    table
        .iter()
        .fold(0, |out, &at| (out << 1) | ((value >> (from - u32::from(at))) & 1))
}

/// The sixteen 48-bit keys of one eight-byte key, in the order they are used to decrypt.
fn subkeys(key: [u8; 8]) -> [u64; 16] {
    let permuted = permute(u64::from_be_bytes(key), &PC1, 64);
    let (mut c, mut d) = ((permuted >> 28) as u32, (permuted & 0x0fff_ffff) as u32);
    let rotate = |half: u32, by: u8| ((half << by) | (half >> (28 - by))) & 0x0fff_ffff;
    let mut keys = [0; 16];
    for (round, key) in keys.iter_mut().enumerate() {
        c = rotate(c, SHIFTS[round]);
        d = rotate(d, SHIFTS[round]);
        // QQ's key schedule takes the bits for the second half of each key from one place after
        // where DES does, the last of them from beyond the end, which is nothing.
        let cd = (u64::from(c) << 28) | u64::from(d);
        *key = PC2.iter().enumerate().fold(0, |out, (at, &position)| {
            let position = if at < 24 {
                u32::from(position)
            } else {
                u32::from(position) + 1
            };
            (out << 1)
                | if position > 56 {
                    0
                } else {
                    (cd >> (56 - position)) & 1
                }
        });
    }
    keys
}

fn round(r: u32, key: u64) -> u32 {
    let mixed = permute(u64::from(r), &E, 32) ^ key;
    let mut out = 0u32;
    for (index, sbox) in SBOX.iter().enumerate() {
        let six = ((mixed >> (42 - 6 * index)) & 0x3f) as usize;
        let row = ((six & 0x20) >> 4) | (six & 1);
        out = (out << 4) | u32::from(sbox[row * 16 + ((six >> 1) & 0xf)]);
    }
    permute(u64::from(out), &P, 32) as u32
}

/// One block through DES with the keys in the order given.
fn block(value: u64, keys: impl Iterator<Item = u64>) -> u64 {
    let ip = permute(value, &IP, 64);
    let (mut l, mut r) = ((ip >> 32) as u32, ip as u32);
    for key in keys {
        (l, r) = (r, l ^ round(r, key));
    }
    permute((u64::from(r) << 32) | u64::from(l), &FP, 64)
}

/// The bytes of each four of eight, the other way round, as QQ's DES takes them.
fn swapped(bytes: [u8; 8]) -> [u8; 8] {
    [
        bytes[3], bytes[2], bytes[1], bytes[0], bytes[7], bytes[6], bytes[5], bytes[4],
    ]
}

/// What QQ's Triple DES makes plain of `data`, of whole blocks.
fn unlock(data: &[u8]) -> Vec<u8> {
    let key = |at: usize| {
        let part: [u8; 8] = KEY[at..at + 8].try_into().unwrap_or_default();
        subkeys(swapped(part))
    };
    let (first, second, third) = (key(0), key(8), key(16));
    let mut out = Vec::with_capacity(data.len());
    for chunk in data.chunks_exact(8) {
        let bytes: [u8; 8] = chunk.try_into().unwrap_or_default();
        let mut value = u64::from_be_bytes(swapped(bytes));
        // Decrypting with the last key, encrypting with the middle, decrypting with the first.
        value = block(value, third.iter().rev().copied());
        value = block(value, second.iter().copied());
        value = block(value, first.iter().rev().copied());
        out.extend_from_slice(&swapped(value.to_be_bytes()));
    }
    out
}

/// The XML a QRC's hex holds.
pub fn decode(hex: &str) -> Option<String> {
    let hex = hex.trim();
    if hex.len() % 2 != 0 {
        return None;
    }
    let bytes: Option<Vec<u8>> = (0..hex.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(hex.get(at..at + 2)?, 16).ok())
        .collect();
    let mut text = String::new();
    flate2::read::ZlibDecoder::new(unlock(&bytes?).as_slice())
        .read_to_string(&mut text)
        .ok()?;
    Some(text)
}

/// The lines of a QRC's XML: `[start,length]word(start,length)word(start,length)…`, in
/// milliseconds, each word's start being from the song's.
pub fn parse(xml: &str) -> Vec<Line> {
    let content = xml
        .split("LyricContent=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap_or(xml);
    let content = content
        .replace("&#10;", "\n")
        .replace("&amp;", "&")
        .replace("&quot;", "\"");
    let mut lines = Vec::new();
    for raw in content.lines() {
        let Some((head, body)) = raw.strip_prefix('[').and_then(|rest| rest.split_once(']')) else {
            continue;
        };
        let Some(start) = head.split(',').next().and_then(|n| n.parse::<f64>().ok()) else {
            continue;
        };
        if !head.contains(',') {
            continue;
        }
        let mut words: Vec<(f64, String)> = Vec::new();
        let mut text = String::new();
        let mut rest = body;
        // Each word is its characters then its "(start,length)".
        while let Some(open) = rest.find('(') {
            let Some(close) = rest[open..].find(')') else {
                break;
            };
            let stamp: Vec<&str> = rest[open + 1..open + close].split(',').collect();
            let times = (
                stamp.first().and_then(|n| n.parse::<f64>().ok()),
                stamp.get(1).and_then(|n| n.parse::<f64>().ok()),
            );
            let (Some(at), Some(length), 2) = (times.0, times.1, stamp.len()) else {
                // A "(" of the words' own.
                text.push_str(&rest[..=open]);
                rest = &rest[open + 1..];
                continue;
            };
            let said = format!("{text}{}", &rest[..open]);
            text.clear();
            let count = said.chars().count().max(1) as f64;
            for (index, c) in said.chars().enumerate() {
                words.push(((at + length * index as f64 / count) / 1000.0, c.to_string()));
            }
            rest = &rest[open + close + 1..];
        }
        let skip = words.iter().take_while(|(_, c)| c.trim().is_empty()).count();
        words.drain(..skip);
        while words.last().is_some_and(|(_, c)| c.trim().is_empty()) {
            words.pop();
        }
        let sung: String = words.iter().map(|(_, c)| c.as_str()).collect();
        if !sung.is_empty() {
            lines.push(Line {
                time: start / 1000.0,
                text: sung,
                words: Some(words),
            });
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unlocks_the_head_of_a_zlib_stream() {
        // The first block of a QRC from QQ Music: what a zlib stream begins with is what it makes.
        let block = [0xf7, 0xf1, 0xd1, 0x21, 0x62, 0x29, 0xa6, 0x63];
        assert_eq!(unlock(&block), [0x78, 0x9c, 0x6d, 0x99, 0x5b, 0xcf, 0x5d, 0xd5]);
    }

    #[test]
    fn reads_words_with_their_own_times() {
        let xml = r#"<Lyric_1 LyricType="1" LyricContent="[0,500]作词 : x(0,500)&#10;[12000,2500]He(12000,300)llo(12300,400)世界(12700,800)"/>"#;
        let lines = parse(xml);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1].text, "Hello世界");
        let words = lines[1].words.as_ref().unwrap();
        assert!((words[2].0 - 12.3).abs() < 1e-9);
        assert!((words[5].0 - 12.7).abs() < 1e-9);
        assert!((words[6].0 - 13.1).abs() < 1e-9);
    }
}
