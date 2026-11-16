//! What an output shows, as render elements. Both backends draw the same scene.

use smithay::{
    backend::renderer::{
        element::{
            memory::MemoryRenderBufferRenderElement,
            render_elements,
            solid::SolidColorRenderElement,
            surface::{render_elements_from_surface_tree, WaylandSurfaceRenderElement},
            utils::{CropRenderElement, Relocate, RelocateRenderElement, RescaleRenderElement},
            AsRenderElements, Id, Kind,
        },
        gles::GlesRenderer,
        utils::CommitCounter,
    },
    desktop::{layer_map_for_output, LayerSurface, PopupKind, PopupManager, Window},
    output::Output,
    utils::{Logical, Physical, Point, Rectangle, Scale, Size},
    wayland::{
        compositor::{with_states, SurfaceAttributes},
        shell::{wlr_layer::Layer, xdg::XdgPopupSurfaceData},
    },
};

use crate::{
    config::{DockPosition, MinimizeEffect},
    dock::Frame,
    glass::{Glass, Pane, Tier},
    menubar::Overlay,
    overview::ICON_SIZE,
    state::{ABOVE_WINDOWS, BELOW_WINDOWS},
    text::Picture,
    windows::{has_square_corners, window_names, CONFIRM_APP_ID},
    Nekoland,
};

