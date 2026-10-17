//! Outdoor terrain mesh: authored tile orientations and baked ground shadows.
//!
//! * One [`Mesh`] + [`Instance`] per `PATCH` x `PATCH` cells (frustum culling granularity).
//! * Four vertices per cell carry normalized, oriented UVs; opaque submeshes batch by texture.
//!   DisplaySystem `FUN_100374c5` rotates the authored tile art, without neighbour blending.
//! * Shadows: rdb 1000007, the day-time blend of two layers (`shadow::at_time`),
//!   supplies the native independent palette texture stage (`shadow::palette`).

use std::collections::BTreeMap;

use anyhow::Result;
use ao_rdb::RecordStore;
use ao_scene::{Instance, Mesh, Scene, Submesh, TextureKey, Vertex, IDENTITY};

use super::ground::Tilemap;
use super::shadow;

const SHADOWS: u32 = 1_000_007;

const NONE: u16 = u16::MAX;

fn texture_key(store: &RecordStore, scene: &mut Scene, id: u16, texture_type: u32) -> Option<TextureKey> {
    let key = TextureKey { rdb_type: texture_type, id: id as u32 };
    if let std::collections::hash_map::Entry::Vacant(e) = scene.textures.entry(key) {
        let b = store.get(texture_type, id as u32).ok()??;
        // 24 zero bytes, then a JPEG (417 tiles) or an RGBA PNG (432 Shadowlands tiles, alpha always 255)
        let mut texture = crate::texture::decode_texture(b.get(24..)?).ok()?;
        texture.source = match texture.source {
            ao_scene::TextureSource::Rgba | ao_scene::TextureSource::PngRgba => ao_scene::TextureSource::GroundRgba,
            _ => ao_scene::TextureSource::GroundRgb,
        };
        e.insert(texture);
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


fn patch_cells(tm: &Tilemap) -> usize {
    1usize << (tm.verts_x - 1).trailing_zeros().min((tm.verts_z - 1).trailing_zeros()).min(6)
}

/// Native tile mip arithmetic is shared with the renderer's resource policy.
fn tile_mip(texture: &ao_scene::Texture) -> [[u8; 4]; 64] {
    ao_scene::ground_texture::tile_mip(texture, 2)
}

fn composite_tile_pixel(raw: u16, x: usize, y: usize) -> usize {
    ao_scene::ground_texture::oriented_pixel(raw >> 14, x, y)
}


fn composite_source(tm: &Tilemap, scene: &Scene) -> ao_scene::TextureSource {
    // DS 100374c5 initializes its pixel format by loading tile index zero.
    let alpha = tm.tile_texture.first().is_some_and(|&id| scene.textures.iter().any(|(key, t)|
        key.id == id as u32 && matches!(t.source, ao_scene::TextureSource::GroundRgba)));
    if alpha { ao_scene::TextureSource::GroundComposite1555 } else { ao_scene::TextureSource::GroundComposite565 }
}

fn composite_patch(tm: &Tilemap, px: usize, pz: usize, cells: usize, mips: &BTreeMap<u16, [[u8; 4]; 64]>, source: ao_scene::TextureSource) -> ao_scene::Texture {
    let side = cells * 8;
    let mut texture = ao_scene::Texture { source, width: side as u32, height: side as u32, rgba: vec![255; side * side * 4] };
    let format = if source == ao_scene::TextureSource::GroundComposite1555 { ao_scene::ground_texture::Format::Rgba1555 } else { ao_scene::ground_texture::Format::Rgb565 };
    for z in pz..pz + cells { for x in px..px + cells {
        let raw = tm.tiles[z * (tm.verts_x - 1) + x];
        let Some(id) = tm.tile_texture.get(tm.tile(x, z) as usize) else { continue };
        let Some(mip) = mips.get(id) else { continue };
        for y in 0..8 { for u in 0..8 {
            let dst = (((z - pz) * 8 + y) * side + (x - px) * 8 + u) * 4;
            texture.rgba[dst..dst + 4].copy_from_slice(&format.pixel(mip[composite_tile_pixel(raw, u, y)]));
        }}
    }}
    texture
}

/// Reload only ground images after native SetTextureQuality invalidation (10037b9d).
/// Patch mesh IDs are retained from the installed scene; unrelated meshes/actors stay untouched.
pub fn refresh_textures(store: &RecordStore, tm: &Tilemap, patches: &[(usize, [i32; 2], usize)]) -> std::collections::HashMap<TextureKey, ao_scene::Texture> {
    let mut scene = Scene::default();
    let mut mips = BTreeMap::new();
    for &id in &tm.tile_texture {
        if mips.contains_key(&id) { continue }
        if let Some(key) = texture_key(store, &mut scene, id, 1_010_006) {
            mips.insert(id, tile_mip(&scene.textures[&key]));
        }
    }
    let source = composite_source(tm, &scene);
    for &(mesh, [x, z], cells) in patches {
        scene.textures.insert(TextureKey { rdb_type: 0, id: mesh as u32 }, composite_patch(tm, x as usize, z as usize, cells, &mips, source));
    }
    scene.textures
}
pub fn build(store: &RecordStore, id: u32, tm: &Tilemap, scene: &mut Scene, shadow_time: f32) -> Result<()> {
    let (cx, cz, cs) = (tm.verts_x - 1, tm.verts_z - 1, tm.cell_size);
    let patch = patch_cells(tm);
    // texture id per cell
    let mut tex: Vec<u16> = (0..cz).flat_map(|z| (0..cx).map(move |x| (x, z))).map(|(x, z)| tm.tile_texture.get(tm.tile(x, z) as usize).copied().unwrap_or(NONE)).collect();
    let mut keys: BTreeMap<u16, TextureKey> = BTreeMap::new();
    let mut missing = std::collections::BTreeSet::new();
    for &t in &tex {
        if t != NONE && !keys.contains_key(&t) && !missing.contains(&t) {
            match texture_key(store, scene, t, 1_010_006) {
                Some(k) => drop(keys.insert(t, k)),
                None => drop(missing.insert(t)),
            }
        }
    }
    // Cells whose tile texture does not exist stay untextured.
    tex.iter_mut().filter(|t| missing.contains(t)).for_each(|t| *t = NONE);
    if let Some(&first) = tm.tile_texture.first() { texture_key(store, scene, first, 1_010_006); }
    let mips: BTreeMap<_, _> = keys.iter().map(|(&id, key)| (id, tile_mip(&scene.textures[key]))).collect();
    let composite_source = composite_source(tm, scene);
    let shade = store.get(SHADOWS, id).ok().flatten().and_then(|d| shadow::parse(&d).ok()).map(|l| shadow::at_time(&l, shadow_time));
    let shadow_uv_scale = shade.as_ref().map_or([0.0; 2], |s| [0.5 / (cs * s.w as f32), 1.0 / (cs * s.h as f32)]);
    let shadow_texture = shade.as_ref().map(|s| {
        let key = TextureKey { rdb_type: SHADOWS, id };
        let rgba = s.px.iter().flat_map(|&v| {
            let p = (shadow::palette(v as f32 / 255.0) * 255.0).round() as u8;
            [p, p, p, 255]
        }).collect();
        scene.textures.insert(key, ao_scene::Texture { width: s.w as u32, height: s.h as u32, rgba, source: Default::default() });
        key
    });
    let normal = |x: usize, z: usize| -> [f32; 3] {
        let (x0, x1) = (x.saturating_sub(1), (x + 1).min(tm.verts_x - 1));
        let (z0, z1) = (z.saturating_sub(1), (z + 1).min(tm.verts_z - 1));
        let dx = (tm.height(x1, z) - tm.height(x0, z)) / ((x1 - x0) as f32 * cs);
        let dz = -(tm.height(x, z1) - tm.height(x, z0)) / ((z1 - z0) as f32 * cs); // scene z = -z
        let l = (dx * dx + 1.0 + dz * dz).sqrt();
        [-dx / l, 1.0 / l, -dz / l]
    };
    // The native palette remains a texture stage, independent of Gouraud lighting.
    let vertex = |x: usize, z: usize, uv: [f32; 2]| {
        let n = normal(x, z);
        let color = [1.0; 4];
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
    for pz in (0..cz).step_by(patch) {
        for px in (0..cx).step_by(patch) {
            let (ex, ez) = (px + patch, pz + patch);
            let mut mesh = Mesh { vertices: Vec::with_capacity(patch * patch * 4 + (patch + 1) * (patch + 1)), ..Default::default() };
            let mut base: BTreeMap<u16, Vec<u32>> = BTreeMap::new();
            let mut fine_cells = Vec::with_capacity(patch * patch);
            let composite = composite_patch(tm, px, pz, patch, &mips, composite_source);
            for z in pz..ez {
                for x in px..ex {
                    let t = tex[z * cx + x];
                    let b = mesh.vertices.len() as u32;
                    let raw = tm.tiles[z * (tm.verts_x - 1) + x];
                    for (dx, dz) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                        mesh.vertices.push(vertex(x + dx, z + dz, tile_uv(raw, [dx as f32, dz as f32])));
                    }
                    base.entry(t).or_default().extend(quad(x, z, b));
                    fine_cells.push((t, raw, [b, b + 1, b + 2, b + 3]));
                }
            }
            let mut material_indices = BTreeMap::new();
            for (t, indices) in base {
                material_indices.insert(t, mesh.submeshes.len());
                let texture = keys.get(&t).copied();
                let mut s = Submesh::new(indices, texture);

                // AnarchyGround constructor FUN_10033b97: texture state 12 = 3 (CLAMP).
                s.texture_clamp = true;
                mesh.submeshes.push(s);
            }
            let fine_cells = fine_cells.into_iter().map(|(t, raw, vertices)| ao_scene::ground_detail::GroundCell { submesh: material_indices[&t], vertices, native_tile: t, ground_material: super::ground::ground_material(t) as u8, orientation: (raw >> 14) as u8 }).collect();
            let lattice_start = mesh.vertices.len() as u32;
            let mut heights = Vec::with_capacity((patch + 1) * (patch + 1));
            for z in pz..=ez { for x in px..=ex {
                mesh.vertices.push(vertex(x, z, [(x - px) as f32 / patch as f32, (z - pz) as f32 / patch as f32]));
                heights.push(tm.heights[z * tm.verts_x + x]);
            }}
            let key = TextureKey { rdb_type: 0, id: scene.meshes.len() as u32 };
            scene.textures.insert(key, composite);
            let coarse_submesh = mesh.submeshes.len();
            // Reserve the maximum index count; the native adaptive stage replaces it before drawing.
            let mut coarse = Submesh::new(vec![lattice_start; patch * patch * 6], Some(key));
            coarse.texture_clamp = true;
            mesh.submeshes.push(coarse);
            scene.ground_detail.push(ao_scene::ground_detail::GroundPatch { mesh: scene.meshes.len(), origin: [px as i32, pz as i32], cells: patch, cell_size: cs, height_scale: tm.height_scale, heights, lattice_start, coarse_submesh, fine_cells, shadow_texture, shadow_uv_scale });
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
    fn native_composite_orientation() {
        let texture = ao_scene::Texture { source: Default::default(), width: 8, height: 8, rgba: (0..64).flat_map(|v| [v, v, v, 255]).collect() };
        let mip = tile_mip(&texture);
        for orientation in 0..4 {
            for y in 0..8 { for x in 0..8 {
                let raw = orientation << 14;
                let uv = tile_uv(raw, [(x as f32 + 0.5) / 8.0, (y as f32 + 0.5) / 8.0]);
                assert_eq!(mip[composite_tile_pixel(raw, x, y)][0], (uv[1] * 8.0) as u8 * 8 + (uv[0] * 8.0) as u8);
            }}
        }
    }

    #[test]
    fn installed_ground_quality_refresh_changes_current_payload_and_keeps_patch_keys() {
        let Some(home) = std::env::var_os("HOME") else { return };
        let Ok(store) = RecordStore::open(&std::path::PathBuf::from(home).join("Games/ProjectRubiKa/client")) else { return };
        let raw = store.get(super::super::RECORD, 4582).unwrap().unwrap();
        let rec = super::super::record::parse(&raw).unwrap();
        let tm = super::super::ground::parse(&store.get(super::super::TILEMAP, rec.tilemap).unwrap().unwrap()).unwrap();
        let cells = patch_cells(&tm);
        let patches = [(37, [0, 0], cells)];
        let tile = tm.tile_texture[tm.tile(0, 0) as usize] as u32;
        let key = TextureKey { rdb_type: 1_010_006, id: tile };
        let composite = TextureKey { rdb_type: 0, id: 37 };
        let mut payloads = Vec::new();
        for (quality, width) in [(0, 64), (1, 128), (2, 256)] {
            store.set_ground_texture_quality(quality);
            let textures = refresh_textures(&store, &tm, &patches);
            assert_eq!(textures[&key].width, width);
            assert_eq!(textures[&composite].width, (cells * 8) as u32);
            assert!(!textures.contains_key(&TextureKey { rdb_type: 0, id: 0 }));
            payloads.push(textures[&key].rgba.clone());
        }
        assert_ne!(payloads[0], payloads[1]);
        assert_ne!(payloads[1], payloads[2]);
    }

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
        assert_eq!(scene.instances.len(), ((tm.verts_x - 1) / patch_cells(&tm)) * ((tm.verts_z - 1) / patch_cells(&tm)));
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
        build(&store, 4582, &tm, &mut scene, super::super::sky::ground_shadow_time(crate::playfield::DEFAULT_DAY_TIME)).unwrap();
        assert!(!scene.meshes.is_empty());
        assert_eq!(scene.meshes.len(), scene.ground_detail.len());
        let mut patches = scene.meshes.iter().zip(&scene.ground_detail).enumerate();
        let patch = patch_cells(&tm);
        for pz in (0..tm.verts_z - 1).step_by(patch) {
            for px in (0..tm.verts_x - 1).step_by(patch) {
                let (mesh_index, (mesh, detail)) = patches.next().unwrap();
                assert_eq!(detail.mesh, mesh_index);
                assert_eq!(detail.origin, [px as i32, pz as i32]);
                assert_eq!(detail.cells, patch);
                assert_eq!(detail.height_scale, tm.height_scale);
                assert_eq!(detail.fine_cells.len(), patch * patch);
                let (ex, ez) = (px + patch, pz + patch);
                assert_eq!(mesh.vertices.len(), patch * patch * 4 + (patch + 1) * (patch + 1));
                assert!(mesh.submeshes.iter().all(|s| s.blend != ao_scene::Blend::AlphaBlend && s.texture_clamp));
                let quads: BTreeMap<_, _> = mesh.submeshes[..mesh.submeshes.len() - 1].iter().flat_map(|s| s.indices.as_chunks::<6>().0.iter().map(move |q| (q[0], (s.texture, q)))).collect();
                assert_eq!(quads.len(), (ex - px) * (ez - pz));
                for z in pz..ez {
                    for x in px..ex {
                        let b = ((z - pz) * (ex - px) + x - px) * 4;
                        let raw = tm.tiles[z * (tm.verts_x - 1) + x];
                        let cell = &detail.fine_cells[(z - pz) * patch + x - px];
                        let t = tm.tile_texture[tm.tile(x, z) as usize];
                        assert_eq!(cell.native_tile, t);
                        assert_eq!(cell.ground_material, super::super::ground::ground_material(t) as u8);
                        assert_eq!(cell.orientation, (raw >> 14) as u8);
                        assert_eq!(cell.vertices, [b as u32, b as u32 + 1, b as u32 + 2, b as u32 + 3]);
                        for (k, (dx, dz)) in [(0, 0), (1, 0), (0, 1), (1, 1)].into_iter().enumerate() {
                            let v = &mesh.vertices[b + k];
                            assert_eq!(v.uv, tile_uv(raw, [dx as f32, dz as f32]));
                            assert_eq!(v.pos, [(x + dx) as f32 * tm.cell_size, tm.height(x + dx, z + dz), -((z + dz) as f32 * tm.cell_size)]);
                            assert_eq!(v.color[3], 1.0);
                            assert_eq!(mesh.vertices[cell.vertices[k] as usize].pos, v.pos);
                        }
                        let b = b as u32;
                        // Native P0-P2 on even checkerboard cells, P1-P3 on odd cells.
                        let expected = if (x + z).is_multiple_of(2) { [b, b + 1, b + 3, b, b + 3, b + 2] } else { [b, b + 1, b + 2, b + 1, b + 3, b + 2] };
                        let t = t as u32;
                        let (texture, indices) = quads.get(&b).unwrap();
                        assert_eq!(*texture, Some(TextureKey { rdb_type: 1_010_006, id: t }));
                        assert_eq!(mesh.submeshes[cell.submesh].texture, *texture);
                        assert_eq!(**indices, expected);
                    }
                }
            }
        }
        assert!(patches.next().is_none());
    }
}
