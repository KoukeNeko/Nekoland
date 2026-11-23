//! Catseye（貓眼）, Nekoland's glass: what floats over the desktop is drawn over a blur of
//! what is behind it, a little more saturated, with a thin line of light just inside its top
//! edge, like the band of light across a cat's-eye stone. Glass that meets the top of the
//! screen, as the menu bar does, has no edge there to light.
//!
//! Once a frame, everything under the floating layers is drawn into a texture, which is
//! blurred by halving it a few times and doubling it back (dual Kawase): once for bars and
//! once for menus, as each tier of glass has a blur of its own strength. Each glass surface
//! then gets an element right behind it that shows its part of the blur, cut to its corners.
//!
//! Windows have glass behind them too, as Windows' Acrylic is: what a window leaves
//! see-through, as GTK's sidebars in Nekoland's look, shows what is under the window
//! blurred, the windows behind it and the wallpaper. Each window has a blur of its own, as
//! large as the window, drawn from the bottom window up, so each one's shows the glass of
//! those under it.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            damage::OutputDamageTracker,
            element::{
                utils::{Relocate, RelocateRenderElement},
                Element, Id, Kind, RenderElement,
            },
            gles::{
                ffi, GlesError, GlesFrame, GlesRenderer, GlesTexProgram, GlesTexture, Uniform, UniformName,
                UniformType,
            },
            utils::{CommitCounter, DamageSet},
            Bind, Color32F, Frame, Offscreen, Renderer, Texture,
        },
    },
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Buffer, IsAlive, Physical, Point, Rectangle, Scale, Size, Transform},
};

/// How many times the scene is halved. Each one roughly doubles how far the blur reaches.
const LEVELS: usize = 5;
/// The grid, in pixels, a window's blur keeps its textures on.
const CANVAS_GRID: i32 = 256;
/// How far apart the samples of a pass are, in texels of the smaller texture, with the
/// blur as strong as the configuration can make it.
const MAX_SPREAD: f32 = 4.0;

/// The kinds of glass, each with a look of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    /// The menu bar and the Dock themselves, and the notifications' banners under the bar.
    Bar,
    /// All other glass: menus, the Notification Center, the Dock's popups, the overview and behind
    /// windows, which shows where they are see-through, as GTK's sidebars.
    Menu,
}

/// The look the configuration gives each tier's glass, in thousandths: how solid the tint
/// over the blur is, and how strong the blur.
static OPACITY: [AtomicU32; 2] = [AtomicU32::new(500), AtomicU32::new(800)];
static BLUR: [AtomicU32; 2] = [const { AtomicU32::new(500) }; 2];

/// Takes how solid a tier's tint is and how strong its blur, each from 0 to 1, and says
/// whether either changed.
pub fn set_look(tier: Tier, opacity: f64, blur: f64) -> bool {
    let thousandths = |value: f64| (value.clamp(0.0, 1.0) * 1000.0).round() as u32;
    let opacity = OPACITY[tier as usize].swap(thousandths(opacity), Ordering::Relaxed) != thousandths(opacity);
    let blur = BLUR[tier as usize].swap(thousandths(blur), Ordering::Relaxed) != thousandths(blur);
    opacity || blur
}

/// The tint drawn over the blur, the design language's sheet, as solid as the configuration
/// says for the tier.
pub fn tint(dark: bool, tier: Tier) -> (f64, f64, f64, f64) {
    let alpha = f64::from(OPACITY[tier as usize].load(Ordering::Relaxed)) / 1000.0;
    if dark {
        (0.133, 0.133, 0.149, alpha)
    } else {
        (0.984, 0.980, 0.973, alpha)
    }
}

/// How far apart the samples of a tier's passes are now, in texels of the smaller texture.
fn spread(tier: Tier) -> f32 {
    BLUR[tier as usize].load(Ordering::Relaxed) as f32 / 1000.0 * MAX_SPREAD
}

/// How far from a pane a change behind it still shows in a blur of `spread`, in pixels.
fn reach(spread: f32) -> i32 {
    (spread.ceil() as i32).max(1) << (LEVELS + 1)
}
/// Offscreen targets are drawn top row first, the way textures are read back.
const OFFSCREEN: Transform = Transform::Flipped180;

