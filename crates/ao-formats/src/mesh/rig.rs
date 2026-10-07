//! Node keyframe animation of static meshes (doors and other animated items), docs/zone/doors.md §3.
//!
//! A mesh node with an `anim` member (`FAFAnim_t` = `RKeyFrameAnimation_t`, randy31 `Archive` @0x10028c87) owns rotation keys
//! (`{x, y, z, w, time}`, 20 bytes) and translation keys (`{x, y, z, time}`, 16 bytes). `RRefFrame_t::SetAnimationTime`
//! (@0x1004506b) evaluates them (`FUN_10028fde` @0x10028fde: lerp of the translation, slerp `FUN_1006eefa` of the rotation) into the
//! node's animation matrix (`FUN_1006e393` quaternion rows + translation row), and `UpdateWorldMatrix` (@0x10044fa2) multiplies it in front
//! of the node's own `R(local_rot) * scale + local_pos` matrix. The file's `anim_matrix` member is that matrix at time 0 (verified by
//! `anim_matrix_is_the_pose_at_time_zero` on every door mesh). The rigid-frame sampler also exposes authored visibility and UV
//! channels; the legacy baked-vertex sampler preserves its caller's UVs.

use super::{base_matrix, mul, FvfLayout, Mat, IDENTITY_MAT, MAX_DEPTH};
use crate::archive::{Archive, Object};
use anyhow::{ensure, Context, Result};
use ao_scene::Vertex;
use std::ops::Range;

/// Keyframes of one node.
struct Keys {
    rot: Vec<([f32; 4], f32)>,
    trans: Vec<([f32; 3], f32)>,
    uv: Vec<(([f32; 4], bool), f32)>,
    visibility: Vec<(bool, f32)>,
    /// `tot_time` of the animation.
    total: f32,
}

struct RigNode {
    parent: Option<usize>,
    /// `R(local_rot) * scale`, translation `local_pos`.
    base: Mat,
    /// The file's `anim_matrix` (identity if absent): the pose of nodes without keys.
    rest: Mat,
    keys: Option<Keys>,
    /// The baked vertices of the node's `SimpleMesh`es (`mesh::decode_mesh_into` appends them in this order).
    verts: Range<usize>,
    /// Index of this frame's object-space mesh in an animated actor model.
    part: Option<usize>,
}

/// The node tree of a mesh with the object-space vertices, posed at any animation time.
pub struct NodeRig {
    nodes: Vec<RigNode>,
    /// Vertices as stored (object space of the owning node, AO's left-handed space).
    pos: Vec<[f32; 3]>,
    nrm: Vec<[f32; 3]>,
    total: f32,
    /// Retained parent matrices used by the rigid-frame sampler.
    world: Vec<Mat>,
}

fn f32s(d: &[u8]) -> Vec<f32> {
    d.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)).collect()
}

