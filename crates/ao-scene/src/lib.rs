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

#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C)]
pub struct Vertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
}

/// Triangle list sharing the parent mesh's vertex buffer.
#[derive(Clone, Debug)]
pub struct Submesh {
    pub indices: Vec<u32>,
    /// Key into [`Scene::textures`]; `None` draws untextured (vertex-lit grey).
    pub texture: Option<TextureKey>,
    /// Alpha-tested (cutout) material, e.g. foliage and fences.
    pub alpha_test: bool,
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

#[derive(Clone, Debug, Default)]
pub struct Scene {
    pub textures: HashMap<TextureKey, Texture>,
    pub meshes: Vec<Mesh>,
    pub instances: Vec<Instance>,
    /// Suggested initial camera position (world space); `None` = frame the scene bounds.
    pub spawn: Option<[f32; 3]>,
}

pub const IDENTITY: [[f32; 4]; 4] = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];
