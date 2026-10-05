//! Default camera ("spawn") selection.
//!
//! The playfield records carry no entry point (the server tells the client where the character
//! arrives), so the viewer needs a stand-in that opens on content: outdoors the densest cluster of
//! placed statels, in dungeons an entrance-like room (see `dungeon::room_spot`).

use std::collections::HashMap;

use ao_scene::{Mesh, Scene};

/// Camera placement in scene space.
#[derive(Clone, Copy, Debug)]
pub struct Spot {
    pub eye: [f32; 3],
    pub at: [f32; 3],
}

pub type Box3 = ([f32; 3], [f32; 3]);

/// Statels larger than this (skydomes, backdrops) neither attract nor block the camera.
const MAX_EXTENT: f32 = 150.0;
const BUCKET: f32 = 32.0;

pub fn mesh_bounds(m: &Mesh) -> Option<Box3> {
    let mut it = m.vertices.iter().map(|v| v.pos);
    let first = it.next()?;
    Some(it.fold((first, first), |(mut lo, mut hi), p| {
        for i in 0..3 {
            lo[i] = lo[i].min(p[i]);
            hi[i] = hi[i].max(p[i]);
        }
        (lo, hi)
    }))
}

/// World-space box of a mesh box under a column-major transform.
pub fn transform_box(b: &Box3, t: &[[f32; 4]; 4]) -> Box3 {
    let mut out = ([f32::MAX; 3], [f32::MIN; 3]);
    for c in 0..8 {
        let p = [if c & 1 == 0 { b.0[0] } else { b.1[0] }, if c & 2 == 0 { b.0[1] } else { b.1[1] }, if c & 4 == 0 { b.0[2] } else { b.1[2] }];
        for i in 0..3 {
            let v = t[0][i] * p[0] + t[1][i] * p[1] + t[2][i] * p[2] + t[3][i];
            out.0[i] = out.0[i].min(v);
            out.1[i] = out.1[i].max(v);
        }
    }
    out
}

/// World boxes of `scene.instances[from..]`.
pub fn instance_boxes(scene: &Scene, from: usize) -> Vec<Box3> {
    let mut cache: HashMap<usize, Option<Box3>> = HashMap::new();
    scene.instances[from..]
        .iter()
        .filter_map(|i| {
            let b = (*cache.entry(i.mesh).or_insert_with(|| mesh_bounds(&scene.meshes[i.mesh])))?;
            Some(transform_box(&b, &i.transform))
        })
        .collect()
}

fn centre(b: &Box3) -> [f32; 3] {
    [(b.0[0] + b.1[0]) * 0.5, (b.0[1] + b.1[1]) * 0.5, (b.0[2] + b.1[2]) * 0.5]
}

