//! Playfields: heightfield terrain, ground texturing and placed statics (static meshes).
//!
//! Source data (see `docs/formats.md` § playfields):
//! * rdb 1000001 — playfield resource: name, tilemap id, zone count, dungeon rooms ([`record`])
//! * rdb 1000009 — `CHGA` heightfield tilemap ([`ground`])
//! * rdb 1000003 — statel (placed static mesh) file, zone/room indexed ([`statel`])
//! * rdb 1010021 — ground tile textures (128 x 128 JPEG after a 24 byte header)
//!
//! AO world space is left-handed, Y up, 1 unit = 1 m, x east, z north-ish. Scene space is the
//! right-handed mirror obtained by negating z (the convention of `mesh::decode_mesh_into`).

mod dungeon;
mod environment;
mod ground;
mod record;
mod sky;
mod spawn;
mod shadow;
mod statel;
mod terrain;
mod water;

pub use spawn::{floor_below, scene_bounds};

use std::collections::HashMap;
use std::path::Path;

use anyhow::{anyhow, Context, Result};
use ao_rdb::RecordStore;
use ao_scene::{Instance, Scene, IDENTITY};

use record::Room;
use statel::{Layout, Statel};

const RECORD: u32 = 1_000_001;
const STATELS: u32 = 1_000_003;
const TILEMAP: u32 = 1_000_009;

/// Playfield ids with name, for `aomac view --list`.
pub fn list_playfields(store: &RecordStore) -> Result<Vec<(u32, String)>> {
    let mut out = Vec::new();
    for id in store.ids(RECORD)? {
        let d = store.get(RECORD, id)?.with_context(|| format!("playfield {id}"))?;
        anyhow::ensure!(d.len() >= 40, "playfield {id}: short record");
        out.push((id, record::cstr(&d[8..40])));
    }
    Ok(out)
}

/// What a load produced and what it had to skip.
#[derive(Debug, Default, Clone)]
pub struct Report {
    pub terrain_cells: usize,
    pub statels: usize,
    pub unique_meshes: usize,
    /// Statels whose mesh record is missing.
    pub missing_meshes: usize,
    /// Statels whose mesh failed to decode (first error kept).
    pub failed_meshes: usize,
    pub first_mesh_error: Option<String>,
}

/// Decodes playfield `id` (terrain + placed static meshes) into a world-space scene.
pub fn load_playfield(store: &RecordStore, client_dir: &Path, id: u32) -> Result<Scene> {
    load_playfield_report(store, client_dir, id).map(|(s, _)| s)
}

