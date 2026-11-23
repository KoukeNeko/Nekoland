//! A title bar and grips for X11 programs that leave their window's decoration to the
//! window manager, as most that are not GTK or Qt do: xterm, Wine's windows, old toolkits.
//! The bar is drawn above the window and holds its title and the buttons that minimize,
//! maximize and close it; the window's edges, and the bar's own, resize it. It is drawn as
//! GTK draws its header bar: in Nekoland's look while that is on, in Adwaita's otherwise.
//!
//! A program of Windows, shown by FreeRDP, has a title bar of Windows' in the window itself;
//! Settings' Windows App names such a window after how tall that is, and the bar is drawn
//! over it, with what it is taller by above the window.
//!
//! shortcut: the bar has no hover; its buttons act as they are pressed, and the cursor does
//! not change over a grip.

use std::cell::RefCell;

use smithay::{
    backend::renderer::{
        element::{memory::MemoryRenderBufferRenderElement, Kind},
        gles::GlesRenderer,
    },
    desktop::{Window, WindowSurface},
    input::pointer::{Focus, GrabStartData},
    utils::{Logical, Physical, Point, Rectangle, Serial, Size},
    xwayland::{xwm::WmWindowType, X11Surface},
};

use crate::{
    grabs::{resize_grab::ResizeEdge, MoveSurfaceGrab, ResizeSurfaceGrab},
    menubar,
    render::SceneElement,
    text::{self, Picture},
    Nekoland,
};

/// As tall as GTK's header bar.
pub const TITLE_HEIGHT: i32 = 47;
/// How far outside a window its grips reach, and how much of the bar's top is a grip.
const GRIP: i32 = 6;
const TOP_GRIP: i32 = 4;
/// A button is a circle with room between it and the next, as GTK's window buttons are in
/// the look; the last is as far from the bar's end.
const BUTTON_WIDTH: i32 = 32;
const BUTTON_INSET: i32 = 8;
const BUTTON_RADIUS: f64 = 12.0;
const BUTTONS: i32 = 3;
const CORNER_RADIUS: f64 = 14.0;
const TITLE_FONT: &str = "Bold 11";
const GLYPH: f64 = 4.0;
/// What Settings' Windows App names a window of Windows', before how much of its top is a
/// title bar of Windows' and how wide and tall Windows has it.
const FOREIGN: &str = "nekoland-window-";
/// FreeRDP's window is larger than Windows' by the margins Windows resizes it at, which it
/// leaves black. More than this is a size not caught up with yet, not a margin.
const TRIM_MAX: i32 = 32;
const BUTTON_LEFT: u32 = 0x110;

/// Whether a window is one of a program's own, not a menu or a tooltip.
pub fn is_own_window(surface: &X11Surface) -> bool {
    !surface.is_override_redirect()
        && matches!(
            surface.window_type(),
            None | Some(WmWindowType::Normal | WmWindowType::Dialog | WmWindowType::Utility)
        )
}

/// Whether a program is to have a bar from the window manager: a window of its own that
/// does not say it draws its own decoration.
pub fn needs_title(surface: &X11Surface) -> bool {
    is_own_window(surface) && !surface.is_decorated()
}

/// What a window of Windows' is named after: how much of its top is a title bar of
/// Windows', and how large Windows has it, in points: Windows says it in dots, as many a
/// point as X11 programs draw.
fn foreign(name: &str) -> Option<(i32, Size<i32, Logical>)> {
    let mut parts = name.strip_prefix(FOREIGN)?.split('-').map(str::parse::<i32>);
    let (title, width, height) = (parts.next()?.ok()?, parts.next()?.ok()?, parts.next()?.ok()?);
    let points = |dots: i32| (f64::from(dots) / crate::xsettings::scale()).round() as i32;
    Some((points(title), (points(width), points(height)).into()))
}

/// How much of a window's top is a title bar its program draws, which the bar goes over.
fn covered(surface: &X11Surface) -> Option<i32> {
    foreign(&surface.instance()).map(|(title, _)| title).filter(|height| *height > 0)
}

/// How much of the right and bottom of a window `shown` large is not the program's, which
/// has it `own` large.
fn trim_of(shown: Size<i32, Logical>, own: Size<i32, Logical>) -> Size<i32, Logical> {
    let extra = |shown: i32, own: i32| Some(shown - own).filter(|extra| (0..=TRIM_MAX).contains(extra)).unwrap_or(0);
    (extra(shown.w, own.w), extra(shown.h, own.h)).into()
}

