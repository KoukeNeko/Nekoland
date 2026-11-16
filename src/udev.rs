//! Runs the compositor on a display of its own, through DRM/KMS, with input from libinput.
//!
//! shortcut: one GPU, and the connectors found at startup; hotplug and a second GPU need
//! the udev monitor and per-device state that Smithay's anvil has.

use std::{cell::RefCell, collections::HashMap, rc::Rc, sync::Mutex, time::Duration};

use smithay::{
    backend::{
        allocator::{
            gbm::{GbmAllocator, GbmBufferFlags, GbmDevice},
            Fourcc,
        },
        drm::{
            compositor::FrameFlags,
            exporter::gbm::GbmFramebufferExporter,
            output::{DrmOutput, DrmOutputManager, DrmOutputRenderElements},
            DrmDevice, DrmDeviceFd, DrmEvent, DrmNode, NodeType,
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
    output::{Mode, Output, PhysicalProperties},
    reexports::{
        calloop::{
            timer::{TimeoutAction, Timer},
            EventLoop, LoopHandle,
        },
        drm::control::{crtc, ModeTypeFlags},
        input::Libinput,
        rustix::fs::OFlags,
    },
    utils::{DeviceFd, IsAlive, Physical, Point},
    wayland::{compositor::with_states, dmabuf::DmabufFeedbackBuilder},
};
use smithay_drm_extras::drm_scanner::{DrmScanEvent, DrmScanner};
use tracing::{info, warn};

use crate::{
    cursor::Cursors,
    render::{scene_elements, SceneElement, CLEAR_COLOR},
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
        let configured = state.config.mode_size().and_then(|size| {
            let found = modes
                .iter()
                .filter(|mode| mode.size() == size)
                .max_by_key(|mode| mode.vrefresh());
            if found.is_none() {
                warn!("{name} has no {}x{} mode", size.0, size.1);
            }
            found
        });
        let drm_mode = *configured
            .or_else(|| {
                modes
                    .iter()
                    .find(|mode| mode.mode_type().contains(ModeTypeFlags::PREFERRED))
            })
            .or(modes.first())
            .ok_or("connector has no modes")?;
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
        // The global lives as long as the compositor does.
        let _global = output.create_global::<Nekoland>(&data.display_handle);

        let x = state
            .space
            .outputs()
            .map(|o| state.space.output_geometry(o).unwrap().size.w)
            .sum::<i32>();
        output.set_preferred(mode);
        output.change_current_state(Some(mode), None, None, Some((x, 0).into()));
        state.space.map_output(&output, (x, 0));

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
        surfaces.insert(crtc, (surface, output));
    }
    if surfaces.is_empty() {
        return Err("no connected display".into());
    }

    let udev: SharedUdev = Rc::new(RefCell::new(Udev {
        renderer,
        manager,
        surfaces,
        cursors: Cursors::load(),
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
    }

    let mut libinput =
        Libinput::new_with_udev::<LibinputSessionInterface<LibSeatSession>>(session.clone().into());
    libinput
        .udev_assign_seat(&seat)
        .map_err(|()| "libinput could not take the seat")?;
    handle.insert_source(LibinputInputBackend::new(libinput.clone()), |event, _, data| {
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
                if let Err(err) = udev.borrow_mut().manager.activate(false) {
                    warn!("could not take the display back: {err}");
                }
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
    let mut guard = udev.borrow_mut();
    let Udev {
        renderer,
        surfaces,
        cursors,
        ..
    } = &mut *guard;
    let Some((surface, output)) = surfaces.get_mut(&crtc) else {
        return;
    };

    let queued = draw(state, renderer, surface, output, cursors).unwrap_or_else(|err| {
        warn!("render: {err}");
        false
    });
    let output = output.clone();
    drop(guard);

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

/// Returns whether a frame was handed to the display.
fn draw(
    state: &Nekoland,
    renderer: &mut GlesRenderer,
    surface: &mut Surface,
    output: &Output,
    cursors: &mut Cursors,
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
    let pointer = (pointer - output_origin.to_f64()).to_physical(1.0).to_i32_round();

    // Front to back: the cursor is above everything else.
    let mut elements = cursor_elements(state, renderer, cursors, pointer);
    elements.extend(scene_elements(state, renderer, output)?);

    let is_empty = surface
        .render_frame(renderer, &elements, CLEAR_COLOR, FrameFlags::DEFAULT)
        .map_err(|err| err.to_string())?
        .is_empty;
    if is_empty {
        return Ok(false);
    }
    surface.queue_frame(()).map_err(|err| err.to_string())?;
    Ok(true)
}

/// The cursor a client drew itself, or the theme's picture for the one it named.
fn cursor_elements(
    state: &Nekoland,
    renderer: &mut GlesRenderer,
    cursors: &mut Cursors,
    pointer: Point<i32, Physical>,
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
            let location = pointer - hotspot.to_physical(1);
            return render_elements_from_surface_tree(renderer, surface, location, 1.0, 1.0, Kind::Cursor);
        }
        // A client that went away leaves its cursor behind; fall back to the arrow.
        CursorImageStatus::Surface(_) => CursorIcon::Default,
        CursorImageStatus::Named(icon) => *icon,
    };

    let Some(image) = cursors.get(icon) else {
        return Vec::new();
    };
    let location = (pointer - image.hotspot).to_f64();
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
