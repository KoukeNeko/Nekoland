//! Cursor pictures from the XCursor theme, for the backend that draws its own cursor.

use std::collections::HashMap;

use smithay::{
    backend::{allocator::Fourcc, renderer::element::memory::MemoryRenderBuffer},
    input::pointer::CursorIcon,
    utils::{Physical, Point, Transform},
};
use xcursor::{parser::parse_xcursor, CursorTheme};

const SIZE: i64 = 24;
// Tried in order after $XCURSOR_THEME; the first one that has an arrow is used.
const FALLBACK_THEMES: [&str; 3] = ["default", "Adwaita", "Yaru"];

pub struct CursorImage {
    pub buffer: MemoryRenderBuffer,
    pub hotspot: Point<i32, Physical>,
}

pub struct Cursors {
    theme: Option<CursorTheme>,
    images: HashMap<CursorIcon, Option<CursorImage>>,
}

impl Cursors {
    pub fn load() -> Self {
        let theme = std::env::var("XCURSOR_THEME")
            .ok()
            .into_iter()
            .chain(FALLBACK_THEMES.map(String::from))
            .map(|name| CursorTheme::load(&name))
            .find(|theme| load_icon(theme, CursorIcon::Default).is_some());
        if theme.is_none() {
            tracing::warn!("no cursor theme found; the cursor will not be drawn");
        }
        Self {
            theme,
            images: HashMap::new(),
        }
    }

    /// The picture for a cursor, or the arrow when the theme does not have it.
    pub fn get(&mut self, icon: CursorIcon) -> Option<&CursorImage> {
        let theme = self.theme.as_ref()?;
        let known = self
            .images
            .entry(icon)
            .or_insert_with(|| load_icon(theme, icon))
            .is_some();
        let icon = if known { icon } else { CursorIcon::Default };
        self.images.get(&icon)?.as_ref()
    }
}

fn load_icon(theme: &CursorTheme, icon: CursorIcon) -> Option<CursorImage> {
    let path = std::iter::once(icon.name())
        .chain(icon.alt_names().iter().copied())
        .find_map(|name| theme.load_icon(name))?;
    let images = parse_xcursor(&std::fs::read(path).ok()?)?;
    // shortcut: the first frame of an animated cursor; a spinner does not spin.
    let image = images
        .iter()
        .min_by_key(|image| (i64::from(image.size) - SIZE).abs())?;
    Some(CursorImage {
        buffer: MemoryRenderBuffer::from_slice(
            &image.pixels_rgba,
            Fourcc::Abgr8888,
            (image.width as i32, image.height as i32),
            1,
            Transform::Normal,
            None,
        ),
        hotspot: (image.xhot as i32, image.yhot as i32).into(),
    })
}
