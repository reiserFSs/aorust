//! Renderer-agnostic scene description shared by `ao-formats` (producer) and `ao-render` (consumer).
//!
//! Conventions: right-handed, +Y up, units are AO world units (1.0 = 1 metre).
//! Matrices are column-major `[[f32; 4]; 4]` (`m[col][row]`), applied as `M * v`.

use std::collections::HashMap;

/// Decoded texture, tightly packed RGBA8 (sRGB), row-major, top row first.
#[derive(Clone, Debug)]
pub struct Texture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct Vertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    /// Linear RGBA multiplier on the lit texel (baked shadow/lighting, terrain blend weight in `a`).
    pub color: [f32; 4],
}

pub const WHITE: [f32; 4] = [1.0; 4];

impl Default for Vertex {
    fn default() -> Self {
        Self { pos: [0.0; 3], normal: [0.0, 1.0, 0.0], uv: [0.0; 2], color: WHITE }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Blend {
    #[default]
    Opaque,
    /// Cutout at alpha 0.5, depth-written (foliage, fences).
    AlphaTest,
    /// Src-alpha blending, drawn after opaques back-to-front, no depth write (glass, water).
    AlphaBlend,
    /// Additive (src*alpha + dst), no depth write (glows, beams).
    Additive,
}

/// Triangle list sharing the parent mesh's vertex buffer.
#[derive(Clone, Debug)]
pub struct Submesh {
    pub indices: Vec<u32>,
    /// Key into [`Scene::textures`]; `None` draws `base_color` only.
    pub texture: Option<TextureKey>,
    pub blend: Blend,
    /// Linear RGBA material colour, multiplied with texture and vertex colour.
    pub base_color: [f32; 4],
    /// Disable back-face culling (front faces are counter-clockwise in ao-scene space).
    pub two_sided: bool,
    /// Linear RGB added to the lighting term before texture modulation: `tex * (lighting + emissive)`, then fog.
    pub emissive: [f32; 3],
    /// Texture alpha is a self-illumination mask (opaque materials only): the texel is
    /// lit by `max(lighting, min(lighting + a, 1))` (engine: `saturate(a + lighting)`).
    pub glow_mask: bool,
    /// Baked-lighting surface (dungeon room shells): vertex colour is *emissive* light,
    /// `lit = tex * material * (vertex.rgb + 0.8 * ambient)`; no sun or point lights. Fog still applies.
    pub prelit: bool,
    /// Texture coordinate drift in uv units per second (added to the vertex uv, wrapped by the sampler): scrolling clouds,
    /// water-like layers. `[0, 0]` = static.
    pub uv_scroll: [f32; 2],
}

impl Submesh {
    pub fn new(indices: Vec<u32>, texture: Option<TextureKey>) -> Self {
        Self {
            indices,
            texture,
            blend: Blend::Opaque,
            base_color: WHITE,
            two_sided: false,
            emissive: [0.0; 3],
            glow_mask: false,
            prelit: false,
            uv_scroll: [0.0; 2],
        }
    }
}

/// Dynamic-style light (world space), the fixed function D3D7 light of the original client (`D3DLIGHT7`).
///
/// Intensity at distance `d <= range` is `1 / (atten0 + atten1 d + atten2 d^2)` (D3D attenuation), 0 beyond `range`;
/// with `atten == [0; 3]` it is the linear ramp `1 - d / range`. A [`Spot`] multiplies the D3D cone factor.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Light {
    pub pos: [f32; 3],
    /// Linear RGB (the D3D diffuse colour).
    pub color: [f32; 3],
    pub range: f32,
    /// `D3DLIGHT7.dvAttenuation0..2`; all zero = linear falloff to zero at `range`.
    pub atten: [f32; 3],
    pub spot: Option<Spot>,
}

/// `D3DLIGHT_SPOT` cone: full inner angle `theta` and outer angle `phi` (radians, `dvTheta` / `dvPhi`), falloff 1:
/// factor 1 inside `theta`, 0 outside `phi`, linear in `cos(angle / 2)` between.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spot {
    /// Unit axis (world space).
    pub dir: [f32; 3],
    pub theta: f32,
    pub phi: f32,
}

#[derive(Clone, Debug, Default)]
pub struct Mesh {
    pub vertices: Vec<Vertex>,
    pub submeshes: Vec<Submesh>,
}

/// Placement of `Scene::meshes[mesh]` in world space.
#[derive(Clone, Copy, Debug)]
pub struct Instance {
    pub mesh: usize,
    pub transform: [[f32; 4]; 4],
}

/// Texture identity: the rdb record (type, id) it was decoded from, or a synthetic id
/// for textures generated at load time (e.g. baked terrain) using `rdb_type = 0`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TextureKey {
    pub rdb_type: u32,
    pub id: u32,
}

/// Per-playfield atmosphere; linear RGB.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Environment {
    pub sky_color: [f32; 3],
    pub fog_color: [f32; 3],
    /// Fog is 0 at `fog_start` metres, full at `fog_end`.
    pub fog_start: f32,
    pub fog_end: f32,
    pub ambient: [f32; 3],
    pub sun_color: [f32; 3],
    /// Unit vector pointing from the scene towards the sun.
    pub sun_dir: [f32; 3],
}

#[derive(Clone, Debug, Default)]
pub struct Scene {
    pub textures: HashMap<TextureKey, Texture>,
    pub meshes: Vec<Mesh>,
    pub instances: Vec<Instance>,
    /// Suggested initial camera position (world space); `None` = frame the scene bounds.
    pub spawn: Option<[f32; 3]>,
    /// Point the initial camera looks at; `None` = renderer's choice.
    pub spawn_look_at: Option<[f32; 3]>,
    /// `None` = renderer defaults.
    pub environment: Option<Environment>,
    pub lights: Vec<Light>,
    /// Sky domes/backdrops: instances of `meshes` drawn first, depth-write off, unfogged,
    /// with the transform's translation replaced by the camera position. Not in `instances`.
    pub sky: Vec<Instance>,
}

pub const IDENTITY: [[f32; 4]; 4] = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];
