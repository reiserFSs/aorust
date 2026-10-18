//! CPU port of ao-vk rvk/shaders/water_sim.comp (MIT; repository attribution).
//! Per-native-surface state: no inferred/nearby actors, and no actor history survives removal.
use ao_scene::liquid::Surface;
use glam::{Vec2, Vec3};
use std::collections::HashMap;

const N: i32 = 128;
const CELL: f32 = 0.35;
const COUNT: usize = (N * N) as usize;
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WakeSettings {
    pub strength: f32,
    pub visibility: f32,
    pub duration: f32,
    pub speed: f32,
    pub treading: f32,
    pub splash: f32,
}
impl Default for WakeSettings {
    fn default() -> Self {
        Self { strength: 2.0, visibility: 2.0, duration: 0.3, speed: 3.6, treading: 1.0, splash: 1.0 }
    }
}

/// Only currently LOS-admitted native wet contacts may be supplied. Positions are
/// world-space feet. Native surface and swimming mode are authoritative, independent inputs.
#[derive(Clone, Copy, Debug)]
pub struct WaterBody {
    pub id: u64,
    pub position: [f32; 3],
    pub previous_position: [f32; 3],
    pub velocity: [f32; 3],
    pub surface: f32,
    pub swimming: bool,
}
impl WaterBody {
    fn valid(&self) -> bool {
        self.surface.is_finite() && self.position[1] < self.surface
            && self.position.iter().chain(&self.previous_position).chain(&self.velocity).all(|v| v.is_finite())
    }
}

#[derive(Clone, Copy, Debug)]
pub struct FootSegment {
    pub start: [f32; 3],
    pub end: [f32; 3],
    pub outward: [f32; 2],
    /// Actual connected static sheet top minus its foot height, in metres.
    pub drop: f32,
}

/// Non-emissive spray geometry; renderer must apply native scene lighting/fog and
/// depth-test it. These records originate exclusively in loaded static triangles.
#[derive(Clone, Copy, Debug)]
pub struct SprayRecord {
    pub position: [f32; 3],
    pub radius: f32,
    pub opacity: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct DynamicsSample {
    pub displacement: f32,
    pub normal: [f32; 3],
    pub foam: f32,
}
impl Default for DynamicsSample {
    fn default() -> Self { Self { displacement: 0.0, normal: [0.0, 1.0, 0.0], foam: 0.0 } }
}

pub struct WaterDynamics {
    triangles: Vec<[Vec3; 3]>,
    feet: Vec<FootSegment>,
    spray: Vec<SprayRecord>,
    state: Vec<[f32; 2]>,
    next: Vec<[f32; 2]>,
    level: Vec<f32>,
    origin: Option<[i32; 2]>,
    admitted: Vec<u64>,
    incoming: Vec<u64>,
    body_levels: Vec<f32>,
    switches: (bool, bool),
    settings: WakeSettings,
    time: f32,
}

fn index(x: i32, z: i32) -> usize { (z.rem_euclid(N) * N + x.rem_euclid(N)) as usize }
fn xz(v: Vec3) -> Vec2 { Vec2::new(v.x, v.z) }
fn cross2(a: Vec2, b: Vec2) -> f32 { a.x * b.y - a.y * b.x }

/// Exact barycentric membership, not a bounding box or a filtered water map.
fn triangle_height(t: &[Vec3; 3], p: Vec2) -> Option<f32> {
    let a = xz(t[0]);
    let b = xz(t[1]) - a;
    let c = xz(t[2]) - a;
    let den = cross2(b, c);
    if den.abs() < 1e-8 { return None; }
    let u = cross2(p - a, c) / den;
    let v = cross2(b, p - a) / den;
    if u < 0.0 || v < 0.0 || u + v > 1.0 { return None; }
    Some(t[0].y + u * (t[1].y - t[0].y) + v * (t[2].y - t[0].y))
}

impl WaterDynamics {
    pub fn set_settings(&mut self, settings: WakeSettings) {
        let defaults = WakeSettings::default();
        let bound = |v: f32, lo: f32, hi: f32, default: f32| if v.is_finite() { v.clamp(lo, hi) } else { default };
        let settings = WakeSettings {
            strength: bound(settings.strength, 0.0, 4.0, defaults.strength),
            visibility: bound(settings.visibility, 0.0, 4.0, defaults.visibility),
            duration: bound(settings.duration, 0.05, 2.0, defaults.duration),
            speed: bound(settings.speed, 0.1, 8.0, defaults.speed),
            treading: bound(settings.treading, 0.0, 3.0, defaults.treading),
            splash: bound(settings.splash, 0.0, 3.0, defaults.splash),
        };
        if settings != self.settings { self.clear(); self.settings = settings; }
    }
    pub fn new(surface: &Surface) -> Self {
        let vertices: Vec<_> = if matches!(surface.style, 0 | 4) { surface.triangles().collect() } else { Vec::new() };
        let triangles = vertices.iter().filter_map(|v| {
            let t = v.map(|v| Vec3::from_array(v.pos));
            let n = (t[1] - t[0]).cross(t[2] - t[0]).normalize_or_zero();
            (n.y.abs() >= 0.9).then_some(t)
        }).collect();
        let feet = waterfall_feet(&vertices);
        Self { triangles, feet, spray: Vec::new(), state: vec![[0.0; 2]; COUNT], next: vec![[0.0; 2]; COUNT],
            level: vec![f32::NAN; COUNT], origin: None, admitted: Vec::new(), incoming: Vec::new(), body_levels: Vec::new(), switches: (false, false), settings: WakeSettings::default(), time: 0.0 }
    }

