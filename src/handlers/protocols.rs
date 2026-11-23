//! The smaller protocols clients expect from a desktop, each with little to decide.

use smithay::{
    delegate_data_control, delegate_fractional_scale, delegate_input_method_manager,
    delegate_primary_selection, delegate_single_pixel_buffer, delegate_text_input_manager,
    delegate_keyboard_shortcuts_inhibit, delegate_viewporter, delegate_virtual_keyboard_manager,
    delegate_pointer_constraints, delegate_relative_pointer, delegate_xdg_activation,
    delegate_xdg_decoration,
    desktop::{PopupKind, PopupManager},
    input::pointer::PointerHandle,
    reexports::{
        wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as DecorationMode,
        wayland_server::{protocol::wl_surface::WlSurface, DisplayHandle},
    },
    utils::{Logical, Point, Rectangle},
    wayland::{
        compositor::with_states,
        fractional_scale::{with_fractional_scale, FractionalScaleHandler, FractionalScaleManagerState},
        idle_inhibit::IdleInhibitManagerState,
        keyboard_shortcuts_inhibit::{
            KeyboardShortcutsInhibitHandler, KeyboardShortcutsInhibitState, KeyboardShortcutsInhibitor,
        },
        pointer_constraints::{with_pointer_constraint, PointerConstraintsHandler, PointerConstraintsState},
        relative_pointer::RelativePointerManagerState,
        security_context::SecurityContextState,
        session_lock::SessionLockManagerState,
        input_method::{InputMethodHandler, InputMethodManagerState, PopupSurface as ImePopupSurface},
        selection::{
            primary_selection::{PrimarySelectionHandler, PrimarySelectionState},
            wlr_data_control::{DataControlHandler, DataControlState},
        },
        shell::xdg::{
            decoration::{XdgDecorationHandler, XdgDecorationState},
            ToplevelSurface,
        },
        single_pixel_buffer::SinglePixelBufferState,
        text_input::TextInputManagerState,
        viewporter::ViewporterState,
        virtual_keyboard::VirtualKeyboardManagerState,
        xdg_activation::{
            XdgActivationHandler, XdgActivationState, XdgActivationToken, XdgActivationTokenData,
        },
    },
};

use crate::{state::unsandboxed, Nekoland};

/// How long after it was made a client may still use a token to bring a window forward.
const ACTIVATION_TOKEN_LIFETIME_SECS: u64 = 10;

/// The state of these protocols; most of it only has to stay alive.
pub struct Protocols {
    pub primary_selection: PrimarySelectionState,
    pub data_control: DataControlState,
    pub xdg_activation: XdgActivationState,
    _xdg_decoration: XdgDecorationState,
    _fractional_scale: FractionalScaleManagerState,
    _viewporter: ViewporterState,
    _single_pixel_buffer: SinglePixelBufferState,
    _security_context: SecurityContextState,
    pub session_lock: SessionLockManagerState,
    _idle_inhibit: IdleInhibitManagerState,
    shortcuts_inhibit: KeyboardShortcutsInhibitState,
    _pointer_constraints: PointerConstraintsState,
    _relative_pointer: RelativePointerManagerState,
}

impl Protocols {
    pub fn new(dh: &DisplayHandle) -> Self {
        // Input methods: the globals are all there is to keep. A sandboxed client cannot be
        // one, nor type into or read the clipboard of other programs.
        TextInputManagerState::new::<Nekoland>(dh);
        InputMethodManagerState::new::<Nekoland, _>(dh, unsandboxed);
        VirtualKeyboardManagerState::new::<Nekoland, _>(dh, unsandboxed);

        let primary_selection = PrimarySelectionState::new::<Nekoland>(dh);
        Self {
            data_control: DataControlState::new::<Nekoland, _>(dh, Some(&primary_selection), unsandboxed),
            primary_selection,
            xdg_activation: XdgActivationState::new::<Nekoland>(dh),
            _xdg_decoration: XdgDecorationState::new::<Nekoland>(dh),
            _fractional_scale: FractionalScaleManagerState::new::<Nekoland>(dh),
            _viewporter: ViewporterState::new::<Nekoland>(dh),
            _single_pixel_buffer: SinglePixelBufferState::new::<Nekoland>(dh),
            // A sandbox inside a sandbox would shed the outer one's limits.
            _security_context: SecurityContextState::new::<Nekoland, _>(dh, unsandboxed),
            session_lock: SessionLockManagerState::new::<Nekoland, _>(dh, unsandboxed),
            _idle_inhibit: IdleInhibitManagerState::new::<Nekoland>(dh),
            shortcuts_inhibit: KeyboardShortcutsInhibitState::new::<Nekoland>(dh),
            _pointer_constraints: PointerConstraintsState::new::<Nekoland>(dh),
            _relative_pointer: RelativePointerManagerState::new::<Nekoland>(dh),
        }
    }
}

impl PrimarySelectionHandler for Nekoland {
    fn primary_selection_state(&mut self) -> &mut PrimarySelectionState {
        &mut self.protocols.primary_selection
    }
}
delegate_primary_selection!(Nekoland);

impl DataControlHandler for Nekoland {
    fn data_control_state(&mut self) -> &mut DataControlState {
        &mut self.protocols.data_control
    }
}
delegate_data_control!(Nekoland);

