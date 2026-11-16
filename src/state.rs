use std::{collections::HashSet, ffi::OsString, sync::Arc, time::Duration};

use smithay::{
    backend::allocator::dmabuf::Dmabuf,
    desktop::{layer_map_for_output, LayerSurface, PopupManager, Space, Window, WindowSurfaceType},
    input::{keyboard::Keycode, pointer::CursorImageStatus, Seat, SeatState},
    output::Output,
    reexports::{
        calloop::{generic::Generic, EventLoop, Interest, LoopSignal, Mode, PostAction},
        wayland_server::{
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::wl_surface::WlSurface,
            Display, DisplayHandle,
        },
    },
    utils::{Logical, Point},
    wayland::{
        compositor::{CompositorClientState, CompositorState},
        cursor_shape::CursorShapeManagerState,
        dmabuf::DmabufState,
        output::OutputManagerState,
        selection::data_device::DataDeviceState,
        shell::{
            wlr_layer::{Layer, WlrLayerShellState},
            xdg::XdgShellState,
        },
        shm::ShmState,
        socket::ListeningSocketSource,
        xwayland_shell::XWaylandShellState,
    },
    xwayland::X11Wm,
};

use crate::{
    config::Config,
    handlers::{foreign_toplevel::ForeignToplevelState, protocols::Protocols},
    overview::Overview,
    text::Apps,
    workspaces::Workspaces,
    CalloopData,
};

/// Takes a client's GPU buffer; returns whether the renderer can use it.
pub type DmabufImport = Box<dyn FnMut(&Dmabuf) -> bool>;

pub const ABOVE_WINDOWS: [Layer; 2] = [Layer::Overlay, Layer::Top];
pub const BELOW_WINDOWS: [Layer; 2] = [Layer::Bottom, Layer::Background];

pub struct Nekoland {
    pub start_time: std::time::Instant,
    pub config: Config,
    pub socket_name: OsString,
    pub display_handle: DisplayHandle,

    pub space: Space<Window>,
    /// Minimized windows, with where they go back to.
    pub minimized: Vec<(Window, Point<i32, Logical>)>,
    pub overview: Option<Overview>,
    pub workspaces: Workspaces,
    /// Names and icons of applications, for the overview.
    pub apps: Apps,
    /// Whether Super went down with nothing else pressed since.
    pub super_tap: bool,
    /// Keys whose press ran a shortcut, so their release is not sent to a client either.
    pub suppressed_keys: HashSet<Keycode>,
    pub loop_signal: LoopSignal,

    // Smithay State
    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub layer_shell_state: WlrLayerShellState,
    pub shm_state: ShmState,
    pub output_manager_state: OutputManagerState,
    pub seat_state: SeatState<Nekoland>,
    pub data_device_state: DataDeviceState,
    pub popups: PopupManager,
    pub cursor_shape_state: CursorShapeManagerState,
    pub protocols: Protocols,
    pub foreign_toplevels: ForeignToplevelState,
    pub dmabuf_state: DmabufState,
    /// Hands a client's GPU buffer to the renderer; set by a backend that takes them.
    pub dmabuf_import: Option<DmabufImport>,
    /// Switches to another virtual terminal; set by the backend that owns a session.
    pub switch_vt: Option<Box<dyn FnMut(i32)>>,
    pub xwayland_shell_state: XWaylandShellState,
    /// The X11 window manager, once XWayland is up.
    pub xwm: Option<X11Wm>,
    /// The cursor the client under the pointer asked for.
    pub cursor_status: CursorImageStatus,

    pub seat: Seat<Self>,
}

impl Nekoland {
    pub fn new(event_loop: &mut EventLoop<CalloopData>, display: Display<Self>) -> Self {
        let start_time = std::time::Instant::now();

        let dh = display.handle();

        let compositor_state = CompositorState::new::<Self>(&dh);
        let xdg_shell_state = XdgShellState::new::<Self>(&dh);
        let layer_shell_state = WlrLayerShellState::new::<Self>(&dh);
        let shm_state = ShmState::new::<Self>(&dh, vec![]);
        let output_manager_state = OutputManagerState::new_with_xdg_output::<Self>(&dh);
        let mut seat_state = SeatState::new();
        let data_device_state = DataDeviceState::new::<Self>(&dh);
        let popups = PopupManager::default();
        let cursor_shape_state = CursorShapeManagerState::new::<Self>(&dh);
        let protocols = Protocols::new(&dh);
        let foreign_toplevels = ForeignToplevelState::new(&dh);
        let xwayland_shell_state = XWaylandShellState::new::<Self>(&dh);

        // A seat is a group of keyboards, pointer and touch devices.
        // A seat typically has a pointer and maintains a keyboard focus and a pointer focus.
        let mut seat: Seat<Self> = seat_state.new_wl_seat(&dh, "winit");

        // Notify clients that we have a keyboard, for the sake of the example we assume that keyboard is always present.
        // You may want to track keyboard hot-plug in real compositor.
        seat.add_keyboard(Default::default(), 200, 25).unwrap();

        // Notify clients that we have a pointer (mouse)
        // Here we assume that there is always pointer plugged in
        seat.add_pointer();

        // A space represents a two-dimensional plane. Windows and Outputs can be mapped onto it.
        //
        // Windows get a position and stacking order through mapping.
        // Outputs become views of a part of the Space and can be rendered via Space::render_output.
        let space = Space::default();

        let socket_name = Self::init_wayland_listener(display, event_loop);

        // Get the loop signal, used to stop the event loop
        let loop_signal = event_loop.get_signal();

        Self {
            start_time,
            config: Config::load(),
            display_handle: dh,

            space,
            minimized: Vec::new(),
            overview: None,
            workspaces: Workspaces::new(),
            apps: Apps::default(),
            super_tap: false,
            suppressed_keys: HashSet::new(),
            loop_signal,
            socket_name,

            compositor_state,
            xdg_shell_state,
            layer_shell_state,
            shm_state,
            output_manager_state,
            seat_state,
            data_device_state,
            popups,
            cursor_shape_state,
            protocols,
            foreign_toplevels,
            dmabuf_state: DmabufState::new(),
            dmabuf_import: None,
            switch_vt: None,
            xwayland_shell_state,
            xwm: None,
            cursor_status: CursorImageStatus::default_named(),
            seat,
        }
    }

