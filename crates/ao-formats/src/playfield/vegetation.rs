//! Enhanced-grass exclusion metadata, never a modification of native geometry.
use super::{spawn, Placed};
use crate::character::NameTable;
use ao_scene::Scene;
use std::collections::HashMap;

fn plant_name(name: &str) -> bool {
    // Native rdb 1000010/1 names include athens_grasspatch3, grass_tust,
    // startupc_gophgrass, jungle_fernthinstriped1 and shadowlands_shrub1_new.
    // Split numeric suffixes too: substring searches also match inferno,
    // implants, breed and nograss, none of which identify plant geometry.
    name.split(|c: char| !c.is_ascii_alphabetic()).any(|word| {
        ["grass", "grasspatch", "grassypatch", "gophgrass", "rockgrassaround",
         "fern", "fernthinstriped", "shrub", "shrubs", "bush", "bushes",
         "bigbush", "stickbush", "groundplant", "desertplant", "tallplants",
         "weed", "reed", "plant", "glowplant", "pinkplant", "bigpinkplant",
         "redblobplant", "palmplant", "rootplant", "rootplants", "flower",
         "flowers", "yellowflower", "petalplantpink", "petalplantblue",
         "petalplantpurple"].iter().any(|plant| word.eq_ignore_ascii_case(plant))
    })
}

fn overlaps(a: &spawn::Box3, b: &spawn::Box3) -> bool {
    a.0[0] <= b.1[0] && a.1[0] >= b.0[0] && a.0[2] <= b.1[2] && a.1[2] >= b.0[2]
}

// This half-metre slab is an Enhanced exclusion policy, not a recovered retail
// rule. Clip individual triangles to it: a plant's low stem must not make its
// high canopy mask all the terrain beneath the mesh's overall bounding box.
fn ground_footprint(points: [[f32; 3]; 3], height: &impl Fn(f32, f32) -> Option<f32>) -> Option<spawn::Box3> {
    const GAP: f32 = 0.5;
    let mut distances = [0.0; 3];
    for (i, p) in points.iter().enumerate() {
        distances[i] = p[1] - height(p[0], p[2])?;
    }
    let mut bounds = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
    let mut include = |p: [f32; 3]| {
        for (i, value) in p.into_iter().enumerate() {
            bounds.0[i] = bounds.0[i].min(value);
            bounds.1[i] = bounds.1[i].max(value);
        }
    };
    for i in 0..3 {
        if distances[i].abs() <= GAP { include(points[i]); }
        let j = (i + 1) % 3;
        for edge in [-GAP, GAP] {
            if (distances[i] < edge && distances[j] > edge) || (distances[i] > edge && distances[j] < edge) {
                let t = (edge - distances[i]) / (distances[j] - distances[i]);
                include(std::array::from_fn(|axis| points[i][axis] + t * (points[j][axis] - points[i][axis])));
            }
        }
    }
    bounds.0[0].is_finite().then_some(bounds)
}

