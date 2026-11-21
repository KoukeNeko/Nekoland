//! Motion: how windows come, go and change place, on springs as iOS moves what it shows.
//!
//! All of it is the compositor's doing, so every program has it whatever drew its window:
//! GTK, Qt or X11. A window grows a little as it fades in, or opens out of the icon it was
//! started from among the applications; one that closes leaves a picture of itself that
//! shrinks and fades; one maximized, put in a half or put back glides there; the desktops
//! slide past each other; and a menu grows out of what it was opened from. `animations`
//! turns all of it off.
//!
//! Nothing here keeps a clock of its own: each animation remembers when it began, and a
//! frame asks where it has got to by then.

use std::{f64::consts::TAU, sync::Mutex, time::Instant};

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            damage::OutputDamageTracker,
            element::{
                surface::{render_elements_from_surface_tree, WaylandSurfaceRenderElement},
                Element, Id, Kind, RenderElement,
            },
            gles::{GlesError, GlesFrame, GlesRenderer, GlesTexture},
            utils::{CommitCounter, RendererSurfaceStateUserData},
            Bind, Color32F, Offscreen, Texture,
        },
    },
    desktop::Window,
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Buffer, IsAlive, Logical, Physical, Point, Rectangle, Scale, Size, Transform},
    wayland::{compositor::with_states, seat::WaylandFocus},
};

use crate::{
    text::Picture,
    windows::is_managed,
    Nekoland,
};

/// A spring from 0 to 1, named as SwiftUI names its own: `response` is how many seconds one
/// swing would take with nothing to slow it, and `damping` how much of the slowing that
/// would just keep it from swinging past: 1 settles without passing, less passes a little.
#[derive(Clone, Copy, Debug)]
pub struct Spring {
    response: f64,
    damping: f64,
}

/// How near its end a spring counts as there.
const SETTLED: f64 = 0.001;
/// What a spring that just does not swing past has decayed by, in radians of its own swing,
/// when it is `SETTLED`: where (1 + x)e⁻ˣ comes down to it.
const CRITICAL_SETTLE: f64 = 9.23;

impl Spring {
    pub const fn new(response: f64, damping: f64) -> Self {
        Self { response, damping }
    }

    fn frequency(&self) -> f64 {
        TAU / self.response
    }

    /// Where the spring is, `elapsed` seconds after it was let go at 0.
    pub fn position(&self, elapsed: f64) -> f64 {
        if elapsed <= 0.0 {
            return 0.0;
        }
        let frequency = self.frequency();
        if self.damping >= 1.0 {
            let turned = frequency * elapsed;
            return 1.0 - (-turned).exp() * (1.0 + turned);
        }
        let decay = (-self.damping * frequency * elapsed).exp();
        let damped = frequency * (1.0 - self.damping * self.damping).sqrt();
        let swing = (damped * elapsed).cos() + self.damping * frequency / damped * (damped * elapsed).sin();
        1.0 - decay * swing
    }

    /// How many seconds it takes to come to rest.
    pub fn duration(&self) -> f64 {
        if self.damping >= 1.0 {
            CRITICAL_SETTLE / self.frequency()
        } else {
            -SETTLED.ln() / (self.damping * self.frequency())
        }
    }

    fn at(&self, started: Instant) -> f64 {
        self.position(seconds_since(started))
    }

    fn is_over(&self, started: Instant) -> bool {
        seconds_since(started) >= self.duration()
    }
}

/// Moves `value` on by `seconds` toward `goal` on a spring that just does not swing past,
/// `rate` its radians a second: for what is steered frame by frame, as an icon after the
/// pointer, where a spring let go once cannot follow a goal that moves.
pub fn settle(value: &mut f64, speed: &mut f64, goal: f64, rate: f64, seconds: f64) {
    let away = *value - goal;
    let pull = *speed + rate * away;
    let decay = (-rate * seconds).exp();
    *value = goal + (away + pull * seconds) * decay;
    *speed = (*speed - pull * rate * seconds) * decay;
}

/// As `settle`, on a spring that swings past its goal and back, the less `damping`, under
/// 1, the further: what a bar let down does at the end of its way.
pub fn swing(value: &mut f64, speed: &mut f64, goal: f64, rate: f64, damping: f64, seconds: f64) {
    // In small steps, which keep it the same spring however late the frame is.
    let steps = (seconds / 0.004).ceil().max(1.0);
    let step = seconds / steps;
    for _ in 0..steps as usize {
        *speed -= (rate * rate * (*value - goal) + 2.0 * damping * rate * *speed) * step;
        *value += *speed * step;
    }
}