    pub fn spray(&self) -> &[SprayRecord] { &self.spray }

    pub fn clear(&mut self) {
        self.state.fill([0.0; 2]);
        self.next.fill([0.0; 2]);
        self.spray.clear();
    }

    fn rebuild(&mut self, origin: [i32; 2]) {
        let previous = self.origin;
        self.level.fill(f32::NAN);
        // Rasterize the actual loaded triangles only when the world-aligned window moves.
        for t in &self.triangles {
            let lo = t.iter().fold(Vec3::splat(f32::INFINITY), |a, b| a.min(*b));
            let hi = t.iter().fold(Vec3::splat(f32::NEG_INFINITY), |a, b| a.max(*b));
            let x0 = ((lo.x / CELL - 0.5).ceil() as i32).max(origin[0]);
            let z0 = ((lo.z / CELL - 0.5).ceil() as i32).max(origin[1]);
            let x1 = ((hi.x / CELL - 0.5).floor() as i32).min(origin[0] + N - 1);
            let z1 = ((hi.z / CELL - 0.5).floor() as i32).min(origin[1] + N - 1);
            for z in z0..=z1 { for x in x0..=x1 {
                if let Some(y) = triangle_height(t, Vec2::new(x as f32 + 0.5, z as f32 + 0.5) * CELL) {
                    let i = index(x, z);
                    if !self.level[i].is_finite() || y > self.level[i] { self.level[i] = y; }
                }
            }}
        }
        for z in origin[1]..origin[1] + N { for x in origin[0]..origin[0] + N {
            let retained = previous.is_some_and(|p| x >= p[0] && x < p[0] + N && z >= p[1] && z < p[1] + N);
            let i = index(x, z);
            if !retained || !self.level[i].is_finite() { self.state[i] = [0.0; 2]; self.next[i] = [0.0; 2]; }
        }}
        self.origin = Some(origin);
    }

    /// Call even with an empty admitted list, before sampling/uploading this frame.
    /// Storage is retained; admission order is immaterial. Every admission-set or
    /// switch change flushes the *whole* field, including propagated waves.
    pub fn update(&mut self, camera: [f32; 3], dt: f32, bodies: &[WaterBody], wakes: bool, waterfalls: bool) {
        self.incoming.clear();
        if wakes { self.incoming.extend(bodies.iter().filter(|b| b.valid()).map(|b| b.id)); }
        self.incoming.sort_unstable();
        self.incoming.dedup();
        if self.admitted != self.incoming || self.switches != (wakes, waterfalls) {
            self.clear();
            std::mem::swap(&mut self.admitted, &mut self.incoming);
        }
        self.switches = (wakes, waterfalls);
        if !wakes && !waterfalls { return; }
        if !camera.iter().all(|v| v.is_finite()) || !dt.is_finite() || dt <= 0.0 { return; }
        if camera[0].abs().max(camera[2].abs()) / CELL > (i32::MAX - 1024) as f32 { self.clear(); return; }
        let origin = [(camera[0] / CELL).floor() as i32 - N / 2, (camera[2] / CELL).floor() as i32 - N / 2];
        if self.origin != Some(origin) { self.rebuild(origin); }
        if dt > 1.0 { self.clear(); }
        self.body_levels.clear();
        if wakes {
            for body in bodies {
                let position = Vec3::from_array(body.position);
                let level = if body.valid() {
                    self.triangles.iter().filter_map(|t| triangle_height(t, xz(position)))
                        .filter(|y| (*y - body.surface).abs() <= CELL)
                        .min_by(|a, b| (a - body.surface).abs().total_cmp(&(b - body.surface).abs())).unwrap_or(f32::NAN)
                } else { f32::NAN };
                self.body_levels.push(level);
            }
        }
        // Same CFL limit, damped wave equation and diffusion as water_sim.comp.
        // ponytail: 128-cell local field; increase N if distant persistent wakes matter.
        let elapsed = dt.min(0.1);
        let steps = (elapsed / (0.6 * CELL / self.settings.speed).min(1.0 / 60.0)).ceil().max(1.0) as u32;
        let step = elapsed / steps as f32;
        for sub in 0..steps {
            self.time = (self.time + step).rem_euclid(3600.0);
            self.step(origin, step, bodies, sub == 0);
        }
        self.build_spray();
    }

