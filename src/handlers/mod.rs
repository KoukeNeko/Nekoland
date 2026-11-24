pub mod appmenu;
pub mod background_effect;
mod compositor;
pub mod foreign_toplevel;
mod layer_shell;
pub mod protocols;
pub mod xdg_shell;

use crate::{focus::KeyFocus, Nekoland};

//
// Wl Seat
//

use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::input::dnd::{DnDGrab, DndGrabHandler, DndTarget, GrabType, Source};
use smithay::input::pointer::Focus;
use smithay::input::{Seat, SeatHandler, SeatState};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::Resource;
use smithay::wayland::dmabuf::{DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier};
use smithay::wayland::output::OutputHandler;
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::selection::data_device::{
    set_data_device_focus, DataDeviceHandler, DataDeviceState, WaylandDndGrabHandler,
};
use smithay::wayland::selection::{SelectionHandler, SelectionSource, SelectionTarget};
use smithay::wayland::tablet_manager::TabletSeatHandler;
use smithay::{delegate_cursor_shape, delegate_data_device, delegate_dmabuf, delegate_output, delegate_seat};

impl SeatHandler for Nekoland {
    type KeyboardFocus = KeyFocus;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Nekoland> {
        &mut self.seat_state
    }

    fn cursor_image(&mut self, _seat: &Seat<Self>, image: smithay::input::pointer::CursorImageStatus) {
        self.cursor_status = image;
    }

    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&KeyFocus>) {
        let dh = &self.display_handle;
        let client = focused
            .and_then(|focus| focus.wl_surface())
            .and_then(|surface| dh.get_client(surface.id()).ok());
        set_data_device_focus(dh, seat, client);
    }
}

delegate_seat!(Nekoland);

//
// Wl Data Device
//

// A Wayland program's selection is offered to X11 programs, and read for them.
impl SelectionHandler for Nekoland {
    /// Files copied from the desktop, which Nekoland itself gives out; none for what an
    /// X11 program copied.
    type SelectionUserData = Option<Vec<std::path::PathBuf>>;

    fn new_selection(&mut self, ty: SelectionTarget, source: Option<SelectionSource>, _seat: Seat<Self>) {
        if let Some(xwm) = self.xwm.as_mut() {
            if let Err(err) = xwm.new_selection(ty, source.map(|source| source.mime_types())) {
                tracing::warn!("offering the {ty:?} selection to X11 programs: {err}");
            }
        }
    }

    fn allow_data_control_read(&mut self, client: smithay::reexports::wayland_server::backend::ClientId) -> bool {
        self.may_read_clipboard(client)
    }

    fn send_selection(
        &mut self,
        ty: SelectionTarget,
        mime_type: String,
        fd: std::os::fd::OwnedFd,
        _seat: Seat<Self>,
        copied: &Option<Vec<std::path::PathBuf>>,
    ) {
        if let Some(paths) = copied {
            crate::desktop::send_copied(&mime_type, paths, fd);
        } else if let Some(xwm) = self.xwm.as_mut() {
            if let Err(err) = xwm.send_selection(ty, mime_type, fd) {
                tracing::warn!("reading the {ty:?} selection from an X11 program: {err}");
            }
        }
    }
}

impl Nekoland {
    /// Whether a program may read the clipboard in the background, as a clipboard manager
    /// does: Nekoland's own may, and another is asked the first time and refused until the
    /// person allows it, as the Mac asks before a program reads the pasteboard.
    // shortcut: a program is known by its executable's name, which another could take; a
    // sandbox's app id once clipboard managers come in sandboxes.
    fn may_read_clipboard(&mut self, client: smithay::reexports::wayland_server::backend::ClientId) -> bool {
        let Some(pid) = self.display_handle.backend_handle().get_client_credentials(client).ok().map(|it| it.pid) else {
            return false;
        };
        let exe = |pid: i32| {
            let path = std::fs::read_link(format!("/proc/{pid}/exe")).ok()?;
            Some(path.file_name()?.to_string_lossy().into_owned())
        };
        let parent = |pid: i32| -> Option<i32> {
            let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
            // The program's name, in parentheses, may hold spaces; the parent is field 4.
            stat.rsplit_once(')')?.1.split_whitespace().nth(1)?.parse().ok()
        };
        // Every program the Dock starts descends from the compositor, so the walk stops there.
        let compositor = std::process::id() as i32;
        let mut ancestor = Some(pid);
        while let Some(process) = ancestor.filter(|&process| process > 1 && process != compositor) {
            let service = u32::try_from(process).is_ok_and(|process| self.clipboard_services.contains(&process));
            if service || exe(process).is_some_and(|name| name.starts_with("nekoland-")) {
                return true;
            }
            ancestor = parent(process);
        }
        let Some(name) = exe(pid) else { return false };
        if let Some(&allowed) = self.config.clipboard_readers.get(&name) {
            return allowed;
        }
        let line = |allowed: bool| {
            let mut readers = self.config.clipboard_readers.clone();
            readers.insert(name.clone(), allowed);
            let readers: Vec<String> =
                readers.iter().map(|(name, allowed)| format!("{} = {allowed}", toml::Value::String(name.clone()))).collect();
            format!("clipboard_readers = {{ {} }}", readers.join(", "))
        };
        let (refused, allowing) = (line(false), line(true));
        if let Err(err) = crate::config::set(&refused) {
            tracing::warn!("{refused}: {err}");
            return false;
        }
        self.reload_config();
        // shortcut: the allowing line holds the list as it stands now, so of two programs
        // asked about at once and both allowed, the later one alone stays allowed.
        crate::config::spawn(&format!(
            "nekoland-confirm clipboard {} {}",
            crate::config::quote(&name),
            crate::config::quote(&allowing)
        ));
        false
    }
}

