pub mod appmenu;
mod compositor;
pub mod foreign_toplevel;
mod layer_shell;
pub mod protocols;
pub mod xdg_shell;

use crate::{focus::KeyFocus, Nekoland};

//
// Wl Seat
//

use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::input::dnd::{DnDGrab, DndGrabHandler, DndTarget, GrabType, Source};
use smithay::input::pointer::Focus;
use smithay::input::{Seat, SeatHandler, SeatState};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::Resource;
use smithay::wayland::dmabuf::{DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier};
use smithay::wayland::output::OutputHandler;
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::selection::data_device::{
    set_data_device_focus, DataDeviceHandler, DataDeviceState, WaylandDndGrabHandler,
};
use smithay::wayland::selection::{SelectionHandler, SelectionSource, SelectionTarget};
use smithay::wayland::tablet_manager::TabletSeatHandler;
use smithay::{delegate_cursor_shape, delegate_data_device, delegate_dmabuf, delegate_output, delegate_seat};

impl SeatHandler for Nekoland {
    type KeyboardFocus = KeyFocus;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Nekoland> {
        &mut self.seat_state
    }

    fn cursor_image(&mut self, _seat: &Seat<Self>, image: smithay::input::pointer::CursorImageStatus) {
        self.cursor_status = image;
    }

    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&KeyFocus>) {
        let dh = &self.display_handle;
        let client = focused
            .and_then(|focus| focus.wl_surface())
            .and_then(|surface| dh.get_client(surface.id()).ok());
        set_data_device_focus(dh, seat, client);
    }
}

delegate_seat!(Nekoland);

//
// Wl Data Device
//

// A Wayland program's selection is offered to X11 programs, and read for them.
impl SelectionHandler for Nekoland {
    /// Files copied from the desktop, which Nekoland itself gives out; none for what an
    /// X11 program copied.
    type SelectionUserData = Option<Vec<std::path::PathBuf>>;

    fn new_selection(&mut self, ty: SelectionTarget, source: Option<SelectionSource>, _seat: Seat<Self>) {
        if let Some(xwm) = self.xwm.as_mut() {
            if let Err(err) = xwm.new_selection(ty, source.map(|source| source.mime_types())) {
                tracing::warn!("offering the {ty:?} selection to X11 programs: {err}");
            }
        }
    }

    fn send_selection(
        &mut self,
        ty: SelectionTarget,
        mime_type: String,
        fd: std::os::fd::OwnedFd,
        _seat: Seat<Self>,
        copied: &Option<Vec<std::path::PathBuf>>,
    ) {
        if let Some(paths) = copied {
            crate::desktop::send_copied(&mime_type, paths, fd);
        } else if let Some(xwm) = self.xwm.as_mut() {
            if let Err(err) = xwm.send_selection(ty, mime_type, fd) {
                tracing::warn!("reading the {ty:?} selection from an X11 program: {err}");
            }
        }
    }
}

impl DataDeviceHandler for Nekoland {
    fn data_device_state(&mut self) -> &mut DataDeviceState {
        &mut self.data_device_state
    }
}

impl WaylandDndGrabHandler for Nekoland {
    fn dnd_requested<S: Source>(
        &mut self,
        source: S,
        icon: Option<WlSurface>,
        seat: Seat<Self>,
        serial: smithay::utils::Serial,
        type_: GrabType,
    ) {
        self.dnd_icon = icon.map(|icon| (icon, Default::default()));
        self.dock_drag_started(&source);
        let display = self.display_handle.clone();
        match type_ {
            GrabType::Pointer => {
                let Some(pointer) = seat.get_pointer() else { return };
                let Some(start) = pointer.grab_start_data() else { return };
                let grab = DnDGrab::new_pointer(&display, start, source, seat);
                pointer.set_grab(self, grab, serial, Focus::Keep);
            }
            GrabType::Touch => {
                let Some(touch) = seat.get_touch() else { return };
                let Some(start) = touch.grab_start_data() else { return };
                touch.set_grab(self, DnDGrab::new_touch(&display, start, source, seat), serial);
            }
        }
    }
}

impl DndGrabHandler for Nekoland {
    fn dropped(
        &mut self,
        _target: Option<DndTarget<'_, Self>>,
        validated: bool,
        _seat: Seat<Self>,
        _location: smithay::utils::Point<f64, smithay::utils::Logical>,
    ) {
        self.dnd_icon = None;
        // Files dragged off the desktop: no window took them, so Nekoland puts them.
        if self.desktop_dragging() {
            if !validated {
                self.desktop_dropped_here();
            }
            self.desktop_drag_ended();
        } else {
            self.dock_dropped(validated);
        }
    }
}

delegate_data_device!(Nekoland);

//
// Wl Output & Xdg Output
//

impl OutputHandler for Nekoland {}
delegate_output!(Nekoland);

//
// Cursor shapes by name, which need the tablet handler to exist
//

impl TabletSeatHandler for Nekoland {}
delegate_cursor_shape!(Nekoland);

//
// GPU buffers from clients
//

impl DmabufHandler for Nekoland {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.dmabuf_state
    }

    fn dmabuf_imported(&mut self, _global: &DmabufGlobal, dmabuf: Dmabuf, notifier: ImportNotifier) {
        let imported = self.dmabuf_import.as_mut().is_some_and(|import| import(&dmabuf));
        if imported {
            let _ = notifier.successful::<Nekoland>();
        } else {
            notifier.failed();
        }
    }
}
delegate_dmabuf!(Nekoland);
