//! Where windows go and which one has the keyboard.

use std::cell::{Cell, RefCell};

use smithay::{
    desktop::{layer_map_for_output, Window, WindowSurface},
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_toplevel::State as ToplevelState,
        wayland_server::protocol::wl_surface::WlSurface,
    },
    utils::{IsAlive, Logical, Point, Rectangle, SERIAL_COUNTER},
    wayland::{compositor::with_states, seat::WaylandFocus, shell::xdg::XdgToplevelSurfaceData},
};

use crate::{dock::dock_space, focus::KeyFocus, menubar::MENU_BAR_HEIGHT, Nekoland};

/// The application ID of Nekoland's own launcher.
pub const LAUNCHER_APP_ID: &str = "nekoland-launcher";
/// The application ID of Nekoland's confirmation dialog, which draws its own opaque card
/// and shadow, so it needs no glass.
pub const CONFIRM_APP_ID: &str = "nekoland-confirm";

/// Whether a window is drawn with square corners: one maximized, fullscreen or tiled, as
/// GTK draws them, or an X11 window, which draws none.
pub fn has_square_corners(window: &Window) -> bool {
    match window.underlying_surface() {
        WindowSurface::Wayland(toplevel) => {
            let states = toplevel.current_state().states;
            PLACEMENT_STATES.iter().any(|state| states.contains(*state))
        }
        WindowSurface::X11(_) => true,
    }
}

/// The states that say where a window was put, as opposed to what it is doing.
const PLACEMENT_STATES: [ToplevelState; 6] = [
    ToplevelState::Maximized,
    ToplevelState::Fullscreen,
    ToplevelState::TiledLeft,
    ToplevelState::TiledRight,
    ToplevelState::TiledTop,
    ToplevelState::TiledBottom,
];

#[derive(Default)]
struct WindowData {
    /// Where the window was before it was maximized, tiled or made fullscreen.
    restore: RefCell<Option<Rectangle<i32, Logical>>>,
    /// Whether the window has been given its first position.
    placed: Cell<bool>,
}

fn data(window: &Window) -> &WindowData {
    window.user_data().get_or_insert(WindowData::default)
}

#[derive(Clone, Copy)]
pub enum Side {
    Left,
    Right,
}

/// What gets the keyboard when a window does.
pub fn key_focus_of(window: &Window) -> KeyFocus {
    match window.underlying_surface() {
        WindowSurface::Wayland(toplevel) => KeyFocus::Wayland(toplevel.wl_surface().clone()),
        WindowSurface::X11(surface) => KeyFocus::X11(surface.clone()),
    }
}

/// A window's title and the identifier of its application.
pub fn window_names(window: &Window) -> (String, String) {
    match window.underlying_surface() {
        WindowSurface::Wayland(toplevel) => with_states(toplevel.wl_surface(), |states| {
            states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .map(|data| {
                    let data = data.lock().unwrap();
                    (
                        data.title.clone().unwrap_or_default(),
                        data.app_id.clone().unwrap_or_default(),
                    )
                })
                .unwrap_or_default()
        }),
        WindowSurface::X11(surface) => (surface.title(), surface.class()),
    }
}

/// Menus and tooltips of X11 programs are windows too, but not ones to list, switch to or arrange.
pub fn is_managed(window: &Window) -> bool {
    !window
        .x11_surface()
        .is_some_and(|surface| surface.is_override_redirect())
}

impl Nekoland {
    /// The part of the first output that panels leave free.
    // shortcut: one output; several need the output a window is on.
    pub fn usable_area(&self) -> Option<Rectangle<i32, Logical>> {
        let output = self.space.outputs().next()?;
        let geometry = self.space.output_geometry(output)?;
        let zone = layer_map_for_output(output).non_exclusive_zone();
        let mut area = Rectangle::new(geometry.loc + zone.loc, zone.size);
        // The menu bar keeps the top for itself, the Dock the bottom.
        let below_bar = geometry.loc.y + MENU_BAR_HEIGHT;
        if area.loc.y < below_bar {
            area.size.h -= below_bar - area.loc.y;
            area.loc.y = below_bar;
        }
        let above_dock = geometry.loc.y + geometry.size.h - dock_space(self.config.dock_size);
        if area.loc.y + area.size.h > above_dock {
            area.size.h = above_dock - area.loc.y;
        }
        Some(area)
    }

