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
use ao_scene::{Blend, Instance, Mesh, Scene, Submesh, TextureKey, Vertex, IDENTITY};
use std::collections::HashMap;

/// Primary static-mesh record type (full detail).
pub const MESH_TYPE: u32 = 1010001;
/// Second mesh record type sharing the ids of a subset of [`MESH_TYPE`] with fewer triangles.
pub const MESH_LOW_TYPE: u32 = 1010026;

const D3DRS_ALPHATESTENABLE: i32 = 15;
const D3DRS_ALPHABLENDENABLE: i32 = 27;
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
    let Some(bytes) = store.get(rdb_type, id)? else { return Ok(None) };
    let mesh = decode_archive(&bytes, |key| {
        if !scene.textures.contains_key(&key) {
            if let Some(t) = load_texture(store, key).ok().flatten() {
                scene.textures.insert(key, t);
            }
        }
        scene.textures.contains_key(&key)
    })
    .with_context(|| format!("decoding mesh {rdb_type}/{id}"))?;
    scene.meshes.push(mesh);
    Ok(Some(scene.meshes.len() - 1))
}

/// Row-major 4x4 for row vectors (`v' = v * M`), as used by the original engine.
type Mat = [[f32; 4]; 4];

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

struct Builder<'a, 'b> {
    ar: &'b Archive<'a>,
    mesh: Mesh,
    /// (texture, alpha_test) -> index into `mesh.submeshes`
    groups: HashMap<(Option<TextureKey>, bool), usize>,
    have_texture: &'b mut dyn FnMut(TextureKey) -> bool,
    visited: Vec<bool>,
}

fn decode_archive(bytes: &[u8], mut have_texture: impl FnMut(TextureKey) -> bool) -> Result<Mesh> {
    let ar = Archive::parse(bytes)?;
    let mut b = Builder {
        ar: &ar,
        mesh: Mesh::default(),
        groups: HashMap::new(),
        have_texture: &mut have_texture,
        visited: vec![false; ar.objects.len()],
    };
    b.node(ar.root, &IDENTITY, 0)?;
    b.mesh.submeshes.retain(|s| !s.indices.is_empty());
    Ok(b.mesh)
}

impl Builder<'_, '_> {
    fn node(&mut self, i: usize, parent: &Mat, depth: usize) -> Result<()> {
        ensure!(depth < MAX_DEPTH, "frame tree too deep");
        ensure!(!std::mem::replace(&mut self.visited[i], true), "cyclic frame tree");
        let ar = self.ar;
        let n = ar.objects.get(i).with_context(|| format!("dangling object ref {i}"))?;
        let world = mul(&local_matrix(n), parent);
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

    /// (texture, alpha_test) for a `SimpleMesh`: render states and texture channels come from the
    /// node's own `delta_state` (e.g. the "alpha" states of foliage nodes) and the material's
    /// `delta_state`; a texture on the node's state wins over the material's.
    fn material(&mut self, node_ds: Option<&Object>, sm: &Object) -> (Option<TextureKey>, bool) {
        let objs = &self.ar.objects;
        let mat_ds = sm.ref1("material").and_then(|m| objs.get(m)).and_then(|m| m.ref1("delta_state")).and_then(|d| objs.get(d));
        let (mut alpha, mut tex) = (false, None);
        for ds in [node_ds, mat_ds].into_iter().flatten() {
            alpha |= ds
                .all("rst_type")
                .zip(ds.all("rst_value"))
                .any(|(t, v)| matches!((le(t), le(v)), (Some(D3DRS_ALPHATESTENABLE | D3DRS_ALPHABLENDENABLE), Some(1))));
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
        (tex, alpha && tex.is_some())
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
        let key = self.material(node_ds, sm);
        let sub = *self.groups.entry(key).or_insert_with(|| {
            self.mesh.submeshes.push(Submesh {
                blend: if key.1 { Blend::AlphaTest } else { Blend::Opaque },
                two_sided: true,
                ..Submesh::new(vec![], key.0)
            });
            self.mesh.submeshes.len() - 1
        });
        for t in tris.chunks_exact(6) {
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
        let q: Vec<u8> = [0.70710678f32, 0.0, 0.0, 0.70710678].iter().flat_map(|f| f.to_le_bytes()).collect();
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

        let m = decode_archive(&b, |_| false).unwrap();
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
}
