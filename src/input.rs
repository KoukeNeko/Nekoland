use smithay::{
    backend::input::{
        AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, InputBackend, InputEvent, KeyState,
        KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent, PointerMotionEvent,
    },
    desktop::{LayerSurface, Window, WindowSurface},
    input::{
        keyboard::{xkb, FilterResult, Keycode, Keysym, ModifiersState},
        pointer::{
            AxisFrame, ButtonEvent, Focus, GrabStartData, MotionEvent, PointerHandle, RelativeMotionEvent,
        },
    },
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Point, Serial, SERIAL_COUNTER},
    wayland::{
        compositor::{get_parent, RegionAttributes},
        keyboard_shortcuts_inhibit::KeyboardShortcutsInhibitorSeat,
        pointer_constraints::{with_pointer_constraint, PointerConstraint},
        seat::WaylandFocus,
        shell::wlr_layer::KeyboardInteractivity,
    },
};

use shortcuts::Combo;

use crate::{
    config,
    grabs::MoveSurfaceGrab,
    menubar::Command,
    state::{Nekoland, ABOVE_WINDOWS, BELOW_WINDOWS},
    windows::{app_id, KEYBOARD_APP_ID, LAUNCHER_APP_ID},
};

/// Sticky keys: a modifier let go with no other key pressed between stays on for the next key
/// pressed, which lets it go; pressed and let go again alone, it goes off.
#[derive(Default)]
pub struct StickyKeys {
    /// The modifier key pressed alone, and which modifier it is: shift, control, alt, logo.
    candidate: Option<(smithay::input::keyboard::Keycode, usize)>,
    latched: [bool; 4],
    /// Set when a key is pressed with modifiers latched, for them to be let go once it is sent.
    release_after: bool,
}

impl StickyKeys {
    fn modifier(sym: Keysym) -> Option<usize> {
        match sym {
            Keysym::Shift_L | Keysym::Shift_R => Some(0),
            Keysym::Control_L | Keysym::Control_R => Some(1),
            Keysym::Alt_L | Keysym::Alt_R => Some(2),
            Keysym::Super_L | Keysym::Super_R => Some(3),
            _ => None,
        }
    }

    fn state(&self) -> ModifiersState {
        let [shift, ctrl, alt, logo] = self.latched;
        ModifiersState { shift, ctrl, alt, logo, ..Default::default() }
    }

    /// Takes a key event in; the modifiers to latch from now on, when they change.
    fn key(
        &mut self,
        code: smithay::input::keyboard::Keycode,
        modifier: Option<usize>,
        pressed: bool,
        alone: bool,
    ) -> Option<ModifiersState> {
        match (modifier, pressed) {
            (Some(modifier), true) => {
                self.candidate = alone.then_some((code, modifier));
                None
            }
            (Some(modifier), false) => {
                if self.candidate.take() != Some((code, modifier)) {
                    return None;
                }
                self.latched[modifier] = !self.latched[modifier];
                Some(self.state())
            }
            (None, true) => {
                self.candidate = None;
                self.release_after = self.latched.contains(&true);
                None
            }
            (None, false) => None,
        }
    }
}