    pub fn window_for_surface(&self, surface: &WlSurface) -> Option<Window> {
        self.space
            .elements()
            .find(|window| window.wl_surface().as_deref() == Some(surface))
            .cloned()
    }

    /// Whether a surface is what the keyboard goes to.
    pub fn keyboard_is_on(&self, surface: &WlSurface) -> bool {
        self.seat
            .get_keyboard()
            .and_then(|keyboard| keyboard.current_focus())
            .is_some_and(|focus| focus.wl_surface().as_deref() == Some(surface))
    }

    pub fn focused_window(&self) -> Option<Window> {
        let focus = self.seat.get_keyboard()?.current_focus()?;
        self.space
            .elements()
            .find(|window| key_focus_of(window) == focus)
            .cloned()
    }

    /// Gives the keyboard to a window or a surface, or to nothing, and tells every
    /// window whether it is the active one.
    /// The launcher is for the moment it is asked for: it goes when anything else is
    /// clicked or given the keyboard.
    pub fn close_launcher_unless(&self, keep: Option<&Window>) {
        for window in self.space.elements() {
            if window_names(window).1 == LAUNCHER_APP_ID && Some(window) != keep {
                if let Some(toplevel) = window.toplevel() {
                    toplevel.send_close();
                }
            }
        }
    }

    pub fn set_keyboard_focus(&mut self, focus: Option<KeyFocus>) {
        // Nothing takes the keyboard from the locker.
        if self.lock.is_some() {
            return self.focus_lock();
        }
        let keep = self
            .space
            .elements()
            .find(|window| Some(key_focus_of(window)) == focus)
            .cloned();
        self.close_launcher_unless(keep.as_ref());
        for window in self.space.elements() {
            window.set_activated(Some(key_focus_of(window)) == focus);
            if let Some(toplevel) = window.toplevel() {
                toplevel.send_pending_configure();
            }
        }
        if let Some(keyboard) = self.seat.get_keyboard() {
            keyboard.set_focus(self, focus, SERIAL_COUNTER.next_serial());
        }
    }

    pub fn focus_window(&mut self, window: &Window) {
        // Whatever brought a window forward also picked it from the overview.
        if self.overview.is_some() {
            self.close_overview(Some(window.clone()));
            return;
        }
        self.raise_and_focus(window);
    }

    pub fn raise_and_focus(&mut self, window: &Window) {
        self.space.raise_element(window, true);
        if let (Some(xwm), Some(surface)) = (self.xwm.as_mut(), window.x11_surface()) {
            let _ = xwm.raise_window(surface);
        }
        self.set_keyboard_focus(Some(key_focus_of(window)));
    }

    /// Hands the keyboard to the topmost window, when whatever had it went away.
    pub fn focus_top_window(&mut self) {
        let top = self
            .space
            .elements()
            .rev()
            .find(|window| window.alive() && is_managed(window))
            .cloned();
        match top {
            Some(window) => self.focus_window(&window),
            None => self.set_keyboard_focus(None),
        }
    }

    /// Centers a window once its size is known, and gives it the keyboard.
    pub fn place_new_window(&mut self, window: &Window) {
        let size = window.geometry().size;
        // An X11 window is placed when it asks to be shown.
        if window.x11_surface().is_some() || data(window).placed.get() || size.w == 0 || size.h == 0 {
            return;
        }
        data(window).placed.set(true);

        if let Some(area) = self.usable_area() {
            let x = area.loc.x + (area.size.w - size.w).max(0) / 2;
            let y = area.loc.y + (area.size.h - size.h).max(0) / 2;
            self.space.map_element(window.clone(), (x, y), false);
        }
        self.focus_window(window);
    }

    fn remember_place(&self, window: &Window) {
        let mut restore = data(window).restore.borrow_mut();
        if restore.is_none() {
            if let Some(location) = self.space.element_location(window) {
                *restore = Some(Rectangle::new(location, window.geometry().size));
            }
        }
    }

    fn put_window(&mut self, window: &Window, rect: Rectangle<i32, Logical>, states: &[ToplevelState]) {
        match window.underlying_surface() {
            WindowSurface::Wayland(toplevel) => {
                toplevel.with_pending_state(|pending| {
                    PLACEMENT_STATES.iter().for_each(|state| {
                        pending.states.unset(*state);
                    });
                    states.iter().for_each(|state| {
                        pending.states.set(*state);
                    });
                    pending.size = Some(rect.size);
                });
                toplevel.send_pending_configure();
            }
            WindowSurface::X11(surface) => {
                let _ = surface.set_maximized(states.contains(&ToplevelState::Maximized));
                let _ = surface.set_fullscreen(states.contains(&ToplevelState::Fullscreen));
                let _ = surface.configure(rect);
            }
        }
        self.space.map_element(window.clone(), rect.loc, true);
    }

