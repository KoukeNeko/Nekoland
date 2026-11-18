//! X11 programs, through XWayland: this is their window manager.
//!
//! shortcut: X11 windows are resized from their own edges only by programs that do it
//! themselves.

use std::{os::fd::OwnedFd, process::Stdio};

use smithay::{
    delegate_xwayland_shell,
    desktop::Window,
    input::pointer::{Focus, GrabStartData},
    reexports::calloop::LoopHandle,
    utils::{Logical, Rectangle, SERIAL_COUNTER},
    wayland::{
        selection::{
            data_device::{
                clear_data_device_selection, current_data_device_selection_userdata,
                request_data_device_client_selection, set_data_device_selection,
            },
            primary_selection::{
                clear_primary_selection, current_primary_selection_userdata,
                request_primary_client_selection, set_primary_selection,
            },
            SelectionTarget,
        },
        xwayland_shell::{XWaylandShellHandler, XWaylandShellState},
    },
    xwayland::{
        xwm::{Reorder, ResizeEdge, XwmId},
        X11Surface, X11Wm, XWayland, XWaylandEvent, XwmHandler,
    },
};

use crate::{focus::KeyFocus, grabs::MoveSurfaceGrab, CalloopData, Nekoland};

/// Starts XWayland, and calls `then` once programs can be told where it is, or once
/// it is clear that it will not come up.
pub fn start(
    handle: &LoopHandle<'static, CalloopData>,
    data: &mut CalloopData,
    then: impl FnOnce(&mut CalloopData) + 'static,
) {
    let spawned = XWayland::spawn(
        &data.display_handle,
        None,
        std::iter::empty::<(String, String)>(),
        true,
        Stdio::null(),
        Stdio::null(),
        |_| (),
    );
    let (xwayland, client) = match spawned {
        Ok(spawned) => spawned,
        Err(err) => {
            tracing::warn!("XWayland did not start, X11 programs will not run: {err}");
            return then(data);
        }
    };

    let mut then = Some(then);
    let wm_handle = handle.clone();
    let inserted = handle.insert_source(xwayland, move |event, _, data| {
        match event {
            XWaylandEvent::Ready {
                x11_socket,
                display_number,
            } => match X11Wm::start_wm(wm_handle.clone(), x11_socket, client.clone()) {
                Ok(wm) => {
                    data.state.xwm = Some(wm);
                    std::env::set_var("DISPLAY", format!(":{display_number}"));
                }
                Err(err) => tracing::warn!("could not manage X11 windows: {err}"),
            },
            XWaylandEvent::Error => tracing::warn!("XWayland failed while starting"),
        }
        if let Some(then) = then.take() {
            then(data);
        }
    });
    if let Err(err) = inserted {
        tracing::warn!("XWayland: {err}");
    }
}

impl Nekoland {
    fn x11_window(&self, surface: &X11Surface) -> Option<Window> {
        self.all_windows()
            .find(|window| window.x11_surface() == Some(surface))
            .cloned()
    }
}

impl XWaylandShellHandler for Nekoland {
    fn xwayland_shell_state(&mut self) -> &mut XWaylandShellState {
        &mut self.xwayland_shell_state
    }
}
delegate_xwayland_shell!(Nekoland);

impl XWaylandShellHandler for CalloopData {
    fn xwayland_shell_state(&mut self) -> &mut XWaylandShellState {
        &mut self.state.xwayland_shell_state
    }
}

impl XwmHandler for Nekoland {
    fn xwm_state(&mut self, _xwm: XwmId) -> &mut X11Wm {
        self.xwm.as_mut().expect("X11 events without a window manager")
    }

    fn new_window(&mut self, _xwm: XwmId, _window: X11Surface) {}

    fn new_override_redirect_window(&mut self, _xwm: XwmId, _window: X11Surface) {}

    fn map_window_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        let state = self;
        if surface.set_mapped(true).is_err() {
            return;
        }
        let size = surface.geometry().size;
        let location = state
            .usable_area()
            .map(|area| {
                (
                    area.loc.x + (area.size.w - size.w).max(0) / 2,
                    area.loc.y + (area.size.h - size.h).max(0) / 2,
                )
                    .into()
            })
            .unwrap_or_default();
        let _ = surface.configure(Rectangle::new(location, size));

