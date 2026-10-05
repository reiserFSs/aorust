//! Character models: skinned `CATMesh` records (rdb 1010002, low-detail 1010027) and `CATAnim`
//! keyframe clips (rdb 1010003), see `docs/formats.md` § characters.
//!
//! A model record carries its own skeleton, per-vertex bone weights and a material → texture table
//! (rdb 1010004 ids); an animation record only holds bone tracks and fits every mesh with the same
//! skeleton hash. [`load_character`] returns the bind pose, [`load_character_posed`] skins the mesh
//! on the CPU at an animation time. Output follows the ao-scene contract: the model's left-handed
//! D3D space is mirrored by negating Z (and reversing triangle winding), as for static meshes.

mod anim;
mod cat;

pub use anim::{CatAnim, Track};
pub use cat::{Attractor, Bone, CatMesh, ColSphere, Material, Part, SkinVertex, SubMesh};

use crate::texture::load_texture;
use anyhow::{bail, ensure, Context, Result};
use ao_rdb::RecordStore;
use ao_scene::{Blend, Instance, Mesh, Scene, Submesh, TextureKey, Vertex, IDENTITY};
use std::collections::HashMap;

/// Skinned character/creature models.
pub const CHAR_MESH_TYPE: u32 = 1010002;
/// Low-detail variants of eight [`CHAR_MESH_TYPE`] ids (towers); same container.
pub const CHAR_MESH_LOW_TYPE: u32 = 1010027;
/// Keyframe animation clips.
pub const CHAR_ANIM_TYPE: u32 = 1010003;
/// Textures referenced by [`Part::texture`].
const TEXTURE_TYPE: u32 = 1010004;
/// Weights at or above this use bone 0 only (the engine's single-bone branch).
const SINGLE_BONE_WEIGHT: f32 = 0.999;

/// Little-endian cursor over a record.
pub(crate) struct Rd<'a> {
    d: &'a [u8],
    o: usize,
}

impl<'a> Rd<'a> {
    pub fn new(d: &'a [u8]) -> Self {
        Self { d, o: 0 }
    }
    pub fn remaining(&self) -> usize {
        self.d.len() - self.o
    }
    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        ensure!(n <= self.remaining(), "record truncated at {}", self.o);
        self.o += n;
        Ok(&self.d[self.o - n..self.o])
    }
    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.bytes(4)?.try_into().unwrap()))
    }
    pub fn i8(&mut self) -> Result<i8> {
        Ok(self.bytes(1)?[0] as i8)
    }
    pub fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_bits(self.u32()?))
    }
    pub fn f32s<const N: usize>(&mut self) -> Result<[f32; N]> {
        let mut out = [0.0; N];
        for c in &mut out {
            *c = self.f32()?;
        }
        Ok(out)
    }
    /// `u32` length + bytes, no terminator.
    pub fn string(&mut self) -> Result<String> {
        let n = self.u32()? as usize;
        Ok(String::from_utf8_lossy(self.bytes(n)?).into_owned())
    }
    /// 32-byte field holding a NUL-terminated string (the rest is padding/garbage).
    pub fn name32(&mut self) -> Result<String> {
        let b = self.bytes(32)?;
        let end = b.iter().position(|&c| c == 0).unwrap_or(32);
        Ok(String::from_utf8_lossy(&b[..end]).into_owned())
    }
}

/// Rigid transform `p' = r * p + t` (column vectors; the engine stores the transpose for row vectors).
#[derive(Clone, Copy, Debug)]
struct Xf {
    r: [[f32; 3]; 3],
    t: [f32; 3],
}

