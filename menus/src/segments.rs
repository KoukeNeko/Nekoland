//! One of a few, in any program: a row of toggle buttons joined together, or libadwaita's
//! toggle group, gets one pill that slides from the button that was chosen to the one that
//! is, as it does on a phone, and whatever of a label or an icon is inside the pill is in
//! the colour that reads on it.
//!
//! A stylesheet cannot do it: each button draws only within itself, so nothing can travel
//! from one to the next. And the programs are to stay as they are, with the widgets GTK
//! gives them. So `GtkWidget`'s own way of drawing is changed before any class is made
//! from it: a row that chooses one of a few is drawn as it was, then the pill over it, and
//! then its buttons once more inside the pill, every colour of theirs turned into the one
//! for the pill. The pill can also be taken hold of and dragged along the row, and chooses
//! the button it is let go over. The row is marked with a style class, which is what tells the stylesheet
//! to leave the chosen button's own background out; a program this is not loaded into has
//! no such class and keeps the look the stylesheet gives it alone.

use std::ffi::{c_char, c_int, c_uint, c_void, CStr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;

use crate::{find, GType};

/// The class a row has while its pill is drawn here.
const SEGMENTED: &CStr = c"nekoland-segmented";
const LINKED: &CStr = c"linked";
const TOGGLE_GROUP: &CStr = c"AdwToggleGroup";
const SLIDE_KEY: &CStr = c"nekoland-segments";
/// What a button is marked with once its row is told of its changes.
const WATCHED_KEY: &CStr = c"nekoland-segment-watched";
const STATE_CHANGED: &CStr = c"state-flags-changed";
const DRAG_BEGIN: &CStr = c"drag-begin";
const DRAG_UPDATE: &CStr = c"drag-update";
const DRAG_END: &CStr = c"drag-end";
const ANIMATIONS: &CStr = c"gtk-enable-animations";

/// `GTK_ORIENTATION_HORIZONTAL`.
const HORIZONTAL: c_int = 0;
/// `GTK_STATE_FLAG_ACTIVE` and `GTK_STATE_FLAG_CHECKED`.
const HELD: c_uint = 1 << 0;
const CHECKED: c_uint = 1 << 11;
/// `GTK_PHASE_CAPTURE`: a row hears of a press before the button under it does.
const CAPTURE: c_int = 1;
/// `GTK_EVENT_SEQUENCE_CLAIMED`: the press is the row's, and no longer a click on a button.
const CLAIMED: c_int = 1;
/// `G_SOURCE_CONTINUE` and `G_SOURCE_REMOVE`.
const AGAIN: c_int = 1;
const DONE: c_int = 0;

/// How fast the pill's spring is: it settles without going past, in about a quarter of a
/// second.
const SPRING_RATE: f64 = 22.8;
/// Nearer its place than this share of the way, the pill is there.
const ARRIVED: f64 = 0.001;
/// How much of its size the pill keeps while the chosen one is held down, and how many
/// seconds it takes to give and to come back.
const HELD_SCALE: f64 = 0.95;
const GIVE_SECONDS: f64 = 0.12;
/// How far the pointer goes with the pill held before the pill comes along, so that a
/// click with an unsteady hand stays a click.
const DRAG_SLACK: f64 = 4.0;
/// The longest a frame is taken to have lasted, after a while with none.
const LONGEST_FRAME: f64 = 0.05;
/// The pill's corners, as nekoland.css rounds what is chosen in a group.
const PILL_RADIUS: f64 = 6.0;
const MICROSECONDS: f64 = 1_000_000.0;

/// How many pointers long GLib's `GObjectClass` is, which `GtkWidgetClass` starts with.
const OBJECT_CLASS_WORDS: usize = 17;

type Root = unsafe extern "C" fn(*mut c_void);
type Snapshot = unsafe extern "C" fn(*mut c_void, *mut c_void);
type Tick = unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> c_int;
type Destroy = unsafe extern "C" fn(*mut c_void);
type StateChanged = unsafe extern "C" fn(*mut c_void, c_uint, *mut c_void);
/// What a drag calls back with: its gesture, and where it began or how far it has gone.
type Dragged = unsafe extern "C" fn(*mut c_void, f64, f64, *mut c_void);

/// GTK's `GtkWidgetClass`, as far as `snapshot`.
#[repr(C)]
struct WidgetClass {
    object_class: [usize; OBJECT_CLASS_WORDS],
    /// `show`, `hide`, `map`, `unmap`, `realize` and `unrealize`.
    before_root: [usize; 6],
    root: Option<Root>,
    /// From `unroot` to `system_setting_changed`.
    before_snapshot: [usize; 16],
    snapshot: Option<Snapshot>,
}

/// Graphene's `graphene_rect_t`.
#[derive(Clone, Copy, Default)]
#[repr(C)]
struct Rect {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

/// GSK's `GskRoundedRect`: a rectangle and the size of each of its corners.
#[repr(C)]
struct RoundedRect {
    bounds: Rect,
    corners: [[f32; 2]; 4],
}

/// GDK's `GdkRGBA`.
#[derive(Default)]
#[repr(C)]
struct Rgba {
    red: f32,
    green: f32,
    blue: f32,
    alpha: f32,
}

/// Room for Graphene's `graphene_matrix_t` and `graphene_vec4_t`, which it fills itself.
#[repr(C, align(16))]
struct Matrix([f32; 16]);
#[repr(C, align(16))]
struct Vec4([f32; 4]);

struct Gtk {
    box_type: GType,
    toggle_type: GType,
    type_from_name: unsafe extern "C" fn(*const c_char) -> GType,
    is_a: unsafe extern "C" fn(*mut c_void, GType) -> c_int,
    first_child: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    next_sibling: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    parent: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    /// Has a signal call a function, whose type is the signal's own.
    connect: unsafe extern "C" fn(
        *mut c_void,
        *const c_char,
        *const c_void,
        *mut c_void,
        Option<Destroy>,
        c_uint,
    ) -> std::ffi::c_ulong,
    drag_gesture: unsafe extern "C" fn() -> *mut c_void,
    set_phase: unsafe extern "C" fn(*mut c_void, c_int),
    add_controller: unsafe extern "C" fn(*mut c_void, *mut c_void),
    controller_widget: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    set_gesture_state: unsafe extern "C" fn(*mut c_void, c_int) -> c_int,
    set_active: unsafe extern "C" fn(*mut c_void, c_int),
    is_visible: unsafe extern "C" fn(*mut c_void) -> c_int,
    has_class: unsafe extern "C" fn(*mut c_void, *const c_char) -> c_int,
    add_class: unsafe extern "C" fn(*mut c_void, *const c_char),
    remove_class: unsafe extern "C" fn(*mut c_void, *const c_char),
    state_flags: unsafe extern "C" fn(*mut c_void) -> c_uint,
    orientation: unsafe extern "C" fn(*mut c_void) -> c_int,
    bounds: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut Rect) -> c_int,
    color: unsafe extern "C" fn(*mut c_void, *mut Rgba),
    queue_draw: unsafe extern "C" fn(*mut c_void),
    add_tick: unsafe extern "C" fn(*mut c_void, Tick, *mut c_void, Option<Destroy>) -> c_uint,
    frame_clock: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    frame_time: unsafe extern "C" fn(*mut c_void) -> i64,
    data: unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_void,
    set_data: unsafe extern "C" fn(*mut c_void, *const c_char, *mut c_void, Option<Destroy>),
    settings: unsafe extern "C" fn() -> *mut c_void,
    get: unsafe extern "C" fn(*mut c_void, *const c_char, ...),
    snapshot_child: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void),
    push_clip: unsafe extern "C" fn(*mut c_void, *const RoundedRect),
    push_colors: unsafe extern "C" fn(*mut c_void, *const Matrix, *const Vec4),
    append_color: unsafe extern "C" fn(*mut c_void, *const Rgba, *const Rect),
    pop: unsafe extern "C" fn(*mut c_void),
    fill_matrix: unsafe extern "C" fn(*mut Matrix, *const f32) -> *mut Matrix,
    fill_vec4: unsafe extern "C" fn(*mut Vec4, f32, f32, f32, f32) -> *mut Vec4,
    /// The class's own `root` and `snapshot`, still called.
    root: Option<Root>,
    snapshot: Option<Snapshot>,
}

