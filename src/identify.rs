//! Each display's number, shown large in its bottom left corner for a while, as Windows'
//! Identify button does, for Settings' picture of the displays to be matched to them.

use std::time::{Duration, Instant};

use smithay::{backend::renderer::element::Id, utils::Rectangle};

use crate::{
    glass::Tier,
    menubar::{self, Overlay},
    text::{self, Picture},
    Nekoland,
};

const SHOWN_FOR: Duration = Duration::from_secs(3);
const SIZE: i32 = 120;
const MARGIN: i32 = 40;
const RADIUS: i32 = 16;
const POINTS: f64 = 64.0;

#[derive(Default)]
pub struct Identify {
    until: Option<Instant>,
    /// Each display's number, by the display's name.
    numbers: Vec<(String, Picture, Id)>,
}

impl Nekoland {
    /// Shows every display's number, the main one 1, in the order Settings shows them.
    pub fn identify_displays(&mut self) {
        let outputs: Vec<String> = if self.connected_outputs.is_empty() {
            self.space.outputs().map(|output| output.name()).collect()
        } else {
            self.connected_outputs.iter().map(|output| output.name()).collect()
        };
        let dark = self.menu_bar_dark();
        self.identify.numbers = outputs
            .into_iter()
            .enumerate()
            .filter_map(|(index, name)| Some((name, draw_number(index + 1, dark)?, Id::new())))
            .collect();
        self.identify.until = Some(Instant::now() + SHOWN_FOR);
    }

    pub fn identify_overlays(&self) -> Vec<Overlay<'_>> {
        if self.identify.until.is_none_or(|until| Instant::now() >= until) {
            return Vec::new();
        }
        self.space
            .outputs()
            .filter_map(|output| {
                let area = self.space.output_geometry(output)?;
                let (_, picture, id) = self.identify.numbers.iter().find(|(name, _, _)| *name == output.name())?;
                let at = (area.loc.x + MARGIN, area.loc.y + area.size.h - SIZE - MARGIN);
                Some(Overlay {
                    fade: 1.0,
                    id,
                    rect: Rectangle::new(at.into(), picture.size),
                    glass: Some((Tier::Menu, RADIUS)),
                    alpha: 1.0,
                    picture,
                })
            })
            .collect()
    }
}

fn draw_number(number: usize, dark: bool) -> Option<Picture> {
    let surface = text::canvas(SIZE, SIZE).ok()?;
    {
        let cr = cairo::Context::new(&surface).ok()?;
        let font = menubar::font("Semibold", POINTS);
        let ink = menubar::colors(dark).ink;
        let text = number.to_string();
        let layout = pangocairo::functions::create_layout(&cr);
        layout.set_font_description(Some(&font));
        layout.set_text(&text);
        let (width, _) = layout.pixel_size();
        menubar::show_text(&cr, &text, &font, f64::from((SIZE - width) / 2), f64::from(SIZE) / 2.0, ink);
    }
    text::picture(surface)
}