impl Xf {
    const ID: Xf = Xf { r: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], t: [0.0; 3] };

    fn from_qt([x, y, z, w]: [f32; 4], t: [f32; 3]) -> Self {
        let r = [
            [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y - z * w), 2.0 * (x * z + y * w)],
            [2.0 * (x * y + z * w), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z - x * w)],
            [2.0 * (x * z - y * w), 2.0 * (y * z + x * w), 1.0 - 2.0 * (x * x + y * y)],
        ];
        Self { r, t }
    }
    fn rot(&self, v: [f32; 3]) -> [f32; 3] {
        std::array::from_fn(|i| self.r[i][0] * v[0] + self.r[i][1] * v[1] + self.r[i][2] * v[2])
    }
    fn apply(&self, p: [f32; 3]) -> [f32; 3] {
        let v = self.rot(p);
        std::array::from_fn(|i| v[i] + self.t[i])
    }
    /// `self ∘ o` (apply `o` first).
    fn mul(&self, o: &Xf) -> Xf {
        Xf { r: std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| self.r[i][k] * o.r[k][j]).sum())), t: self.apply(o.t) }
    }
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| a[i] - b[i])
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn len(a: [f32; 3]) -> f32 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}
fn unit(a: [f32; 3]) -> [f32; 3] {
    let l = len(a);
    if l > 1e-12 { a.map(|c| c / l) } else { [0.0, 1.0, 0.0] }
}

/// World transform of every bone (`FUN_100540a5`): parent ∘ (rotation, translation × ancestor scale).
/// Bones without a track keep an identity rotation and zero offset.
fn bone_world(mesh: &CatMesh, pose: Option<(&CatAnim, f32)>) -> Vec<Xf> {
    let parents = mesh.parents();
    let mut world = vec![Xf::ID; mesh.bones.len()];
    let mut scale = vec![1.0f32; mesh.bones.len()];
    for b in mesh.bone_order() {
        let (q, t) = pose.and_then(|(a, ms)| a.sample(b, ms)).unwrap_or(([0.0, 0.0, 0.0, 1.0], [0.0; 3]));
        let local = Xf::from_qt(q, t.map(|c| c * scale[b]));
        world[b] = parents[b].map_or(local, |p| world[p].mul(&local));
        for &c in &mesh.bones[b].children {
            scale[c as usize] = mesh.bones[b].scale * scale[b];
        }
    }
    world
}

/// Bind-pose world rotation of each bone, recovered from vertices fully weighted to it
/// (`bind = R * local + t` holds exactly; the file stores normals bone-local but no bind matrices).
/// `None` when fewer than three non-collinear such vertices exist.
fn bind_rotations(mesh: &CatMesh) -> Vec<Option<[[f32; 3]; 3]>> {
    let mut pts: Vec<Vec<([f32; 3], [f32; 3])>> = vec![vec![]; mesh.bones.len()];
    for v in mesh.submeshes.iter().flat_map(|s| &s.vertices) {
        if v.weight >= SINGLE_BONE_WEIGHT {
            pts[v.bones[0] as usize].push((v.local[0], v.bind));
        }
    }
    pts.iter()
        .map(|p| {
            let (a0, c0) = *p.first()?;
            let far = |f: &dyn Fn(&([f32; 3], [f32; 3])) -> f32| p.iter().max_by(|x, y| f(x).total_cmp(&f(y))).copied();
            let (ab, cb) = far(&|q| len(sub(q.0, a0)))?;
            let u = sub(ab, a0);
            let (ac, cc) = far(&|q| len(cross(u, sub(q.0, a0))))?;
            // the third point must stand off the first two by at least 1 mm
            if len(cross(u, sub(ac, a0))) < 1e-3 * len(u) {
                return None;
            }
            let frame = |u: [f32; 3], v: [f32; 3]| {
                let e1 = unit(u);
                let e3 = unit(cross(e1, v));
                [e1, cross(e3, e1), e3]
            };
            let (ea, ec) = (frame(u, sub(ac, a0)), frame(sub(cb, c0), sub(cc, c0)));
            Some(std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| ec[k][i] * ea[k][j]).sum())))
        })
        .collect()
}

/// Area-weighted vertex normals of a D3D (clockwise-front) triangle list; `cross(e1, e2)` points outwards.
fn face_normals(pos: &[[f32; 3]], idx: &[u16]) -> Vec<[f32; 3]> {
    let mut n = vec![[0.0f32; 3]; pos.len()];
    for t in idx.chunks_exact(3) {
        let [a, b, c] = [0, 1, 2].map(|k| pos[t[k] as usize]);
        let f = cross(sub(b, a), sub(c, a));
        for &i in t {
            for k in 0..3 {
                n[i as usize][k] += f[k];
            }
        }
    }
    n.into_iter().map(unit).collect()
}

