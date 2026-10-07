//! `CATAnim` stream (rdb 1010003): per-bone rotation + translation keyframes, see `docs/formats.md` § characters.
//!
//! Reader: `CATKeyframeAnimData_t::CATKeyframeAnimData_t(DataIO_t*)` @ randy31.dll 10052068.

use super::Rd;
use anyhow::{bail, ensure, Context, Result};
use flate2::read::ZlibDecoder;
use std::io::Read;

#[derive(Clone, Debug)]
pub struct Track {
    pub bone: u32,
    /// Authored layer bitmask (`CATKeyframeAnimData_t+0x40`); enabled when ANDed with playback layers.
    pub mode: u32,
    /// (time in ms, quaternion x y z w), strictly increasing times.
    pub rot: Vec<(f32, [f32; 4])>,
    /// (time in ms, local translation in the parent's frame).
    pub trans: Vec<(f32, [f32; 3])>,
}

#[derive(Clone, Debug)]
pub struct CatAnim {
    /// RDB 1010003 id supplied by the asset loader; zero for a raw parsed stream.
    pub source_id: u32,
    /// Name field: skeleton root bone (`Bip01_ac`, `Bone01_Main`, …).
    pub root: String,
    /// Named markers (time in ms, name): `loopstart`, `attack`, `right`, …
    pub events: Vec<(u32, String)>,
    pub version: u32,
    /// Clip length in ms (the last key sits at exactly this time).
    pub duration: f32,
    /// Skeleton hash, equal to `CatMesh::signature` of the meshes it animates.
    pub signature: u32,
    /// Float at `CATKeyframeAnimData_t+0x30`; unresolved meaning, not the playback clock multiplier
    /// (DisplaySystem 0x10072fde uses only seconds * 1000 * the animation's speed scale).
    pub param: f32,
    pub tracks: Vec<Track>,
}

/// One node of the retail animation blend chain, in composition order (previous chain first).
/// `layers` is the effective `CATKeyframeAnim_t::SetLayers` mask, not an abstract animation id.
#[derive(Clone, Copy, Debug)]
pub struct AnimLayer<'a> {
    pub clip: &'a CatAnim,
    pub ms: f32,
    pub layers: u32,
    /// Weight of this clip when both this clip and the preceding chain enable the bone.
    pub blend: f32,
}

/// DS 0x10073f23: remaining time of the preceding same-priority animation, clamped by
/// 0x100af8a0 (0x43480000 = 200 ms) and 0x100af89c (0x43960000 = 300 ms).
pub fn animation_fade_ms(previous_duration: f32, previous_ms: f32) -> f32 {
    let remaining = if previous_duration > 0.0 {
        previous_duration - previous_ms.rem_euclid(previous_duration)
    } else { 0.0 };
    remaining.clamp(200.0, 300.0)
}

/// Positive-count DS 0x10072562 blend. Short clips finish before the fade-out switch.
pub fn animation_blend(ms: f32, duration: f32, fade_ms: f32) -> f32 {
    (if ms <= fade_ms { ms / fade_ms } else { (duration - ms) / fade_ms }).clamp(0.0, 1.0)
}

/// Apply DS 0x10074180/0x100729f9 exclusion masks to a priority-descending chain.
/// Each tuple carries priority, original Layer_e, and its time/weight sample.
pub fn animation_layers<'a>(nodes: impl Clone + Iterator<Item = (i32, u32, AnimLayer<'a>)>) -> impl Clone + Iterator<Item = AnimLayer<'a>> {
    nodes.scan((i32::MAX, 0u32, 0u32), |state, (priority, exclusion, mut layer)| {
        layer.layers = animation_layer_mask(state, priority, exclusion, layer.layers);
        Some(layer)
    })
}

/// One step of DS's resource-completion mask refresh. Initialize `state` to `(i32::MAX, 0, 0)`.
/// A zero exclusion leaves the stored mask unchanged; expiration does not restore it.
pub fn animation_layer_mask(state: &mut (i32, u32, u32), priority: i32, exclusion: u32, current_mask: u32) -> u32 {
    if state.0 != priority {
        state.2 = state.1;
        state.0 = priority;
    }
    state.1 |= exclusion;
    if state.2 & 3 == 0 { current_mask } else { !(state.2 & 3) & 3 }
}