    fn neighbour_height(&self, x: i32, z: i32, y: f32) -> f32 {
        let i = index(x, z);
        // Distinct stacked pools must not exchange waves.
        if (self.level[i] - y).abs() > CELL || !self.level[i].is_finite() { 0.0 } else { self.state[i][0] }
    }

    fn step(&mut self, origin: [i32; 2], dt: f32, bodies: &[WaterBody], splash: bool) {
        for z in origin[1]..origin[1] + N { for x in origin[0]..origin[0] + N {
            let i = index(x, z);
            let surface = self.level[i];
            if !surface.is_finite() || x < origin[0] + 3 || x >= origin[0] + N - 3 || z < origin[1] + 3 || z >= origin[1] + N - 3 {
                self.next[i] = [0.0; 2]; continue;
            }
            let p = Vec2::new(x as f32 + 0.5, z as f32 + 0.5) * CELL;
            let [mut h, mut v] = self.state[i];
            let wall = |x, z| {
                let j = index(x, z);
                if !self.level[j].is_finite() || (self.level[j] - surface).abs() > CELL { h } else { self.state[j][0] }
            };
            let lap = wall(x - 1, z) + wall(x + 1, z) + wall(x, z - 1) + wall(x, z + 1) - 4.0 * h;
            v += self.settings.speed * self.settings.speed * dt / (CELL * CELL) * lap;
            if self.switches.0 { for (b, body_level) in bodies.iter().zip(&self.body_levels) {
                if !body_level.is_finite() || (body_level - surface).abs() > CELL || (b.surface - surface).abs() > CELL { continue; }
                let pos = Vec3::from_array(b.position);
                let d = p - xz(pos);
                let r2 = d.length_squared();
                if r2 > 6.0 { continue; }
                let fall = -b.velocity[1];
                if splash && fall > 2.5 && b.previous_position[1] >= b.surface && pos.y < b.surface && r2 < 6.0 {
                    let size = self.settings.splash * ((fall - 1.5) / 4.0).clamp(0.2, 1.0);
                    h += size * 0.3 * (-(-r2 / (2.0 * 0.45 * 0.45)).exp() + 0.55 * (-((r2.sqrt() - 0.95) / 0.4).powi(2)).exp());
                }
                if !b.swimming || r2 > 2.25 { continue; }
                let velocity = Vec2::new(b.velocity[0], b.velocity[2]);
                let speed = velocity.length();
                let direction = if speed > 0.05 { velocity / speed } else { Vec2::ZERO };
                let g = (-r2 / 0.5).exp();
                let depth = b.surface - pos.y;
                let in_water = ((depth + 0.3) * 1.5).clamp(0.0, 1.0) * (depth * 0.8 + 0.25).clamp(0.0, 1.0);
                let movement = ((speed - 0.3) / 3.5).clamp(0.0, 1.5);
                let push = 5.0 * self.settings.strength * movement * d.dot(direction) / 0.5 * g;
                let phase = (b.id as u32) as f32 * 0.618034;
                let stir = 0.25 * self.settings.strength * self.settings.treading * (1.0 - movement.min(1.0)) * (self.time * 5.5 + phase).sin() * g;
                v += (push + stir) * in_water * dt;
            }}
            if self.switches.1 {
                let mut g = 0.0f32;
                for foot in &self.feet {
                    if (foot.start[1] - surface).abs() > 0.75 { continue; }
                    let a = xz(Vec3::from_array(foot.start));
                    let ab = xz(Vec3::from_array(foot.end)) - a;
                    let k = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
                    let d = p - a - ab * k;
                    let behind = (-d.dot(Vec2::from_array(foot.outward))).max(0.0);
                    let strength = (foot.drop / 6.0).sqrt().min(2.0);
                    g = g.max(strength * (-(d.length_squared() + 0.5 * behind * behind) / 1.62).exp());
                }
                if g > 0.0 { v += 1.5 * (hash(x as u32, z as u32, (self.time * 60.0) as u32) - 0.5) * g * dt * 60.0; }
            }
            v *= (-0.6 / self.settings.duration * dt).exp();
            h = (h + v * dt + 0.015 * lap).clamp(-0.4, 0.4);
            self.next[i] = [h, v];
        }}
        std::mem::swap(&mut self.state, &mut self.next);
    }