/// Position + normal of every vertex of every submesh, in the model's left-handed space.
type Skinned = Vec<Vec<([f32; 3], [f32; 3])>>;

fn skin_bind(mesh: &CatMesh) -> Skinned {
    let rot = bind_rotations(mesh);
    mesh.submeshes
        .iter()
        .map(|s| {
            let pos: Vec<_> = s.vertices.iter().map(|v| v.bind).collect();
            let geo = face_normals(&pos, &s.indices);
            s.vertices
                .iter()
                .enumerate()
                .map(|(i, v)| {
                    let n = rot[v.bones[0] as usize].map_or(geo[i], |r| Xf { r, t: [0.0; 3] }.rot(v.normal));
                    (v.bind, unit(n))
                })
                .collect()
        })
        .collect()
}

fn skin_pose(mesh: &CatMesh, world: &[Xf]) -> Skinned {
    mesh.submeshes
        .iter()
        .map(|s| {
            s.vertices
                .iter()
                .map(|v| {
                    let m0 = &world[v.bones[0] as usize];
                    let p0 = m0.apply(v.local[0]);
                    let p = if v.weight >= SINGLE_BONE_WEIGHT {
                        p0
                    } else {
                        let p1 = world[v.bones[1] as usize].apply(v.local[1]);
                        std::array::from_fn(|i| v.weight * p0[i] + (1.0 - v.weight) * p1[i])
                    };
                    (p, unit(m0.rot(v.normal)))
                })
                .collect()
        })
        .collect()
}

