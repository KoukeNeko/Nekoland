//! Runs the compositor on a display of its own, through DRM/KMS, with input from libinput.
//!
//! shortcut: one GPU, and the connectors found at startup; hotplug and a second GPU need
//! the udev monitor and per-device state that Smithay's anvil has.

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
    sync::Mutex,
    time::Duration,
};

use smithay::{
    backend::input::InputEvent,
    backend::{
        allocator::{
            gbm::{GbmAllocator, GbmBufferFlags, GbmDevice},
            Buffer, Fourcc,
        },
        drm::{
            compositor::{FrameFlags, PrimaryPlaneElement, RenderFrameResult},
            exporter::gbm::GbmFramebufferExporter,
            output::{DrmOutput, DrmOutputManager, DrmOutputRenderElements},
            DrmDevice, DrmDeviceFd, DrmEvent, DrmNode, Framebuffer, NodeType,
        },
        egl::{EGLContext, EGLDisplay},
        libinput::{LibinputInputBackend, LibinputSessionInterface},
        renderer::{
            element::{
                memory::MemoryRenderBufferRenderElement, surface::render_elements_from_surface_tree, Kind,
            },
            gles::GlesRenderer,
            ImportDma, ImportMemWl,
        },
        session::{libseat::LibSeatSession, Event as SessionEvent, Session},
        udev::{all_gpus, primary_gpu},
    },
    input::pointer::{CursorIcon, CursorImageAttributes, CursorImageStatus},
    output::{Mode, Output, PhysicalProperties, Scale},
    reexports::{
        calloop::{
            timer::{TimeoutAction, Timer},
            EventLoop, LoopHandle,
        },
        drm::control::{crtc, Device as _, Mode as DrmMode, ModeTypeFlags},
        input::Libinput,
        rustix::fs::OFlags,
    },
    utils::{DeviceFd, IsAlive, Physical, Point},
    wayland::{
        compositor::with_states,
        dmabuf::DmabufFeedbackBuilder,
        drm_syncobj::{supports_syncobj_eventfd, DrmSyncobjState},
    },
};
use smithay_drm_extras::drm_scanner::{DrmScanEvent, DrmScanner};
use tracing::{info, warn};

use crate::{
    config::{Config, Rotation},
    cursor::Cursors,
    glass::Glass,
    render::{dnd_icon_elements, scene_elements, zoomed, OutputElement, SceneElement, CLEAR_COLOR},
    CalloopData, Nekoland,
};

// How soon to look again when a frame had nothing new to show.
const IDLE_REDRAW: Duration = Duration::from_millis(16);

type OutputManager =
    DrmOutputManager<GbmAllocator<DrmDeviceFd>, GbmFramebufferExporter<DrmDeviceFd>, (), DrmDeviceFd>;
type Surface = DrmOutput<GbmAllocator<DrmDeviceFd>, GbmFramebufferExporter<DrmDeviceFd>, (), DrmDeviceFd>;

struct Udev {
    renderer: GlesRenderer,
    manager: OutputManager,
    surfaces: HashMap<crtc::Handle, (Surface, Output)>,
    cursors: Cursors,
    /// Each output's, as what it blurs is what that output shows; none when the GPU could
    /// not build the glass shaders.
    glass: HashMap<crtc::Handle, Glass>,
    /// The outputs turned off while the screens are, to be drawn afresh when they come back.
    dark: HashSet<crtc::Handle>,
    /// The outputs whose gamma can be set, by the length of their ramps.
    gamma: HashMap<crtc::Handle, u32>,
    /// The red, green and blue gains each output's ramps were last set to, and whether the
    /// colours are inverted.
    ramps: HashMap<crtc::Handle, (f64, f64, f64, bool)>,
    /// The `color_filter` each output's colour matrix was last set to; None for one that has
    /// no matrix.
    filters: HashMap<crtc::Handle, Option<String>>,
    /// The `color_profile` setting the ramps follow, and the calibration curves read from it.
    profile: (Option<String>, Option<crate::icc::Curves>),
    /// Each output's modes and size in millimetres, to choose from again as the settings change.
    displays: HashMap<crtc::Handle, (Vec<DrmMode>, (u32, u32))>,
    /// The `mode`, `scale` and `rotation` settings the outputs were last set up with.
    applied: (Option<String>, Option<f64>, Rotation, Vec<String>, String),
    /// While it is open, the power button and the lid are Nekoland's.
    _power_keys: Option<zbus::zvariant::OwnedFd>,
}

