use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};

use smithay::{
    backend::{
        egl::EGLDevice,
        renderer::{damage::OutputDamageTracker, gles::GlesRenderer, ImportDma},
        winit::{self, WinitEvent, WinitGraphicsBackend},
    },
    input::pointer::{CursorIcon, CursorImageStatus},
    output::{Mode, Output, PhysicalProperties, Scale, Subpixel},
    reexports::{
        calloop::{
            ping::make_ping,
            timer::{TimeoutAction, Timer},
            EventLoop,
        },
        winit::{
            dpi::{LogicalSize, PhysicalPosition, PhysicalSize},
            window::Window,
        },
    },
    utils::Transform,
    wayland::dmabuf::DmabufFeedbackBuilder,
};

use crate::{
    glass::Glass,
    render::{dnd_icon_elements, scene_elements, zoomed, CLEAR_COLOR},
    Nekoland,
};

/// How often an xrdp session's window looks whether the screen it covers changed size.
const SCREEN_CHECK: Duration = Duration::from_secs(1);

pub fn init_winit(
    event_loop: &mut EventLoop<Nekoland>,
    data: &mut Nekoland,
) -> Result<(), Box<dyn std::error::Error>> {
    let display_handle = &mut data.display_handle.clone();
    let state = &mut *data;

    let attributes = Window::default_attributes()
        .with_inner_size(LogicalSize::new(1280.0, 800.0))
        .with_title("Nekoland")
        // In an xrdp session there is no window manager: the window is the whole screen.
        .with_decorations(!state.remote)
        .with_visible(true);
    let (backend, winit) = winit::init_from_attributes::<GlesRenderer>(attributes)?;
    if state.remote {
        cover_screen(backend.window());
    }

    let mode = Mode {
        size: backend.window_size(),
        refresh: refresh_rate(backend.window()),
    };
    // Frames come at the pace of the display the window is on: a fixed 60 a second halves
    // every animation on a faster one.
    let frame = Rc::new(Cell::new(frame_time(mode.refresh)));

    let output = Output::new(
        "winit".to_string(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "Smithay".into(),
            model: "Winit".into(),
            serial_number: "Unknown".into(),
        },
    );
    let _global = output.create_global::<Nekoland>(display_handle);
    // As the host scales its own windows, unless the configuration says otherwise.
    let scale = |host: f64| Scale::Fractional(state.config.scale.unwrap_or(host));
    let initial = scale(backend.scale_factor());
    crate::text::set_picture_scale(initial.fractional_scale());
    output.change_current_state(
        Some(mode),
        Some(Transform::Flipped180),
        Some(initial),
        Some((0, 0).into()),
    );
    output.set_preferred(mode);

    state.space.map_output(&output, (0, 0));

    let backend: Rc<RefCell<WinitGraphicsBackend<GlesRenderer>>> = Rc::new(RefCell::new(backend));
    let mut damage_tracker = OutputDamageTracker::from_output(&output);
    // What changed since the frame before, whatever the buffer drawn into last showed: what
    // is sent to the host, so that xrdp encodes no more than that.
    let mut changes = OutputDamageTracker::from_output(&output);
    let mut glass = Glass::new(backend.borrow_mut().renderer())
        .inspect_err(|err| tracing::warn!("no glass: {err}"))
        .ok();
    let mut shown_cursor = Some(CursorIcon::Default);
    let mut knows_age = true;

    // Clients that draw with the GPU hand their buffers over as dmabufs; without a device to
    // name, Xwayland has no DRI3 and its programs (Wine's Vulkan among them) cannot show.
    {
        let importer = backend.clone();
        let mut backend = backend.borrow_mut();
        let node = EGLDevice::device_for_display(backend.renderer().egl_context().display())
            .and_then(|device| device.try_get_render_node());
        match node {
            Ok(Some(node)) => {
                let formats = backend.renderer().dmabuf_formats();
                let feedback = DmabufFeedbackBuilder::new(node.dev_id(), formats).build()?;
                // The global lives as long as the compositor does.
                let _global = state
                    .dmabuf_state
                    .create_global_with_default_feedback::<Nekoland>(display_handle, &feedback);
                state.dmabuf_import = Some(Box::new(move |dmabuf| {
                    importer.borrow_mut().renderer().import_dmabuf(dmabuf, None).is_ok()
                }));
            }
            other => tracing::warn!("no dmabuf for clients: no render node ({other:?})"),
        }
    }

    std::env::set_var("WAYLAND_DISPLAY", &state.socket_name);

    let snapping = backend.clone();
    state.snapshooter = Some(Box::new(move |window, scale| {
        // Not while a frame is being drawn with it; the window then goes without its picture.
        let mut backend = snapping.try_borrow_mut().ok()?;
        crate::motion::snapshot(backend.renderer(), window, scale)
            .inspect_err(|err| tracing::warn!("window snapshot: {err}"))
            .ok()
            .flatten()
    }));

    // The host gives a frame at once whenever one is asked for; frames come at this pace instead.
    let ticking = backend.clone();
    let pace = frame.clone();
    event_loop
        .handle()
        .insert_source(Timer::from_duration(frame.get()), move |_, _, _| {
            ticking.borrow().window().request_redraw();
            TimeoutAction::ToDuration(pace.get())
        })
        .map_err(|err| err.error)?;
    // Input and clients' new frames draw at once, rather than at the next tick.
    let (ping, source) = make_ping()?;
    let pinged = backend.clone();
    event_loop
        .handle()
        .insert_source(source, move |_, _, _| pinged.borrow().window().request_redraw())
        .map_err(|err| err.error)?;
    state.redraw = Some(ping);
    if state.remote {
        let covering = backend.clone();
        event_loop
            .handle()
            .insert_source(Timer::from_duration(SCREEN_CHECK), move |_, _, _| {
                cover_screen(covering.borrow().window());
                TimeoutAction::ToDuration(SCREEN_CHECK)
            })
            .map_err(|err| err.error)?;
    }

    event_loop.handle().insert_source(winit, move |event, _, data| {
        let display = &mut data.display_handle.clone();
        let state = &mut *data;
        let mut backend = backend.borrow_mut();

        match event {
            WinitEvent::Resized { size, scale_factor } => {
                let refresh = refresh_rate(backend.window());
                frame.set(frame_time(refresh));
                let scale = Scale::Fractional(state.config.scale.unwrap_or(scale_factor));
                crate::text::set_picture_scale(scale.fractional_scale());
                output.change_current_state(
                    Some(Mode {
                        size,
                        refresh,
                    }),
                    None,
                    Some(scale),
                    None,
                );
                smithay::desktop::layer_map_for_output(&output).arrange();
            }
            WinitEvent::Input(event) => state.process_input_event(event),
            WinitEvent::Redraw => {
                state.prepare_frame();

                // The host draws the cursor: none is among the elements.
                let wanted = match &state.cursor_status {
                    CursorImageStatus::Hidden => None,
                    CursorImageStatus::Named(icon) => Some(*icon),
                    // shortcut: a cursor a client drew itself shows as the arrow; give the
                    // host its picture once programs that draw their own are common here.
                    CursorImageStatus::Surface(_) => Some(CursorIcon::Default),
                };
                if wanted != shown_cursor {
                    let window = backend.window();
                    window.set_cursor_visible(wanted.is_some());
                    if let Some(icon) = wanted {
                        window.set_cursor(icon);
                    }
                    shown_cursor = wanted;
                }

                let mut scene = dnd_icon_elements(state, backend.renderer(), &output);
                scene.extend(scene_elements(state, backend.renderer(), &output, glass.as_mut()).unwrap());
                let pointer = state.seat.get_pointer().map(|pointer| pointer.current_location()).unwrap_or_default();
                let output_origin = state.space.output_geometry(&output).map(|geometry| geometry.loc).unwrap_or_default();
                let around = (pointer - output_origin.to_f64())
                    .to_physical(output.current_scale().fractional_scale())
                    .to_i32_round();
                let elements = zoomed(scene, around, state.config.zoom);
                let changed = changes
                    .damage_output(1, &elements)
                    .ok()
                    .and_then(|(damage, _)| damage.cloned());
                let submitted = changed.is_some();
                if let Some(changed) = changed {
                    // The buffer's age is known once it is bound. Mesa's software EGL, as
                    // in an xrdp session, cannot tell it: each frame is then drawn whole.
                    let age = if knows_age {
                        drop(backend.bind().unwrap());
                        let age = backend.buffer_age();
                        knows_age = age.is_some();
                        age.unwrap_or(0)
                    } else {
                        0
                    };
                    {
                        let (renderer, mut framebuffer) = backend.bind().unwrap();
                        damage_tracker
                            .render_output(renderer, &mut framebuffer, age, &elements, CLEAR_COLOR)
                            .unwrap();
                    }
                    backend.submit(Some(&changed)).unwrap();
                }
                let frames = state.take_screencopy(&output);
                if !frames.is_empty() {
                    let readings = crate::screencopy::copy(backend.renderer(), &output, &elements, 0, frames);
                    crate::screencopy::finish(backend.renderer(), readings);
                }
                state.capture_windows(backend.renderer());

                state.finish_frame(&output);
                if submitted {
                    state.count_frame();
                }
                let _ = display.flush_clients();
            }
            WinitEvent::CloseRequested => {
                state.loop_signal.stop();
            }
            _ => (),
        };
    })?;

    Ok(())
}

