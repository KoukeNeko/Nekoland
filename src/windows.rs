//! Where windows go and which one has the keyboard.

use std::cell::{Cell, RefCell};

use smithay::{
    backend::renderer::element::Id,
    desktop::{find_popup_root_surface, layer_map_for_output, Window, WindowSurface},
    input::keyboard::Keysym,
    output::Output,
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_toplevel::State as ToplevelState,
        wayland_server::{protocol::wl_surface::WlSurface, Resource},
    },
    utils::{IsAlive, Logical, Point, Rectangle, Size, SERIAL_COUNTER},
    wayland::{
        compositor::{get_parent, with_states},
        seat::WaylandFocus,
        shell::xdg::{SurfaceCachedState, XdgToplevelSurfaceData},
    },
};

use crate::{
    arrange::{self, Arrangement, Part, MAX_GAP},
    config::DockPosition,
    dock::dock_space,
    focus::KeyFocus,
    menubar::MENU_BAR_HEIGHT,
    Nekoland,
};

/// The application ID of Nekoland's own launcher.
pub const LAUNCHER_APP_ID: &str = "nekoland-launcher";
/// The application ID of Nekoland's confirmation dialog, which draws its own opaque card
/// and shadow, so it needs no glass.
pub const CONFIRM_APP_ID: &str = "nekoland-confirm";
/// The title of 截圖's full-screen window for choosing what to take: it shows a still
/// picture of the screen, so it appears and goes at once, without the windows' motion.
pub const SHOT_OVERLAY_TITLE: &str = "截圖範圍";
/// The application ID of the on-screen keyboard, kept in front and never given the keyboard.
pub const KEYBOARD_APP_ID: &str = "io.github.koukeneko.Nekoland.Keyboard";
/// A window's title without the application's name it ends in, for where the application is
/// already shown: "Cargo.toml - Text Editor" under Text Editor's icon or in its menu.
pub fn title_without_app(title: &str, app: &str) -> String {
    [" - ", " — "]
        .iter()
        .find_map(|dash| title.strip_suffix(&format!("{dash}{app}")))
        .unwrap_or(title)
        .to_string()
}

/// Between the on-screen keyboard and the Dock under it.
const KEYBOARD_GAP: i32 = 12;
/// The application ID of a picture 截圖 pinned to the screen, kept in front of the windows.
pub const PIN_APP_ID: &str = "io.github.koukeneko.Nekoland.ShotPin";

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

/// Whether a window is an X11 one that draws its own decoration: its rounded corners and
/// shadow are its own, and X11 says nothing of where its body is, so glass, which is square
/// behind an X11 window, would show past its corners.
pub fn is_x11_drawing_own_frame(window: &Window) -> bool {
    matches!(window.underlying_surface(), WindowSurface::X11(surface) if surface.is_decorated())
}

/// Whether the compositor rounds the corners of a window: an X11 program's own, with a
/// frame of its own, which it draws square, left where it was put. Maximized, fullscreen
/// and arranged ones are square, as GTK's are.
pub fn rounds_x11_corners(window: &Window) -> bool {
    matches!(window.underlying_surface(), WindowSurface::X11(surface)
        if surface.is_decorated() && crate::frame::is_own_window(surface))
        && matches!(placement_of(window), Placement::Free)
}

/// The smallest and largest size a window says it can be; 0 is no limit.
pub fn size_limits(window: &Window) -> (Size<i32, Logical>, Size<i32, Logical>) {
    match window.underlying_surface() {
        WindowSurface::Wayland(toplevel) => with_states(toplevel.wl_surface(), |states| {
            let mut cached = states.cached_state.get::<SurfaceCachedState>();
            let limits = cached.current();
            (limits.min_size, limits.max_size)
        }),
        WindowSurface::X11(surface) => (
            surface.min_size().unwrap_or_default(),
            surface.max_size().unwrap_or_default(),
        ),
    }
}

/// Whether a window has one size only, as a program's welcome or about window may: it is
/// left at that size, as macOS and GNOME leave it, not maximized, tiled or filled.
fn has_fixed_size(window: &Window) -> bool {
    let (min, max) = size_limits(window);
    max.w > 0 && max.h > 0 && min == max
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
    /// The part a window command last put the window in, to go on to the next size when
    /// the same command comes again.
    arranged: RefCell<Option<Arranged>>,
    /// Whether the window fills a part of the display, as a half or maximized, so that
    /// dragging it out brings back its earlier size.
    snapped: Cell<bool>,
}

