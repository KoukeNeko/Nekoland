//! Choosing what to take: the screen as it was a moment ago fills the display, dimmed, and an
//! area is dragged out of it, a window pointed at, or the whole of it clicked. The area keeps
//! its handles until it is taken, moves with the arrow keys, shrinks to its content with A,
//! and a magnifier by the pointer shows its pixels.

use std::{cell::RefCell, f64::consts::TAU, process::Command, rc::Rc};

use gtk::{cairo, gdk, glib, prelude::*};

use super::img::{hex, Area, Img};
use super::marks::chip;

/// The window's title, by which the compositor shows and hides it at once.
pub const TITLE: &str = "截圖範圍";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Area,
    Window,
    Scroll,
    Ocr,
    /// A video of the area, or of the whole screen.
    Record,
}

/// What was chosen.
pub enum Choice {
    /// An area, in points.
    Area(Mode, (f64, f64, f64, f64)),
    Full,
    Delay(u32),
    Repeat,
    Clipboard,
    Cancel,
}

/// The screen as it was when 截圖 was asked for, and the windows on it.
pub struct Frozen {
    pub img: Rc<Img>,
    pub surface: cairo::ImageSurface,
    /// The display's size in points.
    pub size: (f64, f64),
    /// Each window, the topmost first, in points, and its title.
    pub windows: Vec<((f64, f64, f64, f64), String)>,
}

impl Frozen {
    /// Pixels a point.
    pub fn scale(&self) -> f64 {
        self.img.w as f64 / self.size.0.max(1.0)
    }

    pub fn crop(&self, area: (f64, f64, f64, f64)) -> Img {
        let k = self.scale();
        self.img.crop(Area::new(
            (area.0 * k).round() as i32,
            (area.1 * k).round() as i32,
            (area.2 * k).round() as i32,
            (area.3 * k).round() as i32,
        ))
    }
}

/// Takes the picture of the whole screen, through grim.
pub fn grab(geometry: Option<(f64, f64, f64, f64)>) -> Result<Img, String> {
    let mut command = Command::new("grim");
    if let Some((x, y, w, h)) = geometry {
        command
            .arg("-g")
            .arg(format!("{},{} {}x{}", x.round(), y.round(), w.round(), h.round()));
    }
    let out = command
        .args(["-t", "ppm", "-"])
        .output()
        .map_err(|err| format!("grim: {err}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Img::from_bytes(out.stdout)
}

/// The display's size in points, as the compositor tells it.
fn display_size(img: &Img) -> (f64, f64) {
    crate::ipc::msg("displays")
        .and_then(|reply| {
            let line = reply.lines().next()?.to_string();
            let size = line.split('\t').nth(1)?;
            let (w, h) = size.split_once('x')?;
            Some((w.parse().ok()?, h.parse().ok()?))
        })
        .unwrap_or((img.w as f64, img.h as f64))
}

pub fn freeze() -> Result<Frozen, String> {
    let img = grab(None)?;
    let size = display_size(&img);
    let windows = crate::ipc::msg("window-areas")
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let (area, title) = line.split_once('\t').unwrap_or((line, ""));
            let v: Vec<f64> = area.split_whitespace().filter_map(|s| s.parse().ok()).collect();
            (v.len() == 4 && v[2] > 0.0 && v[3] > 0.0).then(|| ((v[0], v[1], v[2], v[3]), title.to_string()))
        })
        .collect();
    let surface = img.surface();
    Ok(Frozen {
        img: Rc::new(img),
        surface,
        size,
        windows,
    })
}

#[derive(Clone, Copy)]
enum Drag {
    None,
    New((f64, f64)),
    Move((f64, f64)),
    /// A handle, 0 to 7 around from the top left, and the area when the drag began.
    Handle(usize, (f64, f64, f64, f64)),
}

struct State {
    mode: Mode,
    sel: Option<(f64, f64, f64, f64)>,
    drag: Drag,
    pointer: (f64, f64),
    pressed_at: (f64, f64),
    alt: bool,
}