    pub fn sample(&self, position: [f32; 3]) -> DynamicsSample {
        let Some(origin) = self.origin else { return DynamicsSample::default(); };
        if !self.switches.0 && !self.switches.1 { return DynamicsSample::default(); }
        let p = Vec3::from_array(position);
        if !p.is_finite() { return DynamicsSample::default(); }
        let q = xz(p) / CELL - Vec2::splat(0.5);
        let x = q.x.floor() as i32;
        let z = q.y.floor() as i32;
        if x < origin[0] + 3 || z < origin[1] + 3 || x >= origin[0] + N - 4 || z >= origin[1] + N - 4 { return DynamicsSample::default(); }
        if !self.triangles.iter().any(|t| triangle_height(t, xz(p)).is_some_and(|y| (y - p.y).abs() <= 0.75)) { return DynamicsSample::default(); }
        let u = q.x - x as f32;
        let v = q.y - z as f32;
        let h00 = self.neighbour_height(x, z, p.y);
        let h10 = self.neighbour_height(x + 1, z, p.y);
        let h01 = self.neighbour_height(x, z + 1, p.y);
        let h11 = self.neighbour_height(x + 1, z + 1, p.y);
        let h = (h00 * (1.0 - u) + h10 * u) * (1.0 - v) + (h01 * (1.0 - u) + h11 * u) * v;
        let dx = ((h10 - h00) * (1.0 - v) + (h11 - h01) * v) / CELL;
        let dz = ((h01 - h00) * (1.0 - u) + (h11 - h10) * u) / CELL;
        let gain = if self.switches.0 { self.settings.visibility } else { 1.0 };
        DynamicsSample { displacement: h * gain, normal: Vec3::new(-dx * gain, 1.0, -dz * gain).normalize().to_array(), foam: (dx.hypot(dz) * 0.35 * gain).clamp(0.0, 1.0) }
    }