struct Arranged {
    command: String,
    step: usize,
    at: Point<i32, Logical>,
}

fn data(window: &Window) -> &WindowData {
    window.user_data().get_or_insert(WindowData::default)
}

/// Where a window being dragged will go if it is let go of where it is, shown under it.
pub struct SnapPreview {
    pub target: Option<SnapTarget>,
    /// The fill and the four sides of the outline, which keep their ids from frame to frame.
    pub ids: [Id; 5],
}

pub struct SnapTarget {
    pub window: Window,
    pub command: &'static str,
    pub rect: Rectangle<i32, Logical>,
}

impl Default for SnapPreview {
    fn default() -> Self {
        Self {
            target: None,
            ids: std::array::from_fn(|_| Id::new()),
        }
    }
}

/// How a window was put on the display.
enum Placement {
    Fullscreen,
    Maximized,
    /// In a part of it, by a window command.
    Arranged,
    Free,
}

fn placement_of(window: &Window) -> Placement {
    let (fullscreen, maximized) = match window.underlying_surface() {
        WindowSurface::Wayland(toplevel) => {
            let states = toplevel.current_state().states;
            (states.contains(ToplevelState::Fullscreen), states.contains(ToplevelState::Maximized))
        }
        WindowSurface::X11(surface) => (surface.is_fullscreen(), surface.is_maximized()),
    };
    if fullscreen {
        Placement::Fullscreen
    } else if maximized {
        Placement::Maximized
    } else if data(window).arranged.borrow().is_some() {
        Placement::Arranged
    } else {
        Placement::Free
    }
}