    pub fn maximize(&mut self, window: &Window) {
        let Some(area) = self.usable_area() else {
            return;
        };
        self.remember_place(window);
        self.put_window(window, area, &[ToplevelState::Maximized]);
    }

    pub fn tile(&mut self, window: &Window, side: Side) {
        let Some(area) = self.usable_area() else {
            return;
        };
        self.remember_place(window);
        let half = area.size.w / 2;
        let (x, width, edge) = match side {
            Side::Left => (area.loc.x, half, ToplevelState::TiledLeft),
            Side::Right => (area.loc.x + half, area.size.w - half, ToplevelState::TiledRight),
        };
        let rect = Rectangle::new(Point::from((x, area.loc.y)), (width, area.size.h).into());
        self.put_window(
            window,
            rect,
            &[edge, ToplevelState::TiledTop, ToplevelState::TiledBottom],
        );
    }

    pub fn is_fullscreen(&self, window: &Window) -> bool {
        match window.underlying_surface() {
            WindowSurface::Wayland(toplevel) => toplevel
                .current_state()
                .states
                .contains(ToplevelState::Fullscreen),
            WindowSurface::X11(surface) => surface.is_fullscreen(),
        }
    }

    /// The window that covers the whole output, panels included: the topmost one, if it is fullscreen.
    pub fn fullscreen_window(&self) -> Option<Window> {
        let top = self.space.elements().next_back()?;
        self.is_fullscreen(top).then(|| top.clone())
    }

    pub fn fullscreen(&mut self, window: &Window) {
        let Some(geometry) = self
            .space
            .outputs()
            .next()
            .and_then(|output| self.space.output_geometry(output))
        else {
            return;
        };
        self.remember_place(window);
        self.put_window(window, geometry, &[ToplevelState::Fullscreen]);
    }

    /// Puts a maximized, tiled or fullscreen window back where it was.
    pub fn restore(&mut self, window: &Window) {
        let Some(rect) = data(window).restore.borrow_mut().take() else {
            return;
        };
        self.put_window(window, rect, &[]);
    }

    pub fn minimize(&mut self, window: &Window) {
        let Some(location) = self.space.element_location(window) else {
            return;
        };
        self.dock_minimizing(window, Rectangle::new(location, window.geometry().size));
        self.space.unmap_elem(window);
        self.minimized.push((window.clone(), location));
        self.focus_top_window();
    }

    /// Brings a window back if it was minimized. Returns whether it was.
    pub fn unminimize(&mut self, window: &Window) -> bool {
        let Some(index) = self
            .minimized
            .iter()
            .position(|(minimized, _)| minimized == window)
        else {
            return false;
        };
        let (window, location) = self.minimized.remove(index);
        self.space.map_element(window, location, true);
        true
    }

    /// Moves on to the next window: minimized ones first, then the one at the bottom of the stack.
    // shortcut: a round robin over the stack, not most-recently-used order with a switcher.
    pub fn focus_next_window(&mut self) {
        self.minimized.retain(|(window, _)| window.alive());
        let next = match self.minimized.first() {
            Some((window, _)) => window.clone(),
            None => match self.space.elements().find(|window| is_managed(window)) {
                Some(window) => window.clone(),
                None => return,
            },
        };
        self.unminimize(&next);
        self.focus_window(&next);
    }

    /// Forgets the window of a surface whose client is gone.
    pub fn window_closed(&mut self, surface: &WlSurface) {
        let window = self
            .all_windows()
            .find(|window| window.wl_surface().as_deref() == Some(surface))
            .cloned();
        if let Some(window) = window {
            self.forget_window(&window);
        }
    }

    /// Takes a window off the desktop, and passes the keyboard on if it had it.
    pub fn forget_window(&mut self, window: &Window) {
        let had_focus = self.focused_window().as_ref() == Some(window);
        self.minimized.retain(|(minimized, _)| minimized != window);
        self.workspaces.forget(window);
        self.space.unmap_elem(window);
        if had_focus {
            self.focus_top_window();
        }
    }
}