impl Nekoland {
    pub fn process_input_event<I: InputBackend>(&mut self, event: InputEvent<I>) {
        // What it changes is drawn once this round of events is through.
        if let Some(redraw) = &self.redraw {
            redraw.ping();
        }
        self.last_input = std::time::Instant::now();
        let released = matches!(&event, InputEvent::Keyboard { event } if event.state() == KeyState::Released);
        if !released {
            self.screens_off = false;
        }
        let locked = self.lock.is_some();
        match event {
            InputEvent::Keyboard { event, .. } => {
                let serial = SERIAL_COUNTER.next_serial();
                let time = Event::time_msec(&event);
                let key_state = event.state();

                self.seat.get_keyboard().unwrap().input::<(), _>(
                    self,
                    event.key_code(),
                    key_state,
                    serial,
                    time,
                    |state, modifiers, keysym| {
                        let code = keysym.raw_code();
                        let pressed = key_state == KeyState::Pressed;
                        // Locked or not: logind leaves it to Nekoland.
                        if keysym.modified_sym() == Keysym::XF86_PowerOff {
                            if pressed {
                                state.power_action(state.config.power_button);
                            }
                            return FilterResult::Intercept(());
                        }
                        if locked {
                            return state.locked_key(keysym.modified_sym(), pressed);
                        }
                        if state.ime_key(modifiers, &keysym, pressed, time) {
                            if pressed {
                                state.suppressed_keys.insert(code);
                            } else {
                                state.key_released(keysym.modified_sym());
                                state.suppressed_keys.remove(&code);
                            }
                            return FilterResult::Intercept(());
                        }
                        let bounce = std::time::Duration::from_millis(state.config.bounce_keys_ms.into());
                        if pressed && state.key_released_at.get(&code).is_some_and(|at| at.elapsed() < bounce) {
                            state.suppressed_keys.insert(code);
                            return FilterResult::Intercept(());
                        }
                        if !pressed && !bounce.is_zero() {
                            state.key_released_at.insert(code, std::time::Instant::now());
                        }
                        let modifier = keysym.raw_syms().first().copied().and_then(StickyKeys::modifier);
                        if state.config.sticky_keys {
                            let keyboard = state.seat.get_keyboard().unwrap();
                            let alone = keyboard.pressed_keys().len() <= 1;
                            if let Some(latched) = state.sticky.key(code, modifier, pressed, alone) {
                                keyboard.set_latched_modifiers(state, &latched);
                            }
                        }
                        let handled = match key_state {
                            KeyState::Pressed => {
                                // Letters by their plain symbol, so that Shift does not change them;
                                // the rest as the keymap gives them, which is where VT switching comes from.
                                let modified = keysym.modified_sym();
                                let sym = match keysym.raw_syms().first().copied() {
                                    Some(raw) if modified.key_char().is_some() => raw,
                                    _ => modified,
                                };
                                let handled =
                                    state.key_pressed(modifiers, sym, keysym.modified_sym().key_char());
                                if handled {
                                    state.suppressed_keys.insert(code);
                                } else if state.config.slow_keys_ms > 0 && modifier.is_none() {
                                    // Held back until it is let go, to be sent then if it was held long enough.
                                    state.slow_pressed.insert(code, std::time::Instant::now());
                                    return FilterResult::Intercept(());
                                }
                                handled
                            }
                            KeyState::Released => {
                                if let Some(at) = state.slow_pressed.remove(&code) {
                                    let slow = std::time::Duration::from_millis(state.config.slow_keys_ms.into());
                                    if at.elapsed() < slow {
                                        return FilterResult::Intercept(());
                                    }
                                    let keyboard = state.seat.get_keyboard().unwrap();
                                    keyboard.input_forward(state, code, KeyState::Pressed, SERIAL_COUNTER.next_serial(), time, false);
                                    return FilterResult::Forward;
                                }
                                state.key_released(keysym.modified_sym());
                                state.switcher_modifiers(modifiers.ctrl, modifiers.alt, modifiers.logo);
                                state.suppressed_keys.remove(&code)
                            }
                        };
                        if handled {
                            FilterResult::Intercept(())
                        } else {
                            FilterResult::Forward
                        }
                    },
                );
                if std::mem::take(&mut self.sticky.release_after) {
                    self.sticky.latched = [false; 4];
                    let keyboard = self.seat.get_keyboard().unwrap();
                    keyboard.set_latched_modifiers(self, &ModifiersState::default());
                }
            }
            InputEvent::PointerMotion { event, .. } => {
                // With every display unplugged there is nowhere for the pointer to be.
                let Some(first) = self.space.outputs().next().cloned() else {
                    return;
                };
                let pointer = self.seat.get_pointer().unwrap();
                let from = pointer.current_location();
                let relative = RelativeMotionEvent {
                    delta: event.delta(),
                    delta_unaccel: event.delta_unaccel(),
                    utime: event.time(),
                };
                let held = self.pointer_held(&pointer, from);
                // A game looking around with the mouse has only the movement, not the pointer.
                if let Some((surface, origin, Held::Locked)) = held {
                    pointer.relative_motion(self, Some((surface, origin)), &relative);
                    pointer.frame(self);
                    return;
                }
                let geometry = |output| self.space.output_geometry(output).unwrap().to_f64();
                let mut pos = from + event.delta();
                // It goes on to the output it moves onto, or stops at the edge of the one it is on,
                // sliding along it: moved up and across at the top, it still crosses to the next.
                let on_an_output = |pos| self.space.outputs().any(|output| geometry(output).contains(pos));
                if !on_an_output(pos) {
                    let output = self.space.outputs().find(|output| geometry(output).contains(from));
                    let bounds = geometry(output.unwrap_or(&first));
                    pos.y = pos.y.clamp(bounds.loc.y, bounds.loc.y + bounds.size.h - 1.0);
                    if !on_an_output(pos) {
                        pos.x = pos.x.clamp(bounds.loc.x, bounds.loc.x + bounds.size.w - 1.0);
                    }
                }
                if let Some((surface, origin, Held::Confined(region))) = &held {
                    let inside = self
                        .surface_under(pos)
                        .is_some_and(|(under, _)| under == *surface)
                        && region
                            .as_ref()
                            .is_none_or(|region| region.contains((pos - *origin).to_i32_round()));
                    if !inside {
                        pos = from;
                    }
                }

                // A window holding the pointer keeps it from the menu bar, the Dock and the corners.
                if !locked && held.is_none() {
                    self.compositor_motion(pos);
                }
                let under = self.surface_under(pos);
                self.focus_under_pointer(under.as_ref().map(|(surface, _)| surface));
                pointer.motion(
                    self,
                    under.clone(),
                    &MotionEvent {
                        location: pos,
                        serial: SERIAL_COUNTER.next_serial(),
                        time: event.time_msec(),
                    },
                );
                pointer.relative_motion(self, under.clone(), &relative);
                pointer.frame(self);
                self.hold_pointer(&pointer, under.as_ref());
            }
            InputEvent::PointerMotionAbsolute { event, .. } => {
                let Some(output) = self.space.outputs().next() else {
                    return;
                };

                let output_geo = self.space.output_geometry(output).unwrap();

                let pos = event.position_transformed(output_geo.size) + output_geo.loc.to_f64();

                let serial = SERIAL_COUNTER.next_serial();

                let pointer = self.seat.get_pointer().unwrap();

                if !locked {
                    self.compositor_motion(pos);
                }
                let under = self.surface_under(pos);
                self.focus_under_pointer(under.as_ref().map(|(surface, _)| surface));

                pointer.motion(
                    self,
                    under,
                    &MotionEvent {
                        location: pos,
                        serial,
                        time: event.time_msec(),
                    },
                );
                pointer.frame(self);
            }
            InputEvent::PointerButton { event, .. } => {
                let pointer = self.seat.get_pointer().unwrap();
                let serial = SERIAL_COUNTER.next_serial();
                let button = event.button_code();
                let button_state = event.state();

                if locked {
                    self.focus_lock();
                } else if ButtonState::Pressed == button_state && !pointer.is_grabbed() {
                    self.super_tap = false;
                    self.pointer_pressed(button, serial);
                } else if ButtonState::Released == button_state {
                    self.menu_bar_release(pointer.current_location());
                    self.dock_release(pointer.current_location());
                    self.desktop_release();
                    self.overview_release(pointer.current_location());
                    self.notice_release(pointer.current_location());
                }

                pointer.button(
                    self,
                    &ButtonEvent {
                        button,
                        state: button_state,
                        serial,
                        time: event.time_msec(),
                    },
                );
                pointer.frame(self);
            }
            InputEvent::PointerAxis { event, .. } => {
                let source = event.source();
                let speed = self.config.scroll_speed;

                let horizontal_amount = event
                    .amount(Axis::Horizontal)
                    .unwrap_or_else(|| event.amount_v120(Axis::Horizontal).unwrap_or(0.0) * 15.0 / 120.)
                    * speed;
                let vertical_amount = event
                    .amount(Axis::Vertical)
                    .unwrap_or_else(|| event.amount_v120(Axis::Vertical).unwrap_or(0.0) * 15.0 / 120.)
                    * speed;
                let horizontal_amount_discrete = event.amount_v120(Axis::Horizontal).map(|v120| v120 * speed);
                let vertical_amount_discrete = event.amount_v120(Axis::Vertical).map(|v120| v120 * speed);

                let mut frame = AxisFrame::new(event.time_msec()).source(source);
                if horizontal_amount != 0.0 {
                    frame = frame.value(Axis::Horizontal, horizontal_amount);
                    if let Some(discrete) = horizontal_amount_discrete {
                        frame = frame.v120(Axis::Horizontal, discrete as i32);
                    }
                }
                if vertical_amount != 0.0 {
                    frame = frame.value(Axis::Vertical, vertical_amount);
                    if let Some(discrete) = vertical_amount_discrete {
                        frame = frame.v120(Axis::Vertical, discrete as i32);
                    }
                }

                if source == AxisSource::Finger {
                    if event.amount(Axis::Horizontal) == Some(0.0) {
                        frame = frame.stop(Axis::Horizontal);
                    }
                    if event.amount(Axis::Vertical) == Some(0.0) {
                        frame = frame.stop(Axis::Vertical);
                    }
                }

                let pointer = self.seat.get_pointer().unwrap();
                if !locked
                    && (self.notification_center_scroll(pointer.current_location(), vertical_amount)
                        || self.dock_scroll(pointer.current_location(), vertical_amount)
                        || self.overview_scroll(pointer.current_location(), vertical_amount + horizontal_amount)
                        || self.apps_scroll(vertical_amount + horizontal_amount))
                {
                    return;
                }
                pointer.axis(self, frame);
                pointer.frame(self);
            }
            InputEvent::SwitchToggle { event } => self.lid_switched(&event),
            _ => {}
        }
    }
}