/// `rect` moved, and shrunk where it is larger, to lie inside `area`.
fn keep_inside(rect: Rectangle<i32, Logical>, area: Rectangle<i32, Logical>) -> Rectangle<i32, Logical> {
    let width = rect.size.w.min(area.size.w);
    let height = rect.size.h.min(area.size.h);
    let x = rect.loc.x.clamp(area.loc.x, area.loc.x + area.size.w - width);
    let y = rect.loc.y.clamp(area.loc.y, area.loc.y + area.size.h - height);
    Rectangle::new((x, y).into(), (width, height).into())
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
    /// The output `point` is on, or the main one off them all.
    pub fn output_at(&self, point: Point<f64, Logical>) -> Option<Output> {
        self.space.output_under(point).next().or_else(|| self.space.outputs().next()).cloned()
    }

    /// The output the middle of a window is on, or the main one.
    pub(crate) fn output_of(&self, window: &Window) -> Option<Output> {
        let frame = self.frame(window)?;
        self.output_at((frame.loc + frame.size.downscale(2).to_point()).to_f64())
    }

    /// The part of the output under the pointer that panels leave free, where new windows open.
    pub fn usable_area(&self) -> Option<Rectangle<i32, Logical>> {
        let pointer = self.seat.get_pointer()?.current_location();
        self.usable_area_on(&self.output_at(pointer)?)
    }

    /// The part of `output` that panels leave free; every output has a menu bar, the main
    /// one the Dock as well.
    pub fn usable_area_on(&self, output: &Output) -> Option<Rectangle<i32, Logical>> {
        let geometry = self.space.output_geometry(output)?;
        let zone = layer_map_for_output(output).non_exclusive_zone();
        let mut area = Rectangle::new(geometry.loc + zone.loc, zone.size);
        // The menu bar keeps the top for itself, the Dock the bottom.
        let below_bar = geometry.loc.y + MENU_BAR_HEIGHT;
        if area.loc.y < below_bar {
            area.size.h -= below_bar - area.loc.y;
            area.loc.y = below_bar;
        }
        if self.space.outputs().next() != Some(output) {
            return Some(area);
        }
        // A Dock that hides leaves its edge to windows, as on the Mac.
        let dock = if self.config.dock_autohide {
            0
        } else {
            dock_space(self.config.dock_size)
        };
        match self.config.dock_position {
            DockPosition::Bottom => {
                let above_dock = geometry.loc.y + geometry.size.h - dock;
                if area.loc.y + area.size.h > above_dock {
                    area.size.h = above_dock - area.loc.y;
                }
            }
            DockPosition::Left => {
                let right_of_dock = geometry.loc.x + dock;
                if area.loc.x < right_of_dock {
                    area.size.w -= right_of_dock - area.loc.x;
                    area.loc.x = right_of_dock;
                }
            }
            DockPosition::Right => {
                let left_of_dock = geometry.loc.x + geometry.size.w - dock;
                if area.loc.x + area.size.w > left_of_dock {
                    area.size.w = left_of_dock - area.loc.x;
                }
            }
        }
        Some(area)
    }

    /// With `focus_follows_pointer`, the keyboard goes from window to window as the pointer
    /// moves into them, none raised, and stays when it leaves for the desktop or a panel.
    pub fn focus_under_pointer(&mut self, under: Option<&WlSurface>) {
        let grabbed = self.seat.get_pointer().is_some_and(|pointer| pointer.is_grabbed());
        // What has the keyboard besides windows, a launcher or the lock, keeps it.
        let on_window = self.seat.get_keyboard().and_then(|keyboard| keyboard.current_focus()).is_none()
            || self.focused_window().is_some();
        if !self.config.focus_follows_pointer || grabbed || !on_window {
            return;
        }
        let Some(mut root) = under.cloned() else {
            return;
        };
        while let Some(parent) = get_parent(&root) {
            root = parent;
        }
        if let Some(window) = self.window_for_surface(&root).filter(is_managed) {
            if self.focused_window().as_ref() != Some(&window) && window_names(&window).1 != KEYBOARD_APP_ID {
                self.set_keyboard_focus(Some(key_focus_of(&window)));
            }
        }
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
        let mut focus = self.seat.get_keyboard()?.current_focus()?;
        // A menu open in a window has the keyboard for it; the window stays the focused one.
        if let KeyFocus::Wayland(surface) = &focus {
            if let Some(root) = self
                .popups
                .find_popup(surface)
                .and_then(|popup| find_popup_root_surface(&popup).ok())
            {
                focus = KeyFocus::Wayland(root);
            }
        }
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
        // An X11 program's dialogs are windows of their own that name the one they are for,
        // and that one waits for them: they stay in front of it, and have the keyboard.
        let dialogs: Vec<Window> = window.x11_surface().map_or_else(Vec::new, |surface| {
            let id = surface.window_id();
            self.space
                .elements()
                .filter(|other| other.x11_surface().is_some_and(|dialog| dialog.is_transient_for() == Some(id)))
                .cloned()
                .collect()
        });
        for dialog in &dialogs {
            self.space.raise_element(dialog, true);
            if let (Some(xwm), Some(surface)) = (self.xwm.as_mut(), dialog.x11_surface()) {
                let _ = xwm.raise_window(surface);
            }
        }
        let window = dialogs.last().unwrap_or(window);
        self.raise_pins();
        // The on-screen keyboard types into the window that has the keyboard, so it never
        // takes it, whatever asks: a click, its own activation, the switcher.
        if window_names(window).1 != KEYBOARD_APP_ID {
            self.set_keyboard_focus(Some(key_focus_of(window)));
        }
    }

    /// Puts the pictures 截圖 pinned back in front of whatever was raised.
    pub fn raise_pins(&mut self) {
        let pins: Vec<Window> = self
            .space
            .elements()
            .filter(|window| [PIN_APP_ID, KEYBOARD_APP_ID].contains(&window_names(window).1.as_str()))
            .cloned()
            .collect();
        for pin in &pins {
            self.space.raise_element(pin, false);
        }
    }

    /// Hands the keyboard to the topmost window, when whatever had it went away.
    pub fn focus_top_window(&mut self) {
        let top = self
            .space
            .elements()
            .rev()
            .find(|window| window.alive() && is_managed(window) && window_names(window).1 != KEYBOARD_APP_ID)
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

        // One that asked to be fullscreen or maximized before it was shown is in place already.
        let free = matches!(placement_of(window), Placement::Free | Placement::Arranged);
        if let Some(area) = self.usable_area().filter(|_| free) {
            let x = area.loc.x + (area.size.w - size.w).max(0) / 2;
            let app_id = window_names(window).1;
            // The launcher opens down from a fifth of the way, as its results come; the
            // on-screen keyboard sits at the bottom, over the Dock.
            let y = if app_id == LAUNCHER_APP_ID {
                area.loc.y + area.size.h / 5
            } else if app_id == KEYBOARD_APP_ID {
                area.loc.y + (area.size.h - size.h - KEYBOARD_GAP).max(0)
            } else {
                area.loc.y + (area.size.h - size.h).max(0) / 2
            };
            self.space.map_element(window.clone(), (x, y), false);
        }
        self.focus_window(window);
        self.motion_open(window);
    }

    fn remember_place(&self, window: &Window) {
        let mut restore = data(window).restore.borrow_mut();
        if restore.is_none() {
            if let Some(location) = self.space.element_location(window) {
                *restore = Some(self.with_title(window, Rectangle::new(location, window.geometry().size)));
            }
        }
    }

    /// Puts a window in `rect`, which holds the title bar that goes over an X11 window, but
    /// not over one made fullscreen.
    fn put_window(&mut self, window: &Window, rect: Rectangle<i32, Logical>, states: &[ToplevelState]) {
        let rect = if states.contains(&ToplevelState::Fullscreen) {
            rect
        } else {
            self.without_title(window, rect)
        };
        self.motion_morph(window, rect);
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
        let area = self.output_of(window).and_then(|output| self.usable_area_on(&output));
        let Some(area) = area.filter(|_| !has_fixed_size(window)) else {
            return;
        };
        self.remember_place(window);
        self.put_window(window, area, &[ToplevelState::Maximized]);
        mark(window, None, true);
    }

    /// Where a window is and how large, its shadow left out: the size it was last asked to
    /// be, so that a command given again before it has been drawn at it goes on from there.
    fn frame(&self, window: &Window) -> Option<Rectangle<i32, Logical>> {
        let asked = window
            .toplevel()
            .and_then(|toplevel| toplevel.with_pending_state(|pending| pending.size))
            .filter(|size| size.w > 0 && size.h > 0);
        let size = asked.unwrap_or_else(|| window.geometry().size);
        Some(self.with_title(window, Rectangle::new(self.space.element_location(window)?, size)))
    }

    /// The points left between windows put side by side, and around them.
    fn window_gap(&self) -> i32 {
        i32::try_from(self.config.window_gap).unwrap_or(MAX_GAP).min(MAX_GAP)
    }

    /// Does to the active window what a window command names, as the launcher, the menu bar
    /// and the shortcuts give them; a half or a quarter given again in a row goes on to its
    /// next size. Returns whether `command` is one.
    pub fn window_command(&mut self, command: &str) -> bool {
        match self.focused_window().filter(is_managed) {
            Some(window) => self.arrange_window(&window, command, true),
            None => arrange::arrangement(command).is_some() || OTHER_COMMANDS.contains(&command),
        }
    }

    fn arrange_window(&mut self, window: &Window, command: &str, cycle: bool) -> bool {
        match command {
            "restore" => self.restore(window),
            "minimize" => self.minimize(window),
            "restore-or-minimize" => {
                if data(window).restore.borrow().is_some() {
                    self.restore(window);
                } else {
                    self.minimize(window);
                }
            }
            "display-left" => self.move_to_display(window, (-1, 0)),
            "display-right" => self.move_to_display(window, (1, 0)),
            "display-up" => self.move_to_display(window, (0, -1)),
            "display-down" => self.move_to_display(window, (0, 1)),
            "move" | "resize" => {
                if let Some(start) = self.frame(window).filter(|_| command == "move" || !has_fixed_size(window)) {
                    self.keyboard_frame = Some(KeyboardFrame {
                        window: window.clone(),
                        resize: command == "resize",
                        start,
                    });
                }
            }
            "fullscreen" => self.toggle_fullscreen(window),
            "close" => crate::menubar::close(window),
            _ => {
                let Some(arrangement) = arrange::arrangement(command) else {
                    return false;
                };
                self.apply_arrangement(window, command, arrangement, cycle);
            }
        }
        true
    }

    fn apply_arrangement(&mut self, window: &Window, command: &str, arrangement: Arrangement, cycle: bool) {
        let area = self.output_of(window).and_then(|output| self.usable_area_on(&output));
        let (Some(area), Some(frame)) = (area, self.frame(window)) else {
            return;
        };
        let inner = arrange::inset(area, self.window_gap());
        let resizes = !matches!(arrangement, Arrangement::Center | Arrangement::Move(_));
        if resizes && has_fixed_size(window) {
            return;
        }
        match arrangement {
            Arrangement::Maximize => self.maximize(window),
            Arrangement::Part(part) => {
                let step = if cycle { next_step(window, command, frame.loc) } else { 0 };
                self.remember_place(window);
                self.put_in_part(window, area, command, arrange::cycled(part, step), step);
            }
            Arrangement::MaximizeHeight => self.reframe(window, arrange::full_height(inner, frame), true),
            Arrangement::Center => self.reframe(window, arrange::centered(inner, frame.size), false),
            Arrangement::Resize(by) => self.reframe(window, arrange::resized(inner, frame, by), false),
            Arrangement::Move(direction) => self.reframe(window, arrange::moved(inner, frame, direction), false),
        }
    }

    /// Puts a window in a part of `area`, as `command` at its `step` named it.
    fn put_in_part(&mut self, window: &Window, area: Rectangle<i32, Logical>, command: &str, part: Part, step: usize) {
        if has_fixed_size(window) {
            return;
        }
        let rect = arrange::part_rect(area, part, self.window_gap());
        // Without gaps, the sides it is against are tiled, which GTK draws square.
        let states = if self.window_gap() == 0 {
            tiled_states(area, rect)
        } else {
            Vec::new()
        };
        self.put_window(window, rect, &states);
        let arranged = Arranged {
            command: command.to_string(),
            step,
            at: rect.loc,
        };
        mark(window, Some(arranged), true);
    }

    /// Puts a window a command put in a part of the display back in that part, as the
    /// display now is.
    fn rearrange(&mut self, window: &Window, area: Rectangle<i32, Logical>) {
        let Some((command, step)) = data(window)
            .arranged
            .borrow()
            .as_ref()
            .map(|arranged| (arranged.command.clone(), arranged.step))
        else {
            return;
        };
        if let Some(Arrangement::Part(part)) = arrange::arrangement(&command) {
            self.put_in_part(window, area, &command, arrange::cycled(part, step), step);
        }
    }

    /// Puts a window somewhere a command worked out from where it was.
    fn reframe(&mut self, window: &Window, rect: Rectangle<i32, Logical>, snapped: bool) {
        self.remember_place(window);
        self.put_window(window, rect, &[]);
        mark(window, None, snapped);
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

    pub fn toggle_fullscreen(&mut self, window: &Window) {
        if self.is_fullscreen(window) {
            self.restore(window);
        } else {
            self.fullscreen(window);
        }
    }

    /// The window that covers the whole output, panels included: the topmost one, if it is fullscreen.
    pub fn fullscreen_window(&self) -> Option<Window> {
        let top = self.space.elements().next_back()?;
        self.is_fullscreen(top).then(|| top.clone())
    }

    pub fn fullscreen(&mut self, window: &Window) {
        let Some(geometry) = self
            .output_of(window)
            .and_then(|output| self.space.output_geometry(&output))
            .filter(|_| !has_fixed_size(window))
        else {
            return;
        };
        self.remember_place(window);
        self.put_window(window, geometry, &[ToplevelState::Fullscreen]);
    }

    /// Fits the windows to the display once its size, scale or rotation changed: those that
    /// filled it or a part of it fill it again, and the others, with the places they go back
    /// to, are moved and shrunk to lie inside it, so that none is left partly off screen.
    // shortcut: windows on the workspaces not shown are fitted only as they are put back.
    pub fn fit_windows_to_display(&mut self) {
        let windows: Vec<Window> = self.space.elements().filter(|window| is_managed(window)).cloned().collect();
        for window in &windows {
            // One left on no output, its display unplugged, goes to the main one.
            let output = self.output_of(window);
            let display = output.as_ref().and_then(|output| self.space.output_geometry(output));
            let area = output.as_ref().and_then(|output| self.usable_area_on(output));
            let (Some(display), Some(area)) = (display, area) else {
                continue;
            };
            match placement_of(window) {
                Placement::Fullscreen => self.put_window(window, display, &[ToplevelState::Fullscreen]),
                Placement::Maximized => self.put_window(window, area, &[ToplevelState::Maximized]),
                Placement::Arranged => self.rearrange(window, area),
                Placement::Free => {
                    let Some(location) = self.space.element_location(window) else {
                        continue;
                    };
                    let rect = self.with_title(window, Rectangle::new(location, window.geometry().size));
                    let inside = keep_inside(rect, area);
                    if inside != rect {
                        self.put_window(window, inside, &[]);
                    }
                }
            }
            if let Some(rect) = data(window).restore.borrow_mut().as_mut() {
                *rect = keep_inside(*rect, area);
            }
        }
        let minimized: Vec<(Window, Point<i32, Logical>)> = self.minimized.clone();
        for (index, (window, location)) in minimized.iter().enumerate() {
            let rect = Rectangle::new(*location, window.geometry().size);
            let middle = (rect.loc + rect.size.downscale(2).to_point()).to_f64();
            if let Some(area) = self.output_at(middle).and_then(|output| self.usable_area_on(&output)) {
                self.minimized[index].1 = keep_inside(rect, area).loc;
            }
        }
    }

    /// Puts a maximized, tiled or fullscreen window back where it was.
    pub fn restore(&mut self, window: &Window) {
        let Some(rect) = data(window).restore.borrow_mut().take() else {
            return;
        };
        self.put_window(window, rect, &[]);
        mark(window, None, false);
    }

    /// Takes a window out of a part of the display or the whole of it, as its program asks
    /// to be no longer maximized or fullscreen. GTK asks as a drag of the title bar begins,
    /// just before asking to move the window: then it comes out at its earlier size under
    /// the pointer, as a drag by Nekoland does.
    pub fn restore_on_request(&mut self, window: &Window) {
        let lifted = self
            .drag_start_on(window)
            .and_then(|pressed| self.lift_dragged(window, pressed));
        if lifted.is_none() {
            self.restore(window);
        }
    }

    /// Where the button held on `window` went down, once the pointer has gone far enough
    /// from there to be dragging it rather than clicking.
    fn drag_start_on(&self, window: &Window) -> Option<Point<f64, Logical>> {
        let pointer = self.seat.get_pointer()?;
        if !pointer.is_grabbed() {
            return None;
        }
        let start = pointer.grab_start_data()?;
        let (pressed_on, _) = start.focus.as_ref()?;
        if !pressed_on.id().same_client_as(&window.wl_surface()?.id()) {
            return None;
        }
        let moved = pointer.current_location() - start.location;
        (moved.x.hypot(moved.y) >= DRAG_START_DISTANCE).then_some(start.location)
    }

    /// Brings a window dragged out of a half, or out of the whole display, back to the size
    /// it had before, still under the pointer that started the drag at `grabbed`. Returns
    /// where it is now, if it changed.
    pub fn lift_dragged(&mut self, window: &Window, grabbed: Point<f64, Logical>) -> Option<Point<i32, Logical>> {
        if !data(window).snapped.get() {
            return None;
        }
        let frame = self.frame(window)?;
        let earlier = (*data(window).restore.borrow())?;
        let lifted = Rectangle::new(arrange::lifted(frame, grabbed, earlier.size), earlier.size);
        self.put_window(window, lifted, &[]);
        // Kept as where it goes back to, since the client takes a while to draw it at that
        // size: snapped again before then, the window would remember the size of the half.
        *data(window).restore.borrow_mut() = Some(lifted);
        mark(window, None, false);
        Some(lifted.loc)
    }

    /// With `edge_tiling`, shows where a window being dragged goes if it is let go of with
    /// the pointer at `pos`.
    pub fn preview_snap(&mut self, window: &Window, pos: Point<f64, Logical>) {
        self.snap.target = self.snap_target(window, pos);
    }

    fn snap_target(&self, window: &Window, pos: Point<f64, Logical>) -> Option<SnapTarget> {
        if !self.config.edge_tiling {
            return None;
        }
        let output = self.output_at(pos)?;
        let display = self.space.output_geometry(&output)?;
        let command = arrange::snap_command(display.to_f64(), pos)?;
        let area = self.usable_area_on(&output)?;
        let rect = match arrange::arrangement(command)? {
            Arrangement::Part(part) => arrange::part_rect(area, part, self.window_gap()),
            _ => area,
        };
        Some(SnapTarget {
            window: window.clone(),
            command,
            rect,
        })
    }

    /// Puts a dragged window where its preview showed, when it was let go of over one.
    pub fn drop_dragged(&mut self, window: &Window) {
        if let Some(target) = self.snap.target.take().filter(|target| &target.window == window) {
            self.arrange_window(window, target.command, false);
        }
    }

    pub fn minimize(&mut self, window: &Window) {
        let Some(location) = self.space.element_location(window) else {
            return;
        };
        self.dock_minimizing(window, Rectangle::new(location, window.geometry().size));
        self.space.unmap_elem(window);
        self.minimized.push((window.clone(), location));
        self.focus_top_window();
        self.repick_pointer();
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
        self.space.map_element(window.clone(), location, true);
        // Out of the Dock, the Dock draws it growing from its tile.
        if !self.dock_draws(&window) {
            self.motion_reappear(&window);
        }
        true
    }

    /// Moves a window to the nearest display that way of its own, at the same place on it,
    /// smaller where it does not fit; a maximized or fullscreen one fills the other display
    /// instead.
    fn move_to_display(&mut self, window: &Window, (dx, dy): (i32, i32)) {
        let Some(from) = self.output_of(window) else {
            return;
        };
        let center = |output: &Output| self.space.output_geometry(output).map(|geometry| geometry.loc + geometry.size.downscale(2).to_point());
        let Some(here) = center(&from) else {
            return;
        };
        // Mostly that way: more along it than across it.
        let to = self
            .space
            .outputs()
            .filter(|output| **output != from)
            .filter_map(|output| Some((output, center(output)? - here)))
            .filter(|(_, d)| {
                (dx != 0 && d.x.signum() == dx && d.x.abs() >= d.y.abs())
                    || (dy != 0 && d.y.signum() == dy && d.y.abs() >= d.x.abs())
            })
            .min_by_key(|(_, d)| i64::from(d.x).pow(2) + i64::from(d.y).pow(2))
            .map(|(output, _)| output.clone());
        let Some(to) = to else {
            return;
        };
        let (Some(old), Some(new), Some(frame)) = (self.usable_area_on(&from), self.usable_area_on(&to), self.frame(window))
        else {
            return;
        };
        let shift = new.loc - old.loc;
        if let Some(restore) = data(window).restore.borrow_mut().as_mut() {
            restore.loc += shift;
        }
        if self.is_fullscreen(window) {
            if let Some(geometry) = self.space.output_geometry(&to) {
                self.put_window(window, geometry, &[ToplevelState::Fullscreen]);
            }
        } else if crate::menubar::is_maximized(window) {
            self.put_window(window, new, &[ToplevelState::Maximized]);
        } else {
            let size = Size::from((frame.size.w.min(new.size.w), frame.size.h.min(new.size.h)));
            let fraction = |at: i32, start: i32, room: i32| f64::from(at - start) / f64::from(room.max(1));
            let place = |at: i32, start: i32, room: i32, new_start: i32, new_room: i32, length: i32| {
                let along = (fraction(at, start, room) * f64::from(new_room)).round() as i32;
                (new_start + along).clamp(new_start, new_start + new_room - length)
            };
            let loc = Point::from((
                place(frame.loc.x, old.loc.x, old.size.w, new.loc.x, new.size.w, size.w),
                place(frame.loc.y, old.loc.y, old.size.h, new.loc.y, new.size.h, size.h),
            ));
            self.put_window(window, Rectangle::new(loc, size), &[]);
            mark(window, None, false);
        }
        self.focus_window(window);
    }

    /// Minimizes every window on the desktop, as Super+D does on Windows; given again, it
    /// brings back the ones it put away that are still in the Dock.
    pub fn toggle_show_desktop(&mut self) {
        let hidden = std::mem::take(&mut self.desktop_shown);
        let back: Vec<Window> = hidden.into_iter().filter(|window| self.unminimize(window)).collect();
        if let Some(top) = back.last() {
            self.focus_window(top);
            return;
        }
        let shown: Vec<Window> = self.space.elements().filter(|window| is_managed(window)).cloned().collect();
        for window in &shown {
            self.minimize(window);
        }
        self.desktop_shown = shown;
    }

    /// Goes on to the next window of the application that has the keyboard, as ⌘` does on
    /// the Mac: the one furthest back comes forward.
    pub fn focus_next_app_window(&mut self) {
        let Some(focused) = self.focused_window().filter(is_managed) else {
            return;
        };
        let app_id = window_names(&focused).1;
        let furthest = self
            .space
            .elements()
            .find(|window| is_managed(window) && window_names(window).1 == app_id)
            .cloned();
        if let Some(window) = furthest.filter(|window| *window != focused) {
            self.focus_window(&window);
        }
    }

    /// Takes an arrow key, Return or Esc for the window being moved or resized with the
    /// keyboard: ten points a press, one with Ctrl. Returns whether there is one.
    pub fn keyboard_frame_key(&mut self, sym: Keysym, ctrl: bool) -> bool {
        let Some(frame) = self.keyboard_frame.as_ref() else {
            return false;
        };
        let (window, resize, start) = (frame.window.clone(), frame.resize, frame.start);
        let step = if ctrl { 1 } else { 10 };
        let (dx, dy) = match sym {
            Keysym::Left => (-step, 0),
            Keysym::Right => (step, 0),
            Keysym::Up => (0, -step),
            Keysym::Down => (0, step),
            Keysym::Escape => {
                self.keyboard_frame = None;
                self.reframe(&window, start, false);
                return true;
            }
            _ => {
                // Return, or any other key, keeps it where it is.
                self.keyboard_frame = None;
                return matches!(sym, Keysym::Return | Keysym::KP_Enter | Keysym::space);
            }
        };
        let Some(mut rect) = self.frame(&window).filter(|_| window.alive()) else {
            self.keyboard_frame = None;
            return false;
        };
        if resize {
            let (least, most) = size_limits(&window);
            let limit = |length: i32, least: i32, most: i32| length.max(least.max(1)).min(if most > 0 { most } else { i32::MAX });
            rect.size = Size::from((limit(rect.size.w + dx, least.w, most.w), limit(rect.size.h + dy, least.h, most.h)));
        } else {
            rect.loc += Point::from((dx, dy));
        }
        self.reframe(&window, rect, false);
        true
    }

    /// Sends the window in front to the back and brings forward the one under it, as
    /// Alt+Esc does; `back` brings forward the one at the back instead.
    pub fn cycle_windows(&mut self, back: bool) {
        let stack: Vec<Window> = self.space.elements().filter(|window| is_managed(window)).cloned().collect();
        let [.., under, _] = stack.as_slice() else {
            return;
        };
        let next = if back {
            stack[0].clone()
        } else {
            let under = under.clone();
            // Raised in their order, all but the front one end up over it.
            for window in &stack[..stack.len() - 1] {
                self.space.raise_element(window, false);
            }
            under
        };
        self.focus_window(&next);
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
        self.motion_close(window);
        self.minimized.retain(|(minimized, _)| minimized != window);
        self.workspaces.forget(window);
        self.space.unmap_elem(window);
        if had_focus {
            self.focus_top_window();
        }
        self.repick_pointer();
    }
}


/// How far the pointer goes from where a button went down on a window before what its
/// program asks counts as the start of a drag, not a double-click.
const DRAG_START_DISTANCE: f64 = 4.0;

/// What a window command does besides putting the window in a part of the display.
pub(crate) const OTHER_COMMANDS: [&str; 11] = [
    "restore",
    "minimize",
    "fullscreen",
    "close",
    "restore-or-minimize",
    "display-left",
    "display-right",
    "display-up",
    "display-down",
    "move",
    "resize",
];

/// A window moved or resized with the arrow keys, as Alt+F7 and Alt+F8 start, and where it
/// was, for Esc to put it back.
pub struct KeyboardFrame {
    pub window: Window,
    pub resize: bool,
    start: Rectangle<i32, Logical>,
}

fn mark(window: &Window, arranged: Option<Arranged>, snapped: bool) {
    *data(window).arranged.borrow_mut() = arranged;
    data(window).snapped.set(snapped);
}

/// The step a command given to a window goes on to: the next, when the same command last
/// put the window where it still is, or the first.
fn next_step(window: &Window, command: &str, at: Point<i32, Logical>) -> usize {
    data(window)
        .arranged
        .borrow()
        .as_ref()
        .filter(|last| last.command == command && last.at == at)
        .map_or(0, |last| last.step + 1)
}

/// The tiled states for the sides of `area` that `rect` is against.
fn tiled_states(area: Rectangle<i32, Logical>, rect: Rectangle<i32, Logical>) -> Vec<ToplevelState> {
    let sides = [
        (rect.loc.x == area.loc.x, ToplevelState::TiledLeft),
        (rect.loc.x + rect.size.w == area.loc.x + area.size.w, ToplevelState::TiledRight),
        (rect.loc.y == area.loc.y, ToplevelState::TiledTop),
        (rect.loc.y + rect.size.h == area.loc.y + area.size.h, ToplevelState::TiledBottom),
    ];
    sides.into_iter().filter(|(against, _)| *against).map(|(_, state)| state).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_is_kept_inside_a_display_turned_on_its_side() {
        let rect = |x, y, width, height| Rectangle::<i32, Logical>::new((x, y).into(), (width, height).into());
        let portrait = rect(0, 30, 1440, 2460);
        assert_eq!(keep_inside(rect(1600, 200, 1200, 800), portrait), rect(240, 200, 1200, 800));
        assert_eq!(keep_inside(rect(100, 100, 2400, 1300), portrait), rect(0, 100, 1440, 1300));
        assert_eq!(keep_inside(rect(100, 100, 400, 300), portrait), rect(100, 100, 400, 300));
        assert_eq!(keep_inside(rect(-50, 0, 400, 300), portrait), rect(0, 30, 400, 300));
    }
}

#[cfg(test)]
mod title_tests {
    use super::title_without_app;

    #[test]
    fn drops_only_the_trailing_app_name() {
        assert_eq!(title_without_app("Cargo.toml (~/Nekoland) - Text Editor", "Text Editor"), "Cargo.toml (~/Nekoland)");
        assert_eq!(title_without_app("Text Editor - notes", "Text Editor"), "Text Editor - notes");
        assert_eq!(title_without_app("Calculator", "Calculator"), "Calculator");
    }
}
