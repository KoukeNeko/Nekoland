//! Runs the compositor on a display of its own, through DRM/KMS, with input from libinput.
//!
//! shortcut: one GPU, and the connectors found at startup; hotplug and a second GPU need
//! the udev monitor and per-device state that Smithay's anvil has.

use std::{cell::RefCell, collections::HashMap, rc::Rc, time::Duration};

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
            DrmDevice, DrmDeviceFd, DrmEvent,
        },
        egl::{EGLContext, EGLDisplay},
        libinput::{LibinputInputBackend, LibinputSessionInterface},
        renderer::{
            element::{solid::SolidColorRenderElement, surface::WaylandSurfaceRenderElement, Id, Kind},
            gles::GlesRenderer,
            utils::CommitCounter,
        },
        session::{libseat::LibSeatSession, Event as SessionEvent, Session},
        udev::{all_gpus, primary_gpu},
    },
    desktop::space::{space_render_elements, SpaceRenderElements},
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
    utils::{DeviceFd, Rectangle},
};
use smithay_drm_extras::drm_scanner::{DrmScanEvent, DrmScanner};
use tracing::{info, warn};

use crate::{CalloopData, Nekoland};

const CLEAR_COLOR: [f32; 4] = [0.1, 0.1, 0.1, 1.0];
const CURSOR_SIZE: i32 = 12;
const CURSOR_COLOR: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
// How soon to look again when a frame had nothing new to show.
const IDLE_REDRAW: Duration = Duration::from_millis(16);

type OutputManager =
    DrmOutputManager<GbmAllocator<DrmDeviceFd>, GbmFramebufferExporter<DrmDeviceFd>, (), DrmDeviceFd>;
type Surface = DrmOutput<GbmAllocator<DrmDeviceFd>, GbmFramebufferExporter<DrmDeviceFd>, (), DrmDeviceFd>;

smithay::backend::renderer::element::render_elements! {
    OutputElement<=GlesRenderer>;
    Space=SpaceRenderElements<GlesRenderer, WaylandSurfaceRenderElement<GlesRenderer>>,
    Cursor=SolidColorRenderElement,
}

struct Udev {
    renderer: GlesRenderer,
    manager: OutputManager,
    surfaces: HashMap<crtc::Handle, (Surface, Output)>,
    // shortcut: a plain square, whatever cursor the client asks for; drawing the
    // client's cursor surface or a themed one replaces this.
    cursor_id: Id,
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
        let drm_mode = *connector
            .modes()
            .iter()
            .find(|mode| mode.mode_type().contains(ModeTypeFlags::PREFERRED))
            .or(connector.modes().first())
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
            .initialize_output::<GlesRenderer, OutputElement>(
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
        cursor_id: Id::new(),
    }));

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

    // The session has to outlive the event loop, which owns everything else.
    std::mem::forget(session);

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
        cursor_id,
        ..
    } = &mut *guard;
    let Some((surface, output)) = surfaces.get_mut(&crtc) else {
        return;
    };

    let queued = draw(state, renderer, surface, output, cursor_id).unwrap_or_else(|err| {
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
    cursor_id: &Id,
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
    let cursor = (pointer - output_origin.to_f64()).to_physical(1.0).to_i32_round();

    // Front to back: the cursor is above every window.
    let mut elements = vec![OutputElement::Cursor(SolidColorRenderElement::new(
        cursor_id.clone(),
        Rectangle::new(cursor, (CURSOR_SIZE, CURSOR_SIZE).into()),
        CommitCounter::default(),
        CURSOR_COLOR,
        Kind::Unspecified,
    ))];
    elements.extend(
        space_render_elements(renderer, [&state.space], output, 1.0)?
            .into_iter()
            .map(OutputElement::Space),
    );

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
