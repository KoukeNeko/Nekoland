//! Screenshots and screen sharing, through wlr-screencopy: grim takes pictures with it,
//! and xdg-desktop-portal-wlr shares the screen through it with the programs that ask the
//! portal, such as browsers and Discord. A frame asked for is copied, into the client's
//! shared memory, when the output is next drawn; ext-image-copy-capture's frames of an
//! output (`image_capture`) wait and are copied with these.
//!
//! A wlr frame of a whole display may also be a dmabuf, drawn into on the GPU: Sunshine
//! takes nothing else.

use std::{sync::Mutex, time::Duration};

use smithay::{
    backend::{
        allocator::{dmabuf::Dmabuf, Buffer, Fourcc},
        renderer::{
            damage::OutputDamageTracker, element::RenderElement, gles::GlesRenderer, Bind, ExportMem,
            Offscreen, TextureMapping,
        },
    },
    output::Output,
    reexports::{
        wayland_protocols::ext::image_copy_capture::v1::server::ext_image_copy_capture_frame_v1::{
            ExtImageCopyCaptureFrameV1, FailureReason,
        },
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
    wayland::{dmabuf::get_dmabuf, shm::with_buffer_contents_mut},
};

use crate::{render::CLEAR_COLOR, Nekoland};

const VERSION: u32 = 3;
pub(crate) const BYTES_PER_PIXEL: i32 = 4;

/// What a client asked to capture.
pub struct FrameData {
    /// None for an output that has gone: the frame fails.
    output: Option<Output>,
    /// The part of the output, in its pixels.
    region: Rectangle<i32, Physical>,
    cursor: bool,
    /// Whether a dmabuf was offered for it, as for a whole display.
    dmabuf: bool,
    /// Set once the client gave a buffer: a frame is copied only once.
    used: Mutex<bool>,
}

/// Which protocol a frame was asked for in, to answer it in.
pub enum Target {
    Wlr(ZwlrScreencopyFrameV1),
    Ext(ExtImageCopyCaptureFrameV1),
}

/// A frame waiting for its output to be drawn.
pub struct Frame {
    pub target: Target,
    pub buffer: WlBuffer,
    pub output: Output,
    /// The part of the output, in its pixels.
    pub region: Rectangle<i32, Physical>,
    pub cursor: bool,
    /// One of a stream, as a recorder's or a screen share's, rather than a screenshot.
    pub streaming: bool,
}

impl Frame {
    fn is_alive(&self) -> bool {
        match &self.target {
            Target::Wlr(frame) => frame.is_alive(),
            Target::Ext(frame) => frame.is_alive(),
        }
    }

    fn failed(&self) {
        match &self.target {
            Target::Wlr(frame) => frame.failed(),
            Target::Ext(frame) => frame.failed(FailureReason::Unknown),
        }
    }

    fn ready(&self, transform: Transform, time: Duration) {
        let (seconds, nanos) = (time.as_secs(), time.subsec_nanos());
        let size = self.region.size;
        match &self.target {
            Target::Wlr(frame) => {
                if self.streaming {
                    frame.damage(0, 0, size.w as u32, size.h as u32);
                }
                frame.flags(Flags::empty());
                frame.ready((seconds >> 32) as u32, seconds as u32, nanos);
            }
            Target::Ext(frame) => {
                frame.transform(transform.into());
                frame.damage(0, 0, size.w, size.h);
                frame.presentation_time((seconds >> 32) as u32, seconds as u32, nanos);
                frame.ready();
            }
        }
    }
}

/// The format a wlr frame of a whole display offers as a dmabuf.
const DMABUF_FORMAT: Fourcc = Fourcc::Xrgb8888;

/// Whether `buffer` is a dmabuf for a whole display `size` large, in the format offered.
fn fits_dmabuf(buffer: &WlBuffer, size: Size<i32, Physical>) -> bool {
    get_dmabuf(buffer).is_ok_and(|dmabuf| {
        dmabuf.format().code == DMABUF_FORMAT
            && (dmabuf.width(), dmabuf.height()) == (size.w as u32, size.h as u32)
    })
}

/// Whether `buffer` is shared memory in the one format frames are copied in, `size` large.
pub fn fits(buffer: &WlBuffer, size: Size<i32, Physical>) -> bool {
    with_buffer_contents_mut(buffer, |_, len, shm| {
        shm.format == wl_shm::Format::Xrgb8888
            && shm.width == size.w
            && shm.height == size.h
            && shm.stride >= size.w * BYTES_PER_PIXEL
            && shm.offset as usize + (shm.stride * shm.height) as usize <= len
    })
    .unwrap_or(false)
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
        manager: &ZwlrScreencopyManagerV1,
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
        let scale = output.as_ref().map_or(1.0, |output| output.current_scale().fractional_scale());
        // The frame is the display's own picture, unturned, as wlroots gives it: the region
        // asked for is on the output as it is turned, and is turned back onto it here; the
        // client turns the picture upright by the output's transform.
        let transform = output.as_ref().map_or(Transform::Normal, Output::current_transform);
        let turned = Rectangle::from_size(transform.transform_size(size));
        let region = region
            .map(|region| region.to_physical_precise_round(scale))
            .map_or(Some(turned), |region| region.intersection(turned))
            .filter(|region| !region.is_empty())
            .map(|region| transform.invert().transform_rect_in(region, &turned.size));
        let (Some(output), Some(region)) = (output, region) else {
            let data = FrameData {
                output: None,
                region: whole,
                cursor: false,
                dmabuf: false,
                used: Mutex::new(true),
            };
            data_init.init(frame, data).failed();
            return;
        };
        // Drawn straight into, a dmabuf holds the whole display only.
        let dmabuf = manager.version() >= 3 && region == whole;
        let frame = data_init.init(
            frame,
            FrameData {
                output: Some(output),
                region,
                cursor: cursor != 0,
                dmabuf,
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
            if dmabuf {
                frame.linux_dmabuf(DMABUF_FORMAT as u32, region.size.w as u32, region.size.h as u32);
            }
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
                state
                    .screencopy
                    .retain(|pending| !matches!(&pending.target, Target::Wlr(known) if known == frame));
                return;
            }
            _ => return,
        };
        let mut used = data.used.lock().unwrap();
        if *used {
            frame.post_error(zwlr_screencopy_frame_v1::Error::AlreadyUsed, "the frame was copied already");
            return;
        }
        // The buffer must be one the frame said.
        if !fits(&buffer, data.region.size) && !(data.dmabuf && fits_dmabuf(&buffer, data.region.size)) {
            frame.post_error(zwlr_screencopy_frame_v1::Error::InvalidBuffer, "not the buffer the frame asked for");
            return;
        }
        *used = true;
        let Some(output) = data.output.clone() else {
            return frame.failed();
        };
        state.screencopy.push(Frame {
            target: Target::Wlr(frame.clone()),
            buffer,
            output,
            region: data.region,
            cursor: data.cursor,
            streaming: with_damage,
        });
    }
}

