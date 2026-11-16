//! What an output shows, as render elements. Both backends draw the same scene.

use smithay::{
    backend::renderer::{
        element::{
            memory::MemoryRenderBufferRenderElement,
            render_elements,
            solid::SolidColorRenderElement,
            surface::WaylandSurfaceRenderElement,
            utils::{Relocate, RelocateRenderElement, RescaleRenderElement},
            AsRenderElements, Kind,
        },
        gles::GlesRenderer,
        utils::CommitCounter,
    },
    desktop::{layer_map_for_output, LayerSurface},
    output::Output,
    utils::{Logical, Point, Rectangle, Scale, Size},
    wayland::shell::wlr_layer::Layer,
};

use crate::{
    glass::{Glass, Pane},
    overview::{BAR_HEIGHT, ICON_SIZE},
    state::{ABOVE_WINDOWS, BELOW_WINDOWS},
    text::Picture,
    Nekoland,
};

pub const CLEAR_COLOR: [f32; 4] = [0.1, 0.1, 0.1, 1.0];
/// The outline around the window under the pointer in the overview.
const OUTLINE: i32 = 4;
const OUTLINE_COLOR: [f32; 4] = [0.04, 0.52, 1.0, 1.0];
/// A layer surface at the top edge no taller than this is a panel.
const TOP_PANEL_MAX_HEIGHT: i32 = 96;

type SurfaceElement = WaylandSurfaceRenderElement<GlesRenderer>;

render_elements! {
    pub SceneElement<=GlesRenderer>;
    Surface=SurfaceElement,
    Glass=Pane,
    Thumbnail=RelocateRenderElement<RescaleRenderElement<SurfaceElement>>,
    Solid=SolidColorRenderElement,
    Picture=MemoryRenderBufferRenderElement<GlesRenderer>,
}

/// Everything on an output but the cursor, front to back.
pub fn scene_elements(
    state: &Nekoland,
    renderer: &mut GlesRenderer,
    output: &Output,
    glass: Option<&mut Glass>,
) -> Result<Vec<SceneElement>, Box<dyn std::error::Error>> {
    let Some(overview) = &state.overview else {
        // A fullscreen window is all there is to see, under the overlay layer.
        if let Some(window) = state.fullscreen_window() {
            let output_loc = state
                .space
                .output_geometry(output)
                .ok_or("output is not mapped")?
                .loc;
            let location = state.space.element_location(&window).unwrap_or_default();
            let mut elements = layer_elements(renderer, output, [Layer::Overlay, Layer::Overlay], false);
            elements.extend(
                AsRenderElements::<GlesRenderer>::render_elements::<SurfaceElement>(
                    &window,
                    renderer,
                    (location - window.geometry().loc - output_loc).to_physical(1),
                    Scale::from(1.0),
                    1.0,
                )
                .into_iter()
                .map(SceneElement::Surface),
            );
            return Ok(elements);
        }
        let output_geometry = state
            .space
            .output_geometry(output)
            .ok_or("output is not mapped")?;
        let mut under: Vec<SceneElement> = state
            .space
            .render_elements_for_region(renderer, &output_geometry, 1.0, 1.0)
            .into_iter()
            .map(SceneElement::Surface)
            .collect();
        under.extend(layer_elements(renderer, output, BELOW_WINDOWS, false));
        let mut elements = floating_elements(state, renderer, output, glass, &under, false);
        elements.extend(under);
        return Ok(elements);
    };

    // The overview: the desktops' bar and the dock, then the windows spread apart, named, over a dimmed wallpaper.
    let output_geometry = state
        .space
        .output_geometry(output)
        .ok_or("output is not mapped")?;
    let progress = overview.progress();
    let fade = progress as f32;
    let to_output = |point: Point<i32, Logical>| (point - output_geometry.loc).to_f64().to_physical(1.0);
    let picture = |renderer: &mut GlesRenderer, picture: &Picture, at: Point<i32, Logical>| {
        MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            to_output(at),
            &picture.buffer,
            Some(fade),
            None,
            None,
            Kind::Unspecified,
        )
        .ok()
        .map(SceneElement::Picture)
    };

    // Everything under the dock and other floating layers, which their glass shows blurred.
    let mut elements = Vec::new();

    // The bar slides down from the top edge.
    let slide = Point::from((0, ((progress - 1.0) * f64::from(BAR_HEIGHT)).round() as i32));
    for item in &overview.bar {
        elements.extend(picture(renderer, &item.picture, item.rect.loc + slide));
    }
    elements.push(SceneElement::Solid(SolidColorRenderElement::from_buffer(
        &overview.bar_background,
        slide.to_physical(1),
        1.0,
        1.0,
        Kind::Unspecified,
    )));

    if let Some(slot) = overview.hovered.and_then(|index| overview.slots.get(index)) {
        let outer = Rectangle::new(
            slot.now.loc - Point::from((OUTLINE, OUTLINE)) - output_geometry.loc,
            slot.now.size + Size::from((OUTLINE * 2, OUTLINE * 2)),
        );
        let sides = [
            Rectangle::new(outer.loc, Size::from((outer.size.w, OUTLINE))),
            Rectangle::new(
                outer.loc + Point::from((0, outer.size.h - OUTLINE)),
                Size::from((outer.size.w, OUTLINE)),
            ),
            Rectangle::new(outer.loc, Size::from((OUTLINE, outer.size.h))),
            Rectangle::new(
                outer.loc + Point::from((outer.size.w - OUTLINE, 0)),
                Size::from((OUTLINE, outer.size.h)),
            ),
        ];
        for (id, side) in overview.outline.iter().zip(sides) {
            elements.push(SceneElement::Solid(SolidColorRenderElement::new(
                id.clone(),
                side.to_physical(1),
                CommitCounter::default(),
                OUTLINE_COLOR,
                Kind::Unspecified,
            )));
        }
    }

    // The icon straddles the window's bottom edge, with the name under it.
    for slot in overview.slots.iter().rev() {
        if let Some(badge) = &slot.badge {
            let at = Point::from((
                slot.now.loc.x + (slot.now.size.w - badge.size.w) / 2,
                slot.now.loc.y + slot.now.size.h - ICON_SIZE / 2,
            ));
            elements.extend(picture(renderer, badge, at));
        }
    }

    for slot in overview.slots.iter().rev() {
        let geometry = slot.window.geometry();
        let natural = (slot.from.loc - geometry.loc - output_geometry.loc).to_physical(1);
        let origin = (slot.from.loc - output_geometry.loc).to_physical(1);
        let offset = (slot.now.loc - slot.from.loc).to_physical(1);
        let scale = f64::from(slot.now.size.w) / f64::from(slot.from.size.w.max(1));
        elements.extend(
            AsRenderElements::<GlesRenderer>::render_elements::<SurfaceElement>(
                &slot.window,
                renderer,
                natural,
                Scale::from(1.0),
                1.0,
            )
            .into_iter()
            .map(|element| {
                let scaled = RescaleRenderElement::from_element(element, origin, scale);
                SceneElement::Thumbnail(RelocateRenderElement::from_element(
                    scaled,
                    offset,
                    Relocate::Relative,
                ))
            }),
        );
    }

    elements.push(SceneElement::Solid(SolidColorRenderElement::from_buffer(
        &overview.dim,
        (0, 0),
        1.0,
        1.0,
        Kind::Unspecified,
    )));
    elements.extend(layer_elements(renderer, output, BELOW_WINDOWS, false));
    let mut scene = floating_elements(state, renderer, output, glass, &elements, true);
    scene.extend(elements);
    Ok(scene)
}

