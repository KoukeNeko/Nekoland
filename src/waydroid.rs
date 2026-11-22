//! Android's apps through Waydroid. Each of Android's tasks is a toplevel as large as
//! Android's whole display, see-through but for the task's window, the only part of it
//! drawn. That part is the window's body: it is made the window's geometry, and its
//! size the only one the window has, so that it is placed, drawn on glass, animated and
//! arranged as any other window is, and Android's display, which Waydroid resizes whole to a
//! size it is given, keeps its own.

use std::cell::Cell;

use smithay::{
    backend::renderer::utils::with_renderer_surface_state,
    desktop::Window,
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Rectangle},
    wayland::{
        compositor::{
            get_children, with_states, with_surface_tree_downward, BufferAssignment, RegionAttributes,
            SubsurfaceCachedState, SurfaceAttributes, TraversalAction,
        },
        shell::xdg::SurfaceCachedState,
    },
};

use crate::windows::window_names;

/// What Waydroid names a task's window: this, then the app's package. Its window with the
/// whole of Android, "Waydroid", is one window as any other.
const TASK_PREFIX: &str = "waydroid.";
/// What Waydroid names the window Android's input method draws in. It is as large as Android's
/// display and see-through but for what the input method shows, the candidates of what is being
/// typed among them, which Android puts by the text on its display.
const INPUT_METHOD_APP_ID: &str = "waydroid.InputMethod";

/// How tall Android's caption over a task's window is, which Nekoland's bar goes over.
/// shortcut: measured at Waydroid's density on a display at scale 2; read it from Android if
/// another density draws the caption at another height.
pub const CAPTION_HEIGHT: i32 = 28;

/// The body a window was last given.
#[derive(Default)]
struct Body(Cell<Option<Rectangle<i32, Logical>>>);

/// Makes a task window's body its geometry and its one size; where Android moves the task's
/// window within its display, the body stays where it is on screen. Whether the window is
/// ready to be placed: a task window is once it has a body.
pub fn fit_body(window: &Window, dark: bool) -> bool {
    let Some(toplevel) = window.toplevel().filter(|_| is_task(window)) else {
        return true;
    };
    let surface = toplevel.wl_surface();
    // Each of Android's layers is a subsurface, and those Waydroid no longer uses have no
    // buffer. The task's window is the largest; its menus and dialogs, smaller, may stand out
    // of it, as a window's popups do. The input region will not do: it adds a margin to grab
    // the edges by.
    let body = get_children(surface)
        .iter()
        .filter_map(|layer| {
            let size = with_renderer_surface_state(layer, |state| state.surface_size()).flatten()?;
            let location = with_states(layer, |states| {
                states
                    .cached_state
                    .get::<SubsurfaceCachedState>()
                    .current()
                    .location
            });
            Some(Rectangle::new(location, size))
        })
        .max_by_key(|layer| layer.size.w * layer.size.h);
    let Some(body) = body.filter(|body| !body.is_empty()) else {
        // Nothing of the task is drawn yet: it is placed and opens once there is.
        return given_body(window).get().is_some();
    };
    with_states(surface, |states| {
        let mut cached = states.cached_state.get::<SurfaceCachedState>();
        let current = cached.current();
        current.geometry = Some(body);
        current.min_size = body.size;
        current.max_size = body.size;
    });
    // Android may have started since Nekoland last told it: it is told again as each of its
    // windows first shows.
    if given_body(window).replace(Some(body)).is_none() {
        follow_dark(dark);
    }
    true
}

/// Whether the commit under way leaves a task window that has shown its body with no layer
/// drawn: Android took the task away, and Waydroid takes the window away next. Asked before
/// the commit is put in effect, while its layers still show what they showed.
pub fn loses_body(window: &Window) -> bool {
    let Some(toplevel) = window.toplevel().filter(|_| is_task(window)) else {
        return false;
    };
    given_body(window).get().is_some()
        && get_children(toplevel.wl_surface()).iter().all(|layer| {
            let attached = with_states(layer, |states| {
                match states.cached_state.get::<SurfaceAttributes>().current().buffer {
                    Some(BufferAssignment::Removed) => Some(false),
                    Some(BufferAssignment::NewBuffer(_)) => Some(true),
                    None => None,
                }
            });
            !attached.unwrap_or_else(|| {
                with_renderer_surface_state(layer, |state| state.buffer().is_some()).unwrap_or(false)
            })
        })
}

pub fn is_task(window: &Window) -> bool {
    let app_id = window_names(window).1;
    app_id.starts_with(TASK_PREFIX) && app_id != INPUT_METHOD_APP_ID
}

pub fn is_input_method(window: &Window) -> bool {
    window_names(window).1 == INPUT_METHOD_APP_ID
}

/// Has the pointer go through Android's input method window, which is see-through nearly all
/// over, to the window it lies over, which Waydroid hands on to Android: as the input method
/// window is put where Android's display is for that one, a point on what the input method
/// shows reaches the input method there.
pub fn pass_pointer_through(window: &Window) {
    let Some(toplevel) = window.toplevel().filter(|_| is_input_method(window)) else {
        return;
    };
    with_surface_tree_downward(
        toplevel.wl_surface(),
        (),
        |_, _, _| TraversalAction::DoChildren(()),
        |_, states, _| {
            states
                .cached_state
                .get::<SurfaceAttributes>()
                .current()
                .input_region = Some(RegionAttributes::default());
        },
        |_, _, _| true,
    );
}

/// The surface of a task window to point into: its toplevel, which stays where it is. Waydroid
/// takes a point on one of the task's layers as on the layer where it is now, while a press
/// holds on to where the layer was when it went down: as Android moves the layers when the
/// task's caption is dragged, the task would go on further and further.
pub fn task_surface(window: &Window) -> Option<WlSurface> {
    let toplevel = window.toplevel().filter(|_| is_task(window))?;
    Some(toplevel.wl_surface().clone())
}

fn given_body(window: &Window) -> &Cell<Option<Rectangle<i32, Logical>>> {
    &window.user_data().get_or_insert(Body::default).0
}

/// Tells Android to be dark, or not, as Nekoland is: an Android property the setup has
/// Android's dark mode follow. Android keeps the last it was told while it is not running.
pub fn follow_dark(dark: bool) {
    let night = if dark { "yes" } else { "no" };
    crate::config::spawn(&format!(
        "command -v waydroid >/dev/null && waydroid prop set persist.nekoland.night {night} >/dev/null 2>&1"
    ));
}