/// Retail per-bone composition: authored masks, sparse-track passthrough, slerp + linear translation.
/// No bone-name mask and no allocation. DS 0x10074228; randy 0x10050742/0x10050586/0x1005063c.
pub(crate) fn sample_layers<'a>(layers: impl IntoIterator<Item = AnimLayer<'a>>, signature: u32, bone: usize) -> Option<([f32; 4], [f32; 3])> {
    let mut pose: Option<([f32; 4], [f32; 3])> = None;
    let mut enabled = false;
    for layer in layers.into_iter().filter(|l| l.clip.signature == signature) {
        // A fully blended whole-body node discards the preceding chain (DS 0x10074228).
        if layer.blend >= 1.0 && layer.layers > 2 {
            pose = None;
            enabled = false;
        }
        let track = layer.clip.tracks.iter().find(|t| t.bone as usize == bone);
        let active = track.is_some_and(|t| t.mode & layer.layers != 0);
        let next = track.map(|track| sample_track(track, layer.ms));
        pose = match (pose, next) {
            (Some((q, t)), Some((nq, nt))) if enabled && active => Some((
                slerp(q, nq, layer.blend),
                std::array::from_fn(|i| t[i] * (1.0 - layer.blend) + nt[i] * layer.blend),
            )),
            (_, Some(next)) if active || pose.is_none() => Some(next),
            (previous, _) => previous,
        };
        enabled |= active;
    }
    pose
}

/// `FUN_1005bb82`: `ncols` interleaved columns, each a zlib blob of big-endian `ceil(bits/8)`-byte
/// words; every column is a running sum (delta coded) masked to `bits`. Reader `FUN_1005b96a`.
struct Delta {
    mask: u32,
    width: usize,
    cols: Vec<(Vec<u8>, u32)>,
    col: usize,
    off: usize,
}

impl Delta {
    fn new(bits: u32, ncols: usize, r: &mut Rd) -> Result<Self> {
        ensure!((1..=32).contains(&bits), "bad stream width {bits}");
        let mut cols = Vec::with_capacity(ncols);
        for _ in 0..ncols {
            let comp = r.u32()? as usize;
            let raw = r.u32()? as usize;
            ensure!(raw <= 1 << 26, "implausible stream size {raw}");
            let mut out = Vec::with_capacity(raw);
            ZlibDecoder::new(r.bytes(comp)?).read_to_end(&mut out).context("inflating key stream")?;
            ensure!(out.len() == raw, "stream inflates to {} bytes, header says {raw}", out.len());
            cols.push((out, 0));
        }
        Ok(Self { mask: if bits == 32 { u32::MAX } else { (1 << bits) - 1 }, width: bits.div_ceil(8) as usize, cols, col: 0, off: 0 })
    }

    fn next(&mut self) -> Result<u32> {
        let (data, acc) = &mut self.cols[self.col];
        let w = data.get(self.off..self.off + self.width).context("key stream underrun")?;
        *acc = acc.wrapping_add(w.iter().fold(0u32, |v, &b| v << 8 | b as u32));
        let out = *acc & self.mask;
        self.col += 1;
        if self.col == self.cols.len() {
            self.col = 0;
            self.off += self.width;
        }
        Ok(out)
    }
}

impl CatAnim {
    /// The skeleton hash of a record without decoding its keys (header fields of [`CatAnim::parse`]).
    pub fn signature_of(d: &[u8]) -> Result<u32> {
        let mut r = Rd::new(d);
        r.name32()?;
        let n_events = r.u32()? as usize;
        ensure!(n_events * 36 <= r.remaining(), "event table overruns record");
        for _ in 0..n_events {
            r.u32()?;
            r.name32()?;
        }
        ensure!(r.u32()? == 3, "file is not a CATAnim");
        let version = r.u32()? & !0x100_0000;
        r.u32()?; // duration (u32 or f32 by version)
        ensure!(version > 0x104, "unsupported CATAnim version {version:#x}");
        r.u32()
    }