static GTK: OnceLock<Gtk> = OnceLock::new();
/// libadwaita's toggle group, once the program has made its type.
static TOGGLE_GROUP_TYPE: AtomicUsize = AtomicUsize::new(0);

/// Where a row's pill is and where it is going, kept with the row.
struct Slide {
    chosen: *mut c_void,
    /// Where the pill set off from and when, while it is on its way.
    from: Option<(Rect, i64)>,
    /// Where it was last drawn, which is where the next slide sets off from.
    shown: Option<Rect>,
    /// How far it has given under a press, from 0 to 1.
    given: f64,
    last_frame: i64,
    moving: bool,
    ticking: bool,
    /// Whether the row already hears of drags.
    draggable: bool,
    /// The pill as it was taken hold of, while the pointer is down on it.
    held: Option<Rect>,
    /// Where the pointer has taken it, once it has come along.
    dragged: Option<Rect>,
}

/// A function of the program's by its name, as the type it is called with.
unsafe fn function<F: Copy>(name: &CStr) -> Option<F> {
    debug_assert_eq!(std::mem::size_of::<F>(), std::mem::size_of::<*mut c_void>());
    find(name).map(|symbol| std::mem::transmute_copy(&symbol))
}

/// How much of the way a spring that does not go past has gone, so many seconds on.
fn spring(seconds: f64) -> f64 {
    let turns = SPRING_RATE * seconds.max(0.0);
    1.0 - (1.0 + turns) * (-turns).exp()
}