pub fn load_playfield_report(store: &RecordStore, client_dir: &Path, id: u32) -> Result<(Scene, Report)> {
    let raw = store.get(RECORD, id)?.ok_or_else(|| anyhow!("no playfield {id}"))?;
    let rec = record::parse(&raw)?;
    let mut scene = Scene::default();
    // RDBPlayfieldAnarchy_t tail: liquid polygons, then the environment (fog/ambient)
    let mut tail = record::Rd::new(&raw, rec.tail);
    let waters = water::parse(&mut tail).with_context(|| format!("liquids of playfield {id}"))?;
    let env = environment::parse(&mut tail).with_context(|| format!("environment of playfield {id}"))?;
    let sky = sky::Tweaks::load(client_dir, id, rec.is_outdoor()).and_then(|t| sky::Sky::new(&t));
    let environment = environment::to_scene(&env, rec.is_outdoor(), sky.as_ref());
    if let (Some(s), true) = (&sky, rec.is_outdoor()) {
        sky::emit(s, store, &mut scene, environment.fog_color, environment.fog_end);
    }
    scene.environment = Some(environment);
    let mut report = Report::default();
    let mut spot = None;
    let mut terrain = None;
    let mut grid = None;
    let mut props: Vec<Vec<[f32; 3]>> = Vec::new();
    if rec.is_outdoor() {
        let d = store.get(TILEMAP, rec.tilemap)?.ok_or_else(|| anyhow!("playfield {id}: no tilemap {}", rec.tilemap))?;
        let tm = ground::parse(&d).with_context(|| format!("tilemap {}", rec.tilemap))?;
        report.terrain_cells = tm.cells_x * tm.cells_z;
        terrain::build(store, id, &tm, &mut scene)?;
        terrain = Some(tm);
    } else {
        grid = Some(dungeon::build(store, &rec, &mut scene)?);
    }
    if let Some(m) = water::build_mesh(&waters) {
        scene.meshes.push(m);
        scene.instances.push(Instance { mesh: scene.meshes.len() - 1, transform: IDENTITY });
    }
    let fixed = scene.instances.len();
    if let Some(d) = store.get(STATELS, id)? {
        let layout = if rec.is_outdoor() { Layout::Outdoor } else { Layout::Dungeon };
        let file = statel::parse(&d, rec.count as usize, layout).with_context(|| format!("statels of playfield {id}"))?;
        let mut placer = Placer { store, scene: &mut scene, cache: HashMap::new(), report: &mut report };
        for s in &file.global {
            placer.place(s, None);
        }
        for (i, zone) in file.zones.iter().enumerate() {
            let room = rec.rooms.get(i);
            for s in zone {
                placer.place(s, room);
            }
        }
        report.unique_meshes = placer.cache.values().filter(|m| m.is_some()).count();
        for (i, zl) in file.lights.iter().enumerate() {
            let frame = rec.rooms.get(i).map(|r| (statel::ry(r.rot as f32 * std::f32::consts::FRAC_PI_2), r.pos));
            scene.lights.extend(zl.iter().filter_map(|l| statel::scene_light(l, frame)));
        }
        props = file.zones.iter().map(|z| z.iter().filter(|s| s.mesh != 0).map(|s| s.pos).collect()).collect();
    }
    if let Some(g) = &grid {
        spot = dungeon::entry_spot(g, &rec, &props);
    }
    if spot.is_none() {
        // outdoor (or a dungeon without a usable room): next to the densest statel cluster
        let boxes = spawn::instance_boxes(&scene, fixed);
        let low = boxes.iter().map(|b| b.0[1]).fold(f32::MAX, f32::min);
        spot = match &terrain {
            Some(tm) => spawn::density_spawn(&boxes, &|x, z| terrain_height(tm, x, z), 4.0).or_else(|| {
                // terrain only: hover above the middle of the map
                let (x, z) = (tm.cells_x as f32 * tm.cell_size * 0.5, -(tm.cells_z as f32 * tm.cell_size * 0.5));
                let y = terrain_height(tm, x, z)? + 25.0;
                Some(spawn::Spot { eye: [x, y, z], at: [x, y - 10.0, z - 60.0] })
            }),
            None => spawn::density_spawn(&boxes, &|_, _| Some(low), 1.7),
        };
    }
    scene.spawn = spot.map(|s| s.eye);
    scene.spawn_look_at = spot.map(|s| s.at);
    Ok((scene, report))
}

/// Bilinear terrain height at scene `(x, z)`; `None` outside the heightfield.
fn terrain_height(tm: &ground::Tilemap, x: f32, z: f32) -> Option<f32> {
    let (fx, fz) = (x / tm.cell_size, -z / tm.cell_size);
    if fx < 0.0 || fz < 0.0 || fx >= tm.cells_x as f32 || fz >= tm.cells_z as f32 {
        return None;
    }
    let (ix, iz, ax, az) = (fx as usize, fz as usize, fx.fract(), fz.fract());
    let (a, b) = (tm.height(ix, iz) * (1.0 - ax) + tm.height(ix + 1, iz) * ax, tm.height(ix, iz + 1) * (1.0 - ax) + tm.height(ix + 1, iz + 1) * ax);
    Some(a * (1.0 - az) + b * az)
}

struct Placer<'a> {
    store: &'a RecordStore,
    scene: &'a mut Scene,
    cache: HashMap<u32, Option<usize>>,
    report: &'a mut Report,
}

