use crate::{grabs::resize_grab, state::ClientState, Nekoland};
use smithay::{
    backend::renderer::utils::on_commit_buffer_handler,
    delegate_compositor, delegate_drm_syncobj, delegate_shm,
    reexports::{
        calloop::{EventSource, Interest},
        wayland_server::{
            protocol::{wl_buffer, wl_surface::WlSurface},
            Client, Resource,
        },
    },
    wayland::{
        buffer::BufferHandler,
        compositor::{
            add_blocker, add_pre_commit_hook, get_parent, is_sync_subsurface, with_states, Blocker,
            BufferAssignment, CompositorClientState, CompositorHandler, CompositorState, SurfaceAttributes,
        },
        dmabuf::get_dmabuf,
        drm_syncobj::{DrmSyncobjCachedState, DrmSyncobjHandler, DrmSyncobjState},
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

    fn new_surface(&mut self, surface: &WlSurface) {
        // A buffer drawn with the GPU is shown once the GPU has finished drawing it: at the
        // acquire point its client gave, or else when its dmabuf's own fences signal.
        // NVIDIA's driver leaves the renderer to read it half drawn otherwise.
        add_pre_commit_hook::<Self, _>(surface, |state, _, surface| {
            let mut acquire_point = None;
            let dmabuf = with_states(surface, |states| {
                acquire_point.clone_from(
                    &states
                        .cached_state
                        .get::<DrmSyncobjCachedState>()
                        .pending()
                        .acquire_point,
                );
                match &states.cached_state.get::<SurfaceAttributes>().pending().buffer {
                    Some(BufferAssignment::NewBuffer(buffer)) => get_dmabuf(buffer).cloned().ok(),
                    _ => None,
                }
            });
            let Some(dmabuf) = dmabuf else { return };
            if let Some(Ok((blocker, source))) = acquire_point.map(|point| point.generate_blocker()) {
                if block_until(state, surface, blocker, source) {
                    return;
                }
            }
            if let Ok((blocker, source)) = dmabuf.generate_blocker(Interest::READ) {
                block_until(state, surface, blocker, source);
            }
        });
    }

    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);
        if let Some(redraw) = &self.redraw {
            redraw.ping();
        }
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

/// Holds the surface's commit back until `source` fires. False when the event loop would not
/// take the source, and the commit then goes ahead unheld.
fn block_until<S>(
    state: &Nekoland,
    surface: &WlSurface,
    blocker: impl Blocker + Send + 'static,
    source: S,
) -> bool
where
    S: EventSource<Event = (), Ret = std::io::Result<()>> + 'static,
{
    let Some(client) = surface.client() else {
        return false;
    };
    let inserted = state.loop_handle.insert_source(source, move |_, _, data| {
        let dh = data.display_handle.clone();
        data.client_compositor_state(&client)
            .blocker_cleared(&mut *data, &dh);
        Ok(())
    });
    if inserted.is_err() {
        return false;
    }
    add_blocker(surface, blocker);
    true
}

impl DrmSyncobjHandler for Nekoland {
    fn drm_syncobj_state(&mut self) -> Option<&mut DrmSyncobjState> {
        self.syncobj_state.as_mut()
    }
}
delegate_drm_syncobj!(Nekoland);

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
