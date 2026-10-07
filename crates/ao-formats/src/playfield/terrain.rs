//! Outdoor terrain mesh: authored tile orientations and baked ground shadows.
//!
//! * One [`Mesh`] + [`Instance`] per `PATCH` x `PATCH` cells (frustum culling granularity).
//! * Four vertices per cell carry normalized, oriented UVs; opaque submeshes batch by texture.
//!   DisplaySystem `FUN_100374c5` rotates the authored tile art, without neighbour blending.
//! * Shadows: rdb 1000007, the client's day-time blend of two layers (`shadow::at_time`), multiplies the vertex colour (see `shadow`). The map already contains
//!   hill shading; the geometric normals are kept anyway (the renderer flips normals that face away from the eye, so
//!   flat "up" normals turn hills above the camera dark).

use std::collections::BTreeMap;

use anyhow::Result;
use ao_rdb::RecordStore;
use ao_scene::{Environment, Instance, Mesh, Scene, Submesh, TextureKey, Vertex, IDENTITY};

use super::ground::Tilemap;
use super::shadow;

/// Ground tile texture quality used for terrain (1010006: 256², 1010021: 128², 1010022: 64²).
const TILE_TEXTURES: u32 = 1_010_021;
const SHADOWS: u32 = 1_000_007;
/// Cells per patch edge.
const PATCH: usize = 64;
const NONE: u16 = u16::MAX;

fn texture_key(store: &RecordStore, scene: &mut Scene, id: u16) -> Option<TextureKey> {
    let key = TextureKey { rdb_type: TILE_TEXTURES, id: id as u32 };
    if let std::collections::hash_map::Entry::Vacant(e) = scene.textures.entry(key) {
        let b = store.get(TILE_TEXTURES, id as u32).ok()??;
        // 24 zero bytes, then a JPEG (417 tiles) or an RGBA PNG (432 Shadowlands tiles, alpha always 255)
        e.insert(crate::texture::decode_texture(b.get(24..)?).ok()?);
    }
    Some(key)
}

/// Source coordinates for a destination cell coordinate (DisplaySystem `FUN_100374c5`).
fn tile_uv(raw: u16, [u, v]: [f32; 2]) -> [f32; 2] {
    match raw >> 14 {
        0 => [u, 1.0 - v],
        1 => [1.0 - v, 1.0 - u],
        2 => [1.0 - u, v],
        _ => [v, u],
    }
}

