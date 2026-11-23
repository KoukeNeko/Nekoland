//! `nekoland-screenshare`: the window a window or a display to share is chosen in.
//! xdg-desktop-portal-wlr runs it as its chooser when a program, as Discord or a browser, asks
//! the portal to share the screen; it prints `Window: <id>` or `Monitor: <name>` for the one
//! chosen, and nothing when cancelled. `nekoland-screenshare stop` ends every screen share
//! xdg-desktop-portal-wlr serves.

use std::{cell::RefCell, rc::Rc};

use adw::prelude::*;
use gtk::{gdk, gio, glib};

mod edid;

const APP_ID: &str = "io.github.koukeneko.Nekoland.ScreenShare";
const WIDTH: i32 = 720;
const HEIGHT: i32 = 520;
/// How much smaller than the display grim takes a thumbnail: sharp enough, and quick.
const THUMB_SCALE: &str = "0.5";
const XDPW: &str = "org.freedesktop.impl.portal.desktop.wlr";
const SESSIONS: &str = "/org/freedesktop/portal/desktop/session";
const SESSION: &str = "org.freedesktop.impl.portal.Session";

/// The room around the tiles and the buttons, and between tiles, on the design language's
/// 4-point grid. The theme pads each tile by `SPACING` (`.thumbnails` in shell/gtk/nekoland.css),
/// so thumbnails are as far from each other as from the window's edge.
const MARGIN: i32 = 16;
const SPACING: u32 = 8;

fn main() -> glib::ExitCode {
    if std::env::args().nth(1).as_deref() == Some("stop") {
        return match stop_sharing() {
            Ok(()) => glib::ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("nekoland-screenshare: {err}");
                glib::ExitCode::FAILURE
            }
        };
    }
    // As the file chooser: the portal is busy with the request this window answers.
    std::env::set_var("GDK_DEBUG", "no-portals");
    std::env::set_var("ADW_DISABLE_PORTAL", "1");
    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.connect_activate(choose);
    app.run_with_args::<&str>(&[])
}

/// What a tile shares, as xdg-desktop-portal-wlr is told it: `Monitor: <name>` or
/// `Window: <ext-foreign-toplevel-list identifier>`.
struct Tile {
    child: gtk::FlowBoxChild,
    answer: String,
}

/// A window that can be shared, as `nekoland msg capturable-windows` lists it.
struct Shareable {
    identifier: String,
    app_id: String,
    title: String,
}

fn parse_windows(text: &str) -> Vec<Shareable> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.splitn(3, '\t');
            Some(Shareable {
                identifier: fields.next().filter(|id| !id.is_empty())?.to_string(),
                app_id: fields.next()?.to_string(),
                title: fields.next().unwrap_or_default().to_string(),
            })
        })
        .collect()
}

fn grid(per_line: u32) -> gtk::FlowBox {
    gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::Single)
        .activate_on_single_click(false)
        .homogeneous(true)
        .min_children_per_line(per_line)
        .max_children_per_line(per_line)
        .column_spacing(SPACING)
        .row_spacing(SPACING)
        .margin_start(MARGIN)
        .margin_end(MARGIN)
        .margin_top(MARGIN)
        .margin_bottom(MARGIN)
        .valign(gtk::Align::Start)
        .css_classes(["thumbnails"])
        .build()
}

