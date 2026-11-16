//! What an output shows, as render elements. Both backends draw the same scene.

use smithay::{
    backend::renderer::{
        element::{
            memory::MemoryRenderBufferRenderElement,
            render_elements,
            solid::SolidColorRenderElement,
            surface::WaylandSurfaceRenderElement,
            utils::{CropRenderElement, Relocate, RelocateRenderElement, RescaleRenderElement},
            AsRenderElements, Id, Kind,
        },
        gles::GlesRenderer,
        utils::CommitCounter,
    },
    desktop::{layer_map_for_output, LayerSurface, Window},
    output::Output,
    utils::{Logical, Physical, Point, Rectangle, Scale, Size},
    wayland::shell::wlr_layer::Layer,
};

use crate::{
    glass::{Glass, Pane},
    menubar::Overlay,
    overview::ICON_SIZE,
    state::{ABOVE_WINDOWS, BELOW_WINDOWS},
    text::Picture,
    windows::window_names,
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
    Desktop=CropRenderElement<RelocateRenderElement<RescaleRenderElement<SurfaceElement>>>,
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
            let mut elements = layer_elements(renderer, output, [Layer::Overlay, Layer::Overlay], false);
            elements.extend(window_elements(state, renderer, &window, output_loc));
            return Ok(elements);
        }
        let output_geometry = state
            .space
            .output_geometry(output)
            .ok_or("output is not mapped")?;
        // Windows the configuration puts on glass float over the rest, as the panels do.
        let floating: Vec<Window> = state
            .space
            .elements()
            .rev()
            .filter(|window| state.config.glass.contains_key(&window_names(window).1))
            .cloned()
            .collect();
        let windows: Vec<Window> = state
            .space
            .elements()
            .rev()
            .filter(|window| !floating.contains(window) && !state.dock_draws(window))
            .cloned()
            .collect();
        let mut under: Vec<SceneElement> = windows
            .iter()
            .flat_map(|window| window_elements(state, renderer, window, output_geometry.loc))
            .collect();
        under.extend(layer_elements(renderer, output, BELOW_WINDOWS, false));
        let mut overlays = state.menu_bar_overlays();
        overlays.extend(state.notice_overlays());
        overlays.extend(state.dock_overlays());
        let on_top = OnTop {
            overlays: &overlays,
            windows: &floating,
            without_top_panel: false,
        };
        let mut elements = dock_front(state, renderer, output_geometry.loc);
        elements.extend(floating_elements(state, renderer, output, glass, &under, &on_top));
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

    // A window being dragged goes over everything else in the overview, without its name.
    let dragged = overview.dragged();
    let window_thumbnail = |renderer: &mut GlesRenderer, slot: &crate::overview::Slot| {
        let geometry = slot.window.geometry();
        let natural = (slot.from.loc - geometry.loc - output_geometry.loc).to_physical(1);
        let origin = (slot.from.loc - output_geometry.loc).to_physical(1);
        let offset = (slot.now.loc - slot.from.loc).to_physical(1);
        let scale = f64::from(slot.now.size.w) / f64::from(slot.from.size.w.max(1));
        AsRenderElements::<GlesRenderer>::render_elements::<SurfaceElement>(
            &slot.window,
            renderer,
            natural,
            Scale::from(1.0),
            1.0,
        )
        .into_iter()
        .map(move |element| {
            let scaled = RescaleRenderElement::from_element(element, origin, scale);
            SceneElement::Thumbnail(RelocateRenderElement::from_element(
                scaled,
                offset,
                Relocate::Relative,
            ))
        })
        .collect::<Vec<_>>()
    };
    // Put in front of the scene, the bar included, once it is put together.
    let dragged_elements: Vec<SceneElement> = dragged
        .and_then(|index| overview.slots.get(index))
        .map(|slot| window_thumbnail(renderer, slot))
        .unwrap_or_default();
    let resting = || {
        overview
            .slots
            .iter()
            .enumerate()
            .rev()
            .filter(move |(index, _)| Some(*index) != dragged)
            .map(|(_, slot)| slot)
    };

    // The icon straddles the window's bottom edge, with the name under it.
    for slot in resting() {
        if let Some(badge) = &slot.badge {
            let at = Point::from((
                slot.now.loc.x + (slot.now.size.w - badge.size.w) / 2,
                slot.now.loc.y + slot.now.size.h - ICON_SIZE / 2,
            ));
            elements.extend(picture(renderer, badge, at));
        }
    }

    for slot in resting() {
        elements.extend(window_thumbnail(renderer, slot));
    }

    elements.push(SceneElement::Solid(SolidColorRenderElement::from_buffer(
        &overview.dim,
        (0, 0),
        1.0,
        1.0,
        Kind::Unspecified,
    )));
    elements.extend(layer_elements(renderer, output, BELOW_WINDOWS, false));
    let mut overlays = state.overview_overlays();
    overlays.extend(state.dock_overlays());
    let on_top = OnTop {
        overlays: &overlays,
        windows: &[],
        without_top_panel: true,
    };
    let floating = floating_elements(state, renderer, output, glass, &elements, &on_top);
    // Each desktop as it is, small, in the opened-up bar, over everything, and the close
    // button of the one the pointer is on over that.
    let mut scene = dragged_elements;
    scene.extend(
        state
            .overview_close_button()
            .and_then(|(rect, button)| picture(renderer, button, rect.loc)),
    );
    scene.extend(desktop_thumbnails(state, renderer, output, output_geometry.loc));
    scene.extend(dock_front(state, renderer, output_geometry.loc));
    scene.extend(floating);
    scene.extend(elements);
    Ok(scene)
}