/// How much of the right and bottom of a window is not to be drawn.
pub fn trim(window: &Window) -> Size<i32, Logical> {
    match window.underlying_surface() {
        WindowSurface::X11(surface) => foreign(&surface.instance())
            .map(|(_, own)| trim_of(window.geometry().size, own))
            .unwrap_or_default(),
        WindowSurface::Wayland(_) => Size::default(),
    }
}

/// How much room above a window its bar takes: all of it, or what it is taller by than the
/// title bar of the program's own that it covers.
pub fn title_room(surface: &X11Surface) -> i32 {
    match covered(surface) {
        Some(height) => (TITLE_HEIGHT - height).max(0),
        None if needs_title(surface) => TITLE_HEIGHT,
        None => 0,
    }
}

/// What a point on a window's bar or grips is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hit {
    Title,
    Minimize,
    Maximize,
    Close,
    Edge(ResizeEdge),
}

/// What the bar was drawn from, to draw it again only when one of them changes.
#[derive(PartialEq)]
struct Drawn {
    title: String,
    width: i32,
    height: i32,
    look: bool,
    focused: bool,
    dark: bool,
    scale: i32,
    /// How round its top corners are, as the window's.
    radius: i32,
}

#[derive(Default)]
struct Cached(RefCell<Option<(Drawn, Picture)>>);

/// Where `point` is on a bar `title` with the window's `content` under it, and its grips.
fn hit_at(title: Rectangle<i32, Logical>, content: Rectangle<i32, Logical>, point: Point<f64, Logical>) -> Hit {
    let mut edges = ResizeEdge::empty();
    if point.x < f64::from(title.loc.x) {
        edges |= ResizeEdge::LEFT;
    }
    if point.x >= f64::from(title.loc.x + title.size.w) {
        edges |= ResizeEdge::RIGHT;
    }
    if point.y >= f64::from(content.loc.y + content.size.h) {
        edges |= ResizeEdge::BOTTOM;
    }
    if point.y < f64::from(title.loc.y + TOP_GRIP) {
        edges |= ResizeEdge::TOP;
    }
    if !edges.is_empty() {
        return Hit::Edge(edges);
    }
    match ((f64::from(title.loc.x + title.size.w - BUTTON_INSET) - point.x).max(0.0) / f64::from(BUTTON_WIDTH)) as i32 {
        0 => Hit::Close,
        1 => Hit::Maximize,
        2 => Hit::Minimize,
        _ => Hit::Title,
    }
}

impl Nekoland {
    /// Whether the window manager is to draw a bar over this window, in the places it goes:
    /// a fullscreen window has none to show, but keeps room for it for when it is back.
    pub fn wants_title(&self, window: &Window) -> bool {
        matches!(window.underlying_surface(), WindowSurface::X11(surface) if needs_title(surface))
    }

    pub fn is_framed(&self, window: &Window) -> bool {
        self.wants_title(window) && !self.is_fullscreen(window)
    }

    /// The room above the window that its bar takes, in the places it goes.
    fn title_room(&self, window: &Window) -> i32 {
        match window.underlying_surface() {
            WindowSurface::X11(surface) => title_room(surface),
            WindowSurface::Wayland(_) => 0,
        }
    }

    /// `rect`, a window's own, with the room for its bar above it.
    pub fn with_title(&self, window: &Window, rect: Rectangle<i32, Logical>) -> Rectangle<i32, Logical> {
        let room = self.title_room(window);
        Rectangle::new((rect.loc.x, rect.loc.y - room).into(), (rect.size.w, rect.size.h + room).into())
    }

    /// The window's own part of `rect`, the bar's room taken off its top.
    pub fn without_title(&self, window: &Window, rect: Rectangle<i32, Logical>) -> Rectangle<i32, Logical> {
        let room = self.title_room(window);
        if room == 0 {
            return rect;
        }
        Rectangle::new((rect.loc.x, rect.loc.y + room).into(), (rect.size.w, (rect.size.h - room).max(1)).into())
    }

    /// How tall the bar over a title bar of the window's own is, if it has one to cover.
    fn covered_title(&self, window: &Window) -> Option<i32> {
        match window.underlying_surface() {
            WindowSurface::X11(surface) if !self.is_fullscreen(window) => covered(surface),
            _ => None,
        }
    }

