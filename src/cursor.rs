//! Cursor pictures from the XCursor theme, for the backend that draws its own cursor.

use std::collections::HashMap;

use smithay::{
    backend::{allocator::Fourcc, renderer::element::memory::MemoryRenderBuffer},
    input::pointer::CursorIcon,
    utils::{Logical, Point, Transform},
};
use xcursor::{parser::parse_xcursor, CursorTheme};

const SIZE: i64 = 24;
// Tried in order after $XCURSOR_THEME; the first one that has an arrow is used.
const FALLBACK_THEMES: [&str; 3] = ["default", "Adwaita", "Yaru"];

pub struct CursorImage {
    pub buffer: MemoryRenderBuffer,
    pub hotspot: Point<f64, Logical>,
}

pub struct Cursors {
    theme: Option<CursorTheme>,
    /// By icon and how many pixels each of its points has.
    images: HashMap<(CursorIcon, i32), Option<CursorImage>>,
}

impl Cursors {
    pub fn load() -> Self {
        let theme = std::env::var("XCURSOR_THEME")
            .ok()
            .into_iter()
            .chain(FALLBACK_THEMES.map(String::from))
            .map(|name| CursorTheme::load(&name))
            .find(|theme| load_icon(theme, CursorIcon::Default, 1).is_some());
        if theme.is_none() {
            tracing::warn!("no cursor theme found; the cursor will not be drawn");
        }
        Self {
            theme,
            images: HashMap::new(),
        }
    }

    /// The picture for a cursor on an output of `scale`, or the arrow when the theme does
    /// not have it.
    pub fn get(&mut self, icon: CursorIcon, scale: f64) -> Option<&CursorImage> {
        let theme = self.theme.as_ref()?;
        let scale = (scale.ceil() as i32).max(1);
        let known = self
            .images
            .entry((icon, scale))
            .or_insert_with(|| load_icon(theme, icon, scale))
            .is_some();
        let icon = if known { icon } else { CursorIcon::Default };
        self.images
            .entry((icon, scale))
            .or_insert_with(|| load_icon(theme, icon, scale))
            .as_ref()
    }
}

/// The cursor's frame nearest `SIZE` points across, drawn with `scale` pixels to a point.
fn load_icon(theme: &CursorTheme, icon: CursorIcon, scale: i32) -> Option<CursorImage> {
    let path = std::iter::once(icon.name())
        .chain(icon.alt_names().iter().copied())
        .find_map(|name| theme.load_icon(name))?;
    let images = parse_xcursor(&std::fs::read(path).ok()?)?;
    // shortcut: the first frame of an animated cursor; a spinner does not spin.
    let image = images
        .iter()
        .min_by_key(|image| (i64::from(image.size) - SIZE * i64::from(scale)).abs())?;
    // A theme without a frame that big has its nearest drawn larger, at fewer pixels a point.
    let scale = ((f64::from(image.size) / SIZE as f64).round() as i32).clamp(1, scale);
    Some(CursorImage {
        buffer: MemoryRenderBuffer::from_slice(
            &image.pixels_rgba,
            Fourcc::Abgr8888,
            (image.width as i32, image.height as i32),
            scale,
            Transform::Normal,
            None,
        ),
        hotspot: (
            f64::from(image.xhot) / f64::from(scale),
            f64::from(image.yhot) / f64::from(scale),
        )
            .into(),
    })
}