const SAMPLER: &str = r#"#version 100
//_DEFINES_
#if defined(EXTERNAL)
#extension GL_OES_EGL_image_external : require
#endif
#ifdef GL_FRAGMENT_PRECISION_HIGH
precision highp float;
#else
precision mediump float;
#endif
#if defined(EXTERNAL)
uniform samplerExternalOES tex;
#else
uniform sampler2D tex;
#endif
uniform float alpha;
varying vec2 v_coords;
"#;

const DOWN: &str = r#"
uniform vec2 half_pixel;
uniform float spread;
void main() {
    vec2 d = half_pixel * spread;
    vec4 sum = texture2D(tex, v_coords) * 4.0;
    sum += texture2D(tex, v_coords - d);
    sum += texture2D(tex, v_coords + d);
    sum += texture2D(tex, v_coords + vec2(d.x, -d.y));
    sum += texture2D(tex, v_coords - vec2(d.x, -d.y));
    gl_FragColor = sum / 8.0;
}
"#;

const UP: &str = r#"
uniform vec2 half_pixel;
uniform float spread;
void main() {
    vec2 d = half_pixel * spread;
    vec4 sum = texture2D(tex, v_coords + vec2(-d.x * 2.0, 0.0));
    sum += texture2D(tex, v_coords + vec2(-d.x, d.y)) * 2.0;
    sum += texture2D(tex, v_coords + vec2(0.0, d.y * 2.0));
    sum += texture2D(tex, v_coords + vec2(d.x, d.y)) * 2.0;
    sum += texture2D(tex, v_coords + vec2(d.x * 2.0, 0.0));
    sum += texture2D(tex, v_coords + vec2(d.x, -d.y)) * 2.0;
    sum += texture2D(tex, v_coords + vec2(0.0, -d.y * 2.0));
    sum += texture2D(tex, v_coords + vec2(-d.x, -d.y)) * 2.0;
    gl_FragColor = sum / 12.0;
}
"#;

// The glass itself: the blur, cut to a rounded rectangle, with the line of light. The
// corners' shape comes in as CORNER_EXTENT and CORNER_EXPONENT.
const PANE: &str = r#"
uniform vec2 uv_origin;
uniform vec2 uv_size;
uniform vec2 size;
uniform float radius;
uniform float edge;
uniform float circular;
void main() {
    vec2 local = (v_coords - uv_origin) / uv_size * size;
    // Corners smooth as the Mac's, as text::rounded_rectangle draws them; one too short for
    // them has round ends instead, as has a circular one.
    bool capsule = circular > 0.5 || radius * CORNER_EXTENT > min(size.x, size.y) * 0.5;
    float corner = capsule ? radius : radius * CORNER_EXTENT;
    float power = capsule ? 2.0 : CORNER_EXPONENT;
    vec2 q = abs(local - size * 0.5) - (size * 0.5 - vec2(corner));
    vec2 c = max(q, 0.0);
    // Square glass, as what covers the screen, has no corners to work out for every pixel.
    float inside = 1.0;
    if (radius > 0.0) {
        float distance = pow(pow(c.x, power) + pow(c.y, power), 1.0 / power) + min(max(q.x, q.y), 0.0) - corner;
        inside = clamp(0.5 - distance, 0.0, 1.0);
    }

    vec3 color = texture2D(tex, v_coords).rgb;
    float grey = dot(color, vec3(0.2126, 0.7152, 0.0722));
    color = mix(vec3(grey), color, 1.4);
    float light = exp(-local.y / 1.5) * 0.22 * edge + (1.0 - local.y / size.y) * 0.04;
    color = min(color + light, 1.0);

    gl_FragColor = vec4(color, 1.0) * inside * alpha;
}
"#;

pub struct Glass {
    down: GlesTexProgram,
    up: GlesTexProgram,
    pane: GlesTexProgram,
    /// What is under the floating surfaces, blurred, for bars and for menus.
    bars: Blur,
    menus: Blur,
    /// What is under each window, blurred, by the window's id.
    windows: HashMap<Id, Blur>,
    ids: HashMap<WlSurface, Id>,
}