pub const CLEAR_COLOR: [f32; 4] = [0.1, 0.1, 0.1, 1.0];
/// The outline around the window under the pointer in the overview.
const OUTLINE: i32 = 4;
pub(crate) const OUTLINE_COLOR: [f32; 4] = [0.04, 0.52, 1.0, 1.0];
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
    mut glass: Option<&mut Glass>,
) -> Result<Vec<SceneElement>, Box<dyn std::error::Error>> {
    let scale = output.current_scale().fractional_scale();
    // Locked, the locker's surface is all there is; until it has one, the clear colour.
    if state.lock.is_some() {
        return Ok(state
            .lock_surface(output)
            .map(|surface| {
                render_elements_from_surface_tree(
                    renderer,
                    surface.wl_surface(),
                    (0, 0),
                    scale,
                    1.0,
                    Kind::Unspecified,
                )
                .into_iter()
                .map(SceneElement::Surface)
                .collect()
            })
            .unwrap_or_default());
    }
    let Some(overview) = &state.overview else {
        // A fullscreen window is all there is to see, under the overlay layer.
        if let Some(window) = state.fullscreen_window() {
            let output_loc = state
                .space
                .output_geometry(output)
                .ok_or("output is not mapped")?
                .loc;
            let mut elements = layer_elements(renderer, output, [Layer::Overlay, Layer::Overlay], false);
            elements.extend(window_elements(state, renderer, &window, output_loc, scale));
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
        let under = windows_on_glass(state, renderer, output, glass.as_deref_mut(), &windows);
        let mut overlays = state.hover_overlays();
        overlays.extend(state.menu_bar_overlays());
        overlays.extend(state.notice_overlays());
        overlays.extend(state.dock_overlays());
        let on_top = OnTop {
            overlays: &overlays,
            windows: &floating,
            popups_of: &windows,
            without_top_panel: false,
        };
        let mut elements: Vec<SceneElement> = state
            .desktop_front_elements(renderer, output_geometry.loc, scale)
            .into_iter()
            .collect();
        elements.extend(dock_front(state, renderer, output_geometry.loc, scale));
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
    let to_output = |point: Point<i32, Logical>| (point - output_geometry.loc).to_f64().to_physical(scale);
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

    // With the applications shown, the windows are not.
    let apps = overview.apps.is_some();
    if let Some(slot) = overview
        .hovered
        .filter(|_| !apps)
        .and_then(|index| overview.slots.get(index))
    {
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
                side.to_physical_precise_round(scale),
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
        let natural = (slot.from.loc - geometry.loc - output_geometry.loc).to_physical_precise_round(scale);
        let origin = (slot.from.loc - output_geometry.loc).to_physical_precise_round(scale);
        let offset = (slot.now.loc - slot.from.loc).to_physical_precise_round(scale);
        let factor = f64::from(slot.now.size.w) / f64::from(slot.from.size.w.max(1));
        AsRenderElements::<GlesRenderer>::render_elements::<SurfaceElement>(
            &slot.window,
            renderer,
            natural,
            Scale::from(scale),
            1.0,
        )
        .into_iter()
        .map(move |element| {
            let scaled = RescaleRenderElement::from_element(element, origin, factor);
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
    for slot in resting().filter(|_| !apps) {
        if let Some(badge) = &slot.badge {
            let at = Point::from((
                slot.now.loc.x + (slot.now.size.w - badge.size.w) / 2,
                slot.now.loc.y + slot.now.size.h - ICON_SIZE / 2,
            ));
            elements.extend(picture(renderer, badge, at));
        }
    }

    for slot in resting().filter(|_| !apps) {
        elements.extend(window_thumbnail(renderer, slot));
    }

    elements.push(SceneElement::Solid(SolidColorRenderElement::from_buffer(
        &overview.dim,
        (0, 0),
        scale,
        1.0,
        Kind::Unspecified,
    )));
    elements.extend(layer_elements(renderer, output, BELOW_WINDOWS, false));
    // The applications in front of the desktops' bar, on glass behind everything else.
    let (mut overlays, backdrop) = state.apps_overlays();
    overlays.extend(state.overview_overlays());
    overlays.extend(state.dock_overlays());
    overlays.extend(backdrop);
    let on_top = OnTop {
        overlays: &overlays,
        windows: &[],
        popups_of: &[],
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
    scene.extend(dock_front(state, renderer, output_geometry.loc, scale));
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
    let scale = output.current_scale().fractional_scale();
    let crop = Rectangle::new(panel.loc - output_loc, panel.size).to_physical_precise_round(scale);
    let output_width = state
        .space
        .output_geometry(output)
        .map_or(1, |geometry| geometry.size.w.max(1));
    let map = layer_map_for_output(output);
    let mut elements = Vec::new();
    for (index, rect) in state.overview_thumbnails() {
        let factor = f64::from(rect.size.w) / f64::from(output_width);
        let offset = (rect.loc - output_loc).to_physical_precise_round(scale);
        let mut surfaces: Vec<SurfaceElement> = Vec::new();
        // Front to back: the windows, top of the stack first, then the wallpaper.
        for (window, location) in state.desktop_windows(index).into_iter().rev() {
            surfaces.extend(
                AsRenderElements::<GlesRenderer>::render_elements::<SurfaceElement>(
                    &window,
                    renderer,
                    (location - window.geometry().loc - output_loc).to_physical_precise_round(scale),
                    Scale::from(scale),
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
                    location.to_physical_precise_round(scale),
                    Scale::from(scale),
                    expansion,
                ),
            );
        }
        elements.extend(surfaces.into_iter().filter_map(|surface| {
            let scaled = RescaleRenderElement::from_element(surface, (0, 0).into(), factor);
            let placed = RelocateRenderElement::from_element(scaled, offset, Relocate::Relative);
            CropRenderElement::from_element(placed, scale, crop).map(SceneElement::Desktop)
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
    scale: f64,
) -> Vec<SceneElement> {
    let scaled = state.config.dock_minimize_effect == MinimizeEffect::Scale;
    let mut elements: Vec<SceneElement> = state
        .dock_genies()
        .into_iter()
        .flat_map(|genie| {
            if scaled {
                // Straight to its place in the Dock, shrinking as it goes.
                let t = 1.0 - (1.0 - genie.progress.clamp(0.0, 1.0)).powi(3);
                let mix = |from: i32, to: i32| (f64::from(from) + f64::from(to - from) * t).round() as i32;
                let (from, into) = (genie.from, genie.into);
                let rect = Rectangle::new(
                    (mix(from.loc.x, into.loc.x), mix(from.loc.y, into.loc.y)).into(),
                    (mix(from.size.w, into.size.w), mix(from.size.h, into.size.h)).into(),
                );
                return window_fitted(renderer, &genie.window, rect, output_loc, scale);
            }
            draw_genie(
                renderer,
                genie.frame,
                &genie.window,
                genie.from,
                genie.into,
                genie.progress,
                output_loc,
                scale,
            )
        })
        .collect();
    elements.extend(state.dock_badge_pictures().into_iter().filter_map(|(badge, at)| {
        MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            (at - output_loc).to_f64().to_physical(scale),
            &badge.buffer,
            None,
            None,
            None,
            Kind::Unspecified,
        )
        .ok()
        .map(SceneElement::Picture)
    }));
    for (window, rect) in state.dock_thumbnails() {
        elements.extend(window_fitted(renderer, &window, rect, output_loc, scale));
    }
    elements
}

/// How tall the bands a window is cut into for its Genie are, about: thin enough that its
/// sides look curved.
const GENIE_BAND: f64 = 5.0;
/// Of a Genie's time, how much goes to bending the window toward the Dock before it is
/// drawn in.
const GENIE_BEND: f64 = 0.4;

/// A window `progress` of the way into the Dock, as the Mac's Genie draws it: first its
/// lower part bends toward `into`, the place of its picture in the Dock, then it slides
/// down the funnel that makes and into it. It is cut into bands across, each squeezed to
/// the funnel's width where it is. All this is worked out in the Dock's turned `frame`, so
/// that into a side Dock the window flows sideways, in bands from top to bottom.
fn draw_genie(
    renderer: &mut GlesRenderer,
    frame: Frame,
    window: &Window,
    from: Rectangle<i32, Logical>,
    into: Rectangle<i32, Logical>,
    progress: f64,
    output_loc: Point<i32, Logical>,
    output_scale: f64,
) -> Vec<SceneElement> {
    let geometry = window.geometry();
    if geometry.size.w <= 0 || geometry.size.h <= 0 {
        return Vec::new();
    }
    let progress = progress.clamp(0.0, 1.0);
    let smooth = |t: f64| t * t * (3.0 - 2.0 * t);
    let (bend, slide) = if progress < GENIE_BEND {
        (smooth(progress / GENIE_BEND), 0.0)
    } else {
        (1.0, smooth((progress - GENIE_BEND) / (1.0 - GENIE_BEND)))
    };
    let (from, into) = (frame.unrect(from.to_f64()), frame.unrect(into.to_f64()));
    // The window's length down the turned output, across which the bands are cut.
    let length = match frame.side {
        DockPosition::Bottom => f64::from(geometry.size.h),
        _ => f64::from(geometry.size.w),
    };
    let lerp = |a: f64, b: f64, t: f64| a + (b - a) * t;
    // How far a height on screen is down the funnel, from the window's top to its picture's.
    let depth = (into.loc.y - from.loc.y).max(1.0);
    let funnel = |y: f64| smooth(((y - from.loc.y) / depth).clamp(0.0, 1.0)) * bend;
    let row_y = |v: f64| lerp(from.loc.y + v * from.size.h, into.loc.y + v * into.size.h, slide);
    let bands = (from.size.h / GENIE_BAND).ceil().clamp(16.0, 240.0) as usize;
    let mut elements = Vec::new();
    for band in 0..bands {
        let (v0, v1) = (band as f64 / bands as f64, (band + 1) as f64 / bands as f64);
        let (y0, y1) = (row_y(v0), row_y(v1));
        if y1 - y0 < 0.25 {
            continue;
        }
        let f = funnel((y0 + y1) / 2.0);
        let left = lerp(from.loc.x, into.loc.x, f);
        let right = lerp(from.loc.x + from.size.w, into.loc.x + into.size.w, f);
        let band = frame.rect(Rectangle::new((left, y0).into(), (right - left, y1 - y0).into()));
        // The band's part of the window, across it on the output: a Dock on the left turns
        // the window's right side to the top of the turned output.
        let part = (v1 - v0) * length;
        let start = match frame.side {
            DockPosition::Left => (1.0 - v1) * length,
            _ => v0 * length,
        };
        let (scale, offset, grown) = match frame.side {
            DockPosition::Bottom => {
                let scale = Scale::from((band.size.w / f64::from(geometry.size.w), band.size.h / part));
                let offset = Point::from((band.loc.x, band.loc.y - start * scale.y));
                (scale, offset, Size::from((0.0, 1.0)))
            }
            _ => {
                let scale = Scale::from((band.size.w / part, band.size.h / f64::from(geometry.size.h)));
                let offset = Point::from((band.loc.x - start * scale.x, band.loc.y));
                (scale, offset, Size::from((1.0, 0.0)))
            }
        };
        let offset: Point<f64, Logical> = offset - output_loc.to_f64();
        // One pixel longer, so that the bands meet without a seam.
        let crop = Rectangle::<f64, Logical>::new(band.loc - output_loc.to_f64(), band.size + grown)
            .to_physical(output_scale)
            .to_i32_round();
        // Each band draws the window anew; its surfaces' elements are not copied.
        let surfaces = AsRenderElements::<GlesRenderer>::render_elements::<SurfaceElement>(
            window,
            renderer,
            Point::from((-geometry.loc.x, -geometry.loc.y)).to_physical_precise_round(output_scale),
            Scale::from(output_scale),
            1.0,
        );
        for surface in surfaces {
            let scaled = RescaleRenderElement::from_element(surface, (0, 0).into(), scale);
            let placed = RelocateRenderElement::from_element(
                scaled,
                offset.to_physical(output_scale).to_i32_round(),
                Relocate::Relative,
            );
            elements.extend(
                CropRenderElement::from_element(placed, output_scale, crop).map(SceneElement::Desktop),
            );
        }
    }
    elements
}

/// A window scaled down to fit `rect`, in the middle of it, its shadow cut off.
fn window_fitted(
    renderer: &mut GlesRenderer,
    window: &Window,
    rect: Rectangle<i32, Logical>,
    output_loc: Point<i32, Logical>,
    scale: f64,
) -> Vec<SceneElement> {
    let geometry = window.geometry();
    if geometry.size.w <= 0 || geometry.size.h <= 0 {
        return Vec::new();
    }
    let factor = (f64::from(rect.size.w) / f64::from(geometry.size.w))
        .min(f64::from(rect.size.h) / f64::from(geometry.size.h));
    let size = Size::from((
        (f64::from(geometry.size.w) * factor).round() as i32,
        (f64::from(geometry.size.h) * factor).round() as i32,
    ));
    let at = rect.loc + Point::from(((rect.size.w - size.w) / 2, (rect.size.h - size.h) / 2)) - output_loc;
    let crop = Rectangle::new(at, size).to_physical_precise_round(scale);
    AsRenderElements::<GlesRenderer>::render_elements::<SurfaceElement>(
        window,
        renderer,
        Point::from((-geometry.loc.x, -geometry.loc.y)).to_physical_precise_round(scale),
        Scale::from(scale),
        1.0,
    )
    .into_iter()
    .filter_map(|surface| {
        let scaled = RescaleRenderElement::from_element(surface, (0, 0).into(), factor);
        let placed = RelocateRenderElement::from_element(
            scaled,
            at.to_physical_precise_round(scale),
            Relocate::Relative,
        );
        CropRenderElement::from_element(placed, scale, crop).map(SceneElement::Desktop)
    })
    .collect()
}

/// What goes over the windows: the compositor's own overlays, windows on glass, and the
/// layers above the windows, less a top panel where the overview's bar takes its place.
struct OnTop<'a> {
    overlays: &'a [Overlay<'a>],
    windows: &'a [Window],
    /// Windows drawn under the glass, whose menus and other popups go over it.
    popups_of: &'a [Window],
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
        popups_of,
        without_top_panel,
    } = *on_top;
    let output_width = state
        .space
        .output_geometry(output)
        .map_or(i32::MAX, |geometry| geometry.size.w);
    let scale = output.current_scale().fractional_scale();
    // Glass corners are drawn in pixels.
    let pixels = |radius: i32| (f64::from(radius) * scale).round() as i32;
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
    let popups: Vec<(PopupKind, Point<i32, Logical>)> = popups_of
        .iter()
        .flat_map(|window| window_popups(state, window, output_loc))
        .collect();
    let areas: Vec<_> = overlays
        .iter()
        .filter_map(|overlay| {
            let (tier, _) = overlay.glass?;
            let rect = Rectangle::new(overlay.rect.loc - output_loc, overlay.rect.size);
            Some((tier, rect.to_physical_precise_round(scale)))
        })
        .chain(
            surfaces
                .iter()
                .filter(|surface| radius_of(surface).is_some())
                .map(|surface| {
                    let geometry = map.layer_geometry(surface).unwrap_or_default();
                    (Tier::Menu, geometry.to_physical_precise_round(scale))
                }),
        )
        .chain(
            windows
                .iter()
                .filter_map(|window| window_area(state, window, output_loc, scale))
                .map(|area| (Tier::Menu, area)),
        )
        .chain(
            popups
                .iter()
                .filter_map(|(popup, origin)| popup_glass_area(popup, *origin))
                .map(|area| (Tier::Menu, area.to_physical_precise_round(scale))),
        )
        .collect();
    if let (Some(glass), Some(mode)) = (glass.as_deref_mut(), output.current_mode()) {
        if !areas.is_empty() {
            if let Err(err) = glass.update(renderer, (mode.size, scale), &areas, under) {
                tracing::warn!("glass: {err}");
            }
        }
    }

    let mut elements = Vec::new();
    for overlay in overlays {
        let rect =
            Rectangle::new(overlay.rect.loc - output_loc, overlay.rect.size).to_physical_precise_round(scale);
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
                .and_then(|(glass, (tier, radius))| glass.pane_with(overlay.id.clone(), tier, rect, pixels(radius)))
                .map(|pane| SceneElement::Glass(pane.faded(overlay.fade))),
        );
    }
    for window in windows {
        elements.extend(window_elements(state, renderer, window, output_loc, scale));
        let radius = state
            .config
            .glass
            .get(&window_names(window).1)
            .copied()
            .unwrap_or_default();
        if let Some(area) = window_area(state, window, output_loc, scale) {
            elements.extend(
                glass
                    .as_deref_mut()
                    .and_then(|glass| glass.pane_with(window_glass_id(window), Tier::Menu, area, pixels(radius)))
                    .map(SceneElement::Glass),
            );
        }
    }
    for (popup, origin) in &popups {
        elements.extend(
            render_elements_from_surface_tree(
                renderer,
                popup.wl_surface(),
                (*origin - popup.geometry().loc).to_physical_precise_round(scale),
                scale,
                1.0,
                Kind::Unspecified,
            )
            .into_iter()
            .map(SceneElement::Surface),
        );
        if let Some(area) = popup_glass_area(popup, *origin) {
            elements.extend(
                glass
                    .as_deref_mut()
                    .and_then(|glass| {
                        glass.pane(
                            popup.wl_surface(),
                            Tier::Menu,
                            area.to_physical_precise_round(scale),
                            pixels(POPUP_RADIUS),
                        )
                    })
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
                geometry.loc.to_physical_precise_round(scale),
                Scale::from(scale),
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
                    .and_then(|glass| {
                        glass.pane(
                            surface.wl_surface(),
                            Tier::Menu,
                            geometry.to_physical_precise_round(scale),
                            pixels(radius),
                        )
                    })
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
    scale: f64,
) -> Vec<SceneElement> {
    let location = state.space.element_location(window).unwrap_or_default();
    AsRenderElements::<GlesRenderer>::render_elements::<SurfaceElement>(
        window,
        renderer,
        (location - window.geometry().loc - output_loc).to_physical_precise_round(scale),
        Scale::from(scale),
        1.0,
    )
    .into_iter()
    .map(SceneElement::Surface)
    .collect()
}

/// A window's own surfaces, without its popups, which go over the windows on glass.
fn window_body_elements(
    state: &Nekoland,
    renderer: &mut GlesRenderer,
    window: &Window,
    output_loc: Point<i32, Logical>,
    scale: f64,
) -> Vec<SceneElement> {
    let Some(toplevel) = window.toplevel() else {
        return window_elements(state, renderer, window, output_loc, scale);
    };
    let location = state.space.element_location(window).unwrap_or_default();
    render_elements_from_surface_tree(
        renderer,
        toplevel.wl_surface(),
        (location - window.geometry().loc - output_loc).to_physical_precise_round(scale),
        scale,
        1.0,
        Kind::Unspecified,
    )
    .into_iter()
    .map(SceneElement::Surface)
    .collect()
}

/// A window's popups, frontmost first, each with where its window geometry starts on the output.
fn window_popups(
    state: &Nekoland,
    window: &Window,
    output_loc: Point<i32, Logical>,
) -> Vec<(PopupKind, Point<i32, Logical>)> {
    let Some(toplevel) = window.toplevel() else {
        return Vec::new();
    };
    let location = state.space.element_location(window).unwrap_or_default();
    PopupManager::popups_for_surface(toplevel.wl_surface())
        .map(|(popup, offset)| (popup, location + offset - output_loc))
        .collect()
}

/// The corners of a GTK popover in Nekoland's look, as `popover > contents` has them.
const POPUP_RADIUS: i32 = 12;
/// GTK draws a popover's arrow inside its window geometry, as a band this deep on the side
/// toward what it points at.
const GTK_TAIL_HEIGHT: i32 = 12;

/// The part of the output a menu's glass covers: its window geometry, less a GTK arrow's band.
fn popup_glass_area(popup: &PopupKind, origin: Point<i32, Logical>) -> Option<Rectangle<i32, Logical>> {
    let PopupKind::Xdg(xdg) = popup else {
        return None;
    };
    let mut area = Rectangle::new(origin, popup.geometry().size);
    // shortcut: a popover with an arrow is known by GTK's input region, which is the arrow and
    // the rounded contents rather than one rectangle; other toolkits drawing arrows would need
    // a way to say where their glass goes.
    let (shaped, placed) = with_states(xdg.wl_surface(), |states| {
        let shaped = states
            .cached_state
            .get::<SurfaceAttributes>()
            .current()
            .input_region
            .as_ref()
            .is_some_and(|region| region.rects.len() > 1);
        let placed = states
            .data_map
            .get::<XdgPopupSurfaceData>()
            .map(|data| {
                let current = data.lock().unwrap().current;
                (current.geometry, current.positioner.anchor_rect)
            });
        (shaped, placed)
    });
    if let (true, Some((geometry, anchor))) = (shaped, placed) {
        if geometry.loc.y >= anchor.loc.y + anchor.size.h {
            area.loc.y += GTK_TAIL_HEIGHT;
            area.size.h -= GTK_TAIL_HEIGHT;
        } else if geometry.loc.y + geometry.size.h <= anchor.loc.y {
            area.size.h -= GTK_TAIL_HEIGHT;
        } else if geometry.loc.x >= anchor.loc.x + anchor.size.w {
            area.loc.x += GTK_TAIL_HEIGHT;
            area.size.w -= GTK_TAIL_HEIGHT;
        } else if geometry.loc.x + geometry.size.w <= anchor.loc.x {
            area.size.w -= GTK_TAIL_HEIGHT;
        }
    }
    (!area.is_empty()).then_some(area)
}

/// The corners GTK's windows have in Nekoland's look, as its `--window-radius` says.
const WINDOW_RADIUS: i32 = 14;

/// `windows`, front to back, over the layers under them, each on glass of what is under it.
fn windows_on_glass(
    state: &Nekoland,
    renderer: &mut GlesRenderer,
    output: &Output,
    mut glass: Option<&mut Glass>,
    windows: &[Window],
) -> Vec<SceneElement> {
    let output_loc = state
        .space
        .output_geometry(output)
        .map(|geometry| geometry.loc)
        .unwrap_or_default();
    let scale = output.current_scale().fractional_scale();
    let output_size = output.current_mode().map(|mode| mode.size);
    let mut scene: Vec<SceneElement> = state
        .desktop_elements(renderer, output_loc, scale)
        .into_iter()
        .collect();
    scene.extend(layer_elements(renderer, output, BELOW_WINDOWS, false));
    let mut kept = Vec::new();
    // From the bottom up, so the glass of each one shows those under it, theirs included.
    for window in windows.iter().rev() {
        let mut elements = window_body_elements(state, renderer, window, output_loc, scale);
        let area = window_area(state, window, output_loc, scale);
        // Its window is mostly transparent shadow margin, which glass would blur.
        let area = area.filter(|_| window_names(window).1 != CONFIRM_APP_ID);
        if let (Some(glass), Some(area), Some(size)) = (glass.as_deref_mut(), area, output_size) {
            let id = window_glass_id(window);
            if let Err(err) = glass.update_window(renderer, id.clone(), (size, scale), area, &scene) {
                tracing::warn!("glass: {err}");
            }
            let radius = if has_square_corners(window) {
                0
            } else {
                (f64::from(WINDOW_RADIUS) * scale).round() as i32
            };
            elements.extend(glass.window_pane(id.clone(), area, radius).map(SceneElement::Glass));
            kept.push(id);
        }
        elements.append(&mut scene);
        scene = elements;
    }
    if let Some(glass) = glass {
        glass.keep_windows(&kept);
    }
    scene
}

/// The part of the output a window covers, without its shadow.
fn window_area(
    state: &Nekoland,
    window: &Window,
    output_loc: Point<i32, Logical>,
    scale: f64,
) -> Option<Rectangle<i32, Physical>> {
    let location = state.space.element_location(window)?;
    Some(Rectangle::new(location - output_loc, window.geometry().size).to_physical_precise_round(scale))
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
    let scale = output.current_scale().fractional_scale();
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
                location.to_physical_precise_round(scale),
                Scale::from(scale),
                1.0,
            )
        })
        .map(SceneElement::Surface)
        .collect()
}
