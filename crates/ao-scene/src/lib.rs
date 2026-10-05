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
}

impl Submesh {
    pub fn new(indices: Vec<u32>, texture: Option<TextureKey>) -> Self {
        Self { indices, texture, blend: Blend::Opaque, base_color: WHITE, two_sided: false }
    }
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
}

pub const IDENTITY: [[f32; 4]; 4] = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];