/// The search field of the applications let down from above the screen, swinging a little
/// past its place.
const APPS_DROP: Spring = Spring::new(0.34, 0.62);

/// How far down to its place what is let down over the applications has come, 0 to 1 and a
/// little past: `opened` a moment ago, or `leaving`, from 1 down to 0, straight back up.
pub fn apps_drop(opened: Instant, leaving: Option<f64>) -> f64 {
    leaving.unwrap_or_else(|| APPS_DROP.at(opened))
}

/// The pages parting for a folder, without swinging past, and how long they take to close
/// over it again.
const FOLDER: Spring = Spring::new(0.28, 1.0);
const FOLDER_CLOSE: f64 = 0.2;

/// How far the pages have parted for a folder, 0 to 1: `opened` a moment ago, or closing
/// again since it was `closed`, after which it is gone.
pub fn folder_arrival(opened: Instant, closed: Option<Instant>) -> Option<f64> {
    let Some(closed) = closed else {
        return Some(FOLDER.at(opened));
    };
    let left = 1.0 - seconds_since(closed) / FOLDER_CLOSE;
    (left > 0.0).then(|| left * left * (3.0 - 2.0 * left))
}

fn seconds_since(started: Instant) -> f64 {
    started.elapsed().as_secs_f64()
}

/// A window opening: lively, and a little past its size before it rests.
const OPEN: Spring = Spring::new(0.40, 0.80);
/// A window with no icon to come out of starts this much of its size.
const OPEN_SCALE: f64 = 0.86;
/// How long a window opening takes to be fully there to see.
const OPEN_FADE: f64 = 0.12;
/// A window going from one place and size to another.
const MORPH: Spring = Spring::new(0.36, 0.86);
/// A window closing shrinks to this much of its size as it fades, without swinging back.
const CLOSE: Spring = Spring::new(0.24, 1.0);
const CLOSE_SCALE: f64 = 0.9;
/// One desktop sliding out as the next slides in.
const SLIDE: Spring = Spring::new(0.42, 1.0);
/// Asked for the desktop past the last one, the one shown is pushed that way a little and
/// comes back, for there is none there: how far, and how long it takes.
const BUMP_DISTANCE: f64 = 60.0;
const BUMP_SECONDS: f64 = 0.32;
/// A menu growing out of what it was opened from.
const POPUP: Spring = Spring::new(0.22, 0.85);
const POPUP_SCALE: f64 = 0.92;
const POPUP_FADE: f64 = 0.08;

/// The applications coming in from a little larger than they are, as a phone's icons
/// come home, and going out the way they came.
const APPS: Spring = Spring::new(0.3, 0.82);
const APPS_SCALE: f64 = 1.12;

/// How many times their size the applications are drawn: `opened` a moment ago, or
/// `leaving`, from 1 down to 0, with the view they are part of.
pub fn apps_zoom(opened: Instant, leaving: Option<f64>) -> f64 {
    let arrived = leaving.unwrap_or_else(|| APPS.at(opened));
    1.0 + (APPS_SCALE - 1.0) * (1.0 - arrived)
}

/// How far out of place an application starts, in times its size, when each comes in on its
/// own, and how long after the middle ones the outermost set off.
const APP_SCALE: f64 = 1.3;
const APPS_SPREAD: f64 = 0.1;
/// How much of the way out the outermost have gone before the middle ones set off.
const APPS_LEAVING_SPREAD: f64 = 0.35;

/// How far an application has come home, 0 to 1 and a little past: one `out` of the way
/// from the middle to the edge, 0 to 1, of those `opened` a moment ago, or `leaving`, from
/// 1 down to 0. As a phone's do, the middle ones land first and the rest follow outwards,
/// and they leave from the edge inwards.
pub fn app_arrival(opened: Instant, leaving: Option<f64>, out: f64) -> f64 {
    let out = out.clamp(0.0, 1.0);
    match leaving {
        Some(leaving) => ((leaving - out * APPS_LEAVING_SPREAD) / (1.0 - APPS_LEAVING_SPREAD)).clamp(0.0, 1.0),
        None => APPS.position(seconds_since(opened) - out * APPS_SPREAD),
    }
}

