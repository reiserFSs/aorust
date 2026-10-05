//! Outdoor terrain mesh: per-patch meshes with smooth tile transitions and baked ground shadows.
//!
//! * One [`Mesh`] + [`Instance`] per `PATCH` x `PATCH` cells (frustum culling granularity). The patch's
//!   `(PATCH+1)^2` vertices are shared by all base submeshes (uv = lattice coordinates, tile textures repeat).
//! * Base: every cell is drawn opaque with its own tile texture (grouped per texture), as the client does
//!   (`FUN_10037957`, DisplaySystem @0x10037957, samples the same tile textures per cell).
//! * Transitions: the client has no blend masks (1010021/1010022 are only the 128/64 pixel mip sets of 1010006);
//!   edges are authored in the tile art. To hide the 4 m grid steps we additionally draw, over every cell, its
//!   neighbours' textures as alpha-blended overlays. Vertex weight `w_M(v)` = fraction of the up to four cells around
//!   vertex `v` that use texture `M`; a cell with base texture `T` overlays each other texture `M` (ascending texture id)
//!   with alpha `w_M / (w_T + sum of w_M' of the overlays up to M)`. Over the opaque base this reproduces
//!   `sum_M w_M(v) * tex_M` exactly at every vertex (continuous across cell and patch borders).
//! * Shadows: rdb 1000007 layer `SUN_LAYER` (noon) multiplies the vertex colour (see `shadow`). The map already contains
//!   hill shading; the geometric normals are kept anyway (the renderer flips normals that face away from the eye, so
//!   flat "up" normals turn hills above the camera dark).

use std::collections::BTreeMap;

use anyhow::Result;
use ao_rdb::RecordStore;
use ao_scene::{Blend, Instance, Mesh, Scene, Submesh, TextureKey, Vertex, IDENTITY};

use super::ground::Tilemap;
use super::shadow;

/// Ground tile texture quality used for terrain (1010006: 256², 1010021: 128², 1010022: 64²).
const TILE_TEXTURES: u32 = 1_010_021;
const SHADOWS: u32 = 1_000_007;
/// Cells per patch edge.
const PATCH: usize = 64;
/// Ground shadow map layer used (noon: the brightest of the five sun positions, see `shadow`).
const SUN_LAYER: usize = 2;
/// Brightness kept in full shadow (calibration knob; the client's lightmap blend factor is not known).
const SHADOW_FLOOR: f32 = 0.35;
/// Overlays are lifted by this much to stay above the coplanar base (the renderer also biases blended depth).
const LIFT: f32 = 0.01;
const NONE: u16 = u16::MAX;

fn texture_key(store: &RecordStore, scene: &mut Scene, id: u16) -> Option<TextureKey> {
    let key = TextureKey { rdb_type: TILE_TEXTURES, id: id as u32 };
    if !scene.textures.contains_key(&key) {
        let b = store.get(TILE_TEXTURES, id as u32).ok()??;
        let at = b.windows(3).position(|w| w == [0xff, 0xd8, 0xff])?; // 24 byte header before the JPEG
        scene.textures.insert(key, crate::texture::decode_texture(&b[at..]).ok()?);
    }
    Some(key)
}

/// Texture weights at a lattice vertex: the cells around it, `(texture, count)`.
fn weights(tex: &[u16], cx: usize, cz: usize, vx: usize, vz: usize) -> ([(u16, u8); 4], usize, f32) {
    let mut w = [(NONE, 0u8); 4];
    let mut n = 0;
    let mut total = 0.0;
    for (dx, dz) in [(1, 1), (0, 1), (1, 0), (0, 0)] {
        let (Some(x), Some(z)) = ((vx + dx).checked_sub(1), (vz + dz).checked_sub(1)) else { continue };
        if x >= cx || z >= cz {
            continue;
        }
        let t = tex[z * cx + x];
        total += 1.0;
        match w[..n].iter_mut().find(|e| e.0 == t) {
            Some(e) => e.1 += 1,
            None => {
                w[n] = (t, 1);
                n += 1;
            }
        }
    }
    (w, n, total)
}

fn weight_of(w: &[(u16, u8); 4], n: usize, total: f32, t: u16) -> f32 {
    w[..n].iter().find(|e| e.0 == t).map_or(0.0, |e| e.1 as f32 / total)
}

