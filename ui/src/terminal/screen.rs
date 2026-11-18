//! VTE's terminal, told when its rows and columns change. With no pty of VTE's own to
//! resize, the shell's pty is resized from here.

use std::cell::RefCell;

use gtk::glib;
use vte4::prelude::*;

type ResizeHandler = Box<dyn Fn(u16, u16)>;
/// Draws the screen itself, given what draws VTE's own picture of it, for what VTE cannot draw.
pub type Painter = Box<dyn Fn(&vte4::Terminal, &gtk::Snapshot, &dyn Fn(&gtk::Snapshot))>;

mod imp {
    use std::cell::Cell;

    use gtk::{glib, prelude::*};
    use vte4::subclass::prelude::*;

    use super::{Painter, RefCell, ResizeHandler};

    #[derive(Default)]
    pub struct Screen {
        pub on_resize: RefCell<Option<ResizeHandler>>,
        pub painters: RefCell<Vec<Painter>>,
        size: Cell<(u16, u16)>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Screen {
        const NAME: &'static str = "NekolandTerminalScreen";
        type Type = super::Screen;
        type ParentType = vte4::Terminal;

        fn class_init(class: &mut Self::Class) {
            restore_parent_commit(class);
        }
    }

    /// vte4 0.8's `commit` trampoline takes the signal's text as its own and frees it, which the signal then frees
    /// again: a double free on keys such as Backspace. This screen has no use for the vfunc, so VTE's own is put back.
    fn restore_parent_commit(class: &mut glib::Class<super::Screen>) {
        // SAFETY: the class struct of a VTE subclass begins with `VteTerminalClass`, and the parent class is
        // initialised before any of its subclasses.
        unsafe {
            let own = class as *mut glib::Class<super::Screen> as *mut vte4::ffi::VteTerminalClass;
            let parent = Screen::type_data().as_ref().parent_class() as *const vte4::ffi::VteTerminalClass;
            (*own).commit = (*parent).commit;
        }
    }

    impl ObjectImpl for Screen {}

    impl WidgetImpl for Screen {
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let painters = self.painters.borrow();
            paint_layers(&painters, self.obj().upcast_ref(), snapshot, &|snapshot| self.parent_snapshot(snapshot));
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            self.parent_size_allocate(width, height, baseline);
            let size = self.obj().rows_and_columns();
            if size != self.size.replace(size) {
                if let Some(on_resize) = self.on_resize.borrow().as_ref() {
                    on_resize(size.0, size.1);
                }
            }
        }
    }

    impl TerminalImpl for Screen {}

    /// VTE's picture drawn through each painter in turn, the last added outermost.
    fn paint_layers(
        painters: &[Painter],
        screen: &vte4::Terminal,
        snapshot: &gtk::Snapshot,
        draw_screen: &dyn Fn(&gtk::Snapshot),
    ) {
        match painters.split_last() {
            Some((outer, inner)) => {
                outer(screen, snapshot, &|snapshot| paint_layers(inner, screen, snapshot, draw_screen));
            }
            None => draw_screen(snapshot),
        }
    }
}

glib::wrapper! {
    pub struct Screen(ObjectSubclass<imp::Screen>)
        @extends vte4::Terminal, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Scrollable;
}

impl Default for Screen {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl Screen {
    pub fn connect_resized(&self, on_resize: impl Fn(u16, u16) + 'static) {
        use gtk::subclass::prelude::ObjectSubclassIsExt;
        self.imp().on_resize.replace(Some(Box::new(on_resize)));
    }

    /// Draws the screen through `painter` too from now on, over the painters added before it.
    pub fn add_painter(&self, painter: Painter) {
        use gtk::subclass::prelude::ObjectSubclassIsExt;
        self.imp().painters.borrow_mut().push(painter);
    }

    pub fn rows_and_columns(&self) -> (u16, u16) {
        (
            u16::try_from(self.row_count()).unwrap_or(u16::MAX),
            u16::try_from(self.column_count()).unwrap_or(u16::MAX),
        )
    }
}
