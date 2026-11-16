//! Screenshots and screen sharing, through wlr-screencopy: grim takes pictures with it,
//! and xdg-desktop-portal-wlr shares the screen through it with the programs that ask the
//! portal, such as browsers and Discord. A frame asked for is copied, into the client's
//! shared memory, when the output is next drawn.
//!
//! shortcut: shared memory only, no dmabufs, so each frame goes through the CPU; offer
//! dmabufs if sharing a large screen proves slow.

use std::{sync::Mutex, time::Duration};

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            damage::OutputDamageTracker, element::RenderElement, gles::GlesRenderer, Bind, ExportMem,
            Offscreen, TextureMapping,
        },
    },
    output::Output,
    reexports::{
        wayland_protocols_wlr::screencopy::v1::server::{
            zwlr_screencopy_frame_v1::{self, Flags, ZwlrScreencopyFrameV1},
            zwlr_screencopy_manager_v1::{self, ZwlrScreencopyManagerV1},
        },
        wayland_server::{
            protocol::{wl_buffer::WlBuffer, wl_shm},
            Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
        },
    },
    utils::{Physical, Rectangle, Size, Transform},
    wayland::shm::with_buffer_contents_mut,
};

use crate::{render::CLEAR_COLOR, Nekoland};

const VERSION: u32 = 3;
const BYTES_PER_PIXEL: i32 = 4;

/// What a client asked to capture.
pub struct FrameData {
    /// None for an output that has gone: the frame fails.
    output: Option<Output>,
    /// The part of the output, in its pixels.
    region: Rectangle<i32, Physical>,
    cursor: bool,
    /// Set once the client gave a buffer: a frame is copied only once.
    used: Mutex<bool>,
}

/// A frame waiting for its output to be drawn.
pub struct Frame {
    frame: ZwlrScreencopyFrameV1,
    buffer: WlBuffer,
    with_damage: bool,
}

impl Frame {
    fn data(&self) -> &FrameData {
        self.frame.data::<FrameData>().expect("frames are made with their data")
    }
}

pub fn create_global(dh: &DisplayHandle) {
    dh.create_global::<Nekoland, ZwlrScreencopyManagerV1, _>(VERSION, ());
}

impl GlobalDispatch<ZwlrScreencopyManagerV1, ()> for Nekoland {
    fn bind(
        _state: &mut Self,
        _dh: &DisplayHandle,
        _client: &Client,
        resource: New<ZwlrScreencopyManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }

    // It sees what every program shows.
    fn can_view(client: Client, _global_data: &()) -> bool {
        crate::state::unsandboxed(&client)
    }
}

impl Dispatch<ZwlrScreencopyManagerV1, ()> for Nekoland {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _manager: &ZwlrScreencopyManagerV1,
        request: zwlr_screencopy_manager_v1::Request,
        _data: &(),
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        let (frame, cursor, output, region) = match request {
            zwlr_screencopy_manager_v1::Request::CaptureOutput {
                frame,
                overlay_cursor,
                output,
            } => (frame, overlay_cursor, output, None),
            zwlr_screencopy_manager_v1::Request::CaptureOutputRegion {
                frame,
                overlay_cursor,
                output,
                x,
                y,
                width,
                height,
            } => (frame, overlay_cursor, output, Some(Rectangle::new((x, y).into(), (width, height).into()))),
            _ => return,
        };
        let output = Output::from_resource(&output);
        let size = output
            .as_ref()
            .and_then(Output::current_mode)
            .map(|mode| mode.size)
            .unwrap_or_default();
        let whole = Rectangle::from_size(size);
        // Outputs are not scaled: their logical points are their pixels.
        let region = region
            .map(|region| region.to_physical(1))
            .map_or(Some(whole), |region| region.intersection(whole))
            .filter(|region| !region.is_empty());
        let (Some(output), Some(region)) = (output, region) else {
            let data = FrameData {
                output: None,
                region: whole,
                cursor: false,
                used: Mutex::new(true),
            };
            data_init.init(frame, data).failed();
            return;
        };
        let frame = data_init.init(
            frame,
            FrameData {
                output: Some(output),
                region,
                cursor: cursor != 0,
                used: Mutex::new(false),
            },
        );
        frame.buffer(
            wl_shm::Format::Xrgb8888,
            region.size.w as u32,
            region.size.h as u32,
            (region.size.w * BYTES_PER_PIXEL) as u32,
        );
        if frame.version() >= 3 {
            frame.buffer_done();
        }
    }
}

