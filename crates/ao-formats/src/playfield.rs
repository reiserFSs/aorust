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
mod ground;
mod record;
mod statel;

use std::collections::HashMap;
use std::path::Path;

use anyhow::{anyhow, Context, Result};
use ao_rdb::RecordStore;
use ao_scene::{Instance, Mesh, Scene, Submesh, TextureKey, Vertex, IDENTITY};

use record::{Record, Room};
use statel::{Layout, Statel};

const RECORD: u32 = 1_000_001;
const STATELS: u32 = 1_000_003;
const TILEMAP: u32 = 1_000_009;
/// Ground tile texture quality used for terrain (1010006: 256², 1010021: 128², 1010022: 64²).
const TILE_TEXTURES: u32 = 1_010_021;

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

pub fn load_playfield_report(store: &RecordStore, _client_dir: &Path, id: u32) -> Result<(Scene, Report)> {
    let rec = record::parse(&store.get(RECORD, id)?.ok_or_else(|| anyhow!("no playfield {id}"))?)?;
    let mut scene = Scene::default();
    let mut report = Report::default();
    let mut spawn = None;
    if rec.is_outdoor() {
        let d = store.get(TILEMAP, rec.tilemap)?.ok_or_else(|| anyhow!("playfield {id}: no tilemap {}", rec.tilemap))?;
        let tm = ground::parse(&d).with_context(|| format!("tilemap {}", rec.tilemap))?;
        report.terrain_cells = tm.cells_x * tm.cells_z;
        let (cx, cz) = (tm.cells_x / 2, tm.cells_z / 2);
        spawn = Some([cx as f32 * tm.cell_size, tm.height(cx, cz) + 80.0, -(cz as f32 * tm.cell_size)]);
        build_terrain(store, &tm, &mut scene)?;
    } else {
        dungeon::build(store, &rec, &mut scene)?;
    }
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
        if spawn.is_none() {
            spawn = first_room_spawn(&rec);
        }
    }
    scene.spawn = spawn;
    Ok((scene, report))
}

fn first_room_spawn(rec: &Record) -> Option<[f32; 3]> {
    rec.rooms.first().map(|r| [r.pos[0], r.pos[1] + 3.0, -r.pos[2]])
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
        let (mut r, sc) = statel::orientation(s.flags, s.scale);
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
        self.scene.instances.push(Instance { mesh, transform: to_scene(&r, sc, p) });
    }
}

/// AO (left-handed) `T * R * S` -> scene (z negated) column-major matrix: `F M F`, `F = diag(1,1,-1)`.
fn to_scene(r: &[[f32; 3]; 3], s: [f32; 3], p: [f32; 3]) -> [[f32; 4]; 4] {
    let f = [1.0, 1.0, -1.0];
    let mut m = IDENTITY;
    for (j, col) in m.iter_mut().take(3).enumerate() {
        for (i, v) in col.iter_mut().take(3).enumerate() {
            *v = r[i][j] * f[i] * f[j] * s[j];
        }
    }
    m[3] = [p[0], p[1], -p[2], 1.0];
    m
}

fn texture_key(store: &RecordStore, scene: &mut Scene, id: u16) -> Option<TextureKey> {
    let key = TextureKey { rdb_type: TILE_TEXTURES, id: id as u32 };
    if !scene.textures.contains_key(&key) {
        let b = store.get(TILE_TEXTURES, id as u32).ok()??;
        let at = b.windows(3).position(|w| w == [0xff, 0xd8, 0xff])?; // 24 byte header before the JPEG
        scene.textures.insert(key, crate::texture::decode_texture(&b[at..]).ok()?);
    }
    Some(key)
}

/// One mesh per ground texture; every cell is its own quad (tile textures are per cell).
fn build_terrain(store: &RecordStore, tm: &ground::Tilemap, scene: &mut Scene) -> Result<()> {
    let mut by_tex: HashMap<Option<u16>, Vec<(u32, u32)>> = HashMap::new();
    for z in 0..tm.cells_z {
        for x in 0..tm.cells_x {
            let tex = tm.tile_texture.get(tm.tile(x, z) as usize).copied();
            by_tex.entry(tex).or_default().push((x as u32, z as u32));
        }
    }
    let cs = tm.cell_size;
    let normal = |x: usize, z: usize| -> [f32; 3] {
        let (x0, x1) = (x.saturating_sub(1), (x + 1).min(tm.verts_x - 1));
        let (z0, z1) = (z.saturating_sub(1), (z + 1).min(tm.verts_z - 1));
        let dx = (tm.height(x1, z) - tm.height(x0, z)) / ((x1 - x0) as f32 * cs);
        let dz = -(tm.height(x, z1) - tm.height(x, z0)) / ((z1 - z0) as f32 * cs); // scene z = -z
        let l = (dx * dx + 1.0 + dz * dz).sqrt();
        [-dx / l, 1.0 / l, -dz / l]
    };
    let mut keys: Vec<_> = by_tex.keys().copied().collect();
    keys.sort();
    for tex in keys {
        let cells = &by_tex[&tex];
        let texture = tex.and_then(|t| texture_key(store, scene, t));
        let mut mesh = Mesh::default();
        let mut indices = Vec::with_capacity(cells.len() * 6);
        mesh.vertices.reserve(cells.len() * 4);
        for &(x, z) in cells {
            let base = mesh.vertices.len() as u32;
            for (dx, dz) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let (vx, vz) = (x as usize + dx, z as usize + dz);
                mesh.vertices.push(Vertex {
                    pos: [vx as f32 * cs, tm.height(vx, vz), -(vz as f32 * cs)],
                    normal: normal(vx, vz),
                    uv: [dx as f32, dz as f32],
                });
            }
            // CCW seen from +Y in scene space (z negated). Bit 14 of the tile value picks the
            // quad diagonal (N3.dll n3RoomSurface_t::GetTileTriangles @ 0x10014888).
            if tm.diagonal_p10_p01(x as usize, z as usize) {
                indices.extend([base, base + 1, base + 2, base + 1, base + 3, base + 2]);
            } else {
                indices.extend([base, base + 1, base + 3, base, base + 3, base + 2]);
            }
        }
        mesh.submeshes.push(Submesh { indices, texture, alpha_test: false });
        scene.meshes.push(mesh);
        scene.instances.push(Instance { mesh: scene.meshes.len() - 1, transform: IDENTITY });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_transform_flips_z() {
        // heading 0, unit scale at AO (1, 2, 3) -> scene (1, 2, -3), identity rotation
        let (r, s) = statel::orientation(0x2d00, 90);
        let m = to_scene(&r, s, [1.0, 2.0, 3.0]);
        assert_eq!(m[3], [1.0, 2.0, -3.0, 1.0]);
        assert_eq!((m[0][0], m[1][1], m[2][2]), (1.0, 1.0, 1.0));
        // a +90 degree AO heading maps AO +z to AO +x; in scene space that is -z -> +x
        let (r, s) = statel::orientation((90 + 180 * 90) << 7, 90);
        let m = to_scene(&r, s, [0.0; 3]);
        // image of scene -z axis (column 2 negated)
        let v = [-m[2][0], -m[2][1], -m[2][2]];
        assert!((v[0] - 1.0).abs() < 1e-5 && v[2].abs() < 1e-5);
    }
}