    fn build_spray(&mut self) {
        self.spray.clear();
        if !self.switches.1 { return; }
        let Some(origin) = self.origin else { return; };
        let low = Vec2::new(origin[0] as f32, origin[1] as f32) * CELL;
        let high = low + Vec2::splat(N as f32 * CELL);
        for (j, foot) in self.feet.iter().enumerate() {
            let a = Vec3::from_array(foot.start);
            let b = Vec3::from_array(foot.end);
            let horizontal = xz(b - a);
            let length = horizontal.length();
            if !length.is_finite() || length <= 1e-5 { continue; }
            let (enter, leave) = match clip_segment(xz(a), horizontal, low, high) {
                Some(interval) => interval,
                None => continue,
            };
            // Two-metre slots stay anchored to static geometry, not the camera's
            // clipped endpoint. Moving the camera never reseeds visible particles.
            let first = (enter * length / 2.0).floor() as i32;
            let last = (leave * length / 2.0).ceil() as i32;
            let strength = (foot.drop / 6.0).sqrt().min(2.0);
            for slot in first..last {
                for k in 0..12 {
                    if self.spray.len() >= 1024 { return; }
                    let seed = (j as u32).wrapping_mul(0x9e3779b9) ^ (slot as u32).wrapping_mul(12).wrapping_add(k);
                    let phase = (self.time * 0.8 + hash(seed, 7, 1)).fract();
                    let distance = (slot as f32 + hash(seed, 13, 2)) * 2.0;
                    if distance > length { continue; }
                    let out = Vec2::from_array(foot.outward) * phase * (0.5 + hash(seed, 19, 3)) * strength;
                    let mut p = a.lerp(b, distance / length) + Vec3::new(out.x, 3.0 * phase * (1.0 - phase) * strength, out.y);
                    if !xz(p).cmpge(low).all() || !xz(p).cmple(high).all() { continue; }
                    p.y += 0.1;
                    let mist = k >= 8;
                    let radius = (if mist { 0.3 + phase * 0.6 } else { 0.035 + phase * 0.09 }) * strength;
                    let opacity = if mist { 0.06 } else { 0.16 };
                    self.spray.push(SprayRecord { position: p.to_array(), radius, opacity: opacity * (1.0 - phase) * strength.min(1.0) });
                }
            }
        }
    }
}

/// Parametric interval intersecting the local XZ rectangle, without marching
/// over the invisible length of a wide authored waterfall.
fn clip_segment(a: Vec2, direction: Vec2, low: Vec2, high: Vec2) -> Option<(f32, f32)> {
    let mut enter = 0.0f32;
    let mut leave = 1.0f32;
    for axis in 0..2 {
        let d = direction[axis];
        if d.abs() < 1e-8 {
            if a[axis] < low[axis] || a[axis] > high[axis] { return None; }
        } else {
            let x = (low[axis] - a[axis]) / d;
            let y = (high[axis] - a[axis]) / d;
            enter = enter.max(x.min(y));
            leave = leave.min(x.max(y));
            if enter >= leave { return None; }
        }
    }
    Some((enter, leave))
}

fn hash(x: u32, z: u32, t: u32) -> f32 {
    let mut n = x.wrapping_mul(73856093) ^ z.wrapping_mul(19349663) ^ t.wrapping_mul(83492791);
    n ^= n >> 16; n = n.wrapping_mul(0x7feb352d); n ^= n >> 15; n = n.wrapping_mul(0x846ca68b); n ^= n >> 16;
    (n >> 8) as f32 / 16777216.0
}

fn waterfall_feet(vertices: &[[ao_scene::Vertex; 3]]) -> Vec<FootSegment> {
    // Connected exact steep native water faces. Native steep edges have zero
    // authored alpha; no minimum cascade size is inferred from observed draws.
    let steep: Vec<_> = vertices.iter().filter(|v| {
        let t = v.map(|v| Vec3::from_array(v.pos));
        (t[1] - t[0]).cross(t[2] - t[0]).normalize_or_zero().y.abs() < 0.9
    }).collect();
    let mut parent: Vec<_> = (0..steep.len()).collect();
    fn root(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i { parent[i] = parent[parent[i]]; i = parent[i]; } i
    }
    let mut weld = HashMap::new();
    for (i, triangle) in steep.iter().enumerate() { for v in triangle.iter() {
        let key = v.pos.map(|v| (v * 64.0).round() as i64);
        if let Some(&j) = weld.get(&key) { let a = root(&mut parent, i); let b = root(&mut parent, j); parent[a] = b; }
        else { weld.insert(key, i); }
    }}
    let mut limits = vec![(f32::INFINITY, f32::NEG_INFINITY, 0.0f32); steep.len()];
    for (i, triangle) in steep.iter().enumerate() {
        let r = root(&mut parent, i);
        for v in triangle.iter() { limits[r].0 = limits[r].0.min(v.pos[1]); limits[r].1 = limits[r].1.max(v.pos[1]); limits[r].2 = limits[r].2.max(v.color[3]); }
    }
    let mut feet = Vec::new();
    for (i, triangle) in steep.iter().enumerate() {
        let (bottom, top, alpha) = limits[root(&mut parent, i)];
        let drop = top - bottom;
        if alpha > 0.0 || drop <= 1e-5 { continue; }
        let t = triangle.map(|v| Vec3::from_array(v.pos));
        let n = xz((t[1] - t[0]).cross(t[2] - t[0])).normalize_or_zero();
        for e in 0..3 {
            let a = t[e]; let b = t[(e + 1) % 3];
            let foot_tolerance = (drop * 0.01).clamp(1e-5, 0.5);
            if a.y > bottom + foot_tolerance || b.y > bottom + foot_tolerance || xz(a - b).length_squared() < 1e-8 { continue; }
            if feet.iter().any(|f: &FootSegment| (f.start == a.to_array() && f.end == b.to_array()) || (f.end == a.to_array() && f.start == b.to_array())) { continue; }
            feet.push(FootSegment { start: a.to_array(), end: b.to_array(), outward: n.to_array(), drop: top - (a.y + b.y) * 0.5 });
        }
    }
    feet
}

#[cfg(test)]
mod tests {
    use super::*;
    use ao_scene::{liquid::SourceTriangle, Vertex};
    fn pool() -> Surface {
        let p = [[-20.0, 0.0, -20.0], [20.0, 0.0, -20.0], [20.0, 0.0, 20.0], [-20.0, 0.0, 20.0]];
        Surface::new(0, 0, [[0, 1, 2], [0, 2, 3]].map(|ids| SourceTriangle { kind: 0, vertices: ids.map(|i| Vertex { pos: p[i], ..Default::default() }) }).into()).unwrap()
    }
    fn swimmer() -> WaterBody { WaterBody { id: 7, position: [0.0, -0.2, 0.0], previous_position: [0.0, 0.1, 0.0], velocity: [4.0, -4.0, 0.0], surface: 0.0, swimming: true } }
    #[test]
    fn native_wet_entry_splashes_before_swim_mode_but_walkers_do_not_stir() {
        let mut sim = WaterDynamics::new(&pool());
        let mut contact = swimmer();
        contact.swimming = false;
        sim.update([0.0; 3], 0.05, &[contact], true, false);
        assert!(sim.state.iter().any(|s| s[0].abs() > 0.01));
        let mut quiet = WaterDynamics::new(&pool());
        contact.previous_position = contact.position;
        contact.velocity = [0.0; 3];
        quiet.update([0.0; 3], 0.05, &[contact], true, false);
        assert!(quiet.state.iter().all(|s| *s == [0.0; 2]));
        contact.velocity = [4.0, 0.0, 0.0];
        quiet.update([0.0; 3], 0.05, &[contact], true, false);
        assert!(quiet.state.iter().all(|s| *s == [0.0; 2]));
        // A native contact in another stacked level must not poke this surface.
        contact.surface = 10.0;
        contact.position[1] = 9.8;
        contact.previous_position[1] = 10.1;
        contact.velocity = [0.0, -4.0, 0.0];
        quiet.update([0.0; 3], 0.05, &[contact], true, false);
        assert!(quiet.state.iter().all(|s| *s == [0.0; 2]));
    }

