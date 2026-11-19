//! Sprites: pictures drawn once and kept by the GPU, which then puts them wherever, as
//! large and turned as far as each frame says. What moves every frame, as the applications'
//! icons do, is made of these, so that an animation costs no drawing at all: only where the
//! same textures go changes.
//!
//! A sprite turned is drawn by a shader of its own, which turns the picture within its
//! rectangle; one that is not goes through the renderer's own.

use std::cell::{OnceCell, RefCell};

use cairo::ImageSurface;
use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            element::{Element, Id, Kind, RenderElement},
            gles::{
                GlesError, GlesFrame, GlesRenderer, GlesTexProgram, GlesTexture, Uniform, UniformName,
                UniformType,
            },
            utils::CommitCounter,
            ImportMem, Texture,
        },
    },
    utils::{Buffer, Logical, Physical, Point, Rectangle, Scale, Size, Transform},
};

/// The picture turned about its middle, by the sine and cosine it is given; `aspect` is its
/// width over its height, so that a picture that is not square turns without being sheared.
const TURNED: &str = r#"#version 100
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
uniform vec2 turn;
uniform float aspect;
varying vec2 v_coords;
void main() {
    vec2 from_middle = (v_coords - 0.5) * vec2(aspect, 1.0);
    vec2 turned = vec2(
        from_middle.x * turn.y + from_middle.y * turn.x,
        from_middle.y * turn.y - from_middle.x * turn.x
    ) / vec2(aspect, 1.0) + 0.5;
    vec2 within = step(vec2(0.0), turned) * step(turned, vec2(1.0));
    gl_FragColor = texture2D(tex, turned) * within.x * within.y * alpha;
}
"#;

thread_local! {
    // shortcut: one renderer draws everything, so its program is kept here rather than
    // handed through every backend; a second renderer would need one of its own.
    static TURNING: OnceCell<Option<GlesTexProgram>> = const { OnceCell::new() };
}

fn turning_program(renderer: &mut GlesRenderer) -> Option<GlesTexProgram> {
    TURNING.with(|program| {
        program
            .get_or_init(|| {
                let uniforms = [
                    UniformName::new("turn", UniformType::_2f),
                    UniformName::new("aspect", UniformType::_1f),
                ];
                renderer
                    .compile_custom_texture_shader(TURNED, &uniforms)
                    .inspect_err(|err| tracing::warn!("sprite shader: {err}"))
                    .ok()
            })
            .clone()
    })
}

/// A picture for the GPU to keep: its pixels until the first frame that draws it, the
/// texture from then on.
pub struct Sprite {
    size: Size<i32, Logical>,
    pixels: Size<i32, Buffer>,
    waiting: RefCell<Option<Vec<u8>>>,
    texture: RefCell<Option<GlesTexture>>,
}

impl Sprite {
    /// A sprite of what was drawn on `surface`, as large as the surface says it is.
    pub fn new(mut surface: ImageSurface) -> Option<Self> {
        let pixels = Size::from((surface.width(), surface.height()));
        let scale = (surface.device_scale().0.round() as i32).max(1);
        surface.flush();
        // Rows may be padded: the texture takes them packed.
        let stride = surface.stride() as usize;
        let row = pixels.w as usize * 4;
        let data = surface.data().ok()?;
        let packed: Vec<u8> = data.chunks(stride).flat_map(|line| &line[..row]).copied().collect();
        Some(Self {
            size: Size::from((pixels.w / scale, pixels.h / scale)),
            pixels,
            waiting: RefCell::new(Some(packed)),
            texture: RefCell::new(None),
        })
    }

    pub fn size(&self) -> Size<i32, Logical> {
        self.size
    }

    fn texture(&self, renderer: &mut GlesRenderer) -> Option<GlesTexture> {
        if let Some(pixels) = self.waiting.borrow_mut().take() {
            // Cairo's ARGB32 is premultiplied and in native byte order, which is this format.
            *self.texture.borrow_mut() = renderer
                .import_memory(&pixels, Fourcc::Argb8888, self.pixels, false)
                .inspect_err(|err| tracing::warn!("sprite: {err}"))
                .ok();
        }
        self.texture.borrow().clone()
    }
}