type SharedUdev = Rc<RefCell<Udev>>;

pub fn init_udev(
    event_loop: &mut EventLoop<'static, CalloopData>,
    data: &mut CalloopData,
) -> Result<(), Box<dyn std::error::Error>> {
    let handle = event_loop.handle();
    let state = &mut data.state;

    let (mut session, session_notifier) = LibSeatSession::new()?;
    let seat = session.seat();

    let gpu_path = primary_gpu(&seat)?
        .or_else(|| all_gpus(&seat).ok()?.into_iter().next())
        .ok_or("no GPU found")?;
    info!("Using {}", gpu_path.display());

    let fd = session.open(
        &gpu_path,
        OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOCTTY | OFlags::NONBLOCK,
    )?;
    let fd = DrmDeviceFd::new(DeviceFd::from(fd));
    if supports_syncobj_eventfd(&fd) {
        state.syncobj_state = Some(DrmSyncobjState::new::<Nekoland>(&data.display_handle, fd.clone()));
    }
    let (drm, drm_notifier) = DrmDevice::new(fd.clone(), true)?;
    let gbm = GbmDevice::new(fd)?;

    let egl_display = unsafe { EGLDisplay::new(gbm.clone())? };
    let mut renderer = unsafe { GlesRenderer::new(EGLContext::new(&egl_display)?)? };
    let render_formats = renderer.egl_context().dmabuf_render_formats().clone();

    let mut manager: OutputManager = DrmOutputManager::new(
        drm,
        GbmAllocator::new(gbm.clone(), GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT),
        GbmFramebufferExporter::new(gbm.clone(), None),
        Some(gbm),
        [Fourcc::Abgr8888, Fourcc::Argb8888],
        render_formats,
    );

    let mut surfaces = HashMap::new();
    let mut displays = HashMap::new();
    let mut gamma = HashMap::new();
    for event in <DrmScanner>::new().scan_connectors(manager.device())? {
        let DrmScanEvent::Connected {
            connector,
            crtc: Some(crtc),
        } = event
        else {
            continue;
        };

        let name = format!("{}-{}", connector.interface().as_str(), connector.interface_id());
        let modes = connector.modes();
        let drm_mode = choose_mode(&name, modes, &state.config).ok_or("connector has no modes")?;
        let mode = Mode::from(drm_mode);
        let (width_mm, height_mm) = connector.size().unwrap_or((0, 0));

        let output = Output::new(
            name.clone(),
            PhysicalProperties {
                size: (width_mm as i32, height_mm as i32).into(),
                subpixel: connector.subpixel().into(),
                make: "Unknown".into(),
                model: "Unknown".into(),
            },
        );
        for mode in modes {
            output.add_mode(Mode::from(*mode));
        }
        // The global lives as long as the compositor does.
        let _global = output.create_global::<Nekoland>(&data.display_handle);

        // Side by side, each as wide as it is in points.
        let x = state
            .space
            .outputs()
            .map(|o| state.space.output_geometry(o).unwrap().size.w)
            .sum::<i32>();
        let scale = chosen_scale(&state.config, &name, mode, (width_mm, height_mm));
        info!("{name}: scale {scale}");
        output.set_preferred(mode);
        output.change_current_state(
            Some(mode),
            Some(state.config.rotation.transform()),
            Some(Scale::Fractional(scale)),
            Some((x, 0).into()),
        );
        crate::text::set_picture_scale(scale);
        state.space.map_output(&output, (x, 0));
        lay_out_outputs(state);

        let surface = manager
            .initialize_output::<GlesRenderer, SceneElement>(
                crtc,
                drm_mode,
                &[connector.handle()],
                &output,
                None,
                &mut renderer,
                &DrmOutputRenderElements::default(),
            )
            .map_err(|err| format!("could not set up {name}: {err}"))?;
        info!("{name}: {}x{}", mode.size.w, mode.size.h);
        match manager.device().get_crtc(crtc) {
            Ok(info) if info.gamma_length() > 1 => {
                gamma.insert(crtc, info.gamma_length());
            }
            Ok(_) => {}
            Err(err) => warn!("{name}: no gamma: {err}"),
        }
        surfaces.insert(crtc, (surface, output));
        displays.insert(crtc, (modes.to_vec(), (width_mm, height_mm)));
    }
    if surfaces.is_empty() {
        return Err("no connected display".into());
    }

    let power_keys = crate::power::take_power_keys()
        .inspect_err(|err| warn!("logind keeps the power button and the lid: {err}"))
        .ok();
    state.night_shift_available = !gamma.is_empty();
    let udev: SharedUdev = Rc::new(RefCell::new(Udev {
        dark: HashSet::new(),
        displays,
        applied: applied_settings(&state.config),
        gamma,
        ramps: HashMap::new(),
        filters: HashMap::new(),
        profile: (None, None),
        _power_keys: power_keys,
        glass: surfaces
            .keys()
            .filter_map(|crtc| {
                Glass::new(&mut renderer)
                    .inspect_err(|err| warn!("no glass: {err}"))
                    .ok()
                    .map(|glass| (*crtc, glass))
            })
            .collect(),
        renderer,
        manager,
        surfaces,
        cursors: Cursors::load(&state.config.cursor_theme),
    }));

    // Clients that draw with the GPU hand their buffers over as dmabufs.
    {
        let backend = udev.borrow();
        state.shm_state.update_formats(backend.renderer.shm_formats());
        let node = DrmNode::from_path(&gpu_path)?;
        let node = node
            .node_with_type(NodeType::Render)
            .and_then(Result::ok)
            .unwrap_or(node);
        let feedback =
            DmabufFeedbackBuilder::new(node.dev_id(), backend.renderer.dmabuf_formats()).build()?;
        // The global lives as long as the compositor does.
        let _global = state
            .dmabuf_state
            .create_global_with_default_feedback::<Nekoland>(&data.display_handle, &feedback);
        let importer = udev.clone();
        state.dmabuf_import = Some(Box::new(move |dmabuf| {
            importer.borrow_mut().renderer.import_dmabuf(dmabuf, None).is_ok()
        }));
        let snapping = udev.clone();
        state.snapshooter = Some(Box::new(move |window, scale| {
            // Not while a frame is being drawn with it; the window then goes without its picture.
            let mut backend = snapping.try_borrow_mut().ok()?;
            crate::motion::snapshot(&mut backend.renderer, window, scale)
                .inspect_err(|err| warn!("window snapshot: {err}"))
                .ok()
                .flatten()
        }));
    }

    let mut libinput =
        Libinput::new_with_udev::<LibinputSessionInterface<LibSeatSession>>(session.clone().into());
    libinput
        .udev_assign_seat(&seat)
        .map_err(|()| "libinput could not take the seat")?;
    handle.insert_source(LibinputInputBackend::new(libinput.clone()), |event, _, data| {
        if let InputEvent::DeviceAdded { device } = &event {
            data.state.add_pointer_device(device.clone());
        }
        data.state.process_input_event(event)
    })?;

    {
        let udev = udev.clone();
        let loop_handle = handle.clone();
        handle.insert_source(drm_notifier, move |event, _, data| match event {
            DrmEvent::VBlank(crtc) => {
                if let Some((surface, _)) = udev.borrow().surfaces.get(&crtc) {
                    if let Err(err) = surface.frame_submitted() {
                        warn!("frame_submitted: {err}");
                    }
                }
                render(&udev, crtc, data, &loop_handle);
            }
            DrmEvent::Error(err) => warn!("drm: {err}"),
        })?;
    }

    {
        let udev = udev.clone();
        let loop_handle = handle.clone();
        handle.insert_source(session_notifier, move |event, _, data| match event {
            SessionEvent::PauseSession => {
                libinput.suspend();
                udev.borrow_mut().manager.pause();
            }
            SessionEvent::ActivateSession => {
                if libinput.resume().is_err() {
                    warn!("libinput did not resume");
                }
                let mut backend = udev.borrow_mut();
                if let Err(err) = backend.manager.activate(false) {
                    warn!("could not take the display back: {err}");
                }
                // Whoever had the display meanwhile may have left ramps of their own.
                backend.ramps.clear();
                backend.filters.clear();
                drop(backend);
                render_all(&udev, data, &loop_handle);
            }
        })?;
    }

    std::env::set_var("WAYLAND_DISPLAY", &state.socket_name);

    let loop_handle = handle.clone();
    handle.insert_idle(move |data| render_all(&udev, data, &loop_handle));

    // The state keeps the session alive, which has to outlive everything opened through it.
    data.state.switch_vt = Some(Box::new(move |vt| {
        if let Err(err) = session.change_vt(vt) {
            warn!("could not switch to VT {vt}: {err}");
        }
    }));

    Ok(())
}

