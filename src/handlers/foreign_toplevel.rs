//! wlr-foreign-toplevel-management: tells docks and taskbars which windows exist, and
//! lets them activate, minimize, maximize and close them.

use smithay::{
    desktop::{Window, WindowSurface},
    output::Output,
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_toplevel::State as ToplevelState,
        wayland_protocols_wlr::foreign_toplevel::v1::server::{
            zwlr_foreign_toplevel_handle_v1::{self, ZwlrForeignToplevelHandleV1},
            zwlr_foreign_toplevel_manager_v1::{self, ZwlrForeignToplevelManagerV1},
        },
        wayland_server::{
            backend::ClientId, Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
        },
    },
    utils::IsAlive,
};

use crate::{
    windows::{is_managed, window_names},
    Nekoland,
};

const VERSION: u32 = 3;

/// What a taskbar is told about a window.
#[derive(Clone, Default, PartialEq)]
struct Description {
    title: String,
    app_id: String,
    maximized: bool,
    minimized: bool,
    activated: bool,
    fullscreen: bool,
}

impl Description {
    /// The protocol's state array: one native-endian u32 per state that holds.
    fn states(&self) -> Vec<u8> {
        use zwlr_foreign_toplevel_handle_v1::State;
        [
            (self.maximized, State::Maximized),
            (self.minimized, State::Minimized),
            (self.activated, State::Activated),
            (self.fullscreen, State::Fullscreen),
        ]
        .into_iter()
        .filter(|(holds, _)| *holds)
        .flat_map(|(_, state)| (state as u32).to_ne_bytes())
        .collect()
    }
}

struct Entry {
    window: Window,
    description: Description,
    handles: Vec<ZwlrForeignToplevelHandleV1>,
}

impl Entry {
    fn send(&self, handle: &ZwlrForeignToplevelHandleV1, previous: Option<&Description>) {
        let now = &self.description;
        if previous.is_none_or(|previous| previous.title != now.title) {
            handle.title(now.title.clone());
        }
        if previous.is_none_or(|previous| previous.app_id != now.app_id) {
            handle.app_id(now.app_id.clone());
        }
        if previous.is_none_or(|previous| previous.states() != now.states()) {
            handle.state(now.states());
        }
        handle.done();
    }
}

#[derive(Default)]
pub struct ForeignToplevelState {
    managers: Vec<ZwlrForeignToplevelManagerV1>,
    entries: Vec<Entry>,
}

impl ForeignToplevelState {
    pub fn new(dh: &DisplayHandle) -> Self {
        dh.create_global::<Nekoland, ZwlrForeignToplevelManagerV1, _>(VERSION, ());
        Self::default()
    }
}

fn new_handle(
    dh: &DisplayHandle,
    manager: &ZwlrForeignToplevelManagerV1,
    entry: &Entry,
    outputs: &[Output],
) -> Option<ZwlrForeignToplevelHandleV1> {
    let client = manager.client()?;
    let handle = client
        .create_resource::<ZwlrForeignToplevelHandleV1, _, Nekoland>(
            dh,
            manager.version(),
            entry.window.clone(),
        )
        .ok()?;
    manager.toplevel(&handle);
    // Taskbars only list the windows of their own output.
    // shortcut: every window is said to be on every output; with several, track the one it is on.
    for output in outputs {
        output
            .client_outputs(&client)
            .for_each(|wl_output| handle.output_enter(&wl_output));
    }
    entry.send(&handle, None);
    Some(handle)
}

impl Nekoland {
    fn describe(&self, window: &Window) -> Description {
        let minimized = self.minimized.iter().any(|(minimized, _)| minimized == window);
        match window.underlying_surface() {
            WindowSurface::Wayland(toplevel) => {
                let (title, app_id) = window_names(window);
                let state = toplevel.current_state();
                Description {
                    title,
                    app_id,
                    maximized: state.states.contains(ToplevelState::Maximized),
                    minimized,
                    activated: state.states.contains(ToplevelState::Activated),
                    fullscreen: state.states.contains(ToplevelState::Fullscreen),
                }
            }
            WindowSurface::X11(surface) => Description {
                title: surface.title(),
                app_id: surface.class(),
                maximized: surface.is_maximized(),
                minimized,
                activated: surface.is_activated(),
                fullscreen: surface.is_fullscreen(),
            },
        }
    }

