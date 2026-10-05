//! Static mesh records (rdb types 1010001 and 1010026), see `docs/formats.md` § meshes.
//!
//! A record is a Funcom `ObjectArchive` ([`crate::archive`]) holding a tree of `RRefFrame_t`-derived
//! nodes. Nodes carrying a `data` reference own an `FAFTriMeshData_t` whose `SimpleMesh`es hold the
//! vertex/index buffers. Everything is baked into one [`Mesh`] in AO's left-handed D3D space
//! converted to ao-scene's right-handed space by negating Z (and reversing triangle winding).

use crate::archive::{Archive, Object};
use crate::texture::load_texture;
use anyhow::{bail, ensure, Context, Result};
use ao_rdb::RecordStore;
use ao_scene::{Blend, Instance, Mesh, Scene, Submesh, Texture, TextureKey, Vertex, IDENTITY};
use std::collections::HashMap;

/// Primary static-mesh record type (full detail).
pub const MESH_TYPE: u32 = 1010001;
/// Second mesh record type sharing the ids of a subset of [`MESH_TYPE`] with fewer triangles.
pub const MESH_LOW_TYPE: u32 = 1010026;

// D3D7 render-state ids (the engine is D3D7: `_D3DMATERIAL7`, `_D3DRENDERSTATETYPE`).
const D3DRS_SRCBLEND: i32 = 19;
const D3DRS_DESTBLEND: i32 = 20;
const D3DRS_CULLMODE: i32 = 22;
const D3DRS_ALPHATESTENABLE: i32 = 15;
const D3DRS_ALPHABLENDENABLE: i32 = 27;
const D3DTSS_COLOROP: i32 = 1;
const D3DTSS_COLORARG1: i32 = 2;
const D3DTOP_ADD: i32 = 7;
const D3DTA_ALPHAREPLICATE: i32 = 0x20;
const D3DBLEND_ONE: i32 = 2;
const D3DBLEND_SRCALPHA: i32 = 5;
const D3DCULL_NONE: i32 = 1;
const MAX_DEPTH: usize = 64;

/// Decodes static mesh record `id` (with its textures) into a scene with one instance at the origin.
pub fn load_mesh(store: &RecordStore, id: u32) -> Result<Scene> {
    let mut scene = Scene::default();
    let mesh = decode_mesh_into(store, id, &mut scene)?.with_context(|| format!("no mesh record {id}"))?;
    scene.instances.push(Instance { mesh, transform: IDENTITY });
    Ok(scene)
}

/// Appends mesh `id` (type [`MESH_TYPE`]) and its textures (deduped by [`TextureKey`]) to `scene`;
/// returns its index in `scene.meshes`, or `None` if the record does not exist.
pub fn decode_mesh_into(store: &RecordStore, id: u32, scene: &mut Scene) -> Result<Option<usize>> {
    decode_record_into(store, MESH_TYPE, id, scene)
}

/// Like [`decode_mesh_into`] for an explicit mesh record type ([`MESH_TYPE`] or [`MESH_LOW_TYPE`]).
pub fn decode_record_into(store: &RecordStore, rdb_type: u32, id: u32, scene: &mut Scene) -> Result<Option<usize>> {
    decode_record(store, rdb_type, id, scene, false, &[])
}

/// Texture used by statel attribute overrides: `NewTextureData_t` ids are 1010004 records (`acg_tiles_metal_corroded_plain.png`, ...).
const OVERRIDE_TEXTURES: u32 = 1_010_004;

/// A statel's mesh (`rdb_type` = [`MESH_TYPE`] or the reduced [`MESH_LOW_TYPE`]) with its attributes applied: `(slot, texture id)` pairs replace the texture of the `slot`-th
/// `SimpleMesh` (counted over the whole node tree in traversal order, empty ones included). Statel attributes are
/// `std::vector<NewTextureData_t>` (`{i32 slot, u32 texture}`, 8 bytes) handed to `VisualMesh_t::SetMesh` (DisplaySystem
/// @0x1006b623) -> `AsyncMesh` (@0x1007125c) -> `FUN_100714c2`; see `docs/formats.md` § playfields.
pub fn decode_statel_mesh(store: &RecordStore, rdb_type: u32, id: u32, overrides: &[(u8, u32)], scene: &mut Scene) -> Result<Option<usize>> {
    decode_record(store, rdb_type, id, scene, false, overrides)
}

/// Like [`decode_mesh_into`] but with the vertices exactly as stored (node object space, the frame tree's matrices are
/// not applied): what a `RTriMesh_t` visual built straight from the mesh data draws (the sky objects).
pub fn decode_mesh_object_space(store: &RecordStore, id: u32, scene: &mut Scene) -> Result<Option<usize>> {
    decode_record(store, MESH_TYPE, id, scene, true, &[])
}