fn choose(app: &adw::Application) {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("分享畫面")
        .default_width(WIDTH)
        .default_height(HEIGHT)
        .build();
    // On glass, as the system's alerts: shell/gtk/nekoland.css. Added, as setting the classes
    // would drop the window's own `background`.
    window.add_css_class("system-alert");
    let tiles: Rc<RefCell<Vec<Tile>>> = Rc::default();
    let add = |grid: &gtk::FlowBox, child: gtk::FlowBoxChild, answer: String| {
        grid.append(&child);
        tiles.borrow_mut().push(Tile { child, answer });
    };

    let windows = grid(3);
    let listed = std::process::Command::new("nekoland")
        .args(["msg", "capturable-windows"])
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
        .unwrap_or_default();
    for shareable in parse_windows(&listed).into_iter().filter(|shareable| shareable.app_id != APP_ID) {
        let (name, icon) = match gio::DesktopAppInfo::new(&format!("{}.desktop", shareable.app_id)) {
            Some(info) => (info.name().to_string(), info.icon()),
            None => (shareable.app_id.clone(), None),
        };
        let child = tile(
            ["-T", &shareable.identifier],
            (16, 10),
            icon.as_ref(),
            &name,
            &shareable.title,
        );
        add(&windows, child, format!("Window: {}", shareable.identifier));
    }

    let displays = grid(2);
    if let Some(display) = gdk::Display::default() {
        let monitors = display.monitors();
        for monitor in (0..monitors.n_items()).filter_map(|at| monitors.item(at).and_downcast::<gdk::Monitor>()) {
            let Some(connector) = monitor.connector().map(String::from) else {
                continue;
            };
            let area = monitor.geometry();
            let scale = monitor.scale_factor();
            let (name, _) =
                edid::connector(Some(&connector)).map_or_else(|| (connector.clone(), None), |path| edid::identity(&path));
            let size = format!("{} × {}", area.width() * scale, area.height() * scale);
            let child = tile(["-o", &connector], (area.width(), area.height()), None, &name, &size);
            add(&displays, child, format!("Monitor: {connector}"));
        }
    }

    let pages = gtk::Stack::builder().transition_type(gtk::StackTransitionType::Crossfade).build();
    let page = |grid: &gtk::FlowBox| {
        // As tall as the tiles would be, not as short as they could be squeezed, so that many
        // windows scroll.
        let viewport = gtk::Viewport::builder().vscroll_policy(gtk::ScrollablePolicy::Natural).child(grid).build();
        gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&viewport)
            .build()
    };
    let windows_page: gtk::Widget = if windows.child_at_index(0).is_some() {
        page(&windows).upcast()
    } else {
        adw::StatusPage::builder().title("沒有視窗").build().upcast()
    };
    pages.add_titled(&windows_page, Some("windows"), "視窗");
    pages.add_titled(&page(&displays), Some("displays"), "顯示器");

    let cancel = gtk::Button::with_label("取消");
    let share = gtk::Button::builder()
        .label("分享")
        .css_classes(["suggested-action"])
        .sensitive(false)
        .build();
    let buttons = gtk::Box::builder()
        .spacing(8)
        .halign(gtk::Align::End)
        .margin_top(MARGIN)
        .margin_bottom(MARGIN)
        .margin_start(MARGIN)
        .margin_end(MARGIN)
        .build();
    buttons.append(&cancel);
    buttons.append(&share);

    let header = adw::HeaderBar::new();
    // Only close, as an alert's, on the side the title bar's buttons are set to be.
    let layout = gtk::Settings::default().and_then(|settings| settings.gtk_decoration_layout()).unwrap_or_default();
    let close_only: Vec<String> = layout
        .split(':')
        .map(|side| side.split(',').filter(|button| !matches!(*button, "minimize" | "maximize")).collect::<Vec<_>>().join(","))
        .collect();
    header.set_decoration_layout(Some(&close_only.join(":")));
    // Which page, as Files chooses its views: libadwaita's toggle group, made from its name as
    // files.rs makes it, its `active` the index of the page.
    let toggles: String = ["視窗", "顯示器"]
        .iter()
        .map(|label| {
            format!(
                "<child><object class=\"AdwToggle\"><property name=\"label\">{label}</property></object></child>"
            )
        })
        .collect();
    let switch: gtk::Widget = gtk::Builder::from_string(&format!(
        "<interface><object class=\"AdwToggleGroup\" id=\"group\">{toggles}</object></interface>"
    ))
    .object("group")
    .expect("the toggle group just described");
    switch.connect_notify_local(
        Some("active"),
        glib::clone!(
            #[weak]
            pages,
            move |switch, _| {
                let page = if switch.property::<u32>("active") == 0 { "windows" } else { "displays" };
                pages.set_visible_child_name(page);
            }
        ),
    );
    header.set_title_widget(Some(&switch));
    let view = adw::ToolbarView::new();
    view.add_top_bar(&header);
    view.set_content(Some(&pages));
    view.add_bottom_bar(&buttons);
    window.set_content(Some(&view));
    window.set_default_widget(Some(&share));

    // What the shown page has selected: switching pages leaves the other's choice behind.
    let chosen = glib::clone!(
        #[strong]
        tiles,
        #[weak]
        pages,
        #[weak]
        windows,
        #[weak]
        displays,
        #[upgrade_or]
        None,
        move || {
            let grid = if pages.visible_child_name().as_deref() == Some("windows") { &windows } else { &displays };
            let selected = grid.selected_children();
            let tiles = tiles.borrow();
            selected
                .first()
                .and_then(|child| tiles.iter().find(|tile| tile.child == *child))
                .map(|tile| tile.answer.clone())
        }
    );
    let refresh = {
        let (share, chosen) = (share.clone(), chosen.clone());
        move || share.set_sensitive(chosen().is_some())
    };
    for grid in [&windows, &displays] {
        let refresh = refresh.clone();
        grid.connect_selected_children_changed(move |_| refresh());
    }
    pages.connect_visible_child_notify(move |_| refresh());

    let answer = {
        let window = window.downgrade();
        move |answer: Option<String>| {
            if let Some(answer) = answer {
                println!("{answer}");
            }
            if let Some(window) = window.upgrade() {
                window.close();
            }
        }
    };
    share.connect_clicked(glib::clone!(
        #[strong]
        answer,
        move |_| answer(chosen())
    ));
    for grid in [&windows, &displays] {
        let (answer, tiles) = (answer.clone(), tiles.clone());
        grid.connect_child_activated(move |_, child| {
            answer(tiles.borrow().iter().find(|tile| tile.child == *child).map(|tile| tile.answer.clone()))
        });
    }
    cancel.connect_clicked(move |_| answer(None));

    // With one display, it is chosen already: sharing it is one press.
    if let Some(only) = displays.child_at_index(0).filter(|_| displays.child_at_index(1).is_none()) {
        displays.select_child(&only);
    }
    window.present();
}

