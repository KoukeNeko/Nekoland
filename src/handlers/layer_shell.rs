use smithay::{
    delegate_layer_shell,
    desktop::{layer_map_for_output, LayerSurface, WindowSurfaceType},
    output::Output,
    reexports::wayland_server::protocol::{wl_output::WlOutput, wl_surface::WlSurface},
    wayland::{
        compositor::with_states,
        shell::wlr_layer::{
            KeyboardInteractivity, Layer, LayerSurface as WlrLayerSurface, LayerSurfaceData,
            WlrLayerShellHandler, WlrLayerShellState,
        },
    },
};

use crate::{state::ABOVE_WINDOWS, Nekoland};

impl WlrLayerShellHandler for Nekoland {
    fn shell_state(&mut self) -> &mut WlrLayerShellState {
        &mut self.layer_shell_state
    }

    fn new_layer_surface(
        &mut self,
        surface: WlrLayerSurface,
        wl_output: Option<WlOutput>,
        _layer: Layer,
        namespace: String,
    ) {
        let Some(output) = wl_output
            .as_ref()
            .and_then(Output::from_resource)
            .or_else(|| self.space.outputs().next().cloned())
        else {
            return;
        };
        let mapped = layer_map_for_output(&output).map_layer(&LayerSurface::new(surface, namespace));
        if let Err(err) = mapped {
            tracing::warn!("could not map layer surface: {err}");
        }
    }

    fn layer_destroyed(&mut self, surface: WlrLayerSurface) {
        for output in self.space.outputs() {
            let mut map = layer_map_for_output(output);
            let layer = map
                .layers()
                .find(|layer| layer.layer_surface() == &surface)
                .cloned();
            if let Some(layer) = layer {
                map.unmap_layer(&layer);
            }
        }

        if self.keyboard_is_on(surface.wl_surface()) {
            self.focus_top_window();
        }
    }
}

delegate_layer_shell!(Nekoland);

impl Nekoland {
    /// Lays the layer surfaces of the output out again, answers a new one's first
    /// commit, and gives the keyboard to one that asks for all of it.
    pub fn layer_commit(&mut self, surface: &WlSurface) {
        let Some(output) = self
            .space
            .outputs()
            .find(|output| {
                layer_map_for_output(output)
                    .layer_for_surface(surface, WindowSurfaceType::TOPLEVEL)
                    .is_some()
            })
            .cloned()
        else {
            return;
        };

        let initial_configure_sent = with_states(surface, |states| {
            states
                .data_map
                .get::<LayerSurfaceData>()
                .map(|data| data.lock().unwrap().initial_configure_sent)
                .unwrap_or(true)
        });

        // Arranged before the first configure, so that it carries the size the client asked for.
        let mut map = layer_map_for_output(&output);
        map.arrange();
        let Some(layer) = map
            .layer_for_surface(surface, WindowSurfaceType::TOPLEVEL)
            .cloned()
        else {
            return;
        };
        drop(map);

        if !initial_configure_sent {
            layer.layer_surface().send_configure();
        } else if self.old_wallpaper.is_some() && self.is_wallpaper(surface) {
            // shortcut: on the first display the new wallpaper draws; once outputs are
            // configured one by one, wait for each.
            self.stop_old_wallpaper();
        }

        let wants_keyboard = layer.cached_state().keyboard_interactivity == KeyboardInteractivity::Exclusive
            && ABOVE_WINDOWS.contains(&layer.layer());
        if wants_keyboard && !self.keyboard_is_on(surface) {
            self.set_keyboard_focus(Some(surface.clone().into()));
        }
    }

    /// Whether a surface is the configured wallpaper's swaybg's, by its process.
    fn is_wallpaper(&self, surface: &WlSurface) -> bool {
        use smithay::reexports::wayland_server::Resource;
        let Some(swaybg) = &self.wallpaper else { return false };
        surface
            .client()
            .and_then(|client| client.get_credentials(&self.display_handle).ok())
            .is_some_and(|credentials| credentials.pid as u32 == swaybg.id())
    }
}