impl DataDeviceHandler for Nekoland {
    fn data_device_state(&mut self) -> &mut DataDeviceState {
        &mut self.data_device_state
    }
}

impl WaylandDndGrabHandler for Nekoland {
    fn dnd_requested<S: Source>(
        &mut self,
        source: S,
        icon: Option<WlSurface>,
        seat: Seat<Self>,
        serial: smithay::utils::Serial,
        type_: GrabType,
    ) {
        self.dnd_icon = icon.map(|icon| (icon, Default::default()));
        // Where what nothing takes goes back to.
        if let Some(start) = seat.get_pointer().and_then(|pointer| pointer.grab_start_data()) {
            let mimes = source.metadata().map(|meta| meta.mime_types).unwrap_or_default();
            self.desktop_drag_started_at(start.location, &mimes);
        }
        self.dock_drag_started(&source);
        let display = self.display_handle.clone();
        match type_ {
            GrabType::Pointer => {
                let Some(pointer) = seat.get_pointer() else { return };
                let Some(start) = pointer.grab_start_data() else { return };
                let grab = DnDGrab::new_pointer(&display, start, source, seat);
                pointer.set_grab(self, grab, serial, Focus::Keep);
            }
            GrabType::Touch => {
                let Some(touch) = seat.get_touch() else { return };
                let Some(start) = touch.grab_start_data() else { return };
                touch.set_grab(self, DnDGrab::new_touch(&display, start, source, seat), serial);
            }
        }
    }
}

impl DndGrabHandler for Nekoland {
    fn dropped(
        &mut self,
        _target: Option<DndTarget<'_, Self>>,
        validated: bool,
        _seat: Seat<Self>,
        _location: smithay::utils::Point<f64, smithay::utils::Logical>,
    ) {
        let icon = self.dnd_icon.take();
        // Files Nekoland dragged: no window took them, so Nekoland puts them.
        if self.desktop_dragging() {
            if !validated {
                self.desktop_dropped_here();
            }
        } else {
            // A program's icon, which it takes away as it lets go, is shown going on as
            // Nekoland's own does.
            let picture = icon.and_then(|(surface, offset)| Some((dragged_picture(&surface)?, offset)));
            self.desktop_program_dropped(picture, validated);
            self.dock_dropped(validated);
        }
        self.desktop_drag_ended();
    }
}

delegate_data_device!(Nekoland);

/// What a drag icon shows, as the compositor last drew it, kept past the program taking
/// the icon away.
fn dragged_picture(surface: &WlSurface) -> Option<crate::desktop::Dragged> {
    use smithay::backend::renderer::{gles::GlesTexture, utils::with_renderer_surface_state};
    with_renderer_surface_state(surface, |state| {
        let textures = state.textures::<GlesTexture>();
        let size = state.surface_size()?;
        (!textures.is_empty()).then(|| crate::desktop::Dragged::Program { textures, size, scale: state.buffer_scale() })
    })?
}

//
// Wl Output & Xdg Output
//

impl OutputHandler for Nekoland {}
delegate_output!(Nekoland);

//
// Cursor shapes by name, which need the tablet handler to exist
//

impl TabletSeatHandler for Nekoland {}
delegate_cursor_shape!(Nekoland);

//
// GPU buffers from clients
//

impl DmabufHandler for Nekoland {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.dmabuf_state
    }

    fn dmabuf_imported(&mut self, _global: &DmabufGlobal, dmabuf: Dmabuf, notifier: ImportNotifier) {
        let imported = self.dmabuf_import.as_mut().is_some_and(|import| import(&dmabuf));
        if imported {
            let _ = notifier.successful::<Nekoland>();
        } else {
            notifier.failed();
        }
    }
}
delegate_dmabuf!(Nekoland);