fn decode_record(store: &RecordStore, rdb_type: u32, id: u32, scene: &mut Scene, object_space: bool, overrides: &[(u8, u32)]) -> Result<Option<usize>> {
    let Some(bytes) = store.get(rdb_type, id)? else { return Ok(None) };
    let mesh = decode_archive_with(&bytes, object_space, overrides, |key| {
        if let std::collections::hash_map::Entry::Vacant(e) = scene.textures.entry(key) {
            if let Some(t) = load_texture(store, key).ok().flatten() {
                e.insert(t);
            }
        }
        scene.textures.contains_key(&key)
    })
    .with_context(|| format!("decoding mesh {rdb_type}/{id}"))?;
    let mut mesh = mesh;
    for sub in &mut mesh.submeshes {
        if sub.blend == Blend::AlphaBlend && sub.base_color[3] >= 1.0 && sub.texture.and_then(|k| scene.textures.get(&k)).is_some_and(is_cutout) {
            sub.blend = Blend::AlphaTest;
        }
    }
    scene.meshes.push(mesh);
    Ok(Some(scene.meshes.len() - 1))
}

/// Every texture a mesh record references (before the load check, in first-use order, deduped); `None` = no record.
pub fn texture_refs(store: &RecordStore, rdb_type: u32, id: u32) -> Result<Option<Vec<TextureKey>>> {
    let Some(bytes) = store.get(rdb_type, id)? else { return Ok(None) };
    let mut refs = Vec::new();
    decode_archive(&bytes, false, |k| {
        if !refs.contains(&k) {
            refs.push(k);
        }
        false
    })
    .with_context(|| format!("decoding mesh {rdb_type}/{id}"))?;
    Ok(Some(refs))
}

/// Fraction of texels with a partial alpha (16..240) below which a blended texture is treated as a
/// cutout. Guess: the engine blends these (rst 27=1, z-write off, sorted), but a texture whose alpha is
/// essentially 0/255 looks the same alpha-tested while avoiding per-instance depth sorting.
const CUTOUT_PARTIAL_MAX: f32 = 0.02;

/// True for a texture whose alpha is (almost) only 0 or 255.
fn is_cutout(t: &Texture) -> bool {
    let partial = t.rgba.as_chunks::<4>().0.iter().filter(|p| (16..240).contains(&p[3])).count();
    (partial as f32) <= CUTOUT_PARTIAL_MAX * (t.rgba.len() / 4) as f32
}

/// Row-major 4x4 for row vectors (`v' = v * M`), as used by the original engine.
type Mat = [[f32; 4]; 4];
const IDENTITY_MAT: Mat = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]];

