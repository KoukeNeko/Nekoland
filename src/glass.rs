//! Catseye（貓眼）, Nekoland's glass: what floats over the desktop is drawn over a blur of
//! what is behind it, a little more saturated, with a thin line of light just inside its top
//! edge, like the band of light across a cat's-eye stone. Glass that meets the top of the
//! screen, as the menu bar does, has no edge there to light.
//!
//! Once a frame, everything under the floating layers is drawn into a texture, which is
//! blurred by halving it a few times and doubling it back (dual Kawase). Each glass surface
//! then gets an element right behind it that shows its part of the blur, cut to its corners.

use std::collections::HashMap;

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            damage::OutputDamageTracker,
            element::{Element, Id, Kind, RenderElement},
            gles::{
                ffi, GlesError, GlesFrame, GlesRenderer, GlesTexProgram, GlesTexture, Uniform, UniformName,
                UniformType,
            },
            utils::CommitCounter,
            Bind, Color32F, Frame, Offscreen, Renderer, Texture,
        },
    },
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Buffer, IsAlive, Physical, Point, Rectangle, Scale, Size, Transform},
};

/// How many times the scene is halved. Each one roughly doubles how far the blur reaches.
const LEVELS: usize = 4;
/// How far apart the samples of a pass are, in texels of the smaller texture.
const SPREAD: f32 = 2.0;
/// How far from a pane a change behind it still shows in its blur, in pixels.
const REACH: i32 = (SPREAD as i32) << (LEVELS + 1);
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