impl Placer<'_> {
    fn mesh(&mut self, id: u32) -> Option<usize> {
        if let Some(&m) = self.cache.get(&id) {
            return m;
        }
        let m = match crate::mesh::decode_mesh_into(self.store, id, self.scene) {
            Ok(m) => m,
            Err(e) => {
                self.report.first_mesh_error.get_or_insert_with(|| format!("mesh {id}: {e:#}"));
                self.report.failed_meshes += 1;
                None
            }
        };
        self.cache.insert(id, m);
        m
    }

    fn place(&mut self, s: &Statel, room: Option<&Room>) {
        self.report.statels += 1;
        let Some(mesh) = self.mesh(s.mesh) else {
            self.report.missing_meshes += 1;
            return;
        };
        let mut r = statel::orientation(s.flags, s.scale);
        let mut p = s.pos;
        if let Some(room) = room {
            let q = statel::ry(room.rot as f32 * std::f32::consts::FRAC_PI_2);
            r = statel::mul(q, r);
            p = [
                q[0][0] * p[0] + q[0][1] * p[1] + q[0][2] * p[2] + room.pos[0],
                q[1][0] * p[0] + q[1][1] * p[1] + q[1][2] * p[2] + room.pos[1],
                q[2][0] * p[0] + q[2][1] * p[1] + q[2][2] * p[2] + room.pos[2],
            ];
        }
        self.scene.instances.push(Instance { mesh, transform: to_scene(&r, p) });
    }
}

/// AO (left-handed) `T * L` (`L` = linear part) -> scene (z negated) column-major matrix: `F M F`, `F = diag(1,1,-1)`.
fn to_scene(r: &[[f32; 3]; 3], p: [f32; 3]) -> [[f32; 4]; 4] {
    let f = [1.0, 1.0, -1.0];
    let mut m = IDENTITY;
    for (j, col) in m.iter_mut().take(3).enumerate() {
        for (i, v) in col.iter_mut().take(3).enumerate() {
            *v = r[i][j] * f[i] * f[j];
        }
    }
    m[3] = [p[0], p[1], -p[2], 1.0];
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_transform_flips_z() {
        // heading 0, unit scale at AO (1, 2, 3) -> scene (1, 2, -3), identity rotation
        let r = statel::orientation(0x2d00, 90);
        let m = to_scene(&r, [1.0, 2.0, 3.0]);
        assert_eq!(m[3], [1.0, 2.0, -3.0, 1.0]);
        assert_eq!((m[0][0], m[1][1], m[2][2]), (1.0, 1.0, 1.0));
        // a +90 degree AO heading maps AO +z to AO +x; in scene space that is -z -> +x
        let r = statel::orientation((90 + 180 * 90) << 7, 90);
        let m = to_scene(&r, [0.0; 3]);
        // image of scene -z axis (column 2 negated)
        let v = [-m[2][0], -m[2][1], -m[2][2]];
        assert!((v[0] - 1.0).abs() < 1e-5 && v[2].abs() < 1e-5);
    }

    /// Statels sit on the terrain: the median gap between a placed object's origin and the heightfield is small,
    /// also for the 22 maps with 16 bit height samples (556 Coast of Peace, 656, 4380 ...).
    #[test]
    fn statels_sit_on_the_terrain() {
        let Some(home) = std::env::var_os("HOME") else { return };
        let dir = std::path::PathBuf::from(home).join("Games/ProjectRubiKa/client");
        let Ok(store) = RecordStore::open(&dir) else { return };
        for id in [566u32, 600, 505, 556, 656, 4894, 6012, 6022] {
            let Ok(Some(raw)) = store.get(RECORD, id) else { continue };
            let rec = record::parse(&raw).unwrap();
            let tm = ground::parse(&store.get(TILEMAP, rec.tilemap).unwrap().unwrap()).unwrap();
            let scene = load_playfield(&store, &dir, id).unwrap();
            let mut gaps: Vec<f32> = scene.instances.iter().filter_map(|i| {
                let t = i.transform[3];
                (t[0] != 0.0 || t[2] != 0.0).then(|| terrain_height(&tm, t[0], t[2]).map(|h| (t[1] - h).abs())).flatten()
            }).collect();
            if gaps.len() < 50 {
                continue;
            }
            gaps.sort_by(f32::total_cmp);
            let median = gaps[gaps.len() / 2];
            assert!(median < 3.0, "playfield {id}: median statel-terrain gap {median} m over {} statels", gaps.len());
        }
    }
}