        let window = Window::new_x11_window(surface);
        state.space.map_element(window.clone(), location, false);
        state.focus_window(&window);
        state.motion_open(&window);
    }

    // Menus and tooltips place themselves, and never take the keyboard through us.
    fn mapped_override_redirect_window(&mut self, _xwm: XwmId, surface: X11Surface) {
        let location = surface.geometry().loc;
        self.space
            .map_element(Window::new_x11_window(surface), location, true);
    }

    fn unmapped_window(&mut self, _xwm: XwmId, surface: X11Surface) {
        if let Some(window) = self.x11_window(&surface) {
            self.forget_window(&window);
        }
        if !surface.is_override_redirect() {
            let _ = surface.set_mapped(false);
        }
    }

    fn destroyed_window(&mut self, _xwm: XwmId, surface: X11Surface) {
        if let Some(window) = self.x11_window(&surface) {
            self.forget_window(&window);
        }
    }

    fn configure_request(
        &mut self,
        _xwm: XwmId,
        surface: X11Surface,
        _x: Option<i32>,
        _y: Option<i32>,
        width: Option<u32>,
        height: Option<u32>,
        _reorder: Option<Reorder>,
    ) {
        // The size is the program's to choose; where the window is, is ours.
        let mut geometry = surface.geometry();
        if let Some(width) = width {
            geometry.size.w = width as i32;
        }
        if let Some(height) = height {
            geometry.size.h = height as i32;
        }
        let _ = surface.configure(geometry);
    }

    fn configure_notify(
        &mut self,
        _xwm: XwmId,
        surface: X11Surface,
        geometry: Rectangle<i32, Logical>,
        _above: Option<u32>,
    ) {
        if !surface.is_override_redirect() {
            return;
        }
        if let Some(window) = self.x11_window(&surface) {
            self.space.map_element(window, geometry.loc, false);
        }
    }

    fn maximize_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        if let Some(window) = self.x11_window(&surface) {
            self.maximize(&window);
        }
    }

    fn unmaximize_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        if let Some(window) = self.x11_window(&surface) {
            self.restore_on_request(&window);
        }
    }

    fn fullscreen_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        if let Some(window) = self.x11_window(&surface) {
            self.fullscreen(&window);
        }
    }

    fn unfullscreen_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        if let Some(window) = self.x11_window(&surface) {
            self.restore_on_request(&window);
        }
    }

    fn minimize_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        if let Some(window) = self.x11_window(&surface) {
            self.minimize(&window);
        }
    }

    fn resize_request(&mut self, _xwm: XwmId, _surface: X11Surface, _button: u32, _edge: ResizeEdge) {}

    fn move_request(&mut self, _xwm: XwmId, surface: X11Surface, button: u32) {
        let state = self;
        let Some(window) = state.x11_window(&surface) else {
            return;
        };
        let (Some(pointer), Some(initial_window_location)) =
            (state.seat.get_pointer(), state.space.element_location(&window))
        else {
            return;
        };
        let start_data = GrabStartData {
            focus: None,
            button,
            location: pointer.current_location(),
        };
        let grab = MoveSurfaceGrab::new(start_data, window, initial_window_location);
        pointer.set_grab(state, grab, SERIAL_COUNTER.next_serial(), Focus::Clear);
    }

    // The clipboard and the primary selection are one for X11 and Wayland programs: what
    // one side copies, the compositor offers to the other.
    fn allow_selection_access(&mut self, xwm: XwmId, _selection: SelectionTarget) -> bool {
        // Only the X11 window typed into reads it, as only the focused Wayland one does.
        matches!(
            self.seat.get_keyboard().and_then(|keyboard| keyboard.current_focus()),
            Some(KeyFocus::X11(surface)) if surface.xwm_id() == Some(xwm)
        )
    }

    fn send_selection(&mut self, _xwm: XwmId, selection: SelectionTarget, mime_type: String, fd: OwnedFd) {
        let sent = match selection {
            SelectionTarget::Clipboard => {
                request_data_device_client_selection(&self.seat, mime_type, fd).map_err(|err| err.to_string())
            }
            SelectionTarget::Primary => {
                request_primary_client_selection(&self.seat, mime_type, fd).map_err(|err| err.to_string())
            }
        };
        if let Err(err) = sent {
            tracing::warn!("X11 program reading the {selection:?} selection: {err}");
        }
    }

    fn new_selection(&mut self, _xwm: XwmId, selection: SelectionTarget, mime_types: Vec<String>) {
        match selection {
            SelectionTarget::Clipboard => {
                set_data_device_selection(&self.display_handle, &self.seat, mime_types, None)
            }
            SelectionTarget::Primary => {
                set_primary_selection(&self.display_handle, &self.seat, mime_types, None)
            }
        }
    }

    fn cleared_selection(&mut self, _xwm: XwmId, selection: SelectionTarget) {
        // Only what X11 set: a Wayland program's selection, or files copied from the desktop,
        // stays.
        match selection {
            SelectionTarget::Clipboard
                if current_data_device_selection_userdata(&self.seat)
                    .is_some_and(|copied| copied.is_none()) =>
            {
                clear_data_device_selection(&self.display_handle, &self.seat)
            }
            SelectionTarget::Primary
                if current_primary_selection_userdata(&self.seat).is_some_and(|copied| copied.is_none()) =>
            {
                clear_primary_selection(&self.display_handle, &self.seat)
            }
            _ => {}
        }
    }
}