fn normalize(a: (f64, f64), b: (f64, f64)) -> (f64, f64, f64, f64) {
    (a.0.min(b.0), a.1.min(b.1), (a.0 - b.0).abs(), (a.1 - b.1).abs())
}

fn handles(s: (f64, f64, f64, f64)) -> [(f64, f64); 8] {
    let (x, y, w, h) = s;
    [
        (x, y),
        (x + w / 2.0, y),
        (x + w, y),
        (x + w, y + h / 2.0),
        (x + w, y + h),
        (x + w / 2.0, y + h),
        (x, y + h),
        (x, y + h / 2.0),
    ]
}

fn inside(s: (f64, f64, f64, f64), p: (f64, f64)) -> bool {
    p.0 >= s.0 && p.0 <= s.0 + s.2 && p.1 >= s.1 && p.1 <= s.1 + s.3
}

/// The window under a point, in points, and its title.
fn window_at(frozen: &Frozen, p: (f64, f64)) -> Option<&((f64, f64, f64, f64), String)> {
    frozen.windows.iter().find(|(area, _)| inside(*area, p))
}

/// The area after A: shrunk to where its content starts.
fn trimmed(frozen: &Frozen, s: (f64, f64, f64, f64)) -> (f64, f64, f64, f64) {
    let k = frozen.scale();
    let a = frozen.img.trim(Area::new(
        (s.0 * k) as i32,
        (s.1 * k) as i32,
        (s.2 * k) as i32,
        (s.3 * k) as i32,
    ));
    (a.x as f64 / k, a.y as f64 / k, a.w as f64 / k, a.h as f64 / k)
}

/// What the button that takes the area says.
fn take_label(mode: Mode) -> &'static str {
    if mode == Mode::Record {
        "錄影"
    } else {
        "截取"
    }
}

/// The whole screen, as a picture or, recording, as the area of a video.
fn whole(mode: Mode, frozen: &Frozen) -> Choice {
    match mode {
        Mode::Record => Choice::Area(Mode::Record, (0.0, 0.0, frozen.size.0, frozen.size.1)),
        _ => Choice::Full,
    }
}

