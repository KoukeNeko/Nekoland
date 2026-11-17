use crate::{grabs::resize_grab, state::ClientState, Nekoland};
use smithay::{
    backend::renderer::utils::on_commit_buffer_handler,
    delegate_compositor, delegate_shm,
    reexports::wayland_server::{
        protocol::{wl_buffer, wl_surface::WlSurface},
        Client,
    },
    wayland::{
        buffer::BufferHandler,
        compositor::{
            get_parent, is_sync_subsurface, with_states, CompositorClientState, CompositorHandler,
            CompositorState, SurfaceAttributes,
        },
        shm::{ShmHandler, ShmState},
    },
};

use super::xdg_shell;
use smithay::xwayland::XWaylandClientData;

impl CompositorHandler for Nekoland {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        // XWayland's connection is made by Smithay, with its own kind of client data.
        if let Some(state) = client.get_data::<XWaylandClientData>() {
            return &state.compositor_state;
        }
        &client
            .get_data::<ClientState>()
            .expect("a client connected without client data")
            .compositor_state
    }

    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);
        // A drag icon moves its picture by the offset it commits, as GTK does to put its
        // hotspot under the pointer.
        if let Some((_, offset)) = self.dnd_icon.as_mut().filter(|(icon, _)| icon == surface) {
            *offset += with_states(surface, |states| {
                states
                    .cached_state
                    .get::<SurfaceAttributes>()
                    .current()
                    .buffer_delta
                    .take()
                    .unwrap_or_default()
            });
        }
        if !is_sync_subsurface(surface) {
            let mut root = surface.clone();
            while let Some(parent) = get_parent(&root) {
                root = parent;
            }
            if let Some(window) = self.window_for_surface(&root) {
                window.on_commit();
                self.place_new_window(&window);
            }
        };

        xdg_shell::handle_commit(&mut self.popups, &self.space, surface);
        self.layer_commit(surface);
        resize_grab::handle_commit(&mut self.space, surface);
    }
}

impl BufferHandler for Nekoland {
    fn buffer_destroyed(&mut self, _buffer: &wl_buffer::WlBuffer) {}
}

impl ShmHandler for Nekoland {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

delegate_compositor!(Nekoland);
delegate_shm!(Nekoland);