// Windows draw their own title bars, as on GNOME.
// shortcut: a client that cannot is left without one; drawing borders for it needs server-side decorations.
impl XdgDecorationHandler for Nekoland {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        toplevel.with_pending_state(|state| state.decoration_mode = Some(DecorationMode::ClientSide));
    }

    fn request_mode(&mut self, toplevel: ToplevelSurface, _mode: DecorationMode) {
        self.unset_mode(toplevel);
    }

    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        toplevel.with_pending_state(|state| state.decoration_mode = Some(DecorationMode::ClientSide));
        if toplevel.is_initial_configure_sent() {
            toplevel.send_pending_configure();
        }
    }
}
delegate_xdg_decoration!(Nekoland);

impl XdgActivationHandler for Nekoland {
    fn activation_state(&mut self) -> &mut XdgActivationState {
        &mut self.protocols.xdg_activation
    }

    fn request_activation(
        &mut self,
        _token: XdgActivationToken,
        data: XdgActivationTokenData,
        surface: WlSurface,
    ) {
        if data.timestamp.elapsed().as_secs() >= ACTIVATION_TOKEN_LIFETIME_SECS {
            return;
        }
        if let Some(window) = self.window_for_surface(&surface) {
            self.focus_window(&window);
        }
    }
}
delegate_xdg_activation!(Nekoland);

impl FractionalScaleHandler for Nekoland {
    fn new_fractional_scale(&mut self, surface: WlSurface) {
        let scale = self.display_scale();
        with_states(&surface, |states| {
            with_fractional_scale(states, |fractional| fractional.set_preferred_scale(scale));
        });
    }
}

impl Nekoland {
    pub fn display_scale(&self) -> f64 {
        (self.space.outputs().next()).map_or(1.0, |output| output.current_scale().fractional_scale())
    }

    /// Tells the windows and panels there are the display's scale, after it changed: one
    /// that was open would go on drawing for the old one, and be shown stretched.
    pub fn tell_surfaces_the_scale(&self) {
        let scale = self.display_scale();
        let tell = |_: &WlSurface, states: &smithay::wayland::compositor::SurfaceData| {
            with_fractional_scale(states, |fractional| fractional.set_preferred_scale(scale));
        };
        for window in self.all_windows() {
            window.with_surfaces(tell);
        }
        for output in self.space.outputs() {
            for layer in smithay::desktop::layer_map_for_output(output).layers() {
                layer.with_surfaces(tell);
            }
        }
        self.scale_x11();
    }
}
delegate_fractional_scale!(Nekoland);

impl InputMethodHandler for Nekoland {
    fn new_popup(&mut self, surface: ImePopupSurface) {
        if let Err(err) = self.popups.track_popup(PopupKind::from(surface)) {
            tracing::warn!("input method popup: {err}");
        }
    }

    fn popup_repositioned(&mut self, _surface: ImePopupSurface) {}

    fn dismiss_popup(&mut self, surface: ImePopupSurface) {
        if let Some(parent) = surface.get_parent().map(|parent| parent.surface.clone()) {
            let _ = PopupManager::dismiss_popup(&parent, &PopupKind::from(surface));
        }
    }

    fn parent_geometry(&self, parent: &WlSurface) -> Rectangle<i32, Logical> {
        self.window_for_surface(parent)
            .map(|window| window.geometry())
            .unwrap_or_default()
    }
}
delegate_input_method_manager!(Nekoland);
delegate_text_input_manager!(Nekoland);
delegate_virtual_keyboard_manager!(Nekoland);

// A window that asks for every key gets them, Nekoland's shortcuts too, while it has the
// keyboard: a virtual machine, a remote desktop, or Settings taking a shortcut's new keys.
impl KeyboardShortcutsInhibitHandler for Nekoland {
    fn keyboard_shortcuts_inhibit_state(&mut self) -> &mut KeyboardShortcutsInhibitState {
        &mut self.protocols.shortcuts_inhibit
    }

    fn new_inhibitor(&mut self, inhibitor: KeyboardShortcutsInhibitor) {
        inhibitor.activate();
    }
}
delegate_keyboard_shortcuts_inhibit!(Nekoland);

// Games lock the pointer in place to look around with the mouse, or keep it in their window;
// Xwayland does both for X11 games that grab and warp the pointer.
impl PointerConstraintsHandler for Nekoland {
    fn new_constraint(&mut self, surface: &WlSurface, pointer: &PointerHandle<Self>) {
        if pointer.current_focus().as_ref() == Some(surface) {
            let under = self.surface_under(pointer.current_location());
            self.hold_pointer(pointer, under.as_ref());
        }
    }

    // Where a game says the pointer is, which is where it shows again once it is let go.
    fn cursor_position_hint(
        &mut self,
        surface: &WlSurface,
        pointer: &PointerHandle<Self>,
        location: Point<f64, Logical>,
    ) {
        let active = with_pointer_constraint(surface, pointer, |constraint| {
            constraint.is_some_and(|constraint| constraint.is_active())
        });
        if let Some((_, origin)) = self
            .surface_under(pointer.current_location())
            .filter(|(under, _)| active && under == surface)
        {
            pointer.set_location(origin + location);
        }
    }
}
delegate_pointer_constraints!(Nekoland);
delegate_relative_pointer!(Nekoland);

delegate_viewporter!(Nekoland);
delegate_single_pixel_buffer!(Nekoland);
