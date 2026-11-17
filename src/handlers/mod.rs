pub mod appmenu;
mod compositor;
pub mod foreign_toplevel;
mod layer_shell;
pub mod protocols;
mod xdg_shell;

use crate::{focus::KeyFocus, Nekoland};

//
// Wl Seat
//

use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::input::{Seat, SeatHandler, SeatState};
use smithay::reexports::wayland_server::protocol::wl_data_source::WlDataSource;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::Resource;
use smithay::wayland::dmabuf::{DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier};
use smithay::wayland::output::OutputHandler;
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::selection::data_device::{
    set_data_device_focus, ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
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
            if let Err(err) = xwm.send_selection(ty, mime_type, fd, self.loop_handle.clone()) {
                tracing::warn!("reading the {ty:?} selection from an X11 program: {err}");
            }
        }
    }
}

impl DataDeviceHandler for Nekoland {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}

impl ClientDndGrabHandler for Nekoland {
    fn started(&mut self, source: Option<WlDataSource>, icon: Option<WlSurface>, _seat: Seat<Self>) {
        self.dnd_icon = icon.map(|icon| (icon, Default::default()));
        self.dock_drag_started(source.as_ref());
    }

    fn dropped(&mut self, _target: Option<WlSurface>, validated: bool, _seat: Seat<Self>) {
        self.dnd_icon = None;
        self.dock_dropped(validated);
    }
}
/// Files dragged off the desktop: Nekoland is where they come from.
impl ServerDndGrabHandler for Nekoland {
    fn send(&mut self, _mime_type: String, fd: std::os::fd::OwnedFd, _seat: Seat<Self>) {
        self.desktop_drag_send(fd);
    }

    fn dropped(&mut self, _seat: Seat<Self>) {
        self.desktop_drag_ended();
    }

    fn cancelled(&mut self, _seat: Seat<Self>) {
        self.desktop_dropped_here();
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