/// What Super+` runs: shows or hides the terminal's drop-down window, as Win+` does Nekominal's.
const DROP_DOWN_TERMINAL: &str = "nekoland-terminal drop-down";
/// Starts Orca, or stops it, and tells GNOME's setting, which starts it at the next login.
const SCREEN_READER_TOGGLE: &str = "key=org.gnome.desktop.a11y.applications; \
    if pkill -x orca; then gsettings set $key screen-reader-enabled false; \
    else gsettings set $key screen-reader-enabled true; exec orca; fi";
/// A note kept in Documents, opened in the text editor.
const QUICK_NOTE: &str = "note=\"$(xdg-user-dir DOCUMENTS 2>/dev/null || echo $HOME/Documents)/快速備忘錄.txt\"; \
    mkdir -p \"$(dirname \"$note\")\" && touch \"$note\" && exec gio open \"$note\"";
/// GNOME's Miracast sender, as Windows' Super+K finds wireless displays.
const CAST: &str = "command -v gnome-network-displays >/dev/null && exec gnome-network-displays; \
    notify-send -i video-display-symbolic '無法投放' '需要 gnome-network-displays'";
const KEYBOARD_LIGHT_UP: &str = "brightnessctl -d '*::kbd_backlight' set 33%+";
const KEYBOARD_LIGHT_DOWN: &str = "brightnessctl -d '*::kbd_backlight' set 33%-";
/// Every radio off, or every one back on when Wi-Fi is off already.
const AIRPLANE_MODE: &str = "if [ \"$(nmcli radio wifi)\" = enabled ]; then nmcli radio all off; bluetoothctl power off; \
    else nmcli radio all on; bluetoothctl power on; fi";
// evdev's BTN_LEFT
const BUTTON_LEFT: u32 = 0x110;
const BUTTON_RIGHT: u32 = 0x111;

/// How a window holds the pointer: in place, for a game's mouse-look, or inside an area of
/// itself, in the surface's coordinates, or all of it without one.
pub enum Held {
    Locked,
    Confined(Option<RegionAttributes>),
}

impl Nekoland {
    /// Whether the window a surface is of has the keyboard: only then may it hold the
    /// pointer, so that switching away from it with the keyboard lets the pointer go.
    fn has_keyboard(&self, surface: &WlSurface) -> bool {
        let mut root = surface.clone();
        while let Some(parent) = get_parent(&root) {
            root = parent;
        }
        self.window_for_surface(&root)
            .is_some_and(|window| self.focused_window() == Some(window))
    }