/// Shows the screen to choose from; `done` is told what was chosen once, as the window goes.
pub fn choose(app: &gtk::Application, frozen: Rc<Frozen>, mode: Mode, done: impl Fn(Choice) + 'static) {
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title(TITLE)
        .decorated(false)
        .default_width(frozen.size.0 as i32)
        .default_height(frozen.size.1 as i32)
        .build();
    window.add_css_class("shot-overlay");
    window.fullscreen();
    let state = Rc::new(RefCell::new(State {
        mode,
        sel: None,
        drag: Drag::None,
        pointer: (-1.0, -1.0),
        pressed_at: (0.0, 0.0),
        alt: false,
    }));
    let done: Rc<RefCell<Option<Box<dyn Fn(Choice)>>>> = Rc::new(RefCell::new(Some(Box::new(done))));
    let finish = {
        let window = window.clone();
        let done = done.clone();
        Rc::new(move |choice: Choice| {
            // Taken before closing, as closing asks this again.
            let taken = done.borrow_mut().take();
            if let Some(done) = taken {
                window.close();
                // Once the window is off the screen, for what is taken next.
                glib::timeout_add_local_once(std::time::Duration::from_millis(60), move || done(choice));
            }
        })
    };

    let area = gtk::DrawingArea::new();
    area.set_hexpand(true);
    area.set_vexpand(true);
    area.set_cursor_from_name(Some("crosshair"));
    {
        let frozen = frozen.clone();
        let state = state.clone();
        area.set_draw_func(move |_, cr, w, h| draw(cr, w as f64, h as f64, &frozen, &state.borrow()));
    }

    // The strip of modes along the bottom.
    let take = gtk::Button::builder()
        .label(take_label(mode))
        .css_classes(["suggested-action"])
        .build();
    take.set_visible(false);
    let strip = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    strip.add_css_class("shot-strip");
    strip.set_halign(gtk::Align::Center);
    strip.set_valign(gtk::Align::End);
    strip.set_margin_bottom(28);
    let mode_buttons: Rc<RefCell<Vec<(Mode, gtk::ToggleButton)>>> = Rc::default();
    let tool = |icon: &str, tip: &str| {
        gtk::Button::builder()
            .icon_name(icon)
            .tooltip_text(tip)
            .css_classes(["flat"])
            .build()
    };
    for (m, icon, tip) in [
        (Mode::Area, "shot-area-symbolic", "區域"),
        (Mode::Window, "shot-window-symbolic", "視窗"),
        (Mode::Scroll, "shot-scroll-symbolic", "捲動"),
        (Mode::Ocr, "shot-ocr-symbolic", "文字辨識"),
        (Mode::Record, "media-record-symbolic", "錄影"),
    ] {
        let button = gtk::ToggleButton::builder()
            .icon_name(icon)
            .tooltip_text(tip)
            .css_classes(["flat"])
            .build();
        button.set_active(m == mode);
        let state = state.clone();
        let area = area.clone();
        let buttons = mode_buttons.clone();
        let take = take.clone();
        button.connect_clicked(move |_| {
            state.borrow_mut().mode = m;
            for (other, b) in buttons.borrow().iter() {
                b.set_active(*other == m);
            }
            take.set_label(take_label(m));
            area.queue_draw();
        });
        mode_buttons.borrow_mut().push((m, button.clone()));
        strip.append(&button);
    }
    let full = tool("view-fullscreen-symbolic", "全螢幕");
    {
        let finish = finish.clone();
        let state = state.clone();
        let frozen = frozen.clone();
        full.connect_clicked(move |_| {
            let mode = state.borrow().mode;
            finish(whole(mode, &frozen));
        });
    }
    strip.append(&full);
    strip.append(&gtk::Separator::new(gtk::Orientation::Vertical));
    let delay_button = gtk::MenuButton::builder()
        .icon_name("alarm-symbolic")
        .tooltip_text("延遲")
        .css_classes(["flat"])
        .build();
    let delay_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
    for seconds in [3u32, 5, 10] {
        let item = gtk::Button::builder()
            .label(format!("{seconds} 秒"))
            .css_classes(["flat"])
            .build();
        let finish = finish.clone();
        item.connect_clicked(move |_| finish(Choice::Delay(seconds)));
        delay_box.append(&item);
    }
    delay_button.set_popover(Some(&gtk::Popover::builder().child(&delay_box).build()));
    strip.append(&delay_button);
    let repeat = tool("media-playlist-repeat-symbolic", "重複上次區域");
    {
        let finish = finish.clone();
        repeat.connect_clicked(move |_| finish(Choice::Repeat));
    }
    repeat.set_sensitive(super::output::last_area().is_some());
    strip.append(&repeat);
    let clipboard = tool("edit-paste-symbolic", "從剪貼簿載入");
    {
        let finish = finish.clone();
        clipboard.connect_clicked(move |_| finish(Choice::Clipboard));
    }
    strip.append(&clipboard);
    strip.append(&gtk::Separator::new(gtk::Orientation::Vertical));
    {
        let finish = finish.clone();
        let state = state.clone();
        take.connect_clicked(move |_| {
            let s = state.borrow();
            if let Some(sel) = s.sel {
                let mode = s.mode;
                drop(s);
                finish(Choice::Area(mode, sel));
            }
        });
    }
    strip.append(&take);
    let close = tool("window-close-symbolic", "取消");
    {
        let finish = finish.clone();
        close.connect_clicked(move |_| finish(Choice::Cancel));
    }
    strip.append(&close);

    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&area));
    overlay.add_overlay(&strip);
    window.set_child(Some(&overlay));

    let update_take = {
        let take = take.clone();
        let state = state.clone();
        move || take.set_visible(state.borrow().sel.is_some())
    };

    // The pointer: dragging out, moving and resizing the area; a click on a window takes it.
    let drag = gtk::GestureDrag::new();
    drag.set_button(gdk::BUTTON_PRIMARY);
    {
        let state = state.clone();
        let frozen = frozen.clone();
        let area_widget = area.clone();
        let update_take = update_take.clone();
        drag.connect_drag_begin(move |gesture, x, y| {
            let mut s = state.borrow_mut();
            s.pressed_at = (x, y);
            let ctrl = gesture
                .current_event_state()
                .contains(gdk::ModifierType::CONTROL_MASK);
            if ctrl {
                // A patch of one colour, chosen whole.
                let k = frozen.scale();
                let a = frozen.img.flood_bounds((x * k) as i32, (y * k) as i32);
                s.sel = Some((a.x as f64 / k, a.y as f64 / k, a.w as f64 / k, a.h as f64 / k));
                s.drag = Drag::None;
            } else if let Some(sel) = s.sel {
                let near = handles(sel)
                    .iter()
                    .position(|h| (h.0 - x).abs() < 10.0 && (h.1 - y).abs() < 10.0);
                s.drag = match near {
                    Some(i) => Drag::Handle(i, sel),
                    None if inside(sel, (x, y)) => Drag::Move((x - sel.0, y - sel.1)),
                    None => Drag::New((x, y)),
                };
            } else {
                s.drag = Drag::New((x, y));
            }
            drop(s);
            update_take();
            area_widget.queue_draw();
        });
    }
    {
        let state = state.clone();
        let area_widget = area.clone();
        let frozen = frozen.clone();
        let update_take = update_take.clone();
        drag.connect_drag_update(move |gesture, dx, dy| {
            let mut s = state.borrow_mut();
            let (x0, y0) = s.pressed_at;
            let (x, y) = (x0 + dx, y0 + dy);
            s.pointer = (x, y);
            s.alt = gesture
                .current_event_state()
                .contains(gdk::ModifierType::ALT_MASK);
            let (w, h) = frozen.size;
            match s.drag {
                Drag::New(start) if dx.abs() + dy.abs() > 3.0 => s.sel = Some(normalize(start, (x, y))),
                Drag::Move(off) => {
                    if let Some(sel) = s.sel {
                        s.sel = Some((
                            (x - off.0).clamp(0.0, w - sel.2),
                            (y - off.1).clamp(0.0, h - sel.3),
                            sel.2,
                            sel.3,
                        ));
                    }
                }
                Drag::Handle(i, (sx, sy, sw, sh)) => {
                    let (mut l, mut t, mut r, mut b) = (sx, sy, sx + sw, sy + sh);
                    if matches!(i, 0 | 6 | 7) {
                        l = x;
                    }
                    if matches!(i, 2..=4) {
                        r = x;
                    }
                    if matches!(i, 0..=2) {
                        t = y;
                    }
                    if matches!(i, 4..=6) {
                        b = y;
                    }
                    s.sel = Some(normalize((l, t), (r, b)));
                }
                _ => {}
            }
            drop(s);
            update_take();
            area_widget.queue_draw();
        });
    }
    {
        let state = state.clone();
        let frozen = frozen.clone();
        let finish = finish.clone();
        let area_widget = area.clone();
        drag.connect_drag_end(move |_, dx, dy| {
            let mut s = state.borrow_mut();
            let clicked = dx.abs() + dy.abs() <= 3.0;
            if let (true, Drag::New(p)) = (clicked, s.drag) {
                // A click: the window under it, or in 全螢幕 terms, nothing yet.
                if let Some((area, _)) = window_at(&frozen, p) {
                    let mode = if s.mode == Mode::Window {
                        Mode::Area
                    } else {
                        s.mode
                    };
                    let area = *area;
                    s.drag = Drag::None;
                    drop(s);
                    finish(Choice::Area(mode, clip_to(area, frozen.size)));
                    return;
                }
            }
            if s.alt {
                if let Some(sel) = s.sel {
                    s.sel = Some(trimmed(&frozen, sel));
                }
            }
            s.drag = Drag::None;
            drop(s);
            area_widget.queue_draw();
        });
    }
    area.add_controller(drag);

    let click = gtk::GestureClick::new();
    {
        let state = state.clone();
        let finish = finish.clone();
        click.connect_pressed(move |_, n, x, y| {
            let s = state.borrow();
            if n == 2 {
                if let Some(sel) = s.sel.filter(|sel| inside(*sel, (x, y))) {
                    let mode = s.mode;
                    drop(s);
                    finish(Choice::Area(mode, sel));
                }
            }
        });
    }
    area.add_controller(click);

    let motion = gtk::EventControllerMotion::new();
    {
        let state = state.clone();
        let area_widget = area.clone();
        motion.connect_motion(move |controller, x, y| {
            let mut s = state.borrow_mut();
            s.pointer = (x, y);
            s.alt = controller
                .current_event_state()
                .contains(gdk::ModifierType::ALT_MASK);
            drop(s);
            area_widget.queue_draw();
        });
    }
    area.add_controller(motion);

    let keys = gtk::EventControllerKey::new();
    // Before the strip's buttons, which would take Return and Space for themselves.
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let state = state.clone();
        let frozen = frozen.clone();
        let finish = finish.clone();
        let area_widget = area.clone();
        let mode_buttons = mode_buttons.clone();
        let update_take = update_take.clone();
        keys.connect_key_pressed(move |_, key, _, modifiers| {
            let shift = modifiers.contains(gdk::ModifierType::SHIFT_MASK);
            let ctrl = modifiers.contains(gdk::ModifierType::CONTROL_MASK);
            let step = if shift { 10.0 } else { 1.0 };
            let mut s = state.borrow_mut();
            match key {
                gdk::Key::Escape => {
                    if s.sel.take().is_none() {
                        drop(s);
                        finish(Choice::Cancel);
                        return glib::Propagation::Stop;
                    }
                }
                gdk::Key::Return | gdk::Key::KP_Enter | gdk::Key::space => {
                    let mode = s.mode;
                    let choice = match s.sel {
                        Some(sel) => Choice::Area(mode, sel),
                        None => whole(mode, &frozen),
                    };
                    drop(s);
                    finish(choice);
                    return glib::Propagation::Stop;
                }
                gdk::Key::Tab => {
                    let next = match s.mode {
                        Mode::Area => Mode::Window,
                        Mode::Window => Mode::Scroll,
                        Mode::Scroll => Mode::Ocr,
                        Mode::Ocr => Mode::Record,
                        Mode::Record => Mode::Area,
                    };
                    s.mode = next;
                    for (m, b) in mode_buttons.borrow().iter() {
                        b.set_active(*m == next);
                    }
                    take.set_label(take_label(next));
                }
                gdk::Key::a | gdk::Key::A => {
                    if let Some(sel) = s.sel {
                        s.sel = Some(trimmed(&frozen, sel));
                    }
                }
                gdk::Key::Left | gdk::Key::Right | gdk::Key::Up | gdk::Key::Down => {
                    let (dx, dy) = match key {
                        gdk::Key::Left => (-step, 0.0),
                        gdk::Key::Right => (step, 0.0),
                        gdk::Key::Up => (0.0, -step),
                        _ => (0.0, step),
                    };
                    if let Some((x, y, w, h)) = s.sel {
                        s.sel = Some(if ctrl {
                            (x, y, (w + dx).max(1.0), (h + dy).max(1.0))
                        } else {
                            (x + dx, y + dy, w, h)
                        });
                    } else {
                        s.pointer = (s.pointer.0 + dx, s.pointer.1 + dy);
                    }
                }
                gdk::Key::bracketright
                | gdk::Key::bracketleft
                | gdk::Key::braceright
                | gdk::Key::braceleft => {
                    let grow = if matches!(key, gdk::Key::bracketright | gdk::Key::braceright) {
                        step
                    } else {
                        -step
                    };
                    if let Some((x, y, w, h)) = s.sel {
                        s.sel = Some((
                            x - grow,
                            y - grow,
                            (w + grow * 2.0).max(1.0),
                            (h + grow * 2.0).max(1.0),
                        ));
                    }
                }
                gdk::Key::Alt_L | gdk::Key::Alt_R => s.alt = true,
                _ => return glib::Propagation::Proceed,
            }
            drop(s);
            update_take();
            area_widget.queue_draw();
            glib::Propagation::Stop
        });
    }
    {
        let state = state.clone();
        let area_widget = area.clone();
        keys.connect_key_released(move |_, key, _, _| {
            if matches!(key, gdk::Key::Alt_L | gdk::Key::Alt_R) {
                state.borrow_mut().alt = false;
                area_widget.queue_draw();
            }
        });
    }
    window.add_controller(keys);
    {
        let finish = finish.clone();
        window.connect_close_request(move |_| {
            finish(Choice::Cancel);
            glib::Propagation::Proceed
        });
    }
    window.present();
}