/// Where a sprite goes this frame.
pub struct Placed<'a> {
    pub sprite: &'a Sprite,
    /// What tells this drawing of it from any other, from one frame to the next.
    pub id: &'a Id,
    pub rect: Rectangle<f64, Logical>,
    pub alpha: f32,
    /// How far it is turned about its middle, in radians.
    pub turn: f64,
    /// What it shows within, if not all of it: the rest of it is not drawn.
    pub clip: Option<Rectangle<f64, Logical>>,
}

impl Placed<'_> {
    /// The same, `factor` times as large and as far from `pivot`.
    pub fn scaled_about(mut self, pivot: Point<f64, Logical>, factor: f64) -> Self {
        let from_pivot = self.rect.loc - pivot;
        self.rect = Rectangle::new(
            pivot + Point::from((from_pivot.x * factor, from_pivot.y * factor)),
            Size::from((self.rect.size.w * factor, self.rect.size.h * factor)),
        );
        self.clip = self.clip.map(|clip| {
            let from_pivot = clip.loc - pivot;
            Rectangle::new(
                pivot + Point::from((from_pivot.x * factor, from_pivot.y * factor)),
                Size::from((clip.size.w * factor, clip.size.h * factor)),
            )
        });
        self
    }

    pub fn clipped(mut self, to: Rectangle<f64, Logical>) -> Self {
        self.clip = Some(to);
        self
    }

    pub fn faded(mut self, alpha: f32) -> Self {
        self.alpha *= alpha;
        self
    }

    /// What draws it on an output at `output_loc`, or nothing for one that cannot be seen.
    pub fn element(
        &self,
        renderer: &mut GlesRenderer,
        output_loc: Point<i32, Logical>,
        scale: f64,
    ) -> Option<SpriteElement> {
        if self.alpha <= 0.0 || self.rect.size.w < 1.0 || self.rect.size.h < 1.0 {
            return None;
        }
        let shown = match self.clip {
            Some(clip) => self.rect.intersection(clip)?,
            None => self.rect,
        };
        let texture = self.sprite.texture(renderer)?;
        // As much of the picture as of its place is shown.
        let (whole, grown) = (texture.size().to_f64(), self.rect.size);
        let (from, kept) = (shown.loc - self.rect.loc, shown.size);
        let src = Rectangle::new(
            (from.x / grown.w * whole.w, from.y / grown.h * whole.h).into(),
            (kept.w / grown.w * whole.w, kept.h / grown.h * whole.h).into(),
        );
        let rect = Rectangle::new(shown.loc - output_loc.to_f64(), shown.size);
        let program = (self.turn != 0.0).then(|| turning_program(renderer)).flatten();
        Some(SpriteElement {
            id: self.id.clone(),
            texture,
            src,
            program,
            dst: rect.to_physical(scale).to_i32_round(),
            alpha: self.alpha,
            turn: self.turn,
        })
    }
}

pub struct SpriteElement {
    id: Id,
    texture: GlesTexture,
    src: Rectangle<f64, Buffer>,
    program: Option<GlesTexProgram>,
    dst: Rectangle<i32, Physical>,
    alpha: f32,
    turn: f64,
}

impl Element for SpriteElement {
    fn id(&self) -> &Id {
        &self.id
    }

    fn alpha(&self) -> f32 {
        self.alpha
    }

    /// Nothing tells one turn from the next but this: the picture itself never changes.
    fn current_commit(&self) -> CommitCounter {
        let steps = (self.turn * TURN_STEPS).round() as i64;
        CommitCounter::from(steps.rem_euclid(TURN_COMMITS) as usize)
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

/// How finely a turn is told from the one drawn before it, to the radian, and how many
/// tellings apart there are before they come round again.
const TURN_STEPS: f64 = 2000.0;
const TURN_COMMITS: i64 = 4096;

impl RenderElement<GlesRenderer> for SpriteElement {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        let uniforms = [
            Uniform::new("turn", (self.turn.sin() as f32, self.turn.cos() as f32)),
            Uniform::new("aspect", (src.size.w / src.size.h.max(1.0)) as f32),
        ];
        frame.render_texture_from_to(
            &self.texture,
            src,
            dst,
            damage,
            opaque_regions,
            Transform::Normal,
            self.alpha,
            self.program.as_ref(),
            if self.program.is_some() { &uniforms } else { &[] },
        )
    }
}