impl Nekoland {
    /// The frames waiting for an output, taken to be copied as it is drawn.
    pub fn take_screencopy(&mut self, output: &Output) -> Vec<Frame> {
        let (frames, rest) = std::mem::take(&mut self.screencopy)
            .into_iter()
            .filter(Frame::is_alive)
            .partition(|frame| frame.output == *output);
        self.screencopy = rest;
        if frames.iter().any(|frame| frame.streaming) {
            self.note_screen_frame();
        }
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
        return frames.iter().for_each(Frame::failed);
    };
    let time = monotonic_now();
    let turned = output.current_transform();
    for frame in frames {
        let elements = if frame.cursor { elements } else { &elements[cursor.min(elements.len())..] };
        // wlr-screencopy's picture is the display's own, unturned; ext-image-copy-capture's
        // is drawn upright, as programs that ignore the transform it says would show it.
        let (size, transform) = match frame.target {
            Target::Wlr(_) => (size, turned),
            Target::Ext(_) => (turned.transform_size(size), Transform::Normal),
        };
        let drawn = (size, output.current_scale().fractional_scale(), transform);
        // The region was cut from the output as it was when the frame was asked for; a
        // display made smaller since then no longer holds it.
        let copied = if let Ok(dmabuf) = get_dmabuf(&frame.buffer) {
            if frame.region == Rectangle::from_size(size) {
                draw_into(renderer, drawn, elements, dmabuf.clone())
            } else {
                Err("the display changed size since the frame was asked for".into())
            }
        } else if Rectangle::from_size(size).contains_rect(frame.region) {
            copy_into(renderer, drawn, elements, frame.region, &frame.buffer)
        } else {
            Err("the display changed size since the frame was asked for".into())
        };
        match copied {
            Ok(()) => frame.ready(transform, time),
            Err(err) => {
                tracing::warn!("screen capture: {err}");
                frame.failed();
            }
        }
    }
}

/// Draws `elements` as on a display `size` large, at `scale` and turned by `transform`, and
/// copies `region` of it into `buffer`, which `fits` it.
pub fn copy_into<E: RenderElement<GlesRenderer>>(
    renderer: &mut GlesRenderer,
    (size, scale, transform): (Size<i32, Physical>, f64, Transform),
    elements: &[E],
    region: Rectangle<i32, Physical>,
    buffer: &WlBuffer,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut texture = Offscreen::<smithay::backend::renderer::gles::GlesTexture>::create_buffer(
        renderer,
        Fourcc::Abgr8888,
        size.to_logical(1).to_buffer(1, Transform::Normal),
    )?;
    let mut target = renderer.bind(&mut texture)?;
    // Drawn as the display is, turned with the output.
    OutputDamageTracker::new(size, scale, transform).render_output(
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
    with_buffer_contents_mut(buffer, |ptr, len, shm| {
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

/// Draws `elements` into `dmabuf`, a whole display's frame, as `copy_into` draws them, and
/// waits for the GPU to finish before the client reads it.
fn draw_into<E: RenderElement<GlesRenderer>>(
    renderer: &mut GlesRenderer,
    (size, scale, transform): (Size<i32, Physical>, f64, Transform),
    elements: &[E],
    mut dmabuf: Dmabuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut target = renderer.bind(&mut dmabuf)?;
    let mut damage = OutputDamageTracker::new(size, scale, transform);
    let drawn = damage.render_output(renderer, &mut target, 0, elements, CLEAR_COLOR)?;
    drawn.sync.wait()?;
    Ok(())
}

/// The time on the clock the protocol's timestamps are on.
pub fn monotonic_now() -> Duration {
    let mut now = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // Cannot fail for CLOCK_MONOTONIC with a valid pointer.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut now) };
    Duration::new(now.tv_sec as u64, now.tv_nsec as u32)
}