    fn init_wayland_listener(
        display: Display<Nekoland>,
        event_loop: &mut EventLoop<CalloopData>,
    ) -> OsString {
        // Creates a new listening socket, automatically choosing the next available `wayland` socket name.
        let listening_socket = ListeningSocketSource::new_auto().unwrap();

        // Get the name of the listening socket.
        // Clients will connect to this socket.
        let socket_name = listening_socket.socket_name().to_os_string();

        let loop_handle = event_loop.handle();

        loop_handle
            .insert_source(listening_socket, move |client_stream, _, state| {
                // Inside the callback, you should insert the client into the display.
                //
                // You may also associate some data with the client when inserting the client.
                state
                    .display_handle
                    .insert_client(client_stream, Arc::new(ClientState::default()))
                    .unwrap();
            })
            .expect("Failed to init the wayland event source.");

        // You also need to add the display itself to the event loop, so that client events will be processed by wayland-server.
        loop_handle
            .insert_source(
                Generic::new(display, Interest::READ, Mode::Level),
                |_, display, state| {
                    // Safety: we don't drop the display
                    unsafe {
                        display.get_mut().dispatch_clients(&mut state.state).unwrap();
                    }
                    Ok(PostAction::Continue)
                },
            )
            .unwrap();

        socket_name
    }

    /// The layer surface of one of `layers` at a position, with where it is.
    pub fn layer_under(
        &self,
        pos: Point<f64, Logical>,
        layers: [Layer; 2],
    ) -> Option<(LayerSurface, Point<i32, Logical>)> {
        let output = self.space.output_under(pos).next()?;
        let output_loc = self.space.output_geometry(output)?.loc;
        let map = layer_map_for_output(output);
        let local = pos - output_loc.to_f64();
        let layer = layers
            .into_iter()
            .find_map(|layer| map.layer_under(layer, local))?;
        let layer_loc = map.layer_geometry(layer)?.loc;
        Some((layer.clone(), layer_loc + output_loc))
    }

    /// The surface at a position: overlay and top layers come before windows, the
    /// bottom and background layers after them.
    pub fn surface_under(&self, pos: Point<f64, Logical>) -> Option<(WlSurface, Point<f64, Logical>)> {
        let layer_surface_under = |layers| {
            let (layer, location) = self.layer_under(pos, layers)?;
            layer
                .surface_under(pos - location.to_f64(), WindowSurfaceType::ALL)
                .map(|(s, p)| (s, (p + location).to_f64()))
        };
        let window_under = || {
            // In the overview, windows are thumbnails to pick, not something to point into.
            if self.overview.is_some() {
                return None;
            }
            self.space.element_under(pos).and_then(|(window, location)| {
                window
                    .surface_under(pos - location.to_f64(), WindowSurfaceType::ALL)
                    .map(|(s, p)| (s, (p + location).to_f64()))
            })
        };

        // A fullscreen window covers the panels; only the overlay layer stays above it.
        if self.overview.is_none() && self.fullscreen_window().is_some() {
            return layer_surface_under([Layer::Overlay, Layer::Overlay]).or_else(window_under);
        }
        layer_surface_under(ABOVE_WINDOWS)
            .or_else(window_under)
            .or_else(|| layer_surface_under(BELOW_WINDOWS))
    }

    /// What every backend does once an output has been drawn: tell its surfaces that a
    /// frame was shown, and drop what has gone away.
    pub fn finish_frame(&mut self, output: &Output) {
        let time = self.start_time.elapsed();
        let throttle = Some(Duration::ZERO);
        self.space
            .elements()
            .for_each(|window| window.send_frame(output, time, throttle, |_, _| Some(output.clone())));
        let mut layers = layer_map_for_output(output);
        layers
            .layers()
            .for_each(|layer| layer.send_frame(output, time, throttle, |_, _| Some(output.clone())));
        layers.cleanup();
        drop(layers);

        self.space.refresh();
        self.popups.cleanup();
        self.refresh_foreign_toplevels();
    }
}

#[derive(Default)]
pub struct ClientState {
    pub compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}