/// At most how many times as far from the middle an application starts: one in the very
/// middle has no way out, and comes in larger alone.
const APP_FAR_MOST: f64 = 6.0;

/// Where an application `arrived` that far is on its way `home` from off the `screen`, by
/// its middle, and how many times its size: out along the line from the screen's middle
/// through its place, and larger. It is `across` wide and high at most.
pub fn app_flight(
    home: Point<f64, Logical>,
    across: f64,
    screen: Rectangle<f64, Logical>,
    arrived: f64,
) -> (Point<f64, Logical>, f64) {
    let away = 1.0 - arrived;
    let middle = screen.loc + Point::from((screen.size.w / 2.0, screen.size.h / 2.0));
    let from_middle = home - middle;
    // How many times as far it is clear of the nearer edge, itself and all.
    let clear_x = (screen.size.w / 2.0 + across * APP_SCALE / 2.0) / from_middle.x.abs();
    let clear_y = (screen.size.h / 2.0 + across * APP_SCALE / 2.0) / from_middle.y.abs();
    let far = clear_x.min(clear_y).clamp(1.0, APP_FAR_MOST);
    let out = 1.0 + (far - 1.0) * away;
    (
        middle + Point::from((from_middle.x * out, from_middle.y * out)),
        1.0 + (APP_SCALE - 1.0) * away,
    )
}

/// The rectangle `t` of the way from one to another; a spring's `t` may pass 1.
pub fn lerp(from: Rectangle<i32, Logical>, to: Rectangle<i32, Logical>, t: f64) -> Rectangle<i32, Logical> {
    let mix = |a: i32, b: i32| (f64::from(a) + f64::from(b - a) * t).round() as i32;
    Rectangle::new(
        Point::from((mix(from.loc.x, to.loc.x), mix(from.loc.y, to.loc.y))),
        Size::from((mix(from.size.w, to.size.w), mix(from.size.h, to.size.h))),
    )
}

/// `rect` made `factor` times its size, `pivot` staying where it is.
pub fn scaled_about(
    rect: Rectangle<i32, Logical>,
    pivot: Point<i32, Logical>,
    factor: f64,
) -> Rectangle<i32, Logical> {
    let from_pivot = |at: i32, pivot: i32| pivot + (f64::from(at - pivot) * factor).round() as i32;
    let grown = |length: i32| (f64::from(length) * factor).round() as i32;
    Rectangle::new(
        Point::from((from_pivot(rect.loc.x, pivot.x), from_pivot(rect.loc.y, pivot.y))),
        Size::from((grown(rect.size.w), grown(rect.size.h))),
    )
}

fn center(rect: Rectangle<i32, Logical>) -> Point<i32, Logical> {
    rect.loc + Point::from((rect.size.w / 2, rect.size.h / 2))
}

/// Where a window is drawn in a frame: `shown`, in place of `actual`, where it is and
/// where clicks reach it, and how much of it is there to see.
/// How much of a window shows while the desktop is peeked at: its outline, as Windows' does.
const PEEK_ALPHA: f32 = 0.12;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub actual: Rectangle<i32, Logical>,
    pub shown: Rectangle<i32, Logical>,
    pub alpha: f32,
}

impl Pose {
    /// Whether the window is drawn just where it is.
    pub fn is_still(&self) -> bool {
        self.shown == self.actual && self.alpha >= 1.0
    }
}

struct Opening {
    window: Window,
    /// The icon it was started from among the applications, when it comes out of one.
    from: Option<Rectangle<i32, Logical>>,
    started: Instant,
}

/// An application started from its icon, whose window has not shown yet.
struct Morph {
    window: Window,
    /// Where it was drawn when it was sent off, and the size it had then, by which to tell
    /// when its program has drawn it at the new one.
    from: Rectangle<i32, Logical>,
    before: Size<i32, Logical>,
    /// The size it was asked to take.
    to: Size<i32, Logical>,
    started: Instant,
}

/// The picture a closed window leaves behind while it fades.
struct Ghost {
    id: Id,
    texture: GlesTexture,
    rect: Rectangle<i32, Logical>,
    started: Instant,
}

struct Slide {
    /// The windows of the desktop left, with where they were on it.
    leaving: Vec<(Window, Point<i32, Logical>)>,
    /// Whether the desktop gone to is after the one left, and so comes in from the right.
    forward: bool,
    /// The menu bar's content as the desktop left had it, sliding out with its windows.
    bar: Option<Picture>,
    started: Instant,
}

