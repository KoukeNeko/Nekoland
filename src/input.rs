use smithay::{
    backend::input::{
        AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, InputBackend, InputEvent, KeyState,
        KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent, PointerMotionEvent,
    },
    desktop::{LayerSurface, Window, WindowSurface},
    input::{
        keyboard::{xkb, FilterResult, Keycode, Keysym, ModifiersState},
        pointer::{AxisFrame, ButtonEvent, Focus, GrabStartData, MotionEvent, RelativeMotionEvent},
    },
    utils::{Serial, SERIAL_COUNTER},
    wayland::{
        keyboard_shortcuts_inhibit::KeyboardShortcutsInhibitorSeat, seat::WaylandFocus,
        shell::wlr_layer::KeyboardInteractivity,
    },
};

use shortcuts::Combo;

use crate::{
    config,
    grabs::MoveSurfaceGrab,
    state::{Nekoland, ABOVE_WINDOWS, BELOW_WINDOWS},
    windows::{window_names, LAUNCHER_APP_ID},
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
        self.screens_off = false;
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
                let pointer = self.seat.get_pointer().unwrap();
                let from = pointer.current_location();
                let geometry = |output| self.space.output_geometry(output).unwrap().to_f64();
                let mut pos = from + event.delta();
                // It goes on to the output it moves onto, or stops at the edge of the one it is on,
                // sliding along it: moved up and across at the top, it still crosses to the next.
                let on_an_output = |pos| self.space.outputs().any(|output| geometry(output).contains(pos));
                if !on_an_output(pos) {
                    let output = self.space.outputs().find(|output| geometry(output).contains(from));
                    let bounds = geometry(output.or(self.space.outputs().next()).unwrap());
                    pos.y = pos.y.clamp(bounds.loc.y, bounds.loc.y + bounds.size.h - 1.0);
                    if !on_an_output(pos) {
                        pos.x = pos.x.clamp(bounds.loc.x, bounds.loc.x + bounds.size.w - 1.0);
                    }
                }

                if !locked {
                    self.menu_bar_motion(pos);
                    self.dock_motion(pos);
                    self.desktop_motion(pos);
                    self.hover_motion(pos);
                    self.notice_motion(pos);
                    self.overview_motion(pos);
                    self.corner_motion(pos);
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
                pointer.relative_motion(
                    self,
                    under,
                    &RelativeMotionEvent {
                        delta: event.delta(),
                        delta_unaccel: event.delta_unaccel(),
                        utime: event.time(),
                    },
                );
                pointer.frame(self);
            }
            InputEvent::PointerMotionAbsolute { event, .. } => {
                let output = self.space.outputs().next().unwrap();

                let output_geo = self.space.output_geometry(output).unwrap();

                let pos = event.position_transformed(output_geo.size) + output_geo.loc.to_f64();

                let serial = SERIAL_COUNTER.next_serial();

                let pointer = self.seat.get_pointer().unwrap();

                if !locked {
                    self.menu_bar_motion(pos);
                    self.dock_motion(pos);
                    self.desktop_motion(pos);
                    self.hover_motion(pos);
                    self.notice_motion(pos);
                    self.overview_motion(pos);
                    self.corner_motion(pos);
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
// evdev's BTN_LEFT
const BUTTON_LEFT: u32 = 0x110;
const BUTTON_RIGHT: u32 = 0x111;

impl Nekoland {
    /// Gives the keyboard to what was clicked, and starts moving a window dragged with Super held.
    fn pointer_pressed(&mut self, button: u32, serial: Serial) {
        let pointer = self.seat.get_pointer().unwrap();
        let pos = pointer.current_location();

        if self.hover_press(pos) {
            return;
        }
        let clicked = self.space.element_under(pos).map(|(window, _)| window.clone());
        self.close_launcher_unless(clicked.as_ref());
        self.desktop_pointer_pressed(pos);
        self.menu_bar_claim(pos);
        if self.menu_bar_press(pos) {
            return;
        }
        let left_or_right = button == BUTTON_LEFT || button == BUTTON_RIGHT;
        if (left_or_right && self.dock_press(pos, button == BUTTON_RIGHT))
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
        } else if let Some((layer, _)) = self.layer_under(pos, ABOVE_WINDOWS) {
            self.focus_layer(&layer);
        } else if self.overview.is_some() {
            self.overview_click(pos);
        } else if self.frame_press(pos, button, serial) {
            // A title bar or grip of an X11 window.
        } else if let Some((window, location)) = self.space.element_under(pos).map(|(w, l)| (w.clone(), l)) {
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
        } else if let Some((layer, _)) = self.layer_under(pos, BELOW_WINDOWS) {
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

        if sym == Keysym::Escape && self.close_dock_stack() {
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
        let found = shortcuts::find(self.config.shortcut_scheme, &self.config.shortcuts, &combo);
        if let Some((action, extended)) = found.filter(|_| !self.shortcuts_inhibited()) {
            self.run_action(action, extended);
            return true;
        }

        match sym {
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
                if focused.as_ref().is_some_and(|window| window_names(window).1 == LAUNCHER_APP_ID) {
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
            "switch-window" => self.focus_next_window(),
            "overview" => self.toggle_overview(),
            "close-window" => close(focused),
            "quit-app" => {
                if let Some(window) = focused {
                    self.quit_app(&window_names(&window).1);
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
            "carry-left" | "carry-right" => {
                if let Some(window) = focused {
                    self.carry_window_by(&window, if action.id == "carry-left" { -1 } else { 1 });
                }
            }
            id => {
                if let Some(command) = id.strip_prefix("arrange-") {
                    self.window_command(command);
                } else if let Some(kind) = id.strip_prefix("shot-") {
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
        let app_id = self.focused_window().map(|window| window_names(&window).1).unwrap_or_default();
        for combo in shortcuts::native(action, &app_id, &self.config.app_shortcuts) {
            self.send_combo(&if shift { combo.with_shift() } else { combo });
        }
    }

    /// What a menu shows for an action's keys: the first the shortcuts give it, or else the
    /// keys applications do an application action with by themselves.
    pub fn shortcut_label(&self, id: &str) -> String {
        let scheme = self.config.shortcut_scheme;
        let native = || {
            let action = shortcuts::action(id)?;
            shortcuts::native(action, "", &self.config.app_shortcuts).into_iter().next()
        };
        shortcuts::keys(scheme, &self.config.shortcuts, id)
            .into_iter()
            .next()
            .or_else(native)
            .map(|combo| combo.label(scheme))
            .unwrap_or_default()
    }

    /// Presses and lets go `combo` for the window that has the keyboard: the modifiers held
    /// are let go meanwhile and pressed again after, so that it sees those keys alone.
    fn send_combo(&mut self, combo: &Combo) {
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
        if matches!(sym, Keysym::Alt_L | Keysym::Alt_R) {
            self.dock_menu_alt(false);
        }
        if self.super_tap && matches!(sym, Keysym::Super_L | Keysym::Super_R) && !self.shortcuts_inhibited() {
            self.super_tap = false;
            let alone = Combo::parse(shortcuts::SUPER_ALONE).expect("Super alone is a combination");
            if let Some((action, _)) = shortcuts::find(self.config.shortcut_scheme, &self.config.shortcuts, &alone) {
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
                assert!(command == "restore" || crate::arrange::arrangement(command).is_some(), "{command}");
            }
        }
    }

    #[test]
    fn every_key_the_schemes_give_is_one_xkb_knows() {
        for scheme in shortcuts::Scheme::ALL {
            for action in shortcuts::actions() {
                for combo in action.defaults(scheme).into_iter().filter(|combo| combo.key != shortcuts::SUPER_ALONE) {
                    let sym = xkb::keysym_from_name(&combo.key, xkb::KEYSYM_NO_FLAGS);
                    assert_ne!(sym.raw(), 0, "{combo} of {}", action.id);
                }
            }
        }
    }
}