fn render_all(udev: &SharedUdev, data: &mut CalloopData, handle: &LoopHandle<'static, CalloopData>) {
    let crtcs: Vec<_> = udev.borrow().surfaces.keys().copied().collect();
    for crtc in crtcs {
        render(udev, crtc, data, handle);
    }
}

/// Draws one output. A frame that reaches the display is followed by a vblank event,
/// which draws the next one; a frame with nothing new is retried after `IDLE_REDRAW`.
fn render(
    udev: &SharedUdev,
    crtc: crtc::Handle,
    data: &mut CalloopData,
    handle: &LoopHandle<'static, CalloopData>,
) {
    let state = &mut data.state;
    state.tick_overview();
    state.tick_motion();
    state.refresh_menu_bar();
    state.dock_ids();
    state.refresh_dock();
    state.refresh_desktop();
    state.refresh_hover();
    state.refresh_apps();
    state.refresh_notices();
    state.refresh_fps();
    let mut guard = udev.borrow_mut();
    if guard.applied != applied_settings(&state.config) {
        apply_display_settings(&mut guard, state);
    }
    let Udev {
        renderer,
        manager,
        surfaces,
        cursors,
        glass,
        dark,
        gamma,
        ramps,
        filters,
        profile,
        ..
    } = &mut *guard;
    let Some((surface, output)) = surfaces.get_mut(&crtc) else {
        return;
    };

    state.follow_night_shift();
    if profile.0 != state.config.color_profile {
        *profile = (state.config.color_profile.clone(), state.config.color_profile.as_deref().and_then(crate::icc::load));
        ramps.clear();
    }
    let filter = &state.config.color_filter;
    let applied = filters.get(&crtc).cloned().unwrap_or(Some("none".into()));
    if applied.is_some_and(|applied| applied != *filter) {
        match set_colour_matrix(manager.device(), crtc, filter_matrix(filter)) {
            Ok(()) => {
                filters.insert(crtc, Some(filter.clone()));
            }
            Err(err) => {
                warn!("could not set the colour matrix of {}: {err}", output.name());
                filters.insert(crtc, None);
            }
        }
    }
    if let Some(&size) = gamma.get(&crtc) {
        let gains = if state.night_shift {
            crate::night::gains(state.config.night_shift_warmth)
        } else {
            (1.0, 1.0, 1.0)
        };
        let invert = state.config.invert_colors;
        let gains = (gains.0, gains.1, gains.2, invert);
        if ramps.get(&crtc) != Some(&gains) {
            let (r, g, b, _) = gains;
            let ramp = |channel: usize, gain: f64| -> Vec<u16> {
                (0..size)
                    .map(|i| {
                        let x = f64::from(i) / f64::from(size - 1);
                        let level = profile.1.as_ref().map_or(x, |curves| curves.at(channel, x));
                        let level = if invert { 1.0 - level } else { level };
                        (level * gain * 65535.0).round() as u16
                    })
                    .collect()
            };
            match manager.device().set_gamma(crtc, &ramp(0, r), &ramp(1, g), &ramp(2, b)) {
                Ok(()) => {
                    ramps.insert(crtc, gains);
                }
                Err(err) => {
                    warn!("could not set the gamma of {}: {err}", output.name());
                    gamma.remove(&crtc);
                }
            }
        }
    }

    // A dark output is still looked at every `IDLE_REDRAW`, to come back on at once.
    let queued = if state.screens_off {
        if dark.insert(crtc) {
            if let Err(err) = surface.with_compositor(|compositor| compositor.clear()) {
                warn!("could not turn {} off: {err}", output.name());
            }
        }
        false
    } else {
        if dark.remove(&crtc) {
            surface.reset_buffers();
        }
        let frames = state.take_screencopy(output);
        draw(state, renderer, surface, output, cursors, glass.get_mut(&crtc), frames).unwrap_or_else(|err| {
            warn!("render: {err}");
            false
        })
    };
    let output = output.clone();
    drop(guard);
    if queued {
        state.count_frame();
    }

    if !queued {
        let udev = udev.clone();
        let loop_handle = handle.clone();
        let retry = Timer::from_duration(IDLE_REDRAW);
        if let Err(err) = handle.insert_source(retry, move |_, _, data| {
            render(&udev, crtc, data, &loop_handle);
            TimeoutAction::Drop
        }) {
            warn!("could not schedule a redraw: {err}");
        }
    }

    state.finish_frame(&output);
    let _ = data.display_handle.flush_clients();
}