#[derive(Default)]
pub struct Motion {
    opening: Vec<Opening>,
    morphing: Vec<Morph>,
    ghosts: Vec<Ghost>,
    slide: Option<Slide>,
    /// When the desktop shown was pushed past the last one, and whether to the right.
    bump: Option<(Instant, bool)>,
}

/// A window as it was last drawn, with the part of it the picture covers, from where its
/// surface starts.
pub struct Snapshot {
    texture: GlesTexture,
    bbox: Rectangle<i32, Logical>,
}

/// Takes a window's picture at an output's scale; set by the backend, which has the renderer.
pub type Snapshooter = Box<dyn FnMut(&Window, f64) -> Option<Snapshot>>;

/// Draws a window as it is now into a texture of its own, which outlives its surface.
pub fn snapshot(
    renderer: &mut GlesRenderer,
    window: &Window,
    scale: f64,
) -> Result<Option<Snapshot>, Box<dyn std::error::Error>> {
    let Some(surface) = window.wl_surface() else {
        return Ok(None);
    };
    let bbox = window.bbox();
    let size: Size<i32, Physical> = bbox.size.to_physical_precise_round(scale);
    if size.w <= 0 || size.h <= 0 {
        return Ok(None);
    }
    let origin: Point<i32, Physical> =
        Point::from((-bbox.loc.x, -bbox.loc.y)).to_physical_precise_round(scale);
    let mut elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
        render_elements_from_surface_tree(renderer, &surface, origin, scale, 1.0, Kind::Unspecified);
    if elements.is_empty() {
        elements.extend(lone_surface_element(renderer, &surface, origin, scale));
    }
    if elements.is_empty() {
        return Ok(None);
    }
    let mut texture: GlesTexture = Offscreen::<GlesTexture>::create_buffer(
        renderer,
        Fourcc::Abgr8888,
        Size::<i32, Buffer>::from((size.w, size.h)),
    )?;
    {
        let mut tracker = OutputDamageTracker::new(size, scale, Transform::Normal);
        let mut target = renderer.bind(&mut texture)?;
        tracker
            .render_output(renderer, &mut target, 0, &elements, Color32F::TRANSPARENT)
            .map_err(|err| format!("{err:?}"))?;
    }
    Ok(Some(Snapshot { texture, bbox }))
}

/// A surface's own picture, without the surfaces under it. A program that ends without
/// closing its windows has its surfaces destroyed before their windows: such a surface is
/// no longer part of its tree, but what it last showed is still kept with it.
fn lone_surface_element(
    renderer: &mut GlesRenderer,
    surface: &WlSurface,
    origin: Point<i32, Physical>,
    scale: f64,
) -> Option<WaylandSurfaceRenderElement<GlesRenderer>> {
    with_states(surface, |states| {
        let offset = states
            .data_map
            .get::<RendererSurfaceStateUserData>()?
            .lock()
            .unwrap()
            .view()?
            .offset;
        let location = origin.to_f64() + offset.to_f64().to_physical(scale);
        WaylandSurfaceRenderElement::from_surface(renderer, surface, states, location, 1.0, Kind::Unspecified)
            .inspect_err(|err| tracing::warn!("window snapshot: {err}"))
            .ok()
            .flatten()
    })
}

/// A closed window's picture where it is drawn in a frame.
pub struct GhostElement {
    id: Id,
    texture: GlesTexture,
    dst: Rectangle<i32, Physical>,
    alpha: f32,
}

impl Element for GhostElement {
    fn id(&self) -> &Id {
        &self.id
    }

    fn alpha(&self) -> f32 {
        self.alpha
    }

    fn current_commit(&self) -> CommitCounter {
        CommitCounter::default()
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        Rectangle::from_size(self.texture.size().to_f64())
    }

    fn geometry(&self, _scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.dst
    }

    fn kind(&self) -> Kind {
        Kind::Unspecified
    }
}

impl RenderElement<GlesRenderer> for GhostElement {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        frame.render_texture_from_to(
            &self.texture,
            src,
            dst,
            damage,
            opaque_regions,
            Transform::Normal,
            self.alpha,
            None,
            &[],
        )
    }
}

/// When a menu was first drawn, kept with its surface.
#[derive(Default)]
struct PopupShown(Mutex<Option<Instant>>);

