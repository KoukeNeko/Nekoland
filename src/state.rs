use std::{collections::HashSet, ffi::OsString, sync::Arc, time::Duration};

use smithay::{
    backend::allocator::dmabuf::Dmabuf,
    desktop::{layer_map_for_output, utils::send_frames_surface_tree, LayerSurface, PopupManager, Space, Window, WindowSurfaceType},
    input::{keyboard::Keycode, pointer::{CursorImageStatus, MotionEvent}, Seat, SeatState},
    output::Output,
    reexports::{
        calloop::{generic::Generic, EventLoop, Interest, LoopHandle, LoopSignal, Mode, PostAction},
        wayland_server::{
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::wl_surface::WlSurface,
            Client, Display, DisplayHandle,
        },
    },
    utils::{Logical, Point, SERIAL_COUNTER},
    wayland::{
        compositor::{CompositorClientState, CompositorState},
        cursor_shape::CursorShapeManagerState,
        dmabuf::DmabufState,
        output::OutputManagerState,
        security_context::{SecurityContext, SecurityContextHandler, SecurityContextListenerSource},
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
    dock::Dock,
    handlers::{foreign_toplevel::ForeignToplevelState, protocols::Protocols},
    menubar::MenuBar,
    notices::Notices,
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
    /// The swaybg drawing the configured wallpaper.
    pub wallpaper: Option<std::process::Child>,
    /// The swaybg of the wallpaper before, until the new one has drawn over it.
    pub old_wallpaper: Option<std::process::Child>,
    /// The pointing devices libinput gave, to set as the configuration says.
    pub pointer_devices: Vec<smithay::reexports::input::Device>,
    /// Where threads send what they learn; the input method starts when first typed into.
    pub events: Option<smithay::reexports::calloop::channel::Sender<crate::dbus::Event>>,
    pub ime: Option<crate::ime::Ime>,
    pub socket_name: OsString,
    pub display_handle: DisplayHandle,
    pub loop_handle: LoopHandle<'static, CalloopData>,
    /// Set from when a locker locks the screen until it unlocks it.
    pub lock: Option<crate::lock::Lock>,
    pub locker_started: Option<std::time::Instant>,
    /// When sleep was asked for, to sleep once a locker has locked the screen.
    pub sleep_asked: Option<std::time::Instant>,
    pub last_input: std::time::Instant,
    /// Whether the displays are turned off, until there is input.
    pub screens_off: bool,
    /// Whether this is an xrdp session: drawn in the X server xrdp started, for a computer
    /// elsewhere, so this one is not put to sleep from it.
    pub remote: bool,
    /// Night Shift, as its setting, its schedule or the control center last said, and
    /// whether a display can show it.
    pub night_shift: bool,
    pub night_shift_available: bool,
    /// What the setting and schedule last said, for Night Shift to follow when it changes.
    pub night_shift_wanted: Option<bool>,
    /// Which corner the pointer is in, for a hot corner to act only as it comes in.
    pub in_hot_corner: Option<u8>,
    /// When a recorder last took a frame of the screen.
    pub screen_shared_at: Option<std::time::Instant>,
    /// Surfaces of programs that keep the screen from locking while it is idle.
    pub idle_inhibitors: Vec<WlSurface>,
    /// Screen captures waiting for their output to be drawn.
    pub screencopy: Vec<crate::screencopy::Frame>,

    pub space: Space<Window>,
    /// Minimized windows, with where they go back to.
    pub minimized: Vec<(Window, Point<i32, Logical>)>,
    /// The windows Show Desktop minimized, which it brings back when given again.
    pub desktop_shown: Vec<Window>,
    /// Which way up iio-sensor-proxy last said the computer is held.
    pub orientation: Option<String>,
    /// The turn last given the computer's own display for it, which the rotation lock holds.
    pub turned: Option<u16>,
    /// Every display plugged in, the main one first, shown or not; empty where the backend has
    /// only the outputs on the desktop.
    pub connected_outputs: Vec<Output>,
    /// Where the window being dragged goes if it is let go of now.
    pub snap: crate::windows::SnapPreview,
    pub overview: Option<Overview>,
    /// The applications' icons and names, drawn once and kept from one showing to the next.
    pub app_stock: crate::appgrid::Stock,
    /// Windows on their way somewhere, and the pictures of those just closed.
    pub motion: crate::motion::Motion,
    /// Takes a window's picture as it closes; set by the backend, which has the renderer.
    pub snapshooter: Option<crate::motion::Snapshooter>,
    pub menu_bar: MenuBar,
    pub dock: Dock,
    pub desktop: crate::desktop::Desktop,
    pub hover: crate::hover::Hover,
    pub notices: Notices,
    pub fps: crate::fps::FpsMeter,
    pub identify: crate::identify::Identify,
    pub workspaces: Workspaces,
    /// Names and icons of applications, for the overview.
    pub apps: Apps,
    /// Whether Super went down with nothing else pressed since.
    pub super_tap: bool,
    /// Keys whose press ran a shortcut, so their release is not sent to a client either.
    pub suppressed_keys: HashSet<Keycode>,
    pub sticky: crate::input::StickyKeys,
    /// When each key held back by `slow_keys_ms` was pressed.
    pub slow_pressed: std::collections::HashMap<Keycode, std::time::Instant>,
    /// When each key was last let go, for `bounce_keys_ms`.
    pub key_released_at: std::collections::HashMap<Keycode, std::time::Instant>,
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
    /// Draws what changed at once; set by a backend that otherwise draws only on its own clock.
    pub redraw: Option<smithay::reexports::calloop::ping::Ping>,
    /// Explicit sync for clients' GPU buffers; set by a backend whose GPU can wait on it.
    pub syncobj_state: Option<smithay::wayland::drm_syncobj::DrmSyncobjState>,
    /// Switches to another virtual terminal; set by the backend that owns a session.
    pub switch_vt: Option<Box<dyn FnMut(i32)>>,
    pub xwayland_shell_state: XWaylandShellState,
    /// The X11 window manager, once XWayland is up.
    pub xwm: Option<X11Wm>,
    /// The cursor the client under the pointer asked for.
    pub cursor_status: CursorImageStatus,
    /// What a client drags, drawn under the pointer, and where from the pointer it is.
    pub dnd_icon: Option<(WlSurface, Point<i32, Logical>)>,

    pub seat: Seat<Self>,
}

impl Nekoland {
    pub fn new(event_loop: &mut EventLoop<'static, CalloopData>, display: Display<Self>) -> Self {
        let start_time = std::time::Instant::now();

        let dh = display.handle();

        let compositor_state = CompositorState::new::<Self>(&dh);
        let xdg_shell_state = XdgShellState::new::<Self>(&dh);
        // Not to sandboxed programs: an overlay that takes the keyboard would read every key
        // and could pass for the lock screen.
        let layer_shell_state = WlrLayerShellState::new_with_filter::<Self, _>(&dh, unsandboxed);
        let shm_state = ShmState::new::<Self>(&dh, vec![]);
        let output_manager_state = OutputManagerState::new_with_xdg_output::<Self>(&dh);
        let mut seat_state = SeatState::new();
        let data_device_state = DataDeviceState::new::<Self>(&dh);
        let popups = PopupManager::default();
        let cursor_shape_state = CursorShapeManagerState::new::<Self>(&dh);
        let protocols = Protocols::new(&dh);
        crate::handlers::appmenu::create_globals(&dh);
        crate::screencopy::create_global(&dh);
        let foreign_toplevels = ForeignToplevelState::new(&dh);
        let xwayland_shell_state = XWaylandShellState::new::<Self>(&dh);

        // A seat is a group of keyboards, pointer and touch devices.
        // A seat typically has a pointer and maintains a keyboard focus and a pointer focus.
        let mut seat: Seat<Self> = seat_state.new_wl_seat(&dh, "winit");

        let config = Config::load();
        crate::text::set_family(&config.font);
        crate::text::set_icon_theme(&config.icon_theme);
        config.set_glass_look();
        // A layout XKB does not know is no reason to have no keyboard.
        let (delay, rate) = config.repeat();
        if let Err(err) = seat.add_keyboard(crate::config::xkb(&config.keyboard()), delay, rate) {
            tracing::warn!("keyboard layout {:?}: {err}", config.keyboard_layout);
            seat.add_keyboard(Default::default(), delay, rate).unwrap();
        }

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
            config,
            wallpaper: None,
            old_wallpaper: None,
            pointer_devices: Vec::new(),
            events: None,
            ime: None,
            display_handle: dh,
            loop_handle: event_loop.handle(),
            lock: None,
            locker_started: None,
            sleep_asked: None,
            last_input: start_time,
            screens_off: false,
            remote: std::env::var_os("XRDP_SESSION").is_some(),
            night_shift: false,
            night_shift_available: false,
            night_shift_wanted: None,
            in_hot_corner: None,
            screen_shared_at: None,
            idle_inhibitors: Vec::new(),
            screencopy: Vec::new(),

            space,
            minimized: Vec::new(),
            desktop_shown: Vec::new(),
            orientation: None,
            turned: None,
            connected_outputs: Vec::new(),
            snap: Default::default(),
            overview: None,
            app_stock: Default::default(),
            motion: Default::default(),
            snapshooter: None,
            menu_bar: MenuBar::default(),
            dock: Dock::new(),
            desktop: Default::default(),
            hover: Default::default(),
            notices: Notices::default(),
            fps: Default::default(),
            identify: Default::default(),
            workspaces: Workspaces::new(),
            apps: Apps::default(),
            super_tap: false,
            suppressed_keys: HashSet::new(),
            sticky: Default::default(),
            slow_pressed: std::collections::HashMap::new(),
            key_released_at: std::collections::HashMap::new(),
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
            syncobj_state: None,
            redraw: None,
            switch_vt: None,
            xwayland_shell_state,
            xwm: None,
            cursor_status: CursorImageStatus::default_named(),
            dnd_icon: None,
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
        if self.lock.is_some() {
            return self.lock_surface_under(pos);
        }
        // The menu bar and its menus are the compositor's own, above everything.
        if self.menu_bar_covers(pos)
            || self.dock_covers(pos)
            || self.notice_covers(pos)
            || self.hover_covers(pos)
        {
            return None;
        }
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

    /// Points the pointer, where it stands, into whatever is under it now: once what it was
    /// over has gone, a click or scroll without moving it first reaches what is left.
    pub fn repick_pointer(&mut self) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        if pointer.is_grabbed() {
            return;
        }
        let location = pointer.current_location();
        let under = self.surface_under(location);
        let motion = MotionEvent {
            location,
            serial: SERIAL_COUNTER.next_serial(),
            time: 0,
        };
        pointer.motion(self, under, &motion);
        pointer.frame(self);
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
        if let Some(surface) = self.lock_surface(output) {
            send_frames_surface_tree(surface.wl_surface(), output, time, throttle, |_, _| Some(output.clone()));
        }
        if let Some((icon, _)) = &self.dnd_icon {
            send_frames_surface_tree(icon, output, time, throttle, |_, _| Some(output.clone()));
        }

        self.space.refresh();
        self.popups.cleanup();
        self.refresh_foreign_toplevels();
    }
}

#[derive(Default)]
pub struct ClientState {
    pub compositor_state: CompositorClientState,
    /// Set for a client a sandbox such as Flatpak connected through its own socket.
    pub security_context: Option<SecurityContext>,
}

/// Whether a client may use what reaches other programs: their input, their clipboard,
/// their windows. Clients without a `ClientState`, such as XWayland, are Nekoland's own.
pub fn unsandboxed(client: &Client) -> bool {
    client
        .get_data::<ClientState>()
        .is_none_or(|data| data.security_context.is_none())
}

impl SecurityContextHandler for Nekoland {
    fn context_created(&mut self, source: SecurityContextListenerSource, context: SecurityContext) {
        let inserted = self.loop_handle.insert_source(source, move |stream, _, data| {
            let client = ClientState { security_context: Some(context.clone()), ..Default::default() };
            if let Err(err) = data.state.display_handle.insert_client(stream, Arc::new(client)) {
                tracing::warn!("sandboxed client: {err}");
            }
        });
        if let Err(err) = inserted {
            tracing::warn!("security context: {err}");
        }
    }
}
smithay::delegate_security_context!(Nekoland);

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}