pub(super) fn mark(scene: &mut Scene, items: &[Placed], names: &NameTable, zone_lod: bool, height: &impl Fn(f32, f32) -> Option<f32>) {
    let mut bounds = HashMap::new();
    // One entry per full placement, before add_lod appends reduced twins. Global
    // zone references only extend Placed.zones; they do not duplicate coverage.
    for item in items {
        // Native zone LOD never displays an unreferenced global placement.
        if zone_lod && item.zones.is_empty() { continue; }
        if !names.name(crate::mesh::MESH_TYPE, item.mesh).is_some_and(plant_name) { continue; }
        let instance = &scene.instances[item.instance];
        let mesh = &scene.meshes[instance.mesh];
        let Some(local) = *bounds.entry(instance.mesh).or_insert_with(|| spawn::mesh_bounds(mesh)) else { continue };
        let world = spawn::transform_box(&local, &instance.transform);
        for patch in &mut scene.ground_detail {
            let size = patch.cell_size;
            let x0 = patch.origin[0] as f32 * size;
            let z0 = -(patch.origin[1] as f32) * size;
            let patch_box = ([x0, 0.0, z0 - patch.cells as f32 * size], [x0 + patch.cells as f32 * size, 0.0, z0]);
            if size <= 0.0 || !overlaps(&world, &patch_box) { continue; }
            for submesh in &mesh.submeshes {
                for triangle in submesh.indices.as_chunks::<3>().0 {
                    let points = std::array::from_fn(|i| {
                        let p = mesh.vertices[triangle[i] as usize].pos;
                        spawn::transform_box(&(p, p), &instance.transform).0
                    });
                    let Some(footprint) = ground_footprint(points, height) else { continue };
                    if !overlaps(&footprint, &patch_box) { continue; }
                    // Closed footprints include both cells touched by an exact
                    // grid edge, including vertical cards with zero projected
                    // width. No artificial geometry thickness is required.
                    let start_x = (((footprint.0[0] - x0) / size).ceil() - 1.0).max(0.0) as usize;
                    let end_x = (((footprint.1[0] - x0) / size).floor() + 1.0).max(0.0) as usize;
                    let start_z = (((z0 - footprint.1[2]) / size).ceil() - 1.0).max(0.0) as usize;
                    let end_z = (((z0 - footprint.0[2]) / size).floor() + 1.0).max(0.0) as usize;
                    // ponytail: conservative triangle AABBs can omit extra Enhanced
                    // blades at corners; use polygon/cell clipping if that becomes visible.
                    for z in start_z..end_z.min(patch.cells) {
                        for x in start_x..end_x.min(patch.cells) {
                            if let Some(cell) = patch.fine_cells.get_mut(z * patch.cells + x) {
                                cell.authored_vegetation = true;
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ao_scene::{ground_detail::{GroundCell, GroundPatch}, Instance, Mesh, Submesh, Vertex, IDENTITY};

    #[test]
    fn authored_plants_mask_only_transformed_near_ground_coverage_without_changing_native_geometry() {
        // Actual names from native rdb 1000010/1; synthetic IDs deliberately do
        // not coincide with their RDB IDs, so the classification must use names.
        let entries = [(1u32, "athens_grasspatch3.abiff"), (2, "green_fern_small_cluster.abiff"), (3, "shadowlands_kolle2_nograss_inferno.abiff")];
        let mut bytes = Vec::new();
        for word in [7u32, crate::mesh::MESH_TYPE, entries.len() as u32] { bytes.extend(word.to_le_bytes()); }
        for (id, name) in entries {
            bytes.extend(id.to_le_bytes());
            bytes.extend((name.len() as u32).to_le_bytes());
            bytes.extend(name.as_bytes());
        }
        let names = NameTable::parse(&bytes).unwrap();
        let vertex = |pos| Vertex { pos, normal: [0.0, 1.0, 0.0], uv: [0.0; 2], color: [1.0; 4] };
        let mesh = Mesh { vertices: [[0.1, 0.0, -0.1], [0.4, 0.0, -0.1], [0.1, 0.3, -0.4],
            // Broad high foliage in the same mesh must not mask its neighbors.
            [0.0, 8.0, 0.0], [3.0, 8.0, 0.0], [0.0, 8.0, -3.0]].map(vertex).to_vec(),
            submeshes: vec![Submesh::new(vec![0, 1, 2, 3, 4, 5], None)] };
        let mut scene = Scene::default();
        scene.meshes.push(mesh);
        scene.ground_detail.push(GroundPatch { cells: 4, cell_size: 1.0,
            fine_cells: vec![GroundCell::default(); 16], ..Default::default() });
        let mut rotated = IDENTITY;
        // Rotate 90 degrees about Y and scale by 2: local +x becomes world -z.
        rotated[0] = [0.0, 0.0, -2.0, 0.0];
        rotated[1] = [0.0, 2.0, 0.0, 0.0];
        rotated[2] = [2.0, 0.0, 0.0, 0.0];
        // Scaling crosses the z=-3 cell boundary; an unscaled placement cannot.
        rotated[3] = [3.0, 0.0, -2.4, 1.0];
        let mut elevated = IDENTITY;
        elevated[3] = [1.0, 6.0, 0.0, 1.0];
        let mut nonplant = IDENTITY;
        nonplant[3] = [3.0, 0.0, 0.0, 1.0];
        let placements = [(1, IDENTITY), (1, rotated), (2, elevated), (3, nonplant)];
        let mut items: Vec<_> = placements.into_iter().enumerate().map(|(instance, (mesh, transform))| {
            scene.instances.push(Instance { mesh: 0, transform });
            Placed { instance, mesh, attrs: Vec::new(), transform, flag8: false, class: 0, zones: vec![0, 1] }
        }).collect();
        // Axis-aligned vertical cards on x=1 and z=-1 must cover both sides
        // of their grid edge, despite a zero-width projected bounding box.
        scene.meshes.push(Mesh { vertices: [[1.0, 0.0, -0.1], [1.0, 0.0, -0.4], [1.0, 1.0, -0.1],
            [2.2, 0.0, -1.0], [2.6, 0.0, -1.0], [2.2, 1.0, -1.0]].map(vertex).to_vec(),
            submeshes: vec![Submesh::new(vec![0, 1, 2, 3, 4, 5], None)] });
        items.push(Placed { instance: scene.instances.len(), mesh: 1, attrs: Vec::new(), transform: IDENTITY,
            flag8: false, class: 0, zones: vec![0] });
        scene.instances.push(Instance { mesh: 1, transform: IDENTITY });
        let native_vertices = scene.meshes[0].vertices.clone();
        let native_indices = scene.meshes[0].submeshes[0].indices.clone();
        let native_instances = scene.instances.iter().map(|i| (i.mesh, i.transform)).collect::<Vec<_>>();
        mark(&mut scene, &items, &names, true, &|_, _| Some(0.0));
        let occupied: Vec<_> = scene.ground_detail[0].fine_cells.iter().enumerate()
            .filter_map(|(i, cell)| cell.authored_vegetation.then_some(i)).collect();
        assert_eq!(occupied, vec![0, 1, 2, 6, 10, 14], "grass, transformed grass and vertical cards only; bare neighbors, elevated fern and nograss remain clear");
        assert_eq!(scene.meshes[0].vertices, native_vertices);
        assert_eq!(scene.meshes[0].submeshes[0].indices, native_indices);
        assert_eq!(scene.instances.iter().map(|i| (i.mesh, i.transform)).collect::<Vec<_>>(), native_instances);
        assert!(!plant_name("hq_shopbooth_implants.abiff"));
        assert!(plant_name("spikey_green_grass_largecluster.abiff"));
        assert!(plant_name("nature_groundplant.abiff"));
        assert!(plant_name("jungle_fern_2m_hipoly.abiff"));
        assert!(!plant_name("cc_button_breed.abiff"));
        assert!(!plant_name("shadowlands_kolle2_nograss_inferno.abiff"));
    }

    #[test]
    fn installed_icc_and_borealis_authored_plants_cover_unique_grass_cells() {
        let Some(home) = std::env::var_os("HOME") else { return };
        let dir = std::path::PathBuf::from(home).join("Games/ProjectRubiKa/client");
        if !dir.join("cd_image/rdb.db").exists() { return; }
        let store = ao_rdb::RecordStore::open(&dir).unwrap();
        // Enhanced grass's native tile/material admission, independent of
        // exposure, distance, native ground LOD and the new exclusion metadata.
        let eligible = |cell: &GroundCell| matches!((cell.native_tile, cell.ground_material), (2 | 258, 15) | (291 | 685, 6));
        for id in [4582, 800] {
            let (scene, _) = super::super::load_playfield_report(&store, &dir, id).unwrap();
            let mut origins = std::collections::HashSet::new();
            let mut eligible_authored = 0;
            let mut icc_cell = None;
            let target = if id == 4582 { [923.0, -759.0] } else { [621.6, -785.8] };
            let mut eligible_cells = Vec::new();
            for patch in &scene.ground_detail {
                for (i, cell) in patch.fine_cells.iter().enumerate() {
                    let gx = patch.origin[0] + (i % patch.cells) as i32;
                    let gz = patch.origin[1] + (i / patch.cells) as i32;
                    assert!(origins.insert((gx, gz)), "playfield {id}: duplicate terrain cell {gx},{gz}");
                    if eligible(cell) && cell.authored_vegetation { eligible_authored += 1; }
                    if eligible(cell) {
                        let offset = (i / patch.cells) * (patch.cells + 1) + i % patch.cells;
                        let mean_y = [offset, offset + 1, offset + patch.cells + 1, offset + patch.cells + 2]
                            .into_iter().map(|corner| patch.heights[corner] as f32 * patch.height_scale / 256.0).sum::<f32>() / 4.0;
                        eligible_cells.push(([gx, gz], [(gx as f32 + 0.5) * patch.cell_size,
                            mean_y, -(gz as f32 + 0.5) * patch.cell_size], cell.authored_vegetation));
                    }
                    if id == 4582 && gx == (923.0 / patch.cell_size).floor() as i32
                        && gz == (759.0 / patch.cell_size).floor() as i32 {
                        icc_cell = Some((eligible(cell), cell.authored_vegetation));
                    }
                }
            }
            assert!(eligible_authored > 0, "playfield {id}: no authored vegetation overlaps grass-admitting native terrain");
            let distance = |p: [f32; 3], q: [f32; 2]| (p[0] - q[0]).powi(2) + (p[2] - q[1]).powi(2);
            let nearest_authored = eligible_cells.iter().filter(|cell| cell.2)
                .min_by(|a, b| distance(a.1, target).total_cmp(&distance(b.1, target))).unwrap();
            let authored_center = nearest_authored.1;
            let nearest_uncovered = eligible_cells.iter().filter(|cell| !cell.2)
                .min_by(|a, b| distance(a.1, [authored_center[0], authored_center[2]])
                    .total_cmp(&distance(b.1, [authored_center[0], authored_center[2]])));
            eprintln!("PF{id}: eligible_authored={eligible_authored}; target scene xz={target:?}; nearest authored eligible (grid, scene center [x, mean terrain y, z], authored)={nearest_authored:?}; nearest uncovered eligible neighbor={nearest_uncovered:?}");
            if id == 4582 {
                // Native 206506 spikey_green_grass_largecluster.abiff at
                // AO (922.9516,23.6283,758.6634), next to this ICC sample.
                let raw = store.get(super::super::RECORD, id).unwrap().unwrap();
                let rec = super::super::record::parse(&raw).unwrap();
                let raw = store.get(super::super::TILEMAP, rec.tilemap).unwrap().unwrap();
                let tm = super::super::ground::parse(&raw).unwrap();
                let ground = super::super::terrain_height(&tm, 923.0, -759.0).unwrap();
                let (admits_grass, authored) = icc_cell.expect("ICC sample lies in loaded terrain");
                if admits_grass && (ground - 23.6283).abs() <= 0.5 {
                    assert!(authored, "ICC native grass cluster's near-ground cell was not marked");
                }
            }
        }
    }
}