fn between(from: Rect, to: Rect, share: f64) -> Rect {
    let mix = |from: f32, to: f32| from + (to - from) * share as f32;
    Rect {
        x: mix(from.x, to.x),
        y: mix(from.y, to.y),
        width: mix(from.width, to.width),
        height: mix(from.height, to.height),
    }
}

/// A rectangle so many times its size about its middle.
fn scaled(rect: Rect, scale: f64) -> Rect {
    let (width, height) = (rect.width * scale as f32, rect.height * scale as f32);
    Rect {
        x: rect.x + (rect.width - width) / 2.0,
        y: rect.y + (rect.height - height) / 2.0,
        width,
        height,
    }
}

impl Gtk {
    /// The toggle buttons among a widget's children that are shown, and whether every
    /// child that is shown is one.
    unsafe fn toggles(&self, row: *mut c_void) -> (Vec<*mut c_void>, bool) {
        let mut toggles = Vec::new();
        let mut only_toggles = true;
        let mut child = (self.first_child)(row);
        while !child.is_null() {
            if (self.is_visible)(child) != 0 {
                if (self.is_a)(child, self.toggle_type) != 0 {
                    toggles.push(child);
                } else {
                    only_toggles = false;
                }
            }
            child = (self.next_sibling)(child);
        }
        (toggles, only_toggles)
    }

    /// libadwaita's toggle group's type, or nothing in a program that has made none.
    unsafe fn toggle_group_type(&self) -> Option<GType> {
        let known = TOGGLE_GROUP_TYPE.load(Ordering::Relaxed);
        if known != 0 {
            return Some(known);
        }
        let found = (self.type_from_name)(TOGGLE_GROUP.as_ptr());
        TOGGLE_GROUP_TYPE.store(found, Ordering::Relaxed);
        (found != 0).then_some(found)
    }

    /// A row's buttons and the chosen one among them, if the row chooses one of a few: a
    /// toggle group, or a box of nothing but toggle buttons joined together, laid out
    /// side by side with exactly one of them on.
    unsafe fn choice(&self, row: *mut c_void) -> Option<(Vec<*mut c_void>, *mut c_void)> {
        let joined = (self.is_a)(row, self.box_type) != 0 && (self.has_class)(row, LINKED.as_ptr()) != 0;
        let group = !joined && self.toggle_group_type().is_some_and(|group| (self.is_a)(row, group) != 0);
        if !(joined || group) || (self.orientation)(row) != HORIZONTAL {
            return None;
        }
        let (toggles, only_toggles) = self.toggles(row);
        if toggles.len() < 2 || (joined && !only_toggles) {
            return None;
        }
        let mut on = toggles.iter().copied().filter(|toggle| (self.state_flags)(*toggle) & CHECKED != 0);
        let chosen = on.next()?;
        on.next().is_none().then_some((toggles, chosen))
    }

