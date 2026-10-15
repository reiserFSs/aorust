//! Life_c 1011bb4a/1011b90b/1011b627: authored district requests, near-zone activation and native switch destruction.
use anyhow::{ensure, Result};
use ao_audio::district::Districts;
use ao_formats::{playfield::{self, collision::Collision, WildlifeGeometry, ZoneLocator}, wildlife::{self, LifeAsset, LifeKind}};
use ao_rdb::RecordStore;
use glam::Vec3;
use std::sync::Arc;
use super::native_life::{Animation, CharacterSensor, Environment, Neighbor, Species};

#[derive(Clone, Copy)]
struct Request { catalog: usize, district: usize, pos: Vec3, count: u32, grouped: bool }
struct InstanceState { animation: Option<Animation>, serial: u64, zone_visible: bool, distance_visible: bool }
struct Rng(u32);
impl Rng {
    fn value(&mut self) -> u32 { self.0 = self.0.wrapping_mul(214013).wrapping_add(2531011); (self.0 >> 16) & 32767 }
    fn unit(&mut self) -> f64 { f64::from(self.value()) / 32767.0 }
}
fn mirror(p: Vec3) -> Vec3 { Vec3::new(p.x, p.y, -p.z) }

pub struct LifeWorld {
    pub catalog: Vec<LifeAsset>,
    pub species: Vec<Species>,
    pub ids: Vec<u32>,
    geometry: WildlifeGeometry,
    zones: ZoneLocator,
    districts: Option<Districts>,
    requests: Vec<Vec<Request>>,
    heights: Vec<f32>,
    clips: Vec<Option<(Arc<[f32]>, f32)>>,
    states: Vec<InstanceState>,
    enabled: bool,
    prepared: bool,
    previous: [u32; 25],
    distance_timer: f32,
    next_id: u32,
    next_group: u32,
    rng: Rng,
    neighbors: Vec<Neighbor>,
}
impl LifeWorld {
    pub fn load(store: &RecordStore, pf: u32) -> Result<Self> {
        let catalog = wildlife::load_catalog(store)?;
        let heights = vec![0.0; catalog.len()];
        let clips = vec![None; catalog.len()];
        let geometry = playfield::wildlife_geometry(store, pf)?;
        let zones = playfield::zone_locator(store, pf)?;
        let districts = store.get(1_000_014, pf)?.map(|b| Districts::parse(&b)).transpose()?;
        if let Some(districts) = &districts {
            for district in &districts.districts {
                ensure!(district.wildlife.iter().all(|&(index, _)| (index as usize) < catalog.len()), "district wildlife catalog index out of range");
            }
        }
        let requests = (0..geometry.zones_x * geometry.zones_z).map(|_| Vec::new()).collect();
        Ok(Self { catalog, species: Vec::new(), ids: Vec::new(), geometry, zones, districts, requests, heights, clips, states: Vec::new(), enabled: false, prepared: false, previous: [0; 25], distance_timer: 0.0, next_id: 1, next_group: 1, rng: Rng(1), neighbors: Vec::new() })
    }
    pub fn catalog_used(&self, index: usize) -> bool {
        self.districts.as_ref().is_some_and(|districts| districts.districts.iter().any(|district| district.wildlife.iter().any(|&(asset,count)| asset as usize == index && count > 0)))
    }
    pub fn set_mesh_height(&mut self, mesh: u32, height: f32) {
        for (asset, h) in self.catalog.iter().zip(&mut self.heights) { if asset.mesh == mesh { *h = height; } }
    }
    pub fn set_mesh_animation(&mut self, mesh: u32, boundaries: Arc<[f32]>, total: f32) {
        for (asset, clip) in self.catalog.iter().zip(&mut self.clips) {
            if asset.mesh == mesh { *clip = Some((Arc::clone(&boundaries), total)); }
        }
    }
    pub fn set_enabled(&mut self, enabled: bool) {
        if self.enabled == enabled { return; }
        self.enabled = enabled;
        if enabled {
            self.prepared = false;
            // Native EnableLife increments just the cached centre zone to force its next locality update.
            self.previous[4] = self.previous[4].wrapping_add(1);
        } else {
            self.species.clear(); self.ids.clear(); self.states.clear(); self.neighbors.clear();
        }
    }
    #[allow(clippy::too_many_arguments)]
    pub fn tick(&mut self, dt: f32, control: Vec3, surface: &Collision, characters: &[CharacterSensor], dangers: &[CharacterSensor], corpses: &[Vec3], wind: Vec3) {
        if !self.enabled { return; }
        if !self.prepared {
            self.rng = Rng(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_secs() as u32));
            self.prepare(surface);
            self.prepared = true;
        }
        let env = WorldEnvironment { surface, zones: &self.zones, districts: self.districts.as_ref(), characters, dangers, corpses, wind };
        let centre = self.zones.zone_at(mirror(control).to_array()).unwrap_or(0) as u32;
        let current = vicinity(centre, self.geometry.zones_x as u32);
        let update_zone = centre != self.previous[4];
        if update_zone {
            for (i, &zone) in current.iter().enumerate() {
                let old = if i < 9 { &self.previous[..9] } else { &self.previous[..25] };
                // Bird's native slot39 uses all25; every other native kind uses the inner9.
                if let Some(requests) = self.requests.get(zone as usize) {
                    for request in requests {
                        let bird = self.catalog[request.catalog].kind == 0;
                        if (bird && !self.previous.contains(&zone)) || (!bird && i < 9 && !old.contains(&zone)) {
                            let group = request.grouped.then(|| { let id = self.next_group; self.next_group += 1; id });
                            for _ in 0..request.count {
                                let kind = LifeKind::from_native(self.catalog[request.catalog].kind).expect("validated native life kind");
                                let mut s = Species::new(kind, request.catalog, request.pos, request.district, self.rng.value());
                                s.group = group; s.body_radius = self.heights[request.catalog] * 0.5;
                                s.pos.y += s.body_radius;
                                s.place(&env);
                                s.id = self.next_id;
                                let animation = self.clips[request.catalog].as_ref().map(|(boundaries,total)| {
                                    let mut animation = Animation::new(Arc::clone(boundaries), *total);
                                    animation.select(s.animation, false);
                                    animation
                                });
                                self.states.push(InstanceState { animation, serial: s.animation_serial, zone_visible: false, distance_visible: false });
                                self.ids.push(self.next_id); self.next_id += 1; self.species.push(s);
                            }
                        }
                    }
                }
            }
        }
        self.distance_timer += dt;
        let update_distance = self.distance_timer > 5.0; // DAT102c5b40
        if update_distance { self.distance_timer = 0.0; }
        self.neighbors.clear();
        self.neighbors.extend(self.species.iter().map(|s| Neighbor { id: s.id, pos: s.pos, vel: s.vel, state: s.state, group: s.group }));
        for (index, (s, state)) in self.species.iter_mut().zip(&mut self.states).enumerate() {
            let bird = s.kind == LifeKind::Bird;
            if update_zone {
                let zone = self.zones.zone_at(mirror(s.pos).to_array()).unwrap_or(0) as u32;
                state.zone_visible = current[..if bird {25} else {9}].contains(&zone);
            }
            if update_distance {
                let d = s.pos - control;
                state.distance_visible = d.x * d.x + d.z * d.z < if bird {2916.0} else {729.0};
            }
            if state.zone_visible && state.distance_visible {
                if let Some(animation) = &mut state.animation {
                    animation.tick(dt);
                    s.animation = animation.index;
                    s.animation_time = animation.time;
                }
            }
            s.tick(dt, &env, &self.neighbors, state.zone_visible, state.distance_visible);
            if let Some(animation) = &mut state.animation {
                if s.animation_serial != state.serial {
                    animation.select(s.animation, false);
                    state.serial = s.animation_serial;
                }
                if let Some(index) = s.pending_animation.take() { animation.select(index, true); }
                s.animation = animation.index;
                s.animation_time = animation.time;
            }
            self.neighbors[index] = Neighbor { id: s.id, pos: s.pos, vel: s.vel, state: s.state, group: s.group };
        }
        let mut read = 0; let mut kept = 0;
        self.species.retain(|s| {
            let index = read; let id = self.ids[index]; read += 1;
            if s.alive {
                self.ids[kept] = id; self.states.swap(kept, index); kept += 1; true
            } else { false }
        });
        self.ids.truncate(kept);
        self.states.truncate(kept);
        self.previous = current;
    }
    fn prepare(&mut self, surface: &Collision) {
        let Some(districts) = &self.districts else { return };
        for (district, data) in districts.districts.iter().enumerate() {
            let zones: Vec<usize> = districts.zone_to_district.iter().enumerate().filter_map(|(z,&d)| (d as usize == district && z < self.requests.len()).then_some(z)).collect();
            if zones.is_empty() { continue; }
            for &(catalog, count) in &data.wildlife {
                let catalog = catalog as usize;
                let Some(asset) = self.catalog.get(catalog) else { continue };
                let Some(kind) = LifeKind::from_native(asset.kind) else { continue };
                let chance = if kind.supports_groups() { asset.group_chance.clamp(0.0, 1.0) } else { 0.0 };
                let mut grouped = (((self.rng.unit() - 0.5) * f64::from(0.1f32) + f64::from(chance)) * f64::from(count)) as i32;
                grouped = grouped.clamp(0, i32::from(count));
                let mut singles = u32::from(count) - grouped as u32;
                let mut sizes = Vec::new();
                while grouped > 0 {
                    if grouped < 3 { singles += grouped as u32; break; }
                    let n = if grouped < 10 { grouped as u32 } else { ((self.rng.unit() * 10.0 + 1.0) as u32).max(3).min(grouped as u32) };
                    sizes.push(n); grouped -= n as i32;
                }
                let positions = positions(kind, sizes.len() + singles as usize, &zones, &self.geometry, surface, &mut self.rng);
                for (index, (zone, pos)) in positions.into_iter().enumerate() {
                    let (count, grouped) = sizes.get(index).map_or((1, false), |&n| (n, true));
                    self.requests[zone].push(Request { catalog, district, pos, count, grouped });
                }
            }
        }
    }
}