    #[test]
    fn bounded_solver_and_removed_actor_leaves_no_history() {
        let mut sim = WaterDynamics::new(&pool());
        for _ in 0..60 { sim.update([0.0; 3], 1.0 / 60.0, &[swimmer()], true, false); }
        assert!(sim.state.iter().any(|s| s[0].abs() > 0.001));
        assert!(sim.state.iter().all(|s| s[0].is_finite() && s[0].abs() <= 0.4 && s[1].is_finite()));
        sim.update([0.0; 3], 1.0 / 60.0, &[], true, false);
        assert!(sim.state.iter().all(|s| *s == [0.0; 2]));
        assert_eq!(sim.sample([0.0; 3]).displacement, 0.0);
        assert!(sim.spray().is_empty());
    }
    #[test]
    fn exact_triangle_membership_and_disabled_path() {
        let t = [Vec3::ZERO, Vec3::X * 10.0, Vec3::Z * 10.0];
        assert!(triangle_height(&t, Vec2::new(2.0, 2.0)).is_some());
        assert!(triangle_height(&t, Vec2::new(8.0, 8.0)).is_none());
        let mut sim = WaterDynamics::new(&pool());
        sim.update([0.0; 3], 0.05, &[swimmer()], false, false);
        assert!(sim.state.iter().all(|s| *s == [0.0; 2]));
        assert_eq!(sim.sample([0.0; 3]).normal, [0.0, 1.0, 0.0]);
        sim.update([0.0; 3], 0.05, &[swimmer()], true, false);
        sim.update([0.0; 3], 0.05, &[swimmer()], false, false);
        assert!(sim.state.iter().all(|s| *s == [0.0; 2]));
        sim.set_settings(WakeSettings { speed: f32::NAN, duration: -1.0, ..Default::default() });
        assert_eq!(sim.settings.speed, WakeSettings::default().speed);
        assert_eq!(sim.settings.duration, 0.05);
    }
    #[test]
    fn splash_crossing_is_an_impulse_and_camera_wrap_drops_old_cells() {
        let mut sim = WaterDynamics::new(&pool());
        sim.update([0.0; 3], 0.05, &[swimmer()], true, false);
        assert!(sim.state.iter().any(|s| s[0].abs() > 0.01));
        sim.update([100.0, 0.0, 100.0], 0.05, &[swimmer()], true, false);
        assert!(sim.state.iter().all(|s| *s == [0.0; 2]));
        assert_eq!(sim.sample([100.0, 0.0, 100.0]).displacement, 0.0);
    }
    #[test]
    fn shore_reflects_and_bank_actor_cannot_push_pool() {
        let mut sim = WaterDynamics::new(&pool());
        let mut bank = swimmer();
        bank.position[0] = 20.1;
        bank.previous_position[0] = 20.1;
        sim.update([19.0, 0.0, 0.0], 0.05, &[bank], true, false);
        assert!(sim.state.iter().all(|s| *s == [0.0; 2]));
        // A constant wet field adjoining a dry cell has zero Laplacian under
        // the reflecting shore condition, not a drain into the dry neighbour.
        sim.state.fill([0.1, 0.0]);
        sim.level.fill(0.0);
        let origin = sim.origin.unwrap();
        let x = origin[0] + 8;
        let z = origin[1] + 8;
        sim.level[index(x - 1, z)] = f32::NAN;
        sim.step(origin, 1.0 / 60.0, &[], false);
        assert!((sim.state[index(x, z)][0] - 0.1).abs() < 1e-6);
    }

