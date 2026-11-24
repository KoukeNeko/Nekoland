//! Cursor pictures from the XCursor theme, for the backend that draws its own cursor.

use std::collections::HashMap;

use smithay::{
    backend::{allocator::Fourcc, renderer::element::memory::MemoryRenderBuffer},
    input::pointer::CursorIcon,
    utils::{Logical, Point, Size, Transform},
};
use xcursor::{parser::parse_xcursor, CursorTheme};

// Tried in order after $XCURSOR_THEME; the first one that has an arrow is used.
const FALLBACK_THEMES: [&str; 3] = ["default", "Adwaita", "Yaru"];

pub struct CursorImage {
    pub buffer: MemoryRenderBuffer,
    pub hotspot: Point<f64, Logical>,
    pub size: Size<i32, Logical>,
}

pub struct Cursors {
    /// The theme asked for, which may not be the one found.
    pub asked: String,
    theme: Option<CursorTheme>,
    /// By icon, how many pixels each of its points has, and how many points across it is.
    images: HashMap<(CursorIcon, i32, i32), Option<CursorImage>>,
}

impl Cursors {
    /// The cursors of the theme `asked`, else of $XCURSOR_THEME, else of a common one.
    pub fn load(asked: &str) -> Self {
        let theme = Some(asked.to_string())
            .filter(|asked| !asked.is_empty())
            .into_iter()
            .chain(std::env::var("XCURSOR_THEME").ok())
            .chain(FALLBACK_THEMES.map(String::from))
            .map(|name| CursorTheme::load(&name))
            .find(|theme| load_icon(theme, CursorIcon::Default, 1, 24).is_some());
        if theme.is_none() {
            tracing::warn!("no cursor theme found; the cursor will not be drawn");
        }
        Self {
            asked: asked.to_string(),
            theme,
            images: HashMap::new(),
        }
    }

    /// The picture for a cursor `size` points across on an output of `scale`, or the arrow
    /// when the theme does not have it.
    pub fn get(&mut self, icon: CursorIcon, scale: f64, size: i32) -> Option<&CursorImage> {
        let theme = self.theme.as_ref()?;
        let scale = (scale.ceil() as i32).max(1);
        let size = size.max(1);
        let known = self
            .images
            .entry((icon, scale, size))
            .or_insert_with(|| load_icon(theme, icon, scale, size))
            .is_some();
        let icon = if known { icon } else { CursorIcon::Default };
        self.images
            .entry((icon, scale, size))
            .or_insert_with(|| load_icon(theme, icon, scale, size))
            .as_ref()
    }
}

/// The cursor's frame nearest `size` points across, drawn with `scale` pixels to a point.
fn load_icon(theme: &CursorTheme, icon: CursorIcon, scale: i32, size: i32) -> Option<CursorImage> {
    let path = std::iter::once(icon.name())
        .chain(icon.alt_names().iter().copied())
        .find_map(|name| theme.load_icon(name))?;
    let images = parse_xcursor(&std::fs::read(path).ok()?)?;
    // shortcut: the first frame of an animated cursor; a spinner does not spin.
    let image = images
        .iter()
        .min_by_key(|image| (i64::from(image.size) - i64::from(size * scale)).abs())?;
    // A theme without a frame that big has its nearest drawn larger, at fewer pixels a point.
    let scale = ((f64::from(image.size) / f64::from(size)).round() as i32).clamp(1, scale);
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
        size: (image.width as i32 / scale, image.height as i32 / scale).into(),
    })
}