pub fn build(store: &RecordStore, id: u32, tm: &Tilemap, scene: &mut Scene, day_time: f32) -> Result<()> {
    let (cx, cz, cs) = (tm.cells_x, tm.cells_z, tm.cell_size);
    // texture id per cell
    let mut tex: Vec<u16> = (0..cz).flat_map(|z| (0..cx).map(move |x| (x, z))).map(|(x, z)| tm.tile_texture.get(tm.tile(x, z) as usize).copied().unwrap_or(NONE)).collect();
    let mut keys: BTreeMap<u16, TextureKey> = BTreeMap::new();
    let mut missing = std::collections::BTreeSet::new();
    for &t in &tex {
        if t != NONE && !keys.contains_key(&t) && !missing.contains(&t) {
            match texture_key(store, scene, t) {
                Some(k) => drop(keys.insert(t, k)),
                None => drop(missing.insert(t)),
            }
        }
    }
    // Cells whose tile texture does not exist stay untextured.
    tex.iter_mut().filter(|t| missing.contains(t)).for_each(|t| *t = NONE);
    let shade = store.get(SHADOWS, id).ok().flatten().and_then(|d| shadow::parse(&d).ok()).map(|l| shadow::at_time(&l, super::sky::ground_shadow_time(day_time)));
    let normal = |x: usize, z: usize| -> [f32; 3] {
        let (x0, x1) = (x.saturating_sub(1), (x + 1).min(tm.verts_x - 1));
        let (z0, z1) = (z.saturating_sub(1), (z + 1).min(tm.verts_z - 1));
        let dx = (tm.height(x1, z) - tm.height(x0, z)) / ((x1 - x0) as f32 * cs);
        let dz = -(tm.height(x, z1) - tm.height(x, z0)) / ((z1 - z0) as f32 * cs); // scene z = -z
        let l = (dx * dx + 1.0 + dz * dz).sqrt();
        [-dx / l, 1.0 / l, -dz / l]
    };
    // one texel per 2 cells along x, one per cell along z (see `shadow`)
    // With a shadow map the ground is `tile * saturate(palette + sun * N.L + lights)` (see `shadow::palette`), which the
    // renderer's `prelit` mode (`saturate(vertex.rgb + 0.8 * ambient + lights)`, all in the client's gamma space) evaluates when
    // the vertex colour is `palette + sun * N.L - 0.8 * ambient` (computed here in gamma space, interpolated by the Gouraud
    // stage like the engine's vertex lighting).
    let env = scene.environment.unwrap_or(Environment { sky_color: [0.0; 3], fog_color: [0.0; 3], fog_start: 0.0, fog_end: 1.0, ambient: [1.0; 3], sun_color: [0.0; 3], sun_dir: [0.0, 1.0, 0.0], sun_specular: 1.0 });
    let (sun, ambient) = (env.sun_color.map(super::environment::linear_to_srgb), env.ambient.map(super::environment::linear_to_srgb));
    let prelit = shade.is_some();
    let vertex = |x: usize, z: usize, uv: [f32; 2]| {
        let n = normal(x, z);
        let color = match &shade {
            Some(s) => {
                let ndl = (n[0] * env.sun_dir[0] + n[1] * env.sun_dir[1] + n[2] * env.sun_dir[2]).max(0.0);
                let p = shadow::palette(s.sample(x as f32 * 0.5, z as f32));
                let c = [0, 1, 2].map(|c| p + sun[c] * ndl - 0.8 * ambient[c]);
                [c[0], c[1], c[2], 1.0]
            }
            None => [1.0, 1.0, 1.0, 1.0],
        };
        Vertex { pos: [x as f32 * cs, tm.height(x, z), -(z as f32 * cs)], normal: n, uv, color }
    };
    // Outdoor checkerboard diagonal (N3 FUN_10017800), independent of texture orientation.
    // CCW seen from +Y in scene space (z negated).
    let quad = |x: usize, z: usize, b: u32| {
        if (!z ^ x) & 1 == 0 {
            [b, b + 1, b + 2, b + 1, b + 3, b + 2]
        } else {
            [b, b + 1, b + 3, b, b + 3, b + 2]
        }
    };
    for pz in (0..cz).step_by(PATCH) {
        for px in (0..cx).step_by(PATCH) {
            let (ex, ez) = ((px + PATCH).min(cx), (pz + PATCH).min(cz));
            let mut mesh = Mesh { vertices: Vec::with_capacity((ex - px) * (ez - pz) * 4), ..Default::default() };
            let mut base: BTreeMap<u16, Vec<u32>> = BTreeMap::new();
            for z in pz..ez {
                for x in px..ex {
                    let t = tex[z * cx + x];
                    let b = mesh.vertices.len() as u32;
                    let raw = tm.tiles[z * (tm.verts_x - 1) + x];
                    for (dx, dz) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                        mesh.vertices.push(vertex(x + dx, z + dz, tile_uv(raw, [dx as f32, dz as f32])));
                    }
                    base.entry(t).or_default().extend(quad(x, z, b));
                }
            }
            for (t, indices) in base {
                let texture = keys.get(&t).copied();
                let mut s = Submesh::new(indices, texture);
                s.prelit = prelit;
                // AnarchyGround constructor FUN_10033b97: texture state 12 = 3 (CLAMP).
                s.texture_clamp = true;
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
    fn native_tile_uv_orientations() {
        let corners = [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]];
        let expected = [
            [[0.0, 1.0], [1.0, 1.0], [0.0, 0.0], [1.0, 0.0]],
            [[1.0, 1.0], [1.0, 0.0], [0.0, 1.0], [0.0, 0.0]],
            [[1.0, 0.0], [0.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
            [[0.0, 0.0], [0.0, 1.0], [1.0, 0.0], [1.0, 1.0]],
        ];
        for (orientation, expected) in expected.iter().enumerate() {
            assert_eq!(corners.map(|uv| tile_uv((orientation as u16) << 14 | 37, uv)), *expected);
        }
        assert_eq!(tile_uv(0x4000, [0.25, 0.75]), [0.25, 0.75]);
    }

    /// Real data (skipped without the client): every playfield record's tail parses completely, the shadow map
    /// size follows the vertex grid (`(verts_x-1)/2` x `verts_z-1`, padded).
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
        build(&store, 566, &tm, &mut scene, crate::playfield::DEFAULT_DAY_TIME).unwrap();
        assert!(scene.instances.len() >= 9); // 150 x 150 cells = 3 x 3 patches
        assert!(scene.meshes.iter().flat_map(|m| &m.submeshes).all(|s| s.blend != ao_scene::Blend::AlphaBlend && s.texture_clamp));
    }

    #[test]
    fn installed_pf4582_authored_terrain() {
        use crate::playfield::record;
        let Some(home) = std::env::var_os("HOME") else { return };
        let Ok(store) = RecordStore::open(&std::path::PathBuf::from(home).join("Games/ProjectRubiKa/client")) else { return };
        let raw = store.get(super::super::RECORD, 4582).unwrap().unwrap();
        let rec = record::parse(&raw).unwrap();
        let tm = super::super::ground::parse(&store.get(super::super::TILEMAP, rec.tilemap).unwrap().unwrap()).unwrap();
        let mut scene = Scene::default();
        build(&store, 4582, &tm, &mut scene, crate::playfield::DEFAULT_DAY_TIME).unwrap();
        assert!(!scene.meshes.is_empty());
        let mut patches = scene.meshes.iter();
        for pz in (0..tm.cells_z).step_by(PATCH) {
            for px in (0..tm.cells_x).step_by(PATCH) {
                let mesh = patches.next().unwrap();
                let (ex, ez) = ((px + PATCH).min(tm.cells_x), (pz + PATCH).min(tm.cells_z));
                assert_eq!(mesh.vertices.len(), (ex - px) * (ez - pz) * 4);
                assert!(mesh.submeshes.iter().all(|s| s.blend != ao_scene::Blend::AlphaBlend && s.texture_clamp));
                let quads: BTreeMap<_, _> = mesh.submeshes.iter().flat_map(|s| s.indices.as_chunks::<6>().0.iter().map(move |q| (q[0], (s.texture, q)))).collect();
                assert_eq!(quads.len(), (ex - px) * (ez - pz));
                for z in pz..ez {
                    for x in px..ex {
                        let b = ((z - pz) * (ex - px) + x - px) * 4;
                        let raw = tm.tiles[z * (tm.verts_x - 1) + x];
                        for (k, (dx, dz)) in [(0, 0), (1, 0), (0, 1), (1, 1)].into_iter().enumerate() {
                            let v = &mesh.vertices[b + k];
                            assert_eq!(v.uv, tile_uv(raw, [dx as f32, dz as f32]));
                            assert_eq!(v.pos, [(x + dx) as f32 * tm.cell_size, tm.height(x + dx, z + dz), -((z + dz) as f32 * tm.cell_size)]);
                            assert_eq!(v.color[3], 1.0);
                        }
                        let b = b as u32;
                        // Native P0-P2 on even checkerboard cells, P1-P3 on odd cells.
                        let expected = if (x + z).is_multiple_of(2) { [b, b + 1, b + 3, b, b + 3, b + 2] } else { [b, b + 1, b + 2, b + 1, b + 3, b + 2] };
                        let t = tm.tile_texture[tm.tile(x, z) as usize] as u32;
                        let (texture, indices) = quads.get(&b).unwrap();
                        assert_eq!(*texture, Some(TextureKey { rdb_type: TILE_TEXTURES, id: t }));
                        assert_eq!(**indices, expected);
                    }
                }
            }
        }
        assert!(patches.next().is_none());
    }
}
