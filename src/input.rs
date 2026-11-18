use smithay::{
    backend::input::{
        AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, InputBackend, InputEvent, KeyState,
        KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent, PointerMotionEvent,
    },
    desktop::{LayerSurface, Window, WindowSurface},
    input::{
        keyboard::{FilterResult, Keysym, ModifiersState},
        pointer::{AxisFrame, ButtonEvent, Focus, GrabStartData, MotionEvent, RelativeMotionEvent},
    },
    utils::{Serial, SERIAL_COUNTER},
    wayland::shell::wlr_layer::KeyboardInteractivity,
};

use crate::{
    config,
    grabs::MoveSurfaceGrab,
    state::{Nekoland, ABOVE_WINDOWS, BELOW_WINDOWS},
    windows::{window_names, LAUNCHER_APP_ID},
};

impl Nekoland {
    pub fn process_input_event<I: InputBackend>(&mut self, event: InputEvent<I>) {
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
                            return state.locked_key(modifiers, keysym.modified_sym(), pressed);
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
                                }
                                handled
                            }
                            KeyState::Released => {
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
            }
            InputEvent::PointerMotion { event, .. } => {
                let pointer = self.seat.get_pointer().unwrap();
                let from = pointer.current_location();
                let geometry = |output| self.space.output_geometry(output).unwrap().to_f64();
                let mut pos = from + event.delta();
                // It goes on to the output it moves onto, or stops at the edge of the one it is on.
                if !self.space.outputs().any(|output| geometry(output).contains(pos)) {
                    let output = self.space.outputs().find(|output| geometry(output).contains(from));
                    let bounds = geometry(output.or(self.space.outputs().next()).unwrap());
                    pos.x = pos.x.clamp(bounds.loc.x, bounds.loc.x + bounds.size.w - 1.0);
                    pos.y = pos.y.clamp(bounds.loc.y, bounds.loc.y + bounds.size.h - 1.0);
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
                    && (self.dock_scroll(pointer.current_location(), vertical_amount)
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
        if self.menu_bar_press(pos) {
            return;
        }
        let left_or_right = button == BUTTON_LEFT || button == BUTTON_RIGHT;
        if (left_or_right && self.dock_press(pos, button == BUTTON_RIGHT))
            || (button == BUTTON_LEFT && self.notice_press(pos))
        {
            return;
        }
        if self.overview.is_some() && self.overview_covers(pos) {
            self.overview_click(pos);
        } else if let Some((layer, _)) = self.layer_under(pos, ABOVE_WINDOWS) {
            self.focus_layer(&layer);
        } else if self.overview.is_some() {
            self.overview_click(pos);
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
        let focused = self.focused_window();

        if self.hover_key(sym, typed) {
            return true;
        }
        if self.menu_bar.control.is_some() && sym == Keysym::Escape {
            self.close_menu();
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

        if let Some(command) = arrange_shortcut(sym, logo && !ctrl, alt, shift) {
            self.window_command(command);
            return true;
        }

        match sym {
            // The only way out on a bare console.
            Keysym::BackSpace if ctrl && alt => self.loop_signal.stop(),
            Keysym::t if ctrl && alt => config::spawn(&self.config.terminal),
            Keysym::Return if logo => config::spawn(&self.config.terminal),
            Keysym::a if logo => self.show_apps(""),
            // As Raycast opens: Alt+Space, which closes it again.
            Keysym::space if alt && !(ctrl || logo || shift) => {
                if focused.as_ref().is_some_and(|window| window_names(window).1 == LAUNCHER_APP_ID) {
                    self.close_launcher_unless(None);
                } else {
                    config::spawn(&self.config.launcher);
                }
            }
            Keysym::l if logo => self.lock_screen(),
            Keysym::F4 if alt => close(focused),
            Keysym::q if logo => close(focused),
            Keysym::Tab if (alt || logo) && !shift => self.focus_next_window(),
            Keysym::F2 if ctrl => self.open_first_menu(),
            Keysym::F3 if ctrl => self.focus_dock(),
            Keysym::Up if ctrl && alt => self.toggle_overview(),
            Keysym::Left if ctrl && alt => self.switch_workspace_by(-1),
            Keysym::Right if ctrl && alt => self.switch_workspace_by(1),
            Keysym::Page_Up if logo => self.switch_workspace_by(-1),
            Keysym::Page_Down if logo => self.switch_workspace_by(1),
            // Screen zoom; Super Alt, as the Mac's Option Command, resizes windows here.
            Keysym::equal | Keysym::plus if logo && ctrl => self.zoom_by(crate::render::ZOOM_STEP),
            Keysym::minus if logo && ctrl => self.zoom_by(-crate::render::ZOOM_STEP),
            Keysym::h if logo => {
                if let Some(window) = focused {
                    self.minimize(&window);
                }
            }
            Keysym::F11 if !(ctrl || alt || logo) => {
                if let Some(window) = focused {
                    self.toggle_fullscreen(&window);
                }
            }
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

    /// Super pressed and let go by itself opens the overview.
    fn key_released(&mut self, sym: Keysym) {
        if matches!(sym, Keysym::Alt_L | Keysym::Alt_R) {
            self.dock_menu_alt(false);
        }
        if self.super_tap && matches!(sym, Keysym::Super_L | Keysym::Super_R) {
            self.super_tap = false;
            self.toggle_overview();
        }
    }
}

/// The window command a shortcut with Super stands for: the arrows alone halve, maximize and
/// restore, as on GNOME; with Alt too, Rectangle's keys, its Ctrl and Option being Super and
/// Alt here, since Ctrl+Alt with the arrows switches desktops.
fn arrange_shortcut(sym: Keysym, logo: bool, alt: bool, shift: bool) -> Option<&'static str> {
    if !logo {
        return None;
    }
    let command = match sym {
        Keysym::Left => "left",
        Keysym::Right => "right",
        Keysym::Up if alt && shift => "maximize-height",
        Keysym::Up if alt => "top",
        Keysym::Down if alt => "bottom",
        Keysym::Up => "maximize",
        Keysym::Down => "restore",
        _ if !alt => return None,
        Keysym::u => "top-left",
        Keysym::i => "top-right",
        Keysym::j => "bottom-left",
        Keysym::k => "bottom-right",
        Keysym::d => "first-third",
        Keysym::f => "center-third",
        Keysym::g => "last-third",
        Keysym::e => "first-two-thirds",
        Keysym::t => "last-two-thirds",
        Keysym::Return => "maximize",
        Keysym::c => "center",
        Keysym::BackSpace => "restore",
        Keysym::equal => "larger",
        Keysym::minus => "smaller",
        _ => return None,
    };
    Some(command)
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
    fn super_with_the_arrows_halves_maximizes_and_restores() {
        assert_eq!(arrange_shortcut(Keysym::Left, true, false, false), Some("left"));
        assert_eq!(arrange_shortcut(Keysym::Right, true, false, false), Some("right"));
        assert_eq!(arrange_shortcut(Keysym::Up, true, false, false), Some("maximize"));
        assert_eq!(arrange_shortcut(Keysym::Down, true, false, false), Some("restore"));
        assert_eq!(arrange_shortcut(Keysym::Left, false, false, false), None);
    }

    #[test]
    fn super_alt_takes_rectangles_keys_and_leaves_super_alone() {
        assert_eq!(arrange_shortcut(Keysym::Up, true, true, false), Some("top"));
        assert_eq!(arrange_shortcut(Keysym::Up, true, true, true), Some("maximize-height"));
        assert_eq!(arrange_shortcut(Keysym::u, true, true, false), Some("top-left"));
        assert_eq!(arrange_shortcut(Keysym::g, true, true, false), Some("last-third"));
        assert_eq!(arrange_shortcut(Keysym::BackSpace, true, true, false), Some("restore"));
        assert_eq!(arrange_shortcut(Keysym::minus, true, true, false), Some("smaller"));
        // Super alone keeps its own: H minimizes, Return opens a terminal.
        assert_eq!(arrange_shortcut(Keysym::h, true, false, false), None);
        assert_eq!(arrange_shortcut(Keysym::Return, true, false, false), None);
        assert_eq!(arrange_shortcut(Keysym::u, true, false, false), None);
    }

    #[test]
    fn every_shortcut_names_a_window_command() {
        let keys = [Keysym::u, Keysym::i, Keysym::j, Keysym::k, Keysym::d, Keysym::f, Keysym::g, Keysym::e,
            Keysym::t, Keysym::Return, Keysym::c, Keysym::equal, Keysym::minus, Keysym::Up, Keysym::Down];
        for key in keys {
            let command = arrange_shortcut(key, true, true, false).expect("a shortcut");
            assert!(command == "restore" || crate::arrange::arrangement(command).is_some(), "{command}");
        }
    }
}