impl Nekoland {
    /// Reads the configuration again and applies what changed that is not read anew each
    /// time it is used: the wallpaper, the keyboard and the pointing devices.
    /// Goes on to the next of Windows' four projections, as its Win+P does, and keeps it.
    pub fn next_projection(&mut self) {
        let all = crate::config::Projection::ALL;
        let now = all.iter().position(|projection| *projection == self.config.projection()).unwrap_or(0);
        let next = all[(now + 1) % all.len()];
        let Some(name) = toml::Value::try_from(next).ok().and_then(|value| value.as_str().map(str::to_owned)) else {
            return;
        };
        // Left on, it would read the projection put back to extending as duplicating.
        let kept = crate::config::unset("mirror_displays").and_then(|()| crate::config::set(&format!("projection = {name:?}")));
        if let Err(err) = kept {
            tracing::warn!("could not keep the projection: {err}");
        }
        self.reload_config();
    }

    pub fn reload_config(&mut self) {
        let old_wallpaper = (self.desktop_wallpaper(self.workspaces.active), self.config.wallpaper_mode);
        let old_keyboard = self.config.keyboard();
        let old_font = std::mem::take(&mut self.config.font);
        self.config = Config::load();
        self.turn_with_orientation();
        // What the bar and the Dock show may have changed with it.
        self.dock.drawn = None;
        self.menu_bar.drawn = None;
        // Icons already drawn are kept by name; the theme, or the files in it, may be new.
        self.dock.icons.clear();
        self.app_stock.clear();
        // So are the desktop entries, as of an application installed since it was first seen.
        self.apps = Apps::default();
        crate::text::set_icon_theme(&self.config.icon_theme);
        self.draw_lock_shortcuts();
        if self.config.font != old_font {
            // shortcut: other text than the bar's and the Dock's keeps the old font until it is
            // next drawn.
            crate::text::set_family(&self.config.font);
        }
        let keyboard = self.seat.get_keyboard().unwrap();
        let (delay, rate) = self.config.repeat();
        keyboard.change_repeat_info(rate, delay);
        let names = self.config.keyboard();
        if names != old_keyboard {
            if let Err(err) = keyboard.set_xkb_config(self, crate::config::xkb(&names)) {
                tracing::warn!("keyboard layout {:?}: {err}", self.config.keyboard_layout);
            }
        }
        // Menus and the like take the glass's new look as they next open.
        self.config.set_glass_look();
        if old_wallpaper != (self.desktop_wallpaper(self.workspaces.active), self.config.wallpaper_mode) {
            self.show_wallpaper();
        }
        let devices = std::mem::take(&mut self.pointer_devices);
        for device in devices {
            self.add_pointer_device(device);
        }
    }

