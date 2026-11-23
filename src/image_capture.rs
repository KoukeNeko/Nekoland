//! ext-image-copy-capture: captures a display, or one window, into a client's shared
//! memory. xdg-desktop-portal-wlr shares a window with it, named by ext-foreign-toplevel-list,
//! and `grim -T` takes a picture of one. A display's frames wait with wlr-screencopy's for
//! it to be drawn; a window's are drawn on their own as the displays are.
//!
//! shortcut: shared memory only, as wlr-screencopy; no cursor sessions, whose captures stop
//! at once.

use std::sync::{
    atomic::{AtomicBool, AtomicU32, Ordering},
    Mutex,
};

use smithay::{
    backend::renderer::{element::AsRenderElements, element::surface::WaylandSurfaceRenderElement, gles::GlesRenderer},
    desktop::Window,
    output::Output,
    reexports::{
        wayland_protocols::ext::{
            image_capture_source::v1::server::{
                ext_foreign_toplevel_image_capture_source_manager_v1::{
                    self, ExtForeignToplevelImageCaptureSourceManagerV1,
                },
                ext_image_capture_source_v1::{self, ExtImageCaptureSourceV1},
                ext_output_image_capture_source_manager_v1::{self, ExtOutputImageCaptureSourceManagerV1},
            },
            image_copy_capture::v1::server::{
                ext_image_copy_capture_cursor_session_v1::{self, ExtImageCopyCaptureCursorSessionV1},
                ext_image_copy_capture_frame_v1::{self, ExtImageCopyCaptureFrameV1, FailureReason},
                ext_image_copy_capture_manager_v1::{self, ExtImageCopyCaptureManagerV1},
                ext_image_copy_capture_session_v1::{self, ExtImageCopyCaptureSessionV1},
            },
        },
        wayland_server::{
            protocol::{wl_buffer::WlBuffer, wl_shm},
            Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
        },
    },
    utils::{IsAlive, Physical, Rectangle, Scale, Size, Transform},
    wayland::foreign_toplevel_list::ForeignToplevelHandle,
};

use crate::{
    screencopy::{self, Target},
    Nekoland,
};

const VERSION: u32 = 1;

/// What a source captures; `Nothing` for an output or a window gone before it was made.
#[derive(Clone)]
pub enum Source {
    Output(Output),
    Window(Window),
    Nothing,
}

pub struct SessionData {
    source: Source,
    cursor: bool,
    /// The size the client was told its buffers are.
    size: Mutex<Size<i32, Physical>>,
    /// How many frames were captured: a second one makes it a stream, not a picture.
    captured: AtomicU32,
}

pub struct FrameData {
    session: ExtImageCopyCaptureSessionV1,
    buffer: Mutex<Option<WlBuffer>>,
    captured: AtomicBool,
}

/// A frame of a window, waiting for the next drawing.
pub struct WindowFrame {
    frame: ExtImageCopyCaptureFrameV1,
    buffer: WlBuffer,
    session: ExtImageCopyCaptureSessionV1,
}

pub fn create_globals(dh: &DisplayHandle) {
    dh.create_global::<Nekoland, ExtImageCopyCaptureManagerV1, _>(VERSION, ());
    dh.create_global::<Nekoland, ExtOutputImageCaptureSourceManagerV1, _>(VERSION, ());
    dh.create_global::<Nekoland, ExtForeignToplevelImageCaptureSourceManagerV1, _>(VERSION, ());
}

impl Nekoland {
    /// How large a picture of `source` is now, in pixels; none once it is gone.
    fn capture_size(&self, source: &Source) -> Option<Size<i32, Physical>> {
        match source {
            // Upright, as the display is seen.
            Source::Output(output) => {
                (output.current_mode()).map(|mode| output.current_transform().transform_size(mode.size))
            }
            Source::Window(window) if window.alive() => {
                let size = window.geometry().size.to_physical_precise_round(self.display_scale());
                (size.w > 0 && size.h > 0).then_some(size)
            }
            Source::Window(_) | Source::Nothing => None,
        }
    }