/// The mode the settings ask for, at its fastest rate unless they name one; the display's
/// preferred one otherwise.
fn choose_mode(name: &str, modes: &[DrmMode], config: &Config) -> Option<DrmMode> {
    let configured = config.wanted_mode().and_then(|(size, rate)| {
        let found = modes
            .iter()
            .filter(|mode| mode.size() == size && rate.is_none_or(|rate| mode.vrefresh() == rate))
            .max_by_key(|mode| mode.vrefresh());
        if found.is_none() {
            warn!("{name} has no {} mode", config.mode.as_deref().unwrap_or_default());
        }
        found
    });
    configured
        .or_else(|| {
            modes
                .iter()
                .find(|mode| mode.mode_type().contains(ModeTypeFlags::PREFERRED))
        })
        .or(modes.first())
        .copied()
}

fn chosen_scale(config: &Config, name: &str, mode: Mode, (width_mm, height_mm): (u32, u32)) -> f64 {
    config
        .display_scales
        .get(name)
        .copied()
        .or(config.scale)
        .unwrap_or_else(|| density_scale(mode.size.w, mode.size.h, width_mm, height_mm))
}

type Matrix = [[f64; 3]; 3];

/// Machado, Oliveira and Fernandes' matrices for how a viewer without the red, green or blue
/// cones sees colours.
const PROTANOPIA: Matrix =
    [[0.152286, 1.052583, -0.204868], [0.114503, 0.786281, 0.099216], [-0.003882, -0.048116, 1.051998]];