fn keys_of(ar: &Archive, n: &Object) -> Result<Option<Keys>> {
    let Some(a) = n.ref1("anim") else { return Ok(None) };
    let a = ar.objects.get(a).context("dangling anim ref")?;
    let rot = a.blob("rot_keys").context("animation without rot_keys")?;
    let trans = a.blob("trans_keys").context("animation without trans_keys")?;
    ensure!(rot.len() % 20 == 0 && trans.len() % 16 == 0, "key blob sizes {} / {}", rot.len(), trans.len());
    let rot: Vec<([f32; 4], f32)> = f32s(rot).as_chunks::<5>().0.iter().map(|k| ([k[0], k[1], k[2], k[3]], k[4])).collect();
    let trans: Vec<([f32; 3], f32)> = f32s(trans).as_chunks::<4>().0.iter().map(|k| ([k[0], k[1], k[2]], k[3])).collect();
    let uv = if a.get("uv_keys").is_some() { a.blob("uv_keys").context("malformed UV key blob")? } else { &[] };
    let visibility = if a.get("vis_keys").is_some() { a.blob("vis_keys").context("malformed visibility key blob")? } else { &[] };
    ensure!(uv.len() % 24 == 0 && visibility.len() % 8 == 0, "invalid UV/visibility key sizes");
    let uv: Vec<_> = uv.as_chunks::<24>().0.iter().map(|key| {
        let value = |i: usize| f32::from_le_bytes(key[i * 4..i * 4 + 4].try_into().unwrap());
        (([value(0), value(1), value(2), value(3)], key[20..24] != [0; 4]), value(4))
    }).collect();
    let visibility: Vec<_> = visibility.as_chunks::<8>().0.iter().map(|key| {
        (key[4] != 0, f32::from_le_bytes(key[..4].try_into().unwrap()))
    }).collect();
    let total = a.f32s::<1>("tot_time").context("animation without tot_time")?[0];
    ensure!(total.is_finite() && total >= 0.0, "invalid animation total time");
    ensure!(rot.iter().all(|(v, t)| t.is_finite() && *t >= 0.0 && v.iter().all(|x| x.is_finite()))
        && trans.iter().all(|(v, t)| t.is_finite() && *t >= 0.0 && v.iter().all(|x| x.is_finite())), "nonfinite animation key");
    ensure!(rot.windows(2).all(|k| k[0].1 <= k[1].1)
        && trans.windows(2).all(|k| k[0].1 <= k[1].1), "unordered animation keys");
    ensure!(uv.iter().all(|((v, _), t)| t.is_finite() && *t >= 0.0 && v.iter().all(|x| x.is_finite()))
        && visibility.iter().all(|(_, t)| t.is_finite() && *t >= 0.0), "nonfinite UV/visibility key");
    ensure!(uv.windows(2).all(|k| k[0].1 <= k[1].1)
        && visibility.windows(2).all(|k| k[0].1 <= k[1].1), "unordered UV/visibility keys");
    Ok(Some(Keys { rot, trans, total, uv, visibility }))
}

/// Segment of `keys` at `t` (clamped to the last key, as `FUN_10028fde` does) and the interpolation factor.
fn segment<T>(keys: &[(T, f32)], t: f32) -> (usize, f32) {
    let Some(last) = keys.last() else { return (0, 0.0) };
    let t = t.min(last.1);
    let k = keys.iter().rposition(|k| k.1 <= t).unwrap_or(0).min(keys.len().saturating_sub(2));
    match keys.get(k + 1) {
        Some(next) if t > keys[k].1 && next.1 > keys[k].1 => (k, (t - keys[k].1) / (next.1 - keys[k].1)),
        _ => (k, 0.0),
    }
}

/// `FUN_1006eefa` @0x1006eefa: slerp with the shorter arc; below `1e-6` of separation plain `(1 - t, t)` weights; the result is not
/// renormalised.
fn slerp(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    let mut dot: f32 = a.iter().zip(&b).map(|(x, y)| x * y).sum();
    let neg = dot < 0.0;
    if neg {
        dot = -dot;
    }
    let (w1, mut w2) = if 1.0 - dot >= 1e-6 {
        let th = dot.min(1.0).acos();
        let s = th.sin();
        (((1.0 - t) * th).sin() / s, (t * th).sin() / s)
    } else {
        (1.0 - t, t)
    };
    if neg {
        w2 = -w2;
    }
    std::array::from_fn(|i| w1 * a[i] + w2 * b[i])
}

