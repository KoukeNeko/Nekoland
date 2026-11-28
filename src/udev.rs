//! Runs the compositor on a display of its own, through DRM/KMS, with input from libinput.
//!
//! shortcut: one GPU; displays on a second one need per-device state as Smithay's anvil has.

use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap, HashSet},
    rc::Rc,
    sync::Mutex,
    time::{Duration, Instant},
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
            exporter::gbm::{GbmFramebufferExporter, NodeFilter},
            output::{DrmOutput, DrmOutputManager, DrmOutputRenderElements},
            DrmDevice, DrmDeviceFd, DrmEvent, DrmNode, Framebuffer, NodeType, VrrSupport,
        },
        egl::{EGLContext, EGLDisplay},
        libinput::{LibinputInputBackend, LibinputSessionInterface},
        renderer::{
            element::{
                memory::MemoryRenderBufferRenderElement, surface::render_elements_from_surface_tree,
                utils::RescaleRenderElement, Kind,
            },
            gles::GlesRenderer,
            utils::with_renderer_surface_state,
            ImportDma, ImportMemWl,
        },
        session::{libseat::{LibSeatSession, LibSeatSessionNotifier}, Event as SessionEvent, Session},
        udev::{all_gpus, primary_gpu, UdevBackend, UdevEvent},
    },
    input::pointer::{CursorIcon, CursorImageAttributes, CursorImageStatus},
    output::{Mode, Output, PhysicalProperties, Scale},
    reexports::wayland_server::{backend::GlobalId, protocol::wl_surface::WlSurface, Resource},
    reexports::{
        calloop::{
            ping::make_ping,
            timer::{TimeoutAction, Timer},
            EventLoop, LoopHandle,
        },
        drm::control::{connector, crtc, Device as _, Mode as DrmMode, ModeFlags, ModeTypeFlags},
        input::{Libinput, SendEventsMode},
        rustix::fs::OFlags,
    },
    utils::{DeviceFd, IsAlive, Logical, Physical, Point, Size},
    wayland::{
        compositor::with_states,
        dmabuf::DmabufFeedbackBuilder,
        drm_syncobj::{supports_syncobj_eventfd, DrmSyncobjState},
    },
    xwayland::XWaylandClientData,
};
use smithay_drm_extras::drm_scanner::{DrmScanEvent, DrmScanner};
use tracing::{info, warn};

use crate::{
    config::{Config, Projection, Rotation},
    cursor::Cursors,
    glass::Glass,
    render::{
        dnd_icon_elements, scene_elements, zoomed, OutputElement, SceneElement, SurfaceElement, CLEAR_COLOR,
    },
    Nekoland,
};

// How soon to look again when a frame had nothing new to show.
const IDLE_REDRAW: Duration = Duration::from_millis(16);
/// How long nothing new has to be drawn before the outputs are looked at less often, every
/// `QUIET_REDRAW`: input, clients' frames, IPC and D-Bus draw at once, so what waits for the
/// next look is what the compositor finds by itself, as the clock does.
// Well under a second, for the outputs to be quiet between the ticks of a clock showing seconds.
const QUIET_AFTER: Duration = Duration::from_millis(100);
const QUIET_REDRAW: Duration = Duration::from_millis(100);

type OutputManager =
    DrmOutputManager<GbmAllocator<DrmDeviceFd>, GbmFramebufferExporter<DrmDeviceFd>, (), DrmDeviceFd>;
type Surface = DrmOutput<GbmAllocator<DrmDeviceFd>, GbmFramebufferExporter<DrmDeviceFd>, (), DrmDeviceFd>;

/// A display's connector, its modes and its size in millimetres.
type Display = (connector::Handle, Vec<DrmMode>, (u32, u32));

struct Udev {
    renderer: GlesRenderer,
    manager: OutputManager,
    surfaces: HashMap<crtc::Handle, (Surface, Output)>,
    cursors: Cursors,
    /// Each output's, as what it blurs is what that output shows; none when the GPU could
    /// not build the glass shaders, or draws too slowly with them.
    glass: HashMap<crtc::Handle, Glass>,
    /// How many frames in a row took longer than `SLOW_GLASS_FRAME` to draw with glass, and
    /// whether that went on long enough to draw without it from then on.
    slow_glass_frames: u32,
    glass_too_slow: bool,
    /// The outputs turned off while the screens are, to be drawn afresh when they come back.
    dark: HashSet<crtc::Handle>,
    /// The outputs with a frame on its way to the display, drawn next at its vblank.
    pending: HashSet<crtc::Handle>,
    /// The outputs with a timer set to look again for something new to draw.
    retrying: HashSet<crtc::Handle>,
    /// When each output last had nothing new to draw, and whether a draw is held back until
    /// a refresh after it.
    empty: HashMap<crtc::Handle, (Instant, bool)>,
    /// When a frame last had something new, for the outputs to be looked at less often after.
    last_drawn: Instant,
    /// The outputs whose frame on its way was drawn locked, for sleep to wait for.
    locked_pending: HashSet<crtc::Handle>,
    /// Screen captures being read back from the GPU, copied to their clients at the next draw.
    readings: Vec<crate::screencopy::Reading>,
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
    /// Each output's connector, modes and size in millimetres, to choose from again as the
    /// settings or the modes change.
    displays: HashMap<crtc::Handle, Display>,
    /// Each output's wl_output, taken away with it when its display is unplugged.
    globals: HashMap<crtc::Handle, GlobalId>,
    /// The connectors seen so far, to tell which were plugged in or out since.
    scanner: DrmScanner,
    /// The `mode`, `scale` and `rotation` settings the outputs were last set up with.
    applied: DisplaySettings,
    /// While it is open, the power button and the lid are Nekoland's.
    _power_keys: Option<zbus::zvariant::OwnedFd>,
}