/// Every desktop's wallpaper and windows, scaled down into its place in the desktops' bar,
/// fading in as the bar opens up, and cut off where it ends.
fn desktop_thumbnails(
    state: &Nekoland,
    renderer: &mut GlesRenderer,
    output: &Output,
    output_loc: Point<i32, Logical>,
) -> Vec<SceneElement> {
    let expansion = state.overview_expansion() as f32;
    let Some(panel) = state.overview_panel().filter(|_| expansion > 0.0) else {
        return Vec::new();
    };
    let crop = Rectangle::new(panel.loc - output_loc, panel.size).to_physical(1);
    let output_width = state
        .space
        .output_geometry(output)
        .map_or(1, |geometry| geometry.size.w.max(1));
    let map = layer_map_for_output(output);
    let mut elements = Vec::new();
    for (index, rect) in state.overview_thumbnails() {
        let scale = f64::from(rect.size.w) / f64::from(output_width);
        let offset = (rect.loc - output_loc).to_physical(1);
        let mut surfaces: Vec<SurfaceElement> = Vec::new();
        // Front to back: the windows, top of the stack first, then the wallpaper.
        for (window, location) in state.desktop_windows(index).into_iter().rev() {
            surfaces.extend(
                AsRenderElements::<GlesRenderer>::render_elements::<SurfaceElement>(
                    &window,
                    renderer,
                    (location - window.geometry().loc - output_loc).to_physical(1),
                    Scale::from(1.0),
                    expansion,
                ),
            );
        }
        for surface in BELOW_WINDOWS
            .into_iter()
            .flat_map(|layer| map.layers_on(layer).rev())
        {
            let location = map.layer_geometry(surface).unwrap_or_default().loc;
            surfaces.extend(
                AsRenderElements::<GlesRenderer>::render_elements::<SurfaceElement>(
                    surface,
                    renderer,
                    location.to_physical(1),
                    Scale::from(1.0),
                    expansion,
                ),
            );
        }
        elements.extend(surfaces.into_iter().filter_map(|surface| {
            let scaled = RescaleRenderElement::from_element(surface, (0, 0).into(), scale);
            let placed = RelocateRenderElement::from_element(scaled, offset, Relocate::Relative);
            CropRenderElement::from_element(placed, 1.0, crop).map(SceneElement::Desktop)
        }));
    }
    elements
}

/// What the Dock shows live over its glass, front first: the icons at the corners of the
/// minimized windows' tiles, and the windows themselves, each fitted to its place.
fn dock_front(
    state: &Nekoland,
    renderer: &mut GlesRenderer,
    output_loc: Point<i32, Logical>,
) -> Vec<SceneElement> {
    let mut elements: Vec<SceneElement> = state
        .dock_badge_pictures()
        .into_iter()
        .filter_map(|(badge, at)| {
            MemoryRenderBufferRenderElement::from_buffer(
                renderer,
                (at - output_loc).to_physical(1).to_f64(),
                &badge.buffer,
                None,
                None,
                None,
                Kind::Unspecified,
            )
            .ok()
            .map(SceneElement::Picture)
        })
        .collect();
    for (window, rect) in state.dock_thumbnails() {
        elements.extend(window_fitted(renderer, &window, rect, output_loc));
    }
    elements
}

/// A window scaled down to fit `rect`, in the middle of it, its shadow cut off.
fn window_fitted(
    renderer: &mut GlesRenderer,
    window: &Window,
    rect: Rectangle<i32, Logical>,
    output_loc: Point<i32, Logical>,
) -> Vec<SceneElement> {
    let geometry = window.geometry();
    if geometry.size.w <= 0 || geometry.size.h <= 0 {
        return Vec::new();
    }
    let scale = (f64::from(rect.size.w) / f64::from(geometry.size.w))
        .min(f64::from(rect.size.h) / f64::from(geometry.size.h));
    let size = Size::from((
        (f64::from(geometry.size.w) * scale).round() as i32,
        (f64::from(geometry.size.h) * scale).round() as i32,
    ));
    let at = rect.loc + Point::from(((rect.size.w - size.w) / 2, (rect.size.h - size.h) / 2)) - output_loc;
    let crop = Rectangle::new(at, size).to_physical(1);
    AsRenderElements::<GlesRenderer>::render_elements::<SurfaceElement>(
        window,
        renderer,
        Point::from((-geometry.loc.x, -geometry.loc.y)).to_physical(1),
        Scale::from(1.0),
        1.0,
    )
    .into_iter()
    .filter_map(|surface| {
        let scaled = RescaleRenderElement::from_element(surface, (0, 0).into(), scale);
        let placed = RelocateRenderElement::from_element(scaled, at.to_physical(1), Relocate::Relative);
        CropRenderElement::from_element(placed, 1.0, crop).map(SceneElement::Desktop)
    })
    .collect()
}

