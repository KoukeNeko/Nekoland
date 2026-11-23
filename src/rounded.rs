//! An X11 program's window with its corners rounded. Wayland programs round their own, as
//! GTK does; an X11 one that draws its own frame, as Electron's do, draws it square because
//! X11 gives it no way to know the desktop is composited, so the corners are cut here, by a
//! shader that draws the window's surface within a rounded rectangle. Nor can it have the
//! soft shadow GTK's windows draw around themselves, so that is drawn here too.

use std::cell::{OnceCell, RefCell};

use smithay::{
    backend::renderer::{
        element::{
            surface::{WaylandSurfaceRenderElement, WaylandSurfaceTexture},
            Element, Id, Kind, RenderElement,
        },
        gles::{
            element::PixelShaderElement, GlesError, GlesFrame, GlesPixelProgram, GlesRenderer, GlesTexProgram,
            Uniform, UniformName, UniformType,
        },
        utils::{CommitCounter, DamageSet, OpaqueRegions},
        Texture,
    },
    desktop::Window,
    utils::{Buffer, Logical, Physical, Point, Rectangle, Scale, Size, Transform},
};

use crate::render::SceneElement;

/// The surface within a rounded rectangle, `area` of where it is drawn; what lies outside it
/// is left clear.
const ROUNDED: &str = r#"#version 100
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
uniform vec2 dst_size;
uniform vec2 uv_origin;
uniform vec2 uv_size;
uniform float radius;
uniform vec4 area;
varying vec2 v_coords;
void main() {
    vec4 color = texture2D(tex, v_coords);
#if defined(NO_ALPHA)
    color = vec4(color.rgb, 1.0);
#endif
    // v_coords is in the whole texture, which may be larger than what is shown.
    vec2 centered = (v_coords - uv_origin) / uv_size * dst_size - (area.xy + area.zw * 0.5);
    vec2 q = abs(centered) - (area.zw * 0.5 - vec2(radius));
    float distance = length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - radius;
    gl_FragColor = color * clamp(0.5 - distance, 0.0, 1.0) * alpha;
}
"#;

thread_local! {
    // shortcut: one renderer draws everything, so its program is kept here rather than
    // handed through every backend; a second renderer would need one of its own.
    static PROGRAM: OnceCell<Option<GlesTexProgram>> = const { OnceCell::new() };
}

fn program(renderer: &mut GlesRenderer) -> Option<GlesTexProgram> {
    PROGRAM.with(|program| {
        program
            .get_or_init(|| {
                let uniforms = [
                    UniformName::new("dst_size", UniformType::_2f),
                    UniformName::new("uv_origin", UniformType::_2f),
                    UniformName::new("uv_size", UniformType::_2f),
                    UniformName::new("radius", UniformType::_1f),
                    UniformName::new("area", UniformType::_4f),
                ];
                renderer
                    .compile_custom_texture_shader(ROUNDED, &uniforms)
                    .inspect_err(|err| tracing::warn!("rounded shader: {err}"))
                    .ok()
            })
            .clone()
    })
}

/// How a window's surfaces are drawn: within `body`, where the window is on the output at
/// `scale`, with its corners `radius` physical pixels round as they are drawn; a body that
/// reaches above the surfaces, under a bar, leaves their top corners square. All of them are
/// drawn again while `turning`, as the corners are.
pub struct Rounding {
    program: GlesTexProgram,
    radius: i32,
    body: Rectangle<i32, Physical>,
    scale: f64,
    turning: bool,
}

impl Rounding {
    /// None where the shader cannot be had.
    pub fn new(
        renderer: &mut GlesRenderer,
        radius: i32,
        body: Rectangle<i32, Physical>,
        scale: f64,
        turning: bool,
    ) -> Option<Self> {
        let program = program(renderer)?;
        Some(Self { program, radius, body, scale, turning })
    }

    pub fn apply(&self, inner: WaylandSurfaceRenderElement<GlesRenderer>) -> RoundedSurface {
        let own = inner.geometry(Scale::from(self.scale));
        RoundedSurface {
            inner,
            program: self.program.clone(),
            radius: self.radius,
            area: Rectangle::new(self.body.loc - own.loc, self.body.size),
            own: own.size,
            turning: self.turning,
        }
    }
}