fn mul(a: &Mat, b: &Mat) -> Mat {
    let mut o = [[0.0; 4]; 4];
    for i in 0..4 {
        for j in 0..4 {
            o[i][j] = (0..4).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    o
}

/// `RRefFrame_t::UpdateWorldMatrix` local part: quaternion rows, uniform scale, translation, then
/// `anim_matrix * local` when an animation matrix is present.
fn local_matrix(n: &Object) -> Mat {
    let [x, y, z, w] = n.f32s::<4>("local_rot").unwrap_or([0.0, 0.0, 0.0, 1.0]);
    let s = n.f32s::<1>("scale").map_or(1.0, |s| s[0]);
    let p = n.f32s::<3>("local_pos").unwrap_or([0.0; 3]);
    let mut m = [
        [1.0 - 2.0 * (y * y + z * z), 2.0 * (z * w + x * y), 2.0 * (x * z - w * y), 0.0],
        [2.0 * (x * y - z * w), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z + w * x), 0.0],
        [2.0 * (w * y + x * z), 2.0 * (y * z - w * x), 1.0 - 2.0 * (x * x + y * y), 0.0],
        [p[0], p[1], p[2], 1.0],
    ];
    for row in m.iter_mut().take(3) {
        for v in row.iter_mut().take(3) {
            *v *= s;
        }
    }
    match n.f32s::<16>("anim_matrix") {
        Some(a) => mul(&[[a[0], a[1], a[2], a[3]], [a[4], a[5], a[6], a[7]], [a[8], a[9], a[10], a[11]], [a[12], a[13], a[14], a[15]]], &m),
        None => m,
    }
}

fn det3(m: &Mat) -> f32 {
    m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
}

/// Everything that makes two `SimpleMesh`es mergeable into one [`Submesh`].
#[derive(Clone, Copy)]
struct MatKey {
    texture: Option<TextureKey>,
    blend: Blend,
    two_sided: bool,
    color: [f32; 4],
    emissive: [f32; 3],
    glow_mask: bool,
}

type MatId = (Option<TextureKey>, u8, bool, [u32; 4], [u32; 3], bool);

impl MatKey {
    /// Hashable identity (`Blend` and `f32` are not `Hash`).
    fn id(&self) -> MatId {
        (self.texture, self.blend as u8, self.two_sided, self.color.map(f32::to_bits), self.emissive.map(f32::to_bits), self.glow_mask)
    }
}

struct Builder<'a, 'b> {
    ar: &'b Archive<'a>,
    mesh: Mesh,
    /// material -> index into `mesh.submeshes`
    groups: HashMap<MatId, usize>,
    have_texture: &'b mut dyn FnMut(TextureKey) -> bool,
    visited: Vec<bool>,
    /// Ignore every node matrix (vertices as stored).
    object_space: bool,
    /// Statel texture overrides `(SimpleMesh slot, texture id)` and the running slot counter.
    overrides: &'b [(u8, u32)],
    slot: usize,
}

fn decode_archive(bytes: &[u8], object_space: bool, have_texture: impl FnMut(TextureKey) -> bool) -> Result<Mesh> {
    decode_archive_with(bytes, object_space, &[], have_texture)
}

fn decode_archive_with(bytes: &[u8], object_space: bool, overrides: &[(u8, u32)], mut have_texture: impl FnMut(TextureKey) -> bool) -> Result<Mesh> {
    let ar = Archive::parse(bytes)?;
    let mut b = Builder {
        ar: &ar,
        mesh: Mesh::default(),
        groups: HashMap::new(),
        have_texture: &mut have_texture,
        visited: vec![false; ar.objects.len()],
        object_space,
        overrides,
        slot: 0,
    };
    b.node(ar.root, &IDENTITY, 0)?;
    b.mesh.submeshes.retain(|s| !s.indices.is_empty());
    Ok(b.mesh)
}

impl Builder<'_, '_> {
    fn node(&mut self, i: usize, parent: &Mat, depth: usize) -> Result<()> {
        ensure!(depth < MAX_DEPTH, "frame tree too deep");
        let ar = self.ar;
        let n = ar.objects.get(i).with_context(|| format!("dangling object ref {i}"))?;
        ensure!(!std::mem::replace(&mut self.visited[i], true), "cyclic frame tree");
        let world = if self.object_space { IDENTITY_MAT } else { mul(&local_matrix(n), parent) };
        if let Some(data) = n.ref1("data") {
            let data = ar.objects.get(data).context("dangling data ref")?;
            let node_ds = n.ref1("delta_state").and_then(|d| ar.objects.get(d));
            for m in data.refs("mesh") {
                self.simple_mesh(node_ds, ar.objects.get(m).context("dangling mesh ref")?, &world)?;
            }
        }
        for c in n.refs("chld") {
            self.node(c, &world, depth + 1)?;
        }
        Ok(())
    }

    /// Material of a `SimpleMesh`. Render states and texture channels come from the node's own
    /// `delta_state` (e.g. the "alpha" states of foliage nodes) and the material's `delta_state`
    /// (applied later by `RViewPort_t::SetMaterial`, so it wins); a texture on the node's state wins
    /// over the material's. Colours come from `FAFMaterial_t` (`RMaterial_t::InitD3DMaterial` @100409c6).
    fn material(&mut self, node_ds: Option<&Object>, sm: &Object) -> MatKey {
        let objs = &self.ar.objects;
        let mat = sm.ref1("material").and_then(|m| objs.get(m));
        let mat_ds = mat.and_then(|m| m.ref1("delta_state")).and_then(|d| objs.get(d));
        let state = |ty: i32| {
            [mat_ds, node_ds].into_iter().flatten().find_map(|ds| ds.all("rst_type").zip(ds.all("rst_value")).filter(|&(t, _)| le(t) == Some(ty)).last().and_then(|(_, v)| le(v)))
        };
        let mut tex = None;
        for ds in [node_ds, mat_ds].into_iter().flatten() {
            // channel 0 = diffuse; fall back to the first bound channel
            let chans: Vec<_> = ds.all("tch_type").map(le).zip(ds.refs("tch_text")).collect();
            let found = chans.iter().find(|(t, _)| *t == Some(0)).or(chans.first()).and_then(|&(_, t)| {
                let creator = objs.get(t)?.ref1("creator").and_then(|c| objs.get(c))?;
                Some(TextureKey { rdb_type: creator.i32("type")? as u32, id: creator.i32("inst")? as u32 })
            });
            if tex.is_none() {
                tex = found;
            }
        }
        let tex = tex.filter(|&k| (self.have_texture)(k));
        // Alpha test wins over blend when both are on (mode 2 of @10040645 sets both; the cutout is
        // what survives without depth sorting). Blend factors ONE/ONE (or SRCALPHA/ONE) = additive.
        let blend = if state(D3DRS_ALPHATESTENABLE) == Some(1) {
            Blend::AlphaTest
        } else if state(D3DRS_ALPHABLENDENABLE) == Some(1) {
            let src = state(D3DRS_SRCBLEND);
            if state(D3DRS_DESTBLEND) == Some(D3DBLEND_ONE) && matches!(src, Some(D3DBLEND_ONE | D3DBLEND_SRCALPHA)) {
                Blend::Additive
            } else {
                Blend::AlphaBlend
            }
        } else {
            Blend::Opaque
        };
        // D3D multiplies gamma-space values; the renderer multiplies sRGB-decoded textures in linear
        // space, so the diffuse colour is converted to linear (power curve => identical result).
        let lin = |c: f32| if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) };
        let [r, g, b] = mat.and_then(|m| m.f32s::<3>("diff")).unwrap_or([1.0; 3]);
        let a = mat.and_then(|m| m.f32s::<1>("opac")).map_or(1.0, |o| o[0]);
        // `FUN_1004ac5f` @1004ac5f: EMISSIVEMATERIALSOURCE (148) = MATERIAL only when the emissive
        // luminance (0.299R+0.587G+0.114B, `FUN_1004a8c0`) exceeds 0; otherwise it reads the (absent) vertex colour.
        let emis = mat.and_then(|m| m.f32s::<3>("emis")).unwrap_or([0.0; 3]);
        let emissive = if 0.299 * emis[0] + 0.587 * emis[1] + 0.114 * emis[2] > 0.0 { emis.map(lin) } else { [0.0; 3] };
        // Mode 5 of `FUN_10040645` (texture-alpha glow): stage 0 COLOROP = ADD, COLORARG1 = TEXTURE|ALPHAREPLICATE.
        let stage0 = |ty: i32| [mat_ds, node_ds].into_iter().flatten().find_map(|ds| tss(ds, 0, ty));
        let glow_mask = blend == Blend::Opaque
            && tex.is_some()
            && stage0(D3DTSS_COLOROP) == Some(D3DTOP_ADD)
            && stage0(D3DTSS_COLORARG1).is_some_and(|v| v & D3DTA_ALPHAREPLICATE != 0);
        MatKey { texture: tex, blend, two_sided: state(D3DRS_CULLMODE) == Some(D3DCULL_NONE), color: [lin(r), lin(g), lin(b), a], emissive, glow_mask }
    }

    fn simple_mesh(&mut self, node_ds: Option<&Object>, sm: &Object, world: &Mat) -> Result<()> {
        let desc = sm.get("vb_desc").context("SimpleMesh without vb_desc")?;
        ensure!(desc.len() >= 16, "short vb_desc");
        let fvf = u32::from_le_bytes(desc[8..12].try_into().unwrap());
        let count = u32::from_le_bytes(desc[12..16].try_into().unwrap()) as usize;
        let verts = sm.blob("vertices").context("SimpleMesh without vertices")?;
        let layout = FvfLayout::new(fvf)?;
        ensure!(verts.len() == count * layout.stride, "vertex blob {} != {count} x {}", verts.len(), layout.stride);
        let tris = sm.ref1("trilist").and_then(|t| self.ar.objects.get(t)).and_then(|t| t.blob("triangles")).context("SimpleMesh without triangle list")?;
        ensure!(tris.len() % 6 == 0, "triangle blob not a multiple of 6 bytes");

        let base = self.mesh.vertices.len() as u32;
        let f = |b: &[u8], o: usize| f32::from_le_bytes(b[o..o + 4].try_into().unwrap());
        for v in verts.chunks_exact(layout.stride) {
            let p = [f(v, 0), f(v, 4), f(v, 8)];
            let n = layout.normal.map_or([0.0, 1.0, 0.0], |o| [f(v, o), f(v, o + 4), f(v, o + 8)]);
            let uv = layout.uv.map_or([0.0, 0.0], |o| [f(v, o), f(v, o + 4)]);
            let xf = |a: [f32; 3], t: f32| -> [f32; 3] { std::array::from_fn(|j| a[0] * world[0][j] + a[1] * world[1][j] + a[2] * world[2][j] + t * world[3][j]) };
            let (mut pos, mut nrm) = (xf(p, 1.0), xf(n, 0.0));
            let len = nrm.iter().map(|c| c * c).sum::<f32>().sqrt();
            if len > 1e-12 {
                nrm.iter_mut().for_each(|c| *c /= len);
            }
            pos[2] = -pos[2];
            nrm[2] = -nrm[2];
            self.mesh.vertices.push(Vertex { pos, normal: nrm, uv, ..Default::default() });
        }

        // The Z flip mirrors the mesh; a node matrix with negative determinant mirrors it back.
        let reverse = det3(world) >= 0.0;
        let mut key = self.material(node_ds, sm);
        let slot = self.slot;
        self.slot += 1;
        if let Some(&(_, id)) = self.overrides.iter().find(|o| o.0 as usize == slot) {
            let tex = TextureKey { rdb_type: OVERRIDE_TEXTURES, id };
            if (self.have_texture)(tex) {
                key.texture = Some(tex);
            }
        }
        let sub = *self.groups.entry(key.id()).or_insert_with(|| {
            self.mesh.submeshes.push(Submesh { blend: key.blend, two_sided: key.two_sided, base_color: key.color, emissive: key.emissive, glow_mask: key.glow_mask, ..Submesh::new(vec![], key.texture) });
            self.mesh.submeshes.len() - 1
        });
        for t in tris.as_chunks::<6>().0 {
            let ix: [u32; 3] = std::array::from_fn(|k| u16::from_le_bytes([t[2 * k], t[2 * k + 1]]) as u32);
            if ix.iter().any(|&i| i as usize >= count) {
                bail!("triangle index out of range (vertex count {count})");
            }
            let [a, b, c] = ix.map(|i| base + i);
            self.mesh.submeshes[sub].indices.extend(if reverse { [a, c, b] } else { [a, b, c] });
        }
        Ok(())
    }
}

