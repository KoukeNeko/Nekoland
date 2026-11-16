//! Hot corners: pointing into a corner of a display does what the configuration says.

use smithay::utils::{Logical, Point};

use crate::{config::HotCorner, state::Nekoland};

impl Nekoland {
    /// Acts once as the pointer comes into a corner, and again only after it has left it,
    /// which a jump straight from one corner to another does too.
    pub fn corner_motion(&mut self, pos: Point<f64, Logical>) {
        let corner = self
            .space
            .outputs()
            .filter_map(|output| self.space.output_geometry(output))
            .find(|geometry| geometry.to_f64().contains(pos))
            .and_then(|geometry| {
                let geometry = geometry.to_f64();
                let left = pos.x < geometry.loc.x + 1.0;
                let right = pos.x >= geometry.loc.x + geometry.size.w - 1.0;
                let top = pos.y < geometry.loc.y + 1.0;
                let bottom = pos.y >= geometry.loc.y + geometry.size.h - 1.0;
                match (top, bottom, left, right) {
                    (true, _, true, _) => Some((0, self.config.hot_corner_top_left)),
                    (true, _, _, true) => Some((1, self.config.hot_corner_top_right)),
                    (_, true, true, _) => Some((2, self.config.hot_corner_bottom_left)),
                    (_, true, _, true) => Some((3, self.config.hot_corner_bottom_right)),
                    _ => None,
                }
            });
        let entered = corner.map(|(which, _)| which);
        if entered == self.in_hot_corner {
            return;
        }
        self.in_hot_corner = entered;
        match corner.map(|(_, action)| action) {
            Some(HotCorner::Overview) => self.toggle_overview(),
            Some(HotCorner::Apps) => self.show_apps(""),
            Some(HotCorner::Lock) => self.lock_screen(),
            Some(HotCorner::None) | None => {}
        }
    }
}