/// Outdoor / fallback spawn: eye `eye_h` above `ground(x, z)` next to the densest statel cluster,
/// looking at the statels around it. `ground` returns `None` outside the walkable area.
pub fn density_spawn(boxes: &[Box3], ground: &dyn Fn(f32, f32) -> Option<f32>, eye_h: f32) -> Option<Spot> {
    let boxes: Vec<&Box3> = boxes.iter().filter(|b| b.0.iter().chain(&b.1).all(|v| v.is_finite()) && (0..3).all(|i| b.1[i] - b.0[i] < MAX_EXTENT)).collect();
    let cen: Vec<[f32; 3]> = boxes.iter().map(|b| centre(b)).collect();
    let key = |p: &[f32; 3]| ((p[0] / BUCKET).floor() as i32, (p[2] / BUCKET).floor() as i32);
    let mut grid: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
    for (i, c) in cen.iter().enumerate() {
        grid.entry(key(c)).or_default().push(i);
    }
    let around = |k: (i32, i32), r: i32| -> Vec<usize> {
        (-r..=r).flat_map(|dx| (-r..=r).map(move |dz| (k.0.saturating_add(dx), k.1.saturating_add(dz)))).filter_map(|k| grid.get(&k)).flatten().copied().collect()
    };
    let mut keys: Vec<_> = grid.keys().copied().collect();
    keys.sort();
    let best = keys.into_iter().max_by_key(|&k| around(k, 1).len())?;
    let block = around(best, 1);
    let n = block.len() as f32;
    let (cx, cz) = (block.iter().map(|&i| cen[i][0]).sum::<f32>() / n, block.iter().map(|&i| cen[i][2]).sum::<f32>() / n);
    // first free eye position on rings around the cluster centre
    let blocked = |x: f32, z: f32, y0: f32, y1: f32| boxes.iter().any(|b| x > b.0[0] - 1.0 && x < b.1[0] + 1.0 && z > b.0[2] - 1.0 && z < b.1[2] + 1.0 && y1 > b.0[1] && y0 < b.1[1]);
    let mut eye = None;
    'rings: for ring in 0..20 {
        let r = ring as f32 * 8.0;
        for a in 0..(if ring == 0 { 1 } else { 12 }) {
            let t = a as f32 * std::f32::consts::TAU / 12.0;
            let (x, z) = (cx + r * t.cos(), cz + r * t.sin());
            let Some(g) = ground(x, z) else { continue };
            if !blocked(x, z, g + 0.3, g + eye_h) {
                eye = Some([x, g + eye_h, z]);
                break 'rings;
            }
        }
    }
    // nothing free at street level (solid city blocks): hover above the tallest statel nearby
    let eye = eye.unwrap_or_else(|| [cx, block.iter().map(|&i| boxes[i].1[1]).fold(f32::MIN, f32::max) + 12.0, cz + 30.0]);
    // face the heading with the most statels (15..250 m, +-40 degrees) that terrain does not block
    let mut best: Option<(bool, usize, f32, f32)> = None; // clear, count, target x, z
    for k in 0..16 {
        let a = k as f32 * std::f32::consts::TAU / 16.0;
        let (dx, dz) = (a.cos(), a.sin());
        let seen: Vec<&[f32; 3]> = cen
            .iter()
            .filter(|c| {
                let (rx, rz) = (c[0] - eye[0], c[2] - eye[2]);
                let d = rx.hypot(rz);
                (15.0..250.0).contains(&d) && (rx * dx + rz * dz) / d > 0.766
            })
            .collect();
        let clear = [3.0, 6.0, 10.0, 15.0].iter().all(|d| !blocked(eye[0] + dx * d, eye[2] + dz * d, eye[1] - 1.0, eye[1] + 1.0)) && [8.0, 16.0, 32.0, 64.0].iter().all(|d| ground(eye[0] + dx * d, eye[2] + dz * d).is_none_or(|g| g < eye[1] - 1.0));
        let n = seen.len().max(1) as f32;
        let t = if seen.is_empty() { (eye[0] + dx * 50.0, eye[2] + dz * 50.0) } else { (seen.iter().map(|c| c[0]).sum::<f32>() / n, seen.iter().map(|c| c[2]).sum::<f32>() / n) };
        if best.is_none_or(|b| (clear, seen.len()) > (b.0, b.1)) {
            best = Some((clear, seen.len(), t.0, t.1));
        }
    }
    let (_, _, tx, tz) = best?;
    Some(Spot { eye, at: [tx, eye[1] - 2.0, tz] })
}

/// Highest triangle surface of a fixed (identity-placed: terrain, room shells) mesh below `p`, or `None`.
pub fn floor_below(scene: &Scene, p: [f32; 3]) -> Option<f32> {
    let mut best: Option<f32> = None;
    for inst in scene.instances.iter().filter(|i| i.transform == ao_scene::IDENTITY) {
        let m = &scene.meshes[inst.mesh];
        for s in &m.submeshes {
            for t in s.indices.chunks_exact(3) {
                let [a, b, c] = [t[0], t[1], t[2]].map(|i| m.vertices[i as usize].pos);
                // barycentric in xz
                let d = (b[2] - c[2]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[2] - c[2]);
                if d.abs() < 1e-9 {
                    continue;
                }
                let l1 = ((b[2] - c[2]) * (p[0] - c[0]) + (c[0] - b[0]) * (p[2] - c[2])) / d;
                let l2 = ((c[2] - a[2]) * (p[0] - c[0]) + (a[0] - c[0]) * (p[2] - c[2])) / d;
                let l3 = 1.0 - l1 - l2;
                if l1 < -1e-4 || l2 < -1e-4 || l3 < -1e-4 {
                    continue;
                }
                let y = l1 * a[1] + l2 * b[1] + l3 * c[1];
                if y <= p[1] && best.is_none_or(|b| y > b) {
                    best = Some(y);
                }
            }
        }
    }
    best
}

/// Bounds of everything in the scene (instance boxes).
pub fn scene_bounds(scene: &Scene) -> Option<Box3> {
    instance_boxes(scene, 0).into_iter().reduce(|a, b| ([a.0[0].min(b.0[0]), a.0[1].min(b.0[1]), a.0[2].min(b.0[2])], [a.1[0].max(b.1[0]), a.1[1].max(b.1[1]), a.1[2].max(b.1[2])]))
}
