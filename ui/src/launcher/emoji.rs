//! Emoji and the words they are found by, from the Unicode CLDR annotations Fcitx 5 installs:
//! zlib-compressed lists of each emoji and its words, in the order emoji are shown.

use std::io::Read;

const DICTS: &str = "/usr/share/fcitx5/emoji/data";

pub struct Emoji {
    pub glyph: String,
    /// Its words, the user's language's first; the first is its name.
    pub words: Vec<String>,
}

/// Every emoji, with the words of the user's language and English.
pub fn load(languages: &[String]) -> Vec<Emoji> {
    // zh_TW reads zh_Hant's; the plain language is a last resort.
    let mut names: Vec<String> = Vec::new();
    for language in languages {
        if language.starts_with("zh_TW") || language.starts_with("zh_HK") {
            names.push("zh_Hant".into());
        }
        names.push(language.clone());
    }
    names.push("en".into());
    names.dedup();

    let mut emoji: Vec<Emoji> = Vec::new();
    let mut index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for name in names {
        let path = format!("{DICTS}/{name}.dict");
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let Some(entries) = parse(&bytes) else {
            eprintln!("{path}: not an emoji list");
            continue;
        };
        for (glyph, words) in entries {
            match index.get(&glyph) {
                Some(&at) => emoji[at].words.extend(words),
                None => {
                    index.insert(glyph.clone(), emoji.len());
                    emoji.push(Emoji { glyph, words });
                }
            }
        }
    }
    emoji
}

/// A list: a little-endian u32 count of bytes, then zlib, of: a u32 count of emoji, then
/// for each, its UTF-8 as a u32 length and bytes, and a u32 count of words given likewise.
fn parse(bytes: &[u8]) -> Option<Vec<(String, Vec<String>)>> {
    let mut data = Vec::new();
    flate2::read::ZlibDecoder::new(bytes.get(4..)?).read_to_end(&mut data).ok()?;
    let mut reader = Reader { data: &data, at: 0 };
    let count = reader.number()?;
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        let glyph = reader.text()?;
        let mut words = (0..reader.number()?).map(|_| reader.text()).collect::<Option<Vec<String>>>()?;
        // The words come sorted; the emoji's name is among them, the longest.
        if let Some(name) = (0..words.len()).max_by_key(|&at| words[at].chars().count()) {
            let name = words.remove(name);
            words.insert(0, name);
        }
        entries.push((glyph, words));
    }
    Some(entries)
}

struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn number(&mut self) -> Option<usize> {
        let value = u32::from_le_bytes(self.data.get(self.at..self.at + 4)?.try_into().ok()?) as usize;
        self.at += 4;
        Some(value)
    }

    fn text(&mut self) -> Option<String> {
        let length = self.number()?;
        let text = std::str::from_utf8(self.data.get(self.at..self.at + length)?).ok()?;
        self.at += length;
        Some(text.to_string())
    }
}