const DEUTERANOPIA: Matrix =
    [[0.367322, 0.860646, -0.227968], [0.280085, 0.672501, 0.047413], [-0.011820, 0.042940, 0.968881]];
const TRITANOPIA: Matrix =
    [[1.255528, -0.076749, -0.178779], [-0.078411, 0.930809, 0.147602], [0.004733, 0.691367, 0.303900]];
/// Rec. 709 luminance in each row.
const GRAYSCALE: Matrix = [[0.2126, 0.7152, 0.0722]; 3];

/// The matrix a `color_filter` is; None for none. A correction adds what the viewer misses,
/// the difference between a colour and how they see it, back in the channels they can tell.
// shortcut: worked on the encoded values, not linear light, which a CRTC's matrix is given.
fn filter_matrix(filter: &str) -> Option<Matrix> {
    let seen = match filter {
        "grayscale" => return Some(GRAYSCALE),
        "protanopia" => PROTANOPIA,
        "deuteranopia" => DEUTERANOPIA,
        "tritanopia" => TRITANOPIA,
        _ => return None,
    };
    // Moves what is missing into the green and blue channels.
    const SHIFT: Matrix = [[0.0, 0.0, 0.0], [0.7, 1.0, 0.0], [0.7, 0.0, 1.0]];
    let mut matrix = [[0.0; 3]; 3];
    for row in 0..3 {
        for column in 0..3 {
            let missing = |k: usize| f64::from(u8::from(k == column)) - seen[k][column];
            let shifted: f64 = (0..3).map(|k| SHIFT[row][k] * missing(k)).sum();
            matrix[row][column] = f64::from(u8::from(row == column)) + shifted;
        }
    }
    Some(matrix)
}