    /// What the compositor draws itself follows the pointer to `pos`: the menu bar, the
    /// Dock, the desktop, the corners and the rest.
    fn compositor_motion(&mut self, pos: Point<f64, Logical>) {
        self.menu_bar_motion(pos);
        self.dock_motion(pos);
        self.desktop_motion(pos);
        self.hover_motion(pos);
        self.notice_motion(pos);
        self.overview_motion(pos);
        self.corner_motion(pos);
        self.snap_layouts_motion(pos);
        self.snap_assist_motion(pos);
        self.divider_motion(pos);
    }

    /// The surface holding the pointer at `at`, where it is, and how; one that no longer has
    /// the keyboard lets it go.
    fn pointer_held(
        &self,
        pointer: &PointerHandle<Self>,
        at: Point<f64, Logical>,
    ) -> Option<(WlSurface, Point<f64, Logical>, Held)> {
        let (surface, origin) = self.surface_under(at)?;
        let may = self.has_keyboard(&surface);
        let held = with_pointer_constraint(&surface, pointer, |constraint| {
            let constraint = constraint.filter(|constraint| constraint.is_active())?;
            if !may {
                constraint.deactivate();
                return None;
            }
            Some(match &*constraint {
                PointerConstraint::Locked(_) => Held::Locked,
                PointerConstraint::Confined(confined) => Held::Confined(confined.region().cloned()),
            })
        })?;
        Some((surface, origin, held))
    }

    /// Has the surface the pointer is in hold it, when it asked to, has the keyboard, and the
    /// pointer is in the area it asked for.
    pub fn hold_pointer(
        &self,
        pointer: &PointerHandle<Self>,
        under: Option<&(WlSurface, Point<f64, Logical>)>,
    ) {
        let Some((surface, origin)) = under.filter(|(surface, _)| self.has_keyboard(surface)) else {
            return;
        };
        let at = (pointer.current_location() - *origin).to_i32_round();
        with_pointer_constraint(surface, pointer, |constraint| {
            if let Some(constraint) =
                constraint.filter(|constraint| constraint.region().is_none_or(|region| region.contains(at)))
            {
                constraint.activate();
            }
        });
    }

    /// Gives the keyboard to what was clicked, and starts moving a window dragged with Super held.
    fn pointer_pressed(&mut self, button: u32, serial: Serial) {
        let pointer = self.seat.get_pointer().unwrap();
        let pos = pointer.current_location();

        // A menu or its submenu is an X11 popup; anywhere else closes a menu from the tray.
        let on_popup = self
            .space
            .element_under(pos)
            .and_then(|(window, _)| window.x11_surface().map(|surface| surface.is_override_redirect()))
            .unwrap_or(false);
        if !on_popup {
            crate::xtray::close_menu(pos, self.display_scale());
        }
        if self.hover_press(pos) {
            return;
        }
        if self.switcher_press(pos) || self.snap_layouts_press(pos) || self.widgets_press(pos) {
            return;
        }
        if self.snap_assist_press(pos) || (button == BUTTON_LEFT && self.divider_press(pos, button, serial)) {
            return;
        }
        let clicked = self.space.element_under(pos).map(|(window, _)| window.clone());
        self.close_launcher_unless(clicked.as_ref());
        self.desktop_pointer_pressed(pos);
        self.menu_bar_claim(pos);
        if self.menu_bar_press(pos, button == BUTTON_RIGHT) {
            return;
        }
        let left_or_right = button == BUTTON_LEFT || button == BUTTON_RIGHT;
        if (left_or_right && self.dock_press(pos, button == BUTTON_RIGHT, serial))
            // Notifications are not shown over the overview, so nothing of them is there to press.
            || (button == BUTTON_LEFT && self.overview.is_none() && self.notice_press(pos))
        {
            return;
        }
        if button == BUTTON_RIGHT && (self.overview_desktop_menu(pos) || self.apps_menu(pos)) {
            return;
        }
        if self.overview.is_some() && self.overview_covers(pos) {
            self.overview_click(pos);
        } else if let Some((layer, _)) = self.layer_under(pos, &ABOVE_WINDOWS) {
            self.focus_layer(&layer);
        } else if self.overview.is_some() {
            self.overview_click(pos);
        } else if self.frame_press(pos, button, serial) {
            // A title bar or grip of an X11 window.
        } else if let Some((window, location)) = self.space.element_under(pos).map(|(w, l)| (w.clone(), l)) {
            // The on-screen keyboard types into the window in front, so never takes its place.
            if app_id(&window) == KEYBOARD_APP_ID {
                return;
            }
            self.focus_window(&window);

            let logo = self
                .seat
                .get_keyboard()
                .is_some_and(|keyboard| keyboard.modifier_state().logo);
            if logo && button == BUTTON_LEFT {
                let start_data = GrabStartData {
                    focus: None,
                    button,
                    location: pos,
                };
                let grab = MoveSurfaceGrab::new(start_data, window, location);
                pointer.set_grab(self, grab, serial, Focus::Clear);
            }
        } else if left_or_right && self.desktop_press(pos, button == BUTTON_RIGHT, serial) {
            self.set_keyboard_focus(None);
        } else if let Some((layer, _)) = self.layer_under(pos, &BELOW_WINDOWS) {
            self.focus_layer(&layer);
        } else {
            self.set_keyboard_focus(None);
        }
    }

    /// A layer surface only takes the keyboard if it said it wants it.
    fn focus_layer(&mut self, layer: &LayerSurface) {
        if layer.cached_state().keyboard_interactivity != KeyboardInteractivity::None {
            self.set_keyboard_focus(Some(layer.wl_surface().clone().into()));
        }
    }

