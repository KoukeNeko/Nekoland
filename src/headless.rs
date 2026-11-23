//! Runs the compositor with no display of its own, for a remote session: what Sunshine
//! streams, through wlr-screencopy, is drawn only when it asks for a frame, and its input
//! comes from the virtual devices Sunshine makes, and from no others.

use std::{
    cell::RefCell,
    fs::OpenOptions,
    os::{fd::OwnedFd, unix::fs::OpenOptionsExt},
    path::Path,
    rc::Rc,
    time::Duration,
};

use smithay::{
    backend::{
        egl::{EGLContext, EGLDevice, EGLDisplay},
        input::InputEvent,
        libinput::LibinputInputBackend,
        renderer::{gles::GlesRenderer, ImportDma, ImportMemWl},
    },
    output::{Mode, Output, PhysicalProperties, Scale, Subpixel},
    reexports::{
        calloop::{
            timer::{TimeoutAction, Timer},
            EventLoop,
        },
        input::{Libinput, LibinputInterface},
    },
    utils::Transform,
    wayland::dmabuf::DmabufFeedbackBuilder,
};
use tracing::{info, warn};

use crate::{
    cursor::Cursors,
    glass::Glass,
    render::{dnd_icon_elements, scene_elements, zoomed, OutputElement},
    udev::cursor_elements,
    Nekoland,
};

pub const OUTPUT_NAME: &str = "HEADLESS-1";
/// The user units Settings writes for Sunshine (ui/src/settings/sunshine.rs): one streams the
/// desktop at the computer, the other runs a session of its own, headless, for it.
pub const DESKTOP_UNIT: &str = "nekoland-sunshine.service";
pub const SESSION_UNIT: &str = "nekoland-remote-session.service";
/// The size of the remote screen, unless the configuration names another for `OUTPUT_NAME`.
const DEFAULT_SIZE: (i32, i32) = (1920, 1080);
const DEFAULT_RATE: u32 = 60;

/// Whether an input device is one Sunshine made for its client: they are all named for the
/// library that makes them, "Sunshine (libvirtualhid) Mouse" and the like.
pub fn is_sunshine_device(name: &str) -> bool {
    name.contains("libvirtualhid")
}

/// Whether Sunshine streams a session of its own, which its devices are then for: its unit
/// is enabled, as systemd records it.
pub fn streams_own_session() -> bool {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| Path::new(&home).join(".config")));
    config.is_some_and(|config| config.join("systemd/user/default.target.wants").join(SESSION_UNIT).exists())
}

/// Opens only the devices Sunshine made: libinput would otherwise take the keyboard and
/// mouse of whoever sits at the computer, where they may be using a session of their own.
struct SunshineDevices;

impl LibinputInterface for SunshineDevices {
    fn open_restricted(&mut self, path: &Path, flags: i32) -> Result<OwnedFd, i32> {
        let name = path
            .file_name()
            .map(|node| Path::new("/sys/class/input").join(node).join("device/name"))
            .and_then(|name| std::fs::read_to_string(name).ok())
            .unwrap_or_default();
        if !is_sunshine_device(&name) {
            return Err(libc::EACCES);
        }
        let access = flags & libc::O_ACCMODE;
        OpenOptions::new()
            .custom_flags(flags)
            .read(access == libc::O_RDONLY || access == libc::O_RDWR)
            .write(access == libc::O_WRONLY || access == libc::O_RDWR)
            .open(path)
            .map(OwnedFd::from)
            .map_err(|err| err.raw_os_error().unwrap_or(libc::EIO))
    }

    fn close_restricted(&mut self, fd: OwnedFd) {
        drop(fd);
    }
}

/// A renderer on the first GPU that will make one, else on Mesa's software device: a session
/// started at boot, with no one logged in at the computer, may not be let use the GPU.
fn renderer() -> Result<GlesRenderer, Box<dyn std::error::Error>> {
    let mut devices: Vec<EGLDevice> = EGLDevice::enumerate()?.collect();
    devices.sort_by_key(EGLDevice::is_software);
    for device in devices {
        // SAFETY: the display and its context live as long as the renderer that holds them.
        let made = unsafe { EGLDisplay::new(device) }
            .and_then(|display| EGLContext::new(&display))
            .map_err(|err| err.to_string())
            .and_then(|context| unsafe { GlesRenderer::new(context) }.map_err(|err| err.to_string()));
        match made {
            Ok(renderer) => return Ok(renderer),
            Err(err) => warn!("headless: an EGL device would not draw: {err}"),
        }
    }
    Err("no EGL device to draw with".into())
}