/// Sets a CRTC's colour matrix, or back to none; an error for a CRTC that has none.
fn set_colour_matrix(
    device: &impl smithay::reexports::drm::control::Device,
    crtc: crtc::Handle,
    matrix: Option<Matrix>,
) -> Result<(), String> {
    /// `struct drm_color_ctm`: nine S31.32 sign-magnitude numbers, a row after another.
    #[repr(C)]
    struct ColorMatrix([u64; 9]);
    let fixed = |number: f64| {
        let magnitude = (number.abs() * (1u64 << 32) as f64) as u64;
        if number < 0.0 { magnitude | 1 << 63 } else { magnitude }
    };

    let properties = device.get_properties(crtc).map_err(|err| err.to_string())?;
    let (handles, _) = properties.as_props_and_values();
    let property = handles
        .iter()
        .find(|handle| device.get_property(**handle).is_ok_and(|info| info.name().to_bytes() == b"CTM"))
        .ok_or("no CTM property")?;
    let value = match matrix {
        Some(matrix) => {
            let mut cells = [0u64; 9];
            for (cell, number) in cells.iter_mut().zip(matrix.iter().flatten()) {
                *cell = fixed(*number);
            }
            device.create_property_blob(&ColorMatrix(cells)).map_err(|err| err.to_string())?.into()
        }
        None => 0,
    };
    device.set_property(crtc, *property, value).map_err(|err| err.to_string())
}

/// The settings the outputs are set up from, to tell when they change.
fn applied_settings(config: &Config) -> (Option<String>, Option<f64>, Rotation, Vec<String>, String) {
    (
        config.mode.clone(),
        config.scale,
        config.rotation,
        config.display_order.clone(),
        format!("{:?}{}", config.display_scales, config.mirror_displays),
    )
}

/// Puts the outputs side by side in the order the settings name, the first mapped first, as
/// the main display everything else follows.
fn lay_out_outputs(state: &mut Nekoland) {
    let mut outputs: Vec<Output> = state.space.outputs().cloned().collect();
    // Stable: the displays the settings leave out stay in the order they were found.
    outputs.sort_by_key(|output| {
        state.config.display_order.iter().position(|name| *name == output.name()).unwrap_or(usize::MAX)
    });
    let mut x = 0;
    for output in &outputs {
        let width = state.space.output_geometry(output).map_or(0, |geometry| geometry.size.w);
        state.space.unmap_output(output);
        output.change_current_state(None, None, None, Some((x, 0).into()));
        state.space.map_output(output, (x, 0));
        if !state.config.mirror_displays {
            x += width;
        }
    }
}