/// Texture-stage state `ty` of `stage` in a `RDeltaState` (`tstm_count` entries per stage, then
/// `tst_type`/`tst_value` pairs, each member holding all elements; last write wins).
fn tss(ds: &Object, stage: usize, ty: i32) -> Option<i32> {
    let ints = |n: &str| ds.all(n).flat_map(|d| d.as_chunks::<4>().0.iter()).map(|c| i32::from_le_bytes(*c)).collect::<Vec<_>>();
    let (counts, types, values) = (ints("tstm_count"), ints("tst_type"), ints("tst_value"));
    let first: usize = counts.get(..stage)?.iter().map(|&c| c as usize).sum();
    let n = *counts.get(stage)? as usize;
    types.get(first..first + n)?.iter().zip(values.get(first..first + n)?).rfind(|(t, _)| **t == ty).map(|(_, v)| *v)
}

fn le(d: &[u8]) -> Option<i32> {
    d.get(..4).map(|b| i32::from_le_bytes(b.try_into().unwrap()))
}

/// Byte offsets inside one D3D9 FVF vertex (the data uses 0x112 = XYZ | NORMAL | TEX1 everywhere).
struct FvfLayout {
    stride: usize,
    normal: Option<usize>,
    uv: Option<usize>,
}

impl FvfLayout {
    fn new(fvf: u32) -> Result<Self> {
        ensure!(fvf & 0xe == 0x2, "unsupported FVF {fvf:#x} (position must be XYZ)");
        let mut o = 12;
        let normal = (fvf & 0x10 != 0).then_some(o);
        if normal.is_some() {
            o += 12;
        }
        o += 4 * ((fvf >> 6) & 1) as usize + 4 * ((fvf >> 7) & 1) as usize; // diffuse, specular
        let tex = ((fvf >> 8) & 0xf) as usize;
        let uv = (tex > 0).then_some(o);
        Ok(FvfLayout { stride: o + 8 * tex, normal, uv })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fvf_0x112_is_32_bytes() {
        let l = FvfLayout::new(0x112).unwrap();
        assert_eq!((l.stride, l.normal, l.uv), (32, Some(12), Some(24)));
    }

    #[test]
    fn identity_node_matrix() {
        let m = local_matrix(&Object { members: vec![] });
        assert_eq!(m, IDENTITY);
    }

    #[test]
    fn quat_90_about_x_rotates_y_to_z() {
        let q: Vec<u8> = [std::f32::consts::FRAC_1_SQRT_2, 0.0, 0.0, std::f32::consts::FRAC_1_SQRT_2].iter().flat_map(|f| f.to_le_bytes()).collect();
        let n = Object { members: vec![crate::archive::Member { name: "local_rot", data: &q }] };
        let m = local_matrix(&n);
        // row-vector convention: (0,1,0) * M = row 1 = (0,0,1)
        assert!((m[1][2] - 1.0).abs() < 1e-5 && m[1][1].abs() < 1e-5);
    }

    fn member(idx: u8, ty: u32, data: &[u8]) -> Vec<u8> {
        let mut v = vec![idx];
        for x in [ty, 0, data.len() as u32] {
            v.extend(x.to_le_bytes());
        }
        v.extend(data);
        v
    }
    fn object(members: &[Vec<u8>]) -> Vec<u8> {
        let mut v = vec![];
        for x in [1u32, 1, members.len() as u32] {
            v.extend(x.to_le_bytes());
        }
        members.concat().into_iter().for_each(|b| v.push(b));
        v
    }
    fn blob(b: &[u8]) -> Vec<u8> {
        [&(b.len() as u32).to_le_bytes()[..], b].concat()
    }

    /// One quad as stored in record 1010001/214381 (Box02 ground plane: x/y in +-25, normal -Z,
    /// rotated to +Y by its anim_matrix), no material.
    #[test]
    fn decodes_quad_node_with_anim_matrix() {
        const NAMES: [&str; 8] = ["obj", "anim_matrix", "data", "mesh", "vb_desc", "vertices", "trilist", "triangles"];
        let mut b = vec![];
        for x in [3u32, 0, 0, 1, NAMES.len() as u32] {
            b.extend(x.to_le_bytes());
        }
        for n in NAMES {
            b.extend(format!("1\0{n}\0").bytes());
        }
        b.extend(4u32.to_le_bytes()); // object count
        let fl = |v: &[f32]| v.iter().flat_map(|f| f.to_le_bytes()).collect::<Vec<u8>>();
        let r = |v: &[i32]| v.iter().flat_map(|f| f.to_le_bytes()).collect::<Vec<u8>>();
        b.extend(object(&[member(0, 0x11, &r(&[0]))]));
        // Rx(+90deg) in row-vector form: y -> z, z -> -y, plus translation (1, 2, 3)
        let am = fl(&[1., 0., 0., 0., 0., 0., 1., 0., 0., -1., 0., 0., 1., 2., 3., 1.]);
        b.extend(object(&[member(1, 0xf, &am), member(2, 0x11, &r(&[1]))]));
        b.extend(object(&[member(3, 0x11, &r(&[2]))]));
        let quad = [(-25., 25.), (25., 25.), (-25., -25.), (25., -25.)];
        let vb: Vec<f32> = quad.iter().flat_map(|&(x, y)| [x, y, 0., 0., 0., -1., 0.5, 0.5]).collect();
        let mut desc = vec![];
        for x in [16u32, 65536, 0x112, 4] {
            desc.extend(x.to_le_bytes());
        }
        b.extend(object(&[member(4, 9, &desc), member(5, 9, &blob(&fl(&vb))), member(6, 0x11, &r(&[3]))]));
        let tris: Vec<u8> = [0u16, 1, 2, 3, 2, 1].iter().flat_map(|i| i.to_le_bytes()).collect();
        b.extend(object(&[member(7, 9, &blob(&tris))]));
        b.extend([0u8; 12]);

        let m = decode_archive(&b, false, |_| false).unwrap();
        assert_eq!(m.vertices.len(), 4);
        // (-25, 25, 0) * M + t = (-24, 2, 28); Z negated for the right-handed output -> -28
        assert_eq!(m.vertices[0].pos, [-24.0, 2.0, -28.0]);
        // normal (0,0,-1) -> (0, 1, 0) after Rx, then Z flip leaves Y untouched
        let n = m.vertices[0].normal;
        assert!((n[1] - 1.0).abs() < 1e-6 && n[0].abs() < 1e-6 && n[2].abs() < 1e-6);
        // winding reversed by the handedness flip
        assert_eq!(m.submeshes.len(), 1);
        assert_eq!(m.submeshes[0].indices, vec![0, 2, 1, 3, 1, 2]);
        assert!(m.submeshes[0].texture.is_none());
    }

    /// Two untextured `SimpleMesh`es (one triangle each) that merge into one submesh; a statel texture override on
    /// slot 1 must split the second one off with the override texture (`NewTextureData_t`, see `decode_statel_mesh`).
    #[test]
    fn statel_override_replaces_the_texture_of_one_slot() {
        const NAMES: [&str; 7] = ["obj", "data", "mesh", "vb_desc", "vertices", "trilist", "triangles"];
        let mut b = vec![];
        for x in [3u32, 0, 0, 1, NAMES.len() as u32] {
            b.extend(x.to_le_bytes());
        }
        for n in NAMES {
            b.extend(format!("1\0{n}\0").bytes());
        }
        b.extend(6u32.to_le_bytes());
        let fl = |v: &[f32]| v.iter().flat_map(|f| f.to_le_bytes()).collect::<Vec<u8>>();
        let r = |v: &[i32]| v.iter().flat_map(|f| f.to_le_bytes()).collect::<Vec<u8>>();
        b.extend(object(&[member(0, 0x11, &r(&[0]))]));
        b.extend(object(&[member(1, 0x11, &r(&[1]))]));
        b.extend(object(&[member(2, 0x11, &r(&[2, 3]))]));
        let vb = fl(&[0., 0., 0., 0., 1., 0., 0., 0., 1., 0., 0., 0., 1., 0., 1., 0., 0., 1., 0., 1., 0., 0., 1., 1.]);
        let mut desc = vec![];
        for x in [16u32, 65536, 0x112, 3] {
            desc.extend(x.to_le_bytes());
        }
        for tri in [4, 5] {
            b.extend(object(&[member(3, 9, &desc), member(4, 9, &blob(&vb)), member(5, 0x11, &r(&[tri]))]));
        }
        let tris: Vec<u8> = [0u16, 1, 2].iter().flat_map(|i| i.to_le_bytes()).collect();
        for _ in 0..2 {
            b.extend(object(&[member(6, 9, &blob(&tris))]));
        }
        b.extend([0u8; 12]);

        assert_eq!(decode_archive(&b, false, |_| true).unwrap().submeshes.len(), 1);
        let key = TextureKey { rdb_type: OVERRIDE_TEXTURES, id: 77 };
        let m = decode_archive_with(&b, false, &[(1, 77)], |k| k == key).unwrap();
        assert_eq!(m.submeshes.len(), 2);
        assert_eq!((m.submeshes[0].texture, m.submeshes[1].texture), (None, Some(key)));
        // an override whose texture does not exist leaves the slot as authored
        assert_eq!(decode_archive_with(&b, false, &[(1, 77)], |_| false).unwrap().submeshes.len(), 1);
    }

    #[test]
    fn out_of_range_child_ref_is_an_error() {
        let mut b = vec![];
        for x in [3u32, 0, 0, 1, 2] {
            b.extend(x.to_le_bytes());
        }
        for n in ["obj", "chld"] {
            b.extend(format!("1\0{n}\0").bytes());
        }
        b.extend(1u32.to_le_bytes());
        let r = |v: &[i32]| v.iter().flat_map(|f| f.to_le_bytes()).collect::<Vec<u8>>();
        b.extend(object(&[member(0, 0x11, &r(&[0]))]));
        b.extend(object(&[member(1, 0x11, &r(&[0x4d00_0000]))]));
        b.extend([0u8; 12]);
        assert!(decode_archive(&b, false, |_| false).is_err());
    }

    /// One triangle with a `FAFMaterial_t` (diffuse, opacity) and a material `RDeltaState` holding `states`.
    fn material_mesh(diff: [f32; 3], opac: f32, states: &[(i32, i32)]) -> Submesh {
        material_mesh_emis(diff, [0.0; 3], opac, states)
    }

    fn material_mesh_emis(diff: [f32; 3], emis: [f32; 3], opac: f32, states: &[(i32, i32)]) -> Submesh {
        const NAMES: [&str; 14] = ["obj", "data", "mesh", "vb_desc", "vertices", "trilist", "triangles", "material", "delta_state", "diff", "opac", "rst_type", "rst_value", "emis"];
        let mut b = vec![];
        for x in [3u32, 0, 0, 1, NAMES.len() as u32] {
            b.extend(x.to_le_bytes());
        }
        for n in NAMES {
            b.extend(format!("1\0{n}\0").bytes());
        }
        b.extend(6u32.to_le_bytes());
        let fl = |v: &[f32]| v.iter().flat_map(|f| f.to_le_bytes()).collect::<Vec<u8>>();
        let r = |v: &[i32]| v.iter().flat_map(|f| f.to_le_bytes()).collect::<Vec<u8>>();
        b.extend(object(&[member(0, 0x11, &r(&[0]))])); // holder -> root 0 (the node)
        b.extend(object(&[member(1, 0x11, &r(&[1]))])); // node: data -> 1
        b.extend(object(&[member(2, 0x11, &r(&[2]))])); // data: mesh -> 2
        let vb = fl(&[0., 0., 0., 0., 1., 0., 0., 0., 1., 0., 0., 0., 1., 0., 0., 1., 0., 1., 0., 0., 1., 0., 0., 1.]);
        let mut desc = vec![];
        for x in [16u32, 65536, 0x112, 3] {
            desc.extend(x.to_le_bytes());
        }
        b.extend(object(&[member(3, 9, &desc), member(4, 9, &blob(&vb)), member(5, 0x11, &r(&[3])), member(7, 0x11, &r(&[4]))]));
        let tris: Vec<u8> = [0u16, 1, 2].iter().flat_map(|i| i.to_le_bytes()).collect();
        b.extend(object(&[member(6, 9, &blob(&tris))]));
        b.extend(object(&[member(8, 0x11, &r(&[5])), member(9, 0x10, &fl(&diff)), member(10, 0xa, &fl(&[opac])), member(13, 0x10, &fl(&emis))]));
        let mut ds = vec![];
        for &(t, v) in states {
            ds.push(member(11, 3, &r(&[t])));
            ds.push(member(12, 3, &r(&[v])));
        }
        b.extend(object(&ds));
        b.extend([0u8; 12]);
        let mut m = decode_archive(&b, false, |_| false).unwrap();
        assert_eq!(m.submeshes.len(), 1);
        m.submeshes.remove(0)
    }

    #[test]
    fn flat_colour_is_linear_base_color_and_default_is_opaque_culled() {
        let s = material_mesh([0.5, 0.0, 1.0], 0.25, &[]);
        assert_eq!((s.blend, s.two_sided, s.texture), (Blend::Opaque, false, None));
        assert!((s.base_color[0] - 0.2140).abs() < 1e-3 && s.base_color[1] == 0.0 && s.base_color[2] == 1.0 && s.base_color[3] == 0.25);
    }

    #[test]
    fn emissive_is_linear_and_only_kept_when_luminous() {
        let s = material_mesh_emis([1.0; 3], [0.5, 0.0, 1.0], 1.0, &[]);
        assert!((s.emissive[0] - 0.2140).abs() < 1e-3 && s.emissive[1] == 0.0 && s.emissive[2] == 1.0);
        assert_eq!(material_mesh([1.0; 3], 1.0, &[]).emissive, [0.0; 3]);
        assert!(!s.glow_mask);
    }

    #[test]
    fn texture_stage_states_are_read_per_stage() {
        // mode 5 of @10040645: stage 0 {ARG1 = TEXTURE|ALPHAREPLICATE, OP = ADD, ARG2 = DIFFUSE, ALPHAOP = DISABLE}, stage 1 {ARG1 = TEXTURE, OP = MODULATE, ARG2 = CURRENT}
        let i = |v: &[i32]| v.iter().flat_map(|f| f.to_le_bytes()).collect::<Vec<u8>>();
        let m = |name, d: &[u8]| crate::archive::Member { name, data: d.to_vec().leak() };
        let ds = Object { members: vec![m("tstm_count", &i(&[4])), m("tst_type", &i(&[2, 1, 3, 4])), m("tst_value", &i(&[0x22, 7, 0, 1])), m("tstm_count", &i(&[3])), m("tst_type", &i(&[2, 1, 3])), m("tst_value", &i(&[2, 4, 1]))] };
        assert_eq!((tss(&ds, 0, D3DTSS_COLORARG1), tss(&ds, 0, D3DTSS_COLOROP)), (Some(0x22), Some(D3DTOP_ADD)));
        assert_eq!((tss(&ds, 1, D3DTSS_COLORARG1), tss(&ds, 1, D3DTSS_COLOROP)), (Some(2), Some(4)));
        assert_eq!(tss(&ds, 2, D3DTSS_COLOROP), None);
    }

    #[test]
    fn render_states_select_blend_and_cull() {
        let blend = |st: &[(i32, i32)]| material_mesh([1.0; 3], 1.0, st);
        assert_eq!(blend(&[(D3DRS_ALPHABLENDENABLE, 1)]).blend, Blend::AlphaBlend);
        assert_eq!(blend(&[(D3DRS_ALPHABLENDENABLE, 0)]).blend, Blend::Opaque);
        assert_eq!(blend(&[(D3DRS_ALPHATESTENABLE, 1)]).blend, Blend::AlphaTest);
        // alpha test wins over blend (mode 2 of RMaterial_t's state preset sets both)
        assert_eq!(blend(&[(D3DRS_ALPHATESTENABLE, 1), (D3DRS_ALPHABLENDENABLE, 1)]).blend, Blend::AlphaTest);
        // ONE/ONE and SRCALPHA/ONE are additive (RSprite::EnableAdditiveRendering @10013128)
        assert_eq!(blend(&[(D3DRS_ALPHABLENDENABLE, 1), (D3DRS_SRCBLEND, 2), (D3DRS_DESTBLEND, 2)]).blend, Blend::Additive);
        assert_eq!(blend(&[(D3DRS_ALPHABLENDENABLE, 1), (D3DRS_SRCBLEND, 5), (D3DRS_DESTBLEND, 2)]).blend, Blend::Additive);
        assert!(blend(&[(D3DRS_CULLMODE, D3DCULL_NONE)]).two_sided);
        assert!(!blend(&[(D3DRS_CULLMODE, 3)]).two_sided);
    }
}