pub fn build(store: &RecordStore, id: u32, tm: &Tilemap, scene: &mut Scene) -> Result<()> {
    let (cx, cz, cs) = (tm.cells_x, tm.cells_z, tm.cell_size);
    // texture id per cell
    let tex: Vec<u16> = (0..cz).flat_map(|z| (0..cx).map(move |x| (x, z))).map(|(x, z)| tm.tile_texture.get(tm.tile(x, z) as usize).copied().unwrap_or(NONE)).collect();
    let mut keys: BTreeMap<u16, Option<TextureKey>> = BTreeMap::new();
    for &t in &tex {
        if t != NONE && !keys.contains_key(&t) {
            keys.insert(t, texture_key(store, scene, t));
        }
    }
    let shade = store.get(SHADOWS, id).ok().flatten().and_then(|d| shadow::parse(&d).ok()).and_then(|l| l.into_iter().nth(SUN_LAYER));
    let normal = |x: usize, z: usize| -> [f32; 3] {
        let (x0, x1) = (x.saturating_sub(1), (x + 1).min(tm.verts_x - 1));
        let (z0, z1) = (z.saturating_sub(1), (z + 1).min(tm.verts_z - 1));
        let dx = (tm.height(x1, z) - tm.height(x0, z)) / ((x1 - x0) as f32 * cs);
        let dz = -(tm.height(x, z1) - tm.height(x, z0)) / ((z1 - z0) as f32 * cs); // scene z = -z
        let l = (dx * dx + 1.0 + dz * dz).sqrt();
        [-dx / l, 1.0 / l, -dz / l]
    };
    // one texel per 2 cells along x, one per cell along z (see `shadow`)
    let lo = shade.as_ref().map_or(1.0, |s| s.percentile(0.9).max(0.05));
    // flat lit ground (~90th percentile of the map) = full brightness; shadows keep SHADOW_FLOOR of it
    let lit = |x: usize, z: usize| shade.as_ref().map_or(1.0, |s| SHADOW_FLOOR + (1.0 - SHADOW_FLOOR) * (s.sample(x as f32 * 0.5, z as f32) / lo).min(1.0));
    let vertex = |x: usize, z: usize, a: f32, lift: f32| {
        let l = lit(x, z);
        Vertex { pos: [x as f32 * cs, tm.height(x, z) + lift, -(z as f32 * cs)], normal: normal(x, z), uv: [x as f32, z as f32], color: [l, l, l, a] }
    };
    // CCW seen from +Y in scene space (z negated); bit 14 of the tile value picks the diagonal.
    let quad = |x: usize, z: usize, b: u32| {
        if tm.diagonal_p10_p01(x, z) {
            [b, b + 1, b + 2, b + 1, b + 3, b + 2]
        } else {
            [b, b + 1, b + 3, b, b + 3, b + 2]
        }
    };
    for pz in (0..cz).step_by(PATCH) {
        for px in (0..cx).step_by(PATCH) {
            let (ex, ez) = ((px + PATCH).min(cx), (pz + PATCH).min(cz));
            let (nx, nz) = (ex - px + 1, ez - pz + 1);
            let mut mesh = Mesh::default();
            mesh.vertices = (0..nz).flat_map(|j| (0..nx).map(move |i| (i, j))).map(|(i, j)| vertex(px + i, pz + j, 1.0, 0.0)).collect();
            let mut base: BTreeMap<u16, Vec<u32>> = BTreeMap::new();
            let mut over: BTreeMap<u16, Vec<u32>> = BTreeMap::new();
            for z in pz..ez {
                for x in px..ex {
                    let t = tex[z * cx + x];
                    let b = ((z - pz) * nx + (x - px)) as u32;
                    // lattice corners (b, b+1, b+nx, b+nx+1) -> the quad helper expects 4 consecutive vertices
                    let q = quad(x, z, 0).map(|i| match i {
                        0 => b,
                        1 => b + 1,
                        2 => b + nx as u32,
                        _ => b + nx as u32 + 1,
                    });
                    base.entry(t).or_default().extend(q);
                    if t == NONE {
                        continue;
                    }
                    let corners = [(0, 0), (1, 0), (0, 1), (1, 1)].map(|(dx, dz)| weights(&tex, cx, cz, x + dx, z + dz));
                    let mut mats: Vec<u16> = corners.iter().flat_map(|c| c.0[..c.1].iter().map(|e| e.0)).filter(|&m| m != t && m != NONE).collect();
                    mats.sort_unstable();
                    mats.dedup();
                    let mut acc = [0.0f32; 4];
                    for (k, c) in corners.iter().enumerate() {
                        acc[k] = weight_of(&c.0, c.1, c.2, t);
                    }
                    for m in mats {
                        let mut alpha = [0.0f32; 4];
                        for (k, c) in corners.iter().enumerate() {
                            let w = weight_of(&c.0, c.1, c.2, m);
                            acc[k] += w;
                            alpha[k] = w / acc[k];
                        }
                        if alpha.iter().all(|&a| a < 0.01) {
                            continue;
                        }
                        let first = mesh.vertices.len() as u32;
                        for (k, (dx, dz)) in [(0, 0), (1, 0), (0, 1), (1, 1)].into_iter().enumerate() {
                            mesh.vertices.push(vertex(x + dx, z + dz, alpha[k], LIFT));
                        }
                        over.entry(m).or_default().extend(quad(x, z, first));
                    }
                }
            }
            for (t, indices) in base {
                let texture = keys.get(&t).copied().flatten();
                mesh.submeshes.push(Submesh::new(indices, texture));
            }
            for (t, indices) in over {
                let mut s = Submesh::new(indices, keys.get(&t).copied().flatten());
                s.blend = Blend::AlphaBlend;
                mesh.submeshes.push(s);
            }
            scene.meshes.push(mesh);
            scene.instances.push(Instance { mesh: scene.meshes.len() - 1, transform: IDENTITY });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vertex_weights_count_cells_around_the_vertex() {
        // 2x2 map: textures [1 2 / 1 2]
        let tex = [1, 2, 1, 2];
        let (w, n, total) = weights(&tex, 2, 2, 1, 1); // centre vertex touches all four cells
        assert_eq!(total, 4.0);
        assert_eq!(n, 2);
        assert_eq!((weight_of(&w, n, total, 1), weight_of(&w, n, total, 2)), (0.5, 0.5));
        let (w, n, total) = weights(&tex, 2, 2, 0, 0); // corner: only cell (0,0)
        assert_eq!((total, weight_of(&w, n, total, 1)), (1.0, 1.0));
        let (w, n, total) = weights(&tex, 2, 2, 2, 1); // right border: cells (1,0),(1,1)
        assert_eq!((total, weight_of(&w, n, total, 2)), (2.0, 1.0));
    }

    /// Alphas of the sequential overlays reproduce `sum w_M tex_M` regardless of which cell's base is `T`.
    #[test]
    fn sequential_alpha_is_a_partition_of_unity() {
        let w = [(1u16, 0.25f32), (2, 0.5), (3, 0.25)]; // weights at a vertex
        let colour = |m: u16| m as f32 * 10.0;
        let expect: f32 = w.iter().map(|&(m, wm)| wm * colour(m)).sum();
        for base in [1u16, 2, 3] {
            let mut acc = w.iter().find(|e| e.0 == base).unwrap().1;
            let mut c = colour(base);
            for &(m, wm) in w.iter().filter(|e| e.0 != base) {
                acc += wm;
                let a = wm / acc;
                c = c * (1.0 - a) + colour(m) * a;
            }
            assert!((c - expect).abs() < 1e-4, "base {base}: {c} vs {expect}");
        }
    }

    /// Real data (skipped without the client): every playfield record's tail parses completely, the shadow map
    /// size follows the vertex grid (`(verts_x-1)/2` x `verts_z-1`, padded) and the terrain builds with overlays.
    #[test]
    fn real_tails_shadows_and_terrain() {
        use crate::playfield::{environment, record, water};
        let Some(home) = std::env::var_os("HOME") else { return };
        let Ok(store) = RecordStore::open(&std::path::PathBuf::from(home).join("Games/ProjectRubiKa/client")) else { return };
        let Some(raw) = store.get(super::super::RECORD, 566).ok().flatten() else { return };
        for id in store.ids(super::super::RECORD).unwrap() {
            let raw = store.get(super::super::RECORD, id).unwrap().unwrap();
            let rec = record::parse(&raw).unwrap();
            let mut tail = record::Rd::new(&raw, rec.tail);
            water::parse(&mut tail).unwrap();
            environment::parse(&mut tail).unwrap();
        }
        let rec = record::parse(&raw).unwrap();
        let tm = super::super::ground::parse(&store.get(super::super::TILEMAP, rec.tilemap).unwrap().unwrap()).unwrap();
        let layers = shadow::parse(&store.get(SHADOWS, 566).unwrap().unwrap()).unwrap();
        assert_eq!((layers[2].w, layers[2].h), (96, 192));
        assert!(layers[2].w * 2 >= tm.verts_x - 1 && layers[2].h >= tm.verts_z - 1);
        let mut scene = Scene::default();
        build(&store, 566, &tm, &mut scene).unwrap();
        assert!(scene.instances.len() >= 9); // 150 x 150 cells = 3 x 3 patches
        assert!(scene.meshes.iter().flat_map(|m| &m.submeshes).any(|s| s.blend == Blend::AlphaBlend));
    }
}
