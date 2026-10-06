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

pub mod collision;
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
mod zone;

pub use ao_scene::FogVolume;
pub use sky::{open_weather, SkyClock, DEFAULT_DAY_TIME};
pub use spawn::{floor_below, scene_bounds, support_below};
pub use zone::{zone_locator, ZoneLocator};

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
/// `VisualCamera_t::GetLengthOfViewcone` (far - near) at the default view distance.
const LOD_VIEW_LENGTH: f32 = environment::VIEW_DISTANCE - environment::NEAR;

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

/// Ambient sound source of the statel file (`n3StatelSound_t`): the client calls `PlayGameSound(id, pos, ...)` every
/// frame the camera is within `radius` (`EvaluateStatelSoundFog`, N3 @0x10024eff). Scene space (z negated).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SoundEmitter {
    /// Dungeon room the emitter belongs to (`None` outdoors: the global list is always evaluated).
    pub room: Option<u32>,
    pub pos: [f32; 3],
    pub sound_id: u32,
    pub radius: f32,
}

/// What a load produced and what it had to skip.
#[derive(Debug, Default, Clone)]
pub struct Report {
    /// Ambient sound sources of the whole playfield (global and per zone/room).
    pub sounds: Vec<SoundEmitter>,
    /// Local fog volumes of the whole playfield (also in `Scene::fog_model`).
    pub fogs: Vec<FogVolume>,
    pub terrain_cells: usize,
    /// Scene instances of the terrain / dungeon room shells / water (after the sky objects, before every statel); the playfield
    /// map draws only these.
    pub ground: std::ops::Range<usize>,
    pub statels: usize,
    pub unique_meshes: usize,
    /// Statels with mesh id 0: `FUN_1002777a` (N3 @0x1002777a) creates no object for them.
    pub no_mesh_statels: usize,
    /// Statels whose mesh record is absent from the database.
    pub missing_meshes: usize,
    /// Their mesh ids (first-seen order, deduplicated).
    pub missing_mesh_ids: Vec<u32>,
    /// Statels whose mesh failed to decode (first error kept).
    pub failed_meshes: usize,
    pub first_mesh_error: Option<String>,
}

/// Decodes playfield `id` (terrain + placed static meshes) into a world-space scene at the client's frozen day time
/// ([`DEFAULT_DAY_TIME`]).
pub fn load_playfield(store: &RecordStore, client_dir: &Path, id: u32) -> Result<Scene> {
    load_playfield_at(store, client_dir, id, DEFAULT_DAY_TIME)
}

/// Like [`load_playfield`] with the sky, sun/ambient light, fog tint and ground shadows of `day_time` (`GameDayTime`,
/// 0..6480 seconds of the 27 minute Rubi-Ka day; 0 is midnight, 3240 about noon).
pub fn load_playfield_at(store: &RecordStore, client_dir: &Path, id: u32, day_time: f32) -> Result<Scene> {
    load_playfield_report_at(store, client_dir, id, day_time).map(|(s, _)| s)
}

pub fn load_playfield_report(store: &RecordStore, client_dir: &Path, id: u32) -> Result<(Scene, Report)> {
    load_playfield_report_at(store, client_dir, id, DEFAULT_DAY_TIME)
}

pub fn load_playfield_report_at(store: &RecordStore, client_dir: &Path, id: u32, day_time: f32) -> Result<(Scene, Report)> {
    load_playfield_report_on_day(store, client_dir, id, day_time, crate::weather::OFFLINE_DAY)
}

/// [`load_playfield_at`] for a server game day: `game_day` (`GameTime_t+0x4C`, `GameTimeIIR_t.arg3`) seeds the weather schedule.
pub fn load_playfield_on_day(store: &RecordStore, client_dir: &Path, id: u32, day_time: f32, game_day: u32) -> Result<Scene> {
    load_playfield_report_on_day(store, client_dir, id, day_time, game_day).map(|(s, _)| s)
}