    pub fn parse(d: &[u8]) -> Result<Self> {
        let mut r = Rd::new(d);
        let root = r.name32()?;
        let n_events = r.u32()? as usize;
        ensure!(n_events * 36 <= r.remaining(), "event table overruns record");
        let events = (0..n_events).map(|_| Ok((r.u32()?, r.name32()?))).collect::<Result<Vec<_>>>()?;
        ensure!(r.u32()? == 3, "file is not a CATAnim");
        let mut version = r.u32()?;
        let compressed = version & 0x100_0000 != 0;
        version &= !0x100_0000;
        ensure!(version > 0x104, "unsupported CATAnim version {version:#x}");
        let duration = if version < 0x106 { r.u32()? as f32 } else { r.f32()? };
        let signature = r.u32()?;
        let param = r.f32()?;
        let nb = r.u32()? as usize;
        ensure!(nb <= 4096, "implausible track count {nb}");
        if !compressed {
            bail!("uncompressed CATAnim keys are not present in the data and not decoded");
        }
        let rot_bits = r.i8()?;
        let trans_bits = r.i8()?;
        ensure!((1..=32).contains(&rot_bits) && (1..=32).contains(&trans_bits), "bad key bit widths {rot_bits}/{trans_bits}");
        let (rot_bits, trans_bits) = (rot_bits as u32, trans_bits as u32);
        let mut idx = Delta::new(32, 1, &mut r)?; // bone, unused, mode, rot key count, trans key count
        let mut time = Delta::new(if version >= 0x106 { 32 } else { 20 }, 1, &mut r)?;
        let mut rot = Delta::new(rot_bits, 4, &mut r)?;
        let mut range = Delta::new(32, 1, &mut r)?; // 6 floats per bone: x0 x1 y0 y1 z0 z1 of the translation box
        let mut trans = Delta::new(trans_bits, 3, &mut r)?;
        let t_of = |v: u32| if version >= 0x106 { f32::from_bits(v) } else { v as f32 };
        let bias = (1u64 << (rot_bits - 1)) as f32;
        let tmax = ((1u64 << trans_bits) - 1) as f32;
        let mut tracks = Vec::with_capacity(nb);
        for _ in 0..nb {
            let bone = idx.next()?;
            idx.next()?;
            let mode = idx.next()?;
            let n_rot = idx.next()? as usize;
            ensure!(n_rot <= 1 << 20, "implausible rotation key count");
            let mut rk = Vec::with_capacity(n_rot);
            for _ in 0..n_rot {
                let t = t_of(time.next()?);
                let mut q = [0f32; 4];
                for c in &mut q {
                    *c = rot.next()? as f32 - bias;
                }
                let len = q.iter().map(|c| c * c).sum::<f32>().sqrt();
                rk.push((t, if len > 0.0 { q.map(|c| c / len) } else { [0.0, 0.0, 0.0, 1.0] }));
            }
            let mut rng = [0f32; 6];
            for c in &mut rng {
                *c = f32::from_bits(range.next()?);
            }
            let n_tr = idx.next()? as usize;
            ensure!(n_tr <= 1 << 20, "implausible translation key count");
            let mut tk = Vec::with_capacity(n_tr);
            for _ in 0..n_tr {
                let t = t_of(time.next()?);
                let mut p = [0f32; 3];
                for (i, c) in p.iter_mut().enumerate() {
                    *c = (rng[2 * i + 1] - rng[2 * i]) * (trans.next()? as f32 / tmax) + rng[2 * i];
                }
                tk.push((t, p));
            }
            ensure!((bone as usize) < 4096, "track bone index {bone} out of range");
            tracks.push(Track { bone, mode, rot: rk, trans: tk });
        }
        Ok(Self { source_id: 0, root, events, version, duration, signature, param, tracks })
    }

    /// Local (rotation quaternion xyzw, translation) of `bone` at `t_ms`, `None` if the clip has no track for it.
    pub fn sample(&self, bone: usize, t_ms: f32) -> Option<([f32; 4], [f32; 3])> {
        let tr = self.tracks.iter().find(|t| t.bone as usize == bone)?;
        Some(sample_track(tr, t_ms))
    }
}

fn sample_track(tr: &Track, t_ms: f32) -> ([f32; 4], [f32; 3]) {
        let q = match key_span(&tr.rot, t_ms, |k| k.0) {
            None => [0.0, 0.0, 0.0, 1.0],
            Some((a, b, f)) => slerp(tr.rot[a].1, tr.rot[b].1, f),
        };
        let p = match key_span(&tr.trans, t_ms, |k| k.0) {
            None => [0.0; 3],
            Some((a, b, f)) => std::array::from_fn(|i| tr.trans[a].1[i] * (1.0 - f) + tr.trans[b].1[i] * f),
        };
    (q, p)
}

/// Surrounding keys and blend factor for `t`, clamped to the first/last key (`FUN_10051d2a` / `FUN_10051df4`).
fn key_span<K>(keys: &[K], t: f32, time: impl Fn(&K) -> f32) -> Option<(usize, usize, f32)> {
    let last = keys.len().checked_sub(1)?;
    let hi = keys.partition_point(|k| time(k) <= t);
    if hi == 0 || hi > last {
        let i = if hi == 0 { 0 } else { last };
        return Some((i, i, 0.0));
    }
    let (t0, t1) = (time(&keys[hi - 1]), time(&keys[hi]));
    Some((hi - 1, hi, if t1 > t0 { (t - t0) / (t1 - t0) } else { 0.0 }))
}

