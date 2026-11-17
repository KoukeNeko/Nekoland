//! The frames drawn each second, counted on glass in the screen's top right corner while
//! the `show_fps` setting is on, as the Mac's performance HUD.

use std::time::{Duration, Instant};

use smithay::{backend::renderer::element::Id, utils::Rectangle};

use crate::{
    glass::Tier,
    menubar::{self, Overlay, MENU_BAR_HEIGHT},
    text::{self, Picture},
    Nekoland,
};

/// How long frames are counted for each figure.
const PERIOD: Duration = Duration::from_secs(1);
const WIDTH: i32 = 84;
const HEIGHT: i32 = 28;
const MARGIN: i32 = 12;
const RADIUS: i32 = 8;
const POINTS: f64 = 13.0;
const PADDING: f64 = 10.0;

pub struct FpsMeter {
    frames: u32,
    counted_since: Option<Instant>,
    /// The last whole period's count, and whether it was drawn on a dark menu bar's colours.
    drawn: Option<(u32, bool)>,
    picture: Option<Picture>,
    id: Id,
}

impl Default for FpsMeter {
    fn default() -> Self {
        FpsMeter {
            frames: 0,
            counted_since: None,
            drawn: None,
            picture: None,
            id: Id::new(),
        }
    }
}

impl Nekoland {
    /// Counts a frame handed to the display.
    pub fn count_frame(&mut self) {
        self.fps.frames += 1;
    }

    /// Takes up the count once a period has passed, and draws it again if it changed.
    pub fn refresh_fps(&mut self) {
        if !self.config.show_fps {
            self.fps = FpsMeter::default();
            return;
        }
        let since = *self.fps.counted_since.get_or_insert_with(Instant::now);
        if since.elapsed() < PERIOD {
            return;
        }
        let per_second = (f64::from(self.fps.frames) / since.elapsed().as_secs_f64()).round() as u32;
        self.fps.frames = 0;
        self.fps.counted_since = Some(Instant::now());
        let dark = self.menu_bar_dark();
        if self.fps.drawn == Some((per_second, dark)) {
            return;
        }
        match draw_count(per_second, dark) {
            Some(picture) => {
                self.fps.picture = Some(picture);
                self.fps.drawn = Some((per_second, dark));
            }
            None => tracing::warn!("could not draw the frame rate"),
        }
    }

    /// What the count puts on screen.
    pub fn fps_overlays(&self) -> Vec<Overlay<'_>> {
        let (Some(picture), Some(area)) = (&self.fps.picture, self.output_area()) else {
            return Vec::new();
        };
        if !self.config.show_fps {
            return Vec::new();
        }
        let at = (area.loc.x + area.size.w - WIDTH - MARGIN, area.loc.y + MENU_BAR_HEIGHT + MARGIN);
        vec![Overlay {
            fade: 1.0,
            id: &self.fps.id,
            rect: Rectangle::new(at.into(), picture.size),
            glass: Some((Tier::Menu, RADIUS)),
            alpha: 1.0,
            picture,
        }]
    }
}

fn draw_count(per_second: u32, dark: bool) -> Option<Picture> {
    let surface = text::canvas(WIDTH, HEIGHT).ok()?;
    {
        let cr = cairo::Context::new(&surface).ok()?;
        let font = menubar::font("Semibold", POINTS);
        let ink = menubar::colors(dark).ink;
        menubar::show_text(&cr, &format!("{per_second} FPS"), &font, PADDING, f64::from(HEIGHT) / 2.0, ink);
    }
    text::picture(surface)
}
