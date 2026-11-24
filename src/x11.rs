//! X11 programs, through XWayland: this is their window manager.
//!
//! A program that leaves its decoration to us gets a title bar and grips; see `frame`.

use std::{os::fd::OwnedFd, process::Stdio};

use smithay::{
    delegate_xwayland_shell,
    desktop::Window,
    input::pointer::{Focus, GrabStartData},
    reexports::{calloop::LoopHandle, wayland_server::Resource},
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
        X11Surface, X11Wm, XWayland, XWaylandClientData, XWaylandEvent, XwmHandler,
    },
};

use crate::{
    focus::KeyFocus,
    frame,
    grabs::{MoveSurfaceGrab, ResizeSurfaceGrab},
    Nekoland,
};

/// Starts XWayland, and calls `then` once programs can be told where it is, or once
/// it is clear that it will not come up.
pub fn start(
    handle: &LoopHandle<'static, Nekoland>,
    data: &mut Nekoland,
    then: impl FnOnce(&mut Nekoland) + 'static,
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

    data.xwayland_client = Some(client.clone());
    let mut then = Some(then);
    let wm_handle = handle.clone();
    let inserted = handle.insert_source(xwayland, move |event, _, data| {
        match event {
            XWaylandEvent::Ready {
                x11_socket,
                display_number,
            } => match X11Wm::start_wm(wm_handle.clone(), &data.display_handle.clone(), x11_socket, client.clone()) {
                Ok(wm) => {
                    data.xwm = Some(wm);
                    data.scale_x11();
                    std::env::set_var("DISPLAY", format!(":{display_number}"));
                    crate::xsettings::start();
                    if let Some(events) = data.events.clone() {
                        crate::xtray::start(display_number, events, data.display_scale());
                    }
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
    /// Has X11 programs draw at the display's density, as Wayland's do, rather than at a dot
    /// a point and be enlarged and blurred: XWayland is told the display's size in dots and
    /// its programs the density to draw at, and its windows keep their size on screen.
    pub fn scale_x11(&self) {
        let Some(data) = self
            .xwayland_client
            .as_ref()
            .and_then(|client| client.get_data::<XWaylandClientData>())
        else {
            return;
        };
        let scale = self.display_scale();
        if data.compositor_state.client_scale() == scale {
            return;
        }
        data.compositor_state.set_client_scale(scale);
        for output in self.space.outputs() {
            output.change_current_state(None, None, None, None);
        }
        for window in self.all_windows() {
            if let Some(surface) = window
                .x11_surface()
                .filter(|surface| !surface.is_override_redirect())
            {
                let _ = surface.configure(None);
            }
        }
        crate::xsettings::set_scale(scale);
    }

    /// The grab a button held down on this program's own window began, which its request to
    /// move or resize it needs: without one, a program could take the pointer whenever it
    /// liked.
    fn pressed_on(&self, surface: &X11Surface) -> Option<GrabStartData<Nekoland>> {
        let pointer = self.seat.get_pointer()?;
        if !pointer.is_grabbed() {
            return None;
        }
        let start = pointer.grab_start_data()?;
        let (focus, _) = start.focus.as_ref()?;
        focus.id().same_client_as(&surface.wl_surface()?.id()).then_some(start)
    }

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
        // A window with a title bar of ours is centred with it, and below the top. One that
        // belongs to another and draws no bar, as Wine's menus that are windows, is placed by
        // its program: moved, it lands away from what opened it, and the program closes it.
        let bar = frame::title_room(&surface);
        let placed = (surface.is_transient_for().is_some() && bar == 0).then(|| surface.geometry().loc);
        let location = placed
            .or_else(|| {
                state.usable_area().map(|area| {
                    (
                        area.loc.x + (area.size.w - size.w).max(0) / 2,
                        area.loc.y + (area.size.h - size.h - bar).max(0) / 2 + bar,
                    )
                        .into()
                })
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
        // A tray icon is shown in the menu bar instead.
        if crate::xtray::holds(surface.window_id()) {
            return;
        }
        crate::xtray::popup_shown(surface.window_id(), surface.geometry());
        let location = surface.geometry().loc;
        self.space
            .map_element(Window::new_x11_window(surface), location, true);
    }

    fn unmapped_window(&mut self, _xwm: XwmId, surface: X11Surface) {
        crate::xtray::popup_gone(surface.window_id());
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

    fn resize_request(&mut self, _xwm: XwmId, surface: X11Surface, _button: u32, edge: ResizeEdge) {
        let state = self;
        let Some(window) = state.x11_window(&surface) else {
            return;
        };
        let (Some(pointer), Some(location)) = (state.seat.get_pointer(), state.space.element_location(&window))
        else {
            return;
        };
        let Some(start_data) = state.pressed_on(&surface) else {
            return;
        };
        let rect = Rectangle::new(location, window.geometry().size);
        let grab = ResizeSurfaceGrab::start(start_data, window, edge.into(), rect);
        pointer.set_grab(state, grab, SERIAL_COUNTER.next_serial(), Focus::Clear);
    }

    fn move_request(&mut self, _xwm: XwmId, surface: X11Surface, _button: u32) {
        let state = self;
        let Some(window) = state.x11_window(&surface) else {
            return;
        };
        let (Some(pointer), Some(initial_window_location)) =
            (state.seat.get_pointer(), state.space.element_location(&window))
        else {
            return;
        };
        let Some(start_data) = state.pressed_on(&surface) else {
            return;
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

    /// An X11 program drags: dropped where no window takes it, on the Dock or the desktop,
    /// its files go where Nekoland puts files.
    fn dnd_started<S: smithay::input::dnd::Source>(&mut self, _xwm: XwmId, source: &S) {
        self.read_dragged_files(source);
    }

    fn send_selection(&mut self, _xwm: XwmId, selection: SelectionTarget, mime_type: String, fd: OwnedFd) {
        // Files copied from the desktop are Nekoland's own to give, as no program holds them.
        if selection == SelectionTarget::Clipboard {
            let copied = current_data_device_selection_userdata(&self.seat).and_then(|copied| copied.clone());
            if let Some(paths) = copied {
                crate::desktop::send_copied(&mime_type, &paths, fd);
                return;
            }
        }
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