/// Shortest-path slerp with lerp fallback for near-parallel quaternions (`FUN_10053d0b`).
fn slerp(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    let dot: f32 = a.iter().zip(&b).map(|(x, y)| x * y).sum();
    let (cos, sign) = if dot < 0.0 { (-dot, -1.0) } else { (dot, 1.0) };
    let (wa, wb) = if 1.0 - cos > 1e-4 {
        let th = cos.min(1.0).acos();
        let s = th.sin();
        (((1.0 - t) * th).sin() / s, (t * th).sin() / s)
    } else {
        (1.0 - t, t)
    };
    std::array::from_fn(|i| wa * a[i] + sign * wb * b[i])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slerp_halfway_and_shortest_path() {
        let h = std::f32::consts::FRAC_1_SQRT_2;
        let r = slerp([0.0, 0.0, 0.0, 1.0], [0.0, 0.0, 1.0, 0.0], 0.5);
        assert!((r[2] - h).abs() < 1e-6 && (r[3] - h).abs() < 1e-6);
        // q and -q are the same rotation: interpolating towards -w must not swing the long way round
        let r = slerp([0.0, 0.0, 0.0, 1.0], [0.0, 0.0, 0.0, -1.0], 0.5);
        assert!((r[3] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn key_span_clamps_and_interpolates() {
        let k = [(0.0f32, ()), (100.0, ()), (300.0, ())];
        let f = |t| key_span(&k, t, |k| k.0);
        assert_eq!(f(-5.0), Some((0, 0, 0.0)));
        assert_eq!(f(400.0), Some((2, 2, 0.0)));
        assert_eq!(f(200.0), Some((1, 2, 0.5)));
        assert_eq!(key_span::<f32>(&[], 0.0, |k| *k), None);
    }

    #[test]
    fn authored_layers_blend_and_pass_through_sparse_tracks() {
        let clip = |tracks| CatAnim { source_id: 0, root: String::new(), events: vec![], version: 0x106, duration: 100.0, signature: 7, param: 0.0, tracks };
        let track = |bone, mode, x| Track { bone, mode, rot: vec![(0.0, [0.0, 0.0, 0.0, 1.0])], trans: vec![(0.0, [x, 0.0, 0.0])] };
        let base = clip(vec![track(0, 1, 2.0), track(1, 2, 4.0), track(2, 2, 6.0)]);
        let upper = clip(vec![track(0, 1, 10.0), track(1, 2, 12.0)]);
        let layers = [
            AnimLayer { clip: &base, ms: 0.0, layers: 3, blend: 1.0 },
            AnimLayer { clip: &upper, ms: 0.0, layers: 2, blend: 0.5 },
        ];
        assert_eq!(sample_layers(layers, 7, 0).unwrap().1[0], 2.0);
        assert_eq!(sample_layers(layers, 7, 1).unwrap().1[0], 8.0);
        assert_eq!(sample_layers(layers, 7, 2).unwrap().1[0], 6.0);
        assert_eq!(sample_layers(layers, 8, 1), None);
        let only = [AnimLayer { clip: &upper, ms: 0.0, layers: 2, blend: 0.0 }];
        // A track enabled only by anim2 passes through even with blend zero.
        assert_eq!(sample_layers(only, 7, 1).unwrap().1[0], 12.0);
        let overlay = [
            AnimLayer { clip: &upper, ms: 0.0, layers: 3, blend: 1.0 },
            AnimLayer { clip: &base, ms: 0.0, layers: 2, blend: 1.0 },
        ];
        assert_eq!(sample_layers(overlay, 7, 0).unwrap().1[0], 10.0);
        assert_eq!(sample_layers(overlay, 7, 1).unwrap().1[0], 4.0);
        assert_eq!(sample_layers(overlay, 7, 2).unwrap().1[0], 6.0);
    }

    #[test]
    fn retail_priority_masks_and_positive_count_fades() {
        assert_eq!(animation_fade_ms(0.0, 0.0), 200.0);
        assert_eq!(animation_fade_ms(1000.0, 750.0), 250.0);
        assert_eq!(animation_fade_ms(1000.0, 500.0), 300.0);
        assert_eq!(animation_blend(100.0, 1000.0, 200.0), 0.5);
        assert_eq!(animation_blend(800.0, 1000.0, 200.0), 1.0);
        assert_eq!(animation_blend(900.0, 1000.0, 200.0), 0.5);
        assert_eq!(animation_blend(100.0, 100.0, 200.0), 0.5, "short clips never switch to fade-out");
        let clip = CatAnim { source_id: 0, root: String::new(), events: vec![], version: 0x106, duration: 1000.0, signature: 7, param: 0.0, tracks: vec![] };
        let layer = AnimLayer { clip: &clip, ms: 0.0, layers: 3, blend: 0.5 };
        let masks: Vec<_> = animation_layers([(0, 0, layer), (-1, 1, layer), (-1, 0, layer), (-2, 0, layer)].into_iter()).map(|l| l.layers).collect();
        assert_eq!(masks, [3, 3, 3, 2], "only lower-priority groups inherit exclusion bits");
    }
}
