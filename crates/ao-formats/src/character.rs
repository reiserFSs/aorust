//! Character models: skinned `CATMesh` records (rdb 1010002, low-detail 1010027) and `CATAnim`
//! keyframe clips (rdb 1010003), see `docs/formats.md` § characters.
//!
//! A model record carries its own skeleton, per-vertex bone weights and a material → texture table
//! (rdb 1010004 ids); an animation record only holds bone tracks and fits every mesh with the same
//! skeleton hash. [`load_character`] returns the bind pose, [`load_character_posed`] skins the mesh
//! on the CPU at an animation time. Output follows the ao-scene contract: the model's left-handed
//! D3D space is mirrored by negating Z (and reversing triangle winding), as for static meshes.

pub mod actor;
mod anim;
mod cat;
mod names;
mod player;
mod viewer_cache;

pub use names::NameTable;
pub use player::*;
pub use viewer_cache::{CachedCharacter, ClothEntry, MeshEntry, ViewerCache};
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
/// Per-material texture overrides of a character: material name (`body`, `hands`, …) → texture + its key.
pub type PartTextures = HashMap<String, (TextureKey, ao_scene::Texture)>;
/// A vertex's (bone-local or bind, other) position pair.
type BindPair = ([f32; 3], [f32; 3]);
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
    let scale = translation_scales(mesh);
    let mut world = vec![Xf::ID; mesh.bones.len()];
    for b in mesh.bone_order() {
        let (q, t) = pose.and_then(|(a, ms)| a.sample(b, ms)).unwrap_or(([0.0, 0.0, 0.0, 1.0], [0.0; 3]));
        let local = Xf::from_qt(q, t.map(|c| c * scale[b]));
        world[b] = parents[b].map_or(local, |p| world[p].mul(&local));
    }
    world
}

/// Factor applied to each bone's animated translation: the product of its ancestors' `Bone::scale`
/// (race proportions: the opifex skeleton shortens the shared human clips' bone lengths).
fn translation_scales(mesh: &CatMesh) -> Vec<f32> {
    let mut scale = vec![1.0f32; mesh.bones.len()];
    for b in mesh.bone_order() {
        for &c in &mesh.bones[b].children {
            scale[c as usize] = mesh.bones[b].scale * scale[b];
        }
    }
    scale
}

/// Bind frame of a bone without skin vertices: the nearest fitted ancestor's frame carried down the
/// chain with the local transforms of `rest` (the clip whose first frame is closest to the bind pose).
fn derived_bind_frame(mesh: &CatMesh, frames: &[Option<Xf>], rest: &CatAnim, bone: usize) -> Xf {
    let (parents, scale) = (mesh.parents(), translation_scales(mesh));
    let (mut chain, mut cur) = (vec![], bone);
    let base = loop {
        if let Some(f) = frames[cur] {
            break f;
        }
        chain.push(cur);
        match parents[cur] {
            Some(p) => cur = p,
            None => break Xf::ID,
        }
    };
    chain.iter().rev().fold(base, |w, &b| {
        let (q, t) = rest.sample(b, 0.0).unwrap_or(([0.0, 0.0, 0.0, 1.0], [0.0; 3]));
        w.mul(&Xf::from_qt(q, t.map(|c| c * scale[b])))
    })
}

/// The compatible clip whose first frame deviates least (summed rotation angle) from the fitted bind frames.
fn best_rest_clip(store: &RecordStore, mesh: &CatMesh, frames: &[Option<Xf>]) -> Result<CatAnim> {
    let trace = |a: &Xf, b: &Xf| (0..3).map(|i| (0..3).map(|k| a.r[k][i] * b.r[k][i]).sum::<f32>()).sum::<f32>();
    let mut best: Option<(f32, CatAnim)> = None;
    for id in store.ids(CHAR_ANIM_TYPE)? {
        let Some(a) = store.get(CHAR_ANIM_TYPE, id)?.and_then(|b| CatAnim::parse(&b).ok()).filter(|a| a.signature == mesh.signature) else { continue };
        let world = bone_world(mesh, Some((&a, 0.0)));
        let score: f32 = frames.iter().zip(&world).filter_map(|(f, w)| f.map(|f| ((trace(&f, w) - 1.0) / 2.0).clamp(-1.0, 1.0).acos())).sum();
        if best.as_ref().is_none_or(|b| score < b.0) {
            best = Some((score, a));
        }
    }
    best.map(|b| b.1).context("no animation to derive the head bone's rest frame from")
}