    fn title_rect(&self, window: &Window) -> Option<Rectangle<i32, Logical>> {
        let location = self.space.element_location(window)?;
        if let Some(height) = self.covered_title(window) {
            let room = self.title_room(window);
            return Some(Rectangle::new(
                (location.x, location.y - room).into(),
                (window.geometry().size.w - trim(window).w, room + height).into(),
            ));
        }
        if !self.is_framed(window) {
            return None;
        }
        Some(Rectangle::new(
            (location.x, location.y - TITLE_HEIGHT).into(),
            (window.geometry().size.w, TITLE_HEIGHT).into(),
        ))
    }

    /// The window whose bar or grip is at `point`, and which it is, unless a window is in
    /// front of it.
    pub(crate) fn frame_hit(&self, point: Point<f64, Logical>) -> Option<(Window, Hit)> {
        if self.overview.is_some() {
            return None;
        }
        for window in self.space.elements().rev() {
            // A bar over the window's own is partly in the window; the program resizes itself.
            let covered = self.covered_title(window);
            if let Some((title, covered)) = self.title_rect(window).zip(covered) {
                if title.to_f64().contains(point) {
                    let content = Rectangle::new(
                        (title.loc.x, title.loc.y + title.size.h).into(),
                        (title.size.w, window.geometry().size.h - covered).into(),
                    );
                    return Some((window.clone(), hit_at(title, content, point)));
                }
            }
            if self.space.element_bbox(window).is_some_and(|bbox| bbox.to_f64().contains(point)) {
                return None;
            }
            if covered.is_some() {
                continue;
            }
            let Some(title) = self.title_rect(window) else {
                continue;
            };
            let content = Rectangle::new(
                (title.loc.x, title.loc.y + TITLE_HEIGHT).into(),
                (title.size.w, window.geometry().size.h).into(),
            );
            let outer = Rectangle::new(
                (title.loc.x - GRIP, title.loc.y).into(),
                (title.size.w + 2 * GRIP, TITLE_HEIGHT + content.size.h + GRIP).into(),
            );
            if outer.to_f64().contains(point) {
                return Some((window.clone(), hit_at(title, content, point)));
            }
        }
        None
    }

    /// Acts on a press on a bar or grip. Returns whether there was one.
    pub fn frame_press(&mut self, point: Point<f64, Logical>, button: u32, serial: Serial) -> bool {
        let Some((window, hit)) = self.frame_hit(point) else {
            return false;
        };
        self.focus_window(&window);
        if button != BUTTON_LEFT {
            return true;
        }
        let Some(pointer) = self.seat.get_pointer() else {
            return true;
        };
        let Some(location) = self.space.element_location(&window) else {
            return true;
        };
        let start_data = GrabStartData {
            focus: None,
            button,
            location: point,
        };
        match hit {
            Hit::Close => menubar::close(&window),
            Hit::Minimize => self.minimize(&window),
            Hit::Maximize => {
                let maximized = matches!(window.underlying_surface(), WindowSurface::X11(surface) if surface.is_maximized());
                if maximized {
                    self.restore(&window);
                } else {
                    self.maximize(&window);
                }
            }
            Hit::Title => {
                let grab = MoveSurfaceGrab::new(start_data, window, location);
                pointer.set_grab(self, grab, serial, Focus::Clear);
            }
            Hit::Edge(edges) => {
                let rect = Rectangle::new(location, window.geometry().size);
                let grab = ResizeSurfaceGrab::start(start_data, window, edges, rect);
                pointer.set_grab(self, grab, serial, Focus::Clear);
            }
        }
        true
    }

    /// The bar's picture, drawn again when its title, width, focus, the scale or the colours
    /// changed.
    fn title_picture(&self, window: &Window, width: i32, height: i32) -> Option<Picture> {
        let WindowSurface::X11(surface) = window.underlying_surface() else {
            return None;
        };
        let drawn = Drawn {
            title: surface.title(),
            width,
            height,
            look: crate::xsettings::is_nekoland_look(),
            focused: self.focused_window().as_ref() == Some(window),
            dark: self.menu_bar.dark,
            scale: text::picture_scale(),
            radius: (CORNER_RADIUS * crate::windows::roundness(window).0).round() as i32,
        };
        let cached = window.user_data().get_or_insert(Cached::default);
        let mut cached = cached.0.borrow_mut();
        if let Some((was, picture)) = cached.as_ref() {
            if *was == drawn {
                return Some(picture.clone());
            }
        }
        let picture = draw(&drawn)?;
        *cached = Some((drawn, picture.clone()));
        Some(picture)
    }
}