pub fn init_headless(
    event_loop: &mut EventLoop<Nekoland>,
    state: &mut Nekoland,
) -> Result<(), Box<dyn std::error::Error>> {
    let renderer = Rc::new(RefCell::new(renderer()?));
    state.shm_state.update_formats(renderer.borrow().shm_formats());

    let ((width, height), rate) = state
        .config
        .wanted_mode(OUTPUT_NAME)
        .map_or((DEFAULT_SIZE, DEFAULT_RATE), |((width, height), rate)| {
            ((i32::from(width), i32::from(height)), rate.unwrap_or(DEFAULT_RATE))
        });
    let mode = Mode {
        size: (width, height).into(),
        refresh: (rate * 1000) as i32,
    };
    let output = Output::new(
        OUTPUT_NAME.to_string(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "Nekoland".into(),
            model: "Remote".into(),
            serial_number: "Unknown".into(),
        },
    );
    let _global = output.create_global::<Nekoland>(&state.display_handle);
    let scale = state.config.display_scales.get(OUTPUT_NAME).copied().or(state.config.scale).unwrap_or(1.0);
    crate::text::set_picture_scale(scale);
    output.change_current_state(Some(mode), Some(Transform::Normal), Some(Scale::Fractional(scale)), Some((0, 0).into()));
    output.set_preferred(mode);
    state.space.map_output(&output, (0, 0));

    // Clients that draw with the GPU hand their buffers over as dmabufs; the software device
    // has no node to name, and they draw into shared memory then.
    let node = EGLDevice::device_for_display(renderer.borrow().egl_context().display())
        .and_then(|device| device.try_get_render_node());
    if let Ok(Some(node)) = node {
        let feedback = DmabufFeedbackBuilder::new(node.dev_id(), renderer.borrow().dmabuf_formats()).build()?;
        // The global lives as long as the compositor does.
        let _global = state
            .dmabuf_state
            .create_global_with_default_feedback::<Nekoland>(&state.display_handle, &feedback);
        let importer = renderer.clone();
        state.dmabuf_import = Some(Box::new(move |dmabuf| importer.borrow_mut().import_dmabuf(dmabuf, None).is_ok()));
    }
    let snapping = renderer.clone();
    state.snapshooter = Some(Box::new(move |window, scale| {
        let mut renderer = snapping.try_borrow_mut().ok()?;
        crate::motion::snapshot(&mut renderer, window, scale)
            .inspect_err(|err| warn!("window snapshot: {err}"))
            .ok()
            .flatten()
    }));

    let mut libinput = Libinput::new_with_udev(SunshineDevices);
    libinput
        .udev_assign_seat("seat0")
        .map_err(|()| "libinput could not look for Sunshine's devices")?;
    event_loop
        .handle()
        .insert_source(LibinputInputBackend::new(libinput), |event, _, data| {
            if let InputEvent::DeviceAdded { device } = &event {
                data.add_pointer_device(device.clone());
            }
            data.process_input_event(event)
        })
        .map_err(|err| err.error)?;

    std::env::set_var("WAYLAND_DISPLAY", &state.socket_name);

    let mut cursors = Cursors::load(&state.config.cursor_theme);
    let mut glass = Glass::new(&mut renderer.borrow_mut())
        .inspect_err(|err| warn!("no glass: {err}"))
        .ok();
    let mut draw = move |state: &mut Nekoland| {
        // The client's own size and rate while one streams, as its prep command set them.
        let wanted = state.stream_mode.map_or(mode, |((width, height), rate)| Mode {
            size: (i32::from(width), i32::from(height)).into(),
            refresh: (rate.unwrap_or(DEFAULT_RATE) * 1000) as i32,
        });
        if output.current_mode() != Some(wanted) {
            output.change_current_state(Some(wanted), None, None, None);
            output.set_preferred(wanted);
            smithay::desktop::layer_map_for_output(&output).arrange();
            state.dock.drawn = None;
            state.menu_bar.drawn = None;
            state.fit_windows_to_display();
            info!("{OUTPUT_NAME}: {}x{}", wanted.size.w, wanted.size.h);
        }
        state.prepare_frame();
        let mut renderer = renderer.borrow_mut();
        // Nothing is shown here: the screen is drawn only into the frames Sunshine asks for.
        let frames = state.take_screencopy(&output);
        if !frames.is_empty() {
            let pointer = state.seat.get_pointer().map(|pointer| pointer.current_location()).unwrap_or_default();
            let pointer = pointer.to_physical(scale).to_i32_round();
            let mut elements: Vec<OutputElement> = cursor_elements(state, &mut renderer, &mut cursors, pointer, scale)
                .into_iter()
                .map(OutputElement::Scene)
                .collect();
            let mut scene = dnd_icon_elements(state, &mut renderer, &output);
            let cursor = elements.len() + scene.len();
            match scene_elements(state, &mut renderer, &output, glass.as_mut()) {
                Ok(drawn) => scene.extend(drawn),
                Err(err) => warn!("render: {err}"),
            }
            elements.extend(zoomed(scene, pointer, state.config.zoom));
            crate::screencopy::copy(&mut renderer, &output, &elements, cursor, frames);
            state.count_frame();
        }
        state.capture_windows(&mut renderer);
        drop(renderer);
        // Nothing is shown here, so whatever is mapped counts as shown.
        state.finish_frame(&output);
        let _ = state.display_handle.flush_clients();
    };

    // The tick is the only clock: drawn at every commit too, a client that draws again on
    // each frame it is told was shown would spin. Sunshine's frames wait for it, a frame's
    // time at most.
    let frame = |millihertz: i32| Duration::from_micros(1_000_000_000 / u64::from(millihertz.max(1000) as u32));
    event_loop
        .handle()
        .insert_source(Timer::from_duration(frame(mode.refresh)), move |_, _, data| {
            draw(data);
            let refresh = data.space.outputs().next().and_then(Output::current_mode).map_or(mode.refresh, |mode| mode.refresh);
            TimeoutAction::ToDuration(frame(refresh))
        })
        .map_err(|err| err.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_sunshines_devices_are_its_own() {
        assert!(is_sunshine_device("Sunshine (libvirtualhid) Mouse\n"));
        assert!(is_sunshine_device("Sunshine (libvirtualhid) Keyboard"));
        assert!(!is_sunshine_device("AT Translated Set 2 keyboard"));
        assert!(!is_sunshine_device("Logitech USB Receiver Mouse"));
    }
}
