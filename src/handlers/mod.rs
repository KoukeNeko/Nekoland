mod compositor;
mod layer_shell;
pub mod protocols;
mod xdg_shell;

use crate::{focus::KeyFocus, Nekoland};

//
// Wl Seat
//

use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::input::{Seat, SeatHandler, SeatState};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::Resource;
use smithay::wayland::dmabuf::{DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier};
use smithay::wayland::output::OutputHandler;
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::selection::data_device::{
    set_data_device_focus, ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
};
use smithay::wayland::selection::SelectionHandler;
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

impl SelectionHandler for Nekoland {
    type SelectionUserData = ();
}

impl DataDeviceHandler for Nekoland {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}

impl ClientDndGrabHandler for Nekoland {}
impl ServerDndGrabHandler for Nekoland {}

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