    #[test]
    fn visible_swimmer_eleven_metres_from_camera_remains_inside_field() {
        let mut sim = WaterDynamics::new(&pool());
        let mut body = swimmer();
        body.position[0] = 11.0;
        body.previous_position[0] = 11.0;
        sim.update([0.0; 3], 0.05, &[body], true, false);
        assert!(sim.sample([11.0, 0.0, 0.0]).displacement.abs() > 0.001);
        sim.update([0.0; 3], 0.05, &[], true, false);
        assert_eq!(sim.sample([11.0, 0.0, 0.0]).displacement, 0.0);
    }

    #[test]
    fn wide_foot_crossing_window_emits_only_local_bounded_spray() {
        let mut sim = WaterDynamics::new(&pool());
        sim.feet = vec![FootSegment {
            start: [-10.0, 0.0, 0.0], end: [200.0, 0.0, 0.0],
            outward: [0.0, 1.0], drop: 6.0,
        }];
        sim.update([0.0; 3], 0.05, &[], false, true);
        assert!(!sim.spray().is_empty(), "foot midpoint is outside, but its visible section crosses the window");
        assert!(sim.spray().len() <= 1024);
        let origin = sim.origin.unwrap();
        let low = Vec2::new(origin[0] as f32, origin[1] as f32) * CELL;
        let high = low + Vec2::splat(N as f32 * CELL);
        assert!(sim.spray().iter().all(|record| {
            let p = xz(Vec3::from_array(record.position));
            p.cmpge(low).all() && p.cmple(high).all()
        }));
    }

    #[test]
    fn waterfall_records_include_gentle_short_static_cascades() {
        let vertices = [[0.0, 0.0, 0.0], [4.0, 0.0, 0.0], [4.0, 6.0, 0.0]];
        let face = vertices.map(|pos| Vertex { pos, color: [0.0; 4], ..Default::default() });
        let tall = waterfall_feet(&[face]);
        assert_eq!(tall.len(), 1);
        assert_eq!(tall[0].drop, 6.0);
        let opaque = face.map(|v| Vertex { color: [1.0; 4], ..v });
        assert!(waterfall_feet(&[opaque]).is_empty());
        let short = face.map(|v| Vertex { pos: [v.pos[0], v.pos[1] * 0.25, v.pos[2]], ..v });
        let small = waterfall_feet(&[short]);
        assert_eq!(small.len(), 1);
        assert_eq!(small[0].drop, 1.5);
        assert!((small[0].drop / 6.0).sqrt() < (tall[0].drop / 6.0).sqrt());
        let flat = face.map(|v| Vertex { pos: [v.pos[0], 0.0, v.pos[2]], ..v });
        assert!(waterfall_feet(&[flat]).is_empty());
    }
}