    /// Brings what taskbars know up to date with the windows there are.
    // shortcut: compared once per frame; fine for the handful of windows a desktop has.
    pub fn refresh_foreign_toplevels(&mut self) {
        let mut state = std::mem::take(&mut self.foreign_toplevels);

        state.entries.retain(|entry| {
            let alive = entry.window.alive();
            if !alive {
                entry.handles.iter().for_each(|handle| handle.closed());
            }
            alive
        });

        let windows: Vec<Window> = self
            .all_windows()
            .filter(|window| window.alive() && is_managed(window))
            .cloned()
            .collect();
        let outputs: Vec<Output> = self.space.outputs().cloned().collect();
        for window in windows {
            let description = self.describe(&window);
            match state.entries.iter_mut().find(|entry| entry.window == window) {
                Some(entry) if entry.description != description => {
                    let previous = std::mem::replace(&mut entry.description, description);
                    entry
                        .handles
                        .iter()
                        .for_each(|handle| entry.send(handle, Some(&previous)));
                }
                Some(_) => {}
                None => {
                    let mut entry = Entry {
                        window,
                        description,
                        handles: Vec::new(),
                    };
                    entry.handles = state
                        .managers
                        .iter()
                        .filter_map(|manager| new_handle(&self.display_handle, manager, &entry, &outputs))
                        .collect();
                    state.entries.push(entry);
                }
            }
        }

        self.foreign_toplevels = state;
    }
}

impl GlobalDispatch<ZwlrForeignToplevelManagerV1, ()> for Nekoland {
    fn bind(
        state: &mut Self,
        dh: &DisplayHandle,
        _client: &Client,
        resource: New<ZwlrForeignToplevelManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        let manager = data_init.init(resource, ());
        let outputs: Vec<Output> = state.space.outputs().cloned().collect();
        for entry in &mut state.foreign_toplevels.entries {
            if let Some(handle) = new_handle(dh, &manager, entry, &outputs) {
                entry.handles.push(handle);
            }
        }
        state.foreign_toplevels.managers.push(manager);
    }
}

impl Dispatch<ZwlrForeignToplevelManagerV1, ()> for Nekoland {
    fn request(
        state: &mut Self,
        _client: &Client,
        manager: &ZwlrForeignToplevelManagerV1,
        request: zwlr_foreign_toplevel_manager_v1::Request,
        _data: &(),
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        if let zwlr_foreign_toplevel_manager_v1::Request::Stop = request {
            manager.finished();
            state.foreign_toplevels.managers.retain(|known| known != manager);
        }
    }

    fn destroyed(state: &mut Self, _client: ClientId, manager: &ZwlrForeignToplevelManagerV1, _data: &()) {
        state.foreign_toplevels.managers.retain(|known| known != manager);
    }
}

impl Dispatch<ZwlrForeignToplevelHandleV1, Window> for Nekoland {
    fn request(
        state: &mut Self,
        _client: &Client,
        _handle: &ZwlrForeignToplevelHandleV1,
        request: zwlr_foreign_toplevel_handle_v1::Request,
        window: &Window,
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        use zwlr_foreign_toplevel_handle_v1::Request;
        if !window.alive() {
            return;
        }
        match request {
            Request::Activate { .. } | Request::UnsetMinimized => state.reveal_window(window),
            Request::SetMinimized => state.minimize(window),
            Request::SetMaximized => state.maximize(window),
            Request::SetFullscreen { .. } => state.fullscreen(window),
            Request::UnsetMaximized | Request::UnsetFullscreen => state.restore(window),
            Request::Close => match window.underlying_surface() {
                WindowSurface::Wayland(toplevel) => toplevel.send_close(),
                WindowSurface::X11(surface) => {
                    let _ = surface.close();
                }
            },
            // Where the taskbar shows the window; only an animation would use it.
            Request::SetRectangle { .. } | Request::Destroy => {}
            _ => {}
        }
    }

    fn destroyed(
        state: &mut Self,
        _client: ClientId,
        handle: &ZwlrForeignToplevelHandleV1,
        _window: &Window,
    ) {
        for entry in &mut state.foreign_toplevels.entries {
            entry.handles.retain(|known| known != handle);
        }
    }
}
