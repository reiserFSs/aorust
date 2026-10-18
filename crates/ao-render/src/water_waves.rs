//! Opt-in water geometry, ported from ao-vk water_common.glsl and water.frag.
//! Geometry only: no lighting, opacity, actor positions, or scene-copy visibility.
use std::{collections::{BTreeMap, BTreeSet}, sync::LazyLock};

use ao_scene::{liquid::Surface, Vertex};
use glam::{DVec3, Vec2, Vec3};

#[derive(Clone, Copy, Debug)]
pub struct WaveOptions {
    pub waves: bool,
    pub ripples: bool,
    /// Local vertex spacing, for the reference wavelength anti-aliasing filter.
    pub spacing: f32,
    pub height_scale: f32,
    pub wavelength_scale: f32,
    pub view_distance: f32,
}

impl Default for WaveOptions {
    fn default() -> Self {
        Self { waves: false, ripples: false, spacing: 0.0, height_scale: 1.0, wavelength_scale: 1.0, view_distance: 0.0 }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct WaveSample {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    /// Reference Gerstner compression: one at rest, lower on sharp crests.
    pub fold: f32,
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn wind_axis(wind: [f32; 2]) -> Vec2 {
    Vec2::from_array(wind).try_normalize().unwrap_or(Vec2::X)
}

/// Eight reference Gerstner waves; returns displacement, unnormalised normal, fold.
/// This raw mathematical helper is not the fairness-constrained vertex API.
pub fn gerstner(xz: [f32; 2], time: f32, wind: [f32; 2], spacing: f32, height_scale: f32, wavelength_scale: f32) -> ([f32; 3], [f32; 3], f32) {
    const LAMBDA: [f32; 8] = [31.0, 23.0, 17.0, 11.5, 7.7, 5.3, 3.7, 2.6];
    const ANGLE: [f32; 8] = [0.0, 0.55, -0.42, 0.95, -0.8, 0.25, -1.2, 1.35];
    const HEIGHT: [f32; 8] = [1.0, 0.75, 0.6, 0.42, 0.3, 0.2, 0.13, 0.09];
    const PHASE: [f32; 8] = [0.0, 1.7, 4.1, 2.3, 5.5, 0.9, 3.3, 6.0];
    let wd = wind_axis(wind);
    let p = Vec2::from_array(xz);
    let mut offset = Vec3::ZERO;
    let mut sum = Vec3::ZERO;
    for i in 0..8 {
        let length = LAMBDA[i] * wavelength_scale.max(0.001);
        let fade = 1.0 - smoothstep(length * 0.18, length * 0.4, spacing);
        if fade <= 0.0 { continue; }
        let k = std::f32::consts::TAU / length;
        let (sa, ca) = ANGLE[i].sin_cos();
        let d = Vec2::new(wd.x * ca - wd.y * sa, wd.x * sa + wd.y * ca);
        let amplitude = 0.012 * length * HEIGHT[i] * height_scale.max(0.0) * fade;
        let theta = k * d.dot(p) - (9.81 * k).sqrt() * time + PHASE[i];
        let (s, c) = theta.sin_cos();
        offset += Vec3::new(0.55 * amplitude * d.x * c, amplitude * s, 0.55 * amplitude * d.y * c);
        sum += Vec3::new(d.x * k * amplitude * c, 0.55 * k * amplitude * s, d.y * k * amplitude * c);
    }
    (offset.to_array(), Vec3::new(-sum.x, 1.0 - sum.y, -sum.z).to_array(), 1.0 - sum.y)
}

/// Animate only ordinary/shallow water. Callers retain native vertex alpha and UVs.
/// Shores are pinned in all three axes; troughs are clipped at the native surface
/// rather than exposing geometry the native opaque water would have covered.
pub fn animate(position: [f32; 3], normal: [f32; 3], time: f32, wind: [f32; 2], shore_distance: f32, options: WaveOptions) -> WaveSample {
    let mut sample = WaveSample { position, normal, fold: 1.0 };
    if (!options.waves && !options.ripples) || shore_distance <= 0.0 || is_waterfall(normal) { return sample; }
    let shore = smoothstep(0.0, 3.0, shore_distance);
    let p = Vec3::from_array(position);
    let mut n = Vec3::from_array(normal);
    if options.waves {
        let (offset, wave_normal, fold) = gerstner([p.x, p.z], time, wind, options.spacing, options.height_scale, options.wavelength_scale);
        let offset = Vec3::from_array(offset) * shore;
        sample.position = (p + Vec3::new(offset.x, offset.y.max(0.0), offset.z)).to_array();
        // A clipped trough is flat, not the unmodified sine-wave slope.
        if offset.y > 0.0 { n += (Vec3::from_array(wave_normal) - Vec3::Y) * shore; }
        sample.fold = 1.0 + (fold - 1.0) * shore;
    }
    if options.ripples {
        let fade = 1.0 - smoothstep(25.0, 220.0, options.view_distance);
        if fade > 0.0 {
            let slope = Vec2::from_array(detail_ripples([p.x, p.z], time, wind)) * (0.22 * fade * shore);
            n += Vec3::new(-slope.x, 0.0, -slope.y);
        }
    }
    sample.normal = n.try_normalize().unwrap_or(Vec3::Y).to_array();
    sample
}

/// Reference detail RG texture (256², deterministic 48-wave spectrum). Cached
/// once, not rebuilt/evaluated trigonometrically for every animated vertex.
fn detail_texture() -> &'static [[f32; 2]] {
    static DETAIL: LazyLock<Vec<[f32; 2]>> = LazyLock::new(|| {
        let mut state = 12345u32;
        let mut next = || {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            (state >> 8) as f32 / 16777216.0
        };
        let waves: [(Vec2, f32, f32); 48] = std::array::from_fn(|_| {
            let mut k = Vec2::ZERO;
            while k == Vec2::ZERO { k = Vec2::new((next() * 25.0) as i32 as f32 - 12.0, (next() * 25.0) as i32 as f32 - 12.0); }
            (k, 1.0 / k.length().powf(1.6), next() * std::f32::consts::TAU)
        });
        let mut texture = vec![[0.0; 2]; 256 * 256];
        let mut maximum = 0.0f32;
        for (i, pixel) in texture.iter_mut().enumerate() {
            let uv = Vec2::new((i % 256) as f32, (i / 256) as f32) / 256.0;
            let mut slope = Vec2::ZERO;
            for &(k, amplitude, phase) in &waves {
                slope += k * (amplitude * (std::f32::consts::TAU * k.dot(uv) + phase).cos());
            }
            maximum = maximum.max(slope.abs().max_element());
            *pixel = slope.to_array();
        }
        for pixel in &mut texture {
            for value in pixel {
                // Same RGBA8 round-trip as MakeDetail, before bilinear sampling.
                let encoded = ((0.5 + 0.5 * *value / maximum).clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                *value = f32::from(encoded) / 255.0 * 2.0 - 1.0;
            }
        }
        texture
    });
    &DETAIL
}

fn detail_sample(uv: Vec2) -> Vec2 {
    let texture = detail_texture();
    let p = uv.rem_euclid(Vec2::ONE) * 256.0 - Vec2::splat(0.5);
    let base = p.floor();
    let f = p - base;
    let at = |x: i32, y: i32| Vec2::from_array(texture[y.rem_euclid(256) as usize * 256 + x.rem_euclid(256) as usize]);
    let x = base.x as i32;
    let y = base.y as i32;
    at(x, y).lerp(at(x + 1, y), f.x).lerp(at(x, y + 1).lerp(at(x + 1, y + 1), f.x), f.y)
}

/// Independent normal-only ripples, with the reference three drift layers.
pub fn detail_ripples(xz: [f32; 2], time: f32, wind: [f32; 2]) -> [f32; 2] {
    let p = Vec2::from_array(xz);
    let wd = wind_axis(wind);
    let wp = Vec2::new(-wd.y, wd.x);
    (detail_sample(p * 0.11 + wd * (time * 0.021)) * 0.55
        + detail_sample(p * 0.037 - wp * (time * 0.011) + Vec2::new(0.37, 0.71)) * 0.75
        + detail_sample(p * 0.29 + (wd + wp) * (time * 0.035) + Vec2::new(0.13, 0.53)) * 0.3).to_array()
}

/// Exact, undirected loaded-topology boundary, not a mesh bounding rectangle.
/// Distance uses XYZ so vertically stacked pools do not share a false shoreline.
#[derive(Default, Debug)]
pub struct Shoreline {
    pub edges: Vec<[[f32; 3]; 2]>,
}

fn point_key(p: [f32; 3]) -> [u32; 3] {
    p.map(|x| if x == 0.0 { 0 } else { x.to_bits() })
}

impl Shoreline {
    pub fn new(surface: &Surface) -> Self {
        Self::from_triangles(surface.triangles().map(|triangle| triangle.map(|vertex| vertex.pos)))
    }

    /// Topological edges plus exact full-fine native terrain/water contact.
    /// Work visits only terrain cells in each source triangle's overlapping XZ
    /// bounds, then clips every intersection to the actual liquid footprint.
    /// Independent of camera-dependent ROAM simplification.
    pub fn new_with_ground(surface: &Surface, patches: &[ao_scene::ground_detail::GroundPatch]) -> Self {
        let mut shore = Self::new(surface);
        let mut edges: BTreeMap<_, _> = shore.edges.drain(..).map(|edge| (edge_key(edge[0], edge[1]), edge)).collect();
        for triangle in surface.triangles() {
            let positions = triangle.map(|vertex| vertex.pos);
            let [a, b, c] = positions.map(Vec3::from_array);
            let normal = (b - a).cross(c - a);
            if normal.length_squared() == 0.0 || is_waterfall(normal.to_array()) { continue; }
            let plane = ContactPlane::new(positions);
            let low = a.min(b).min(c);
            let high = a.max(b).max(c);
            for patch in patches {
                if patch.cells == 0 || patch.cell_size <= 0.0 || patch.height_scale <= 0.0 { continue; }
                let size = patch.cell_size;
                let x0 = patch.origin[0] as f32 * size;
                let z0 = -(patch.origin[1] as f32) * size;
                let side = patch.cells as f32 * size;
                if high.x < x0 || low.x > x0 + side || low.z > z0 || high.z < z0 - side { continue; }
                let last = patch.cells - 1;
                let start_x = (((low.x - x0) / size).floor().max(0.0) as usize).min(last);
                let end_x = (((high.x - x0) / size).floor().max(0.0) as usize).min(last);
                let start_z = (((z0 - high.z) / size).floor().max(0.0) as usize).min(last);
                let end_z = (((z0 - low.z) / size).floor().max(0.0) as usize).min(last);
                for z in start_z..=end_z {
                    for x in start_x..=end_x {
                        let offset = z * (patch.cells + 1) + x;
                        let corners = [offset, offset + 1, offset + patch.cells + 1, offset + patch.cells + 2];
                        if corners.iter().any(|&index| index >= patch.heights.len()) { continue; }
                        let heights = corners.map(|index| f32::from(patch.heights[index]) * (patch.height_scale / 256.0));
                        if heights.iter().all(|&y| y < low.y) || heights.iter().all(|&y| y > high.y) { continue; }
                        let points: [DVec3; 4] = std::array::from_fn(|i| DVec3::new(
                            f64::from(x0 + (x + i % 2) as f32 * size),
                            f64::from(heights[i]),
                            f64::from(z0 - (z + i / 2) as f32 * size)));
                        let gx = patch.origin[0].wrapping_add(x as i32) as u32;
                        let gz = patch.origin[1].wrapping_add(z as i32) as u32;
                        let triangles = if (!gz ^ gx) & 1 == 0 { [[0, 1, 2], [1, 3, 2]] }
                            else { [[0, 1, 3], [0, 3, 2]] };
                        for terrain in triangles {
                            if let Some(edge) = plane.contact(terrain.map(|i| points[i])) {
                                edges.insert(edge_key(edge[0], edge[1]), edge);
                            }
                        }
                    }
                }
            }
        }
        shore.edges = edges.into_values().collect();
        shore
    }

    pub fn from_triangles(triangles: impl IntoIterator<Item = [[f32; 3]; 3]>) -> Self {
        let triangles: Vec<_> = triangles.into_iter().collect();
        let mut points = BTreeMap::new();
        for triangle in &triangles {
            for &point in triangle { points.insert(point_key(point), point); }
        }
        let mut counts = BTreeMap::<([u32; 3], [u32; 3]), usize>::new();
        for triangle in triangles {
            for i in 0..3 {
                let a = DVec3::from_array(triangle[i].map(f64::from));
                let b = DVec3::from_array(triangle[(i + 1) % 3].map(f64::from));
                let edge = b - a;
                let edge_squared = edge.length_squared();
                if edge_squared == 0.0 { continue; }
                // ponytail: load-time O(edges * vertices); spatial bins if large
                // authored water meshes warrant it. Splitting exact T junctions
                // prevents a coarse/refined interior seam becoming a shoreline.
                let mut splits = Vec::new();
                for (&key, &point) in &points {
                    let d = DVec3::from_array(point.map(f64::from)) - a;
                    let t = d.dot(edge) / edge_squared;
                    if (0.0..=1.0).contains(&t) && d.cross(edge) == DVec3::ZERO { splits.push((t, key)); }
                }
                splits.sort_by(|a, b| a.0.total_cmp(&b.0));
                for pair in splits.windows(2) {
                    let (a, b) = (pair[0].1, pair[1].1);
                    if a != b { *counts.entry(if a < b { (a, b) } else { (b, a) }).or_default() += 1; }
                }
            }
        }
        Self { edges: counts.into_iter().filter(|(_, count)| *count == 1).map(|((a, b), _)| [points[&a], points[&b]]).collect() }
    }

    pub fn distance(&self, position: [f32; 3]) -> f32 {
        let p = DVec3::from_array(position.map(f64::from));
        self.edges.iter().map(|edge| {
            let a = DVec3::from_array(edge[0].map(f64::from));
            let d = DVec3::from_array(edge[1].map(f64::from)) - a;
            let delta = p - a;
            let t = delta.dot(d) / d.length_squared();
            if (0.0..=1.0).contains(&t) && delta.cross(d) == DVec3::ZERO { return 0.0; }
            p.distance(a + d * t.clamp(0.0, 1.0)) as f32
        }).fold(f32::INFINITY, f32::min)
    }

    /// Cache alongside the current native refinement; no allocations per frame.
    pub fn distances(&self, vertices: &[Vertex], out: &mut Vec<f32>) {
        out.clear();
        out.extend(vertices.iter().map(|vertex| self.distance(vertex.pos)));
    }
}

struct ContactPlane {
    a: DVec3,
    ab: DVec3,
    ac: DVec3,
    normal: DVec3,
    dot00: f64,
    dot01: f64,
    dot11: f64,
    inverse: f64,
}

impl ContactPlane {
    fn new(points: [[f32; 3]; 3]) -> Self {
        let [a, b, c] = points.map(|point| DVec3::from_array(point.map(f64::from)));
        let ab = b - a;
        let ac = c - a;
        let cross = ab.cross(ac);
        Self { a, ab, ac, normal: cross.normalize(), dot00: ab.length_squared(),
            dot01: ab.dot(ac), dot11: ac.length_squared(), inverse: 1.0 / cross.length_squared() }
    }

    fn barycentric(&self, point: DVec3) -> [f64; 3] {
        let d = point - self.a;
        let ab = d.dot(self.ab);
        let ac = d.dot(self.ac);
        let u = (self.dot11 * ab - self.dot01 * ac) * self.inverse;
        let v = (self.dot00 * ac - self.dot01 * ab) * self.inverse;
        [1.0 - u - v, u, v]
    }

    fn contact(&self, terrain: [DVec3; 3]) -> Option<[[f32; 3]; 2]> {
        let distances = terrain.map(|point| (point - self.a).dot(self.normal));
        // A wholly coplanar ground face is not a unique shore contour.
        if distances.iter().all(|&d| d == 0.0) { return None; }
        let mut hits = [DVec3::ZERO; 3];
        let mut count = 0;
        for i in 0..3 {
            let j = (i + 1) % 3;
            let hit = if distances[i] == 0.0 { Some(terrain[i]) }
                else if (distances[i] < 0.0 && distances[j] > 0.0) || (distances[i] > 0.0 && distances[j] < 0.0) {
                    Some(terrain[i].lerp(terrain[j], distances[i] / (distances[i] - distances[j])))
                } else { None };
            if let Some(hit) = hit {
                if !hits[..count].contains(&hit) { hits[count] = hit; count += 1; }
            }
        }
        if count != 2 { return None; }
        let a = self.barycentric(hits[0]);
        let b = self.barycentric(hits[1]);
        let mut from = 0.0f64;
        let mut to = 1.0f64;
        for i in 0..3 {
            let slope = b[i] - a[i];
            if slope > 0.0 { from = from.max(-a[i] / slope); }
            else if slope < 0.0 { to = to.min(-a[i] / slope); }
            else if a[i] < 0.0 { return None; }
        }
        if from >= to { return None; }
        let edge = [hits[0].lerp(hits[1], from), hits[0].lerp(hits[1], to)]
            .map(|point| point.to_array().map(|value| value as f32));
        (edge[0] != edge[1]).then_some(edge)
    }
}

/// Reference WaterMapFor classification: |normal.y| / |normal| < 0.9.
pub fn is_waterfall(normal: [f32; 3]) -> bool {
    let n = Vec3::from_array(normal);
    n.length_squared() > 0.0 && n.y.abs() < 0.9 * n.length()
}

struct SheetTriangle {
    a: DVec3,
    ab: DVec3,
    ac: DVec3,
    plane: DVec3,
    dot00: f64,
    dot01: f64,
    dot11: f64,
    inverse: f64,
    tolerance: f64,
    barycentric_epsilon: f64,
    top: f32,
}

/// Source-connected falling sheets. Cache once with the loaded liquid surface;
/// sample `along` only when rebuilding enhanced vertices, not each frame.
#[derive(Default)]
pub struct WaterfallSheets {
    triangles: Vec<SheetTriangle>,
}

impl WaterfallSheets {
    pub fn new(surface: &Surface) -> Self {
        Self::from_triangles(surface.triangles().map(|triangle| triangle.map(|vertex| vertex.pos)))
    }

    fn from_triangles(triangles: impl IntoIterator<Item = [[f32; 3]; 3]>) -> Self {
        let triangles: Vec<_> = triangles.into_iter().filter(|triangle| {
            let [a, b, c] = triangle.map(Vec3::from_array);
            is_waterfall((b - a).cross(c - a).to_array())
        }).collect();
        let mut parents: Vec<_> = (0..triangles.len()).collect();
        fn root(parents: &mut [usize], mut index: usize) -> usize {
            while parents[index] != index {
                parents[index] = parents[parents[index]];
                index = parents[index];
            }
            index
        }
        let mut owners = BTreeMap::new();
        for (index, triangle) in triangles.iter().enumerate() {
            for &point in triangle {
                if let Some(&other) = owners.get(&point_key(point)) {
                    let a = root(&mut parents, index);
                    let b = root(&mut parents, other);
                    parents[a.max(b)] = a.min(b);
                } else { owners.insert(point_key(point), index); }
            }
        }
        let mut tops = vec![f32::NEG_INFINITY; triangles.len()];
        for (index, triangle) in triangles.iter().enumerate() {
            let group = root(&mut parents, index);
            for point in triangle { tops[group] = tops[group].max(point[1]); }
        }
        let triangles = triangles.into_iter().enumerate().map(|(index, triangle)| {
            let [a, b, c] = triangle.map(|point| DVec3::from_array(point.map(f64::from)));
            let ab = b - a;
            let ac = c - a;
            let dot00 = ab.length_squared();
            let dot01 = ab.dot(ac);
            let dot11 = ac.length_squared();
            let tolerance = (a.abs().max(b.abs()).max(c.abs()).max_element() * f64::from(f32::EPSILON) * 4.0).max(0.0001);
            let cross = ab.cross(ac);
            SheetTriangle { a, ab, ac, plane: cross.normalize(), dot00, dot01, dot11,
                inverse: 1.0 / cross.length_squared(), tolerance,
                barycentric_epsilon: tolerance / dot00.min(dot11).sqrt(),
                top: tops[root(&mut parents, index)] }
        }).collect();
        Self { triangles }
    }

    /// Height dropped from this sheet's own lip, continuous over source seams.
    /// Membership uses its exact triangle plane and barycentrics; no mesh bounds
    /// or nearest unrelated waterfall can assign flow to a different surface.
    pub fn along(&self, position: [f32; 3]) -> f32 {
        let p = DVec3::from_array(position.map(f64::from));
        let mut best = f64::INFINITY;
        let mut along = 0.0;
        for triangle in &self.triangles {
            let d = p - triangle.a;
            let error = d.dot(triangle.plane).abs();
            if error > triangle.tolerance || error >= best { continue; }
            let dab = d.dot(triangle.ab);
            let dac = d.dot(triangle.ac);
            let u = (triangle.dot11 * dab - triangle.dot01 * dac) * triangle.inverse;
            let v = (triangle.dot00 * dac - triangle.dot01 * dab) * triangle.inverse;
            let epsilon = triangle.barycentric_epsilon;
            if u >= -epsilon && v >= -epsilon && u + v <= 1.0 + epsilon {
                best = error;
                along = (triangle.top - position[1]).max(0.0);
            }
        }
        along
    }
}

/// Reference gravity-accelerated falling-sheet flow. `along` is distance from lip.
#[cfg(test)]
pub fn fall_tau(along: f32, speed: f32) -> f32 {
    ((9.0 + 19.6 * along.max(0.0)).sqrt() - 3.0) / (9.8 * speed.max(0.05))
}

#[cfg(test)]
pub fn fall_speed(along: f32, speed: f32) -> f32 {
    (9.0 + 19.6 * along.max(0.0)).sqrt() * speed.max(0.05)
}

/// Falling-sheet detail coordinates, independent of waves/ripples switches.
/// Parent applies only with waterfalls enabled, never changes sheet opacity.
#[cfg(test)]
pub fn fall_uv(along: f32, across: f32, time: f32, speed: f32) -> [[f32; 2]; 2] {
    let tau = fall_tau(along, speed);
    [[across * 0.45, (tau - time) * 0.9], [across * 1.15 + 0.3, (tau - time * 1.2) * 1.7 + 0.5]]
}

#[cfg(test)]
fn value_noise(p: Vec2) -> f32 {
    fn hash(x: i32, y: i32) -> f32 {
        let mut x = (x as u32).wrapping_mul(1664525).wrapping_add(1013904223);
        let mut y = (y as u32).wrapping_mul(1664525).wrapping_add(1013904223);
        x = x.wrapping_add(y.wrapping_mul(1664525));
        y = y.wrapping_add(x.wrapping_mul(1664525));
        x ^= x >> 16;
        y ^= y >> 16;
        x = x.wrapping_add(y.wrapping_mul(1664525));
        x ^= x >> 16;
        (x >> 8) as f32 / 16777216.0
    }
    let base = p.floor();
    let f = p - base;
    let f = f * f * (Vec2::splat(3.0) - 2.0 * f);
    let x = base.x as i32;
    let y = base.y as i32;
    let mix = |a: f32, b: f32, t: f32| a + (b - a) * t;
    mix(mix(hash(x, y), hash(x.wrapping_add(1), y), f.x),
        mix(hash(x, y.wrapping_add(1)), hash(x.wrapping_add(1), y.wrapping_add(1)), f.x), f.y)
}

/// Reference falling-sheet white-water mask, NOT emitted radiance or alpha.
/// The compositor must cap its contribution to native visible illumination.
#[cfg(test)]
pub fn fall_foam(along: f32, across: f32, time: f32, speed: f32) -> f32 {
    let tau = fall_tau(along, speed);
    let noise = |x, y| value_noise(Vec2::new(x, y));
    let columns = noise(across * 0.9, along * 0.03) * 0.2
        + noise(across * 2.3 + 7.0, along * 0.05 + 3.0) * 0.25
        + noise(across * 6.1 + 2.0, along * 0.12 + 9.0) * 0.3
        + noise(across * 13.0 + 4.0, along * 0.25 + 1.0) * 0.25;
    let lumps = noise(across * 1.3 + 11.0, (tau - time) * 1.2) * 0.6
        + noise(across * 2.9 + 3.0, (tau - time * 1.15) * 2.4) * 0.4;
    let clumps = noise(across * 3.4 + 17.0, (tau - time * 1.05) * 3.2)
        * noise(across * 8.3 + 5.0, (tau - time * 1.1) * 5.5);
    (columns + (lumps - 0.5) * 0.5 + clumps * clumps * 1.6 * smoothstep(1.0, 6.0, along)).clamp(0.0, 1.0)
}

fn edge_key(a: [f32; 3], b: [f32; 3]) -> ([u32; 3], [u32; 3]) {
    let a = point_key(a);
    let b = point_key(b);
    if a < b { (a, b) } else { (b, a) }
}

fn midpoint_vertex(a: Vertex, b: Vertex) -> Vertex {
    // Double addition is symmetric even when neighbouring source triangles have
    // distinct indices and opposite winding along the same geometric edge.
    fn average<const N: usize>(a: [f32; N], b: [f32; N]) -> [f32; N] {
        std::array::from_fn(|i| ((f64::from(a[i]) + f64::from(b[i])) * 0.5) as f32)
    }
    Vertex { pos: average(a.pos, b.pos), normal: average(a.normal, b.normal),
        uv: average(a.uv, b.uv), color: average(a.color, b.color) }
}

/// Cached enhanced-only conforming refinement of native liquid geometry.
/// Red/green edge splits preserve winding, every source triangle, and interpolated
/// native opacity. Exact-position marks cross duplicated material/UV vertices.
/// Rebuild on native geometry/camera-cell changes, never once per draw/pass.
///
/// Refinement stops BEFORE a whole round exceeds 150k triangles/250k vertices;
/// it never drops source triangles to meet the optional-detail ceiling.
pub fn refine(vertices: &[Vertex], indices: &[u32], camera: [f32; 3], out_vertices: &mut Vec<Vertex>, out_indices: &mut Vec<u32>) {
    const MAX_TRIANGLES: usize = 150_000;
    const MAX_VERTICES: usize = 250_000;
    out_vertices.clear();
    out_indices.clear();
    out_vertices.extend_from_slice(vertices);
    out_indices.extend_from_slice(indices);
    let camera = Vec2::new(camera[0], camera[2]);
    let mut next_indices = Vec::new();
    // Native camera refinement is dyadic. Recognising existing exact midpoint
    // positions also closes its coarse/fine T junctions before wave animation.
    for _ in 0..16 {
        let positions: BTreeSet<_> = out_vertices.iter().map(|vertex| point_key(vertex.pos)).collect();
        let mut marks = BTreeSet::new();
        for triangle in out_indices.as_chunks::<3>().0 {
            for i in 0..3 {
                let a = out_vertices[triangle[i] as usize];
                let b = out_vertices[triangle[(i + 1) % 3] as usize];
                let pa = Vec3::from_array(a.pos);
                let pb = Vec3::from_array(b.pos);
                let axz = Vec2::new(pa.x, pa.z);
                let bxz = Vec2::new(pb.x, pb.z);
                let edge = bxz - axz;
                let t = if edge.length_squared() > 0.0 {
                    ((camera - axz).dot(edge) / edge.length_squared()).clamp(0.0, 1.0)
                } else { 0.0 };
                let distance = camera.distance(axz + edge * t);
                let target = if distance <= 12.0 { 0.5 } else if distance <= 40.0 { 1.0 }
                    else if distance <= 100.0 { 4.0 } else { f32::INFINITY };
                let midpoint = point_key(midpoint_vertex(a, b).pos);
                let hanging = midpoint != point_key(a.pos) && midpoint != point_key(b.pos) && positions.contains(&midpoint);
                if pa.distance_squared(pb) > target * target || hanging {
                    marks.insert(edge_key(a.pos, b.pos));
                }
            }
        }
        if marks.is_empty() { break; }
        let mut masks = Vec::with_capacity(out_indices.len() / 3);
        let mut midpoint_edges = BTreeSet::new();
        let mut triangles = 0usize;
        for triangle in out_indices.as_chunks::<3>().0 {
            let mut mask = 0u8;
            for i in 0..3 {
                let a = triangle[i];
                let b = triangle[(i + 1) % 3];
                if marks.contains(&edge_key(out_vertices[a as usize].pos, out_vertices[b as usize].pos)) {
                    mask |= 1 << i;
                    midpoint_edges.insert(if a < b { (a, b) } else { (b, a) });
                }
            }
            triangles += 1 + mask.count_ones() as usize;
            masks.push(mask);
        }
        if triangles > MAX_TRIANGLES || out_vertices.len() + midpoint_edges.len() > MAX_VERTICES { break; }
        let mut midpoints = BTreeMap::new();
        for (a, b) in midpoint_edges {
            let midpoint = midpoint_vertex(out_vertices[a as usize], out_vertices[b as usize]);
            let index = out_vertices.len() as u32;
            out_vertices.push(midpoint);
            midpoints.insert((a, b), index);
        }
        let midpoint = |a, b| midpoints[&if a < b { (a, b) } else { (b, a) }];
        next_indices.clear();
        next_indices.reserve(triangles * 3);
        for (triangle, mask) in out_indices.as_chunks::<3>().0.iter().zip(masks) {
            let [a, b, c] = [triangle[0], triangle[1], triangle[2]];
            match mask.count_ones() {
                0 => next_indices.extend_from_slice(&[a, b, c]),
                1 => {
                    let i = mask.trailing_zeros() as usize;
                    let (a, b, c) = (triangle[i], triangle[(i + 1) % 3], triangle[(i + 2) % 3]);
                    let m = midpoint(a, b);
                    next_indices.extend_from_slice(&[a, m, c, m, b, c]);
                }
                2 => {
                    let i = (0..3).find(|&i| mask & (1 << i) != 0 && mask & (1 << ((i + 2) % 3)) != 0).unwrap();
                    let (a, b, c) = (triangle[i], triangle[(i + 1) % 3], triangle[(i + 2) % 3]);
                    let ab = midpoint(a, b);
                    let ca = midpoint(c, a);
                    next_indices.extend_from_slice(&[a, ab, ca, ab, b, c, ab, c, ca]);
                }
                _ => {
                    let ab = midpoint(a, b);
                    let bc = midpoint(b, c);
                    let ca = midpoint(c, a);
                    next_indices.extend_from_slice(&[a, ab, ca, ab, b, bc, ca, bc, c, ab, bc, ca]);
                }
            }
        }
        std::mem::swap(out_indices, &mut next_indices);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ground_contact_shore_is_inside_water_plane_and_clipped_to_footprint() {
        use ao_scene::{ground_detail::GroundPatch, liquid::SourceTriangle};
        let surface = Surface::new(0, 0, vec![SourceTriangle {
            kind: 0,
            vertices: [[0.0, 1.0, 0.0], [8.0, 1.0, 0.0], [0.0, 1.0, -8.0]]
                .map(|pos| Vertex { pos, ..Default::default() }),
        }]).unwrap();
        let patch = GroundPatch { origin: [0, 0], cells: 1, cell_size: 8.0,
            height_scale: 1.0, heights: vec![0, 0, 512, 512], ..Default::default() };
        let boundary = Shoreline::new(&surface);
        let shore = Shoreline::new_with_ground(&surface, std::slice::from_ref(&patch));
        assert!(boundary.distance([2.0, 1.0, -4.0]) > 1.0);
        assert_eq!(shore.distance([2.0, 1.0, -4.0]), 0.0);
        assert_eq!(shore.distance([4.0, 1.0, -4.0]), 0.0);
        assert_eq!(shore.distance([6.0, 1.0, -4.0]), boundary.distance([6.0, 1.0, -4.0]));
        assert!(shore.edges.iter().all(|edge| edge.iter().all(|p| p[0] >= 0.0 && -p[2] >= 0.0 && p[0] - p[2] <= 8.0)));
        let coplanar = GroundPatch { heights: vec![256; 4], ..patch };
        assert_eq!(Shoreline::new_with_ground(&surface, &[coplanar]).edges.len(), boundary.edges.len());
    }

    #[test]
    fn waterfall_flow_uses_connected_sheet_lip_not_triangle_or_mesh_top() {
        let a = [0.0, 10.0, 0.0];
        let b = [2.0, 10.0, 0.0];
        let c = [2.0, 0.0, 0.0];
        let d = [0.0, 0.0, 0.0];
        let sheets = WaterfallSheets::from_triangles([
            [a, b, c], [a, c, d], [[0.0, 15.0, 0.0], b, a],
            [[10.0, 20.0, 0.0], [12.0, 20.0, 0.0], [12.0, 0.0, 0.0]],
        ]);
        assert_eq!(sheets.along([1.0, 5.0, 0.0]), 10.0);
        assert_eq!(sheets.along([0.5, 2.0, 0.0]), 13.0);
        assert_eq!(sheets.along([11.0, 15.0, 0.0]), 5.0);
        assert_eq!(sheets.along([1.0, 5.0, 2.0]), 0.0);
        assert_eq!(sheets.along([8.0, 5.0, 0.0]), 0.0);
        assert_eq!(sheets.along([1.0, 5.0, 0.00001]), 10.0);
        assert_eq!(WaterfallSheets::default().along([0.0; 3]), 0.0);
    }

    #[test]
    fn refinement_closes_native_t_junctions_preserves_area_and_opacity() {
        let vertices: Vec<_> = [[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [2.0, 0.0, -2.0],
            [0.0, 0.0, -2.0], [1.0, 0.0, -1.0]].map(|pos| Vertex {
                pos, normal: [0.0, 1.0, 0.0], color: [0.2, 0.3, 0.4, 0.8], ..Default::default()
            }).into();
        let mut refined = Vec::new();
        let mut indices = Vec::new();
        refine(&vertices, &[0, 1, 2, 0, 4, 3, 4, 2, 3], [1.0, 2.0, -1.0], &mut refined, &mut indices);
        assert!(indices.len() > 9 && indices.len() / 3 <= 150_000);
        assert!(refined.len() <= 250_000);
        assert!(refined.iter().all(|vertex| vertex.pos[1] == 0.0 && vertex.color[3] == 0.8));
        let mut edges = BTreeMap::new();
        let mut area = 0.0;
        for triangle in indices.as_chunks::<3>().0 {
            let p = [0, 1, 2].map(|i| Vec3::from_array(refined[triangle[i] as usize].pos));
            let cross = (p[1] - p[0]).cross(p[2] - p[0]);
            assert!(cross.y > 0.0, "source winding is preserved");
            area += cross.length() * 0.5;
            for i in 0..3 {
                assert!(p[i].distance(p[(i + 1) % 3]) <= 0.50001);
                *edges.entry(edge_key(p[i].to_array(), p[(i + 1) % 3].to_array())).or_insert(0) += 1;
            }
        }
        assert!((area - 4.0).abs() < 1e-5);
        for ((a, b), count) in edges {
            assert!(count == 1 || count == 2);
            if count == 1 {
                let a = a.map(f32::from_bits);
                let b = b.map(f32::from_bits);
                assert!((a[0] == b[0] && (a[0] == 0.0 || a[0] == 2.0))
                    || (a[2] == b[2] && (a[2] == 0.0 || a[2] == -2.0)), "interior edge must have two triangles");
            }
        }
    }

    #[test]
    fn switches_edges_and_native_height_are_preserved() {
        let position = [9.0, 4.0, -7.0];
        let normal = [0.0, 1.0, 0.0];
        let off = animate(position, normal, 3.0, [1.0, 0.0], 20.0, WaveOptions::default());
        assert_eq!(off.position, position);
        assert_eq!(off.normal, normal);
        let options = WaveOptions { waves: true, ..Default::default() };
        for time in 0..100 {
            let edge = animate(position, normal, time as f32, [1.0, 0.0], 0.0, options);
            assert_eq!(edge.position, position);
            let wave = animate(position, normal, time as f32, [1.0, 0.0], 20.0, options);
            assert!(wave.position[1] >= position[1]);
            assert!((Vec3::from_array(wave.normal).length() - 1.0).abs() < 1e-5);
        }
        let ripple = animate(position, normal, 3.0, [1.0, 0.0], 20.0, WaveOptions { ripples: true, ..Default::default() });
        assert_eq!(ripple.position, position);
        assert_ne!(ripple.normal, normal);
    }

    #[test]
    fn exact_boundary_ignores_diagonals_and_t_junctions() {
        let a = [0.0, 0.0, 0.0];
        let b = [8.0, 0.0, 0.0];
        let c = [8.0, 0.0, -8.0];
        let d = [0.0, 0.0, -8.0];
        let m = [4.0, 0.0, -4.0];
        let shore = Shoreline::from_triangles([[a, b, c], [a, m, d], [m, c, d]]);
        assert_eq!(shore.edges.len(), 4);
        assert_eq!(shore.distance(m), 4.0);
        assert_eq!(shore.distance(a), 0.0);
        assert_eq!(shore.distance([4.0, 0.0, 0.0]), 0.0);
        assert!(shore.distance([4.0, 20.0, -4.0]) > 20.0);
    }

    #[test]
    fn gerstner_filter_and_falling_sheet_reference() {
        assert_eq!(gerstner([2.0, 3.0], 1.0, [1.0, 0.0], 100.0, 1.0, 1.0), ([0.0; 3], [0.0, 1.0, 0.0], 1.0));
        let (offset, normal, fold) = gerstner([0.0; 2], 0.0, [1.0, 0.0], 0.0, 0.0, 1.0);
        assert_eq!(offset, [0.0; 3]);
        assert_eq!(normal, [0.0, 1.0, 0.0]);
        assert_eq!(fold, 1.0);
        assert!(!is_waterfall([0.0, 1.0, 0.0]));
        assert!(is_waterfall([1.0, 0.0, 0.0]));
        assert_eq!(fall_tau(0.0, 1.0), 0.0);
        assert_eq!(fall_speed(0.0, 1.0), 3.0);
        assert!(fall_speed(10.0, 1.0) > fall_speed(1.0, 1.0));
        assert_eq!(fall_uv(0.0, 0.0, 0.0, 1.0), [[0.0, 0.0], [0.3, 0.5]]);
        for step in 0..20 {
            let foam = fall_foam(step as f32, 1.25, 2.0, 1.0);
            assert!((0.0..=1.0).contains(&foam));
        }
    }
}
