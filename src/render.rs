//! What an output shows, as render elements. Both backends draw the same scene.

use smithay::{
    backend::renderer::{
        element::{
            memory::MemoryRenderBufferRenderElement, render_elements, surface::WaylandSurfaceRenderElement,
            AsRenderElements,
        },
        gles::GlesRenderer,
    },
    desktop::{
        layer_map_for_output,
        space::{space_render_elements, SpaceRenderElements},
    },
    output::Output,
    utils::Scale,
    wayland::shell::wlr_layer::Layer,
};

use crate::Nekoland;

pub const CLEAR_COLOR: [f32; 4] = [0.1, 0.1, 0.1, 1.0];

type SurfaceElement = WaylandSurfaceRenderElement<GlesRenderer>;

render_elements! {
    pub SceneElement<=GlesRenderer>;
    Space=SpaceRenderElements<GlesRenderer, SurfaceElement>,
    Surface=SurfaceElement,
    Picture=MemoryRenderBufferRenderElement<GlesRenderer>,
}

/// Everything on an output but the cursor, front to back.
pub fn scene_elements(
    state: &Nekoland,
    renderer: &mut GlesRenderer,
    output: &Output,
) -> Result<Vec<SceneElement>, Box<dyn std::error::Error>> {
    // A fullscreen window is all there is to see, under the overlay layer.
    if let Some(window) = state.fullscreen_window() {
        let output_loc = state
            .space
            .output_geometry(output)
            .ok_or("output is not mapped")?
            .loc;
        let location = state.space.element_location(&window).unwrap_or_default();
        let mut elements = layer_elements(renderer, output, [Layer::Overlay, Layer::Overlay]);
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
    Ok(space_render_elements(renderer, [&state.space], output, 1.0)?
        .into_iter()
        .map(SceneElement::Space)
        .collect())
}

/// The surfaces of some layers.
fn layer_elements(renderer: &mut GlesRenderer, output: &Output, layers: [Layer; 2]) -> Vec<SceneElement> {
    let map = layer_map_for_output(output);
    layers
        .into_iter()
        .flat_map(|layer| map.layers_on(layer).rev())
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