fn vicinity(zone: u32, width: u32) -> [u32; 25] {
    let mut out = [0; 25]; let mut p = 0;
    for z in -1i32..=1 { for x in -1i32..=1 { out[p] = zone.wrapping_add_signed(z.wrapping_mul(width as i32).wrapping_add(x)); p += 1; } }
    for z in -2i32..=2 { for x in -2i32..=2 { if x.abs() == 2 || z.abs() == 2 { out[p] = zone.wrapping_add_signed(z.wrapping_mul(width as i32).wrapping_add(x)); p += 1; } } }
    out
}
fn positions(kind: LifeKind, count: usize, zones: &[usize], geometry: &WildlifeGeometry, surface: &Collision, rng: &mut Rng) -> Vec<(usize, Vec3)> {
    let mut out = Vec::with_capacity(count);
    let edge = geometry.zone_edge;
    let mut bounds = Vec::new();
    if kind == LifeKind::Fish {
        let mut p = 0;
        while p < geometry.water_triangles.len() {
            let first = geometry.water_triangles[p].0[0];
            let mut lo = [f32::MAX; 2]; let mut hi = [0.0f32; 2];
            loop {
                for v in geometry.water_triangles[p].0 { lo[0] = lo[0].min(v[0]); lo[1] = lo[1].min(v[2]); hi[0] = hi[0].max(v[0]); hi[1] = hi[1].max(v[2]); }
                p += 1;
                if p == geometry.water_triangles.len() || geometry.water_triangles[p].0[0] != first { break; }
            }
            let mut dlo = [f32::MAX; 2]; let mut dhi = [0.0f32; 2];
            for &zone in zones { let x = (zone % geometry.zones_x) as f32 * edge; let z = (zone / geometry.zones_x) as f32 * edge; dlo[0] = dlo[0].min(x); dlo[1] = dlo[1].min(z); dhi[0] = dhi[0].max(x+edge); dhi[1] = dhi[1].max(z+edge); }
            lo[0] = lo[0].max(dlo[0]); lo[1] = lo[1].max(dlo[1]); hi[0] = hi[0].min(dhi[0]); hi[1] = hi[1].min(dhi[1]);
            if lo[0] < hi[0] && lo[1] < hi[1] { bounds.push((lo,hi)); }
        }
        if bounds.is_empty() { return out; }
    }
    for _ in 0..count {
        let fish_bounds = if kind == LifeKind::Fish { Some(bounds[(rng.unit() * bounds.len() as f64) as usize % bounds.len()]) } else { None };
        for _ in 0..10 {
            let (zone, x, z) = if let Some((lo,hi)) = fish_bounds { (None, lo[0] + rng.unit() as f32 * (hi[0]-lo[0]), lo[1] + rng.unit() as f32 * (hi[1]-lo[1])) } else {
                let zone = zones[(rng.unit() * zones.len() as f64) as usize % zones.len()];
                (Some(zone), (zone % geometry.zones_x) as f32 * edge + rng.unit() as f32 * edge, (zone / geometry.zones_x) as f32 * edge + rng.unit() as f32 * edge)
            };
            let pos = Vec3::new(x, 0.01, z);
            let liquid = surface.liquid_surface_at(mirror(pos).to_array());
            if kind == LifeKind::Fish {
                let Some(liquid) = liquid else { continue };
                if liquid.level <= 0.0 || !matches!(liquid.kind, 0 | 4 | 10) { continue; }
                let Some(floor) = surface.line([x,100.0,-z], [x,-0.1,-z]) else { continue };
                if liquid.level - floor.p[1] <= 0.48000002 { continue; }
                let zone = (z / edge) as usize * geometry.zones_x + (x / edge) as usize;
                if zones.contains(&zone) { out.push((zone,pos)); break; }
            } else if liquid.is_none_or(|l| l.level < 0.0) { out.push((zone.expect("selected district zone"),pos)); break; }
        }
    }
    out
}