// The window manager is driven by the event loop, whose data is what it calls into.
impl XwmHandler for CalloopData {
    fn xwm_state(&mut self, xwm: XwmId) -> &mut X11Wm {
        self.state.xwm_state(xwm)
    }
    fn new_window(&mut self, xwm: XwmId, window: X11Surface) {
        self.state.new_window(xwm, window)
    }
    fn new_override_redirect_window(&mut self, xwm: XwmId, window: X11Surface) {
        self.state.new_override_redirect_window(xwm, window)
    }
    fn map_window_request(&mut self, xwm: XwmId, window: X11Surface) {
        self.state.map_window_request(xwm, window)
    }
    fn mapped_override_redirect_window(&mut self, xwm: XwmId, window: X11Surface) {
        self.state.mapped_override_redirect_window(xwm, window)
    }
    fn unmapped_window(&mut self, xwm: XwmId, window: X11Surface) {
        self.state.unmapped_window(xwm, window)
    }
    fn destroyed_window(&mut self, xwm: XwmId, window: X11Surface) {
        self.state.destroyed_window(xwm, window)
    }
    fn configure_request(
        &mut self,
        xwm: XwmId,
        window: X11Surface,
        x: Option<i32>,
        y: Option<i32>,
        width: Option<u32>,
        height: Option<u32>,
        reorder: Option<Reorder>,
    ) {
        self.state
            .configure_request(xwm, window, x, y, width, height, reorder)
    }
    fn configure_notify(
        &mut self,
        xwm: XwmId,
        window: X11Surface,
        geometry: Rectangle<i32, Logical>,
        above: Option<u32>,
    ) {
        self.state.configure_notify(xwm, window, geometry, above)
    }
    fn maximize_request(&mut self, xwm: XwmId, window: X11Surface) {
        self.state.maximize_request(xwm, window)
    }
    fn unmaximize_request(&mut self, xwm: XwmId, window: X11Surface) {
        self.state.unmaximize_request(xwm, window)
    }
    fn fullscreen_request(&mut self, xwm: XwmId, window: X11Surface) {
        self.state.fullscreen_request(xwm, window)
    }
    fn unfullscreen_request(&mut self, xwm: XwmId, window: X11Surface) {
        self.state.unfullscreen_request(xwm, window)
    }
    fn minimize_request(&mut self, xwm: XwmId, window: X11Surface) {
        self.state.minimize_request(xwm, window)
    }
    fn resize_request(&mut self, xwm: XwmId, window: X11Surface, button: u32, edge: ResizeEdge) {
        self.state.resize_request(xwm, window, button, edge)
    }
    fn move_request(&mut self, xwm: XwmId, window: X11Surface, button: u32) {
        self.state.move_request(xwm, window, button)
    }
    fn allow_selection_access(&mut self, xwm: XwmId, selection: SelectionTarget) -> bool {
        self.state.allow_selection_access(xwm, selection)
    }
    fn send_selection(&mut self, xwm: XwmId, selection: SelectionTarget, mime_type: String, fd: OwnedFd) {
        self.state.send_selection(xwm, selection, mime_type, fd)
    }
    fn new_selection(&mut self, xwm: XwmId, selection: SelectionTarget, mime_types: Vec<String>) {
        XwmHandler::new_selection(&mut self.state, xwm, selection, mime_types)
    }
    fn cleared_selection(&mut self, xwm: XwmId, selection: SelectionTarget) {
        self.state.cleared_selection(xwm, selection)
    }
}
