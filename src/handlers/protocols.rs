//! The smaller protocols clients expect from a desktop, each with little to decide.

use smithay::{
    delegate_data_control, delegate_fractional_scale, delegate_input_method_manager,
    delegate_primary_selection, delegate_single_pixel_buffer, delegate_text_input_manager,
    delegate_viewporter, delegate_virtual_keyboard_manager, delegate_xdg_activation, delegate_xdg_decoration,
    desktop::{PopupKind, PopupManager},
    reexports::{
        wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as DecorationMode,
        wayland_server::{protocol::wl_surface::WlSurface, DisplayHandle},
    },
    utils::{Logical, Rectangle},
    wayland::{
        compositor::with_states,
        fractional_scale::{with_fractional_scale, FractionalScaleHandler, FractionalScaleManagerState},
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

use crate::Nekoland;

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
}

impl Protocols {
    pub fn new(dh: &DisplayHandle) -> Self {
        // Input methods: the globals are all there is to keep.
        TextInputManagerState::new::<Nekoland>(dh);
        InputMethodManagerState::new::<Nekoland, _>(dh, |_client| true);
        VirtualKeyboardManagerState::new::<Nekoland, _>(dh, |_client| true);

        let primary_selection = PrimarySelectionState::new::<Nekoland>(dh);
        Self {
            data_control: DataControlState::new::<Nekoland, _>(dh, Some(&primary_selection), |_client| true),
            primary_selection,
            xdg_activation: XdgActivationState::new::<Nekoland>(dh),
            _xdg_decoration: XdgDecorationState::new::<Nekoland>(dh),
            _fractional_scale: FractionalScaleManagerState::new::<Nekoland>(dh),
            _viewporter: ViewporterState::new::<Nekoland>(dh),
            _single_pixel_buffer: SinglePixelBufferState::new::<Nekoland>(dh),
        }
    }
}

impl PrimarySelectionHandler for Nekoland {
    fn primary_selection_state(&self) -> &PrimarySelectionState {
        &self.protocols.primary_selection
    }
}
delegate_primary_selection!(Nekoland);

impl DataControlHandler for Nekoland {
    fn data_control_state(&self) -> &DataControlState {
        &self.protocols.data_control
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
        let scale = self
            .space
            .outputs()
            .next()
            .map(|output| output.current_scale().fractional_scale())
            .unwrap_or(1.0);
        with_states(&surface, |states| {
            with_fractional_scale(states, |fractional| fractional.set_preferred_scale(scale));
        });
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

delegate_viewporter!(Nekoland);
delegate_single_pixel_buffer!(Nekoland);
