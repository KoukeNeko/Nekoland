//! What an output shows, as render elements. Both backends draw the same scene.

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            damage::OutputDamageTracker,
            element::{
                memory::MemoryRenderBufferRenderElement,
                render_elements,
                solid::SolidColorRenderElement,
                surface::{render_elements_from_surface_tree, WaylandSurfaceRenderElement},
                texture::{TextureBuffer, TextureRenderElement},
                utils::{CropRenderElement, Relocate, RelocateRenderElement, RescaleRenderElement},
                AsRenderElements, Element, Id, Kind,
            },
            gles::{element::PixelShaderElement, GlesRenderer, GlesTexture},
            utils::{CommitCounter, RendererSurfaceStateUserData},
            Bind, Color32F, Offscreen,
        },
    },
    desktop::{layer_map_for_output, LayerSurface, PopupKind, PopupManager, Window},
    output::Output,
    utils::{IsAlive, Logical, Physical, Point, Rectangle, Scale, Size, Transform},
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    wayland::{
        alpha_modifier::AlphaModifierSurfaceCachedState,
        compositor::{with_states, SurfaceAttributes},
        shell::wlr_layer::Layer,
    },
};

use crate::{
    config::{DockPosition, MinimizeEffect},
    dock::Frame,
    glass::{Glass, Pane, Tier},
    appgrid::{Backdrop, Layer as AppsLayer},
    menubar::{Overlay, MENU_RADIUS},
    sprite::Placed,
    motion::{self, GhostElement, Pose},
    overview::ICON_SIZE,
    state::{ABOVE_WINDOWS, BELOW_WINDOWS},
    text::Picture,
    windows::{
        app_id, is_managed, is_x11_drawing_own_frame, ALERT_APP_IDS, CONFIRM_APP_ID,
        LAUNCHER_APP_ID,
    },
    Nekoland,
};

pub const CLEAR_COLOR: [f32; 4] = [0.1, 0.1, 0.1, 1.0];
/// The outline around the window under the pointer in the overview.
const OUTLINE: i32 = 4;
pub(crate) const OUTLINE_COLOR: [f32; 4] = [0.04, 0.52, 1.0, 1.0];
/// A layer surface at the top edge no taller than this is a panel.
const TOP_PANEL_MAX_HEIGHT: i32 = 96;

pub(crate) type SurfaceElement = WaylandSurfaceRenderElement<GlesRenderer>;
type Snapshot = TextureRenderElement<GlesTexture>;

render_elements! {
    pub SceneElement<=GlesRenderer>;
    Surface=SurfaceElement,
    Cursor=RescaleRenderElement<SurfaceElement>,
    Rounded=crate::rounded::RoundedSurface,
    Shadow=PixelShaderElement,
    Glass=Pane,
    Thumbnail=RelocateRenderElement<RescaleRenderElement<SurfaceElement>>,
    RoundedThumbnail=RelocateRenderElement<RescaleRenderElement<crate::rounded::RoundedSurface>>,
    Desktop=CropRenderElement<RelocateRenderElement<RescaleRenderElement<SurfaceElement>>>,
    BentGlass=CropRenderElement<RelocateRenderElement<RescaleRenderElement<Pane>>>,
    BentPicture=CropRenderElement<RelocateRenderElement<RescaleRenderElement<Snapshot>>>,
    Solid=SolidColorRenderElement,
    Ghost=GhostElement,
    ZoomedPicture=RescaleRenderElement<MemoryRenderBufferRenderElement<GlesRenderer>>,
    Picture=MemoryRenderBufferRenderElement<GlesRenderer>,
    Texture=Snapshot,
    Sprite=crate::sprite::SpriteElement,
}

render_elements! {
    pub OutputElement<=GlesRenderer>;
    Scene=SceneElement,
    Zoomed=RescaleRenderElement<SceneElement>,
}

/// The most the screen is magnified, and the step its shortcuts change it by.
pub const ZOOM_MOST: f64 = 8.0;
pub const ZOOM_STEP: f64 = 0.5;

/// The screen as the Mac's zoom shows it: magnified around the pointer, which so stays over
/// what it was over, and is where clicks land without being moved.
pub fn zoomed(elements: Vec<SceneElement>, around: Point<i32, Physical>, zoom: f64) -> Vec<OutputElement> {
    if zoom <= 1.0 {
        return elements.into_iter().map(OutputElement::Scene).collect();
    }
    elements
        .into_iter()
        .map(|element| OutputElement::Zoomed(RescaleRenderElement::from_element(element, around, zoom)))
        .collect()
}

/// What a client drags, under the pointer and over everything else.
pub fn dnd_icon_elements(
    state: &Nekoland,
    renderer: &mut GlesRenderer,
    output: &Output,
) -> Vec<SceneElement> {
    let Some((icon, offset)) = state.dnd_icon.as_ref().filter(|(icon, _)| icon.alive()) else {
        return Vec::new();
    };
    let (Some(pointer), Some(geometry)) = (state.seat.get_pointer(), state.space.output_geometry(output))
    else {
        return Vec::new();
    };
    let scale = output.current_scale().fractional_scale();
    let location = (pointer.current_location() - geometry.loc.to_f64() + offset.to_f64())
        .to_physical_precise_round(scale);
    render_elements_from_surface_tree(renderer, icon, location, scale, 1.0, Kind::Unspecified)
}

