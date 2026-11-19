//! An X11 program's window with its corners rounded. Wayland programs round their own, as
//! GTK does; an X11 one that draws its own frame, as Electron's do, draws it square because
//! X11 gives it no way to know the desktop is composited, so the corners are cut here, by a
//! shader that draws the window's surface within a rounded rectangle.

use std::cell::OnceCell;

use smithay::{
    backend::renderer::{
        element::{
            surface::{WaylandSurfaceRenderElement, WaylandSurfaceTexture},
            Element, Id, Kind, RenderElement,
        },
        gles::{GlesError, GlesFrame, GlesRenderer, GlesTexProgram, Uniform, UniformName, UniformType},
        utils::{CommitCounter, DamageSet, OpaqueRegions},
        Texture,
    },
    utils::{Buffer, Physical, Point, Rectangle, Scale, Transform},
};

use crate::render::SceneElement;

/// The surface within a rounded rectangle as large as where it is drawn; what lies outside
/// the corners is left clear.
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
varying vec2 v_coords;
void main() {
    vec4 color = texture2D(tex, v_coords);
#if defined(NO_ALPHA)
    color = vec4(color.rgb, 1.0);
#endif
    // v_coords is in the whole texture, which may be larger than what is shown.
    vec2 centered = (v_coords - (uv_origin + uv_size * 0.5)) / uv_size * dst_size;
    vec2 q = abs(centered) - (dst_size * 0.5 - vec2(radius));
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
                ];
                renderer
                    .compile_custom_texture_shader(ROUNDED, &uniforms)
                    .inspect_err(|err| tracing::warn!("rounded shader: {err}"))
                    .ok()
            })
            .clone()
    })
}

/// `elements` with the surfaces among them drawn `radius` physical pixels round at the
/// corners; unchanged where the shader cannot be had.
pub fn round(renderer: &mut GlesRenderer, elements: Vec<SceneElement>, radius: i32) -> Vec<SceneElement> {
    let Some(program) = program(renderer) else {
        return elements;
    };
    elements
        .into_iter()
        .map(|element| match element {
            SceneElement::Surface(inner) => {
                SceneElement::Rounded(RoundedSurface { inner, program: program.clone(), radius })
            }
            other => other,
        })
        .collect()
}

pub struct RoundedSurface {
    inner: WaylandSurfaceRenderElement<GlesRenderer>,
    program: GlesTexProgram,
    radius: i32,
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
        frame.override_default_tex_program(
            self.program.clone(),
            vec![
                Uniform::new("dst_size", (dst.size.w as f32, dst.size.h as f32)),
                Uniform::new("uv_origin", ((src.loc.x / tex_size.w) as f32, (src.loc.y / tex_size.h) as f32)),
                Uniform::new("uv_size", ((src.size.w / tex_size.w) as f32, (src.size.h / tex_size.h) as f32)),
                Uniform::new("radius", self.radius as f32),
            ],
        );
        let drawn = RenderElement::<GlesRenderer>::draw(&self.inner, frame, src, dst, damage, opaque_regions);
        frame.clear_tex_program_override();
        drawn
    }
}