/// The bar over a window, where `output_loc` is the output's, as render elements: drawn as
/// the window is in `pose`, so that it comes and goes with the window and not after it.
pub fn title_element(
    state: &Nekoland,
    renderer: &mut GlesRenderer,
    window: &Window,
    pose: crate::motion::Pose,
    output_loc: Point<i32, Logical>,
    scale: f64,
) -> Option<SceneElement> {
    let title = state.title_rect(window)?;
    let picture = state.title_picture(window, title.size.w, title.size.h)?;
    let (actual, shown) = (pose.actual, pose.shown);
    // As far from where the window is shown as it is from where the window is.
    let at: Point<i32, Physical> = (title.loc - actual.loc + shown.loc - output_loc).to_physical_precise_round(scale);
    let alpha = Some(pose.alpha).filter(|alpha| *alpha < 1.0);
    let bar =
        MemoryRenderBufferRenderElement::from_buffer(renderer, at.to_f64(), &picture.buffer, alpha, None, None, Kind::Unspecified)
            .ok()?;
    if shown.size == actual.size || actual.size.w <= 0 || actual.size.h <= 0 {
        return Some(SceneElement::Picture(bar));
    }
    let origin: Point<i32, Physical> = (shown.loc - output_loc).to_physical_precise_round(scale);
    let factor = smithay::utils::Scale::from((
        f64::from(shown.size.w) / f64::from(actual.size.w),
        f64::from(shown.size.h) / f64::from(actual.size.h),
    ));
    Some(SceneElement::ZoomedPicture(smithay::backend::renderer::element::utils::RescaleRenderElement::from_element(bar, origin, factor)))
}

/// The colours of GTK's header bar: its background, what is written on it, and the line
/// under it.
fn header_colors(drawn: &Drawn) -> ((f64, f64, f64), (f64, f64, f64, f64), (f64, f64, f64, f64)) {
    if drawn.look {
        // Paper, ink and line of shell/gtk/nekoland.css, the bar the same with the focus
        // and without.
        let colors = menubar::colors(drawn.dark);
        let (ink, paper, line) = if drawn.dark {
            (if drawn.focused { colors.ink } else { colors.ink_3 }, (0.106, 0.106, 0.118), (0.196, 0.196, 0.216))
        } else {
            (if drawn.focused { colors.ink } else { colors.ink_3 }, (0.957, 0.953, 0.941), (0.886, 0.878, 0.859))
        };
        return (paper, (ink.0, ink.1, ink.2, 1.0), (line.0, line.1, line.2, 1.0));
    }
    // libadwaita's: the header bar's colour with the focus, the window's without, where
    // what is written is at half strength.
    let strength = if drawn.focused { 1.0 } else { 0.5 };
    match (drawn.dark, drawn.focused) {
        (true, true) => ((0.180, 0.180, 0.196), (1.0, 1.0, 1.0, strength), (0.0, 0.0, 0.024, 0.36)),
        (true, false) => ((0.133, 0.133, 0.149), (1.0, 1.0, 1.0, strength), (0.0, 0.0, 0.024, 0.36)),
        (false, true) => ((1.0, 1.0, 1.0), (0.0, 0.0, 0.024, 0.8 * strength), (0.0, 0.0, 0.024, 0.12)),
        (false, false) => ((0.980, 0.980, 0.984), (0.0, 0.0, 0.024, 0.8 * strength), (0.0, 0.0, 0.024, 0.12)),
    }
}