/// Something drawn into a texture and blurred, for panes to show their parts of.
#[derive(Default)]
struct Blur {
    /// What is behind the glass, then copies of it at half, a quarter, and so on.
    levels: Vec<GlesTexture>,
    tracker: Option<OutputDamageTracker>,
    /// The output scale the tracker draws at.
    scale: f64,
    /// Where on the output the textures start: the top left for the scene, a window's
    /// surroundings for its own.
    origin: Point<i32, Physical>,
    /// Where the blur is known to be right, the panes it was last made for, and how far
    /// apart its samples were.
    areas: Vec<Rectangle<i32, Physical>>,
    spread: f32,
    commit: CommitCounter,
}

impl Glass {
    pub fn new(renderer: &mut GlesRenderer) -> Result<Self, GlesError> {
        let pass_uniforms = [
            UniformName::new("half_pixel", UniformType::_2f),
            UniformName::new("spread", UniformType::_1f),
        ];
        let pane_uniforms = [
            UniformName::new("uv_origin", UniformType::_2f),
            UniformName::new("uv_size", UniformType::_2f),
            UniformName::new("size", UniformType::_2f),
            UniformName::new("radius", UniformType::_1f),
            UniformName::new("edge", UniformType::_1f),
            UniformName::new("circular", UniformType::_1f),
        ];
        Ok(Self {
            down: renderer.compile_custom_texture_shader(format!("{SAMPLER}{DOWN}"), &pass_uniforms)?,
            up: renderer.compile_custom_texture_shader(format!("{SAMPLER}{UP}"), &pass_uniforms)?,
            pane: renderer.compile_custom_texture_shader(
                format!(
                    "{SAMPLER}#define CORNER_EXTENT {:?}\n#define CORNER_EXPONENT {:?}\n{PANE}",
                    crate::text::CORNER_EXTENT,
                    crate::text::CORNER_EXPONENT
                ),
                &pane_uniforms,
            )?,
            bars: Blur::default(),
            menus: Blur::default(),
            windows: HashMap::new(),
            ids: HashMap::new(),
        })
    }

    /// Draws what is behind the floating glass and blurs it again, when what changed since
    /// the last frame is near enough to one of the `areas` the panes cover to show in it;
    /// bars and menus each have a blur of their own strength.
    pub fn update<E: RenderElement<GlesRenderer>>(
        &mut self,
        renderer: &mut GlesRenderer,
        (size, scale): (Size<i32, Physical>, f64),
        areas: &[(Tier, Rectangle<i32, Physical>)],
        behind: &[E],
    ) -> Result<(), Box<dyn std::error::Error>> {
        for tier in [Tier::Bar, Tier::Menu] {
            let of_tier: Vec<_> = areas
                .iter()
                .filter(|(of, _)| *of == tier)
                .map(|(_, area)| *area)
                .collect();
            if of_tier.is_empty() {
                continue;
            }
            let blur = if tier == Tier::Bar { &mut self.bars } else { &mut self.menus };
            blur.update(
                renderer,
                (&self.down, &self.up, spread(tier)),
                (Rectangle::from_size(size), scale),
                &of_tier,
                behind,
            )?;
        }
        Ok(())
    }

    /// The same for what is `behind` the window named `id`, over `area` of the `output`;
    /// `behind` goes back to front, as the scene is put together from the bottom up.
    pub fn update_window<E: RenderElement<GlesRenderer>>(
        &mut self,
        renderer: &mut GlesRenderer,
        id: Id,
        (output, scale): (Size<i32, Physical>, f64),
        area: Rectangle<i32, Physical>,
        behind: &[E],
    ) -> Result<(), Box<dyn std::error::Error>> {
        let spread = spread(Tier::Menu);
        let reach = reach(spread);
        let reach = Rectangle::new(
            area.loc - Point::from((reach, reach)),
            area.size + Size::from((reach * 2, reach * 2)),
        );
        // On a grid, so that a window moving or growing a little, as it does through every
        // frame of an animation or a drag, keeps its textures and what they hold.
        let snapped = Rectangle::<i32, Physical>::from_extremities(
            (reach.loc.x.div_euclid(CANVAS_GRID) * CANVAS_GRID, reach.loc.y.div_euclid(CANVAS_GRID) * CANVAS_GRID),
            (
                (reach.loc.x + reach.size.w + CANVAS_GRID - 1).div_euclid(CANVAS_GRID) * CANVAS_GRID,
                (reach.loc.y + reach.size.h + CANVAS_GRID - 1).div_euclid(CANVAS_GRID) * CANVAS_GRID,
            ),
        );
        let Some(canvas) = snapped.intersection(Rectangle::from_size(output)) else {
            return Ok(());
        };
        self.windows.entry(id).or_default().update(
            renderer,
            (&self.down, &self.up, spread),
            (canvas, scale),
            &[area],
            behind.iter().rev(),
        )
    }

