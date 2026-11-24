//! ext-background-effect-v1: where a program asks for the blur behind its surface, which
//! Nekoland draws as Catseye glass. Only input methods' candidates get it for now, as other
//! popups and windows have glass already (see render.rs).

use std::sync::Mutex;

use smithay::reexports::wayland_protocols::ext::background_effect::v1::server::{
    ext_background_effect_manager_v1::{self, Capability, ExtBackgroundEffectManagerV1},
    ext_background_effect_surface_v1::{self, ExtBackgroundEffectSurfaceV1},
};
use smithay::reexports::wayland_server::{
    protocol::wl_surface::WlSurface, Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
};
use smithay::utils::{Logical, Rectangle};
use smithay::wayland::compositor::{get_region_attributes, with_states, RectangleKind};

use crate::Nekoland;

pub fn create_global(dh: &DisplayHandle) {
    dh.create_global::<Nekoland, ExtBackgroundEffectManagerV1, _>(1, ());
}

/// The blur a surface asked for: the region's bounds, and the radius of its rounded corners.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Blur {
    pub area: Rectangle<i32, Logical>,
    pub radius: i32,
}

/// A surface's effect object, while it has one, and the blur it set.
#[derive(Default)]
struct Effect {
    bound: bool,
    blur: Option<Blur>,
}

#[derive(Default)]
struct EffectData(Mutex<Effect>);

fn with_effect<T>(surface: &WlSurface, then: impl FnOnce(&mut Effect) -> T) -> T {
    with_states(surface, |states| {
        states.data_map.insert_if_missing_threadsafe(EffectData::default);
        let effect = states.data_map.get::<EffectData>().expect("inserted above");
        then(&mut effect.0.lock().unwrap())
    })
}

/// The blur `surface` asked for, in its own coordinates.
pub fn blur_of(surface: &WlSurface) -> Option<Blur> {
    with_effect(surface, |effect| effect.blur)
}

/// shortcut: a region is taken as one rounded rectangle given a row at a time through its
/// corners, as Project Key sends it; other shapes would need a mask.
fn blur_from(rects: &[(RectangleKind, Rectangle<i32, Logical>)]) -> Option<Blur> {
    let added: Vec<_> = rects
        .iter()
        .filter(|(kind, _)| matches!(kind, RectangleKind::Add))
        .map(|(_, rect)| *rect)
        .collect();
    let area = added.iter().copied().reduce(|a, b| a.merge(b))?;
    // The corners' rows are above and below the body, the tallest rectangle.
    let body = added.iter().max_by_key(|rect| rect.size.h)?;
    (!area.is_empty()).then_some(Blur {
        area,
        radius: body.loc.y - area.loc.y,
    })
}

impl GlobalDispatch<ExtBackgroundEffectManagerV1, ()> for Nekoland {
    fn bind(
        _state: &mut Self,
        _dh: &DisplayHandle,
        _client: &Client,
        resource: New<ExtBackgroundEffectManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        let manager = data_init.init(resource, ());
        manager.capabilities(Capability::Blur);
    }
}

impl Dispatch<ExtBackgroundEffectManagerV1, ()> for Nekoland {
    fn request(
        _state: &mut Self,
        _client: &Client,
        manager: &ExtBackgroundEffectManagerV1,
        request: ext_background_effect_manager_v1::Request,
        _data: &(),
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        if let ext_background_effect_manager_v1::Request::GetBackgroundEffect { id, surface } = request {
            if with_effect(&surface, |effect| std::mem::replace(&mut effect.bound, true)) {
                manager.post_error(
                    ext_background_effect_manager_v1::Error::BackgroundEffectExists,
                    "the surface has a background effect already",
                );
                return;
            }
            data_init.init(id, surface);
        }
    }
}

impl Dispatch<ExtBackgroundEffectSurfaceV1, WlSurface> for Nekoland {
    fn request(
        _state: &mut Self,
        _client: &Client,
        effect: &ExtBackgroundEffectSurfaceV1,
        request: ext_background_effect_surface_v1::Request,
        surface: &WlSurface,
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        if let ext_background_effect_surface_v1::Request::SetBlurRegion { region } = request {
            if !surface.is_alive() {
                effect.post_error(ext_background_effect_surface_v1::Error::SurfaceDestroyed, "the surface is gone");
                return;
            }
            // shortcut: taken at once rather than at the surface's next commit, which clients
            // send right after anyway.
            let blur = region.and_then(|region| blur_from(&get_region_attributes(&region).rects));
            with_effect(surface, |effect| effect.blur = blur);
        }
    }

    fn destroyed(
        _state: &mut Self,
        _client: smithay::reexports::wayland_server::backend::ClientId,
        _effect: &ExtBackgroundEffectSurfaceV1,
        surface: &WlSurface,
    ) {
        if surface.is_alive() {
            with_effect(surface, |effect| *effect = Effect::default());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounded_rectangle_by_rows() {
        let row = |x, y, w, h| (RectangleKind::Add, Rectangle::new((x, y).into(), (w, h).into()));
        // A 20x10 rectangle at (2, 3) with corners of radius 2, as Project Key sends it.
        let rects = [row(3, 3, 18, 1), row(3, 12, 18, 1), row(2, 4, 20, 1), row(2, 11, 20, 1), row(2, 5, 20, 6)];
        let blur = blur_from(&rects).unwrap();
        assert_eq!(blur.area, Rectangle::new((2, 3).into(), (20, 10).into()));
        assert_eq!(blur.radius, 2);
        assert_eq!(blur_from(&[]), None);
    }
}
