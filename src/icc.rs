//! A display's ICC profile, as far as the gamma ramps can carry it: its calibration curves,
//! the `vcgt` tag colord and DisplayCAL write. The rest of a profile, which converts colours
//! between spaces, needs colour management in the renderer, which Nekoland does not have.

/// The red, green and blue curves, each taking 0–1 to 0–1.
#[derive(Debug, PartialEq)]
pub enum Curves {
    /// Evenly spaced values from 0 to 1.
    Table([Vec<f64>; 3]),
    /// gamma, min and max, as min + (max − min) · x^gamma.
    Formula([(f64, f64, f64); 3]),
}

impl Curves {
    /// Channel `channel`'s value at `x`.
    pub fn at(&self, channel: usize, x: f64) -> f64 {
        match self {
            Curves::Table(tables) => {
                let table = &tables[channel];
                let place = x.clamp(0.0, 1.0) * (table.len() - 1) as f64;
                let (below, above) = (place.floor() as usize, place.ceil() as usize);
                table[below] + (table[above] - table[below]) * (place - below as f64)
            }
            Curves::Formula(formulas) => {
                let (gamma, min, max) = formulas[channel];
                min + (max - min) * x.clamp(0.0, 1.0).powf(gamma)
            }
        }
    }
}

/// The curves in the profile at `path`, or None when it has none or is not a profile.
pub fn load(path: &str) -> Option<Curves> {
    match std::fs::read(path) {
        Ok(bytes) => parse(&bytes).or_else(|| {
            tracing::warn!("colour profile {path}: no calibration curves");
            None
        }),
        Err(err) => {
            tracing::warn!("colour profile {path}: {err}");
            None
        }
    }
}

fn parse(bytes: &[u8]) -> Option<Curves> {
    let u32_at = |at: usize| Some(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?));
    if bytes.get(36..40)? != b"acsp" {
        return None;
    }
    let count = u32_at(128)? as usize;
    let tag = (0..count.min(1024)).find_map(|index| {
        let entry = 132 + index * 12;
        (bytes.get(entry..entry + 4)? == b"vcgt").then(|| Some((u32_at(entry + 4)? as usize, u32_at(entry + 8)? as usize)))?
    })?;
    let (offset, size) = tag;
    let tag = bytes.get(offset..offset.checked_add(size)?)?;
    let u32_in = |at: usize| Some(u32::from_be_bytes(tag.get(at..at + 4)?.try_into().ok()?));
    let u16_in = |at: usize| Some(u16::from_be_bytes(tag.get(at..at + 2)?.try_into().ok()?));
    match u32_in(8)? {
        0 => {
            let (channels, entries, width) = (u16_in(12)? as usize, u16_in(14)? as usize, u16_in(16)? as usize);
            if !(channels == 1 || channels == 3) || entries < 2 || !(width == 1 || width == 2) {
                return None;
            }
            let full = if width == 1 { 255.0 } else { 65535.0 };
            let read = |channel: usize| -> Option<Vec<f64>> {
                (0..entries)
                    .map(|index| {
                        let at = 18 + (channel * entries + index) * width;
                        let value = if width == 1 { f64::from(*tag.get(at)?) } else { f64::from(u16_in(at)?) };
                        Some(value / full)
                    })
                    .collect()
            };
            let red = read(0)?;
            let (green, blue) = if channels == 3 { (read(1)?, read(2)?) } else { (red.clone(), red.clone()) };
            Some(Curves::Table([red, green, blue]))
        }
        1 => {
            let fixed = |at: usize| Some(f64::from(u32_in(at)? as i32) / 65536.0);
            let channel = |first: usize| Some((fixed(first)?, fixed(first + 4)?, fixed(first + 8)?));
            Some(Curves::Formula([channel(12)?, channel(24)?, channel(36)?]))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A profile of only a header and the one tag given.
    fn profile(tag: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0; 128];
        bytes[36..40].copy_from_slice(b"acsp");
        bytes.extend(1u32.to_be_bytes());
        let offset = 128 + 4 + 12;
        bytes.extend(b"vcgt");
        bytes.extend((offset as u32).to_be_bytes());
        bytes.extend((tag.len() as u32).to_be_bytes());
        bytes.extend(tag);
        bytes
    }

    #[test]
    fn a_table_is_read_and_interpolated() {
        let mut tag = b"vcgt\0\0\0\0".to_vec();
        tag.extend(0u32.to_be_bytes());
        tag.extend([0, 3, 0, 2, 0, 2]);
        for value in [0u16, 65535, 0, 32768, 0, 16384] {
            tag.extend(value.to_be_bytes());
        }
        let curves = parse(&profile(&tag)).unwrap();
        assert_eq!(curves.at(0, 1.0), 1.0);
        assert!((curves.at(1, 0.5) - 0.25).abs() < 1e-3);
        assert!((curves.at(2, 1.0) - 0.25).abs() < 1e-3);
    }

    #[test]
    fn a_formula_is_read() {
        let mut tag = b"vcgt\0\0\0\0".to_vec();
        tag.extend(1u32.to_be_bytes());
        for (gamma, min, max) in [(2.0, 0.0, 1.0), (1.0, 0.1, 0.9), (1.0, 0.0, 0.5)] {
            for value in [gamma, min, max] {
                tag.extend(((value * 65536.0) as i32 as u32).to_be_bytes());
            }
        }
        let curves = parse(&profile(&tag)).unwrap();
        assert!((curves.at(0, 0.5) - 0.25).abs() < 1e-3);
        assert!((curves.at(1, 0.0) - 0.1).abs() < 1e-3);
        assert!((curves.at(2, 1.0) - 0.5).abs() < 1e-3);
    }

    #[test]
    fn what_is_not_a_profile_has_no_curves() {
        assert_eq!(parse(b"not a profile"), None);
        assert_eq!(parse(&profile(b"vcgt")), None);
    }
}