    /// Lets go of the blurs of windows no longer drawn, all but those of `kept`.
    pub fn keep_windows(&mut self, kept: &[Id]) {
        self.windows.retain(|id, _| kept.contains(id));
    }

    /// The glass of `tier` behind a surface, over `rect` of the output, with corners of
    /// `radius`.
    pub fn pane(
        &mut self,
        surface: &WlSurface,
        tier: Tier,
        rect: Rectangle<i32, Physical>,
        radius: i32,
    ) -> Option<Pane> {
        self.ids.retain(|surface, _| surface.alive());
        let id = self.ids.entry(surface.clone()).or_insert_with(Id::new).clone();
        self.pane_with(id, tier, rect, radius)
    }

    /// The glass behind something the compositor draws itself, which names it with `id`.
    pub fn pane_with(&self, id: Id, tier: Tier, rect: Rectangle<i32, Physical>, radius: i32) -> Option<Pane> {
        let blur = if tier == Tier::Bar { &self.bars } else { &self.menus };
        blur.pane(&self.pane, id, rect, radius)
    }

    /// The glass behind the window named `id`, of what is under it.
    pub fn window_pane(&self, id: Id, rect: Rectangle<i32, Physical>, radius: i32) -> Option<Pane> {
        self.windows.get(&id)?.pane(&self.pane, id, rect, radius)
    }
}