    /// Marks a row as one whose pill is drawn here, and has each of its buttons tell it
    /// when it is chosen or held: with the pill drawn over them the buttons look the
    /// same either way, so nothing else would draw the row again.
    unsafe fn mark(&self, row: *mut c_void, toggles: &[*mut c_void]) {
        (self.add_class)(row, SEGMENTED.as_ptr());
        for toggle in toggles {
            if (self.data)(*toggle, WATCHED_KEY.as_ptr()).is_null() {
                (self.set_data)(*toggle, WATCHED_KEY.as_ptr(), SEGMENTED.as_ptr() as *mut c_void, None);
                self.on(*toggle, STATE_CHANGED, state_changed as StateChanged as *const c_void);
            }
        }
        let slide = &mut *self.slide_of(row);
        if !slide.draggable {
            slide.draggable = true;
            let drag = (self.drag_gesture)();
            (self.set_phase)(drag, CAPTURE);
            self.on(drag, DRAG_BEGIN, drag_begun as Dragged as *const c_void);
            self.on(drag, DRAG_UPDATE, drag_moved as Dragged as *const c_void);
            self.on(drag, DRAG_END, drag_ended as Dragged as *const c_void);
            // The row keeps the gesture from here on.
            (self.add_controller)(row, drag);
        }
    }

    unsafe fn on(&self, object: *mut c_void, signal: &CStr, handler: *const c_void) {
        (self.connect)(object, signal.as_ptr(), handler, std::ptr::null_mut(), None, 0);
    }

    /// Where a button is in its row.
    unsafe fn place_of(&self, toggle: *mut c_void, row: *mut c_void) -> Option<Rect> {
        let mut place = Rect::default();
        ((self.bounds)(toggle, row, &mut place) != 0).then_some(place)
    }

    /// The pill so far along its row from where it was taken hold of, kept within the
    /// row's buttons.
    unsafe fn dragged_to(&self, row: *mut c_void, held: Rect, offset: f64) -> Rect {
        let (toggles, _) = self.toggles(row);
        let places: Vec<Rect> = toggles.iter().filter_map(|toggle| self.place_of(*toggle, row)).collect();
        let left = places.iter().map(|place| place.x).fold(held.x, f32::min);
        let right = places.iter().map(|place| place.x + place.width).fold(held.x + held.width, f32::max);
        Rect { x: (held.x + offset as f32).clamp(left, right - held.width), ..held }
    }

    /// The button the middle of the pill is nearest to.
    unsafe fn nearest(&self, row: *mut c_void, pill: Rect) -> Option<*mut c_void> {
        let middle = |place: Rect| place.x + place.width / 2.0;
        let (toggles, _) = self.toggles(row);
        toggles
            .into_iter()
            .filter_map(|toggle| Some((toggle, self.place_of(toggle, row)?)))
            .min_by(|(_, one), (_, other)| {
                let away = |place: &Rect| (middle(*place) - middle(pill)).abs();
                away(one).total_cmp(&away(other))
            })
            .map(|(toggle, _)| toggle)
    }

    unsafe fn animated(&self) -> bool {
        let settings = (self.settings)();
        let mut animated: c_int = 1;
        if !settings.is_null() {
            (self.get)(settings, ANIMATIONS.as_ptr(), &mut animated as *mut c_int, std::ptr::null::<c_void>());
        }
        animated != 0
    }

    /// What is kept with a row, made the first time it is asked for.
    unsafe fn slide_of(&self, row: *mut c_void) -> *mut Slide {
        let kept = (self.data)(row, SLIDE_KEY.as_ptr()) as *mut Slide;
        if !kept.is_null() {
            return kept;
        }
        let slide = Box::into_raw(Box::new(Slide {
            chosen: std::ptr::null_mut(),
            from: None,
            shown: None,
            given: 0.0,
            last_frame: 0,
            moving: false,
            ticking: false,
            draggable: false,
            held: None,
            dragged: None,
        }));
        (self.set_data)(row, SLIDE_KEY.as_ptr(), slide as *mut c_void, Some(drop_slide));
        slide
    }