    /// Magnifies the screen more or less, and keeps the level for next time, without reading
    /// the rest of the configuration again.
    pub fn zoom_by(&mut self, step: f64) {
        let zoom = (self.config.zoom + step).clamp(1.0, crate::render::ZOOM_MOST);
        if zoom == self.config.zoom {
            return;
        }
        self.config.zoom = zoom;
        if let Err(err) = crate::config::set(&format!("zoom = {zoom:?}")) {
            tracing::warn!("zoom: {err}");
        }
    }

    pub fn stop_old_wallpaper(&mut self) {
        if let Some(mut swaybg) = self.old_wallpaper.take() {
            if let Err(err) = swaybg.kill().and_then(|()| swaybg.wait().map(|_| ())) {
                tracing::warn!("swaybg: {err}");
            }
        }
    }

    /// Sets a pointing device as the configuration says, and keeps it for when that changes.
    pub fn add_pointer_device(&mut self, mut device: smithay::reexports::input::Device) {
        use smithay::reexports::input::{AccelProfile, DeviceCapability};
        if !device.has_capability(DeviceCapability::Pointer) {
            return;
        }
        // A device that cannot do one of these says so, and is left as it is.
        let _ = device.config_tap_set_enabled(self.config.tap_to_click);
        let _ = device.config_scroll_set_natural_scroll_enabled(self.config.natural_scroll);
        let _ = device.config_accel_set_speed(self.config.pointer_speed.clamp(-1.0, 1.0));
        let _ = device.config_accel_set_profile(if self.config.pointer_acceleration {
            AccelProfile::Adaptive
        } else {
            AccelProfile::Flat
        });
        let _ = device.config_left_handed_set(self.config.left_handed);
        let _ = device.config_dwt_set_enabled(self.config.disable_while_typing);
        self.pointer_devices.retain(|known| *known != device);
        self.pointer_devices.push(device);
    }
}