impl Blur {
    /// `behind` goes front to back.
    fn update<'a, E: RenderElement<GlesRenderer> + 'a>(
        &mut self,
        renderer: &mut GlesRenderer,
        (down, up, spread): (&GlesTexProgram, &GlesTexProgram, f32),
        (canvas, scale): (Rectangle<i32, Physical>, f64),
        areas: &[Rectangle<i32, Physical>],
        behind: impl IntoIterator<Item = &'a E>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let size = canvas.size;
        // What is behind, and where the panes are, as the textures have them.
        let shift = Point::from((-canvas.loc.x, -canvas.loc.y));
        let behind: Vec<_> = behind
            .into_iter()
            .map(|element| RelocateRenderElement::from_element(element, shift, Relocate::Relative))
            .collect();
        let areas: Vec<_> = areas
            .iter()
            .map(|area| Rectangle::new(area.loc + shift, area.size))
            .collect();
        if self.origin != canvas.loc {
            self.origin = canvas.loc;
            self.areas.clear();
        }
        // A blur of another strength is drawn again, all of it.
        if self.spread != spread {
            self.spread = spread;
            self.areas.clear();
        }
        let reach = reach(spread);
        let full = Size::<i32, Buffer>::from((size.w, size.h));
        if self.levels.first().map(Texture::size) != Some(full) || self.scale != scale {
            self.levels = (0..=LEVELS)
                .map(|level| {
                    let scaled = Size::from(((full.w >> level).max(1), (full.h >> level).max(1)));
                    Offscreen::<GlesTexture>::create_buffer(renderer, Fourcc::Abgr8888, scaled)
                })
                .collect::<Result<_, _>>()?;
            // A sample past an edge must read the edge, not wrap around to the other side of
            // the screen, which GL does by default: the menu bar at the top would blur in
            // what is at the bottom.
            for level in &self.levels {
                let texture = level.tex_id();
                renderer.with_context(|gl| unsafe {
                    gl.BindTexture(ffi::TEXTURE_2D, texture);
                    gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_WRAP_S, ffi::CLAMP_TO_EDGE as i32);
                    gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_WRAP_T, ffi::CLAMP_TO_EDGE as i32);
                    gl.BindTexture(ffi::TEXTURE_2D, 0);
                })?;
            }
            // New textures hold whatever memory they were given; the passes only write
            // around the panes, so the rest has to start out empty.
            for level in &mut self.levels {
                let level_size = level.size();
                let area = Rectangle::<i32, Physical>::from_size((level_size.w, level_size.h).into());
                let mut target = renderer.bind(level)?;
                let mut frame = renderer.render(&mut target, area.size, OFFSCREEN)?;
                frame.clear(Color32F::TRANSPARENT, &[area])?;
                let _ = frame.finish()?;
            }
            self.tracker = Some(OutputDamageTracker::new(size, scale, OFFSCREEN));
            self.scale = scale;
            self.areas.clear();
        }
        let Some(tracker) = self.tracker.as_mut() else {
            return Ok(());
        };

        let (damaged, changed) = {
            let mut target = renderer.bind(&mut self.levels[0])?;
            // The texture keeps the last frame, so only what changed since is drawn again.
            let result = tracker
                .render_output(renderer, &mut target, 1, &behind, Color32F::TRANSPARENT)
                .map_err(|err| format!("{err:?}"))?;
            let near = result.damage.is_some_and(|damage| {
                damage.iter().any(|damage| {
                    areas.iter().any(|area| {
                        let reach = Rectangle::new(
                            area.loc - Point::from((reach, reach)),
                            area.size + Size::from((reach * 2, reach * 2)),
                        );
                        reach.overlaps(*damage)
                    })
                })
            });
            (result.damage.is_some(), near)
        };
        // A pane where there was none shows a part of the blur that may be out of date; one
        // within where the blur is right, as a bar growing over glass that covers the screen,
        // shows a part that is not.
        let covered = areas
            .iter()
            .all(|area| self.areas.iter().any(|right| right.contains_rect(*area)));
        if !changed && covered {
            // What changed was too far to show in these panes, but may show in the rest.
            if damaged {
                self.areas = areas.clone();
            }
            return Ok(());
        }
        self.areas = areas.clone();

        // Only the parts of the scene that can show through a pane are blurred.
        let regions: Vec<_> = areas
            .iter()
            .map(|area| {
                Rectangle::new(
                    area.loc - Point::from((reach, reach)),
                    area.size + Size::from((reach * 2, reach * 2)),
                )
            })
            .collect();
        let full = self.levels[0].size();
        for level in 0..LEVELS {
            let source = self.levels[level].clone();
            pass(
                renderer,
                &source,
                &mut self.levels[level + 1],
                (down, spread),
                &regions,
                full,
                (level + 1) % 2 == 1,
            )?;
        }
        for level in (1..LEVELS).rev() {
            let source = self.levels[level + 1].clone();
            pass(
                renderer,
                &source,
                &mut self.levels[level],
                (up, spread),
                &regions,
                full,
                level % 2 == 1,
            )?;
        }
        self.commit.increment();
        Ok(())
    }

    fn pane(
        &self,
        program: &GlesTexProgram,
        id: Id,
        rect: Rectangle<i32, Physical>,
        radius: i32,
    ) -> Option<Pane> {
        let texture = self.levels.get(1)?.clone();
        let blurred = texture.size();
        let full = self.levels[0].size();
        let scale_x = f64::from(blurred.w) / f64::from(full.w);
        let scale_y = f64::from(blurred.h) / f64::from(full.h);
        let at = rect.loc - self.origin;
        let src = Rectangle::<f64, Buffer>::new(
            (f64::from(at.x) * scale_x, f64::from(at.y) * scale_y).into(),
            (f64::from(rect.size.w) * scale_x, f64::from(rect.size.h) * scale_y).into(),
        );
        Some(Pane {
            id,
            commit: self.commit,
            texture,
            program: program.clone(),
            src,
            dst: rect,
            radius: radius.min(rect.size.w / 2).min(rect.size.h / 2),
            alpha: 1.0,
            reshaped: false,
            circular: false,
        })
    }
}