/// Sets every output to the mode, scale and rotation the settings now ask for, lays the
/// outputs out side by side again, and fits the windows to them.
fn apply_display_settings(udev: &mut Udev, state: &mut Nekoland) {
    udev.applied = applied_settings(&state.config);
    for (crtc, (surface, output)) in &mut udev.surfaces {
        let Some((modes, size_mm)) = udev.displays.get(crtc) else {
            continue;
        };
        let Some(drm_mode) = choose_mode(&output.name(), modes, &state.config) else {
            continue;
        };
        let mode = Mode::from(drm_mode);
        if output.current_mode() != Some(mode) {
            let elements = DrmOutputRenderElements::<GlesRenderer, SceneElement>::default();
            if let Err(err) = surface.use_mode(drm_mode, &mut udev.renderer, &elements) {
                warn!("{}: could not change the mode: {err}", output.name());
                continue;
            }
            info!("{}: {}x{}", output.name(), mode.size.w, mode.size.h);
        }
        let scale = chosen_scale(&state.config, &output.name(), mode, *size_mm);
        output.change_current_state(
            Some(mode),
            Some(state.config.rotation.transform()),
            Some(Scale::Fractional(scale)),
            None,
        );
        crate::text::set_picture_scale(scale);
        smithay::desktop::layer_map_for_output(output).arrange();
    }
    lay_out_outputs(state);
    state.tell_surfaces_the_scale();
    state.dock.drawn = None;
    state.menu_bar.drawn = None;
    state.fit_windows_to_display();
}

/// Returns whether a frame was handed to the display.
fn draw(
    state: &Nekoland,
    renderer: &mut GlesRenderer,
    surface: &mut Surface,
    output: &Output,
    cursors: &mut Cursors,
    glass: Option<&mut Glass>,
    screencopy: Vec<crate::screencopy::Frame>,
) -> Result<bool, Box<dyn std::error::Error>> {
    let output_origin = state
        .space
        .output_geometry(output)
        .ok_or("output is not mapped")?
        .loc;
    let pointer = state
        .seat
        .get_pointer()
        .ok_or("seat has no pointer")?
        .current_location();
    let scale = output.current_scale().fractional_scale();
    let pointer = (pointer - output_origin.to_f64())
        .to_physical(scale)
        .to_i32_round();

    // Front to back: the cursor is above everything else, and keeps its size when zoomed.
    let mut elements: Vec<OutputElement> =
        cursor_elements(state, renderer, cursors, pointer, scale).into_iter().map(OutputElement::Scene).collect();
    let mut scene = dnd_icon_elements(state, renderer, output);
    let cursor = elements.len() + scene.len();
    scene.extend(scene_elements(state, renderer, output, glass)?);
    elements.extend(zoomed(scene, pointer, state.config.zoom));

    let frame = surface
        .render_frame(renderer, &elements, CLEAR_COLOR, FrameFlags::DEFAULT)
        .map_err(|err| err.to_string())?;
    wait_until_drawn(&frame);
    let is_empty = frame.is_empty;
    if !screencopy.is_empty() {
        crate::screencopy::copy(renderer, output, &elements, cursor, screencopy);
    }
    if is_empty {
        return Ok(false);
    }
    surface.queue_frame(()).map_err(|err| err.to_string())?;
    Ok(true)
}

/// Where the display cannot be told to wait for the renderer, waits for it here. Without
/// this a software renderer's frame is shown half drawn: on Hyper-V the menu bar and the
/// Dock, drawn last, were missing or flickered.
fn wait_until_drawn<B: Buffer, F: Framebuffer, E>(frame: &RenderFrameResult<'_, B, F, E>) {
    if !frame.needs_sync() {
        return;
    }
    if let PrimaryPlaneElement::Swapchain(element) = &frame.primary_element {
        if let Err(err) = element.sync.wait() {
            warn!("the frame may be shown before it is drawn: {err:?}");
        }
    }
}