    /// Copies each window a frame waits for, as it is now; called as the displays are drawn.
    pub fn capture_windows(&mut self, renderer: &mut GlesRenderer) {
        let frames: Vec<WindowFrame> = std::mem::take(&mut self.window_captures);
        if frames.is_empty() {
            return;
        }
        let scale = self.display_scale();
        let time = screencopy::monotonic_now();
        for pending in frames.into_iter().filter(|pending| pending.frame.is_alive()) {
            let data = session_data(&pending.session);
            let Source::Window(window) = &data.source else {
                continue;
            };
            let Some(size) = self.capture_size(&data.source) else {
                pending.session.stopped();
                pending.frame.failed(FailureReason::Stopped);
                continue;
            };
            if size != *data.size.lock().unwrap() {
                announce(&pending.session, data, size);
                pending.frame.failed(FailureReason::BufferConstraints);
                continue;
            }
            // The window alone, its corner at the picture's: what is outside its geometry,
            // as a shadow, falls off the picture.
            let corner = window.geometry().loc;
            let at = smithay::utils::Point::<i32, smithay::utils::Logical>::from((-corner.x, -corner.y))
                .to_physical_precise_round(scale);
            let elements = AsRenderElements::<GlesRenderer>::render_elements::<WaylandSurfaceRenderElement<GlesRenderer>>(
                window,
                renderer,
                at,
                Scale::from(scale),
                1.0,
            );
            let drawn = (size, scale, Transform::Normal);
            match screencopy::copy_into(renderer, drawn, &elements, Rectangle::from_size(size), &pending.buffer) {
                Ok(()) => {
                    if data.captured.fetch_add(1, Ordering::Relaxed) > 0 {
                        self.note_screen_frame();
                    }
                    pending.frame.transform(Transform::Normal.into());
                    pending.frame.damage(0, 0, size.w, size.h);
                    let seconds = time.as_secs();
                    pending.frame.presentation_time((seconds >> 32) as u32, seconds as u32, time.subsec_nanos());
                    pending.frame.ready();
                }
                Err(err) => {
                    tracing::warn!("window capture: {err}");
                    pending.frame.failed(FailureReason::Unknown);
                }
            }
        }
    }
}

fn session_data(session: &ExtImageCopyCaptureSessionV1) -> &SessionData {
    session.data::<SessionData>().expect("sessions are made with their data")
}

/// Tells the client the buffers its frames take: this size, in the one format copied in.
fn announce(session: &ExtImageCopyCaptureSessionV1, data: &SessionData, size: Size<i32, Physical>) {
    *data.size.lock().unwrap() = size;
    session.buffer_size(size.w as u32, size.h as u32);
    session.shm_format(wl_shm::Format::Xrgb8888);
    session.done();
}

impl GlobalDispatch<ExtImageCopyCaptureManagerV1, ()> for Nekoland {
    fn bind(
        _state: &mut Self,
        _dh: &DisplayHandle,
        _client: &Client,
        resource: New<ExtImageCopyCaptureManagerV1>,
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

impl GlobalDispatch<ExtOutputImageCaptureSourceManagerV1, ()> for Nekoland {
    fn bind(
        _state: &mut Self,
        _dh: &DisplayHandle,
        _client: &Client,
        resource: New<ExtOutputImageCaptureSourceManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }

    fn can_view(client: Client, _global_data: &()) -> bool {
        crate::state::unsandboxed(&client)
    }
}

impl GlobalDispatch<ExtForeignToplevelImageCaptureSourceManagerV1, ()> for Nekoland {
    fn bind(
        _state: &mut Self,
        _dh: &DisplayHandle,
        _client: &Client,
        resource: New<ExtForeignToplevelImageCaptureSourceManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }

    fn can_view(client: Client, _global_data: &()) -> bool {
        crate::state::unsandboxed(&client)
    }
}

impl Dispatch<ExtOutputImageCaptureSourceManagerV1, ()> for Nekoland {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _manager: &ExtOutputImageCaptureSourceManagerV1,
        request: ext_output_image_capture_source_manager_v1::Request,
        _data: &(),
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        if let ext_output_image_capture_source_manager_v1::Request::CreateSource { source, output } = request {
            let captured = Output::from_resource(&output).map_or(Source::Nothing, Source::Output);
            data_init.init(source, captured);
        }
    }
}

impl Dispatch<ExtForeignToplevelImageCaptureSourceManagerV1, ()> for Nekoland {
    fn request(
        state: &mut Self,
        _client: &Client,
        _manager: &ExtForeignToplevelImageCaptureSourceManagerV1,
        request: ext_foreign_toplevel_image_capture_source_manager_v1::Request,
        _data: &(),
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        if let ext_foreign_toplevel_image_capture_source_manager_v1::Request::CreateSource {
            source,
            toplevel_handle,
        } = request
        {
            let window = ForeignToplevelHandle::from_resource(&toplevel_handle)
                .and_then(|handle| state.foreign_toplevels.listed_window(&handle).cloned());
            data_init.init(source, window.map_or(Source::Nothing, Source::Window));
        }
    }
}

impl Dispatch<ExtImageCaptureSourceV1, Source> for Nekoland {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _source: &ExtImageCaptureSourceV1,
        _request: ext_image_capture_source_v1::Request,
        _data: &Source,
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
    }
}