/// Draws `source` over `target` through one of the blur programs, where `regions` of the
/// full-size scene fall in `target`.
fn pass(
    renderer: &mut GlesRenderer,
    source: &GlesTexture,
    target: &mut GlesTexture,
    (program, spread): (&GlesTexProgram, f32),
    regions: &[Rectangle<i32, Physical>],
    full: Size<i32, Buffer>,
    upright: bool,
) -> Result<(), GlesError> {
    let from = source.size();
    let to = target.size();
    let area = Rectangle::<i32, Physical>::from_size((to.w, to.h).into());
    let scale = (
        f64::from(to.w) / f64::from(full.w),
        f64::from(to.h) / f64::from(full.h),
    );
    // Each pass turns the picture upside down: the scene drawn first is, the first half
    // size is not, and so on. The frame flips what it is told too, so a region goes where
    // it is in an upright target only if it is named mirrored, and as it is otherwise.
    let parts: Vec<_> = regions
        .iter()
        .filter_map(|region| {
            let part = region
                .to_f64()
                .upscale(scale)
                .to_i32_up::<i32>()
                .intersection(area)?;
            Some(if upright {
                Rectangle::new(
                    (part.loc.x, area.size.h - part.loc.y - part.size.h).into(),
                    part.size,
                )
            } else {
                part
            })
        })
        .collect();
    let mut target = renderer.bind(target)?;
    let mut frame = renderer.render(&mut target, area.size, OFFSCREEN)?;
    // Opaque, so it replaces what the texture held instead of blending over it.
    frame.render_texture_from_to(
        source,
        Rectangle::from_size(from.to_f64()),
        area,
        &parts,
        &parts,
        Transform::Normal,
        1.0,
        Some(program),
        &[
            Uniform::new("half_pixel", (0.5 / from.w as f32, 0.5 / from.h as f32)),
            Uniform::new("spread", spread),
        ],
    )?;
    // The next pass runs in the same GL context, which keeps the order; nothing to wait for.
    let _ = frame.finish()?;
    Ok(())
}

/// The glass behind one surface.
#[derive(Clone)]
pub struct Pane {
    id: Id,
    commit: CommitCounter,
    texture: GlesTexture,
    program: GlesTexProgram,
    src: Rectangle<f64, Buffer>,
    dst: Rectangle<i32, Physical>,
    radius: i32,
    alpha: f32,
    /// Drawn again whole, as while its corners turn.
    reshaped: bool,
    /// With corners round as a circle's, as a window's GTK or the compositor rounds.
    circular: bool,
}

impl Pane {
    /// The same glass, only partly there, as while it fades in.
    pub fn faded(self, alpha: f32) -> Self {
        Self { alpha, ..self }
    }

    /// The same glass, drawn again whole each frame while `reshaped`.
    pub fn reshaped(self, reshaped: bool) -> Self {
        Self { reshaped, ..self }
    }

    /// The same glass, its corners round as a circle's, to follow a window's.
    pub fn circular(self) -> Self {
        Self { circular: true, ..self }
    }
}

impl Element for Pane {
    fn id(&self) -> &Id {
        &self.id
    }

    fn alpha(&self) -> f32 {
        self.alpha
    }

    fn current_commit(&self) -> CommitCounter {
        self.commit
    }

    fn damage_since(&self, scale: Scale<f64>, commit: Option<CommitCounter>) -> DamageSet<i32, Physical> {
        if self.reshaped || commit != Some(self.commit) {
            return DamageSet::from_slice(&[Rectangle::from_size(self.geometry(scale).size)]);
        }
        DamageSet::default()
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        self.src
    }

    fn geometry(&self, _scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.dst
    }

    fn kind(&self) -> Kind {
        Kind::Unspecified
    }
}

impl RenderElement<GlesRenderer> for Pane {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        let texture = self.texture.size().to_f64();
        // Its corners and light are where they are on the whole pane, of which this may be a
        // part, stretched, as a window's bands are on their way into the Dock.
        let stretch = (dst.size.w as f64 / src.size.w, dst.size.h as f64 / src.size.h);
        let whole = (self.src.size.w * stretch.0, self.src.size.h * stretch.1);
        frame.render_texture_from_to(
            &self.texture,
            src,
            dst,
            damage,
            opaque_regions,
            Transform::Normal,
            self.alpha,
            Some(&self.program),
            &[
                Uniform::new(
                    "uv_origin",
                    ((self.src.loc.x / texture.w) as f32, (self.src.loc.y / texture.h) as f32),
                ),
                Uniform::new(
                    "uv_size",
                    ((self.src.size.w / texture.w) as f32, (self.src.size.h / texture.h) as f32),
                ),
                Uniform::new("size", (whole.0 as f32, whole.1 as f32)),
                Uniform::new("radius", self.radius as f32),
                Uniform::new("edge", if self.dst.loc.y > 0 { 1.0f32 } else { 0.0 }),
                Uniform::new("circular", if self.circular { 1.0f32 } else { 0.0 }),
            ],
        )
    }
}