fn draw(drawn: &Drawn) -> Option<Picture> {
    let (paper, ink, line) = header_colors(drawn);
    let height = f64::from(drawn.height);
    let width = f64::from(drawn.width);
    let surface = text::canvas(drawn.width, drawn.height).ok()?;
    let cr = cairo::Context::new(&surface).ok()?;

    // Its top corners are rounded; the bottom ones fall below the picture.
    let radius = f64::from(drawn.radius);
    text::rounded_rectangle(&cr, 0.0, 0.0, width, height + CORNER_RADIUS * 4.0, radius);
    cr.set_source_rgb(paper.0, paper.1, paper.2);
    cr.fill().ok()?;
    cr.set_source_rgba(line.0, line.1, line.2, line.3);
    cr.rectangle(0.0, height - 1.0, width, 1.0);
    cr.fill().ok()?;

    // The title between room as wide as the buttons on both sides, so that it is centred.
    let room = f64::from(BUTTON_INSET + BUTTON_WIDTH * BUTTONS);
    let layout = text::new_layout(&cr);
    layout.set_font_description(Some(&text::font(TITLE_FONT)));
    layout.set_text(&drawn.title);
    layout.set_width(((width - 2.0 * room).max(0.0) * f64::from(pango::SCALE)) as i32);
    layout.set_ellipsize(pango::EllipsizeMode::End);
    layout.set_alignment(pango::Alignment::Center);
    // A window too narrow for it goes without: its ellipsis alone would be on the buttons.
    if width - 2.0 * room >= f64::from(BUTTON_WIDTH) {
        let (_, text_height) = layout.pixel_size();
        cr.move_to(room, (height - f64::from(text_height)) / 2.0);
        cr.set_source_rgba(ink.0, ink.1, ink.2, ink.3);
        pangocairo::functions::show_layout(&cr, &layout);
    }

    // Minimize, maximize and close, as GTK's: a glyph on a circle of a tenth of its colour.
    let middle = |from_right: i32| {
        let x = width - f64::from(BUTTON_INSET + BUTTON_WIDTH * from_right) - f64::from(BUTTON_WIDTH) / 2.0;
        (x, height / 2.0)
    };
    cr.set_source_rgba(ink.0, ink.1, ink.2, ink.3 * 0.1);
    for button in 0..BUTTONS {
        let (x, y) = middle(button);
        cr.arc(x, y, BUTTON_RADIUS.min(height / 2.0 - 2.0), 0.0, std::f64::consts::TAU);
        cr.fill().ok()?;
    }
    cr.set_source_rgba(ink.0, ink.1, ink.2, ink.3);
    cr.set_line_width(1.5);
    let (x, y) = middle(2);
    cr.move_to(x - GLYPH, y);
    cr.line_to(x + GLYPH, y);
    cr.stroke().ok()?;
    let (x, y) = middle(1);
    cr.rectangle(x - GLYPH, y - GLYPH, 2.0 * GLYPH, 2.0 * GLYPH);
    cr.stroke().ok()?;
    let (x, y) = middle(0);
    cr.move_to(x - GLYPH, y - GLYPH);
    cr.line_to(x + GLYPH, y + GLYPH);
    cr.move_to(x + GLYPH, y - GLYPH);
    cr.line_to(x - GLYPH, y + GLYPH);
    cr.stroke().ok()?;

    drop(cr);
    text::picture(surface)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_of_windows_says_its_title_bar_and_size() {
        assert_eq!(foreign("nekoland-window-31-461-259"), Some((31, (461, 259).into())));
        assert_eq!(foreign("nekoland-window-31"), None);
        assert_eq!(foreign("xterm"), None);
        // FreeRDP's is larger by the margins; a size Windows has not caught up with is not one.
        assert_eq!(trim_of((475, 266).into(), (461, 259).into()), (14, 7).into());
        assert_eq!(trim_of((700, 266).into(), (461, 259).into()), (0, 7).into());
        assert_eq!(trim_of((450, 259).into(), (461, 259).into()), (0, 0).into());
    }

    #[test]
    fn a_bar_has_buttons_at_its_right_and_grips_around_it() {
        let title = Rectangle::new((100, 100).into(), (400, TITLE_HEIGHT).into());
        let content = Rectangle::new((100, 100 + TITLE_HEIGHT).into(), (400, 300).into());
        let at = |x: f64, y: f64| hit_at(title, content, (x, y).into());
        assert_eq!(at(480.0, 116.0), Hit::Close);
        assert_eq!(at(450.0, 116.0), Hit::Maximize);
        assert_eq!(at(420.0, 116.0), Hit::Minimize);
        assert_eq!(at(200.0, 116.0), Hit::Title);
        assert_eq!(at(200.0, 101.0), Hit::Edge(ResizeEdge::TOP));
        assert_eq!(at(97.0, 200.0), Hit::Edge(ResizeEdge::LEFT));
        assert_eq!(at(503.0, 453.0), Hit::Edge(ResizeEdge::BOTTOM_RIGHT));
        assert_eq!(at(97.0, 101.0), Hit::Edge(ResizeEdge::TOP_LEFT));
    }
}