impl Dispatch<ZwlrScreencopyFrameV1, FrameData> for Nekoland {
    fn request(
        state: &mut Self,
        _client: &Client,
        frame: &ZwlrScreencopyFrameV1,
        request: zwlr_screencopy_frame_v1::Request,
        data: &FrameData,
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        let (buffer, with_damage) = match request {
            zwlr_screencopy_frame_v1::Request::Copy { buffer } => (buffer, false),
            zwlr_screencopy_frame_v1::Request::CopyWithDamage { buffer } => (buffer, true),
            zwlr_screencopy_frame_v1::Request::Destroy => {
                state.screencopy.retain(|pending| &pending.frame != frame);
                return;
            }
            _ => return,
        };
        let mut used = data.used.lock().unwrap();
        if *used {
            frame.post_error(zwlr_screencopy_frame_v1::Error::AlreadyUsed, "the frame was copied already");
            return;
        }
        // The buffer must be the one the frame said.
        let size = data.region.size;
        let fits = with_buffer_contents_mut(&buffer, |_, len, shm| {
            shm.format == wl_shm::Format::Xrgb8888
                && shm.width == size.w
                && shm.height == size.h
                && shm.stride >= size.w * BYTES_PER_PIXEL
                && shm.offset as usize + (shm.stride * shm.height) as usize <= len
        });
        if !fits.unwrap_or(false) {
            frame.post_error(zwlr_screencopy_frame_v1::Error::InvalidBuffer, "not the buffer the frame asked for");
            return;
        }
        *used = true;
        state.screencopy.push(Frame {
            frame: frame.clone(),
            buffer,
            with_damage,
        });
    }
}

impl Nekoland {
    /// The frames waiting for an output, taken to be copied as it is drawn.
    pub fn take_screencopy(&mut self, output: &Output) -> Vec<Frame> {
        let (frames, rest) = std::mem::take(&mut self.screencopy)
            .into_iter()
            .filter(|frame| frame.frame.is_alive())
            .partition(|frame| frame.data().output.as_ref() == Some(output));
        self.screencopy = rest;
        frames
    }
}

/// Copies what is drawn on `output` into each frame; `cursor` is how many of the elements,
/// at their front, are the cursor.
pub fn copy<E: RenderElement<GlesRenderer>>(
    renderer: &mut GlesRenderer,
    output: &Output,
    elements: &[E],
    cursor: usize,
    frames: Vec<Frame>,
) {
    let Some(size) = output.current_mode().map(|mode| mode.size) else {
        return frames.iter().for_each(|frame| frame.frame.failed());
    };
    let time = monotonic_now();
    for frame in frames {
        let data = frame.data();
        let elements = if data.cursor { elements } else { &elements[cursor.min(elements.len())..] };
        match copy_frame(renderer, size, elements, &frame) {
            Ok(()) => {
                if frame.with_damage {
                    let region = data.region.size;
                    frame.frame.damage(0, 0, region.w as u32, region.h as u32);
                }
                frame.frame.flags(Flags::empty());
                let seconds = time.as_secs();
                frame.frame.ready((seconds >> 32) as u32, seconds as u32, time.subsec_nanos());
            }
            Err(err) => {
                tracing::warn!("screen capture: {err}");
                frame.frame.failed();
            }
        }
    }
}

fn copy_frame<E: RenderElement<GlesRenderer>>(
    renderer: &mut GlesRenderer,
    size: Size<i32, Physical>,
    elements: &[E],
    frame: &Frame,
) -> Result<(), Box<dyn std::error::Error>> {
    let region = frame.data().region;
    let mut texture = Offscreen::<smithay::backend::renderer::gles::GlesTexture>::create_buffer(
        renderer,
        Fourcc::Abgr8888,
        size.to_logical(1).to_buffer(1, Transform::Normal),
    )?;
    let mut target = renderer.bind(&mut texture)?;
    OutputDamageTracker::new(size, 1.0, Transform::Normal).render_output(
        renderer,
        &mut target,
        0,
        elements,
        CLEAR_COLOR,
    )?;
    // The whole output is read, and the region cut from it here, whichever way up it comes.
    let whole = Rectangle::from_size((size.w, size.h).into());
    let mapping = renderer.copy_framebuffer(&target, whole, Fourcc::Xrgb8888)?;
    // Drawn into a texture, the picture is bottom row first, as GL counts from the lower
    // left: the mapping's flag says so the other way round.
    let flipped = !mapping.flipped();
    let pixels = renderer.map_texture(&mapping)?;
    let output_row = (size.w * BYTES_PER_PIXEL) as usize;
    let row = (region.size.w * BYTES_PER_PIXEL) as usize;
    let left = (region.loc.x * BYTES_PER_PIXEL) as usize;
    with_buffer_contents_mut(&frame.buffer, |ptr, len, shm| {
        // Checked when the buffer was given: it holds every row at its stride.
        let memory = unsafe { std::slice::from_raw_parts_mut(ptr, len) };
        for y in 0..region.size.h as usize {
            let line = region.loc.y as usize + y;
            let from = if flipped { size.h as usize - 1 - line } else { line } * output_row + left;
            let at = shm.offset as usize + y * shm.stride as usize;
            memory[at..at + row].copy_from_slice(&pixels[from..from + row]);
        }
    })?;
    Ok(())
}

/// The time on the clock the protocol's timestamps are on.
fn monotonic_now() -> Duration {
    let mut now = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // Cannot fail for CLOCK_MONOTONIC with a valid pointer.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut now) };
    Duration::new(now.tv_sec as u64, now.tv_nsec as u32)
}