pub fn load_playfield_report_on_day(store: &RecordStore, client_dir: &Path, id: u32, day_time: f32, game_day: u32) -> Result<(Scene, Report)> {
    // the day repeats: wrap so colour tracks, ground shadows, sun and moons agree; NaN/inf fall back to the default
    let day_time = if day_time.is_finite() { day_time.rem_euclid(sky::DAY_LENGTH) } else { DEFAULT_DAY_TIME };
    let raw = store.get(RECORD, id)?.ok_or_else(|| anyhow!("no playfield {id}"))?;
    let rec = record::parse(&raw)?;
    let mut scene = Scene::default();
    // RDBPlayfieldAnarchy_t tail: liquid polygons, then the environment (fog/ambient)
    let mut tail = record::Rd::new(&raw, rec.tail);
    let waters = water::parse(&mut tail).with_context(|| format!("liquids of playfield {id}"))?;
    let env = environment::parse(&mut tail).with_context(|| format!("environment of playfield {id}"))?;
    let tweaks = sky::Tweaks::load(client_dir, id, rec.is_outdoor());
    let sky = tweaks.as_ref().and_then(|t| sky::Sky::new(t, day_time)).map(|mut s| {
        s.set_weather(sky::weather_at(&env, game_day, day_time));
        s
    });
    let mut environment = environment::to_scene(&env, rec.is_outdoor(), sky.as_ref());
    if let Some(c) = tweaks.as_ref().and_then(sky::clear_color) {
        environment.sky_color = c.map(environment::srgb_to_linear);
    }
    if let (Some(s), Some(t), true) = (&sky, &tweaks, rec.is_outdoor()) {
        sky::emit(s, t, store, &mut scene, environment.fog_color, environment.fog_end);
        sky::emit_distant(t, store, &mut scene, day_time);
    }
    scene.environment = Some(environment);
    let mut report = Report::default();
    let mut spot = None;
    let mut terrain = None;
    let mut grid = None;
    let mut props: Vec<Vec<[f32; 3]>> = Vec::new();
    let mut lod_items: Vec<Placed> = Vec::new();
    let (mut lod_zone_size, mut lod_tiles_x, mut lod_count) = (0.0f32, 0usize, 0usize);
    if rec.is_outdoor() {
        let d = store.get(TILEMAP, rec.tilemap)?.ok_or_else(|| anyhow!("playfield {id}: no tilemap {}", rec.tilemap))?;
        let tm = ground::parse(&d).with_context(|| format!("tilemap {}", rec.tilemap))?;
        report.terrain_cells = tm.cells_x * tm.cells_z;
        report.ground.start = scene.instances.len();
        terrain::build(store, id, &tm, &mut scene, day_time)?;
        terrain = Some(tm);
    } else {
        report.ground.start = scene.instances.len();
        grid = Some(dungeon::build(store, &rec, &mut scene)?);
    }
    water::emit(store, &waters, &mut scene);
    let fixed = scene.instances.len();
    report.ground.end = fixed;
    if let Some(d) = store.get(STATELS, id)? {
        let layout = if rec.is_outdoor() { Layout::Outdoor } else { Layout::Dungeon };
        let file = statel::parse(&d, rec.count as usize, layout).with_context(|| format!("statels of playfield {id}"))?;
        let mut placer = Placer { store, scene: &mut scene, cache: HashMap::new(), report: &mut report };
        // outdoor statels are under the client's zone LOD (`StatelLod`); global statels (the compacted list of mesh != 0
        // entries) are shown by the zones whose index list names them
        let tiles_x = terrain.as_ref().map_or(0, |t| t.cells_x / rec.zone_size.max(1) as usize);
        let lod = file.outdoor && tiles_x > 0;
        let mut globals: Vec<usize> = Vec::new();
        for s in &file.global.statels {
            if let Some(it) = placer.place(s, None, 4) {
                globals.push(lod_items.len());
                lod_items.push(it);
            }
        }
        for (i, zone) in file.zones.iter().enumerate() {
            let room = rec.rooms.get(i);
            for s in &zone.statels {
                if let Some(mut it) = placer.place(s, room, s.list) {
                    it.zones.push(i as u32);
                    lod_items.push(it);
                }
            }
            for &g in &zone.global_refs {
                if let Some(&k) = globals.get(g as usize) {
                    lod_items[k].zones.push(i as u32);
                }
            }
        }
        lod_zone_size = terrain.as_ref().map_or(0.0, |t| t.cell_size * rec.zone_size as f32);
        lod_tiles_x = if lod { tiles_x } else { 0 };
        lod_count = file.zones.len();
        report.unique_meshes = placer.cache.values().filter(|m| m.is_some()).count();
        for (i, zone) in file.zones.iter().enumerate() {
            let frame = rec.rooms.get(i).map(|r| (statel::ry(r.rot as f32 * std::f32::consts::FRAC_PI_2), r.pos));
            // outdoor lights belong to their tile-block zone and follow its distance level (`StatelLod::lights_active`)
            let zone_id = file.outdoor.then_some(i as u32);
            scene.lights.extend(zone.lights.iter().filter_map(|l| statel::scene_light(l, frame)).map(|l| ao_scene::Light { zone: zone_id, ..l }));
        }
        let rooms = std::iter::once(None).chain((0..file.zones.len()).map(|i| rec.rooms.get(i)));
        for (zi, (z, room)) in std::iter::once(&file.global).chain(&file.zones).zip(rooms).enumerate() {
            // dungeon zone `zi - 1` is room `zi - 1`; the global list and every outdoor zone are always evaluated
            let room_id = (!file.outdoor && zi > 0).then(|| zi as u32 - 1);
            let at = |p: [f32; 3]| {
                let p = match room {
                    Some(r) => {
                        let q = statel::ry(r.rot as f32 * std::f32::consts::FRAC_PI_2);
                        [0, 1, 2].map(|i| q[i][0] * p[0] + q[i][1] * p[1] + q[i][2] * p[2] + r.pos[i])
                    }
                    None => p,
                };
                [p[0], p[1], -p[2]]
            };
            report.sounds.extend(z.sounds.iter().map(|e| SoundEmitter { room: room_id, pos: at(e.pos), sound_id: e.value, radius: e.radius as f32 }));
            report.fogs.extend(z.fogs.iter().map(|e| FogVolume {
                pos: at(e.pos),
                color: [e.value >> 16, e.value >> 8, e.value].map(|c| (c & 0xff) as f32 / 255.0),
                density: (e.value >> 24) as f32 / 100.0,
                radius: e.radius as f32,
                room: room_id,
            }));
        }
        if !report.fogs.is_empty() {
            let volumes = report.fogs.clone();
            let mut model = environment::fog_model(&env, sky.as_ref(), volumes);
            model.rooms = (!rec.is_outdoor()).then(|| zone_locator(store, id).ok()).flatten().map(|zl| ao_scene::RoomLocator(std::sync::Arc::new(move |p| zl.room_at(p))));
            scene.fog_model = Some(model);
        }
        props = file.zones.iter().map(|z| z.statels.iter().filter(|s| s.mesh != 0).map(|s| s.pos).collect()).collect();
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
    if lod_tiles_x > 0 {
        add_lod(store, &mut scene, &mut report, lod_items, lod_tiles_x, lod_zone_size, lod_count);
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

/// A statel's mesh record type and id plus its texture-override attributes: each distinct triple is one decoded scene mesh.
type MeshKey = (u32, u32, Vec<(u32, u32)>);

/// A placed statel awaiting its LOD bookkeeping.
struct Placed {
    instance: usize,
    mesh: u32,
    attrs: Vec<(u32, u32)>,
    transform: [[f32; 4]; 4],
    flag8: bool,
    class: u8,
    zones: Vec<u32>,
}

struct Placer<'a> {
    store: &'a RecordStore,
    scene: &'a mut Scene,
    cache: HashMap<MeshKey, Option<usize>>,
    report: &'a mut Report,
}

impl Placer<'_> {
    fn mesh(&mut self, rdb_type: u32, id: u32, attrs: &[(u32, u32)]) -> Option<usize> {
        let key = (rdb_type, id, attrs.to_vec());
        if let Some(&m) = self.cache.get(&key) {
            return m;
        }
        let m = match crate::mesh::decode_statel_mesh(self.store, rdb_type, id, attrs, self.scene) {
            Ok(m) => m,
            Err(e) => {
                self.report.first_mesh_error.get_or_insert_with(|| format!("mesh {id}: {e:#}"));
                self.report.failed_meshes += 1;
                None
            }
        };
        self.cache.insert(key, m);
        m
    }

    /// Places statel `s` (zone list `class`, 4 = global); returns the placement for the LOD pass.
    fn place(&mut self, s: &Statel, room: Option<&Room>, class: u8) -> Option<Placed> {
        self.report.statels += 1;
        if s.mesh == 0 {
            self.report.no_mesh_statels += 1;
            return None;
        }
        let Some(mesh) = self.mesh(crate::mesh::MESH_TYPE, s.mesh, &s.attrs) else {
            self.report.missing_meshes += 1;
            if !self.report.missing_mesh_ids.contains(&s.mesh) {
                self.report.missing_mesh_ids.push(s.mesh);
            }
            return None;
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
        let transform = to_scene(&r, p);
        self.scene.instances.push(Instance { mesh, transform });
        Some(Placed { instance: self.scene.instances.len() - 1, mesh: s.mesh, attrs: s.attrs.clone(), transform, flag8: s.flags & 8 != 0, class, zones: Vec::new() })
    }
}

/// Builds `Scene::statel_lod`: appends the reduced-mesh (rdb 1010026) twin of every statel that has one, after all
/// fixed content so spawn selection does not see them. Items without a zone (global statels no zone references) are
/// never created by the client either and stay hidden (`pick` with no zones).
fn add_lod(store: &RecordStore, scene: &mut Scene, report: &mut Report, items: Vec<Placed>, tiles_x: usize, zone_size: f32, zone_count: usize) {
    let zones = (0..zone_count).map(|i| [((i % tiles_x) as f32 + 0.5) * zone_size, -(((i / tiles_x) as f32 + 0.5) * zone_size)]).collect();
    let mut placer = Placer { store, scene: &mut *scene, cache: HashMap::new(), report };
    let items = items
        .into_iter()
        .map(|it| {
            let reduced = placer.mesh(crate::mesh::MESH_LOW_TYPE, it.mesh, &it.attrs).map(|mesh| {
                placer.scene.instances.push(Instance { mesh, transform: it.transform });
                placer.scene.instances.len() - 1
            });
            ao_scene::LodItem { full: it.instance, reduced, flag8: it.flag8, class: it.class, zones: it.zones }
        })
        .collect();
    scene.statel_lod = Some(ao_scene::StatelLod::new(LOD_VIEW_LENGTH, zones, items));
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