// The glass itself: the blur, cut to a rounded rectangle, with the line of light.
const PANE: &str = r#"
uniform vec2 uv_origin;
uniform vec2 uv_size;
uniform vec2 size;
uniform float radius;
uniform float edge;
void main() {
    vec2 local = (v_coords - uv_origin) / uv_size * size;
    vec2 q = abs(local - size * 0.5) - (size * 0.5 - vec2(radius));
    float distance = length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - radius;
    float inside = clamp(0.5 - distance, 0.0, 1.0);

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
    /// The scene behind the glass, then copies of it at half, a quarter, and so on.
    levels: Vec<GlesTexture>,
    tracker: Option<OutputDamageTracker>,
    /// Where the panes were when the scene was last blurred.
    areas: Vec<Rectangle<i32, Physical>>,
    commit: CommitCounter,
    ids: HashMap<WlSurface, Id>,
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
        ];
        Ok(Self {
            down: renderer.compile_custom_texture_shader(format!("{SAMPLER}{DOWN}"), &pass_uniforms)?,
            up: renderer.compile_custom_texture_shader(format!("{SAMPLER}{UP}"), &pass_uniforms)?,
            pane: renderer.compile_custom_texture_shader(format!("{SAMPLER}{PANE}"), &pane_uniforms)?,
            levels: Vec::new(),
            tracker: None,
            areas: Vec::new(),
            commit: CommitCounter::default(),
            ids: HashMap::new(),
        })
    }

    /// Draws what is behind the glass and blurs it again, when what changed since the last
    /// frame is near enough to one of the `areas` the panes cover to show in it.
    pub fn update<E: RenderElement<GlesRenderer>>(
        &mut self,
        renderer: &mut GlesRenderer,
        size: Size<i32, Physical>,
        areas: &[Rectangle<i32, Physical>],
        behind: &[E],
    ) -> Result<(), Box<dyn std::error::Error>> {
        let full = Size::<i32, Buffer>::from((size.w, size.h));
        if self.levels.first().map(Texture::size) != Some(full) {
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
            self.tracker = Some(OutputDamageTracker::new(size, 1.0, OFFSCREEN));
            self.areas.clear();
        }
        let Some(tracker) = self.tracker.as_mut() else {
            return Ok(());
        };

        let changed = {
            let mut target = renderer.bind(&mut self.levels[0])?;
            // The texture keeps the last frame, so only what changed since is drawn again.
            let result = tracker
                .render_output(renderer, &mut target, 1, behind, Color32F::TRANSPARENT)
                .map_err(|err| format!("{err:?}"))?;
            result.damage.is_some_and(|damage| {
                damage.iter().any(|damage| {
                    areas.iter().any(|area| {
                        let reach = Rectangle::new(
                            area.loc - Point::from((REACH, REACH)),
                            area.size + Size::from((REACH * 2, REACH * 2)),
                        );
                        reach.overlaps(*damage)
                    })
                })
            })
        };
        // A pane where there was none shows a part of the blur that may be out of date.
        if !changed && self.areas == areas {
            return Ok(());
        }
        self.areas = areas.to_vec();

        // Only the parts of the scene that can show through a pane are blurred.
        let regions: Vec<_> = areas
            .iter()
            .map(|area| {
                Rectangle::new(
                    area.loc - Point::from((REACH, REACH)),
                    area.size + Size::from((REACH * 2, REACH * 2)),
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
                &self.down,
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
                &self.up,
                &regions,
                full,
                level % 2 == 1,
            )?;
        }
        self.commit.increment();
        Ok(())
    }

    /// The glass behind a surface, over `rect` of the output, with corners of `radius`.
    pub fn pane(&mut self, surface: &WlSurface, rect: Rectangle<i32, Physical>, radius: i32) -> Option<Pane> {
        self.ids.retain(|surface, _| surface.alive());
        let id = self.ids.entry(surface.clone()).or_insert_with(Id::new).clone();
        self.pane_with(id, rect, radius)
    }

    /// The glass behind something the compositor draws itself, which names it with `id`.
    pub fn pane_with(&self, id: Id, rect: Rectangle<i32, Physical>, radius: i32) -> Option<Pane> {
        let texture = self.levels.get(1)?.clone();
        let blurred = texture.size();
        let full = self.levels[0].size();
        let scale_x = f64::from(blurred.w) / f64::from(full.w);
        let scale_y = f64::from(blurred.h) / f64::from(full.h);
        let src = Rectangle::<f64, Buffer>::new(
            (f64::from(rect.loc.x) * scale_x, f64::from(rect.loc.y) * scale_y).into(),
            (f64::from(rect.size.w) * scale_x, f64::from(rect.size.h) * scale_y).into(),
        );
        Some(Pane {
            id,
            commit: self.commit,
            texture,
            program: self.pane.clone(),
            src,
            dst: rect,
            radius: radius.min(rect.size.w / 2).min(rect.size.h / 2),
            alpha: 1.0,
        })
    }
}

/// Draws `source` over `target` through one of the blur programs, where `regions` of the
/// full-size scene fall in `target`.
fn pass(
    renderer: &mut GlesRenderer,
    source: &GlesTexture,
    target: &mut GlesTexture,
    program: &GlesTexProgram,
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
            Uniform::new("spread", SPREAD),
        ],
    )?;
    // The next pass runs in the same GL context, which keeps the order; nothing to wait for.
    let _ = frame.finish()?;
    Ok(())
}

/// The glass behind one surface.
pub struct Pane {
    id: Id,
    commit: CommitCounter,
    texture: GlesTexture,
    program: GlesTexProgram,
    src: Rectangle<f64, Buffer>,
    dst: Rectangle<i32, Physical>,
    radius: i32,
    alpha: f32,
}

impl Pane {
    /// The same glass, only partly there, as while it fades in.
    pub fn faded(self, alpha: f32) -> Self {
        Self { alpha, ..self }
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
                    ((src.loc.x / texture.w) as f32, (src.loc.y / texture.h) as f32),
                ),
                Uniform::new(
                    "uv_size",
                    ((src.size.w / texture.w) as f32, (src.size.h / texture.h) as f32),
                ),
                Uniform::new("size", (dst.size.w as f32, dst.size.h as f32)),
                Uniform::new("radius", self.radius as f32),
                Uniform::new("edge", if dst.loc.y > 0 { 1.0f32 } else { 0.0 }),
            ],
        )
    }
}
