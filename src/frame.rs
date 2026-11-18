//! A title bar and grips for X11 programs that leave their window's decoration to the
//! window manager, as most that are not GTK or Qt do: xterm, Wine's windows, old toolkits.
//! The bar is drawn above the window and holds its title and the buttons that minimize,
//! maximize and close it; the window's edges, and the bar's own, resize it.
//!
//! shortcut: the bar is a picture of the compositor's own, in the menu bar's colours, not
//! GTK's look, which X11 programs draw for themselves; its buttons act as they are pressed,
//! and the cursor does not change over a grip.

use std::cell::RefCell;

use smithay::{
    backend::renderer::{
        element::{memory::MemoryRenderBufferRenderElement, Kind},
        gles::GlesRenderer,
    },
    desktop::{Window, WindowSurface},
    input::pointer::{Focus, GrabStartData},
    utils::{Logical, Physical, Point, Rectangle, Serial},
    xwayland::{xwm::WmWindowType, X11Surface},
};

use crate::{
    grabs::{resize_grab::ResizeEdge, MoveSurfaceGrab, ResizeSurfaceGrab},
    menubar,
    render::SceneElement,
    text::{self, Picture},
    Nekoland,
};

pub const TITLE_HEIGHT: i32 = 32;
/// How far outside a window its grips reach, and how much of the bar's top is a grip.
const GRIP: i32 = 6;
const TOP_GRIP: i32 = 4;
const BUTTON_WIDTH: i32 = 36;
const BUTTONS: i32 = 3;
const CORNER_RADIUS: f64 = 10.0;
const TITLE_FONT: &str = "11";
const GLYPH: f64 = 5.0;
const BUTTON_LEFT: u32 = 0x110;

/// Whether a program is to have a bar from the window manager: a window of its own that
/// does not say it draws its own decoration.
pub fn needs_title(surface: &X11Surface) -> bool {
    !surface.is_override_redirect()
        && !surface.is_decorated()
        && matches!(
            surface.window_type(),
            None | Some(WmWindowType::Normal | WmWindowType::Dialog | WmWindowType::Utility)
        )
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
    focused: bool,
    dark: bool,
    scale: i32,
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
    match ((f64::from(title.loc.x + title.size.w) - point.x) / f64::from(BUTTON_WIDTH)) as i32 {
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

    /// `rect`, a window's own, with the room for its bar above it.
    pub fn with_title(&self, window: &Window, rect: Rectangle<i32, Logical>) -> Rectangle<i32, Logical> {
        if self.wants_title(window) {
            Rectangle::new(
                (rect.loc.x, rect.loc.y - TITLE_HEIGHT).into(),
                (rect.size.w, rect.size.h + TITLE_HEIGHT).into(),
            )
        } else {
            rect
        }
    }

    /// The window's own part of `rect`, the bar's room taken off its top.
    pub fn without_title(&self, window: &Window, rect: Rectangle<i32, Logical>) -> Rectangle<i32, Logical> {
        if self.wants_title(window) {
            Rectangle::new(
                (rect.loc.x, rect.loc.y + TITLE_HEIGHT).into(),
                (rect.size.w, (rect.size.h - TITLE_HEIGHT).max(1)).into(),
            )
        } else {
            rect
        }
    }

    fn title_rect(&self, window: &Window) -> Option<Rectangle<i32, Logical>> {
        if !self.is_framed(window) {
            return None;
        }
        let location = self.space.element_location(window)?;
        Some(Rectangle::new(
            (location.x, location.y - TITLE_HEIGHT).into(),
            (window.geometry().size.w, TITLE_HEIGHT).into(),
        ))
    }

    /// The window whose bar or grip is at `point`, and which it is, unless a window is in
    /// front of it.
    fn frame_hit(&self, point: Point<f64, Logical>) -> Option<(Window, Hit)> {
        if self.overview.is_some() {
            return None;
        }
        for window in self.space.elements().rev() {
            if self.space.element_bbox(window).is_some_and(|bbox| bbox.to_f64().contains(point)) {
                return None;
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
    fn title_picture(&self, window: &Window, width: i32) -> Option<Picture> {
        let WindowSurface::X11(surface) = window.underlying_surface() else {
            return None;
        };
        let drawn = Drawn {
            title: surface.title(),
            width,
            focused: self.focused_window().as_ref() == Some(window),
            dark: self.menu_bar.dark,
            scale: text::picture_scale(),
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

/// The bar over a window `width` wide, where `output_loc` is the output's, as render
/// elements.
pub fn title_element(
    state: &Nekoland,
    renderer: &mut GlesRenderer,
    window: &Window,
    output_loc: Point<i32, Logical>,
    scale: f64,
) -> Option<SceneElement> {
    let title = state.title_rect(window)?;
    let picture = state.title_picture(window, title.size.w)?;
    let at: Point<i32, Physical> = (title.loc - output_loc).to_physical_precise_round(scale);
    MemoryRenderBufferRenderElement::from_buffer(renderer, at.to_f64(), &picture.buffer, None, None, None, Kind::Unspecified)
        .ok()
        .map(SceneElement::Picture)
}

fn draw(drawn: &Drawn) -> Option<Picture> {
    let colors = menubar::colors(drawn.dark);
    let height = f64::from(TITLE_HEIGHT);
    let width = f64::from(drawn.width);
    let surface = text::canvas(drawn.width, TITLE_HEIGHT).ok()?;
    let cr = cairo::Context::new(&surface).ok()?;

    // Its top corners are rounded; the bottom ones fall below the picture.
    text::rounded_rectangle(&cr, 0.0, 0.0, width, height + CORNER_RADIUS * 4.0, CORNER_RADIUS);
    cr.set_source_rgb(colors.sheet.0, colors.sheet.1, colors.sheet.2);
    cr.fill().ok()?;
    let (r, g, b, a) = colors.edge;
    cr.set_source_rgba(r, g, b, a);
    cr.rectangle(0.0, height - 1.0, width, 1.0);
    cr.fill().ok()?;

    // The title between room as wide as the buttons on both sides, so that it is centred.
    let room = f64::from(BUTTON_WIDTH * BUTTONS);
    let ink = if drawn.focused { colors.ink } else { colors.ink_3 };
    let layout = text::new_layout(&cr);
    layout.set_font_description(Some(&text::font(TITLE_FONT)));
    layout.set_text(&drawn.title);
    layout.set_width(((width - 2.0 * room).max(0.0) * f64::from(pango::SCALE)) as i32);
    layout.set_ellipsize(pango::EllipsizeMode::End);
    layout.set_alignment(pango::Alignment::Center);
    let (_, text_height) = layout.pixel_size();
    cr.move_to(room, (height - f64::from(text_height)) / 2.0);
    cr.set_source_rgb(ink.0, ink.1, ink.2);
    pangocairo::functions::show_layout(&cr, &layout);

    // Minimize, maximize and close, as the buttons of GTK's own bar in the look.
    cr.set_source_rgb(ink.0, ink.1, ink.2);
    cr.set_line_width(1.5);
    let middle = |from_right: i32| (width - f64::from(BUTTON_WIDTH) * (f64::from(from_right) + 0.5), height / 2.0);
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
        assert_eq!(at(503.0, 438.0), Hit::Edge(ResizeEdge::BOTTOM_RIGHT));
        assert_eq!(at(97.0, 101.0), Hit::Edge(ResizeEdge::TOP_LEFT));
    }
}