    /// Where the pill is this frame, on its way to the chosen button, and how far it has
    /// given under a press; starts the frames that take it the rest of the way.
    unsafe fn step(&self, row: *mut c_void, chosen: *mut c_void, goal: Rect) -> Rect {
        let slide = &mut *self.slide_of(row);
        let clock = (self.frame_clock)(row);
        let now = if clock.is_null() { slide.last_frame } else { (self.frame_time)(clock) };
        let animated = self.animated();

        if slide.chosen != chosen {
            slide.from = slide.shown.filter(|_| animated && !slide.chosen.is_null()).map(|shown| (shown, now));
            slide.chosen = chosen;
        }
        let place = match slide.from {
            _ if slide.dragged.is_some() => slide.dragged.unwrap_or(goal),
            Some((from, started)) => {
                let gone = spring((now - started) as f64 / MICROSECONDS);
                if 1.0 - gone < ARRIVED {
                    slide.from = None;
                    goal
                } else {
                    between(from, goal, gone)
                }
            }
            None => goal,
        };
        slide.shown = Some(place);

        let in_hand = slide.dragged.is_some() || (self.state_flags)(chosen) & HELD != 0;
        let held = if in_hand { 1.0 } else { 0.0 };
        let frame = ((now - slide.last_frame) as f64 / MICROSECONDS).clamp(0.0, LONGEST_FRAME);
        let step = if animated { frame / GIVE_SECONDS } else { 1.0 };
        slide.given += (held - slide.given).clamp(-step, step);
        slide.last_frame = now;

        slide.moving = slide.from.is_some() || slide.given != held;
        if slide.moving && !slide.ticking {
            slide.ticking = true;
            (self.add_tick)(row, tick, std::ptr::null_mut(), None);
        }
        scaled(place, 1.0 - (1.0 - HELD_SCALE) * slide.given)
    }

    /// The pill over a row already drawn, and the row's buttons again inside it in the
    /// colour that reads on it.
    unsafe fn draw_pill(&self, row: *mut c_void, toggles: &[*mut c_void], chosen: *mut c_void, snapshot: *mut c_void) {
        let mut goal = Rect::default();
        if (self.bounds)(chosen, row, &mut goal) == 0 {
            return;
        }
        let pill = self.step(row, chosen, goal);
        let radius = (PILL_RADIUS as f32).min(pill.width / 2.0).min(pill.height / 2.0);
        let shape = RoundedRect { bounds: pill, corners: [[radius, radius]; 4] };
        // The row's own colour is the pill's, and a button's the one that reads on it; what
        // is in a button has the colour that reads on the track.
        let (mut ink, mut on_ink) = (Rgba::default(), Rgba::default());
        (self.color)(row, &mut ink);
        (self.color)(chosen, &mut on_ink);
        // Every colour becomes the one for the pill, as see-through as it was.
        let mut only_alpha = Matrix([0.0; 16]);
        let mut kept = [0.0f32; 16];
        kept[15] = 1.0;
        (self.fill_matrix)(&mut only_alpha, kept.as_ptr());
        let mut colour = Vec4([0.0; 4]);
        (self.fill_vec4)(&mut colour, on_ink.red, on_ink.green, on_ink.blue, 0.0);

        (self.push_clip)(snapshot, &shape);
        (self.append_color)(snapshot, &ink, &pill);
        (self.push_colors)(snapshot, &only_alpha, &colour);
        for toggle in toggles {
            (self.snapshot_child)(row, *toggle, snapshot);
        }
        (self.pop)(snapshot);
        (self.pop)(snapshot);
    }
}

unsafe extern "C" fn drop_slide(slide: *mut c_void) {
    drop(Box::from_raw(slide as *mut Slide));
}

/// Draws the row again each frame for as long as its pill is moving.
unsafe extern "C" fn tick(row: *mut c_void, _clock: *mut c_void, _data: *mut c_void) -> c_int {
    let Some(gtk) = GTK.get() else {
        return DONE;
    };
    let slide = (gtk.data)(row, SLIDE_KEY.as_ptr()) as *mut Slide;
    if slide.is_null() {
        return DONE;
    }
    if !(*slide).moving {
        (*slide).ticking = false;
        return DONE;
    }
    (gtk.queue_draw)(row);
    AGAIN
}

/// The row a drag is on and what is kept with it.
unsafe fn dragged_row(gtk: &Gtk, gesture: *mut c_void) -> Option<(*mut c_void, &'static mut Slide)> {
    let row = (gtk.controller_widget)(gesture);
    let slide = if row.is_null() { std::ptr::null_mut() } else { (gtk.data)(row, SLIDE_KEY.as_ptr()) as *mut Slide };
    (!slide.is_null()).then(|| (row, &mut *slide))
}