/// Everything on an output but the cursor, front to back.
pub fn scene_elements(
    state: &Nekoland,
    renderer: &mut GlesRenderer,
    output: &Output,
    mut glass: Option<&mut Glass>,
) -> Result<Vec<SceneElement>, Box<dyn std::error::Error>> {
    let scale = output.current_scale().fractional_scale();
    // Locked, the locker's surface is all there is, under a shortcut opened over it; until
    // it has one, the clear colour.
    if state.lock.is_some() {
        let output_loc = state.space.output_geometry(output).ok_or("output is not mapped")?.loc;
        let mut elements: Vec<SceneElement> = state
            .lock_guests()
            .iter()
            .rev()
            .flat_map(|window| window_elements(state, renderer, window, output_loc, scale))
            .collect();
        elements.extend(state
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
                .collect::<Vec<_>>()
            })
            .unwrap_or_default());
        return Ok(elements);
    }
    let Some(overview) = &state.overview else {
        // A fullscreen window is all there is to see, under the overlay layer, once it has
        // grown to fill the output: on its way there the others still show around it.
        if let Some(window) = state.fullscreen_window().filter(|window| !state.motion_moves(window)) {
            let output_loc = state
                .space
                .output_geometry(output)
                .ok_or("output is not mapped")?
                .loc;
            let mut elements = layer_elements(renderer, output, &[Layer::Overlay]);
            // The system's alerts ask over a fullscreen window too, on its glass.
            let alerts: Vec<(Window, Pose)> = state
                .space
                .elements()
                .rev()
                .filter(|other| ALERT_APP_IDS.contains(&app_id(other).as_str()))
                .filter_map(|alert| Some((alert.clone(), state.window_pose(alert)?)))
                .collect();
            let fullscreen = window_elements(state, renderer, &window, output_loc, scale);
            elements.extend(windows_on_glass(state, renderer, output, glass.as_deref_mut(), &alerts, fullscreen));
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
            .filter(|window| state.config.glass.contains_key(&app_id(window)))
            .cloned()
            .collect();
        let windows: Vec<Window> = state
            .space
            .elements()
            .rev()
            .filter(|window| !floating.contains(window) && !state.dock_draws(window))
            .cloned()
            .collect();
        // The desktop just left slides out under the windows of the one gone to.
        let posed: Vec<(Window, Pose)> = windows
            .iter()
            .filter_map(|window| Some((window.clone(), state.window_pose(window)?)))
            .chain(state.leaving_windows().into_iter().rev())
            .collect();
        // What a closed window leaves fades over the windows it was among.
        let mut under: Vec<SceneElement> = state
            .ghost_elements(output_geometry.loc, scale)
            .into_iter()
            .map(SceneElement::Ghost)
            .collect();
        let mut backdrop: Vec<SceneElement> = state
            .desktop_elements(renderer, output_geometry.loc, scale)
            .into_iter()
            .collect();
        backdrop.extend(layer_elements(renderer, output, &BELOW_WINDOWS));
        under.extend(windows_on_glass(state, renderer, output, glass.as_deref_mut(), &posed, backdrop));
        let mut overlays = state.fps_overlays();
        overlays.extend(state.identify_overlays());
        overlays.extend(state.hover_overlays());
        overlays.extend(state.menu_bar_overlays());
        overlays.extend(state.notice_overlays());
        overlays.extend(state.dock_overlays());
        let on_top = OnTop {
            overlays: &overlays,
            sprites: &[],
            backdrop: None,
            windows: &floating,
            popups_of: &windows,
            without_top_panel: false,
        };
        let mut elements = switcher_front(state, renderer, output_geometry.loc, scale);
        elements.extend(state.desktop_front_elements(renderer, output_geometry.loc, scale));
        elements.extend(dock_front(state, renderer, glass.as_deref(), output_geometry.loc, scale));
        elements.extend(menu_bar_sliding(state, renderer, output_geometry, scale));
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
        for (id, side) in overview.outline.iter().zip(outline_sides(outer, OUTLINE)) {
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

    // Under the applications too, whose glass shows them blurred where they are. Each
    // window keeps its own glass, as on the desktop, so a see-through sidebar stays blurred
    // all the way in and out.
    let mut backdrop = vec![SceneElement::Solid(SolidColorRenderElement::from_buffer(
        &overview.dim,
        (0, 0),
        scale,
        1.0,
        Kind::Unspecified,
    ))];
    backdrop.extend(layer_elements(renderer, output, &BELOW_WINDOWS));
    let posed: Vec<(Window, Pose)> = resting()
        .map(|slot| {
            let pose = Pose {
                actual: slot.from,
                shown: slot.now,
                alpha: 1.0,
            };
            (slot.window.clone(), pose)
        })
        .collect();
    elements.extend(windows_on_glass(state, renderer, output, glass.as_deref_mut(), &posed, backdrop));
    // The applications in front of the desktops' bar, on glass behind everything else.
    let apps = state.apps_scene();
    // A menu a right click opened here, over it all.
    let mut overlays = state.menu_bar_overlays();
    // Each group of sprites goes in front of the overlay that follows it.
    let mut sprites = Vec::new();
    for layer in apps.front {
        match layer {
            AppsLayer::Overlay(overlay) => overlays.push(overlay),
            AppsLayer::Sprites(group) => sprites.push((overlays.len(), group)),
        }
    }
    overlays.extend(state.overview_overlays());
    overlays.extend(state.dock_overlays());
    let on_top = OnTop {
        overlays: &overlays,
        sprites: &sprites,
        backdrop: apps.backdrop.as_ref(),
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
    scene.extend(
        state
            .overview_add_button()
            .filter(|_| state.overview_expansion() > 0.0)
            .and_then(|(rect, plus)| {
                let within = state.overview_panel()?.intersection(rect)?;
                let cut = Rectangle::from_size(within.size).to_f64();
                MemoryRenderBufferRenderElement::from_buffer(
                    renderer,
                    to_output(rect.loc),
                    &plus.buffer,
                    Some(state.overview_expansion() as f32),
                    Some(cut),
                    Some(within.size),
                    Kind::Unspecified,
                )
                .ok()
                .map(SceneElement::Picture)
            }),
    );
    scene.extend(desktop_thumbnails(state, renderer, output, output_geometry.loc));
    // The menu bar's content over its tint, sliding up off the screen as the overview opens,
    // and back.
    let away = state.overview_menu_away();
    let bar = [state.menu_bar_picture(), state.menu_bar_tint()];
    scene.extend(bar.into_iter().flatten().filter(|_| away < 1.0).filter_map(|bar| {
        let gone = (f64::from(bar.size.h) * away).round() as i32;
        let shown = Size::from((bar.size.w, bar.size.h - gone));
        MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            to_output(output_geometry.loc),
            &bar.buffer,
            None,
            Some(Rectangle::new(Point::from((0, gone)), shown).to_f64()),
            Some(shown),
            Kind::Unspecified,
        )
        .ok()
        .map(SceneElement::Picture)
    }));
    scene.extend(dock_front(state, renderer, None, output_geometry.loc, scale));
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
    // The add button is a desktop with no windows yet: the wallpaper alone.
    let add = state.overview_add_button().map(|(rect, _)| (None, rect));
    let desktops = state.overview_thumbnails().into_iter().map(|(index, rect)| (Some(index), rect));
    let row_end = state.overview_row_end().unwrap_or(i32::MAX);
    for (index, rect) in desktops.chain(add) {
        let factor = f64::from(rect.size.w) / f64::from(output_width);
        let offset = (rect.loc - output_loc).to_physical_precise_round(scale);
        // Each is cut to its own picture as well as to the bar.
        // Desktops scrolled along the row stop short of the add button.
        let mut shown = Rectangle::new(rect.loc - output_loc, rect.size);
        if index.is_some() {
            shown.size.w = shown.size.w.min(row_end - rect.loc.x);
            // Wholly past the row's end, it would make a rectangle of negative width.
            if shown.size.w <= 0 {
                continue;
            }
        }
        let Some(crop) = crop.intersection(shown.to_physical_precise_round(scale))
        else {
            continue;
        };
        let mut surfaces: Vec<SurfaceElement> = Vec::new();
        // Front to back: the windows, top of the stack first, then the wallpaper.
        let windows = index.map(|index| state.desktop_windows(index)).unwrap_or_default();
        for (window, location) in windows.into_iter().rev() {
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
        // A desktop with a wallpaper of its own has it on its frame instead.
        let own_wallpaper = index.is_some_and(|index| state.overview_own_wallpaper(index).is_some());
        for surface in BELOW_WINDOWS
            .into_iter()
            .filter(|_| !own_wallpaper)
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

/// The switcher's windows, small in their cells, each with its icon in front of it.
fn switcher_front(
    state: &Nekoland,
    renderer: &mut GlesRenderer,
    output_loc: Point<i32, Logical>,
    scale: f64,
) -> Vec<SceneElement> {
    let thumbnails: Vec<_> = state
        .switcher_thumbnails()
        .iter()
        .chain(state.snap_assist_thumbnails())
        .collect();
    let mut elements: Vec<SceneElement> = thumbnails
        .iter()
        .filter_map(|(_, _, icon, at)| {
            MemoryRenderBufferRenderElement::from_buffer(
                renderer,
                (*at - output_loc).to_f64().to_physical(scale),
                &icon.as_ref()?.buffer,
                None,
                None,
                None,
                Kind::Unspecified,
            )
            .ok()
            .map(SceneElement::Picture)
        })
        .collect();
    for (window, rect, _, _) in thumbnails {
        elements.extend(window_fitted(renderer, window, *rect, output_loc, scale));
    }
    elements
}

/// The menu bar's content of the desktop gone to and of the one left, sliding past each other
/// with their windows, each cut to the bar's display.
fn menu_bar_sliding(
    state: &Nekoland,
    renderer: &mut GlesRenderer,
    output_geometry: Rectangle<i32, Logical>,
    scale: f64,
) -> Vec<SceneElement> {
    if state.bar_area() != Some(output_geometry) || !state.menu_bar_shown() {
        return Vec::new();
    }
    let width = output_geometry.size.w;
    let Some((coming, others)) = state.menu_bar_slide(width) else {
        return Vec::new();
    };
    state
        .menu_bar_picture()
        .map(|bar| (bar, coming))
        .into_iter()
        .chain(others)
        .filter_map(|(bar, offset)| {
            // A desktop further than a screen away, as the view hurries past, shows none.
            let shown_width = (bar.size.w - offset.abs()).min(width - offset.max(0));
            if shown_width <= 0 {
                return None;
            }
            let shown = Size::from((shown_width, bar.size.h));
            MemoryRenderBufferRenderElement::from_buffer(
                renderer,
                Point::<i32, Logical>::from((offset.max(0), 0)).to_f64().to_physical(scale),
                &bar.buffer,
                None,
                Some(Rectangle::new(Point::from(((-offset).max(0), 0)), shown).to_f64()),
                Some(shown),
                Kind::Unspecified,
            )
            .ok()
            .map(SceneElement::Picture)
        })
        .collect()
}

/// What the Dock shows live over its glass, front first: the icons at the corners of the
/// minimized windows' tiles, and the windows themselves, each fitted to its place.
fn dock_front(
    state: &Nekoland,
    renderer: &mut GlesRenderer,
    glass: Option<&Glass>,
    output_loc: Point<i32, Logical>,
    scale: f64,
) -> Vec<SceneElement> {
    let scaled = state.config.dock_minimize_effect == MinimizeEffect::Scale;
    let mut elements: Vec<SceneElement> = state
        .dock_genies()
        .into_iter()
        .flat_map(|genie| {
            let shown = genie_shown(state, &genie.window, genie.from);
            let Some(picture) = window_picture(state, renderer, &genie.window, shown, scale) else {
                return Vec::new();
            };
            // The glass its see-through parts show, of what is where it was, goes with it.
            let pane = glass.zip(genie_glass(state, &genie.window, shown, output_loc, scale)).and_then(
                |(glass, (area, radius))| glass.window_pane(window_glass_id(&genie.window), area, radius),
            )
            .map(Pane::circular);
            if scaled {
                // Straight to its place in the Dock, shrinking as it goes.
                let t = 1.0 - (1.0 - genie.progress.clamp(0.0, 1.0)).powi(3);
                let mix = |from: i32, to: i32| (f64::from(from) + f64::from(to - from) * t).round() as i32;
                let (from, into) = (shown, genie.into);
                let rect = Rectangle::new(
                    (mix(from.loc.x, into.loc.x), mix(from.loc.y, into.loc.y)).into(),
                    (mix(from.size.w, into.size.w), mix(from.size.h, into.size.h)).into(),
                );
                let Some((factor, fitted)) = fit(shown.size, rect) else {
                    return Vec::new();
                };
                let crop =
                    Rectangle::new(fitted.loc - output_loc, fitted.size).to_physical_precise_round(scale);
                return bent_picture(&picture, shown.size, factor, crop.loc, scale, crop)
                    .into_iter()
                    .chain(pane.and_then(|pane| bent_glass(&pane, factor, crop.loc, scale, crop)))
                    .collect();
            }
            let (into, progress) = (genie.into, genie.progress);
            draw_genie(genie.frame, &picture, pane.as_ref(), shown, into, progress, output_loc, scale)
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
    frame: Frame,
    picture: &TextureBuffer<GlesTexture>,
    pane: Option<&Pane>,
    from: Rectangle<i32, Logical>,
    into: Rectangle<i32, Logical>,
    progress: f64,
    output_loc: Point<i32, Logical>,
    output_scale: f64,
) -> Vec<SceneElement> {
    let size = from.size;
    if size.w <= 0 || size.h <= 0 {
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
        DockPosition::Bottom => f64::from(size.h),
        _ => f64::from(size.w),
    };
    let lerp = |a: f64, b: f64, t: f64| a + (b - a) * t;
    // How far a height on screen is down the funnel, from the window's top to its picture's.
    let depth = (into.loc.y - from.loc.y).max(1.0);
    let funnel = |y: f64| smooth(((y - from.loc.y) / depth).clamp(0.0, 1.0)) * bend;
    let row_y = |v: f64| lerp(from.loc.y + v * from.size.h, into.loc.y + v * into.size.h, slide);
    let bands = (from.size.h / GENIE_BAND).ceil().clamp(16.0, 240.0) as usize;
    let mut elements = Vec::new();
    // Behind all the bands, which its edge past each one would otherwise cross.
    let mut glass = Vec::new();
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
                let scale = Scale::from((band.size.w / f64::from(size.w), band.size.h / part));
                let offset = Point::from((band.loc.x, band.loc.y - start * scale.y));
                (scale, offset, Size::from((0.0, 1.0)))
            }
            _ => {
                let scale = Scale::from((band.size.w / part, band.size.h / f64::from(size.h)));
                let offset = Point::from((band.loc.x - start * scale.x, band.loc.y));
                (scale, offset, Size::from((1.0, 0.0)))
            }
        };
        let offset: Point<f64, Logical> = offset - output_loc.to_f64();
        // One pixel longer, so that the bands meet without a seam.
        let crop = Rectangle::<f64, Logical>::new(band.loc - output_loc.to_f64(), band.size + grown)
            .to_physical(output_scale)
            .to_i32_round();
        let offset = offset.to_physical(output_scale).to_i32_round();
        elements.extend(bent_picture(picture, size, scale, offset, output_scale, crop));
        glass.extend(pane.and_then(|pane| bent_glass(pane, scale, offset, output_scale, crop)));
    }
    elements.extend(glass);
    elements
}

/// `picture` of a window `size` large, stretched by `scale` and put at `at`, as much of it as
/// is within `crop`.
fn bent_picture(
    picture: &TextureBuffer<GlesTexture>,
    size: Size<i32, Logical>,
    scale: impl Into<Scale<f64>>,
    at: Point<i32, Physical>,
    output_scale: f64,
    crop: Rectangle<i32, Physical>,
) -> Option<SceneElement> {
    // The picture is as many pixels as the window is at `output_scale`; without its own
    // source the element would read only `size` of them.
    let pixels: Size<i32, Physical> = size.to_physical_precise_round(output_scale);
    let src = Rectangle::from_size(Size::from((f64::from(pixels.w), f64::from(pixels.h))));
    let element =
        Snapshot::from_texture_buffer((0.0, 0.0), picture, None, Some(src), Some(size), Kind::Unspecified);
    let scaled = RescaleRenderElement::from_element(element, (0, 0).into(), scale);
    let placed = RelocateRenderElement::from_element(scaled, at, Relocate::Relative);
    CropRenderElement::from_element(placed, output_scale, crop).map(SceneElement::BentPicture)
}

/// Where a window on its way into the Dock or out is shown when it is not minimized, at
/// `from`: without what is trimmed off it, with the compositor's bar over it.
fn genie_shown(state: &Nekoland, window: &Window, from: Rectangle<i32, Logical>) -> Rectangle<i32, Logical> {
    state.with_title(window, Rectangle::new(from.loc, from.size - crate::frame::trim(window)))
}

/// A window as it is drawn where it is `shown`: under the compositor's bar and with the
/// corners it cuts, in a texture of its own for its way into the Dock to bend.
fn window_picture(
    state: &Nekoland,
    renderer: &mut GlesRenderer,
    window: &Window,
    shown: Rectangle<i32, Logical>,
    scale: f64,
) -> Option<TextureBuffer<GlesTexture>> {
    let size: Size<i32, Physical> = shown.size.to_physical_precise_round(scale);
    if size.w <= 0 || size.h <= 0 {
        return None;
    }
    // Where the window's own top left is, under the bar.
    let own = state.without_title(window, shown).loc;
    let location: Point<i32, Physical> =
        (own - shown.loc - window.geometry().loc).to_physical_precise_round(scale);
    let corner = (f64::from(WINDOW_RADIUS) * crate::windows::roundness(window).0).round() as i32;
    let rounding = (corner > 0 && state.cuts_corners(window))
        .then(|| {
            let radius = (f64::from(corner) * scale).round() as i32;
            crate::rounded::Rounding::new(renderer, radius, Rectangle::from_size(size), scale, false)
        })
        .flatten();
    let mut elements: Vec<SceneElement> =
        crate::frame::title_element_at(state, renderer, window, own, shown.loc, scale).into_iter().collect();
    let surfaces = AsRenderElements::<GlesRenderer>::render_elements::<SurfaceElement>(
        window,
        renderer,
        location,
        Scale::from(scale),
        1.0,
    );
    elements.extend(surfaces.into_iter().map(|surface| match &rounding {
        Some(rounding) => SceneElement::Rounded(rounding.apply(surface)),
        None => SceneElement::Surface(surface),
    }));
    let drawn = (|| -> Result<GlesTexture, Box<dyn std::error::Error>> {
        let mut texture: GlesTexture =
            Offscreen::<GlesTexture>::create_buffer(renderer, Fourcc::Abgr8888, (size.w, size.h).into())?;
        let mut tracker = OutputDamageTracker::new(size, scale, Transform::Normal);
        let mut target = renderer.bind(&mut texture)?;
        tracker
            .render_output(renderer, &mut target, 0, &elements, Color32F::TRANSPARENT)
            .map_err(|err| format!("{err:?}"))?;
        drop(target);
        Ok(texture)
    })();
    let texture = drawn.inspect_err(|err| tracing::warn!("window picture: {err}")).ok()?;
    Some(TextureBuffer::from_texture(renderer, texture, 1, Transform::Normal, None))
}

/// How much smaller something `size` large is drawn to fit `rect`, and where it then is, in
/// the middle of it.
fn fit(size: Size<i32, Logical>, rect: Rectangle<i32, Logical>) -> Option<(f64, Rectangle<i32, Logical>)> {
    if size.w <= 0 || size.h <= 0 {
        return None;
    }
    let factor = (f64::from(rect.size.w) / f64::from(size.w)).min(f64::from(rect.size.h) / f64::from(size.h));
    let fitted = Size::from((
        (f64::from(size.w) * factor).round() as i32,
        (f64::from(size.h) * factor).round() as i32,
    ));
    let at = rect.loc + Point::from(((rect.size.w - fitted.w) / 2, (rect.size.h - fitted.h) / 2));
    Some((factor, Rectangle::new(at, fitted)))
}

/// `pane`, the glass behind a window and its edge, stretched by `scale` as the window is and
/// put where the window's top left then is, `at`, as much of it as is within `crop` of the
/// window and the edge around that.
fn bent_glass(
    pane: &Pane,
    scale: impl Into<Scale<f64>>,
    at: Point<i32, Physical>,
    output_scale: f64,
    crop: Rectangle<i32, Physical>,
) -> Option<SceneElement> {
    let origin = pane.geometry(Scale::from(output_scale)).loc + Point::from((RIM, RIM));
    let scaled = RescaleRenderElement::from_element(pane.clone(), origin, scale);
    let placed = RelocateRenderElement::from_element(scaled, at - origin, Relocate::Relative);
    let crop = Rectangle::new(
        crop.loc - Point::from((RIM, RIM)),
        crop.size + Size::from((2 * RIM, 2 * RIM)),
    );
    CropRenderElement::from_element(placed, output_scale, crop).map(SceneElement::BentGlass)
}

/// Where the glass behind a window on its way into the Dock or out is, and how round, as
/// where it is when it is not minimized, `from`: behind all of it, with the edge that shows
/// past it, for one that has glass or an edge of it.
fn genie_glass(
    state: &Nekoland,
    window: &Window,
    from: Rectangle<i32, Logical>,
    output_loc: Point<i32, Logical>,
    scale: f64,
) -> Option<(Rectangle<i32, Physical>, i32)> {
    let corner = (f64::from(WINDOW_RADIUS) * crate::windows::roundness(window).0).round() as i32;
    let radius = match state.shadow_radius(window, corner) {
        Some(radius) => radius,
        None if is_managed(window) && !is_x11_drawing_own_frame(window) => corner,
        None => return None,
    };
    let body = Rectangle::new(from.loc - output_loc, from.size).to_physical_precise_round(scale);
    let area = Rectangle::new(
        body.loc - Point::from((RIM, RIM)),
        body.size + Size::from((2 * RIM, 2 * RIM)),
    );
    Some((area, (f64::from(radius) * scale).round() as i32 + RIM))
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
    let Some((factor, fitted)) = fit(geometry.size, rect) else {
        return Vec::new();
    };
    let at = fitted.loc - output_loc;
    let crop = Rectangle::new(at, fitted.size).to_physical_precise_round(scale);
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
    /// Sprites, each group in front of the overlay at the place it names.
    sprites: &'a [(usize, Vec<Placed<'a>>)],
    /// The applications' glass, behind every overlay.
    backdrop: Option<&'a Backdrop<'a>>,
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
        sprites,
        backdrop,
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
        .chain(backdrop.map(|backdrop| {
            let rect = Rectangle::new(backdrop.rect.loc - output_loc, backdrop.rect.size);
            (Tier::Menu, rect.to_physical_precise_round(scale))
        }))
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
                .map(|(area, _)| (Tier::Menu, area.to_physical_precise_round(scale))),
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
    for (index, overlay) in overlays.iter().enumerate() {
        for (_, group) in sprites.iter().filter(|(before, _)| *before == index) {
            elements.extend(
                group
                    .iter()
                    .filter_map(|sprite| sprite.element(renderer, output_loc, scale))
                    .map(SceneElement::Sprite),
            );
        }
        let rect =
            Rectangle::new(overlay.rect.loc - output_loc, overlay.rect.size).to_physical_precise_round(scale);
        // An overlay smaller than its picture shows as much of it as fits, from the top left.
        let fits = Size::from((
            overlay.rect.size.w.min(overlay.picture.size.w),
            overlay.rect.size.h.min(overlay.picture.size.h),
        ));
        let crop = (fits != overlay.picture.size).then_some(fits);
        let picture = MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            rect.loc.to_f64(),
            &overlay.picture.buffer,
            Some(overlay.alpha * overlay.fade),
            crop.map(|size| Rectangle::from_size(size).to_f64()),
            crop,
            Kind::Unspecified,
        )
        .ok();
        // The applications come in and go out a little larger, around their middle; the
        // glass behind goes with them.
        let zoom = state
            .apps_zoom_of(overlay.id)
            .or_else(|| state.notice_zoom_of(overlay.id))
            .map(|zoom| (rect.loc + Point::from((rect.size.w / 2, rect.size.h / 2)), zoom));
        elements.extend(picture.map(|picture| match zoom {
            Some((pivot, zoom)) => {
                SceneElement::ZoomedPicture(RescaleRenderElement::from_element(picture, pivot, zoom))
            }
            None => SceneElement::Picture(picture),
        }));
        let (rect, grown) = match zoom {
            Some((pivot, zoom)) => (
                Rectangle::new(
                    pivot + (rect.loc - pivot).to_f64().upscale(zoom).to_i32_round(),
                    rect.size.to_f64().upscale(zoom).to_i32_round(),
                ),
                zoom,
            ),
            None => (rect, 1.0),
        };
        elements.extend(
            glass
                .as_deref_mut()
                .zip(overlay.glass)
                .and_then(|(glass, (tier, radius))| {
                    glass.pane_with(overlay.id.clone(), tier, rect, (f64::from(pixels(radius)) * grown).round() as i32)
                })
                .map(|pane| SceneElement::Glass(pane.faded(overlay.fade))),
        );
    }
    if let Some(backdrop) = backdrop {
        let rect = Rectangle::new(backdrop.rect.loc - output_loc, backdrop.rect.size);
        elements.push(SceneElement::Solid(SolidColorRenderElement::from_buffer(
            backdrop.tint,
            rect.loc.to_physical_precise_round(scale),
            scale,
            backdrop.fade,
            Kind::Unspecified,
        )));
        elements.extend(
            glass
                .as_deref_mut()
                .and_then(|glass| {
                    glass.pane_with(backdrop.id.clone(), Tier::Menu, rect.to_physical_precise_round(scale), 0)
                })
                .map(|pane| SceneElement::Glass(pane.faded(backdrop.fade))),
        );
    }
    for window in windows {
        // Before the window, which a bar over a title bar of the program's own covers.
        if let Some(pose) = state.window_pose(window).filter(|pose| pose.is_still()) {
            elements.extend(crate::frame::title_element(state, renderer, window, pose, output_loc, scale));
        }
        elements.extend(window_elements(state, renderer, window, output_loc, scale));
        let radius = state
            .config
            .glass
            .get(&app_id(window))
            .copied()
            .unwrap_or_default();
        if let Some(pose) = state.window_pose(window) {
            let area = pose_area(window, pose, output_loc, scale);
            elements.extend(
                glass
                    .as_deref_mut()
                    .and_then(|glass| glass.pane_with(window_glass_id(window), Tier::Menu, area, pixels(radius)))
                    .map(|pane| SceneElement::Glass(pane.faded(pose.alpha))),
            );
        }
    }
    // An input method's candidates follow the caret anywhere, so they go over everything here,
    // the Dock and the menu bar included; other popups stay among the windows.
    let mut in_front = Vec::new();
    for (popup, origin) in &popups {
        let elements = if matches!(popup, PopupKind::InputMethod(_)) {
            &mut in_front
        } else {
            &mut elements
        };
        // A menu grows out of what it was opened from, its glass with it.
        let pop = state
            .config
            .animations
            .then(|| motion::popup_pop(popup.wl_surface()))
            .flatten();
        let alpha = pop.map_or(1.0, |(_, alpha)| alpha);
        let pivot = popup_pivot(popup, Rectangle::new(*origin, popup.geometry().size));
        let surfaces: Vec<SurfaceElement> = render_elements_from_surface_tree(
            renderer,
            popup.wl_surface(),
            (*origin - popup.geometry().loc).to_physical_precise_round(scale),
            scale,
            alpha,
            Kind::Unspecified,
        );
        match pop {
            Some((factor, _)) => {
                let about: Point<i32, Physical> = pivot.to_physical_precise_round(scale);
                elements.extend(surfaces.into_iter().map(|surface| {
                    let scaled = RescaleRenderElement::from_element(surface, about, factor);
                    SceneElement::Thumbnail(RelocateRenderElement::from_element(
                        scaled,
                        Point::default(),
                        Relocate::Relative,
                    ))
                }));
            }
            None => elements.extend(surfaces.into_iter().map(SceneElement::Surface)),
        }
        if let Some((area, radius)) = popup_glass_area(popup, *origin) {
            let area = pop.map_or(area, |(factor, _)| motion::scaled_about(area, pivot, factor));
            elements.extend(
                glass
                    .as_deref_mut()
                    .and_then(|glass| {
                        glass.pane(
                            popup.wl_surface(),
                            Tier::Menu,
                            area.to_physical_precise_round(scale),
                            pixels(radius),
                        )
                    })
                    .map(|pane| SceneElement::Glass(pane.faded(alpha * surface_alpha(popup.wl_surface())))),
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
    in_front.extend(elements);
    in_front
}

/// A window's surfaces, with its popups, where it is drawn on the output.
fn window_elements(
    state: &Nekoland,
    renderer: &mut GlesRenderer,
    window: &Window,
    output_loc: Point<i32, Logical>,
    scale: f64,
) -> Vec<SceneElement> {
    state
        .window_pose(window)
        .map(|pose| posed_elements(renderer, window, pose, output_loc, scale, true, None))
        .unwrap_or_default()
}

/// A window's surfaces where `pose` draws it: stretched from where the window is to there,
/// and as much there to see. Without its popups, they are left to go over the windows on
/// glass.
fn posed_elements(
    renderer: &mut GlesRenderer,
    window: &Window,
    pose: Pose,
    output_loc: Point<i32, Logical>,
    scale: f64,
    with_popups: bool,
    rounding: Option<&crate::rounded::Rounding>,
) -> Vec<SceneElement> {
    let natural: Point<i32, Physical> =
        (pose.actual.loc - window.geometry().loc - output_loc).to_physical_precise_round(scale);
    let surfaces: Vec<SurfaceElement> = match window.toplevel().filter(|_| !with_popups) {
        Some(toplevel) => render_elements_from_surface_tree(
            renderer,
            toplevel.wl_surface(),
            natural,
            scale,
            pose.alpha,
            Kind::Unspecified,
        ),
        None => AsRenderElements::<GlesRenderer>::render_elements::<SurfaceElement>(
            window,
            renderer,
            natural,
            Scale::from(scale),
            pose.alpha,
        ),
    };
    let (actual, shown) = (pose.actual, pose.shown);
    if shown == actual || actual.size.w <= 0 || actual.size.h <= 0 {
        return surfaces
            .into_iter()
            .map(|surface| match rounding {
                Some(rounding) => SceneElement::Rounded(rounding.apply(surface)),
                None => SceneElement::Surface(surface),
            })
            .collect();
    }
    let origin: Point<i32, Physical> = (actual.loc - output_loc).to_physical_precise_round(scale);
    let offset: Point<i32, Physical> = (shown.loc - actual.loc).to_physical_precise_round(scale);
    let factor = Scale::from((
        f64::from(shown.size.w) / f64::from(actual.size.w),
        f64::from(shown.size.h) / f64::from(actual.size.h),
    ));
    surfaces
        .into_iter()
        .map(|surface| match rounding {
            // Round as it is drawn, so its corners are as round however far it is stretched.
            Some(rounding) => {
                let scaled = RescaleRenderElement::from_element(rounding.apply(surface), origin, factor);
                let relocated = RelocateRenderElement::from_element(scaled, offset, Relocate::Relative);
                SceneElement::RoundedThumbnail(relocated)
            }
            None => {
                let scaled = RescaleRenderElement::from_element(surface, origin, factor);
                let relocated = RelocateRenderElement::from_element(scaled, offset, Relocate::Relative);
                SceneElement::Thumbnail(relocated)
            }
        })
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
/// How much a surface's program fades it through wp_alpha_modifier, which smithay applies to
/// what the surface draws; its glass goes with it.
fn surface_alpha(surface: &WlSurface) -> f32 {
    with_states(surface, |states| {
        states
            .cached_state
            .get::<AlphaModifierSurfaceCachedState>()
            .current()
            .multiplier_f32()
            .unwrap_or(1.0)
    })
}

/// Where a popup's glass goes, and the radius of its corners: a menu's, or the blur an input
/// method asked for through ext-background-effect.
fn popup_glass_area(popup: &PopupKind, origin: Point<i32, Logical>) -> Option<(Rectangle<i32, Logical>, i32)> {
    let xdg = match popup {
        PopupKind::Xdg(xdg) => xdg,
        PopupKind::InputMethod(_) => {
            let blur = crate::handlers::background_effect::blur_of(popup.wl_surface())?;
            let area = Rectangle::new(blur.area.loc + origin - popup.geometry().loc, blur.area.size);
            return Some((area, blur.radius));
        }
    };
    let mut area = Rectangle::new(origin, popup.geometry().size);
    // shortcut: a popover with an arrow is known by GTK's input region, which is the arrow and
    // the rounded contents rather than one rectangle; other toolkits drawing arrows would need
    // a way to say where their glass goes.
    let shaped = with_states(xdg.wl_surface(), |states| {
        states
            .cached_state
            .get::<SurfaceAttributes>()
            .current()
            .input_region
            .as_ref()
            .is_some_and(|region| region.rects.len() > 1)
    });
    if let (true, Some((geometry, anchor))) = (shaped, popup_placement(popup)) {
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
    (!area.is_empty()).then_some((area, POPUP_RADIUS))
}

/// Where a menu was put and what it was opened from, both from where its parent's window
/// geometry starts.
fn popup_placement(popup: &PopupKind) -> Option<(Rectangle<i32, Logical>, Rectangle<i32, Logical>)> {
    let PopupKind::Xdg(xdg) = popup else {
        return None;
    };
    xdg.with_committed_state(|state| state.map(|state| (state.geometry, state.positioner.anchor_rect)))
}

/// The point of a menu, drawn over `rect`, nearest the middle of what it was opened from:
/// where it grows out of. Its own middle, when it does not say what that was.
fn popup_pivot(popup: &PopupKind, rect: Rectangle<i32, Logical>) -> Point<i32, Logical> {
    let Some((geometry, anchor)) = popup_placement(popup) else {
        return rect.loc + Point::from((rect.size.w / 2, rect.size.h / 2));
    };
    let anchor_middle = anchor.loc + Point::from((anchor.size.w / 2, anchor.size.h / 2)) - geometry.loc;
    rect.loc
        + Point::from((
            anchor_middle.x.clamp(0, rect.size.w.max(0)),
            anchor_middle.y.clamp(0, rect.size.h.max(0)),
        ))
}

/// The corners GTK's windows have in Nekoland's look, as its `--window-radius` says.
pub const WINDOW_RADIUS: i32 = 14;
/// How far past a window not GTK's its glass shows, in physical pixels, as GTK's shows at its
/// edge at any scale.
const RIM: i32 = 1;

/// `windows`, front to back, each where it is drawn, over `backdrop` and on glass of what is
/// under it.
fn windows_on_glass(
    state: &Nekoland,
    renderer: &mut GlesRenderer,
    output: &Output,
    mut glass: Option<&mut Glass>,
    windows: &[(Window, Pose)],
    backdrop: Vec<SceneElement>,
) -> Vec<SceneElement> {
    let output_loc = state
        .space
        .output_geometry(output)
        .map(|geometry| geometry.loc)
        .unwrap_or_default();
    let scale = output.current_scale().fractional_scale();
    let output_size = output.current_mode().map(|mode| mode.size);
    // Back to front while it is put together, each window added on top, and turned round once
    // at the end.
    let mut scene = backdrop;
    scene.reverse();
    let mut kept = Vec::new();
    // From the bottom up, so the glass of each one shows those under it, theirs included.
    for (window, pose) in windows.iter().rev() {
        // Where a window being dragged will go shows under it, over the rest.
        if let Some(target) = state.snap.target.as_ref().filter(|target| &target.window == window) {
            let area = Rectangle::new(target.rect.loc - output_loc, target.rect.size)
                .to_physical_precise_round(scale);
            if let (Some(glass), Some(size)) = (glass.as_deref_mut(), output_size) {
                let id = state.snap.id.clone();
                if let Err(err) = glass.update_window(renderer, id, (size, scale), &[area], &scene) {
                    tracing::warn!("glass: {err}");
                }
                let radius = (f64::from(MENU_RADIUS) * scale).round() as i32;
                scene.extend(glass.window_pane(state.snap.id.clone(), area, radius).map(SceneElement::Glass));
                kept.push(state.snap.id.clone());
            }
            scene.extend(
                state
                    .snap
                    .picture
                    .as_ref()
                    .filter(|(size, ..)| *size == target.rect.size)
                    .and_then(|(_, _, picture)| {
                        MemoryRenderBufferRenderElement::from_buffer(
                            renderer,
                            area.loc.to_f64(),
                            &picture.buffer,
                            None,
                            None,
                            None,
                            Kind::Unspecified,
                        )
                        .ok()
                    })
                    .map(SceneElement::Picture),
            );
        }
        let (round, turning) = crate::windows::roundness(window);
        let corner = (f64::from(WINDOW_RADIUS) * round).round() as i32;
        // Corners a program draws square are cut here: still while they turn square, for a
        // window maximized as it grows.
        let rounding = (corner > 0 && state.cuts_corners(window))
            .then(|| {
                let radius = (f64::from(corner) * scale).round() as i32;
                // Where the window is, not where it is shown: its surfaces are put there and
                // then stretched. Under the compositor's bar the corners are the bar's, so
                // only the bottom ones are round.
                let body = Rectangle::new(
                    pose.actual.loc - output_loc,
                    pose.actual.size - crate::frame::trim(window),
                );
                let body = state.with_title(window, body).to_physical_precise_round(scale);
                crate::rounded::Rounding::new(renderer, radius, body, scale, turning)
            })
            .flatten();
        let mut elements =
            posed_elements(renderer, window, *pose, output_loc, scale, false, rounding.as_ref());
        // In front of the window, which a bar over a title bar of the program's own covers.
        elements.splice(0..0, crate::frame::title_element(state, renderer, window, *pose, output_loc, scale));
        // Where the compositor draws the window's frame: around the window and the bar over it.
        let framed = state.shadow_radius(window, corner).map(|radius| {
            let body = Rectangle::new(
                pose.shown.loc - output_loc,
                pose.shown.size - crate::frame::trim(window),
            );
            (state.with_title(window, body), radius)
        });
        // A program not GTK's has only a line at its edge, as wide as GTK's own and grey as
        // it, lighter than what is around the window.
        let rim = framed.filter(|_| ![CONFIRM_APP_ID, LAUNCHER_APP_ID].contains(&app_id(window).as_str()));
        // Only what shows is blurred and drawn: the program's body covers the rest.
        let (area, radius, covered) = match rim {
            Some((body, radius)) => {
                let body = body.to_physical_precise_round(scale);
                let outer = Rectangle::new(
                    body.loc - Point::from((RIM, RIM)),
                    body.size + Size::from((2 * RIM, 2 * RIM)),
                );
                let corner = (f64::from(radius) * scale).round() as i32;
                // All of the body but its corners, the compositor cuts them; none while it fades.
                let covered = if pose.alpha < 1.0 {
                    Vec::new()
                } else {
                    vec![
                        Rectangle::new(
                            body.loc + Point::from((corner, 0)),
                            body.size - Size::from((2 * corner, 0)),
                        ),
                        Rectangle::new(
                            body.loc + Point::from((0, corner)),
                            body.size - Size::from((0, 2 * corner)),
                        ),
                    ]
                };
                (Some(outer), corner + RIM, covered)
            }
            None => {
                // An X11 program's menus and tooltips are mostly transparent shadow margin,
                // which glass would blur: X11 gives no way to tell their shadow from their
                // body, and they draw their own background anyway.
                let area = Some(pose_area(window, *pose, output_loc, scale))
                    .filter(|_| is_managed(window) && !is_x11_drawing_own_frame(window));
                let radius = framed.map_or(corner, |(_, radius)| radius);
                // Nor behind one that covers all of it, which would never show.
                let area = area.filter(|_| !hides_glass(window, *pose, radius));
                let covered = opaque_area(window, *pose, output_loc, scale);
                (area, (f64::from(radius) * scale).round() as i32, covered)
            }
        };
        let area = area.filter(|area| !area.is_empty());
        if let (Some(glass), Some(area), Some(size)) = (glass.as_deref_mut(), area, output_size) {
            let id = window_glass_id(window);
            let shown = area.subtract_rects(covered.into_iter().filter(|part| !part.is_empty()));
            if let Err(err) = glass.update_window(renderer, id.clone(), (size, scale), &shown, &scene) {
                tracing::warn!("glass: {err}");
            }
            elements.extend(
                glass
                    .window_pane(id.clone(), area, radius)
                    // An edge round as the corners the compositor cuts, past them all along.
                    .map(|pane| if rim.is_some() { pane.circular().outline() } else { pane })
                    .map(|pane| SceneElement::Glass(pane.only(&shown).faded(pose.alpha).reshaped(turning))),
            );
            kept.push(id);
        }
        if let Some((body, radius)) = framed {
            let focused = state.focused_window().as_ref() == Some(window);
            elements.extend(crate::rounded::shadow(
                renderer,
                window,
                output_loc,
                body,
                radius,
                pose.alpha,
                focused,
            ));
        }
        scene.extend(elements.into_iter().rev());
    }
    if let Some(glass) = glass {
        // A window on its way into the Dock keeps the glass of what is under where it was.
        for genie in state.dock_genies() {
            let shown = genie_shown(state, &genie.window, genie.from);
            let (Some((area, _)), Some(size)) =
                (genie_glass(state, &genie.window, shown, output_loc, scale), output_size)
            else {
                continue;
            };
            let id = window_glass_id(&genie.window);
            if let Err(err) = glass.update_window(renderer, id.clone(), (size, scale), &[area], &scene) {
                tracing::warn!("glass: {err}");
            }
            kept.push(id);
        }
        glass.keep_windows(&kept);
    }
    scene.reverse();
    scene
}

/// Whether a Wayland window drawn as `pose` is opaque wherever its glass would show, which
/// is then never seen: the opaque region it gives covers all of it but its corners, `radius`
/// round, which GTK leaves out. Not while it fades, nor when it is drawn larger than it is,
/// with corners rounder than the glass's.
fn hides_glass(window: &Window, pose: Pose, radius: i32) -> bool {
    let (actual, shown) = (pose.actual.size, pose.shown.size);
    if pose.alpha < 1.0 || shown.w <= 0 || shown.h <= 0 {
        return false;
    }
    // The corners in the surface's own points, smaller as it is drawn larger: left out to
    // be safe.
    let (corner_w, corner_h) = (radius * actual.w / shown.w, radius * actual.h / shown.h);
    if actual.w <= 2 * corner_w || actual.h <= 2 * corner_h {
        return false;
    }
    let body = Rectangle::new(window.geometry().loc, actual);
    let without_corners = [
        Rectangle::new(
            body.loc + Point::from((corner_w, 0)),
            body.size - Size::from((2 * corner_w, 0)),
        ),
        Rectangle::new(
            body.loc + Point::from((0, corner_h)),
            body.size - Size::from((0, 2 * corner_h)),
        ),
    ];
    opaque_region(window)
        .is_some_and(|opaque| Rectangle::subtract_rects_many(without_corners, opaque).is_empty())
}

/// Where a Wayland window drawn as `pose` says it is opaque, on the output, rounded inward;
/// nothing while it fades or is drawn at another size than its own.
fn opaque_area(
    window: &Window,
    pose: Pose,
    output_loc: Point<i32, Logical>,
    scale: f64,
) -> Vec<Rectangle<i32, Physical>> {
    if pose.alpha < 1.0 || pose.shown.size != pose.actual.size {
        return Vec::new();
    }
    // Put as `pose_area` puts the surface.
    let surface: Point<i32, Physical> =
        (pose.shown.loc - window.geometry().loc - output_loc).to_physical_precise_round(scale);
    opaque_region(window)
        .unwrap_or_default()
        .into_iter()
        .map(|part| {
            let part = part.to_f64().to_physical(scale);
            Rectangle::from_extremities(
                surface + part.loc.to_i32_ceil(),
                surface + (part.loc + part.size.to_point()).to_i32_floor(),
            )
        })
        .collect()
}

/// The opaque region a Wayland window's surface gave, in its own points.
fn opaque_region(window: &Window) -> Option<Vec<Rectangle<i32, Logical>>> {
    let toplevel = window.toplevel()?;
    with_states(toplevel.wl_surface(), |states| {
        let surface = states.data_map.get::<RendererSurfaceStateUserData>()?;
        let surface = surface.lock().unwrap();
        surface.opaque_regions().map(<[_]>::to_vec)
    })
}

/// How opaque the white of the box choosing desktop items is, inside and at its edge. The
/// design page in Settings shows them as `.nk-band`.
pub(crate) const BAND_FILL_ALPHA: f32 = 0.15;
pub(crate) const BAND_EDGE_ALPHA: f32 = 0.6;

/// The four sides of a line width thick just inside outer: top, bottom, left, right.
fn outline_sides(outer: Rectangle<i32, Logical>, width: i32) -> [Rectangle<i32, Logical>; 4] {
    [
        Rectangle::new(outer.loc, Size::from((outer.size.w, width))),
        Rectangle::new(
            outer.loc + Point::from((0, outer.size.h - width)),
            Size::from((outer.size.w, width)),
        ),
        Rectangle::new(outer.loc, Size::from((width, outer.size.h))),
        Rectangle::new(
            outer.loc + Point::from((outer.size.w - width, 0)),
            Size::from((width, outer.size.h)),
        ),
    ]
}

/// The part of the output a window is drawn over, without its shadow.
fn window_area(
    state: &Nekoland,
    window: &Window,
    output_loc: Point<i32, Logical>,
    scale: f64,
) -> Option<Rectangle<i32, Physical>> {
    state.window_pose(window).map(|pose| pose_area(window, pose, output_loc, scale))
}

fn pose_area(
    window: &Window,
    pose: Pose,
    output_loc: Point<i32, Logical>,
    scale: f64,
) -> Rectangle<i32, Physical> {
    // Rounded from where the window's surface is put, as its pixels are: rounded from the
    // body, it lies half a pixel off them one way or the other at a fractional scale, as the
    // window moves, and its edge shows past the window's own or not.
    let margin = window.geometry().loc;
    let surface: Point<i32, Physical> =
        (pose.shown.loc - margin - output_loc).to_physical_precise_round(scale);
    let body = surface + margin.to_physical_precise_round(scale);
    Rectangle::new(body, pose.shown.size.to_physical_precise_round(scale))
}

/// What a window's glass goes by from frame to frame: the window keeps it.
fn window_glass_id(window: &Window) -> Id {
    window.user_data().get_or_insert(Id::new).clone()
}

fn is_top_panel(map: &smithay::desktop::LayerMap, surface: &LayerSurface) -> bool {
    let geometry = map.layer_geometry(surface).unwrap_or_default();
    geometry.loc.y == 0 && geometry.size.h <= TOP_PANEL_MAX_HEIGHT
}

/// The surfaces of some layers, front to back.
fn layer_elements(renderer: &mut GlesRenderer, output: &Output, layers: &[Layer]) -> Vec<SceneElement> {
    let scale = output.current_scale().fractional_scale();
    let map = layer_map_for_output(output);
    layers
        .iter()
        .flat_map(|layer| map.layers_on(*layer).rev())
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