fn with_popup_shown<T>(surface: &WlSurface, then: impl FnOnce(&mut Option<Instant>) -> T) -> T {
    with_states(surface, |states| {
        states.data_map.insert_if_missing_threadsafe(PopupShown::default);
        let shown = states.data_map.get::<PopupShown>().expect("inserted above");
        let mut shown = shown.0.lock().unwrap();
        then(&mut shown)
    })
}

/// A menu was made on `surface`: it grows from the first frame that shows it, also on a
/// surface a menu before it was on.
pub fn popup_created(surface: &WlSurface) {
    with_popup_shown(surface, |shown| *shown = None);
}

/// How much of its size a menu is drawn at in this frame and how much of it is there to see,
/// while it still grows; it starts with the frame that first asks.
pub fn popup_pop(surface: &WlSurface) -> Option<(f64, f32)> {
    let started = with_popup_shown(surface, |shown| *shown.get_or_insert_with(Instant::now));
    if POPUP.is_over(started) {
        return None;
    }
    let factor = POPUP_SCALE + (1.0 - POPUP_SCALE) * POPUP.at(started);
    let alpha = (seconds_since(started) / POPUP_FADE).min(1.0) as f32;
    Some((factor, alpha))
}

impl Slide {
    /// How far right of their places the windows come to and those left are drawn, on an
    /// output `width` wide.
    fn offsets(&self, width: i32) -> (i32, i32) {
        slide_offsets(SLIDE.at(self.started), width, self.forward)
    }
}

/// The same, `progress` of the way through the slide.
fn slide_offsets(progress: f64, width: i32, forward: bool) -> (i32, i32) {
    let progress = progress.clamp(0.0, 1.0);
    let direction = if forward { 1.0 } else { -1.0 };
    let width = f64::from(width);
    (
        (direction * width * (1.0 - progress)).round() as i32,
        (-direction * width * progress).round() as i32,
    )
}

/// How far right the desktop is drawn `seconds` into a push toward `forward`: out quickly,
/// back more slowly, and still at both ends.
fn bump_offset(seconds: f64, forward: bool) -> i32 {
    let progress = (seconds / BUMP_SECONDS).clamp(0.0, 1.0);
    let out = 0.3;
    let shape = if progress < out {
        let t = progress / out;
        t * (2.0 - t)
    } else {
        let t = (progress - out) / (1.0 - out);
        1.0 - t * t * (3.0 - 2.0 * t)
    };
    let direction = if forward { -1.0 } else { 1.0 };
    (direction * BUMP_DISTANCE * shape).round() as i32
}

impl Nekoland {
    /// Lets go of the animations that have come to rest, and of windows that went meanwhile;
    /// called before each frame.
    pub fn tick_motion(&mut self) {
        let motion = &mut self.motion;
        motion
            .opening
            .retain(|opening| opening.window.alive() && !OPEN.is_over(opening.started));
        motion
            .morphing
            .retain(|morph| morph.window.alive() && !MORPH.is_over(morph.started));
        motion.ghosts.retain(|ghost| !CLOSE.is_over(ghost.started));
        if motion
            .slide
            .as_ref()
            .is_some_and(|slide| SLIDE.is_over(slide.started))
        {
            motion.slide = None;
        }
        if motion.bump.is_some_and(|(started, _)| seconds_since(started) >= BUMP_SECONDS) {
            motion.bump = None;
        }
    }

    /// Whether a window is drawn somewhere other than where it is, or is about to be.
    pub fn motion_moves(&self, window: &Window) -> bool {
        let motion = &self.motion;
        motion.slide.is_some()
            || motion.bump.is_some()
            || motion.opening.iter().any(|opening| &opening.window == window)
            || motion.morphing.iter().any(|morph| &morph.window == window)
    }

    fn output_width(&self) -> i32 {
        self.space
            .outputs()
            .next()
            .and_then(|output| self.space.output_geometry(output))
            .map_or(0, |geometry| geometry.size.w)
    }

