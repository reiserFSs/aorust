//! Node keyframe animation of static meshes (doors and other animated items), docs/zone/doors.md §3.
//!
//! A mesh node with an `anim` member (`FAFAnim_t` = `RKeyFrameAnimation_t`, randy31 `Archive` @0x10028c87) owns rotation keys
//! (`{x, y, z, w, time}`, 20 bytes) and translation keys (`{x, y, z, time}`, 16 bytes). `RRefFrame_t::SetAnimationTime`
//! (@0x1004506b) evaluates them (`FUN_10028fde` @0x10028fde: lerp of the translation, slerp `FUN_1006eefa` of the rotation) into the
//! node's animation matrix (`FUN_1006e393` quaternion rows + translation row), and `UpdateWorldMatrix` (@0x10044fa2) multiplies it in front
//! of the node's own `R(local_rot) * scale + local_pos` matrix. The file's `anim_matrix` member is that matrix at time 0 (verified by
//! `anim_matrix_is_the_pose_at_time_zero` on every door mesh). The visibility and uv keys are not applied (same as the static decoder).

use super::{base_matrix, mul, FvfLayout, Mat, IDENTITY_MAT, MAX_DEPTH};
use crate::archive::{Archive, Object};
use anyhow::{ensure, Context, Result};
use ao_scene::Vertex;
use std::ops::Range;

/// Keyframes of one node.
struct Keys {
    rot: Vec<([f32; 4], f32)>,
    trans: Vec<([f32; 3], f32)>,
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
}

/// The node tree of a mesh with the object-space vertices, posed at any animation time.
pub struct NodeRig {
    nodes: Vec<RigNode>,
    /// Vertices as stored (object space of the owning node, AO's left-handed space).
    pos: Vec<[f32; 3]>,
    nrm: Vec<[f32; 3]>,
    total: f32,
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
    let rot = f32s(rot).as_chunks::<5>().0.iter().map(|k| ([k[0], k[1], k[2], k[3]], k[4])).collect();
    let trans = f32s(trans).as_chunks::<4>().0.iter().map(|k| ([k[0], k[1], k[2]], k[3])).collect();
    let total = a.f32s::<1>("tot_time").context("animation without tot_time")?[0];
    Ok(Some(Keys { rot, trans, total }))
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
    /// The animation matrix at animation time `t` of the mesh (`RRefFrame_t::SetAnimationTime` @0x1004506b: `t` wraps by this node's
    /// total time, an exact multiple stays at the end).
    fn matrix(&self, t: f32) -> Mat {
        let wrapped = if self.total > 0.0 { t % self.total } else { 0.0 };
        let t = if wrapped == 0.0 && t > 0.0 { self.total } else { wrapped };
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
        let mut rig = NodeRig { nodes: vec![], pos: vec![], nrm: vec![], total: 0.0 };
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
        self.nodes.push(RigNode { parent, base: base_matrix(n), rest, keys, verts: start..self.pos.len() });
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
}