/// The refresh rate, in millihertz, of the display the window is on, or of the fastest
/// display before the host says which that is.
// shortcut: on a host with displays of different rates the window may pace itself to the
// faster one until it is resized; follow the display it moves to if that ever matters.
fn refresh_rate(window: &Window) -> i32 {
    window
        .current_monitor()
        .and_then(|monitor| monitor.refresh_rate_millihertz())
        .or_else(|| window.available_monitors().filter_map(|monitor| monitor.refresh_rate_millihertz()).max())
        .filter(|&millihertz| millihertz > 0)
        .map_or(60_000, |millihertz| millihertz as i32)
}

fn frame_time(millihertz: i32) -> Duration {
    Duration::from_micros(1_000_000_000 / millihertz.max(1) as u64)
}

/// Puts the window over the whole of the X server's screen, which is what xrdp's client shows.
fn cover_screen(window: &Window) {
    let (right, bottom) = window
        .available_monitors()
        .fold((0, 0), |(right, bottom), monitor| {
            let (position, size) = (monitor.position(), monitor.size());
            (
                right.max(position.x + size.width as i32),
                bottom.max(position.y + size.height as i32),
            )
        });
    let size = PhysicalSize::new(right as u32, bottom as u32);
    if right > 0 && bottom > 0 && window.inner_size() != size {
        window.set_outer_position(PhysicalPosition::new(0, 0));
        let _ = window.request_inner_size(size);
    }
}