/// `FUN_1006e393` @0x1006e393 (the rotation rows of `RRefFrame_t`): quaternion `(x, y, z, w)` to a row-vector matrix.
fn rotation(q: [f32; 4]) -> Mat {
    let [x, y, z, w] = q;
    [
        [1.0 - 2.0 * (y * y + z * z), 2.0 * (z * w + x * y), 2.0 * (x * z - w * y), 0.0],
        [2.0 * (x * y - z * w), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z + w * x), 0.0],
        [2.0 * (w * y + x * z), 2.0 * (y * z - w * x), 1.0 - 2.0 * (x * x + y * y), 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

impl Keys {
    fn time(&self, t: f32) -> f32 {
        let wrapped = if self.total > 0.0 { t % self.total } else { 0.0 };
        if wrapped == 0.0 && t > 0.0 { self.total } else { wrapped }
    }

    fn visual(&self, t: f32) -> ([f32; 4], bool) {
        let t = self.time(t);
        let (index, fraction) = segment(&self.uv, t);
        let mut uv = [1.0, 1.0, 0.0, 0.0];
        if let Some(((value, interpolate_offset), _)) = self.uv.get(index) {
            uv = *value;
            if let Some(((next, _), _)) = self.uv.get(index + 1) {
                for axis in 0..4 {
                    if axis < 2 || *interpolate_offset {
                        uv[axis] = value[axis] * (1.0 - fraction) + next[axis] * fraction;
                    }
                }
            }
        }
        let (index, _) = segment(&self.visibility, t);
        let visible = self.visibility.get(index).is_none_or(|(visible, _)| *visible);
        (uv, visible)
    }
    /// The animation matrix at animation time `t` of the mesh (`RRefFrame_t::SetAnimationTime` @0x1004506b: `t` wraps by this node's
    /// total time, an exact multiple stays at the end).
    fn matrix(&self, t: f32) -> Mat {
        let t = self.time(t);
        let (k, f) = segment(&self.rot, t);
        let q = match (self.rot.get(k), self.rot.get(k + 1)) {
            (Some(a), Some(b)) if f > 0.0 => slerp(a.0, b.0, f),
            (Some(a), _) => a.0,
            _ => [0.0, 0.0, 0.0, 1.0],
        };
        let mut m = rotation(q);
        let (k, f) = segment(&self.trans, t);
        if let Some(a) = self.trans.get(k) {
            let b = self.trans.get(k + 1).filter(|_| f > 0.0).map_or(a.0, |b| b.0);
            m[3] = [a.0[0] * (1.0 - f) + b[0] * f, a.0[1] * (1.0 - f) + b[1] * f, a.0[2] * (1.0 - f) + b[2] * f, 1.0];
        }
        m
    }
}

impl NodeRig {
    /// The rig of mesh record `id` (type 1010001); `None` = no record or nothing animated.
    pub fn load(store: &ao_rdb::RecordStore, id: u32) -> Result<Option<NodeRig>> {
        let Some(bytes) = store.get(super::MESH_TYPE, id)? else { return Ok(None) };
        Self::parse(&bytes).with_context(|| format!("rig of mesh {id}"))
    }

    /// Builds the rig of a mesh archive; `None` when no node has keyframes (nothing to animate).
    pub fn parse(bytes: &[u8]) -> Result<Option<NodeRig>> {
        let ar = Archive::parse(bytes)?;
        let mut rig = NodeRig { nodes: vec![], pos: vec![], nrm: vec![], total: 0.0, world: vec![] };
        let mut visited = vec![false; ar.objects.len()];
        rig.walk(&ar, ar.root, None, 0, &mut visited)?;
        Ok(rig.nodes.iter().any(|n| n.keys.is_some()).then_some(rig))
    }

    fn walk(&mut self, ar: &Archive, i: usize, parent: Option<usize>, depth: usize, visited: &mut [bool]) -> Result<()> {
        ensure!(depth < MAX_DEPTH, "frame tree too deep");
        let n = ar.objects.get(i).with_context(|| format!("dangling object ref {i}"))?;
        ensure!(!std::mem::replace(&mut visited[i], true), "cyclic frame tree");
        let keys = keys_of(ar, n)?;
        if let Some(k) = &keys {
            self.total = self.total.max(k.total);
        }
        let start = self.pos.len();
        if let Some(data) = n.ref1("data") {
            let data = ar.objects.get(data).context("dangling data ref")?;
            for m in data.refs("mesh") {
                let sm = ar.objects.get(m).context("dangling mesh ref")?;
                let desc = sm.get("vb_desc").context("SimpleMesh without vb_desc")?;
                ensure!(desc.len() >= 16, "short vb_desc");
                let fvf = u32::from_le_bytes(desc[8..12].try_into().unwrap());
                let layout = FvfLayout::new(fvf)?;
                let verts = sm.blob("vertices").context("SimpleMesh without vertices")?;
                ensure!(verts.len() % layout.stride == 0, "partial frame vertex");
                for v in verts.chunks_exact(layout.stride) {
                    let f = |o: usize| f32::from_le_bytes(v[o..o + 4].try_into().unwrap());
                    self.pos.push([f(0), f(4), f(8)]);
                    self.nrm.push(layout.normal.map_or([0.0, 1.0, 0.0], |o| [f(o), f(o + 4), f(o + 8)]));
                }
            }
        }
        let rest = match n.f32s::<16>("anim_matrix") {
            Some(a) => [[a[0], a[1], a[2], a[3]], [a[4], a[5], a[6], a[7]], [a[8], a[9], a[10], a[11]], [a[12], a[13], a[14], a[15]]],
            None => IDENTITY_MAT,
        };
        let me = self.nodes.len();
        self.nodes.push(RigNode { parent, base: base_matrix(n), rest, keys, verts: start..self.pos.len(), part: None });
        for c in n.refs("chld") {
            self.walk(ar, c, Some(me), depth + 1, visited)?;
        }
        Ok(())
    }

    /// Number of baked vertices (the vertex count of the static decode of the same record).
    pub fn vertex_count(&self) -> usize {
        self.pos.len()
    }

    /// `GetAnimationTreeTotalTime`: the longest node animation (seconds).
    pub fn total_time(&self) -> f32 {
        self.total
    }

    /// Samples the native frame tree into actor model-part transforms without vertex copies.
    /// Call with the retained output vector; neither it nor the parent scratch reallocates after
    /// the first call. Matrices are transposed into scene column-vector, Z-mirrored space.
    pub fn pose_parts(&mut self, t: f32, out: &mut Vec<Mat>) {
        self.world.clear();
        out.clear();
        for n in &self.nodes {
            let local = mul(&n.keys.as_ref().map_or(n.rest, |k| k.matrix(t)), &n.base);
            let w = n.parent.map_or(local, |p| mul(&local, &self.world[p]));
            self.world.push(w);
            if let Some(part) = n.part {
                debug_assert_eq!(part, out.len());
                out.push(std::array::from_fn(|i| std::array::from_fn(|j| {
                    w[i][j] * if (i == 2) != (j == 2) { -1.0 } else { 1.0 }
                })));
            }
        }
    }

    /// Samples per-geometry-frame UV scale/offset and visibility (`FUN_10028fde`).
    /// Missing authored channels preserve identity UVs and visibility. Reuse both output vectors.
    pub fn pose_visuals(&self, t: f32, uvs: &mut Vec<[f32; 4]>, visible: &mut Vec<bool>) {
        uvs.clear();
        visible.clear();
        for node in &self.nodes {
            if node.part.is_some() {
                let (uv, visibility) = node.keys.as_ref().map_or(([1.0, 1.0, 0.0, 0.0], true), |keys| keys.visual(t));
                uvs.push(uv);
                visible.push(visibility);
            }
        }
    }

    /// A depth-first frame's world matrix after [`Self::pose_parts`], including ancestor-only
    /// connector frames. Uses the same scene-space convention as the actor part matrices.
    pub fn frame_transform(&self, frame: usize) -> Option<Mat> {
        self.world.get(frame).map(|w| std::array::from_fn(|i| std::array::from_fn(|j| {
            w[i][j] * if (i == 2) != (j == 2) { -1.0 } else { 1.0 }
        })))
    }

    pub(super) fn set_parts(&mut self, parts: &[Option<usize>]) {
        for (node, part) in self.nodes.iter_mut().zip(parts) {
            node.part = *part;
        }
        self.world.reserve(self.nodes.len());
    }

    /// Writes the posed positions and normals (scene space: Z negated, as the static decode) of animation time `t` into `out`, which holds
    /// the rest vertices of the static decode (uv and the other fields are kept).
    pub fn pose(&self, t: f32, out: &mut [Vertex]) {
        let mut world: Vec<Mat> = Vec::with_capacity(self.nodes.len());
        for n in &self.nodes {
            let anim = n.keys.as_ref().map_or(n.rest, |k| k.matrix(t));
            let local = mul(&anim, &n.base);
            let w = match n.parent {
                Some(p) => mul(&local, &world[p]),
                None => local,
            };
            for i in n.verts.clone() {
                let Some(o) = out.get_mut(i) else { break };
                let xf = |a: [f32; 3], t: f32| -> [f32; 3] { std::array::from_fn(|j| a[0] * w[0][j] + a[1] * w[1][j] + a[2] * w[2][j] + t * w[3][j]) };
                let (mut p, mut nr) = (xf(self.pos[i], 1.0), xf(self.nrm[i], 0.0));
                let len = nr.iter().map(|c| c * c).sum::<f32>().sqrt();
                if len > 1e-12 {
                    nr.iter_mut().for_each(|c| *c /= len);
                }
                p[2] = -p[2];
                nr[2] = -nr[2];
                o.pos = p;
                o.normal = nr;
            }
            world.push(w);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys() -> Keys {
        Keys {
            rot: vec![([0.0, 0.0, 0.0, 1.0], 0.0), ([0.0, 0.0, 1.0, 0.0], 1.0)],
            trans: vec![([0.0, 0.0, 0.0], 0.0), ([0.0, 2.0, 0.0], 0.5), ([0.0, 2.0, 0.0], 1.0)],
            total: 1.0,
            uv: vec![], visibility: vec![],
        }
    }

    #[test]
    fn translation_is_a_lerp_and_clamps_after_the_last_key() {
        let k = keys();
        assert_eq!(k.matrix(0.0)[3], [0.0, 0.0, 0.0, 1.0]);
        assert!((k.matrix(0.25)[3][1] - 1.0).abs() < 1e-6);
        assert_eq!(k.matrix(0.75)[3][1], 2.0);
        assert_eq!(k.matrix(1.0)[3][1], 2.0); // a multiple of the total time stays at the end (RRefFrame_t::SetAnimationTime)
        assert_eq!(k.matrix(2.0)[3][1], 2.0);
    }

    #[test]
    fn rotation_is_a_slerp_of_the_quaternion_rows() {
        let k = keys();
        let half = k.matrix(0.5); // 90 degrees about Z of the 180 degree key
        assert!((half[0][0] - 0.0).abs() < 1e-5 && (half[0][1] - 1.0).abs() < 1e-5, "{half:?}");
        let end = k.matrix(1.0);
        assert!((end[0][0] + 1.0).abs() < 1e-5 && (end[1][1] + 1.0).abs() < 1e-5);
    }

    #[test]
    fn rotation_rows_match_the_static_loader() {
        // (1, 0, 0, 0) is the 180 degree turn about X that every door node of the data starts with
        let m = rotation([1.0, 0.0, 0.0, 0.0]);
        assert_eq!([m[0][0], m[1][1], m[2][2]], [1.0, -1.0, -1.0]);
    }

    #[test]
    fn rigid_parts_compose_animated_ancestors_without_steady_state_allocations() {
        let mut base = IDENTITY_MAT;
        base[3] = [3.0, 0.0, 4.0, 1.0];
        let mut rig = NodeRig {
            nodes: vec![
                RigNode { parent: None, base, rest: IDENTITY_MAT, keys: Some(keys()), verts: 0..0, part: None },
                RigNode { parent: Some(0), base: IDENTITY_MAT, rest: IDENTITY_MAT, keys: None, verts: 0..0, part: Some(0) },
            ],
            pos: vec![], nrm: vec![], total: 1.0, world: vec![],
        };
        let mut parts = Vec::new();
        rig.pose_parts(0.25, &mut parts);
        assert_eq!(parts[0][3], [3.0, 1.0, -4.0, 1.0]);
        let pointers = (parts.as_ptr(), rig.world.as_ptr());
        let capacities = (parts.capacity(), rig.world.capacity());
        for tick in 0..100 {
            rig.pose_parts(tick as f32 / 100.0, &mut parts);
            assert_eq!((parts.as_ptr(), rig.world.as_ptr()), pointers);
            assert_eq!((parts.capacity(), rig.world.capacity()), capacities);
        }
    }

    #[test]
    fn uv_scales_interpolate_but_offset_interpolation_is_authored() {
        let mut keys = keys();
        keys.uv = vec![(([1.0, 2.0, 0.0, 1.0], true), 0.0), (([3.0, 4.0, 2.0, 3.0], true), 1.0)];
        keys.visibility = vec![(true, 0.0), (false, 0.5), (false, 1.0)];
        assert_eq!(keys.visual(0.25), ([1.5, 2.5, 0.5, 1.5], true));
        assert!(!keys.visual(0.75).1);
        keys.uv[0].0.1 = false;
        assert_eq!(keys.visual(0.25).0, [1.5, 2.5, 0.0, 1.0]);
        assert_eq!(keys.visual(1.0).0, [3.0, 4.0, 0.0, 1.0]);
    }
}