    /// Runs the shortcut a key press stands for. Returns whether there was one.
    fn key_pressed(&mut self, modifiers: &ModifiersState, sym: Keysym, typed: Option<char>) -> bool {
        self.super_tap = matches!(sym, Keysym::Super_L | Keysym::Super_R);
        self.peeking = false;
        if matches!(sym, Keysym::Alt_L | Keysym::Alt_R) {
            self.dock_menu_alt(true);
        }

        let ModifiersState {
            ctrl,
            alt,
            logo,
            shift,
            ..
        } = *modifiers;

        if self.hover_key(sym, typed) {
            return true;
        }
        if self.keyboard_frame_key(sym, ctrl) {
            return true;
        }
        if self.switcher_key(sym, shift)
            || self.snap_layouts_key(sym)
            || self.snap_assist_key(sym, shift)
            || self.widgets_key(sym)
        {
            return true;
        }
        if self.alert_shown() && sym == Keysym::Escape {
            self.dismiss_alert();
            return true;
        }
        if (self.menu_bar_panel_open() || self.menu_bar.clock_panel.is_some()) && sym == Keysym::Escape {
            self.close_menu();
            return true;
        }
        // A pinned panel leaves the keyboard to the window in front.
        if !(ctrl || alt || logo) && !self.clock_panel_pinned() && self.clock_panel_key(sym, typed, shift) {
            return true;
        }
        if self.menu_bar.open.is_some() {
            self.menu_bar_key(sym, typed);
            return true;
        }

        if sym == Keysym::Escape && (self.close_dock_stack() || self.media_card_escape()) {
            return true;
        }
        if self.media_card_key(sym) {
            return true;
        }
        if self.dock_key(sym) {
            return true;
        }

        if self.overview.is_some() && !(ctrl || alt || logo) {
            if self.overview_rename_key(sym, typed) {
                return true;
            }
            if self.apps_shown() {
                return self.apps_key(sym, typed);
            }
            match typed {
                _ if sym == Keysym::Escape => self.close_overview(None),
                // Typing in the overview searches the applications, as it does on GNOME.
                Some(character) if !character.is_control() => self.show_apps(&character.to_string()),
                _ => return false,
            }
            return true;
        }

        if self.desktop_key(modifiers, sym, typed) {
            return true;
        }

        // The only way out on a bare console, whatever the shortcuts are.
        if sym == Keysym::BackSpace && ctrl && alt {
            tracing::info!("quitting: Ctrl+Alt+BackSpace");
            self.loop_signal.stop();
            return true;
        }
        let combo = Combo {
            ctrl,
            alt,
            shift,
            logo,
            key: xkb::keysym_get_name(sym),
        };
        let inhibited = self.shortcuts_inhibited();
        let app_id = self.focused_window().map(|window| app_id(&window));
        let found = shortcuts::find(&self.shortcut_bindings, &combo);
        // Ctrl with a letter in a terminal is the program's, though an application action
        // was given it: tmux and readline take them as they are.
        let terminal_own = |action: &shortcuts::Action| {
            action.is_app()
                && combo.ctrl
                && !(combo.alt || combo.logo)
                && app_id.as_deref().is_some_and(shortcuts::is_terminal)
        };
        // Super+Esc is how the keys come back from a window that took them all.
        let allowed = |action: &shortcuts::Action| !inhibited || action.id == "restore-shortcuts";
        if let Some((action, extended)) = found.filter(|(action, _)| allowed(action) && !terminal_own(action)) {
            self.run_action(action, extended);
            return true;
        }

        let player = |method: &'static str| {
            std::thread::spawn(move || {
                if let Some(player) = crate::system::read_player() {
                    crate::system::player_command(&player.bus, method);
                }
            });
        };
        match sym {
            Keysym::XF86_AudioPlay | Keysym::XF86_AudioPause => player("PlayPause"),
            Keysym::XF86_AudioNext => player("Next"),
            Keysym::XF86_AudioPrev => player("Previous"),
            Keysym::XF86_AudioStop => player("Stop"),
            Keysym::XF86_AudioMicMute => config::spawn("wpctl set-mute @DEFAULT_AUDIO_SOURCE@ toggle"),
            Keysym::XF86_KbdBrightnessUp => config::spawn(KEYBOARD_LIGHT_UP),
            Keysym::XF86_KbdBrightnessDown => config::spawn(KEYBOARD_LIGHT_DOWN),
            Keysym::XF86_TouchpadToggle => self.toggle_setting("touchpad", !self.config.touchpad),
            Keysym::XF86_TouchpadOn => self.toggle_setting("touchpad", true),
            Keysym::XF86_TouchpadOff => self.toggle_setting("touchpad", false),
            Keysym::XF86_Calculator => config::spawn("gnome-calculator"),
            Keysym::XF86_RFKill => config::spawn(AIRPLANE_MODE),
            Keysym::XF86_Sleep => self.run_menu_command(Command::Sleep),
            Keysym::XF86_ScreenSaver => self.lock_screen(),
            Keysym::XF86_Display => self.next_projection(),
            Keysym::XF86_Search => config::spawn(&self.config.launcher),
            Keysym::XF86_Explorer | Keysym::XF86_MyComputer => config::spawn("nekoland-files"),
            Keysym::XF86_AudioRaiseVolume => config::spawn(&self.config.volume_up),
            Keysym::XF86_AudioLowerVolume => config::spawn(&self.config.volume_down),
            Keysym::XF86_AudioMute => config::spawn(&self.config.volume_mute),
            Keysym::XF86_MonBrightnessUp => config::spawn(&self.config.brightness_up),
            Keysym::XF86_MonBrightnessDown => config::spawn(&self.config.brightness_down),
            _ if self.vt_key(sym) => {}
            _ => return false,
        }
        true
    }

    /// Writes a setting and takes the configuration in again.
    pub(crate) fn toggle_setting(&mut self, key: &str, on: bool) {
        match config::set(&format!("{key} = {on}")) {
            Ok(()) => self.reload_config(),
            Err(err) => tracing::warn!("{key}: {err}"),
        }
    }

    /// Lets the window that has the keyboard no longer take the shortcuts' keys.
    fn restore_shortcuts(&mut self) {
        let focus = self.seat.get_keyboard().and_then(|keyboard| keyboard.current_focus());
        if let Some(surface) = focus.as_ref().and_then(|focus| focus.wl_surface()) {
            if let Some(inhibitor) = self.seat.keyboard_shortcuts_inhibitor_for_surface(&surface) {
                inhibitor.inactivate();
            }
        }
    }

    /// Whether the window that has the keyboard asked for every key, shortcuts too.
    fn shortcuts_inhibited(&self) -> bool {
        let focus = self.seat.get_keyboard().and_then(|keyboard| keyboard.current_focus());
        let surface = focus.as_ref().and_then(|focus| focus.wl_surface());
        surface.is_some_and(|surface| {
            self.seat.keyboard_shortcuts_inhibitor_for_surface(&surface).is_some_and(|inhibitor| inhibitor.is_active())
        })
    }

    /// Does an action: a system one here, an application one in the window that has the
    /// keyboard; `extended` adds Shift to the application's keys, as selecting does to moving.
    pub fn run_action(&mut self, action: &shortcuts::Action, extended: bool) {
        if action.is_app() {
            self.app_action(action, extended);
            return;
        }
        let focused = self.focused_window();
        match action.id {
            // As Raycast opens: its key closes it again.
            "launcher" => {
                if focused.as_ref().is_some_and(|window| app_id(window) == LAUNCHER_APP_ID) {
                    self.close_launcher_unless(None);
                } else {
                    config::spawn(&self.config.launcher);
                }
            }
            "apps" => self.show_apps(""),
            "terminal" => config::spawn(&self.config.terminal),
            "drop-down-terminal" => config::spawn(DROP_DOWN_TERMINAL),
            "settings" => config::spawn("nekoland-settings"),
            "lock" => self.lock_screen(),
            "projection" => self.next_projection(),
            "menu-bar" => self.open_first_menu(),
            "dock" => self.focus_dock(),
            "zoom-in" => self.zoom_by(crate::render::ZOOM_STEP),
            "zoom-out" => self.zoom_by(-crate::render::ZOOM_STEP),
            "switch-window" => self.switch(extended, true),
            "switch-window-held" => self.switch(extended, false),
            "cycle-windows" => self.cycle_windows(extended),
            "window-menu" => self.open_window_menu(),
            "peek-desktop" => self.peeking = true,
            "snap-layouts" => self.toggle_snap_layouts(),
            "move-window" => {
                self.window_command("move");
            }
            "resize-window" => {
                self.window_command("resize");
            }
            "overview" => self.toggle_overview(),
            "close-window" => close(focused),
            "quit-app" => {
                if let Some(window) = focused {
                    self.quit_app(&app_id(&window));
                }
            }
            "minimize" => {
                if let Some(window) = focused {
                    self.minimize(&window);
                }
            }
            "fullscreen" => {
                if let Some(window) = focused {
                    self.toggle_fullscreen(&window);
                }
            }
            "workspace-left" => self.switch_workspace_by(-1),
            "workspace-right" => self.switch_workspace_by(1),
            "new-workspace" => self.add_and_show_workspace(),
            "close-workspace" => self.remove_workspace(self.workspaces.active),
            "control-center" => {
                if let Some(anchor) = self.control_center_anchor() {
                    self.toggle_control_center(anchor);
                }
            }
            "notification-center" => self.toggle_notification_center(),
            "files" => config::spawn("nekoland-files"),
            "task-manager" => config::spawn("nekoland-tasks"),
            // fcitx5's own toggle: back to the input method used before.
            "input-method" => config::spawn("fcitx5-remote -t"),
            "log-out" => self.run_menu_command(Command::LogOut),
            "screen-reader" => config::spawn(SCREEN_READER_TOGGLE),
            "zoom-toggle" => {
                let zoom = self.config.zoom;
                self.zoom_by(if zoom > 1.0 { 1.0 - zoom } else { 1.0 });
            }
            "app-windows" => {
                if let Some(window) = focused {
                    self.show_app_windows(&app_id(&window));
                }
            }
            "next-app-window" => self.focus_next_app_window(),
            "hide-app" => self.run_menu_command(Command::Hide),
            "hide-others" => self.run_menu_command(Command::HideOthers),
            "show-all" => self.run_menu_command(Command::ShowAll),
            "show-desktop" => self.toggle_show_desktop(),
            "toggle-maximize" => self.run_menu_command(Command::Zoom),
            "sticky" => self.run_menu_command(Command::ShowOnAllDesktops),
            "workspace-first" => self.show_workspace(0, true),
            "workspace-last" => self.show_workspace(self.workspaces.count().saturating_sub(1), true),
            "carry-left" | "carry-right" | "carry-first" | "carry-last" => {
                if let Some(window) = focused {
                    let last = self.workspaces.count().saturating_sub(1) as isize;
                    let active = self.workspaces.active as isize;
                    let step = match action.id {
                        "carry-left" => -1,
                        "carry-right" => 1,
                        "carry-first" => -active,
                        _ => last - active,
                    };
                    self.carry_window_by(&window, step);
                }
            }
            "emoji" => config::spawn("nekoland-launcher --open=emoji"),
            "clipboard-history" => config::spawn("nekoland-launcher --open=clipboard"),
            "quick-note" => config::spawn(QUICK_NOTE),
            "quick-link" => self.open_quick_link_menu(),
            "widgets" => self.toggle_widgets(),
            "date-time" => {
                if let Some(anchor) = self.clock_title_rect() {
                    self.toggle_clock_panel(anchor);
                }
            }
            "do-not-disturb" => self.notices.quiet = !self.notices.quiet,
            "dock-autohide" => self.toggle_setting("dock_autohide", !self.config.dock_autohide),
            "invert-colors" => self.toggle_setting("invert_colors", !self.config.invert_colors),
            "accessibility" => self.run_menu_command(Command::SettingsPage("協助工具")),
            "about" => self.run_menu_command(Command::SettingsPage("關於")),
            "restore-shortcuts" => self.restore_shortcuts(),
            "sleep" => self.run_menu_command(Command::Sleep),
            "restart" => self.run_menu_command(Command::Restart),
            "shut-down" => self.run_menu_command(Command::ShutDown),
            "screens-off" => self.screens_off = true,
            "dictation" => config::spawn(&self.config.dictation),
            "rotation-lock" => self.toggle_setting("rotation_lock", !self.config.rotation_lock),
            "cast" => config::spawn(CAST),
            "security" => config::spawn("nekoland-security"),
            // Started again, it goes away.
            "on-screen-keyboard" => config::spawn("nekoland-keyboard"),
            // GNOME records the whole screen at once.
            "record-screen" => config::spawn("nekoland-shot record"),
            id => {
                let number = |prefix: &str| id.strip_prefix(prefix).and_then(|n| n.parse::<usize>().ok());
                if let Some(number) = number("workspace-") {
                    if number <= self.workspaces.count() {
                        self.show_workspace(number - 1, true);
                    }
                } else if let Some(number) = number("dock-new-") {
                    self.dock_app_command(number, false);
                } else if let Some(number) = number("dock-menu-") {
                    self.dock_app_command(number, true);
                } else if let Some(number) = number("dock-") {
                    self.open_dock_app(number);
                } else if let Some(command) = id.strip_prefix("arrange-") {
                    self.window_command(command);
                } else if let Some(kind) = id.strip_prefix("shot-") {
                    self.close_shot_thumbnails();
                    config::spawn(&format!("nekoland-shot {kind}"));
                } else {
                    tracing::warn!("no way to do the action {id}");
                }
            }
        }
    }

    /// Asks the window that has the keyboard to do an application action, with the keys its
    /// application does it with; `shift` adds Shift to them.
    pub fn app_action(&mut self, action: &shortcuts::Action, shift: bool) {
        let app_id = self.focused_window().map(|window| app_id(&window)).unwrap_or_default();
        for combo in shortcuts::native(action, &app_id, &self.config.app_shortcuts) {
            self.send_combo(&if shift { combo.with_shift() } else { combo });
        }
    }

    /// What a menu shows for an action's keys: the first the shortcuts give it, or else the
    /// keys applications do an application action with by themselves.
    pub fn shortcut_label(&self, id: &str) -> String {
        let native = || {
            let action = shortcuts::action(id)?;
            shortcuts::native(action, "", &self.config.app_shortcuts).into_iter().next()
        };
        shortcuts::keys(&self.config.shortcuts, id)
            .into_iter()
            .next()
            .or_else(native)
            .map(|combo| combo.label())
            .unwrap_or_default()
    }

    /// Presses and lets go `combo` for the window that has the keyboard: the modifiers held
    /// are let go meanwhile and pressed again after, so that it sees those keys alone.
    pub(crate) fn send_combo(&mut self, combo: &Combo) {
        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
        };
        let pressed = keyboard.pressed_keys();
        let wanted = [
            (combo.ctrl, "Control_L"),
            (combo.alt, "Alt_L"),
            (combo.shift, "Shift_L"),
            (combo.logo, "Super_L"),
            (true, combo.key.as_str()),
        ];
        let (held, codes) = keyboard.with_xkb_state(self, |context| {
            let xkb = context.xkb().lock().unwrap();
            // SAFETY: the keymap is only read, while its lock is held, and nothing of it is kept.
            let keymap = unsafe { xkb.keymap() };
            let layout = xkb.active_layout().0;
            let held: Vec<Keycode> = pressed
                .iter()
                .copied()
                .filter(|code| {
                    let symbol = keymap.key_get_syms_by_level(*code, layout, 0).first().copied();
                    symbol.and_then(StickyKeys::modifier).is_some()
                })
                .collect();
            let code_of = |name: &str| {
                let sym = xkb::keysym_from_name(name, xkb::KEYSYM_NO_FLAGS);
                (keymap.min_keycode().raw()..=keymap.max_keycode().raw())
                    .map(Keycode::new)
                    .find(|code| sym.raw() != 0 && keymap.key_get_syms_by_level(*code, layout, 0).contains(&sym))
            };
            let codes: Option<Vec<Keycode>> =
                wanted.iter().filter(|(on, _)| *on).map(|(_, name)| code_of(name)).collect();
            (held, codes)
        });
        let Some(codes) = codes else {
            tracing::warn!("no key for {combo} in this keymap");
            return;
        };
        let time = self.start_time.elapsed().as_millis() as u32;
        let send = |state: &mut Self, code: Keycode, key_state: KeyState| {
            keyboard.input::<(), _>(state, code, key_state, SERIAL_COUNTER.next_serial(), time, |_, _, _| {
                FilterResult::Forward
            });
        };
        for code in &held {
            send(self, *code, KeyState::Released);
        }
        for code in &codes {
            send(self, *code, KeyState::Pressed);
        }
        for code in codes.iter().rev() {
            send(self, *code, KeyState::Released);
        }
        for code in &held {
            send(self, *code, KeyState::Pressed);
        }
    }

    /// Ctrl+Alt+F1 and so on, which the keymap turns into these, switch the virtual
    /// terminal. Returns whether the key was one.
    pub fn vt_key(&mut self, sym: Keysym) -> bool {
        if !(Keysym::XF86_Switch_VT_1.raw()..=Keysym::XF86_Switch_VT_12.raw()).contains(&sym.raw()) {
            return false;
        }
        let vt = (sym.raw() - Keysym::XF86_Switch_VT_1.raw() + 1) as i32;
        if let Some(switch_vt) = self.switch_vt.as_mut() {
            switch_vt(vt);
        }
        true
    }

    /// Super pressed and let go by itself does what the shortcuts give it.
    fn key_released(&mut self, sym: Keysym) {
        if matches!(sym, Keysym::Super_L | Keysym::Super_R) {
            self.peeking = false;
        }
        if matches!(sym, Keysym::Alt_L | Keysym::Alt_R) {
            self.dock_menu_alt(false);
        }
        if self.super_tap && matches!(sym, Keysym::Super_L | Keysym::Super_R) && !self.shortcuts_inhibited() {
            self.super_tap = false;
            let alone = Combo::parse(shortcuts::SUPER_ALONE).expect("Super alone is a combination");
            if let Some((action, _)) = shortcuts::find(&self.shortcut_bindings, &alone) {
                self.run_action(action, false);
            }
        }
    }
}