/// What goes over the windows: the compositor's own overlays, windows on glass, and the
/// layers above the windows, less a top panel where the overview's bar takes its place.
struct OnTop<'a> {
    overlays: &'a [Overlay<'a>],
    windows: &'a [Window],
    without_top_panel: bool,
}

/// What goes over `under`, each overlay, window and surface the configuration names on
/// glass that shows `under` blurred.
fn floating_elements(
    state: &Nekoland,
    renderer: &mut GlesRenderer,
    output: &Output,
    mut glass: Option<&mut Glass>,
    under: &[SceneElement],
    on_top: &OnTop<'_>,
) -> Vec<SceneElement> {
    let OnTop {
        overlays,
        windows,
        without_top_panel,
    } = *on_top;
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

    let output_loc = state
        .space
        .output_geometry(output)
        .map(|geometry| geometry.loc)
        .unwrap_or_default();
    let areas: Vec<_> = overlays
        .iter()
        .filter(|overlay| overlay.glass.is_some())
        .map(|overlay| Rectangle::new(overlay.rect.loc - output_loc, overlay.rect.size).to_physical(1))
        .chain(
            surfaces
                .iter()
                .filter(|surface| radius_of(surface).is_some())
                .map(|surface| map.layer_geometry(surface).unwrap_or_default().to_physical(1)),
        )
        .chain(
            windows
                .iter()
                .filter_map(|window| window_area(state, window, output_loc)),
        )
        .collect();
    if let (Some(glass), Some(mode)) = (glass.as_deref_mut(), output.current_mode()) {
        if !areas.is_empty() {
            if let Err(err) = glass.update(renderer, mode.size, &areas, under) {
                tracing::warn!("glass: {err}");
            }
        }
    }

    let mut elements = Vec::new();
    for overlay in overlays {
        let rect = Rectangle::new(overlay.rect.loc - output_loc, overlay.rect.size).to_physical(1);
        // An overlay smaller than its picture shows as much of it as fits, from the top left.
        let fits = Size::from((
            overlay.rect.size.w.min(overlay.picture.size.w),
            overlay.rect.size.h.min(overlay.picture.size.h),
        ));
        let crop = (fits != overlay.picture.size).then_some(fits);
        elements.extend(
            MemoryRenderBufferRenderElement::from_buffer(
                renderer,
                rect.loc.to_f64(),
                &overlay.picture.buffer,
                Some(overlay.alpha * overlay.fade),
                crop.map(|size| Rectangle::from_size(size).to_f64()),
                crop,
                Kind::Unspecified,
            )
            .ok()
            .map(SceneElement::Picture),
        );
        elements.extend(
            glass
                .as_deref_mut()
                .zip(overlay.glass)
                .and_then(|(glass, radius)| glass.pane_with(overlay.id.clone(), rect, radius))
                .map(|pane| SceneElement::Glass(pane.faded(overlay.fade))),
        );
    }
    for window in windows {
        elements.extend(window_elements(state, renderer, window, output_loc));
        let radius = state
            .config
            .glass
            .get(&window_names(window).1)
            .copied()
            .unwrap_or_default();
        if let Some(area) = window_area(state, window, output_loc) {
            elements.extend(
                glass
                    .as_deref_mut()
                    .and_then(|glass| glass.pane_with(window_glass_id(window), area, radius))
                    .map(SceneElement::Glass),
            );
        }
    }
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

/// A window's surfaces, with its popups, where it is on the output.
fn window_elements(
    state: &Nekoland,
    renderer: &mut GlesRenderer,
    window: &Window,
    output_loc: Point<i32, Logical>,
) -> Vec<SceneElement> {
    let location = state.space.element_location(window).unwrap_or_default();
    AsRenderElements::<GlesRenderer>::render_elements::<SurfaceElement>(
        window,
        renderer,
        (location - window.geometry().loc - output_loc).to_physical(1),
        Scale::from(1.0),
        1.0,
    )
    .into_iter()
    .map(SceneElement::Surface)
    .collect()
}

/// The part of the output a window covers, without its shadow.
fn window_area(
    state: &Nekoland,
    window: &Window,
    output_loc: Point<i32, Logical>,
) -> Option<Rectangle<i32, Physical>> {
    let location = state.space.element_location(window)?;
    Some(Rectangle::new(location - output_loc, window.geometry().size).to_physical(1))
}

/// What a window's glass goes by from frame to frame: the window keeps it.
fn window_glass_id(window: &Window) -> Id {
    window.user_data().get_or_insert(Id::new).clone()
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
