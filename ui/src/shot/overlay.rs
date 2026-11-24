//! Choosing what to take: the screen as it was a moment ago fills the display, and an area is
//! dragged out of it, a window pointed at, or the whole of it clicked.
//!
//! With the strip of tools (⇧⌘5), the screen is dimmed and the area keeps its handles until it
//! is taken, moves with the arrow keys, shrinks to its content with A, and a magnifier by the
//! pointer shows its pixels. Quick (⇧⌘4), as the Mac's: the area is taken as the button comes
//! up; Space toggles windows, or held while dragging moves the area; Shift keeps a side,
//! Option grows it from its centre, and Control sends it to the clipboard alone.

use std::{
    cell::{Cell, RefCell},
    f64::consts::TAU,
    path::Path,
    process::{Child, Command, Stdio},
    rc::Rc,
};

use gtk::{cairo, gdk, glib, graphene, gsk, prelude::*};

use super::img::{hex, Area, Img};
use super::marks::chip;

/// The window's title, by which the compositor shows and hides it at once.
pub const TITLE: &str = "截圖範圍";
/// How round the strip of tools is, as `.shot-strip` in shot.rs draws its outline.
const STRIP_RADIUS: f32 = 14.0;
/// How far the strip stays above the Dock, or the bottom of the screen.
const STRIP_GAP: f64 = 16.0;

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
    /// An area to put on the clipboard alone, in points.
    Copied((f64, f64, f64, f64)),
    /// A window, by its place in [`Frozen::windows`], and whether for the clipboard alone.
    Window(usize, bool),
    Full,
    Delay(u32),
    Repeat,
    Clipboard,
    Cancel,
}

/// The screen as it was when 截圖 was asked for, and the windows on it.
pub struct Frozen {
    pub img: Rc<Img>,
    /// The picture as the GPU draws it: kept there, so a frame redraws only what moved.
    pub texture: gdk::Texture,
    /// The display's size in points.
    pub size: (f64, f64),
    /// Each window, the topmost first.
    pub windows: Vec<Shown>,
}

/// A window on the frozen screen.
pub struct Shown {
    /// In points.
    pub area: (f64, f64, f64, f64),
    /// How round the compositor cuts its corners, in points; 0 where its program draws them.
    pub radius: f64,
    pub title: String,
    /// The name to take a picture of it alone by, when that picture is all of it.
    pub name: String,
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

    /// A window as the Mac takes one: all of it, what is behind it left out. Its program's
    /// own picture, corners as it draws them, round or square; otherwise as it was on the
    /// screen, what is outside the corners the compositor cuts cleared.
    pub fn window(&self, index: usize) -> Img {
        let window = &self.windows[index];
        if !window.name.is_empty() {
            match picture_of(grim_window(&window.name)) {
                Ok(img) => return img,
                Err(err) => eprintln!("nekoland-shot: the window alone: {err}"),
            }
        }
        let shown = self.window_area(index);
        let k = self.scale();
        let mut img = self.crop(shown);
        let area = window.area;
        let whole = ((area.0 - shown.0) * k, (area.1 - shown.1) * k, area.2 * k, area.3 * k);
        img.round_corners(whole, window.radius * k);
        img
    }

    /// A window's area on the screen, in points.
    pub fn window_area(&self, index: usize) -> (f64, f64, f64, f64) {
        clip_to(self.windows[index].area, self.size)
    }
}

/// How the area is chosen.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Style {
    /// With the strip of tools, the area kept until it is taken.
    Tools,
    /// Taken as the button comes up; `copy` for the clipboard alone.
    Quick { copy: bool },
}

thread_local! {
    /// The picture of the whole screen, asked for as 截圖 starts, before GTK.
    static EARLY: RefCell<Option<Child>> = const { RefCell::new(None) };
}