/// A press on the pill takes hold of it; one anywhere else is left to the button under it.
unsafe extern "C" fn drag_begun(gesture: *mut c_void, x: f64, y: f64, _data: *mut c_void) {
    let Some((_, slide)) = GTK.get().and_then(|gtk| dragged_row(gtk, gesture)) else {
        return;
    };
    let on_pill = |pill: &Rect| {
        let (x, y) = (x as f32, y as f32);
        x >= pill.x && x < pill.x + pill.width && y >= pill.y && y < pill.y + pill.height
    };
    slide.held = slide.shown.filter(on_pill);
    slide.dragged = None;
}

/// The pill comes along with the pointer once it has gone far enough to be a drag.
unsafe extern "C" fn drag_moved(gesture: *mut c_void, offset: f64, _down: f64, _data: *mut c_void) {
    let Some(gtk) = GTK.get() else {
        return;
    };
    let Some((row, slide)) = dragged_row(gtk, gesture) else {
        return;
    };
    let Some(held) = slide.held else {
        return;
    };
    if slide.dragged.is_none() {
        if offset.abs() < DRAG_SLACK {
            return;
        }
        (gtk.set_gesture_state)(gesture, CLAIMED);
        slide.from = None;
    }
    slide.dragged = Some(gtk.dragged_to(row, held, offset));
    (gtk.queue_draw)(row);
}

/// Let go, the pill chooses the button it is nearest and springs the rest of the way.
unsafe extern "C" fn drag_ended(gesture: *mut c_void, _offset: f64, _down: f64, _data: *mut c_void) {
    let Some(gtk) = GTK.get() else {
        return;
    };
    let Some((row, slide)) = dragged_row(gtk, gesture) else {
        return;
    };
    slide.held = None;
    let Some(pill) = slide.dragged.take() else {
        return;
    };
    // From where it was let go, whether to another button or back to its own.
    slide.shown = Some(pill);
    slide.from = gtk.animated().then_some((pill, slide.last_frame));
    if let Some(toggle) = gtk.nearest(row, pill) {
        (gtk.set_active)(toggle, 1);
    }
    (gtk.queue_draw)(row);
}

/// Has a button's row drawn again when the button is chosen, let go of, or held.
unsafe extern "C" fn state_changed(toggle: *mut c_void, _before: c_uint, _data: *mut c_void) {
    let Some(gtk) = GTK.get() else {
        return;
    };
    let row = (gtk.parent)(toggle);
    if !row.is_null() && (gtk.has_class)(row, SEGMENTED.as_ptr()) != 0 {
        (gtk.queue_draw)(row);
    }
}

/// Marks a row that chooses one of a few as it is put in a window, before it is first
/// measured and drawn.
unsafe extern "C" fn root(widget: *mut c_void) {
    let Some(gtk) = GTK.get() else {
        return;
    };
    if let Some(root) = gtk.root {
        root(widget);
    }
    if let Some((toggles, _)) = gtk.choice(widget) {
        gtk.mark(widget, &toggles);
    }
}

unsafe extern "C" fn snapshot(widget: *mut c_void, snapshot: *mut c_void) {
    let Some(gtk) = GTK.get() else {
        return;
    };
    if let Some(draw) = gtk.snapshot {
        draw(widget, snapshot);
    }
    let marked = (gtk.has_class)(widget, SEGMENTED.as_ptr()) != 0;
    match gtk.choice(widget) {
        Some((toggles, chosen)) if marked => {
            // A button put in the row since it was marked is told of as well.
            gtk.mark(widget, &toggles);
            gtk.draw_pill(widget, &toggles, chosen, snapshot)
        }
        // The look that goes with the pill is there from the next frame.
        Some((toggles, _)) => gtk.mark(widget, &toggles),
        None if marked => (gtk.remove_class)(widget, SEGMENTED.as_ptr()),
        None => {}
    }
}

/// Changes how every widget is put in a window and drawn, in a program with GTK 4.
pub unsafe fn init() {
    let Some(gtk) = load() else {
        return;
    };
    let (Some(widget_type), Some(class_ref)) = (
        function::<unsafe extern "C" fn() -> GType>(c"gtk_widget_get_type"),
        function::<unsafe extern "C" fn(GType) -> *mut WidgetClass>(c"g_type_class_ref"),
    ) else {
        return;
    };
    // Only GtkWidget's class is made here: every other is made later, from it as it is
    // then. Kept for as long as the program runs, so the change stays.
    let class = class_ref(widget_type());
    let gtk = Gtk { root: (*class).root, snapshot: (*class).snapshot, ..gtk };
    if GTK.set(gtk).is_ok() {
        (*class).root = Some(root);
        (*class).snapshot = Some(snapshot);
    }
}