    /// Where a window on screen is drawn now.
    pub fn window_pose(&self, window: &Window) -> Option<Pose> {
        let location = self.space.element_location(window)?;
        let actual = Rectangle::new(location, window.geometry().size);
        let mut shown = actual;
        let mut alpha = 1.0;
        if let Some(morph) = self.motion.morphing.iter().find(|morph| &morph.window == window) {
            // Until its program draws it at the size asked for, it is stretched toward that.
            let target = if actual.size == morph.before {
                Rectangle::new(actual.loc, morph.to)
            } else {
                actual
            };
            shown = lerp(morph.from, target, MORPH.at(morph.started));
        }
        if let Some(opening) = self
            .motion
            .opening
            .iter()
            .find(|opening| &opening.window == window)
        {
            let progress = OPEN.at(opening.started);
            shown = match opening.from {
                Some(icon) => lerp(icon, shown, progress),
                None => scaled_about(shown, center(shown), OPEN_SCALE + (1.0 - OPEN_SCALE) * progress),
            };
            alpha = (seconds_since(opening.started) / OPEN_FADE).min(1.0) as f32;
        }
        if let Some(slide) = &self.motion.slide {
            shown.loc.x += slide.offsets(self.output_width()).0;
        }
        shown.loc.x += self.bump_offset();
        // Super+, shows the desktop through the windows while it is held.
        if self.peeking && crate::windows::is_managed(window) {
            alpha *= PEEK_ALPHA;
        }
        Some(Pose { actual, shown, alpha })
    }

    /// While the desktops slide past each other on a display `width` wide: how far right the
    /// menu bar's content is drawn, and the desktop left's content with how far right it is.
    pub fn menu_bar_slide(&self, width: i32) -> Option<(i32, Option<(&Picture, i32)>)> {
        if self.motion.bump.is_some() {
            return Some((self.bump_offset(), None));
        }
        let slide = self.motion.slide.as_ref()?;
        let (coming, going) = slide.offsets(width);
        Some((coming, slide.bar.as_ref().map(|bar| (bar, going))))
    }

    /// The windows of the desktop just left, bottom of the stack first, each where it is
    /// drawn on its way out.
    pub fn leaving_windows(&self) -> Vec<(Window, Pose)> {
        let Some(slide) = &self.motion.slide else {
            return Vec::new();
        };
        let offset = slide.offsets(self.output_width()).1;
        slide
            .leaving
            .iter()
            .filter(|(window, _)| window.alive())
            .map(|(window, location)| {
                let actual = Rectangle::new(*location, window.geometry().size);
                let shown = Rectangle::new(actual.loc + Point::from((offset, 0)), actual.size);
                (
                    window.clone(),
                    Pose {
                        actual,
                        shown,
                        alpha: 1.0,
                    },
                )
            })
            .collect()
    }

    /// The pictures of the windows closed a moment ago, each shrinking as it fades.
    pub fn ghost_elements(&self, output_loc: Point<i32, Logical>, scale: f64) -> Vec<GhostElement> {
        self.motion
            .ghosts
            .iter()
            .map(|ghost| {
                let progress = CLOSE.at(ghost.started).clamp(0.0, 1.0);
                let rect = scaled_about(
                    ghost.rect,
                    center(ghost.rect),
                    1.0 - (1.0 - CLOSE_SCALE) * progress,
                );
                GhostElement {
                    id: ghost.id.clone(),
                    texture: ghost.texture.clone(),
                    dst: Rectangle::new(rect.loc - output_loc, rect.size).to_physical_precise_round(scale),
                    alpha: (1.0 - progress) as f32,
                }
            })
            .collect()
    }

    /// A window was shown for the first time: it grows a little where it is. One started
    /// from the applications does too, as they are gone by then, back to the desktop.
    pub fn motion_open(&mut self, window: &Window) {
        if self.config.animations && !is_shot_overlay(window) {
            self.start_opening(window, None);
        }
    }

    /// A window was brought back on screen other than out of the Dock.
    pub fn motion_reappear(&mut self, window: &Window) {
        if self.config.animations {
            self.start_opening(window, None);
        }
    }

    fn start_opening(&mut self, window: &Window, from: Option<Rectangle<i32, Logical>>) {
        self.motion.opening.retain(|opening| &opening.window != window);
        self.motion.opening.push(Opening {
            window: window.clone(),
            from,
            started: Instant::now(),
        });
    }

