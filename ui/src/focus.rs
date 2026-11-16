//! Fields as the Mac has them: none holds the keyboard as a window opens, and a click
//! anywhere but on one lets go of it. GTK gives a new window's first field the focus, and
//! keeps it there through clicks on what cannot take it.

use gtk::prelude::*;

/// Lets go of the focus `window` gave itself as it opened, and of a field's whenever a click
/// lands outside every field. Called once `window` is presented.
pub fn let_fields_go(window: &impl IsA<gtk::Window>) {
    let window = window.as_ref();
    GtkWindowExt::set_focus(window, None::<&gtk::Widget>);
    let click = gtk::GestureClick::new();
    // Before what is clicked: a button that takes the focus on a click still takes it.
    click.set_propagation_phase(gtk::PropagationPhase::Capture);
    click.connect_pressed(|click, _, x, y| {
        let Some(window) = click.widget().and_downcast::<gtk::Window>() else {
            return;
        };
        let in_field = window
            .pick(x, y, gtk::PickFlags::DEFAULT)
            .is_some_and(|picked| picked.ancestor(gtk::Editable::static_type()).is_some());
        if !in_field {
            GtkWindowExt::set_focus(&window, None::<&gtk::Widget>);
        }
    });
    window.add_controller(click);
}