pub struct RoundedSurface {
    inner: WaylandSurfaceRenderElement<GlesRenderer>,
    program: GlesTexProgram,
    radius: i32,
    /// The body, from where the surface starts, as large as the surface's `own` size.
    area: Rectangle<i32, Physical>,
    own: Size<i32, Physical>,
    turning: bool,
}

impl Element for RoundedSurface {
    fn id(&self) -> &Id {
        self.inner.id()
    }

    fn current_commit(&self) -> CommitCounter {
        self.inner.current_commit()
    }

    fn location(&self, scale: Scale<f64>) -> Point<i32, Physical> {
        self.inner.location(scale)
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        self.inner.src()
    }

    fn transform(&self) -> Transform {
        self.inner.transform()
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.inner.geometry(scale)
    }

    fn damage_since(&self, scale: Scale<f64>, commit: Option<CommitCounter>) -> DamageSet<i32, Physical> {
        if self.turning {
            return DamageSet::from_slice(&[Rectangle::from_size(self.geometry(scale).size)]);
        }
        self.inner.damage_since(scale, commit)
    }

    /// The corners are clear, so what is under them has to be drawn.
    fn opaque_regions(&self, _scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        OpaqueRegions::default()
    }

    fn alpha(&self) -> f32 {
        self.inner.alpha()
    }

    fn kind(&self) -> Kind {
        // Not Kind::ScanoutCandidate: the client's buffer must not reach the screen unshaded.
        Kind::Unspecified
    }
}

impl RenderElement<GlesRenderer> for RoundedSurface {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        // The part of the texture shown, as v_coords has it.
        let WaylandSurfaceTexture::Texture(texture) = self.inner.texture() else {
            return RenderElement::<GlesRenderer>::draw(&self.inner, frame, src, dst, damage, opaque_regions);
        };
        let tex_size = texture.size().to_f64();
        // Where it is drawn stretched, so is its body.
        let (fx, fy) = (
            f64::from(dst.size.w) / f64::from(self.own.w.max(1)),
            f64::from(dst.size.h) / f64::from(self.own.h.max(1)),
        );
        let area = (
            (f64::from(self.area.loc.x) * fx) as f32,
            (f64::from(self.area.loc.y) * fy) as f32,
            (f64::from(self.area.size.w) * fx) as f32,
            (f64::from(self.area.size.h) * fy) as f32,
        );
        frame.override_default_tex_program(
            self.program.clone(),
            vec![
                Uniform::new("dst_size", (dst.size.w as f32, dst.size.h as f32)),
                Uniform::new("uv_origin", ((src.loc.x / tex_size.w) as f32, (src.loc.y / tex_size.h) as f32)),
                Uniform::new("uv_size", ((src.size.w / tex_size.w) as f32, (src.size.h / tex_size.h) as f32)),
                Uniform::new("radius", self.radius as f32),
                Uniform::new("area", area),
            ],
        );
        let drawn = RenderElement::<GlesRenderer>::draw(&self.inner, frame, src, dst, damage, opaque_regions);
        frame.clear_tex_program_override();
        drawn
    }
}

/// How far a shadow reaches from the window, the widest of its layers: spread and blur.
const SHADOW_MARGIN: i32 = 20;