fn close(window: Option<Window>) {
    match window.as_ref().map(Window::underlying_surface) {
        Some(WindowSurface::Wayland(toplevel)) => toplevel.send_close(),
        Some(WindowSurface::X11(surface)) => {
            let _ = surface.close();
        }
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_modifier_let_go_alone_stays_on_for_the_next_key() {
        use smithay::input::keyboard::Keycode;
        let (shift, letter) = (Keycode::new(42), Keycode::new(30));
        let modifier = StickyKeys::modifier(Keysym::Shift_L);
        assert_eq!(modifier, Some(0));
        let mut sticky = StickyKeys::default();
        assert_eq!(sticky.key(shift, modifier, true, true), None);
        assert!(sticky.key(shift, modifier, false, true).unwrap().shift);
        // The next key takes it, and lets it go once it is sent.
        assert_eq!(sticky.key(letter, None, true, true), None);
        assert!(sticky.release_after);
        // Held with another key, a modifier is not latched.
        let mut sticky = StickyKeys::default();
        sticky.key(letter, None, true, true);
        sticky.key(shift, modifier, true, false);
        assert_eq!(sticky.key(shift, modifier, false, false), None);
        // Alone again, it goes off.
        let mut sticky = StickyKeys::default();
        for _ in 0..2 {
            sticky.key(shift, modifier, true, true);
            sticky.key(shift, modifier, false, true);
        }
        assert!(!sticky.latched.contains(&true));
    }

    #[test]
    fn every_window_arrangement_action_names_a_window_command() {
        for action in shortcuts::actions() {
            if let Some(command) = action.id.strip_prefix("arrange-") {
                assert!(
                    crate::windows::OTHER_COMMANDS.contains(&command) || crate::arrange::arrangement(command).is_some(),
                    "{command}"
                );
            }
        }
    }

    #[test]
    fn every_system_action_is_done_by_something() {
        // The ids `run_action` names, each before its `=>`, and those it takes by their prefix.
        let source = include_str!("input.rs");
        let start = source.find("        match action.id {").unwrap();
        let end = start + source[start..].find("            id => {").unwrap();
        let named: Vec<&str> = source[start..end]
            .lines()
            .filter_map(|line| line.split_once("=>").map(|(before, _)| before))
            .flat_map(|before| before.split('"').skip(1).step_by(2))
            .collect();
        let numbered = |id: &str, prefix: &str| id.strip_prefix(prefix).is_some_and(|n| n.parse::<usize>().is_ok());
        for action in shortcuts::actions().iter().filter(|action| !action.is_app()) {
            let id = action.id;
            assert!(
                named.contains(&id)
                    || ["workspace-", "dock-new-", "dock-menu-", "dock-"].iter().any(|prefix| numbered(id, prefix))
                    || id.starts_with("arrange-")
                    || id.starts_with("shot-"),
                "nothing does {id}"
            );
        }
    }

    #[test]
    fn every_key_the_actions_have_is_one_xkb_knows() {
        for action in shortcuts::actions() {
            for combo in action.defaults().into_iter().filter(|combo| combo.key != shortcuts::SUPER_ALONE) {
                let sym = xkb::keysym_from_name(&combo.key, xkb::KEYSYM_NO_FLAGS);
                assert_ne!(sym.raw(), 0, "{combo} of {}", action.id);
            }
        }
    }
}