/// A display or a window as it is now, taken with grim by `what` and `shape` wide to high,
/// with its name and a detail under it.
fn tile(what: [&str; 2], shape: (i32, i32), icon: Option<&gio::Icon>, name: &str, detail: &str) -> gtk::FlowBoxChild {
    let shot = gtk::Picture::builder().content_fit(gtk::ContentFit::Contain).build();
    // Every tile one shape, whatever shape the window is: a tall one is shown narrower. An
    // empty picture of that shape gives the tile its height for its width, which an
    // AspectFrame does not: it is measured as its child is.
    let shape = gtk::Picture::for_paintable(&gdk::Paintable::new_empty(shape.0.max(1), shape.1.max(1)));
    let frame = gtk::Overlay::builder()
        .css_classes(["card"])
        .overflow(gtk::Overflow::Hidden)
        .child(&shape)
        .build();
    frame.add_overlay(&shot);
    let label = |text: &str, classes: &[&str]| {
        gtk::Label::builder()
            .label(text)
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(classes.to_vec())
            .build()
    };
    let words = gtk::Box::builder().orientation(gtk::Orientation::Vertical).hexpand(true).build();
    words.append(&label(name, &["heading"]));
    words.append(&label(detail, &["caption", "dim-label"]));
    let line = gtk::Box::builder().spacing(SPACING as i32).margin_top(SPACING as i32).build();
    if let Some(icon) = icon {
        line.append(&gtk::Image::builder().gicon(icon).pixel_size(32).valign(gtk::Align::Center).build());
    }
    line.append(&words);
    let column = gtk::Box::builder().orientation(gtk::Orientation::Vertical).build();
    column.append(&frame);
    column.append(&line);

    // Taken away from the window, as grim waits for the next frame of what it takes.
    let args: Vec<String> = what.iter().map(|arg| arg.to_string()).collect();
    glib::spawn_future_local(glib::clone!(
        #[weak]
        shot,
        async move {
            let taken = gio::spawn_blocking(move || {
                std::process::Command::new("grim").args(&args).args(["-s", THUMB_SCALE, "-t", "png", "-"]).output()
            })
            .await;
            match taken {
                Ok(Ok(out)) if out.status.success() => {
                    match gdk::Texture::from_bytes(&glib::Bytes::from_owned(out.stdout)) {
                        Ok(texture) => shot.set_paintable(Some(&texture)),
                        Err(err) => eprintln!("nekoland-screenshare: thumbnail: {err}"),
                    }
                }
                Ok(Ok(out)) => eprintln!("nekoland-screenshare: grim: {}", String::from_utf8_lossy(&out.stderr).trim()),
                Ok(Err(err)) => eprintln!("nekoland-screenshare: grim: {err}"),
                Err(_) => eprintln!("nekoland-screenshare: the thumbnail's thread panicked"),
            }
        }
    ));
    gtk::FlowBoxChild::builder().child(&column).build()
}

/// Closes every screen-share session xdg-desktop-portal-wlr has; each stream ends with it,
/// and the program sharing is told so.
fn stop_sharing() -> Result<(), glib::Error> {
    let bus = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE)?;
    for path in sessions(&bus, SESSIONS)? {
        bus.call_sync(
            Some(XDPW),
            &path,
            SESSION,
            "Close",
            None,
            None,
            gio::DBusCallFlags::NONE,
            -1,
            gio::Cancellable::NONE,
        )?;
    }
    Ok(())
}

/// The sessions under `path`: `session/<program's bus name>/<token>`.
fn sessions(bus: &gio::DBusConnection, path: &str) -> Result<Vec<String>, glib::Error> {
    let xml = bus.call_sync(
        Some(XDPW),
        path,
        "org.freedesktop.DBus.Introspectable",
        "Introspect",
        None,
        Some(glib::VariantTy::new("(s)").expect("a valid type")),
        gio::DBusCallFlags::NONE,
        -1,
        gio::Cancellable::NONE,
    )?;
    let (xml,) = xml.get::<(String,)>().unwrap_or_default();
    let node = gio::DBusNodeInfo::for_xml(&xml)?;
    if node.lookup_interface(SESSION).is_some() {
        return Ok(vec![path.to_string()]);
    }
    let mut found = Vec::new();
    for child in node.nodes() {
        if let Some(name) = child.path() {
            found.extend(sessions(bus, &format!("{path}/{name}"))?);
        }
    }
    Ok(found)
}