    /// A window on screen is about to be put at `to`: it glides there from where it is drawn.
    pub fn motion_morph(&mut self, window: &Window, to: Rectangle<i32, Logical>) {
        if !self.config.animations {
            return;
        }
        let Some(pose) = self.window_pose(window) else {
            return;
        };
        // A window not drawn yet has nowhere to come from.
        if pose.actual.size.w <= 0 || pose.actual.size.h <= 0 || pose.shown == to {
            return;
        }
        self.motion.morphing.retain(|morph| &morph.window != window);
        self.motion.morphing.push(Morph {
            window: window.clone(),
            from: pose.shown,
            before: pose.actual.size,
            to: to.size,
            started: Instant::now(),
        });
    }

    /// A window on screen is going away: its picture is kept to fade where it was.
    pub fn motion_close(&mut self, window: &Window) {
        // In the overview windows are drawn small, elsewhere; under the lock, not at all. An
        // X11 program's menus and tooltips come and go too often to leave pictures behind.
        if !self.config.animations
            || self.overview.is_some()
            || self.lock.is_some()
            || !is_managed(window)
            || is_shot_overlay(window)
        {
            return;
        }
        let Some(pose) = self.window_pose(window) else {
            return;
        };
        let Some(scale) = self
            .space
            .outputs()
            .next()
            .map(|output| output.current_scale().fractional_scale())
        else {
            return;
        };
        let Some(Snapshot { texture, bbox }) = self.snapshooter.as_mut().and_then(|take| take(window, scale))
        else {
            return;
        };
        self.motion.opening.retain(|opening| &opening.window != window);
        self.motion.morphing.retain(|morph| &morph.window != window);
        self.motion.ghosts.push(Ghost {
            id: Id::new(),
            texture,
            rect: Rectangle::new(pose.actual.loc - window.geometry().loc + bbox.loc, bbox.size),
            started: Instant::now(),
        });
    }

    /// The desktop past the last one, `forward` or back, was asked for: the one shown is
    /// pushed toward it and comes back.
    pub fn motion_bump(&mut self, forward: bool) {
        if !self.config.animations || self.overview.is_some() || self.motion.slide.is_some() {
            return;
        }
        self.motion.bump = Some((Instant::now(), forward));
    }

    /// How far right the desktop shown is pushed now.
    fn bump_offset(&self) -> i32 {
        self.motion
            .bump
            .map_or(0, |(started, forward)| bump_offset(seconds_since(started), forward))
    }