type SharedUdev = Rc<RefCell<Udev>>;

/// The framebuffer formats tried, in order, for 8 bits a colour and for 10.
const EIGHT_BIT: [Fourcc; 2] = [Fourcc::Abgr8888, Fourcc::Argb8888];
const TEN_BIT: [Fourcc; 2] = [Fourcc::Abgr2101010, Fourcc::Argb2101010];

/// What a display and the card can do beyond a mode, for Settings to offer: kept on its output.
#[derive(Clone, Copy, Debug)]
pub struct Abilities {
    pub ten_bit: bool,
    pub variable_refresh: bool,
}

pub fn init_udev(
    event_loop: &mut EventLoop<'static, Nekoland>,
    data: &mut Nekoland,
) -> Result<(), Box<dyn std::error::Error>> {
    let handle = event_loop.handle();
    let state = &mut *data;

    let mut access = DeviceAccess::new()?;
    let seat = access.seat();

    let gpu_path = primary_gpu(&seat)?
        .or_else(|| all_gpus(&seat).ok()?.into_iter().next())
        .ok_or("no GPU found")?;
    info!("Using {}", gpu_path.display());

    let fd = DrmDeviceFd::new(DeviceFd::from(access.open(&gpu_path)?));
    if supports_syncobj_eventfd(&fd) {
        state.syncobj_state = Some(DrmSyncobjState::new::<Nekoland>(&state.display_handle, fd.clone()));
    }
    let (drm, drm_notifier) = DrmDevice::new(fd.clone(), true)?;
    let gbm = GbmDevice::new(fd)?;

    let egl_display = unsafe { EGLDisplay::new(gbm.clone())? };
    let renderer = unsafe { GlesRenderer::new(EGLContext::new(&egl_display)?)? };
    let render_formats = renderer.egl_context().dmabuf_render_formats().clone();

    let manager: OutputManager = DrmOutputManager::new(
        drm,
        GbmAllocator::new(gbm.clone(), GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT),
        GbmFramebufferExporter::new(gbm.clone(), NodeFilter::None),
        Some(gbm),
        EIGHT_BIT,
        render_formats,
    );

    let power_keys = crate::power::take_power_keys()
        .inspect_err(|err| warn!("logind keeps the power button and the lid: {err}"))
        .ok();
    let udev: SharedUdev = Rc::new(RefCell::new(Udev {
        dark: HashSet::new(),
        pending: HashSet::new(),
        retrying: HashSet::new(),
        empty: HashMap::new(),
        last_drawn: Instant::now(),
        locked_pending: HashSet::new(),
        readings: Vec::new(),
        displays: HashMap::new(),
        globals: HashMap::new(),
        scanner: DrmScanner::new(),
        applied: DisplaySettings::of(&state.config, state.stream_mode),
        gamma: HashMap::new(),
        ramps: HashMap::new(),
        filters: HashMap::new(),
        profile: (None, None),
        _power_keys: power_keys,
        glass: HashMap::new(),
        slow_glass_frames: 0,
        glass_too_slow: false,
        renderer,
        manager,
        surfaces: HashMap::new(),
        cursors: Cursors::load(&state.config.cursor_theme),
    }));
    rescan(&mut udev.borrow_mut(), state)?;
    if udev.borrow().surfaces.is_empty() {
        return Err("no connected display".into());
    }

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
            .create_global_with_default_feedback::<Nekoland>(&state.display_handle, &feedback);
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

    match access {
        DeviceAccess::Seat(session, session_notifier) => {
            watch_seat(&handle, session, session_notifier, &seat, &udev, data)?;
            watch_udev_displays(&handle, &seat, &udev, &gpu_path)?;
        }
        DeviceAccess::Direct => {
            crate::evdev::watch(&handle)?;
            watch_kernel_uevents(&handle, &udev, &gpu_path)?;
        }
    }
    let state = &mut *data;

    {
        let udev = udev.clone();
        let loop_handle = handle.clone();
        handle.insert_source(drm_notifier, move |event, _, data| match event {
            DrmEvent::VBlank(crtc) => {
                udev.borrow_mut().pending.remove(&crtc);
                if udev.borrow_mut().locked_pending.remove(&crtc) {
                    let output = udev.borrow().surfaces.get(&crtc).map(|(_, output)| output.clone());
                    if let Some(output) = output {
                        data.locked_frame_shown(&output);
                    }
                }
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

    std::env::set_var("WAYLAND_DISPLAY", &state.socket_name);

    // Input and clients' new frames draw at once, rather than when the output is next looked
    // at, up to `IDLE_REDRAW` later: two refreshes at 120 Hz before anything reacts.
    {
        let udev = udev.clone();
        let loop_handle = handle.clone();
        let (ping, source) = make_ping()?;
        handle.insert_source(source, move |_, _, data| render_all(&udev, data, &loop_handle))?;
        crate::text::redraw_on_decoded_icons(ping.clone());
        data.redraw = Some(ping);
    }

    let loop_handle = handle.clone();
    handle.insert_idle(move |data| render_all(&udev, data, &loop_handle));

    Ok(())
}

/// Where udev runs, its daemon listens here.
const UDEV_CONTROL_SOCKET: &str = "/run/udev/control";
const SEATLESS_SEAT: &str = "seat0";
const DEVICE_FLAGS: OFlags = OFlags::RDWR
    .union(OFlags::CLOEXEC)
    .union(OFlags::NOCTTY)
    .union(OFlags::NONBLOCK);

/// How the GPU and the input devices are opened.
enum DeviceAccess {
    /// Through logind or seatd, which pause the session for another on a VT switch.
    Seat(LibSeatSession, LibSeatSessionNotifier),
    /// Straight, as the user may (its groups video and input), where udev does not run and
    /// so neither does a seat manager, nor anything libinput would take: unkernel's guest,
    /// Linux built to WebAssembly, which has no VTs either.
    Direct,
}

impl DeviceAccess {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        if !std::path::Path::new(UDEV_CONTROL_SOCKET).exists() {
            info!("no udev: opening the devices directly");
            return Ok(Self::Direct);
        }
        let (session, notifier) = LibSeatSession::new()?;
        Ok(Self::Seat(session, notifier))
    }

    fn seat(&self) -> String {
        match self {
            Self::Seat(session, _) => session.seat(),
            Self::Direct => SEATLESS_SEAT.into(),
        }
    }

    fn open(&mut self, path: &std::path::Path) -> Result<std::os::fd::OwnedFd, Box<dyn std::error::Error>> {
        match self {
            Self::Seat(session, _) => Ok(session.open(path, DEVICE_FLAGS)?),
            Self::Direct => Ok(smithay::reexports::rustix::fs::open(path, DEVICE_FLAGS, smithay::reexports::rustix::fs::Mode::empty())?),
        }
    }
}

/// Looks again at the displays and their modes once the GPU says they changed: displays are
/// plugged in and out, and their modes change under them, as QEMU's and unkernel's follow the
/// size of their window.
fn displays_changed(udev: &SharedUdev, data: &mut Nekoland) {
    let mut backend = udev.borrow_mut();
    if let Err(err) = rescan(&mut backend, data) {
        warn!("could not look for displays: {err}");
    }
    reprobe_modes(&mut backend, data);
}

/// Hears of the GPU's changes from udevd.
fn watch_udev_displays(
    handle: &LoopHandle<'static, Nekoland>,
    seat: &str,
    udev: &SharedUdev,
    gpu_path: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let udev = udev.clone();
    let device = DrmNode::from_path(gpu_path)?.dev_id();
    handle.insert_source(UdevBackend::new(seat)?, move |event, _, data| {
        if matches!(event, UdevEvent::Changed { device_id } if device_id == device) {
            displays_changed(&udev, data);
        }
    })?;
    Ok(())
}

/// Hears of the GPU's changes and input devices plugged in from the kernel itself, where no
/// udevd passes them on.
fn watch_kernel_uevents(
    handle: &LoopHandle<'static, Nekoland>,
    udev: &SharedUdev,
    gpu_path: &std::path::Path,
) -> std::io::Result<()> {
    const DRM_SUBSYSTEM: &str = "drm";
    const INPUT_SUBSYSTEM: &str = "input";
    const ADDED: &str = "add";
    const CHANGED: &str = "change";
    let udev = udev.clone();
    // Uevents name a device by its node under /dev.
    const DEVICE_DIRECTORY: &str = "/dev";
    let gpu_devname = gpu_path.strip_prefix(DEVICE_DIRECTORY).unwrap_or(gpu_path).display().to_string();
    let loop_handle = handle.clone();
    crate::uevent::watch(handle, move |event, data| match (event.subsystem.as_str(), event.action.as_str()) {
        (DRM_SUBSYSTEM, CHANGED) if event.hotplug && event.devname == gpu_devname => {
            displays_changed(&udev, data);
        }
        (INPUT_SUBSYSTEM, ADDED) => crate::evdev::watch_devname(&loop_handle, &event.devname),
        _ => {}
    })
}

/// Takes input from libinput on the seat, and gives the displays and input up while another
/// session has the seat.
fn watch_seat(
    handle: &LoopHandle<'static, Nekoland>,
    mut session: LibSeatSession,
    notifier: LibSeatSessionNotifier,
    seat: &str,
    udev: &SharedUdev,
    data: &mut Nekoland,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut libinput =
        Libinput::new_with_udev::<LibinputSessionInterface<LibSeatSession>>(session.clone().into());
    libinput
        .udev_assign_seat(seat)
        .map_err(|()| "libinput could not take the seat")?;
    handle.insert_source(LibinputInputBackend::new(libinput.clone()), |event, _, data| {
        if let InputEvent::DeviceAdded { device } = &event {
            // Sunshine's mouse and keyboard belong to the session it streams.
            if crate::headless::is_sunshine_device(&device.name()) && crate::headless::streams_own_session() {
                if let Err(err) = device.clone().config_send_events_set_mode(SendEventsMode::DISABLED) {
                    warn!("could not leave {} to the remote session: {err:?}", device.name());
                }
                return;
            }
            data.add_pointer_device(device.clone());
        }
        data.process_input_event(event)
    })?;

    let udev = udev.clone();
    let loop_handle = handle.clone();
    handle.insert_source(notifier, move |event, _, data| match event {
        SessionEvent::PauseSession => {
            libinput.suspend();
            udev.borrow_mut().manager.pause();
        }
        SessionEvent::ActivateSession => {
            if libinput.resume().is_err() {
                warn!("libinput did not resume");
            }
            let mut backend = udev.borrow_mut();
            if let Err(err) = backend.manager.lock().activate(false) {
                warn!("could not take the display back: {err}");
            }
            // Whoever had the display meanwhile may have left ramps of their own.
            backend.ramps.clear();
            backend.filters.clear();
            backend.pending.clear();
            drop(backend);
            render_all(&udev, data, &loop_handle);
        }
    })?;

    // The state keeps the session alive, which has to outlive everything opened through it.
    data.switch_vt = Some(Box::new(move |vt| {
        if let Err(err) = session.change_vt(vt) {
            warn!("could not switch to VT {vt}: {err}");
        }
    }));
    Ok(())
}

fn render_all(udev: &SharedUdev, data: &mut Nekoland, handle: &LoopHandle<'static, Nekoland>) {
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
    data: &mut Nekoland,
    handle: &LoopHandle<'static, Nekoland>,
) {
    // Its vblank draws it next, or the timer holding it back.
    let held = udev.borrow().empty.get(&crtc).is_some_and(|(_, held)| *held);
    if held || udev.borrow().pending.contains(&crtc) {
        return;
    }
    // After a frame with nothing new, the next waits a refresh, as a shown frame waits for
    // its vblank: a program shown but drawing nothing new, as one asking for frames without
    // damage, is told at once that its frame was shown and would otherwise have everything
    // drawn again as fast as it can. Not after an idle look, which no program asked for.
    if let Some(wait) = hold_after_empty(udev, crtc) {
        let timed = udev.clone();
        let loop_handle = handle.clone();
        if let Err(err) = handle.insert_source(Timer::from_duration(wait), move |_, _, data| {
            if let Some((_, held)) = timed.borrow_mut().empty.get_mut(&crtc) {
                *held = false;
            }
            render(&timed, crtc, data, &loop_handle);
            TimeoutAction::Drop
        }) {
            warn!("could not schedule a redraw: {err}");
            udev.borrow_mut().empty.remove(&crtc);
        }
        return;
    }
    let state = &mut *data;
    state.prepare_frame();
    let mut guard = udev.borrow_mut();
    if !guard.applied.holds(&state.config, state.stream_mode) {
        apply_display_settings(&mut guard, state);
    }
    let Udev {
        renderer,
        manager,
        surfaces,
        cursors,
        glass,
        slow_glass_frames,
        glass_too_slow,
        dark,
        gamma,
        ramps,
        filters,
        profile,
        readings,
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
    let applied = filters.get(&crtc).map_or(Some("none"), Option::as_deref);
    if applied.is_some_and(|applied| applied != filter) {
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

    // Read back by now, at a refresh or more since they were drawn.
    crate::screencopy::finish(renderer, std::mem::take(readings));

    // A dark output is still looked at every `IDLE_REDRAW`, to come back on at once.
    let queued = if state.screens_off || state.space.output_geometry(output).is_none() {
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
        let started = Instant::now();
        let with_glass = glass.contains_key(&crtc);
        let queued = draw(state, renderer, surface, output, cursors, glass.get_mut(&crtc), frames, readings).unwrap_or_else(|err| {
            warn!("render: {err}");
            false
        });
        if queued && with_glass {
            judge_glass_speed(glass, slow_glass_frames, glass_too_slow, started.elapsed());
        }
        readings.extend(state.capture_windows(renderer));
        queued
    };
    let output = output.clone();
    // Captures read back are copied at the next draw, which comes soon for them.
    let idle = if guard.last_drawn.elapsed() < QUIET_AFTER || !guard.readings.is_empty() {
        IDLE_REDRAW
    } else {
        QUIET_REDRAW
    };
    let retry = if queued {
        guard.last_drawn = Instant::now();
        guard.pending.insert(crtc);
        if state.unshown_locked.is_some() && state.lock.is_some() {
            guard.locked_pending.insert(crtc);
        }
        guard.empty.remove(&crtc);
        false
    } else {
        guard.empty.insert(crtc, (Instant::now(), false));
        guard.retrying.insert(crtc)
    };
    drop(guard);
    if queued {
        state.count_frame();
    }

    // One timer an output at most, however often it is drawn in between.
    if retry {
        let timed = udev.clone();
        let loop_handle = handle.clone();
        let timer = Timer::from_duration(idle);
        if let Err(err) = handle.insert_source(timer, move |_, _, data| {
            timed.borrow_mut().retrying.remove(&crtc);
            render(&timed, crtc, data, &loop_handle);
            // An idle look that drew nothing new holds back no program's next frame, which
            // would otherwise reach the screen a refresh late; one held back stays held.
            let mut guard = timed.borrow_mut();
            if guard.empty.get(&crtc).is_some_and(|(_, held)| !*held) {
                guard.empty.remove(&crtc);
            }
            TimeoutAction::Drop
        }) {
            warn!("could not schedule a redraw: {err}");
            udev.borrow_mut().retrying.remove(&crtc);
        }
    }

    state.finish_frame(&output);
    let _ = data.display_handle.flush_clients();
}

/// A frame drawn with glass slower than this, under seven a second, is too slow for it.
const SLOW_GLASS_FRAME: Duration = Duration::from_millis(150);
/// How many such frames in a row turn the glass off; one alone may be the first, which
/// builds the shaders.
const SLOW_GLASS_FRAMES: u32 = 3;

/// Turns the glass off for good once frames drawn with it keep being too slow, as where Mesa
/// draws on a slow CPU: unkernel's guest takes half a second over its blur, holding back the
/// input read after it, so that a key let go is seen long after it was and repeats.
fn judge_glass_speed(
    glass: &mut HashMap<crtc::Handle, Glass>,
    slow_frames: &mut u32,
    too_slow: &mut bool,
    took: Duration,
) {
    if took <= SLOW_GLASS_FRAME {
        *slow_frames = 0;
        return;
    }
    *slow_frames += 1;
    if *slow_frames >= SLOW_GLASS_FRAMES {
        warn!("frames take {} ms with glass: drawing without it", took.as_millis());
        glass.clear();
        *too_slow = true;
    }
}

/// How long a draw of `crtc` waits, when its last frame had nothing new and a refresh has not
/// passed since; None when it draws now.
fn hold_after_empty(udev: &SharedUdev, crtc: crtc::Handle) -> Option<Duration> {
    let mut guard = udev.borrow_mut();
    let refresh = guard
        .surfaces
        .get(&crtc)
        .and_then(|(_, output)| output.current_mode())
        .map(|mode| mode.refresh)
        .filter(|millihertz| *millihertz > 0)
        .map_or(IDLE_REDRAW, |millihertz| Duration::from_secs_f64(1000.0 / f64::from(millihertz)));
    let (at, held) = guard.empty.get_mut(&crtc)?;
    let wait = refresh.checked_sub(at.elapsed())?;
    *held = true;
    Some(wait)
}

/// Sets up the displays plugged in since the last look, and lets go of those unplugged.
fn rescan(udev: &mut Udev, state: &mut Nekoland) -> std::io::Result<()> {
    let mut changed = false;
    for event in udev.scanner.scan_connectors(udev.manager.device())? {
        match event {
            DrmScanEvent::Connected {
                connector,
                crtc: Some(crtc),
            } => match connect(udev, state, &connector, crtc) {
                Ok(()) => changed = true,
                Err(err) => warn!("{err}"),
            },
            DrmScanEvent::Disconnected { crtc: Some(crtc), .. } => {
                disconnect(udev, state, crtc);
                changed = true;
            }
            _ => {}
        }
    }
    if changed {
        state.night_shift_available = !udev.gamma.is_empty();
        lay_out_outputs(udev, state);
        state.tell_surfaces_the_scale();
        state.dock.drawn = None;
        state.menu_bar.drawn = None;
        state.fit_windows_to_display();
    }
    Ok(())
}

/// Sets a display up at the mode and scale the settings ask for, to the right of the others.
fn connect(udev: &mut Udev, state: &mut Nekoland, connector: &connector::Info, crtc: crtc::Handle) -> Result<(), String> {
    let name = format!("{}-{}", connector.interface().as_str(), connector.interface_id());
    let modes = connector.modes();
    let drm_mode = choose_mode(&name, modes, &state.config, None).ok_or(format!("{name} has no modes"))?;
    let mode = Mode::from(drm_mode);
    let (width_mm, height_mm) = connector.size().unwrap_or((0, 0));

    let output = Output::new(
        name.clone(),
        PhysicalProperties {
            size: (width_mm as i32, height_mm as i32).into(),
            subpixel: connector.subpixel().into(),
            make: "Unknown".into(),
            model: "Unknown".into(),
            serial_number: "Unknown".into(),
        },
    );
    for mode in modes {
        output.add_mode(Mode::from(*mode));
    }

    let scale = chosen_scale(&state.config, &name, mode, (width_mm, height_mm));
    info!("{name}: scale {scale}");
    // What Settings offers as the default, whichever mode the settings set now.
    output.set_preferred(preferred_mode(modes).map_or(mode, Mode::from));
    // Where it goes, and whether it is shown at all, is the layout's.
    output.change_current_state(
        Some(mode),
        Some(state.config.rotation_of(&name).transform()),
        Some(Scale::Fractional(scale)),
        None,
    );

    let surface = udev
        .manager
        .lock()
        .initialize_output::<GlesRenderer, SceneElement>(
            crtc,
            drm_mode,
            &[connector.handle()],
            &output,
            None,
            &mut udev.renderer,
            &DrmOutputRenderElements::default(),
        )
        .map_err(|err| format!("could not set up {name}: {err}"))?;
    info!("{name}: {}x{}", mode.size.w, mode.size.h);
    match udev.manager.device().get_crtc(crtc) {
        Ok(info) if info.gamma_length() > 1 => {
            udev.gamma.insert(crtc, info.gamma_length());
        }
        Ok(_) => {}
        Err(err) => warn!("{name}: no gamma: {err}"),
    }
    if !udev.glass_too_slow {
        match Glass::new(&mut udev.renderer) {
            Ok(glass) => {
                udev.glass.insert(crtc, glass);
            }
            Err(err) => warn!("no glass: {err}"),
        }
    }
    let render_formats = udev.renderer.egl_context().dmabuf_render_formats();
    let ten_bit = surface.with_compositor(|compositor| {
        compositor.surface().planes().primary.iter().any(|plane| {
            TEN_BIT.iter().any(|code| {
                plane.formats.iter().any(|format| format.code == *code)
                    && render_formats.iter().any(|format| format.code == *code)
            })
        })
    });
    let variable_refresh = surface
        .with_compositor(|compositor| compositor.vrr_supported(connector.handle()))
        .is_ok_and(|support| support != VrrSupport::NotSupported);
    output.user_data().insert_if_missing(|| Abilities { ten_bit, variable_refresh });
    udev.surfaces.insert(crtc, (surface, output));
    udev.displays.insert(crtc, (connector.handle(), modes.to_vec(), (width_mm, height_mm)));
    apply_colour_depth_and_refresh(udev, &state.config, crtc);
    Ok(())
}

/// Sends a display its colours at the depth the settings ask for, and lets its refresh follow
/// the frames if they ask for it and it can; a depth the display or the card cannot take is
/// left as it was.
fn apply_colour_depth_and_refresh(udev: &mut Udev, config: &Config, crtc: crtc::Handle) {
    let (Some((surface, output)), Some((connector, _, _))) = (udev.surfaces.get(&crtc), udev.displays.get(&crtc)) else {
        return;
    };
    let name = output.name();
    let depth = config.color_depth_of(&name);
    let wanted = if depth >= 10 { TEN_BIT } else { EIGHT_BIT };
    let render_formats = udev.renderer.egl_context().dmabuf_render_formats();
    let allocator = udev.manager.allocator().clone();
    let variable_refresh = config.variable_refresh_of(&name);
    surface.with_compositor(|compositor| {
        if !wanted.contains(&compositor.format()) {
            let set = wanted.iter().find_map(|code| {
                let modifiers = render_formats.iter().filter(|format| format.code == *code).map(|format| format.modifier);
                compositor
                    .set_format(allocator.clone(), *code, modifiers)
                    .inspect_err(|err| warn!("{name}: no {code} framebuffer: {err}"))
                    .ok()
                    .map(|()| *code)
            });
            match set {
                Some(code) => info!("{name}: {depth}-bit colour ({code})"),
                None => warn!("{name}: kept {} for {depth}-bit colour", compositor.format()),
            }
        }
        let supported = compositor.vrr_supported(*connector).is_ok_and(|support| support != VrrSupport::NotSupported);
        if supported && compositor.vrr_enabled() != variable_refresh {
            if let Err(err) = compositor.use_vrr(variable_refresh) {
                warn!("{name}: could not set variable refresh: {err}");
            }
        }
    });
    // Intel's and AMD's links carry no more bits than this lets them, whatever the framebuffer
    // holds; NVIDIA's has no such property and follows the framebuffer.
    if let Err(err) = set_max_bpc(udev.manager.device(), *connector, depth) {
        tracing::debug!("{name}: max bpc: {err}");
    }
}

/// Sets a connector's `max bpc`, held to the range it takes; an error for one that has none.
fn set_max_bpc(
    device: &impl smithay::reexports::drm::control::Device,
    connector: connector::Handle,
    depth: u32,
) -> Result<(), String> {
    use smithay::reexports::drm::control::property::ValueType;
    let properties = device.get_properties(connector).map_err(|err| err.to_string())?;
    let (handles, values) = properties.as_props_and_values();
    let (handle, info, current) = handles
        .iter()
        .zip(values)
        .find_map(|(handle, value)| {
            let info = device.get_property(*handle).ok()?;
            (info.name().to_bytes() == b"max bpc").then_some((*handle, info, *value))
        })
        .ok_or("no max bpc property")?;
    let wanted = match info.value_type() {
        ValueType::UnsignedRange(least, most) => u64::from(depth).clamp(least, most),
        _ => u64::from(depth),
    };
    if wanted == current {
        return Ok(());
    }
    device.set_property(connector, handle, wanted).map_err(|err| err.to_string())
}

/// Lets go of an unplugged display; its windows are fitted to the displays left.
fn disconnect(udev: &mut Udev, state: &mut Nekoland, crtc: crtc::Handle) {
    if let Some((_, output)) = udev.surfaces.remove(&crtc) {
        info!("{}: unplugged", output.name());
        state.space.unmap_output(&output);
        state.connected_outputs.retain(|connected| *connected != output);
    }
    if let Some(global) = udev.globals.remove(&crtc) {
        state.display_handle.remove_global::<Nekoland>(global);
    }
    udev.displays.remove(&crtc);
    udev.glass.remove(&crtc);
    udev.gamma.remove(&crtc);
    udev.ramps.remove(&crtc);
    udev.filters.remove(&crtc);
    udev.dark.remove(&crtc);
    udev.pending.remove(&crtc);
}

/// The mode the settings ask for, at its fastest rate unless they name one; the display's
/// preferred mode otherwise. While a Sunshine client streams, `stream` is its size and rate,
/// and the mode nearest it is chosen instead.
fn choose_mode(
    name: &str,
    modes: &[DrmMode],
    config: &Config,
    stream: Option<((u16, u16), Option<u32>)>,
) -> Option<DrmMode> {
    if let Some((wanted, rate)) = stream {
        let sizes: Vec<(u16, u16)> = modes.iter().map(DrmMode::size).collect();
        let size = nearest_size(&sizes, wanted)?;
        let of_size = || modes.iter().filter(move |mode| mode.size() == size);
        // As fast as the client shows, else as fast as the display goes.
        return of_size()
            .filter(|mode| rate.is_none_or(|rate| mode.vrefresh() <= rate))
            .max_by_key(|mode| mode.vrefresh())
            .or_else(|| of_size().max_by_key(|mode| mode.vrefresh()))
            .copied();
    }
    let configured = config.wanted_mode(name).and_then(|(size, rate)| {
        let found = modes
            .iter()
            .filter(|mode| mode.size() == size && rate.is_none_or(|rate| mode.vrefresh() == rate))
            .max_by_key(|mode| mode.vrefresh());
        if found.is_none() {
            warn!("{name} has no {} mode", config.mode.as_deref().unwrap_or_default());
        }
        found
    });
    configured.copied().or_else(|| preferred_mode(modes))
}

/// The size of a display's nearest to a client's screen: its own, else the smallest that holds
/// it, which Sunshine scales down sharply, else the largest the display has.
fn nearest_size(sizes: &[(u16, u16)], wanted: (u16, u16)) -> Option<(u16, u16)> {
    let area = |(width, height): (u16, u16)| u32::from(width) * u32::from(height);
    sizes
        .iter()
        .copied()
        .filter(|&(width, height)| width >= wanted.0 && height >= wanted.1)
        .min_by_key(|&size| area(size))
        .or_else(|| sizes.iter().copied().max_by_key(|&size| area(size)))
}

/// The display's preferred size at its fastest rate: a display's EDID prefers 60 Hz even
/// where it runs at 120 or more, and every animation then gets half its frames.
fn preferred_mode(modes: &[DrmMode]) -> Option<DrmMode> {
    let preferred = modes.iter().find(|mode| mode.mode_type().contains(ModeTypeFlags::PREFERRED));
    preferred
        .and_then(|preferred| {
            modes
                .iter()
                .filter(|mode| mode.size() == preferred.size() && !mode.flags().contains(ModeFlags::INTERLACE))
                .max_by_key(|mode| mode.vrefresh())
        })
        .or(preferred)
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
struct DisplaySettings {
    stream: Option<((u16, u16), Option<u32>)>,
    mode: Option<String>,
    display_modes: BTreeMap<String, String>,
    scale: Option<f64>,
    display_scales: BTreeMap<String, f64>,
    rotation: Rotation,
    display_rotations: BTreeMap<String, Rotation>,
    color_depth: u32,
    display_color_depths: BTreeMap<String, u32>,
    variable_refresh: bool,
    display_variable_refresh: BTreeMap<String, bool>,
    display_positions: BTreeMap<String, [i32; 2]>,
    display_order: Vec<String>,
    projection: Projection,
}

impl DisplaySettings {
    fn of(config: &Config, stream: Option<((u16, u16), Option<u32>)>) -> Self {
        Self {
            stream,
            mode: config.mode.clone(),
            display_modes: config.display_modes.clone(),
            scale: config.scale,
            display_scales: config.display_scales.clone(),
            rotation: config.rotation,
            display_rotations: config.display_rotations.clone(),
            color_depth: config.color_depth,
            display_color_depths: config.display_color_depths.clone(),
            variable_refresh: config.variable_refresh,
            display_variable_refresh: config.display_variable_refresh.clone(),
            display_positions: config.display_positions.clone(),
            display_order: config.display_order.clone(),
            projection: config.projection(),
        }
    }

    /// Whether the settings are still these, compared in place: it is asked at every frame.
    fn holds(&self, config: &Config, stream: Option<((u16, u16), Option<u32>)>) -> bool {
        self.stream == stream
            && self.mode == config.mode
            && self.display_modes == config.display_modes
            && self.scale == config.scale
            && self.display_scales == config.display_scales
            && self.rotation == config.rotation
            && self.display_rotations == config.display_rotations
            && self.color_depth == config.color_depth
            && self.display_color_depths == config.display_color_depths
            && self.variable_refresh == config.variable_refresh
            && self.display_variable_refresh == config.display_variable_refresh
            && self.display_positions == config.display_positions
            && self.display_order == config.display_order
            && self.projection == config.projection()
    }
}

/// An output's size in points, as its mode, scale and rotation make it, mapped or not.
pub fn logical_size(output: &Output) -> Size<i32, Logical> {
    let mode = output.current_mode().map_or_else(Size::default, |mode| mode.size);
    let size = output.current_transform().transform_size(mode);
    size.to_f64().to_logical(output.current_scale().fractional_scale()).to_i32_round()
}

/// Lights the displays the projection shows and puts them where the settings say, or side by
/// side in the order they name; the first shown is the main display everything else follows.
/// Those it leaves dark are taken off the desktop, and from the programs.
fn lay_out_outputs(udev: &mut Udev, state: &mut Nekoland) {
    let outputs = ordered_outputs(udev, &state.config);
    let projection = state.config.projection();
    let mut places: Vec<(Output, Point<i32, Logical>)> = Vec::new();
    let mut right = 0;
    for (_, output, shown) in &outputs {
        if !shown {
            continue;
        }
        let place: Point<i32, Logical> = match projection {
            Projection::Duplicate => (0, 0).into(),
            _ => state.config.display_positions.get(&output.name()).map_or((right, 0).into(), |&[x, y]| (x, y).into()),
        };
        right = right.max(place.x + logical_size(output).w);
        places.push((output.clone(), place));
    }
    // The desktop starts at the top left of the displays, wherever they were put.
    let origin = places.iter().fold(None, |least: Option<Point<i32, Logical>>, (_, place)| {
        Some(least.map_or(*place, |least| (least.x.min(place.x), least.y.min(place.y)).into()))
    });

    for (_, output, _) in &outputs {
        state.space.unmap_output(output);
    }
    for (crtc, output, _) in &outputs {
        let place = places.iter().find(|(placed, _)| placed == output).map(|(_, place)| *place - origin.unwrap_or_default());
        match place {
            Some(place) => {
                output.change_current_state(None, None, None, Some(place));
                state.space.map_output(output, place);
                udev.globals
                    .entry(*crtc)
                    .or_insert_with(|| output.create_global::<Nekoland>(&state.display_handle));
            }
            None => {
                if let Some(global) = udev.globals.remove(crtc) {
                    state.display_handle.remove_global::<Nekoland>(global);
                }
            }
        }
    }
    state.connected_outputs = outputs.into_iter().map(|(_, output, _)| output).collect();
    // The menu bar's and the Dock's pictures are drawn for the main display.
    if let Some((main, _)) = places.first() {
        crate::text::set_picture_scale(main.current_scale().fractional_scale());
    }
}

/// The outputs in the order the settings give them, each with whether the projection shows
/// it; the first shown is the main display.
fn ordered_outputs(udev: &Udev, config: &Config) -> Vec<(crtc::Handle, Output, bool)> {
    let mut outputs: Vec<(crtc::Handle, Output)> =
        udev.surfaces.iter().map(|(crtc, (_, output))| (*crtc, output.clone())).collect();
    let order = |output: &Output| config.display_order.iter().position(|name| *name == output.name());
    outputs.sort_by_key(|(_, output)| (order(output).unwrap_or(usize::MAX), output.name()));
    let projection = config.projection();
    let count = outputs.len();
    outputs
        .into_iter()
        .enumerate()
        .map(|(index, (crtc, output))| {
            let shown = match projection {
                _ if count < 2 => true,
                Projection::MainOnly => index == 0,
                Projection::SecondOnly => index > 0,
                Projection::Extend | Projection::Duplicate => true,
            };
            (crtc, output, shown)
        })
        .collect()
}

/// Sets every output to the mode, scale and rotation the settings now ask for, lays the
/// outputs out side by side again, and fits the windows to them.
fn apply_display_settings(udev: &mut Udev, state: &mut Nekoland) {
    udev.applied = DisplaySettings::of(&state.config, state.stream_mode);
    // What Sunshine captures is the main display.
    let main = ordered_outputs(udev, &state.config)
        .into_iter()
        .find(|(_, _, shown)| *shown)
        .map(|(crtc, _, _)| crtc);
    for (crtc, (surface, output)) in &mut udev.surfaces {
        let Some((_, modes, size_mm)) = udev.displays.get(crtc) else {
            continue;
        };
        let stream = state.stream_mode.filter(|_| main == Some(*crtc));
        let Some(drm_mode) = choose_mode(&output.name(), modes, &state.config, stream) else {
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
            Some(state.config.rotation_of(&output.name()).transform()),
            Some(Scale::Fractional(scale)),
            None,
        );
        smithay::desktop::layer_map_for_output(output).arrange();
    }
    let crtcs: Vec<crtc::Handle> = udev.surfaces.keys().copied().collect();
    for crtc in crtcs {
        apply_colour_depth_and_refresh(udev, &state.config, crtc);
    }
    lay_out_outputs(udev, state);
    state.tell_surfaces_the_scale();
    state.dock.drawn = None;
    state.menu_bar.drawn = None;
    state.fit_windows_to_display();
}

/// Reads the displays' modes again, and where they changed sets the displays up from the
/// settings again: without a mode set, each takes its new preferred one.
fn reprobe_modes(udev: &mut Udev, state: &mut Nekoland) {
    let mut changed = false;
    for (crtc, (connector, modes, _)) in &mut udev.displays {
        let Ok(info) = udev.manager.device().get_connector(*connector, true) else {
            continue;
        };
        if info.modes().is_empty() || info.modes() == modes.as_slice() {
            continue;
        }
        if let Some((_, output)) = udev.surfaces.get(crtc) {
            for mode in modes.iter() {
                output.delete_mode(Mode::from(*mode));
            }
            for mode in info.modes() {
                output.add_mode(Mode::from(*mode));
            }
            if let Some(preferred) = preferred_mode(info.modes()) {
                output.set_preferred(Mode::from(preferred));
            }
        }
        *modes = info.modes().to_vec();
        changed = true;
    }
    if changed {
        apply_display_settings(udev, state);
    }
}

/// Returns whether a frame was handed to the display, and what it showed of each element.
fn draw(
    state: &Nekoland,
    renderer: &mut GlesRenderer,
    surface: &mut Surface,
    output: &Output,
    cursors: &mut Cursors,
    glass: Option<&mut Glass>,
    screencopy: Vec<crate::screencopy::Frame>,
    readings: &mut Vec<crate::screencopy::Reading>,
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
        readings.extend(crate::screencopy::copy(renderer, output, &elements, cursor, screencopy));
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
pub(crate) fn cursor_elements(
    state: &Nekoland,
    renderer: &mut GlesRenderer,
    cursors: &mut Cursors,
    pointer: Point<i32, Physical>,
    scale: f64,
) -> Vec<SceneElement> {
    if cursors.asked != state.config.cursor_theme {
        *cursors = Cursors::load(&state.config.cursor_theme);
    }
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
            let larger = x11_picture_scale(state, cursors, surface);
            let elements: Vec<SurfaceElement> =
                render_elements_from_surface_tree(renderer, surface, location, scale, 1.0, Kind::Cursor);
            return elements
                .into_iter()
                .map(|element| {
                    SceneElement::Cursor(RescaleRenderElement::from_element(element, pointer, larger))
                })
                .collect();
        }
        // A client that went away leaves its cursor behind; fall back to the arrow.
        CursorImageStatus::Surface(_) => CursorIcon::Default,
        CursorImageStatus::Named(icon) => *icon,
    };
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

/// How many times larger than its windows an X11 program's cursor is drawn. X11 programs
/// draw at the display's density, and the theme's cursors are given to them at it, so their
/// pixels are shrunk with the windows'. A picture of a program's own no bigger than the
/// theme's arrow at one pixel a point (Wine's and games' 32-pixel cursors, X's 16-pixel ones)
/// was drawn for a display without scaling, and shrunk would be two thirds of the arrow at 1.5.
fn x11_picture_scale(state: &Nekoland, cursors: &mut Cursors, surface: &WlSurface) -> f64 {
    let Some(data) = state
        .xwayland_client
        .as_ref()
        .filter(|xwayland| {
            surface
                .client()
                .is_some_and(|client| client.id() == xwayland.id())
        })
        .and_then(|xwayland| xwayland.get_data::<XWaylandClientData>())
    else {
        return 1.0;
    };
    let picture = with_renderer_surface_state(surface, |surface| surface.buffer_size()).flatten();
    let arrow = cursors
        .get(CursorIcon::Default, 1.0, state.config.cursor_size)
        .map(|arrow| arrow.size);
    match (picture, arrow) {
        (Some(picture), Some(arrow)) if picture.w <= arrow.w && picture.h <= arrow.h => {
            data.compositor_state.client_scale()
        }
        _ => 1.0,
    }
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
    use super::{density_scale, filter_matrix, nearest_size};

    #[test]
    fn a_client_gets_its_own_size_else_the_smallest_that_holds_it() {
        let sizes = [(3840, 2160), (2560, 1440), (1920, 1080), (1680, 1050)];
        assert_eq!(nearest_size(&sizes, (1920, 1080)), Some((1920, 1080)));
        // A 16:10 Mac: no mode of its shape holds it, so the 4K one, scaled down.
        assert_eq!(nearest_size(&sizes, (2560, 1600)), Some((3840, 2160)));
        assert_eq!(nearest_size(&sizes, (1280, 800)), Some((1680, 1050)));
        // Larger than any mode: the largest.
        assert_eq!(nearest_size(&sizes, (5120, 2880)), Some((3840, 2160)));
        assert_eq!(nearest_size(&[], (1920, 1080)), None);
    }

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