/// The cursor a client drew itself, or the theme's picture for the one it named.
fn cursor_elements(
    state: &Nekoland,
    renderer: &mut GlesRenderer,
    cursors: &mut Cursors,
    pointer: Point<i32, Physical>,
    scale: f64,
) -> Vec<SceneElement> {
    let icon = match &state.cursor_status {
        CursorImageStatus::Hidden => return Vec::new(),
        CursorImageStatus::Surface(surface) if surface.alive() => {
            let hotspot = with_states(surface, |states| {
                states
                    .data_map
                    .get::<Mutex<CursorImageAttributes>>()
                    .map(|attributes| attributes.lock().unwrap().hotspot)
                    .unwrap_or_default()
            });
            let location = pointer - hotspot.to_physical_precise_round(scale);
            return render_elements_from_surface_tree(renderer, surface, location, scale, 1.0, Kind::Cursor);
        }
        // A client that went away leaves its cursor behind; fall back to the arrow.
        CursorImageStatus::Surface(_) => CursorIcon::Default,
        CursorImageStatus::Named(icon) => *icon,
    };

    if cursors.asked != state.config.cursor_theme {
        *cursors = Cursors::load(&state.config.cursor_theme);
    }
    let Some(image) = cursors.get(icon, scale, state.config.cursor_size) else {
        return Vec::new();
    };
    let location = pointer.to_f64() - image.hotspot.to_physical(scale);
    MemoryRenderBufferRenderElement::from_buffer(
        renderer,
        location,
        &image.buffer,
        None,
        None,
        None,
        Kind::Cursor,
    )
    .map(|element| vec![SceneElement::Picture(element)])
    .unwrap_or_else(|err| {
        warn!("cursor: {err}");
        Vec::new()
    })
}

/// The scale the Mac would give a display: twice for one dense enough to be Retina,
/// which a 4K desktop display and a laptop's high-resolution panel are, and none for one
/// as coarse as a 1080p or 1440p monitor, whose points would then be too big.
pub fn density_scale(width: i32, height: i32, width_mm: u32, height_mm: u32) -> f64 {
    const RETINA_PPI: f64 = 130.0;
    const RETINA_ROWS: i32 = 1600;
    // 4K and up is drawn at twice whatever size it claims: virtual displays, QEMU's
    // among them, claim 100 dpi at any resolution.
    const FOUR_K_ROWS: i32 = 2000;
    let ppi = (width_mm > 0 && height_mm > 0).then(|| {
        let diagonal_px = f64::from(width).hypot(f64::from(height));
        let diagonal_in = f64::from(width_mm).hypot(f64::from(height_mm)) / 25.4;
        diagonal_px / diagonal_in
    });
    let dense = height >= FOUR_K_ROWS || (height >= RETINA_ROWS && ppi.is_some_and(|ppi| ppi >= RETINA_PPI));
    if dense {
        2.0
    } else {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::{density_scale, filter_matrix};

    #[test]
    fn a_filter_keeps_white_white_and_none_is_no_matrix() {
        assert_eq!(filter_matrix("none"), None);
        assert_eq!(filter_matrix("nonsense"), None);
        for filter in ["grayscale", "protanopia", "deuteranopia", "tritanopia"] {
            let matrix = filter_matrix(filter).unwrap();
            for row in matrix {
                assert!((row.iter().sum::<f64>() - 1.0).abs() < 1e-3, "{filter}: {row:?}");
            }
        }
    }

    #[test]
    fn a_correction_leaves_the_red_channel_alone() {
        assert_eq!(filter_matrix("protanopia").unwrap()[0], [1.0, 0.0, 0.0]);
    }

    #[test]
    fn dense_displays_are_drawn_at_twice() {
        // 27-inch and 32-inch 4K, a 13-inch 2560x1600 laptop, and a 4K of unknown size.
        assert_eq!(density_scale(3840, 2160, 597, 336), 2.0);
        assert_eq!(density_scale(3840, 2160, 708, 398), 2.0);
        assert_eq!(density_scale(2560, 1600, 286, 179), 2.0);
        assert_eq!(density_scale(3840, 2160, 0, 0), 2.0);
        // QEMU's virtual display at 4K, which it says is 100 dpi.
        assert_eq!(density_scale(3840, 2160, 975, 549), 2.0);
        // 27-inch 1440p, 24-inch 1080p, a 13-inch 1080p laptop, and a 1080p of unknown size.
        assert_eq!(density_scale(2560, 1440, 597, 336), 1.0);
        assert_eq!(density_scale(1920, 1080, 527, 296), 1.0);
        assert_eq!(density_scale(1920, 1080, 294, 165), 1.0);
        assert_eq!(density_scale(1920, 1080, 0, 0), 1.0);
    }
}