    /// The desktop shown changed: the windows `leaving` slide out as the others slide in,
    /// from the right when the desktop gone to is `forward` of the one left.
    pub fn motion_slide(&mut self, leaving: Vec<(Window, Point<i32, Logical>)>, forward: bool) {
        if !self.config.animations || self.overview.is_some() {
            return;
        }
        self.motion.slide = Some(Slide {
            leaving,
            forward,
            bar: self.menu_bar_picture().cloned(),
            started: Instant::now(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Logical> {
        Rectangle::new((x, y).into(), (w, h).into())
    }

    const SPRINGS: [Spring; 5] = [OPEN, MORPH, CLOSE, SLIDE, POPUP];

    #[test]
    fn a_spring_starts_at_rest_and_ends_where_it_was_sent() {
        for spring in SPRINGS {
            assert_eq!(spring.position(0.0), 0.0);
            assert!(
                (spring.position(spring.duration()) - 1.0).abs() <= SETTLED * 2.0,
                "{spring:?}"
            );
            assert!(spring.duration() < 0.7, "{spring:?} takes too long for a window");
        }
    }

    #[test]
    fn only_a_spring_damped_less_than_fully_passes_its_end() {
        let furthest = |spring: Spring| {
            (0..1000)
                .map(|step| spring.position(f64::from(step) / 1000.0))
                .fold(0.0, f64::max)
        };
        assert!(furthest(OPEN) > 1.0);
        assert!(
            furthest(OPEN) < 1.05,
            "a window should not swell past its size to notice"
        );
        assert!(furthest(SLIDE) <= 1.0);
        assert!(furthest(CLOSE) <= 1.0);
    }

    #[test]
    fn a_rectangle_scales_around_what_stays_put() {
        let scaled = scaled_about(rect(100, 100, 200, 100), Point::from((200, 150)), 0.5);
        assert_eq!(scaled, rect(150, 125, 100, 50));
        // A menu grows down from the top edge it hangs from.
        let menu = scaled_about(rect(100, 100, 200, 100), Point::from((200, 100)), 0.5);
        assert_eq!(menu, rect(150, 100, 100, 50));
    }

    #[test]
    fn a_rectangle_part_of_the_way_is_between_the_two() {
        let (from, to) = (rect(0, 0, 100, 100), rect(100, 200, 300, 500));
        assert_eq!(lerp(from, to, 0.0), from);
        assert_eq!(lerp(from, to, 1.0), to);
        assert_eq!(lerp(from, to, 0.5), rect(50, 100, 200, 300));
    }

    #[test]
    fn a_push_past_the_last_desktop_goes_out_that_way_and_comes_back() {
        assert_eq!(bump_offset(0.0, true), 0);
        assert_eq!(bump_offset(BUMP_SECONDS * 0.3, true), -BUMP_DISTANCE as i32);
        assert_eq!(bump_offset(BUMP_SECONDS * 0.3, false), BUMP_DISTANCE as i32);
        assert_eq!(bump_offset(BUMP_SECONDS, true), 0);
    }

    #[test]
    fn desktops_slide_past_each_other_a_screen_apart() {
        // Going on to the next, it comes in from the right as the one left goes out left.
        assert_eq!(slide_offsets(0.0, 1920, true), (1920, 0));
        assert_eq!(slide_offsets(0.25, 1920, true), (1440, -480));
        assert_eq!(slide_offsets(1.0, 1920, true), (0, -1920));
        // And the other way going back.
        assert_eq!(slide_offsets(0.25, 1920, false), (-1440, 480));
    }

    #[test]
    fn the_applications_come_in_larger_and_leave_the_way_they_came() {
        let long_ago = Instant::now() - std::time::Duration::from_secs(5);
        // The middle ones are home before the outermost, and are the last to leave.
        let a_while = Instant::now() - std::time::Duration::from_millis(150);
        assert!(app_arrival(a_while, None, 0.0) > app_arrival(a_while, None, 1.0));
        assert!(app_arrival(a_while, Some(0.5), 0.0) > app_arrival(a_while, Some(0.5), 1.0));
        assert_eq!(app_arrival(a_while, Some(1.0), 1.0), 1.0);
        assert_eq!(app_arrival(a_while, Some(0.0), 0.0), 0.0);
        // A folder closed is on its way in for a moment, then gone.
        assert!(folder_arrival(long_ago, Some(Instant::now())).is_some_and(|out| out > 0.9));
        assert_eq!(folder_arrival(long_ago, Some(long_ago)), None);
        // A swing goes past its goal, and comes to rest on it.
        let (mut value, mut speed, mut most) = (0.0f64, 0.0f64, 0.0f64);
        for _ in 0..120 {
            swing(&mut value, &mut speed, 1.0, 30.0, 0.55, 1.0 / 60.0);
            most = most.max(value);
        }
        assert!(most > 1.05 && most < 1.2);
        assert!((value - 1.0).abs() < 0.002);
        // Home when there, and off the screen before it sets off.
        let screen = Rectangle::new(Point::from((0.0, 0.0)), Size::from((1280.0, 800.0)));
        let home = Point::from((300.0, 500.0));
        assert_eq!(app_flight(home, 132.0, screen, 1.0), (home, 1.0));
        let (start, grown) = app_flight(home, 132.0, screen, 0.0);
        assert!(start.y - 66.0 * grown >= 800.0 || start.x + 66.0 * grown <= 0.0);
        assert_eq!(apps_zoom(Instant::now(), Some(1.0)), 1.0);
        assert!((apps_zoom(Instant::now(), Some(0.0)) - APPS_SCALE).abs() < 1e-9);
        assert!((apps_zoom(Instant::now(), None) - APPS_SCALE).abs() < 0.01);
        assert!((apps_zoom(long_ago, None) - 1.0).abs() < SETTLED);
    }

    #[test]
    fn a_window_drawn_where_it_is_is_still() {
        let actual = rect(10, 20, 300, 200);
        assert!(Pose {
            actual,
            shown: actual,
            alpha: 1.0
        }
        .is_still());
        assert!(!Pose {
            actual,
            shown: actual,
            alpha: 0.5
        }
        .is_still());
        assert!(!Pose {
            actual,
            shown: rect(11, 20, 300, 200),
            alpha: 1.0
        }
        .is_still());
    }
}

/// Whether a window is 截圖's choosing of what to take, which shows the screen as it was.
fn is_shot_overlay(window: &Window) -> bool {
    crate::windows::window_names(window).0 == crate::windows::SHOT_OVERLAY_TITLE
}