/// GTK's functions, or nothing in a program without them all.
unsafe fn load() -> Option<Gtk> {
    let box_type = function::<unsafe extern "C" fn() -> GType>(c"gtk_box_get_type")?;
    let toggle_type = function::<unsafe extern "C" fn() -> GType>(c"gtk_toggle_button_get_type")?;
    Some(Gtk {
        type_from_name: function(c"g_type_from_name")?,
        is_a: function(c"g_type_check_instance_is_a")?,
        first_child: function(c"gtk_widget_get_first_child")?,
        next_sibling: function(c"gtk_widget_get_next_sibling")?,
        parent: function(c"gtk_widget_get_parent")?,
        connect: function(c"g_signal_connect_data")?,
        drag_gesture: function(c"gtk_gesture_drag_new")?,
        set_phase: function(c"gtk_event_controller_set_propagation_phase")?,
        add_controller: function(c"gtk_widget_add_controller")?,
        controller_widget: function(c"gtk_event_controller_get_widget")?,
        set_gesture_state: function(c"gtk_gesture_set_state")?,
        set_active: function(c"gtk_toggle_button_set_active")?,
        is_visible: function(c"gtk_widget_get_visible")?,
        has_class: function(c"gtk_widget_has_css_class")?,
        add_class: function(c"gtk_widget_add_css_class")?,
        remove_class: function(c"gtk_widget_remove_css_class")?,
        state_flags: function(c"gtk_widget_get_state_flags")?,
        orientation: function(c"gtk_orientable_get_orientation")?,
        bounds: function(c"gtk_widget_compute_bounds")?,
        color: function(c"gtk_widget_get_color")?,
        queue_draw: function(c"gtk_widget_queue_draw")?,
        add_tick: function(c"gtk_widget_add_tick_callback")?,
        frame_clock: function(c"gtk_widget_get_frame_clock")?,
        frame_time: function(c"gdk_frame_clock_get_frame_time")?,
        data: function(c"g_object_get_data")?,
        set_data: function(c"g_object_set_data_full")?,
        settings: function(c"gtk_settings_get_default")?,
        get: function(c"g_object_get")?,
        snapshot_child: function(c"gtk_widget_snapshot_child")?,
        push_clip: function(c"gtk_snapshot_push_rounded_clip")?,
        push_colors: function(c"gtk_snapshot_push_color_matrix")?,
        append_color: function(c"gtk_snapshot_append_color")?,
        pop: function(c"gtk_snapshot_pop")?,
        fill_matrix: function(c"graphene_matrix_init_from_float")?,
        fill_vec4: function(c"graphene_vec4_init")?,
        box_type: box_type(),
        toggle_type: toggle_type(),
        root: None,
        snapshot: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_spring_starts_still_and_arrives_without_going_past() {
        assert_eq!(spring(0.0), 0.0);
        let mut last = 0.0;
        for step in 1..=100 {
            let gone = spring(f64::from(step) * 0.01);
            assert!(gone > last && gone < 1.0);
            last = gone;
        }
        assert!(1.0 - spring(0.5) < ARRIVED);
    }

    #[test]
    fn a_pill_on_its_way_is_between_where_it_was_and_where_it_goes() {
        let from = Rect { x: 0.0, y: 2.0, width: 40.0, height: 28.0 };
        let to = Rect { x: 100.0, y: 2.0, width: 60.0, height: 28.0 };
        let halfway = between(from, to, 0.5);
        assert_eq!((halfway.x, halfway.width, halfway.y, halfway.height), (50.0, 50.0, 2.0, 28.0));
    }

    #[test]
    fn a_held_pill_shrinks_about_its_middle() {
        let held = scaled(Rect { x: 10.0, y: 0.0, width: 100.0, height: 20.0 }, 0.5);
        assert_eq!((held.x, held.y, held.width, held.height), (35.0, 5.0, 50.0, 10.0));
    }
}