impl Dispatch<ExtImageCopyCaptureManagerV1, ()> for Nekoland {
    fn request(
        state: &mut Self,
        _client: &Client,
        manager: &ExtImageCopyCaptureManagerV1,
        request: ext_image_copy_capture_manager_v1::Request,
        _data: &(),
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            ext_image_copy_capture_manager_v1::Request::CreateSession { session, source, options } => {
                let Ok(options) = options.into_result() else {
                    manager.post_error(ext_image_copy_capture_manager_v1::Error::InvalidOption, "unknown options");
                    return;
                };
                let source = source.data::<Source>().cloned().unwrap_or(Source::Nothing);
                let size = state.capture_size(&source);
                let data = SessionData {
                    source,
                    cursor: options.contains(ext_image_copy_capture_manager_v1::Options::PaintCursors),
                    size: Mutex::default(),
                    captured: AtomicU32::new(0),
                };
                let session = data_init.init(session, data);
                match size {
                    Some(size) => announce(&session, session_data(&session), size),
                    None => session.stopped(),
                }
            }
            ext_image_copy_capture_manager_v1::Request::CreatePointerCursorSession { session, .. } => {
                data_init.init(session, ());
            }
            _ => {}
        }
    }
}

impl Dispatch<ExtImageCopyCaptureCursorSessionV1, ()> for Nekoland {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _cursor: &ExtImageCopyCaptureCursorSessionV1,
        request: ext_image_copy_capture_cursor_session_v1::Request,
        _data: &(),
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        if let ext_image_copy_capture_cursor_session_v1::Request::GetCaptureSession { session } = request {
            let data = SessionData {
                source: Source::Nothing,
                cursor: false,
                size: Mutex::default(),
                captured: AtomicU32::new(0),
            };
            data_init.init(session, data).stopped();
        }
    }
}

impl Dispatch<ExtImageCopyCaptureSessionV1, SessionData> for Nekoland {
    fn request(
        _state: &mut Self,
        _client: &Client,
        session: &ExtImageCopyCaptureSessionV1,
        request: ext_image_copy_capture_session_v1::Request,
        _data: &SessionData,
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        if let ext_image_copy_capture_session_v1::Request::CreateFrame { frame } = request {
            data_init.init(
                frame,
                FrameData {
                    session: session.clone(),
                    buffer: Mutex::default(),
                    captured: AtomicBool::new(false),
                },
            );
        }
    }
}

impl Dispatch<ExtImageCopyCaptureFrameV1, FrameData> for Nekoland {
    fn request(
        state: &mut Self,
        _client: &Client,
        frame: &ExtImageCopyCaptureFrameV1,
        request: ext_image_copy_capture_frame_v1::Request,
        data: &FrameData,
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        use ext_image_copy_capture_frame_v1::{Error, Request};
        match request {
            Request::AttachBuffer { buffer } => *data.buffer.lock().unwrap() = Some(buffer),
            // The whole picture is copied each time, whatever the client says changed.
            Request::DamageBuffer { .. } => {}
            Request::Capture => {
                if data.captured.swap(true, Ordering::Relaxed) {
                    frame.post_error(Error::AlreadyCaptured, "the frame was captured already");
                    return;
                }
                let Some(buffer) = data.buffer.lock().unwrap().clone() else {
                    frame.post_error(Error::NoBuffer, "capture without a buffer");
                    return;
                };
                let session = session_data(&data.session);
                let size = *session.size.lock().unwrap();
                if !screencopy::fits(&buffer, size) {
                    frame.failed(FailureReason::BufferConstraints);
                    return;
                }
                match &session.source {
                    Source::Output(output) => {
                        if state.capture_size(&session.source) != Some(size) {
                            announce(&data.session, session, state.capture_size(&session.source).unwrap_or(size));
                            frame.failed(FailureReason::BufferConstraints);
                            return;
                        }
                        let streaming = session.captured.fetch_add(1, Ordering::Relaxed) > 0;
                        state.screencopy.push(screencopy::Frame {
                            target: Target::Ext(frame.clone()),
                            buffer,
                            output: output.clone(),
                            region: Rectangle::from_size(size),
                            cursor: session.cursor,
                            streaming,
                        });
                    }
                    Source::Window(_) => state.window_captures.push(WindowFrame {
                        frame: frame.clone(),
                        buffer,
                        session: data.session.clone(),
                    }),
                    Source::Nothing => frame.failed(FailureReason::Stopped),
                }
            }
            Request::Destroy => {
                state.window_captures.retain(|pending| &pending.frame != frame);
                state
                    .screencopy
                    .retain(|pending| !matches!(&pending.target, Target::Ext(known) if known == frame));
            }
            _ => {}
        }
    }
}