struct WorldEnvironment<'a> {
    surface: &'a Collision, zones: &'a ZoneLocator, districts: Option<&'a Districts>,
    characters: &'a [CharacterSensor], dangers: &'a [CharacterSensor], corpses: &'a [Vec3], wind: Vec3,
}
impl Environment for WorldEnvironment<'_> {
    fn align(&self, old: Vec3, new: Vec3, body: &ao_formats::playfield::collision::Body, state: &mut ao_formats::playfield::collision::SurfaceState) -> ao_formats::playfield::collision::Aligned {
        // SurfaceState.last remains in collision coordinates across steps.
        let mut aligned = self.surface.align(mirror(old).to_array(), mirror(new).to_array(), body, state);
        aligned.pos = mirror(Vec3::from_array(aligned.pos)).to_array();
        aligned.normal = mirror(Vec3::from_array(aligned.normal)).to_array();
        aligned
    }
    fn ground(&self, pos: Vec3) -> Option<(Vec3,Vec3)> { self.surface.closest(mirror(pos).to_array(), -1).map(|p| (mirror(Vec3::from_array(p.pos)), mirror(Vec3::from_array(p.normal)))) }
    fn ground_ray(&self, origin: Vec3) -> Option<(Vec3,Vec3)> { self.surface.line(mirror(origin).to_array(), [origin.x,-0.1,-origin.z]).map(|p| (mirror(Vec3::from_array(p.p)),mirror(Vec3::from_array(p.n)))) }
    fn liquid(&self, pos: Vec3) -> Option<f32> { self.surface.liquid_surface_at(mirror(pos).to_array()).map(|l| l.level) }
    fn district(&self, pos: Vec3) -> Option<usize> { Some(*self.districts?.zone_to_district.get(self.zones.zone_at(mirror(pos).to_array())?)? as usize) }
    fn obstacle(&self, pos: Vec3, ahead: Vec3) -> Option<(Vec3,Vec3)> { self.surface.line(mirror(pos).to_array(), mirror(pos+ahead).to_array()).map(|p| (mirror(Vec3::from_array(p.p)),mirror(Vec3::from_array(p.n)))) }
    fn alignment(&self, pos: Vec3) -> Vec3 {
        let diagonal = std::f32::consts::FRAC_1_SQRT_2;
        let hits = [Vec3::new(-1.0,0.0,0.0), Vec3::new(diagonal,0.0,diagonal), Vec3::new(diagonal,0.0,-diagonal)].map(|direction| {
            let start = pos + Vec3::Y * 0.4 + direction * 0.2;
            let end = pos - Vec3::Y * 0.4 + direction * 0.8;
            self.surface.line(mirror(start).to_array(), mirror(end).to_array()).map_or(end, |hit| mirror(Vec3::from_array(hit.p)))
        });
        let normal = (hits[1]-hits[0]).cross(hits[2]-hits[0]).normalize_or_zero();
        if normal.y < 0.0 { -normal } else { normal }
    }
    fn characters(&self) -> &[CharacterSensor] { self.characters }
    fn dangers(&self) -> &[CharacterSensor] { self.dangers }
    fn corpses(&self) -> &[Vec3] { self.corpses }
    fn wind(&self) -> Vec3 { self.wind }
    fn landing(&self, pos: Vec3, radius: f32) -> Option<Vec3> { self.corpses.iter().copied().find(|&p| p.distance_squared(pos) < radius*radius && self.district(p) == self.district(pos)) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::play::native_life_render::LifeModels;

    #[test]
    fn retail_authored_spawn_disable_and_reenable() {
        let client = std::path::PathBuf::from(std::env::var_os("AOMAC_CLIENT").unwrap_or_else(|| "Games/ProjectRubiKa/client".into()));
        let client = if client.is_absolute() { client } else { std::path::PathBuf::from(std::env::var_os("HOME").unwrap()).join(client) };
        if !client.join("cd_image/rdb.db").exists() { return; }
        let store = RecordStore::open(&client).unwrap();
        let collision = Collision::load(&store, 566).unwrap();
        let mut world = LifeWorld::load(&store, 566).unwrap();
        let mut models = LifeModels::default();
        for index in 0..world.catalog.len() {
            if world.catalog_used(index) {
                let mesh = world.catalog[index].mesh;
                models.load(&store, mesh).unwrap();
                world.set_mesh_height(mesh, models.height(mesh).unwrap());
                world.set_mesh_animation(mesh, models.boundaries(mesh), models.total_time(mesh).unwrap());
            }
        }
        world.set_enabled(true);
        world.prepare(&collision);
        world.prepared = true;
        let control = world.requests.iter().skip(2).find_map(|requests| requests.first().map(|r| r.pos)).expect("retail district contains authored wildlife requests");
        world.tick(0.001, control, &collision, &[], &[], &[], Vec3::ZERO);
        assert!(!world.species.is_empty());
        assert_eq!(world.species.len(), world.ids.len());
        assert_eq!(world.species.len(), world.states.len());
        assert!(world.species.iter().all(|s| s.body_radius == world.heights[s.catalog_index] * 0.5));
        assert!(world.species.iter().any(|s| s.body_radius > 0.0));
        let last_id = *world.ids.last().unwrap();
        world.set_enabled(false);
        assert!(world.species.is_empty() && world.ids.is_empty() && world.states.is_empty());
        world.tick(0.001, control, &collision, &[], &[], &[], Vec3::ZERO);
        assert!(world.species.is_empty());
        world.set_enabled(true);
        world.rng = Rng(1);
        world.prepare(&collision);
        world.prepared = true;
        world.tick(0.001, control, &collision, &[], &[], &[], Vec3::ZERO);
        assert!(!world.species.is_empty());
        assert!(world.ids.iter().all(|&id| id > last_id));
    }
}