fn grim(geometry: Option<(f64, f64, f64, f64)>) -> std::io::Result<Child> {
    let mut command = Command::new("grim");
    if let Some((x, y, w, h)) = geometry {
        command
            .arg("-g")
            .arg(format!("{},{} {}x{}", x.round(), y.round(), w.round(), h.round()));
    }
    command
        .args(["-t", "ppm", "-"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
}

/// Takes a picture of one window alone, through grim: PNG, which keeps what is clear.
fn grim_window(name: &str) -> std::io::Result<Child> {
    Command::new("grim")
        .args(["-T", name, "-t", "png", "-l", "0", "-"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
}

/// Starts taking the screen as it is when the key is pressed, for the next `freeze`, while
/// GTK starts.
pub fn grab_early() {
    if let Ok(child) = grim(None) {
        EARLY.with(|early| early.replace(Some(child)));
    }
}

/// Takes the picture of the whole screen, through grim.
pub fn grab(geometry: Option<(f64, f64, f64, f64)>) -> Result<Img, String> {
    picture_of(grim(geometry))
}

fn picture_of(child: std::io::Result<Child>) -> Result<Img, String> {
    let out = child
        .and_then(Child::wait_with_output)
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
    let img = match EARLY.with(|early| early.take()) {
        Some(child) => picture_of(Ok(child))?,
        None => grab(None)?,
    };
    let size = display_size(&img);
    let windows = crate::ipc::msg("window-areas")
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let v: Vec<f64> = fields.next()?.split_whitespace().filter_map(|s| s.parse().ok()).collect();
            (v.len() >= 4 && v[2] > 0.0 && v[3] > 0.0).then(|| Shown {
                area: (v[0], v[1], v[2], v[3]),
                radius: v.get(4).copied().unwrap_or(0.0),
                title: fields.next().unwrap_or_default().to_string(),
                name: fields.next().unwrap_or_default().to_string(),
            })
        })
        .collect();
    let texture = img.texture();
    Ok(Frozen {
        img: Rc::new(img),
        texture,
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
    style: Style,
    sel: Option<(f64, f64, f64, f64)>,
    drag: Drag,
    pointer: (f64, f64),
    pressed_at: (f64, f64),
    alt: bool,
    /// Space held: dragging moves the area rather than sizing it.
    space: bool,
    /// Where the pointer was when Shift was pressed in a quick drag, and the corner then.
    lock: Option<(f64, f64)>,
    /// Where the strip of tools is, its glass drawn there; its top left once it is moved.
    strip: Option<(f64, f64, f64, f64)>,
    strip_moved_to: Option<(f64, f64)>,
    /// How solid the strip's glass is, as Appearance sets it for menus.
    glass_opacity: f64,
}

impl State {
    fn quick(&self) -> bool {
        matches!(self.style, Style::Quick { .. })
    }
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

/// The window under a point, by its place in [`Frozen::windows`].
fn window_at(frozen: &Frozen, p: (f64, f64)) -> Option<usize> {
    frozen.windows.iter().position(|window| inside(window.area, p))
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

/// Where pictures are saved, as the Mac's 選項 has it: 桌面, 文件, Pictures' 截圖 folder,
/// and the folder chosen in Settings when it is none of these.
fn save_menu() -> gtk::MenuButton {
    use super::prefs::Prefs;
    let current = Prefs::load().folder;
    let special = |dir| glib::user_special_dir(dir).map(|path| path.display().to_string());
    let mut places: Vec<(String, String)> = [
        ("桌面", special(glib::UserDirectory::Desktop)),
        ("文件", special(glib::UserDirectory::Documents)),
    ]
    .into_iter()
    .filter_map(|(label, folder)| Some((label.to_string(), folder?)))
    .chain([("圖片 › 截圖".to_string(), String::new())])
    .collect();
    if !places.iter().any(|(_, folder)| *folder == current) {
        let name = Path::new(&current)
            .file_name()
            .map_or(current.clone(), |name| name.to_string_lossy().into_owned());
        places.push((name, current.clone()));
    }
    let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let mut first: Option<gtk::CheckButton> = None;
    for (label, folder) in places {
        let path = match folder.as_str() {
            "" => Prefs::default().folder(),
            folder => folder.into(),
        };
        let item = gtk::CheckButton::builder()
            .label(label)
            .tooltip_text(path.display().to_string())
            .active(folder == current)
            .build();
        item.set_group(first.as_ref());
        first.get_or_insert(item.clone());
        item.connect_toggled(move |item| {
            if item.is_active() {
                // Read again, for what Settings changed since the strip opened.
                let mut prefs = Prefs::load();
                prefs.folder = folder.clone();
                prefs.auto_save = true;
                prefs.save();
            }
        });
        list.append(&item);
    }
    let button = gtk::MenuButton::builder()
        .icon_name("document-save-symbolic")
        .tooltip_text("儲存位置")
        .css_classes(["flat"])
        .build();
    button.set_popover(Some(&gtk::Popover::builder().child(&list).build()));
    button
}

/// Shows the screen to choose from; `done` is told what was chosen once, as the window goes.
pub fn choose(
    app: &gtk::Application,
    frozen: Rc<Frozen>,
    mode: Mode,
    style: Style,
    done: impl Fn(Choice) + 'static,
) {
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title(TITLE)
        .decorated(false)
        .default_width(frozen.size.0 as i32)
        .default_height(frozen.size.1 as i32)
        .build();
    window.add_css_class("shot-overlay");
    window.fullscreen();
    let quick = matches!(style, Style::Quick { .. });
    let state = Rc::new(RefCell::new(State {
        mode,
        style,
        // As the Mac's 截圖 remembers the last area.
        sel: if quick { None } else { super::output::last_area() },
        drag: Drag::None,
        pointer: (-1.0, -1.0),
        pressed_at: (0.0, 0.0),
        alt: false,
        space: false,
        lock: None,
        strip: None,
        strip_moved_to: None,
        glass_opacity: if quick { 0.0 } else { f64::from(super::apps::Look::load().glass_opacity) },
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

    let area = {
        let frozen = frozen.clone();
        let state = state.clone();
        canvas::Canvas::new(move |snapshot, w, h| draw(snapshot, w, h, &frozen, &state.borrow()))
    };
    area.set_hexpand(true);
    area.set_vexpand(true);
    area.set_cursor_from_name(Some(cursor(mode)));

    // The strip of modes along the bottom.
    let take = gtk::Button::builder()
        .label(take_label(mode))
        .css_classes(["suggested-action"])
        .build();
    take.set_visible(state.borrow().sel.is_some());
    let strip = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    strip.add_css_class("shot-strip");
    strip.set_visible(!quick);
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
    strip.append(&save_menu());
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
    place_strip(&overlay, &strip, &area, &state);

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
            if s.quick() {
                // Control is for the clipboard here, and a quick area is only ever new.
                s.sel = None;
                s.lock = None;
                s.drag = Drag::New((x, y));
            } else if ctrl {
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
            let held = gesture.current_event_state();
            let last = s.pointer;
            s.pointer = (x, y);
            s.alt = held.contains(gdk::ModifierType::ALT_MASK);
            let (w, h) = frozen.size;
            match s.drag {
                // Windows are pointed at, not dragged over.
                Drag::New(_) if s.quick() && s.mode == Mode::Window => {}
                Drag::New(start) if s.quick() => {
                    if s.space && s.sel.is_some() {
                        // The area goes with the pointer, its size kept.
                        let start = (start.0 + x - last.0, start.1 + y - last.1);
                        s.drag = Drag::New(start);
                        if let Some(sel) = s.sel {
                            s.sel = Some((sel.0 + x - last.0, sel.1 + y - last.1, sel.2, sel.3));
                        }
                    } else if dx.abs() + dy.abs() > 3.0 || s.sel.is_some() {
                        let mut corner = (x, y);
                        if held.contains(gdk::ModifierType::SHIFT_MASK) {
                            // Only the side the pointer goes on moves after Shift.
                            let at = *s.lock.get_or_insert(corner);
                            if (x - at.0).abs() > (y - at.1).abs() {
                                corner.1 = at.1;
                            } else {
                                corner.0 = at.0;
                            }
                        } else {
                            s.lock = None;
                        }
                        let from = if s.alt {
                            (2.0 * start.0 - corner.0, 2.0 * start.1 - corner.1)
                        } else {
                            start
                        };
                        s.sel = Some(normalize(from, corner));
                    }
                }
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
        drag.connect_drag_end(move |gesture, dx, dy| {
            let mut s = state.borrow_mut();
            let clicked = dx.abs() + dy.abs() <= 3.0;
            if let Style::Quick { copy } = s.style {
                let copy = copy
                    || gesture
                        .current_event_state()
                        .contains(gdk::ModifierType::CONTROL_MASK);
                let mode = s.mode;
                let p = s.pressed_at;
                if mode == Mode::Window {
                    s.drag = Drag::None;
                    drop(s);
                    match window_at(&frozen, p).filter(|_| clicked) {
                        Some(index) => finish(Choice::Window(index, copy)),
                        None => area_widget.queue_draw(),
                    }
                    return;
                }
                let chosen = s
                    .sel
                    .map(|sel| clip_to(sel, frozen.size))
                    .filter(|a| a.2 >= 1.0 && a.3 >= 1.0);
                s.drag = Drag::None;
                s.sel = None;
                drop(s);
                match chosen {
                    Some(a) if copy => finish(Choice::Copied(a)),
                    Some(a) => finish(Choice::Area(Mode::Area, a)),
                    None => area_widget.queue_draw(),
                }
                return;
            }
            if let (true, Drag::New(p)) = (clicked, s.drag) {
                // A click: the window under it, or in 全螢幕 terms, nothing yet.
                if let Some(index) = window_at(&frozen, p) {
                    let mode = s.mode;
                    s.drag = Drag::None;
                    drop(s);
                    finish(match mode {
                        Mode::Area | Mode::Window => Choice::Window(index, false),
                        _ => Choice::Area(mode, frozen.window_area(index)),
                    });
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
            if s.quick() {
                match key {
                    gdk::Key::Escape => {
                        drop(s);
                        finish(Choice::Cancel);
                    }
                    // Held, it repeats: only its first press counts.
                    gdk::Key::space if !s.space => {
                        s.space = true;
                        if matches!(s.drag, Drag::None) {
                            s.mode = if s.mode == Mode::Window {
                                Mode::Area
                            } else {
                                Mode::Window
                            };
                            area_widget.set_cursor_from_name(Some(cursor(s.mode)));
                        }
                        drop(s);
                        area_widget.queue_draw();
                    }
                    gdk::Key::space => {}
                    _ => return glib::Propagation::Proceed,
                }
                return glib::Propagation::Stop;
            }
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
            if key == gdk::Key::space {
                state.borrow_mut().space = false;
            }
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

/// Puts the strip of tools above the Dock, in the middle, as the Mac does, and lets it be
/// dragged anywhere on the screen by any part of it.
fn place_strip(overlay: &gtk::Overlay, strip: &gtk::Box, area: &impl IsA<gtk::Widget>, state: &Rc<RefCell<State>>) {
    let usable_bottom = crate::ipc::msg("usable-area").and_then(|reply| {
        let v: Vec<f64> = reply.split_whitespace().filter_map(|s| s.parse().ok()).collect();
        (v.len() == 4).then(|| v[1] + v[3])
    });
    {
        let state = state.clone();
        let area = area.clone().upcast::<gtk::Widget>();
        overlay.connect_get_child_position(move |overlay, child| {
            let (_, size) = child.preferred_size();
            let (w, h) = (f64::from(size.width()), f64::from(size.height()));
            let (screen_w, screen_h) = (f64::from(overlay.width()), f64::from(overlay.height()));
            let mut s = state.borrow_mut();
            let (x, y) = s.strip_moved_to.unwrap_or_else(|| {
                let bottom = usable_bottom.unwrap_or(screen_h).min(screen_h);
                ((screen_w - w) / 2.0, bottom - h - STRIP_GAP)
            });
            let x = x.clamp(0.0, (screen_w - w).max(0.0));
            let y = y.clamp(0.0, (screen_h - h).max(0.0));
            let placed = Some((x, y, w, h));
            if s.strip != placed {
                s.strip = placed;
                area.queue_draw();
            }
            Some(gdk::Rectangle::new(x.round() as i32, y.round() as i32, size.width(), size.height()))
        });
    }
    // In the capture phase, so that a drag that starts on a button moves the strip; the
    // button keeps a click, as the drag takes the pointer only once it goes a little way.
    let drag = gtk::GestureDrag::new();
    drag.set_propagation_phase(gtk::PropagationPhase::Capture);
    // The pointer on the screen, as the strip moves under it: where the strip was when the
    // drag began and where the pointer was then.
    let on_screen = {
        let (strip, overlay) = (strip.clone(), overlay.clone());
        move |gesture: &gtk::GestureDrag| {
            let (x, y) = gesture.point(None)?;
            let p = strip.compute_point(&overlay, &graphene::Point::new(x as f32, y as f32))?;
            Some((f64::from(p.x()), f64::from(p.y())))
        }
    };
    let from: Rc<Cell<Option<((f64, f64), (f64, f64))>>> = Rc::default();
    let moving = Rc::new(Cell::new(false));
    {
        let state = state.clone();
        let from = from.clone();
        let moving = moving.clone();
        let on_screen = on_screen.clone();
        drag.connect_drag_begin(move |gesture, _, _| {
            moving.set(false);
            let strip = state.borrow().strip.map(|(x, y, _, _)| (x, y));
            from.set(strip.zip(on_screen(gesture)));
        });
    }
    {
        let state = state.clone();
        let overlay = overlay.clone();
        drag.connect_drag_update(move |gesture, dx, dy| {
            let (Some((strip, pressed)), Some(pointer)) = (from.get(), on_screen(gesture)) else {
                return;
            };
            if !moving.get() {
                if dx.hypot(dy) < 4.0 {
                    return;
                }
                moving.set(true);
                gesture.set_state(gtk::EventSequenceState::Claimed);
            }
            state.borrow_mut().strip_moved_to = Some((strip.0 + pointer.0 - pressed.0, strip.1 + pointer.1 - pressed.1));
            overlay.queue_allocate();
        });
    }
    strip.add_controller(drag);
}

/// Catseye glass, as the compositor draws it behind menus: what is `under` blurred and a
/// little more saturated, the tint over it, and a line of light just inside the top edge.
fn glass(snapshot: &gtk::Snapshot, under: &gsk::RenderNode, (x, y, w, h): (f64, f64, f64, f64), opacity: f64) {
    const BLUR: f64 = 30.0;
    const SATURATION: f32 = 1.4;
    let shape = gsk::RoundedRect::from_rect(rect(x, y, w, h), STRIP_RADIUS);
    snapshot.push_rounded_clip(&shape);
    // As CSS's saturate(): a row for each channel in, by how bright it looks.
    let row = |weight: f32, own: usize| {
        let mut r = [weight * (1.0 - SATURATION); 4];
        r[own] += SATURATION;
        r[3] = 0.0;
        r
    };
    let (red, green, blue) = (row(0.2126, 0), row(0.7152, 1), row(0.0722, 2));
    let matrix = graphene::Matrix::from_float([
        red[0], red[1], red[2], 0.0, green[0], green[1], green[2], 0.0, blue[0], blue[1], blue[2], 0.0, 0.0, 0.0, 0.0,
        1.0,
    ]);
    snapshot.push_color_matrix(&matrix, &graphene::Vec4::zero());
    snapshot.push_blur(BLUR);
    // Only what the blur reaches is drawn again.
    snapshot.push_clip(&rect(x - 2.0 * BLUR, y - 2.0 * BLUR, w + 4.0 * BLUR, h + 4.0 * BLUR));
    snapshot.append_node(under);
    snapshot.pop();
    snapshot.pop();
    snapshot.pop();
    // The design language's sheet, dark, as the strip's white symbols need.
    snapshot.append_color(&rgba(0.133, 0.133, 0.149, opacity), &rect(x, y, w, h));
    // White fading out downward: faintly over all of it, and brightly in the top few pixels.
    let light = |height: f64, alpha: f64| {
        let stops = [
            gsk::ColorStop::new(0.0, rgba(1.0, 1.0, 1.0, alpha)),
            gsk::ColorStop::new(1.0, rgba(1.0, 1.0, 1.0, 0.0)),
        ];
        let (top, bottom) = (graphene::Point::new(x as f32, y as f32), graphene::Point::new(x as f32, (y + height) as f32));
        snapshot.append_linear_gradient(&rect(x, y, w, height), &top, &bottom, &stops);
    };
    light(h, 0.04);
    light(4.0, 0.22);
    snapshot.pop();
}

/// The pointer over the screen: a crosshair for an area, the usual arrow for a window.
fn cursor(mode: Mode) -> &'static str {
    if mode == Mode::Window {
        "default"
    } else {
        "crosshair"
    }
}

fn rect(x: f64, y: f64, w: f64, h: f64) -> graphene::Rect {
    graphene::Rect::new(x as f32, y as f32, w.max(0.0) as f32, h.max(0.0) as f32)
}

fn rgba(r: f64, g: f64, b: f64, a: f64) -> gdk::RGBA {
    gdk::RGBA::new(r as f32, g as f32, b as f32, a as f32)
}

/// Cairo for what only it draws, over no more than `bounds`: a frame drawn whole in cairo
/// is redrawn and sent to the GPU entire, which a display of 4K cannot do at its rate.
fn cairo_in(snapshot: &gtk::Snapshot, bounds: (f64, f64, f64, f64), f: impl FnOnce(&cairo::Context)) {
    let cr = snapshot.append_cairo(&rect(bounds.0, bounds.1, bounds.2, bounds.3));
    f(&cr);
}

fn label_at(snapshot: &gtk::Snapshot, label: &str, at: (f64, f64), size: f64) {
    cairo_in(
        snapshot,
        (at.0 - 600.0, at.1 - size * 2.0, 1200.0, size * 4.0),
        |cr| chip(cr, label, at, size),
    );
}

fn outline(snapshot: &gtk::Snapshot, a: (f64, f64, f64, f64), width: f64, colour: gdk::RGBA) {
    let r = gsk::RoundedRect::from_rect(rect(a.0, a.1, a.2, a.3), 0.0);
    let w = width as f32;
    snapshot.append_border(&r, &[w; 4], &[colour; 4]);
}

fn draw(snapshot: &gtk::Snapshot, w: f64, h: f64, frozen: &Frozen, s: &State) {
    let scene = gtk::Snapshot::new();
    draw_scene(&scene, w, h, frozen, s);
    let Some(scene) = scene.to_node() else {
        return;
    };
    snapshot.append_node(&scene);
    if let Some(strip) = s.strip.filter(|_| !s.quick()) {
        glass(snapshot, &scene, strip, s.glass_opacity);
    }
}

fn draw_scene(snapshot: &gtk::Snapshot, w: f64, h: f64, frozen: &Frozen, s: &State) {
    snapshot.append_texture(&frozen.texture, &rect(0.0, 0.0, w, h));
    let blue = (0.21, 0.52, 0.89);
    let hovered = match (s.mode, s.sel) {
        (Mode::Window, None) => window_at(frozen, s.pointer).map(|i| (frozen.window_area(i), &frozen.windows[i].title)),
        _ => None,
    };

    if s.quick() {
        // As the Mac's: the screen as it is, the area in grey, the window under the pointer in blue.
        if let Some((a, _)) = hovered {
            snapshot.append_color(&rgba(blue.0, blue.1, blue.2, 0.3), &rect(a.0, a.1, a.2, a.3));
        }
        if let Some(sel) = s.sel {
            snapshot.append_color(&rgba(0.5, 0.5, 0.5, 0.3), &rect(sel.0, sel.1, sel.2, sel.3));
            outline(snapshot, sel, 1.0, rgba(1.0, 1.0, 1.0, 0.8));
        }
        if s.mode != Mode::Window && s.pointer.0 >= 0.0 {
            let label = match s.sel {
                Some(sel) => format!("{} × {}", sel.2.round(), sel.3.round()),
                None => format!("{}, {}", s.pointer.0.round(), s.pointer.1.round()),
            };
            let x = if s.pointer.0 + 120.0 > w {
                s.pointer.0 - 60.0
            } else {
                s.pointer.0 + 60.0
            };
            let y = if s.pointer.1 + 40.0 > h {
                s.pointer.1 - 24.0
            } else {
                s.pointer.1 + 24.0
            };
            label_at(snapshot, &label, (x, y), 11.0);
        }
        return;
    }

    // What is not chosen is dimmed.
    let lit = s.sel.or(hovered.map(|(a, _)| a));
    let dim = rgba(0.0, 0.0, 0.0, if hovered.is_some() { 0.35 } else { 0.45 });
    match lit {
        Some((x, y, sw, sh)) => {
            snapshot.append_color(&dim, &rect(0.0, 0.0, w, y));
            snapshot.append_color(&dim, &rect(0.0, y + sh, w, h - y - sh));
            snapshot.append_color(&dim, &rect(0.0, y, x, sh));
            snapshot.append_color(&dim, &rect(x + sw, y, w - x - sw, sh));
        }
        None => snapshot.append_color(&dim, &rect(0.0, 0.0, w, h)),
    }

    if let Some((a, title)) = hovered {
        snapshot.append_color(&rgba(blue.0, blue.1, blue.2, 0.22), &rect(a.0, a.1, a.2, a.3));
        outline(snapshot, a, 2.0, rgba(blue.0, blue.1, blue.2, 1.0));
        let label = format!("{title}　{} × {}", a.2.round(), a.3.round());
        label_at(snapshot, &label, (a.0 + a.2 / 2.0, (a.1 + 20.0).max(16.0)), 13.0);
    }

    if let Some(sel) = s.sel {
        let (x, y, sw, sh) = sel;
        outline(snapshot, sel, 1.5, rgba(1.0, 1.0, 1.0, 1.0));
        for (hx, hy) in handles(sel) {
            cairo_in(snapshot, (hx - 7.0, hy - 7.0, 14.0, 14.0), |cr| {
                cr.arc(hx, hy, 4.5, 0.0, TAU);
                cr.set_source_rgb(1.0, 1.0, 1.0);
                let _ = cr.fill_preserve();
                cr.set_source_rgb(blue.0, blue.1, blue.2);
                cr.set_line_width(1.5);
                let _ = cr.stroke();
            });
        }
        let label = format!("{} × {}", sw.round(), sh.round());
        let above = if y > 30.0 { y - 16.0 } else { y + sh + 16.0 };
        label_at(snapshot, &label, (x + 40.0, above), 12.0);
        if s.alt {
            let t = trimmed(frozen, sel);
            // Each side on its own, so cairo draws lines rather than the whole area.
            for (ax, ay, bx, by) in [
                (t.0, t.1, t.0 + t.2, t.1),
                (t.0 + t.2, t.1, t.0 + t.2, t.1 + t.3),
                (t.0, t.1 + t.3, t.0 + t.2, t.1 + t.3),
                (t.0, t.1, t.0, t.1 + t.3),
            ] {
                let bounds = (
                    ax.min(bx) - 2.0,
                    ay.min(by) - 2.0,
                    (bx - ax).abs() + 4.0,
                    (by - ay).abs() + 4.0,
                );
                cairo_in(snapshot, bounds, |cr| {
                    cr.move_to(ax, ay);
                    cr.line_to(bx, by);
                    cr.set_dash(&[5.0, 4.0], 0.0);
                    cr.set_source_rgb(blue.0, blue.1, blue.2);
                    cr.set_line_width(1.5);
                    let _ = cr.stroke();
                });
            }
        }
    } else if s.mode != Mode::Window && s.pointer.0 >= 0.0 {
        // Lines through the pointer, the width and height of the screen.
        let line = rgba(1.0, 1.0, 1.0, 0.55);
        snapshot.append_color(&line, &rect(s.pointer.0.floor(), 0.0, 1.0, h));
        snapshot.append_color(&line, &rect(0.0, s.pointer.1.floor(), w, 1.0));
    }

    if s.pointer.0 >= 0.0 && s.mode != Mode::Window {
        loupe(snapshot, w, h, frozen, s.pointer);
    }
}

/// A magnifier by the pointer: the pixels around it, large, the one under it outlined, and
/// where it is with its colour.
fn loupe(snapshot: &gtk::Snapshot, w: f64, h: f64, frozen: &Frozen, p: (f64, f64)) {
    let r = 56.0;
    let zoom = 10.0;
    let k = frozen.scale();
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
    let (px, py) = ((p.0 * k).floor() as i32, (p.1 * k).floor() as i32);
    // Only the pixels the magnifier shows, around the one under the pointer.
    let n = (r / zoom).ceil() as i32 + 1;
    let side = 2 * n + 1;
    let wanted = Area::new(px - n, py - n, side, side);
    let there = wanted.clamp(frozen.img.w, frozen.img.h);
    let mut patch = Img::new(side, side, [51, 51, 56, 255]);
    patch.paste(&frozen.img.crop(there), there.x - wanted.x, there.y - wanted.y);
    let lens = rect(cx - r, cy - r, r * 2.0, r * 2.0);
    snapshot.push_rounded_clip(&gsk::RoundedRect::from_rect(lens, r as f32));
    let left = cx - (n as f64 + 0.5) * zoom;
    let top = cy - (n as f64 + 0.5) * zoom;
    let size = side as f64 * zoom;
    snapshot.append_scaled_texture(
        &patch.texture(),
        gsk::ScalingFilter::Nearest,
        &rect(left, top, size, size),
    );
    snapshot.pop();
    cairo_in(
        snapshot,
        (cx - r - 2.0, cy - r - 2.0, r * 2.0 + 4.0, r * 2.0 + 4.0),
        |cr| {
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
        },
    );
    let colour = frozen.img.rgba(px, py);
    let label = format!("{}, {}　{}", p.0.round(), p.1.round(), hex(colour));
    label_at(snapshot, &label, (cx, cy + r + 16.0), 11.0);
}

/// A widget drawn by a function into GTK's snapshot, which the GPU renders.
mod canvas {
    use gtk::{glib, subclass::prelude::*};

    type Paint = Box<dyn Fn(&gtk::Snapshot, f64, f64)>;

    mod imp {
        use std::cell::RefCell;

        use gtk::{glib, prelude::*, subclass::prelude::*};

        #[derive(Default)]
        pub struct Canvas {
            pub paint: RefCell<Option<super::Paint>>,
        }

        #[glib::object_subclass]
        impl ObjectSubclass for Canvas {
            const NAME: &'static str = "NekolandShotCanvas";
            type Type = super::Canvas;
            type ParentType = gtk::Widget;
        }

        impl ObjectImpl for Canvas {}

        impl WidgetImpl for Canvas {
            fn snapshot(&self, snapshot: &gtk::Snapshot) {
                let widget = self.obj();
                if let Some(paint) = self.paint.borrow().as_ref() {
                    paint(snapshot, widget.width() as f64, widget.height() as f64);
                }
            }
        }
    }

    glib::wrapper! {
        pub struct Canvas(ObjectSubclass<imp::Canvas>)
            @extends gtk::Widget,
            @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
    }

    impl Canvas {
        pub fn new(paint: impl Fn(&gtk::Snapshot, f64, f64) + 'static) -> Canvas {
            let canvas: Canvas = glib::Object::new();
            canvas.imp().paint.replace(Some(Box::new(paint)));
            canvas
        }
    }
}