/// The shadow of a rounded rectangle, as libadwaita 1.10's `window.csd` gives GTK's windows,
/// with `box-shadow: 0 0 14px 5px 15%, 0 0 5px 2px 10%, 0 0 0 1px 5%` while it has the
/// focus and `0 0 10px 5px 8%, 0 0 0 1px 5%` when it does not. A blur of `b` px spans `b`
/// either way of the edge. It is drawn in logical pixels, in a rectangle `margin` larger than
/// the window on every side.
const SHADOW: &str = r#"
precision mediump float;
uniform float alpha;
uniform vec2 size;
uniform float radius;
uniform float margin;
uniform float strength;
uniform float focused;
varying vec2 v_coords;
float rounded(vec2 p, vec2 half_size, float r) {
    vec2 q = abs(p) - (half_size - vec2(r));
    return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - r;
}
float layer(vec2 p, vec2 half_size, float spread, float blur) {
    float d = rounded(p, half_size, radius) - spread;
    return 1.0 - smoothstep(-blur, blur, d);
}
void main() {
    vec2 p = (v_coords - 0.5) * size;
    vec2 half_size = size * 0.5 - vec2(margin);
    float d = rounded(p, half_size, radius);
    float line = clamp(1.0 - abs(d - 0.5), 0.0, 1.0);
    float shade = focused * (0.15 * layer(p, half_size, 5.0, 14.0) + 0.10 * layer(p, half_size, 2.0, 5.0))
        + (1.0 - focused) * 0.08 * layer(p, half_size, 5.0, 10.0)
        + 0.05 * line;
    gl_FragColor = vec4(0.0, 0.0, 0.0, shade) * strength * alpha;
}
"#;

thread_local! {
    static SHADOW_PROGRAM: OnceCell<Option<GlesPixelProgram>> = const { OnceCell::new() };
}

/// A window's shadow element on the output at `output_loc`, kept so that its identity, and
/// with it what the renderer knows of its damage, lasts from frame to frame.
struct ShadowState {
    output_loc: Point<i32, Logical>,
    element: PixelShaderElement,
    strength: f32,
    focused: bool,
    radius: i32,
}

fn shadow_program(renderer: &mut GlesRenderer) -> Option<GlesPixelProgram> {
    SHADOW_PROGRAM.with(|program| {
        program
            .get_or_init(|| {
                let uniforms = [
                    UniformName::new("radius", UniformType::_1f),
                    UniformName::new("margin", UniformType::_1f),
                    UniformName::new("strength", UniformType::_1f),
                    UniformName::new("focused", UniformType::_1f),
                ];
                renderer
                    .compile_custom_pixel_shader(SHADOW, &uniforms)
                    .inspect_err(|err| tracing::warn!("shadow shader: {err}"))
                    .ok()
            })
            .clone()
    })
}

/// The shadow of `window` on the output at `output_loc`, drawn where it is shown as `body`
/// there, with corners `radius` round, at `strength` of its full darkness, as it is with
/// the focus or without.
pub fn shadow(
    renderer: &mut GlesRenderer,
    window: &Window,
    output_loc: Point<i32, Logical>,
    body: Rectangle<i32, Logical>,
    radius: i32,
    strength: f32,
    focused: bool,
) -> Option<SceneElement> {
    let program = shadow_program(renderer)?;
    let area = Rectangle::new(
        body.loc - Point::from((SHADOW_MARGIN, SHADOW_MARGIN)),
        body.size + Size::from((2 * SHADOW_MARGIN, 2 * SHADOW_MARGIN)),
    );
    let uniforms = || {
        vec![
            Uniform::new("radius", radius as f32),
            Uniform::new("margin", SHADOW_MARGIN as f32),
            Uniform::new("strength", strength),
            Uniform::new("focused", f32::from(u8::from(focused))),
        ]
    };
    // One an output: a single one moved to each output's place in turn would be damaged at
    // every frame with two.
    // shortcut: those of outputs moved or gone stay with the window; drop them if outputs
    // are ever rearranged often.
    let mut states = window
        .user_data()
        .get_or_insert(|| RefCell::new(Vec::<ShadowState>::new()))
        .borrow_mut();
    let state = match states.iter().position(|state| state.output_loc == output_loc) {
        Some(index) => {
            let state = &mut states[index];
            state.element.resize(area, None);
            if state.strength != strength || state.focused != focused || state.radius != radius {
                state.element.update_uniforms(uniforms());
                state.strength = strength;
                state.focused = focused;
                state.radius = radius;
            }
            state
        }
        None => {
            let element = PixelShaderElement::new(program, area, None, 1.0, uniforms(), Kind::Unspecified);
            states.push(ShadowState {
                output_loc,
                element,
                strength,
                focused,
                radius,
            });
            states.last_mut()?
        }
    };
    Some(SceneElement::Shadow(state.element.clone()))
}