fn clip_to(a: (f64, f64, f64, f64), size: (f64, f64)) -> (f64, f64, f64, f64) {
    let x0 = a.0.clamp(0.0, size.0);
    let y0 = a.1.clamp(0.0, size.1);
    let x1 = (a.0 + a.2).clamp(0.0, size.0);
    let y1 = (a.1 + a.3).clamp(0.0, size.1);
    (x0, y0, x1 - x0, y1 - y0)
}

fn draw(cr: &cairo::Context, w: f64, h: f64, frozen: &Frozen, s: &State) {
    let k = frozen.img.w as f64 / w.max(1.0);
    cr.save().ok();
    cr.scale(1.0 / k, 1.0 / k);
    let _ = cr.set_source_surface(&frozen.surface, 0.0, 0.0);
    let _ = cr.paint();
    cr.restore().ok();

    // What is not chosen is dimmed.
    let lit = match (s.mode, s.sel) {
        (_, Some(sel)) => Some(sel),
        (Mode::Window, None) => window_at(frozen, s.pointer).map(|(a, _)| clip_to(*a, frozen.size)),
        _ => None,
    };
    cr.set_fill_rule(cairo::FillRule::EvenOdd);
    cr.rectangle(0.0, 0.0, w, h);
    if let Some((x, y, sw, sh)) = lit {
        cr.rectangle(x, y, sw, sh);
    }
    cr.set_source_rgba(
        0.0,
        0.0,
        0.0,
        if s.mode == Mode::Window && s.sel.is_none() {
            0.35
        } else {
            0.45
        },
    );
    let _ = cr.fill();
    cr.set_fill_rule(cairo::FillRule::Winding);

    let blue = (0.21, 0.52, 0.89);

    if let (Mode::Window, None) = (s.mode, s.sel) {
        if let Some((a, title)) = window_at(frozen, s.pointer) {
            let a = clip_to(*a, frozen.size);
            cr.rectangle(a.0, a.1, a.2, a.3);
            cr.set_source_rgba(blue.0, blue.1, blue.2, 0.22);
            let _ = cr.fill_preserve();
            cr.set_source_rgb(blue.0, blue.1, blue.2);
            cr.set_line_width(2.0);
            let _ = cr.stroke();
            let label = format!("{title}　{} × {}", a.2.round(), a.3.round());
            chip(cr, &label, (a.0 + a.2 / 2.0, (a.1 + 20.0).max(16.0)), 13.0);
        }
    }

    if let Some(sel) = s.sel {
        let (x, y, sw, sh) = sel;
        cr.rectangle(x + 0.5, y + 0.5, sw - 1.0, sh - 1.0);
        cr.set_source_rgb(1.0, 1.0, 1.0);
        cr.set_line_width(1.5);
        let _ = cr.stroke();
        for (hx, hy) in handles(sel) {
            cr.arc(hx, hy, 4.5, 0.0, TAU);
            cr.set_source_rgb(1.0, 1.0, 1.0);
            let _ = cr.fill_preserve();
            cr.set_source_rgb(blue.0, blue.1, blue.2);
            cr.set_line_width(1.5);
            let _ = cr.stroke();
        }
        let label = format!("{} × {}", sw.round(), sh.round());
        let above = if y > 30.0 { y - 16.0 } else { y + sh + 16.0 };
        chip(cr, &label, (x + 40.0, above), 12.0);
        if s.alt {
            let t = trimmed(frozen, sel);
            cr.rectangle(t.0, t.1, t.2, t.3);
            cr.set_dash(&[5.0, 4.0], 0.0);
            cr.set_source_rgb(blue.0, blue.1, blue.2);
            cr.set_line_width(1.5);
            let _ = cr.stroke();
            cr.set_dash(&[], 0.0);
        }
    } else if s.mode != Mode::Window && s.pointer.0 >= 0.0 {
        // Lines through the pointer, the width and height of the screen.
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.55);
        cr.set_line_width(1.0);
        cr.move_to(s.pointer.0.floor() + 0.5, 0.0);
        cr.line_to(s.pointer.0.floor() + 0.5, h);
        cr.move_to(0.0, s.pointer.1.floor() + 0.5);
        cr.line_to(w, s.pointer.1.floor() + 0.5);
        let _ = cr.stroke();
    }

    if s.pointer.0 >= 0.0 && s.mode != Mode::Window {
        loupe(cr, w, h, frozen, s.pointer, k);
    }
}