/// The layers above the windows, each surface the configuration names on glass over `under`.
fn floating_elements(
    state: &Nekoland,
    renderer: &mut GlesRenderer,
    output: &Output,
    mut glass: Option<&mut Glass>,
    under: &[SceneElement],
    without_top_panel: bool,
) -> Vec<SceneElement> {
    let output_width = state
        .space
        .output_geometry(output)
        .map_or(i32::MAX, |geometry| geometry.size.w);
    let map = layer_map_for_output(output);
    let radius_of = |surface: &LayerSurface| state.config.glass.get(surface.namespace()).copied();
    let surfaces: Vec<LayerSurface> = ABOVE_WINDOWS
        .into_iter()
        .flat_map(|layer| map.layers_on(layer).rev())
        .filter(|surface| !(without_top_panel && is_top_panel(&map, surface)))
        .cloned()
        .collect();

    let areas: Vec<_> = surfaces
        .iter()
        .filter(|surface| radius_of(surface).is_some())
        .map(|surface| map.layer_geometry(surface).unwrap_or_default().to_physical(1))
        .collect();
    if let (Some(glass), Some(mode)) = (glass.as_deref_mut(), output.current_mode()) {
        if !areas.is_empty() {
            if let Err(err) = glass.update(renderer, mode.size, &areas, under) {
                tracing::warn!("glass: {err}");
            }
        }
    }

    let mut elements = Vec::new();
    for surface in &surfaces {
        let geometry = map.layer_geometry(surface).unwrap_or_default();
        elements.extend(
            AsRenderElements::<GlesRenderer>::render_elements::<SurfaceElement>(
                surface,
                renderer,
                geometry.loc.to_physical(1),
                Scale::from(1.0),
                1.0,
            )
            .into_iter()
            .map(SceneElement::Surface),
        );
        if let Some(radius) = radius_of(surface) {
            // A bar across the whole output has no corners to round.
            let radius = if geometry.size.w >= output_width {
                0
            } else {
                radius
            };
            elements.extend(
                glass
                    .as_deref_mut()
                    .and_then(|glass| glass.pane(surface.wl_surface(), geometry.to_physical(1), radius))
                    .map(SceneElement::Glass),
            );
        }
    }
    elements
}

fn is_top_panel(map: &smithay::desktop::LayerMap, surface: &LayerSurface) -> bool {
    let geometry = map.layer_geometry(surface).unwrap_or_default();
    geometry.loc.y == 0 && geometry.size.h <= TOP_PANEL_MAX_HEIGHT
}

/// The surfaces of some layers. In the overview the desktops' bar takes the place of
/// a panel at the top edge, which is then left out.
fn layer_elements(
    renderer: &mut GlesRenderer,
    output: &Output,
    layers: [Layer; 2],
    without_top_panel: bool,
) -> Vec<SceneElement> {
    let map = layer_map_for_output(output);
    layers
        .into_iter()
        .flat_map(|layer| map.layers_on(layer).rev())
        .filter(|surface| !(without_top_panel && is_top_panel(&map, surface)))
        .flat_map(|surface| {
            let location = map.layer_geometry(surface).unwrap_or_default().loc;
            AsRenderElements::<GlesRenderer>::render_elements::<SurfaceElement>(
                surface,
                renderer,
                location.to_physical(1),
                Scale::from(1.0),
                1.0,
            )
        })
        .map(SceneElement::Surface)
        .collect()
}