/// Bind-pose world transform of each bone, recovered from vertices fully weighted to it
/// (`bind = R * local + t` holds exactly; the file stores normals bone-local but no bind matrices).
/// `None` when fewer than three non-collinear such vertices exist.
fn bind_frames(mesh: &CatMesh) -> Vec<Option<Xf>> {
    let mut pts: Vec<Vec<([f32; 3], [f32; 3])>> = vec![vec![]; mesh.bones.len()];
    for v in mesh.submeshes.iter().flat_map(|s| &s.vertices) {
        if v.weight >= SINGLE_BONE_WEIGHT {
            pts[v.bones[0] as usize].push((v.local[0], v.bind));
        }
    }
    pts.iter()
        .map(|p| {
            let (a0, c0) = *p.first()?;
            let far = |f: &dyn Fn(&BindPair) -> f32| p.iter().max_by(|x, y| f(x).total_cmp(&f(y))).copied();
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
            let r = std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| ec[k][i] * ea[k][j]).sum()));
            let ra = Xf { r, t: [0.0; 3] }.rot(a0);
            Some(Xf { r, t: sub(c0, ra) })
        })
        .collect()
}

/// Area-weighted vertex normals of a D3D (clockwise-front) triangle list; `cross(e1, e2)` points outwards.
fn face_normals(pos: &[[f32; 3]], idx: &[u16]) -> Vec<[f32; 3]> {
    let mut n = vec![[0.0f32; 3]; pos.len()];
    for t in idx.as_chunks::<3>().0 {
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

fn skin_bind(mesh: &CatMesh, frames: &[Option<Xf>]) -> Skinned {
    mesh.submeshes
        .iter()
        .map(|s| {
            let pos: Vec<_> = s.vertices.iter().map(|v| v.bind).collect();
            let geo = face_normals(&pos, &s.indices);
            s.vertices
                .iter()
                .enumerate()
                .map(|(i, v)| {
                    let n = frames[v.bones[0] as usize].map_or(geo[i], |f| f.rot(v.normal));
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

/// How far limbs have come apart in a pose, in metres: for every parent/child bone pair the nearest
/// distance between the two bones' rigidly skinned vertices (at most 256 sampled each) in the pose
/// minus the same in the bind pose; the maximum over pairs. A sound pose stays within a few
/// centimetres (the pieces overlap at the joint and rotate about it), detached limbs open a gap.
/// `time_s` loops like [`load_character_posed`].
pub fn pose_detachment(store: &RecordStore, id: u32, anim_id: u32, time_s: f32) -> Result<f32> {
    let mesh = load_cat_mesh(store, CHAR_MESH_TYPE, id)?;
    let anim = load_anim(store, anim_id)?;
    ensure!(anim.signature == mesh.signature, "animation {anim_id} does not fit model {id}");
    let ms = if anim.duration > 0.0 { (time_s * 1000.0).rem_euclid(anim.duration) } else { 0.0 };
    let world = bone_world(&mesh, Some((&anim, ms)));
    let mut sets: Vec<Vec<BindPair>> = vec![vec![]; mesh.bones.len()]; // (bind, posed)
    for v in mesh.submeshes.iter().flat_map(|s| &s.vertices).filter(|v| v.weight >= SINGLE_BONE_WEIGHT) {
        let b = v.bones[0] as usize;
        sets[b].push((v.bind, world[b].apply(v.local[0])));
    }
    let sample = |s: &[BindPair]| s.iter().step_by(s.len().div_ceil(256).max(1)).copied().collect::<Vec<_>>();
    let gap = |a: &[BindPair], b: &[BindPair]| {
        let min = |f: fn(&BindPair) -> [f32; 3]| a.iter().flat_map(|p| b.iter().map(move |q| len(sub(f(p), f(q))))).fold(f32::MAX, f32::min);
        min(|p| p.1) - min(|p| p.0)
    };
    let sets: Vec<_> = sets.iter().map(|s| sample(s)).collect();
    let mut worst = 0.0f32;
    for (p, bone) in mesh.bones.iter().enumerate() {
        for &c in &bone.children {
            if !sets[p].is_empty() && !sets[c as usize].is_empty() {
                worst = worst.max(gap(&sets[p], &sets[c as usize]));
            }
        }
    }
    Ok(worst)
}

/// The scene contract carries colours as `c^2.2`; the renderer shades in the client's gamma space and inverts that.
fn linear(c: f32) -> f32 {
    c.max(0.0).powf(2.2)
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
    // `RViewPort_t::SetMaterial` (randy31 @0x1004b199) copies only opac, emis, spec * shin_str and shin into the `_D3DMATERIAL7`: its
    // diffuse / ambient RGB stay white (`SetDefaultMaterial` @0x1004b61e), so `diffuse` / `ambient` tint nothing (they only reach D3D
    // through `InitD3DMaterial` @0x100409c6 for per-frame material modifiers); EMISSIVEMATERIALSOURCE is the device default MATERIAL.
    s.base_color = [1.0, 1.0, 1.0, mat.opacity];
    s.emissive = mat.emissive.map(linear);
    // `RMaterial_t::UpdateSpecular` @10040ff8 (ctor): SPECULARENABLE (29) = spec != black && shin_str > 0 && shin >= 0;
    // `_D3DMATERIAL7.specular` = spec * shin_str, power = shin (`InitD3DMaterial` @100409c6).
    if mat.specular != [0.0; 3] && mat.shininess_strength > 0.0 && mat.shininess >= 0.0 {
        s.specular = mat.specular.map(|c| linear(c * mat.shininess_strength));
        s.shininess = mat.shininess;
    }
    s
}

/// `with_head`: a head mesh is mounted, so the body's own `head` part (a one-triangle stub textured with
/// the green `head_*_default.png` placeholder on Atrox) is left out.
fn assemble(store: &RecordStore, mesh: &CatMesh, skin: &Skinned, overrides: &PartTextures, with_head: bool) -> (Scene, [f32; 3], [f32; 3]) {
    let mut scene = Scene::default();
    let mut out = Mesh::default();
    let mut textures: HashMap<u32, Option<TextureKey>> = HashMap::new();
    let mut by_material: HashMap<u32, usize> = HashMap::new();
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for (sm, sk) in mesh.submeshes.iter().zip(skin).filter(|(sm, _)| !(with_head && mesh.parts[sm.material as usize].name == "head")) {
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
            let key = match overrides.get(&part.name) {
                // player body materials: the client's skin/cloth composite, replaces the model's own texture
                Some((key, tex)) => {
                    scene.textures.insert(*key, tex.clone());
                    Some(*key)
                }
                None => *textures.entry(part.texture).or_insert_with(|| {
                    let key = TextureKey { rdb_type: TEXTURE_TYPE, id: part.texture };
                    let tex = (part.texture != 0).then(|| load_texture(store, key).ok().flatten()).flatten()?;
                    scene.textures.insert(key, tex);
                    Some(key)
                }),
            };
            out.submeshes.push(submesh_for(&mesh.materials[sm.material as usize], key.map(|k| (k, &scene.textures[&k]))));
            out.submeshes.len() - 1
        });
        // mirroring Z turns the clockwise-front triangles counter-clockwise only if the winding is reversed
        out.submeshes[si].indices.extend(sm.indices.as_chunks::<3>().0.iter().flat_map(|t| [t[0], t[2], t[1]]).map(|i| base + i as u32));
    }
    out.submeshes.retain(|s| !s.indices.is_empty());
    scene.meshes.push(out);
    scene.instances.push(Instance { mesh: 0, transform: IDENTITY });
    (scene, lo, hi)
}

/// Camera in front of the model's bounds (characters face +Z in model space, i.e. -Z after the mirror).
fn frame_view(scene: &mut Scene, lo: [f32; 3], hi: [f32; 3]) {
    if lo[0] <= hi[0] {
        let c: [f32; 3] = std::array::from_fn(|k| (lo[k] + hi[k]) / 2.0);
        let r = (0..3).map(|k| hi[k] - lo[k]).fold(0.0f32, f32::max);
        scene.spawn = Some([c[0], c[1], c[2] - 1.25 * r]);
        scene.spawn_look_at = Some(c);
    }
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
    build(store, rdb_type, id, pose, None, &HashMap::new())
}

/// Character `id` with the static head mesh `head_mesh_id` (rdb 1010001, e.g. 40098 = `head_athroxmale001`)
/// mounted on its `Attractor01_head` point; `pose` as in [`load_character_record`].
pub fn load_character_with_head(store: &RecordStore, id: u32, head_mesh_id: u32, pose: Option<(u32, f32)>) -> Result<Scene> {
    build(store, CHAR_MESH_TYPE, id, pose, Some(head_mesh_id), &HashMap::new())
}

/// [`load_character_with_head`] with an optional head and per-material texture overrides (material name →
/// texture), the skin/cloth composites of [`load_player`].
pub(crate) fn load_character_head_skin(
    store: &RecordStore,
    id: u32,
    head: Option<u32>,
    pose: Option<(u32, f32)>,
    overrides: &PartTextures,
) -> Result<Scene> {
    build(store, CHAR_MESH_TYPE, id, pose, head, overrides)
}

fn build(store: &RecordStore, rdb_type: u32, id: u32, pose: Option<(u32, f32)>, head: Option<u32>, overrides: &PartTextures) -> Result<Scene> {
    let mesh = load_cat_mesh(store, rdb_type, id)?;
    let (skin, frames) = match pose {
        None => {
            let frames = bind_frames(&mesh);
            (skin_bind(&mesh, &frames), frames)
        }
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
            let world = bone_world(&mesh, Some((&anim, ms)));
            (skin_pose(&mesh, &world), world.into_iter().map(Some).collect())
        }
    };
    let (mut scene, lo, mut hi) = assemble(store, &mesh, &skin, overrides, head.is_some());
    if let Some(head) = head {
        let att = mesh.attractors.iter().find(|a| a.name.ends_with("_head")).context("model has no head attractor")?;
        let bone = match frames[att.bone as usize] {
            Some(f) => f,
            None => derived_bind_frame(&mesh, &frames, &best_rest_clip(store, &mesh, &frames)?, att.bone as usize),
        };
        let w = bone.mul(&Xf::from_qt(att.rot, att.pos));
        let idx = crate::mesh::decode_mesh_into(store, head, &mut scene)?.with_context(|| format!("no head mesh {head}"))?;
        // the head vertices are already mirrored (Z negated): conjugate the mounting transform by the mirror
        let flip = |i: usize, j: usize| if (i == 2) != (j == 2) { -1.0 } else { 1.0 };
        let mut m = IDENTITY;
        for (c, col) in m.iter_mut().enumerate().take(3) {
            for (r, x) in col.iter_mut().enumerate().take(3) {
                *x = w.r[r][c] * flip(r, c);
            }
        }
        m[3] = [w.t[0], w.t[1], -w.t[2], 1.0];
        scene.instances.push(Instance { mesh: idx, transform: m });
        hi[1] = hi[1].max(w.t[1] + 0.3); // head height above the mount point
    }
    frame_view(&mut scene, lo, hi);
    Ok(scene)
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

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{write::ZlibEncoder, Compression};
    use std::io::Write;

    fn u(v: &mut Vec<u8>, x: u32) {
        v.extend(x.to_le_bytes());
    }
    fn f(v: &mut Vec<u8>, x: f32) {
        v.extend(x.to_le_bytes());
    }
    fn s(v: &mut Vec<u8>, x: &str) {
        u(v, x.len() as u32);
        v.extend(x.as_bytes());
    }
    fn name32(v: &mut Vec<u8>, x: &str) {
        let mut b = [0xCDu8; 32];
        b[..x.len()].copy_from_slice(x.as_bytes());
        b[x.len()] = 0;
        v.extend(b);
    }

    /// root bone 0 -> child bone 1; one triangle fully weighted to bone 1 whose bind pose is the child
    /// placed at (1,0,0); an attractor on bone 1. Texture table: 1 part.
    fn mesh_record() -> Vec<u8> {
        let mut v = vec![];
        name32(&mut v, "root");
        u(&mut v, 2 * 1009);
        name32(&mut v, "skin");
        for x in [7, 0, 0] {
            u(&mut v, x);
        }
        u(&mut v, 0x05010500); // unidentified word
        for x in [4, 0x104, 0xdead_beef] {
            u(&mut v, x);
        }
        f(&mut v, 0.0);
        f(&mut v, 0.0);
        u(&mut v, 1); // materials
        s(&mut v, "skin");
        u(&mut v, 8); // flags: alpha is not transparency
        s(&mut v, "skin.png");
        for x in [0.5, 0.5, 0.5, 0.0, 0.0, 0.0, 0.9, 0.9, 0.9, 0.0, 0.0, 0.0, 0.01, 0.0, 1.0] {
            f(&mut v, x);
        }
        u(&mut v, 0); // header spheres
        u(&mut v, 2); // bones
        for (name, kids) in [("root", &[1u32][..]), ("child", &[][..])] {
            s(&mut v, name);
            f(&mut v, 1.0);
            u(&mut v, kids.len() as u32);
            kids.iter().for_each(|&k| u(&mut v, k));
        }
        u(&mut v, 1); // groups
        s(&mut v, "-noselgroup-");
        u(&mut v, 1); // submeshes
        u(&mut v, 0); // material
        u(&mut v, 3); // vertices: local (0,0,0) (0,1,0) (1,0,0) of bone 1; bind = local + (1,0,0)
        for l in [[0.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]] {
            for p in [l, l, [l[0] + 1.0, l[1], l[2]], [0.0, 0.0, 1.0]] {
                p.iter().for_each(|&c| f(&mut v, c));
            }
            f(&mut v, 0.25);
            f(&mut v, 0.75);
            u(&mut v, 1);
            u(&mut v, 1);
            f(&mut v, 1.0);
        }
        u(&mut v, 3);
        for i in [0u16, 1, 2] {
            v.extend(i.to_le_bytes());
        }
        u(&mut v, 1); // collision spheres
        for x in [0.0, 0.5, 0.0, 0.25] {
            f(&mut v, x);
        }
        u(&mut v, 1);
        u(&mut v, 1); // attractors
        s(&mut v, "Attractor01_head");
        for x in [0.0, 0.1, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0] {
            f(&mut v, x);
        }
        u(&mut v, 1);
        v.extend([0u8; 12]);
        v
    }

    fn find(v: &[u8], pat: &[u8], nth: usize) -> usize {
        v.windows(pat.len()).enumerate().filter(|(_, w)| *w == pat).nth(nth).unwrap().0
    }

    #[test]
    fn parser_rejects_bad_bone_references_and_hierarchies() {
        assert!(CatMesh::parse(&mesh_record()).is_ok());
        let n = mesh_record().len();
        for (what, off) in [("attractor", n - 16), ("collision sphere", n - 76)] {
            let mut v = mesh_record();
            v[off..off + 4].copy_from_slice(&9u32.to_le_bytes());
            let e = format!("{:#}", CatMesh::parse(&v).unwrap_err());
            assert!(e.contains(what) && e.contains("out of range"), "{e}");
        }
        // second `root` is the bone table entry: name(4+4) scale(4) n_child(4) child(4)
        let root = find(&mesh_record(), b"root", 1) + 4 + 4;
        let duplicate = |extra: u32| {
            let mut v = mesh_record();
            v[root..root + 4].copy_from_slice(&2u32.to_le_bytes());
            v.splice(root + 8..root + 8, extra.to_le_bytes());
            v
        };
        assert!(format!("{:#}", CatMesh::parse(&duplicate(1)).unwrap_err()).contains("more than one parent"));
        // a bone that is its own child: the child entry's child count 0 -> 1, child 1
        let mut v = mesh_record();
        let child = find(&v, b"child", 0) + 5 + 4;
        v[child..child + 4].copy_from_slice(&1u32.to_le_bytes());
        v.splice(child + 4..child + 4, 1u32.to_le_bytes());
        let e = format!("{:#}", CatMesh::parse(&v).unwrap_err());
        assert!(e.contains("parent") || e.contains("cycle"), "{e}");
    }

    /// Delta-code `vals` (one column) as big-endian `width`-byte words.
    fn plane(vals: &[u32], width: usize) -> Vec<u8> {
        let mut prev = 0u32;
        let mut raw = vec![];
        for &x in vals {
            raw.extend(&x.wrapping_sub(prev).to_be_bytes()[4 - width..]);
            prev = x;
        }
        let mut z = ZlibEncoder::new(vec![], Compression::default());
        z.write_all(&raw).unwrap();
        let comp = z.finish().unwrap();
        let mut out = vec![];
        u(&mut out, comp.len() as u32);
        u(&mut out, raw.len() as u32);
        out.extend(comp);
        out
    }

    /// `ncols` interleaved columns of `vals`.
    fn stream(vals: &[u32], ncols: usize, bits: u32) -> Vec<u8> {
        (0..ncols).flat_map(|c| plane(&vals.iter().skip(c).step_by(ncols).copied().collect::<Vec<_>>(), bits.div_ceil(8) as usize)).collect()
    }

    /// Two tracks: bone 0 (root) identity; bone 1 translates (0,0,0) -> (2,0,0) over 1000 ms and
    /// turns 0 -> 90 degrees about Z. Rotation 10 bit, translation 8 bit.
    fn anim_record(signature: u32) -> Vec<u8> {
        let mut v = vec![];
        name32(&mut v, "root");
        u(&mut v, 1);
        u(&mut v, 250);
        name32(&mut v, "left");
        for x in [3, 0x100_0106] {
            u(&mut v, x);
        }
        f(&mut v, 1000.0);
        u(&mut v, signature);
        f(&mut v, 0.5);
        u(&mut v, 2);
        v.extend([10i8 as u8, 8]);
        let (t0, t1) = (0f32.to_bits(), 1000f32.to_bits());
        let idx = [0, 0, 2, 1, 1, /* bone 1 */ 1, 0, 2, 2, 2];
        let time = [t0, t0, t0, t1, t0, t1];
        // bone 0 rot key: identity; bone 1: identity then 90 degrees; q = raw - 512
        let rot = [512, 512, 512, 513, 512, 512, 512, 513, 512, 512, 612, 612];
        let range = [0f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 2.0, 0.0, 0.0, 0.0, 0.0].map(f32::to_bits);
        let trans = [0, 0, 0, 0, 0, 0, 255, 0, 0];
        v.extend(stream(&idx, 1, 32));
        v.extend(stream(&time, 1, 32));
        v.extend(stream(&rot, 4, 10));
        v.extend(stream(&range, 1, 32));
        v.extend(stream(&trans, 3, 8));
        v
    }

    #[test]
    fn cat_mesh_fixture_parses() {
        let m = CatMesh::parse(&mesh_record()).unwrap();
        assert_eq!((m.root.as_str(), m.signature), ("root", 0xdead_beef));
        assert_eq!(m.parts, [Part { name: "skin".into(), texture: 7, env_texture: 0, alpha_aux: 0 }]);
        assert_eq!(m.materials[0].flags, 8);
        assert_eq!(m.bones[0].children, [1]);
        assert_eq!(m.parents(), [None, Some(0)]);
        assert_eq!(m.bone_order(), [0, 1]);
        assert_eq!(m.submeshes[0].vertices[2].bind, [2.0, 0.0, 0.0]);
        assert_eq!(m.submeshes[0].indices, [0, 1, 2]);
        assert_eq!((m.col_spheres.len(), m.attractors[0].name.as_str(), m.attractors[0].bone), (1, "Attractor01_head", 1));
    }

    #[test]
    fn cat_mesh_rejects_corruption() {
        let good = mesh_record();
        assert!(CatMesh::parse(&good[..good.len() - 40]).is_err(), "truncated");
        let mut bad = good.clone();
        bad[32..36].copy_from_slice(&1010u32.to_le_bytes());
        assert!(CatMesh::parse(&bad).is_err(), "part marker not a multiple of 1009");
        let mut bad = good.clone();
        let at = good.windows(4).rposition(|w| w == 3u32.to_le_bytes()).unwrap(); // index count
        bad[at + 4..at + 6].copy_from_slice(&9u16.to_le_bytes());
        assert!(CatMesh::parse(&bad).is_err(), "index beyond the vertex list");
    }

    #[test]
    fn cat_anim_fixture_decodes_and_samples() {
        let a = CatAnim::parse(&anim_record(0xdead_beef)).unwrap();
        assert_eq!((a.version, a.duration, a.events.clone()), (0x106, 1000.0, vec![(250, "left".to_string())]));
        assert_eq!(a.tracks.len(), 2);
        let t = &a.tracks[1];
        assert_eq!((t.bone, t.mode, t.rot.len(), t.trans.len()), (1, 2, 2, 2));
        assert!((t.rot[1].1[2] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-3 && (t.rot[1].1[3] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-3);
        assert!((t.trans[1].1[0] - 2.0).abs() < 1e-6 && t.trans[0].1 == [0.0; 3]);
        let (q, p) = a.sample(1, 500.0).unwrap();
        assert!((p[0] - 1.0).abs() < 1e-5);
        assert!((q[2] - (std::f32::consts::PI / 8.0).sin()).abs() < 1e-3, "45 degrees halfway: {q:?}");
        assert!(a.sample(7, 0.0).is_none());
    }

    #[test]
    fn skinning_follows_the_animated_skeleton() {
        let mesh = CatMesh::parse(&mesh_record()).unwrap();
        let anim = CatAnim::parse(&anim_record(mesh.signature)).unwrap();
        // bind: reconstructed frame of the child bone is a pure translation by (1,0,0)
        let frames = bind_frames(&mesh);
        assert!(frames[0].is_none());
        let f1 = frames[1].unwrap();
        assert!(f1.t.iter().zip([1.0, 0.0, 0.0]).all(|(a, b)| (a - b).abs() < 1e-5), "{:?}", f1.t);
        assert!(f1.r.iter().flatten().zip([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]).all(|(a, b)| (a - b).abs() < 1e-5));
        // t = 0: child sits at the root origin with identity rotation -> vertices are the bone-local positions
        let at0 = skin_pose(&mesh, &bone_world(&mesh, Some((&anim, 0.0))));
        assert_eq!(at0[0][1].0, [0.0, 1.0, 0.0]);
        // t = 1000 (looped by the caller to the last key): +2 in x and a quarter turn about Z
        let end = skin_pose(&mesh, &bone_world(&mesh, Some((&anim, 1000.0))));
        let p = end[0][1].0;
        assert!(p.iter().zip([1.0, 0.0, 0.0]).all(|(a, b)| (a - b).abs() < 1e-3), "{p:?}"); // R*(0,1,0)+(2,0,0) = (-1,0,0)+(2,0,0)
        let n = end[0][1].1; // local normal +Z is the rotation axis
        assert!((n[2] - 1.0).abs() < 1e-5);
    }

    #[test]
    fn material_blend_follows_flags_and_alpha() {
        let mut m = CatMesh::parse(&mesh_record()).unwrap().materials.remove(0);
        let key = TextureKey { rdb_type: TEXTURE_TYPE, id: 1 };
        let tex = |alphas: &[u8]| ao_scene::Texture { width: alphas.len() as u32, height: 1, rgba: alphas.iter().flat_map(|&a| [9, 9, 9, a]).collect() };
        let blend = |m: &Material, t: &ao_scene::Texture| submesh_for(m, Some((key, t))).blend;
        assert_eq!(blend(&m, &tex(&[0, 255])), Blend::Opaque, "flags & 8: alpha is not transparency");
        m.flags = 0;
        assert_eq!(blend(&m, &tex(&[0, 255])), Blend::AlphaTest);
        assert_eq!(blend(&m, &tex(&[0, 128])), Blend::AlphaBlend);
        assert_eq!(blend(&m, &tex(&[255])), Blend::Opaque);
        m.opacity = 0.5;
        assert_eq!(blend(&m, &tex(&[255])), Blend::AlphaBlend);
        assert_eq!(submesh_for(&m, None).base_color[3], 0.5);
    }
}
