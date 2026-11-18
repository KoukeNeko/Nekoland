//! How the tab strip of the title bar is laid out with the tabs on top: the tab bar as wide as its tabs want, of the
//! room the widgets after it leave, and those right after it, so that the new tab follows the last tab as in Nekominal
//! rather than sitting at the end of a stretched bar. libadwaita's tab bar asks for no width of its own.

use gtk::{glib, graphene, gsk, prelude::*, subclass::prelude::*};

/// How wide libadwaita draws a tab that is not stretched, and the bar's own padding around its tabs.
const TAB_WIDTH: i32 = 220;
const TAB_BAR_PADDING: i32 = 4;
const SPACING: i32 = 4;

/// The width the tabs of `widget` want, when it is a tab bar.
fn tabs_width(widget: &gtk::Widget) -> Option<i32> {
    let bar = widget.downcast_ref::<adw::TabBar>()?;
    let pages = bar.view().map_or(0, |view| view.n_pages());
    Some(pages * TAB_WIDTH + TAB_BAR_PADDING)
}

fn children(widget: &gtk::Widget) -> impl Iterator<Item = gtk::Widget> {
    std::iter::successors(widget.first_child(), |child| child.next_sibling()).filter(|child| child.should_layout())
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct StripLayout;

    #[glib::object_subclass]
    impl ObjectSubclass for StripLayout {
        const NAME: &'static str = "NekolandTerminalStripLayout";
        type Type = super::StripLayout;
        type ParentType = gtk::LayoutManager;
    }

    impl ObjectImpl for StripLayout {}

    impl LayoutManagerImpl for StripLayout {
        fn request_mode(&self, _widget: &gtk::Widget) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::ConstantSize
        }

        fn measure(&self, widget: &gtk::Widget, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let (minimum, natural) = children(widget).fold((0, 0), |(minimum, natural), child| {
                let (child_minimum, child_natural, _, _) = child.measure(orientation, for_size);
                match orientation {
                    gtk::Orientation::Horizontal => {
                        let child_natural = tabs_width(&child).unwrap_or(child_natural);
                        (minimum + child_minimum + SPACING, natural + child_natural + SPACING)
                    }
                    _ => (minimum.max(child_minimum), natural.max(child_natural)),
                }
            });
            (minimum, natural, -1, -1)
        }

        fn allocate(&self, widget: &gtk::Widget, width: i32, height: i32, _baseline: i32) {
            let natural_width = |child: &gtk::Widget| child.measure(gtk::Orientation::Horizontal, height).1;
            let others: i32 = children(widget)
                .filter(|child| tabs_width(child).is_none())
                .map(|child| natural_width(&child) + SPACING)
                .sum();
            let mut x = 0;
            for child in children(widget) {
                let child_width = match tabs_width(&child) {
                    Some(wanted) => wanted.min(width - others).max(0),
                    None => natural_width(&child),
                };
                let at = gsk::Transform::new().translate(&graphene::Point::new(x as f32, 0.0));
                child.allocate(child_width, height, -1, Some(at));
                x += child_width + SPACING;
            }
        }
    }
}

glib::wrapper! {
    pub struct StripLayout(ObjectSubclass<imp::StripLayout>) @extends gtk::LayoutManager;
}

impl Default for StripLayout {
    fn default() -> Self {
        glib::Object::new()
    }
}