fn linear(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

/// Renderer material for one CAT material: blend from opacity and the texture's alpha channel
/// (`flags & 8` = alpha is not transparency), flat colour only when untextured.
fn submesh_for(mat: &Material, tex: Option<(TextureKey, &ao_scene::Texture)>) -> Submesh {
    let mut s = Submesh::new(vec![], tex.map(|t| t.0));
    let alpha_is_transparency = mat.flags & 8 == 0;
    let (mut cut, mut soft) = (false, false);
    if let (true, Some((_, t))) = (alpha_is_transparency, tex) {
        for a in t.rgba.iter().skip(3).step_by(4) {
            cut |= *a == 0;
            soft |= *a != 0 && *a != 255;
        }
    }
    s.blend = if mat.opacity < 1.0 || soft { Blend::AlphaBlend } else if cut { Blend::AlphaTest } else { Blend::Opaque };
    s.base_color = match tex {
        Some(_) => [1.0, 1.0, 1.0, mat.opacity],
        None => [linear(mat.diffuse[0]), linear(mat.diffuse[1]), linear(mat.diffuse[2]), mat.opacity],
    };
    s
}

fn assemble(store: &RecordStore, mesh: &CatMesh, skin: &Skinned) -> Scene {
    let mut scene = Scene::default();
    let mut out = Mesh::default();
    let mut textures: HashMap<u32, Option<TextureKey>> = HashMap::new();
    let mut by_material: HashMap<u32, usize> = HashMap::new();
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for (sm, sk) in mesh.submeshes.iter().zip(skin) {
        let base = out.vertices.len() as u32;
        for (v, &(p, n)) in sm.vertices.iter().zip(sk) {
            let pos = [p[0], p[1], -p[2]];
            for k in 0..3 {
                lo[k] = lo[k].min(pos[k]);
                hi[k] = hi[k].max(pos[k]);
            }
            out.vertices.push(Vertex { pos, normal: [n[0], n[1], -n[2]], uv: v.uv, ..Default::default() });
        }
        let si = *by_material.entry(sm.material).or_insert_with(|| {
            let part = &mesh.parts[sm.material as usize];
            let key = *textures.entry(part.texture).or_insert_with(|| {
                let key = TextureKey { rdb_type: TEXTURE_TYPE, id: part.texture };
                let tex = (part.texture != 0).then(|| load_texture(store, key).ok().flatten()).flatten()?;
                scene.textures.insert(key, tex);
                Some(key)
            });
            out.submeshes.push(submesh_for(&mesh.materials[sm.material as usize], key.map(|k| (k, &scene.textures[&k]))));
            out.submeshes.len() - 1
        });
        // mirroring Z turns the clockwise-front triangles counter-clockwise only if the winding is reversed
        out.submeshes[si].indices.extend(sm.indices.chunks_exact(3).flat_map(|t| [t[0], t[2], t[1]]).map(|i| base + i as u32));
    }
    out.submeshes.retain(|s| !s.indices.is_empty());
    scene.meshes.push(out);
    scene.instances.push(Instance { mesh: 0, transform: IDENTITY });
    if lo[0] <= hi[0] {
        let c: [f32; 3] = std::array::from_fn(|k| (lo[k] + hi[k]) / 2.0);
        let r = (0..3).map(|k| hi[k] - lo[k]).fold(0.0f32, f32::max);
        // characters face +Z in model space, i.e. -Z after the mirror
        scene.spawn = Some([c[0], c[1], c[2] - 1.8 * r]);
        scene.spawn_look_at = Some(c);
    }
    scene
}

/// Decodes character model `id` (rdb [`CHAR_MESH_TYPE`]) in its bind pose.
pub fn load_character(store: &RecordStore, id: u32) -> Result<Scene> {
    load_character_record(store, CHAR_MESH_TYPE, id, None)
}

/// Decodes character model `id` skinned by animation `anim_id` (rdb [`CHAR_ANIM_TYPE`]) at `time_s`
/// seconds; the clip loops. Fails if the animation belongs to a different skeleton.
pub fn load_character_posed(store: &RecordStore, id: u32, anim_id: u32, time_s: f32) -> Result<Scene> {
    load_character_record(store, CHAR_MESH_TYPE, id, Some((anim_id, time_s)))
}

/// General form: `rdb_type` is [`CHAR_MESH_TYPE`] or [`CHAR_MESH_LOW_TYPE`]; `pose` = (animation id, seconds).
pub fn load_character_record(store: &RecordStore, rdb_type: u32, id: u32, pose: Option<(u32, f32)>) -> Result<Scene> {
    let mesh = load_cat_mesh(store, rdb_type, id)?;
    let skin = match pose {
        None => skin_bind(&mesh),
        Some((anim_id, time_s)) => {
            let anim = load_anim(store, anim_id)?;
            ensure!(
                anim.signature == mesh.signature,
                "animation {anim_id} (skeleton {:#010x}) does not fit model {id} (skeleton {:#010x})",
                anim.signature,
                mesh.signature
            );
            let ms = time_s * 1000.0;
            let ms = if anim.duration > 0.0 { ms.rem_euclid(anim.duration) } else { 0.0 };
            skin_pose(&mesh, &bone_world(&mesh, Some((&anim, ms))))
        }
    };
    Ok(assemble(store, &mesh, &skin))
}

pub fn load_cat_mesh(store: &RecordStore, rdb_type: u32, id: u32) -> Result<CatMesh> {
    let bytes = store.get(rdb_type, id)?.with_context(|| format!("no character record {rdb_type}/{id}"))?;
    CatMesh::parse(&bytes).with_context(|| format!("decoding character {rdb_type}/{id}"))
}

pub fn load_anim(store: &RecordStore, id: u32) -> Result<CatAnim> {
    let Some(bytes) = store.get(CHAR_ANIM_TYPE, id)? else { bail!("no animation record {CHAR_ANIM_TYPE}/{id}") };
    CatAnim::parse(&bytes).with_context(|| format!("decoding animation {id}"))
}

/// Ids and lengths (ms) of all animations that fit `mesh` (equal skeleton hash), ascending by id.
pub fn compatible_animations(store: &RecordStore, mesh: &CatMesh) -> Result<Vec<(u32, f32)>> {
    let mut out = vec![];
    for id in store.ids(CHAR_ANIM_TYPE)? {
        if let Some(a) = store.get(CHAR_ANIM_TYPE, id)?.and_then(|b| CatAnim::parse(&b).ok()) {
            if a.signature == mesh.signature {
                out.push((id, a.duration));
            }
        }
    }
    Ok(out)
}
