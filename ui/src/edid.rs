//! What a display is called: the name its EDID gives, else how it is connected. Settings'
//! Displays page and the screen-share picker name displays alike.

use std::path::{Path, PathBuf};

const DRM: &str = "/sys/class/drm";
/// The connector kinds of a panel built into the computer.
pub const BUILT_IN: [&str; 3] = ["eDP", "LVDS", "DSI"];
const VIRTUAL: &str = "Virtual";
/// Where EDID's display descriptors are, each this long, and the kind and text of the one
/// that names the display.
const EDID_DESCRIPTORS: [usize; 4] = [54, 72, 90, 108];
const EDID_DESCRIPTOR_LENGTH: usize = 18;
const EDID_NAME_TAG: u8 = 0xFC;
const EDID_TEXT_START: usize = 5;
/// Where EDID keeps the picture's width and height, in centimetres.
const EDID_WIDTH_CM: usize = 21;
const EDID_HEIGHT_CM: usize = 22;
const CM_AN_INCH: f64 = 2.54;

/// The connected display's folder in sysfs, as `card1-HDMI-A-1` for `HDMI-A-1`; the first
/// connected one for none.
pub fn connector(name: Option<&str>) -> Option<PathBuf> {
    std::fs::read_dir(DRM).ok()?.flatten().map(|entry| entry.path()).find(|path| {
        path.join("status").is_file()
            && name.is_none_or(|name| {
                path.file_name().and_then(|file| file.to_str()).is_some_and(|file| file.ends_with(&format!("-{name}")))
            })
            && std::fs::read_to_string(path.join("status")).is_ok_and(|status| status.trim() == "connected")
    })
}

/// A display's name and size, from its EDID; a display that gives none, as a virtual
/// machine's, is named by how it is connected.
pub fn identity(connector: &Path) -> (String, Option<String>) {
    let edid = std::fs::read(connector.join("edid")).unwrap_or_default();
    let name = edid_name(&edid).unwrap_or_else(|| kind_name(connector).into());
    let size = edid_inches(&edid).map(|inches| format!("{inches:.0} 吋"));
    (name, size)
}

/// `card1-eDP-1` is an eDP panel's.
fn kind_name(connector: &Path) -> &'static str {
    let name = connector.file_name().and_then(|name| name.to_str()).unwrap_or_default();
    let kind = name
        .split_once('-')
        .map_or("", |(_, rest)| rest.rsplit_once('-').map_or(rest, |(kind, _)| kind));
    if BUILT_IN.contains(&kind) {
        "內建顯示器"
    } else if kind == VIRTUAL {
        "虛擬顯示器"
    } else {
        "外接顯示器"
    }
}

fn edid_name(edid: &[u8]) -> Option<String> {
    EDID_DESCRIPTORS.iter().find_map(|&start| {
        let descriptor = edid.get(start..start + EDID_DESCRIPTOR_LENGTH)?;
        if descriptor[..3] != [0, 0, 0] || descriptor[3] != EDID_NAME_TAG {
            return None;
        }
        let text = &descriptor[EDID_TEXT_START..];
        let end = text.iter().position(|byte| *byte == b'\n').unwrap_or(text.len());
        let name = String::from_utf8_lossy(&text[..end]).trim().to_string();
        (!name.is_empty()).then_some(name)
    })
}

fn edid_inches(edid: &[u8]) -> Option<f64> {
    let (width, height) = (*edid.get(EDID_WIDTH_CM)?, *edid.get(EDID_HEIGHT_CM)?);
    (width > 0 && height > 0).then(|| f64::from(width).hypot(f64::from(height)) / CM_AN_INCH)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_name_comes_from_the_edid_else_the_connector() {
        let mut edid = vec![0u8; 128];
        edid[EDID_WIDTH_CM] = 60;
        edid[EDID_HEIGHT_CM] = 34;
        edid[72..77].copy_from_slice(&[0, 0, 0, EDID_NAME_TAG, 0]);
        edid[77..90].copy_from_slice(b"DELL U2723Q\n ");
        assert_eq!(edid_name(&edid).as_deref(), Some("DELL U2723Q"));
        assert_eq!(edid_inches(&edid).map(|inches| inches.round()), Some(27.0));
        assert_eq!(edid_name(&[]), None);
        assert_eq!(kind_name(Path::new("/sys/class/drm/card0-eDP-1")), "內建顯示器");
        assert_eq!(kind_name(Path::new("/sys/class/drm/card1-Virtual-1")), "虛擬顯示器");
        assert_eq!(kind_name(Path::new("/sys/class/drm/card0-HDMI-A-1")), "外接顯示器");
    }
}