/// A magnifier by the pointer: the pixels around it, large, the one under it outlined, and
/// where it is with its colour.
fn loupe(cr: &cairo::Context, w: f64, h: f64, frozen: &Frozen, p: (f64, f64), k: f64) {
    let r = 56.0;
    let zoom = 10.0;
    let cx = if p.0 + 24.0 + r * 2.0 > w {
        p.0 - 24.0 - r
    } else {
        p.0 + 24.0 + r
    };
    let cy = if p.1 + 24.0 + r * 2.0 + 30.0 > h {
        p.1 - 24.0 - r - 30.0
    } else {
        p.1 + 24.0 + r
    };
    let (px, py) = ((p.0 * k).floor(), (p.1 * k).floor());
    cr.save().ok();
    cr.arc(cx, cy, r, 0.0, TAU);
    cr.clip();
    cr.set_source_rgb(0.2, 0.2, 0.22);
    let _ = cr.paint();
    cr.translate(cx, cy);
    cr.scale(zoom, zoom);
    cr.translate(-(px + 0.5), -(py + 0.5));
    let _ = cr.set_source_surface(&frozen.surface, 0.0, 0.0);
    cr.source().set_filter(cairo::Filter::Nearest);
    let _ = cr.paint();
    cr.restore().ok();
    cr.save().ok();
    // The grid of pixels and the one under the pointer.
    cr.arc(cx, cy, r, 0.0, TAU);
    cr.clip();
    cr.set_source_rgba(1.0, 1.0, 1.0, 0.12);
    cr.set_line_width(1.0);
    let mut x = cx - zoom / 2.0 - (r / zoom).ceil() * zoom;
    while x < cx + r {
        cr.move_to(x, cy - r);
        cr.line_to(x, cy + r);
        x += zoom;
    }
    let mut y = cy - zoom / 2.0 - (r / zoom).ceil() * zoom;
    while y < cy + r {
        cr.move_to(cx - r, y);
        cr.line_to(cx + r, y);
        y += zoom;
    }
    let _ = cr.stroke();
    cr.rectangle(cx - zoom / 2.0, cy - zoom / 2.0, zoom, zoom);
    cr.set_source_rgb(1.0, 1.0, 1.0);
    cr.set_line_width(1.5);
    let _ = cr.stroke();
    cr.restore().ok();
    cr.arc(cx, cy, r, 0.0, TAU);
    cr.set_source_rgb(1.0, 1.0, 1.0);
    cr.set_line_width(2.0);
    let _ = cr.stroke();
    let colour = frozen.img.rgba(px as i32, py as i32);
    let label = format!("{}, {}　{}", p.0.round(), p.1.round(), hex(colour));
    chip(cr, &label, (cx, cy + r + 16.0), 11.0);
}
