//! Native binary ground triangles (DisplaySystem 10031c0a/10031f3c/10038c9c).
use std::collections::HashMap;

#[derive(Clone, Debug, Default)]
pub struct GroundCell {
    pub submesh: usize,
    /// p00, p10, p01, p11, with each cell's oriented tile UVs.
    pub vertices: [u32; 4],
    /// Resolved original RDB texture ID; `u16::MAX` when unavailable.
    pub native_tile: u16,
    /// Native material class from RDB 1000024/1; zero is unclassified.
    pub ground_material: u8,
    /// Raw tile bits 14..15 (DisplaySystem 100374c5).
    pub orientation: u8,
    /// Authored near-ground plant geometry occupies this cell; Enhanced grass only.
    /// Native ground selection and authored mesh rendering do not consume this flag.
    pub authored_vegetation: bool,
}

#[derive(Clone, Debug, Default)]
pub struct GroundPatch {
    pub mesh: usize,
    pub origin: [i32; 2],
    pub cells: usize,
    pub cell_size: f32,
    /// Metres = raw height sample * (height_scale / 256).
    pub height_scale: f32,
    pub heights: Vec<u16>,
    pub lattice_start: u32,
    pub coarse_submesh: usize,
    pub fine_cells: Vec<GroundCell>,
    pub shadow_texture: Option<crate::TextureKey>,
    /// World x and negative z to normalized native shadow texture coordinates.
    pub shadow_uv_scale: [f32; 2],
}

/// 1003216f: pow(2.25, variance * quality / 100) + radius², bounded before _ftol.
pub fn thresholds(quality: i32, radius: i32) -> [i32; 256] {
    let quality = quality.clamp(100, 200);
    let radius = radius.clamp(7, 44);
    std::array::from_fn(|i| (2.25f64.powf(i as f64 * quality as f64 / 100.0) + (radius * radius) as f64).min(i32::MAX as f64) as i32)
}

type Point = [i32; 2];
const NONE: usize = usize::MAX;
#[derive(Clone, Debug)]
struct Node {
    triangle: [Point; 3], // apex, left, right
    children: [usize; 2],
    neighbors: [usize; 3], // base, left, right (native +8/+c/+10)
    variance: u8,
    patch: usize,
    split: bool,
    high_quality: bool,
}

#[derive(Debug, Default)]
pub struct GroundDetail {
    patches: Vec<GroundPatch>,
    nodes: Vec<Node>,
    roots: Vec<usize>,
    root_neighbors: Vec<[usize; 3]>,
    last: Option<(Point, i32, i32)>,
    table: Option<(i32, i32, [i32; 256])>,
}

impl GroundDetail {
    /// Existing native selection state; optional vegetation reuses it without extra traversal.
    pub fn selection_key(&self) -> Option<([i32; 2], i32, i32)> { self.last }

    /// Conservative fully-fine radius, allowing for camera quantization and cell corners.
    pub fn fine_radius(&self) -> f32 {
        self.last.zip(self.patches.first()).map_or(0.0, |((_, _, radius), patch)| {
            (radius as f32 / 3.0 - 3.0).max(0.0) * patch.cell_size
        })
    }
    pub fn new(patches: Vec<GroundPatch>) -> Self {
        let mut out = Self { patches, ..Self::default() };
        for p in 0..out.patches.len() {
            let s = out.patches[p].cells;
            assert!(s.is_power_of_two() && s <= 64 && out.patches[p].heights.len() == (s + 1) * (s + 1) && out.patches[p].fine_cells.len() == s * s, "invalid native ground patch metadata");
            let [x, z] = out.patches[p].origin;
            let s = s as i32;
            for triangle in [[[x, z + s], [x, z], [x + s, z + s]], [[x + s, z], [x + s, z + s], [x, z]]] {
                let root = out.build(p, triangle).0;
                out.roots.push(root);
            }
        }
        let mut edges = HashMap::new();
        for &root in &out.roots {
            let [a, l, r] = out.nodes[root].triangle;
            for (slot, (a, b)) in [(l, r), (r, a), (a, l)].into_iter().enumerate() {
                let key = if a < b { (a, b) } else { (b, a) };
                if let Some((other, other_slot)) = edges.remove(&key) {
                    out.nodes[root].neighbors[slot] = other;
                    out.nodes[other].neighbors[other_slot] = root;
                } else {
                    edges.insert(key, (root, slot));
                }
            }
        }
        out.root_neighbors = out.roots.iter().map(|&r| out.nodes[r].neighbors).collect();
        out
    }

    fn build(&mut self, patch: usize, triangle: [Point; 3]) -> (usize, u16) {
        let [a, l, r] = triangle;
        let m = midpoint(l, r);
        let i = self.nodes.len();
        self.nodes.push(Node { triangle, children: [NONE; 2], neighbors: [NONE; 3], variance: 0, patch, split: false, high_quality: false });
        // Unit-cell halves are the native terminal triangles.
        if area(triangle) == 1 {
            return (i, 0);
        }
        let p = &self.patches[patch];
        let h = |v: Point| p.heights[(v[1] - p.origin[1]) as usize * (p.cells + 1) + (v[0] - p.origin[0]) as usize] as i32;
        let mut variance = (h(m) - (h(l) + h(r)) / 2).unsigned_abs() as u16;
        let (left, lv) = self.build(patch, [m, r, a]);
        let (right, rv) = self.build(patch, [m, a, l]);
        variance = variance.max(lv).max(rv);
        self.nodes[i].children = [left, right];
        self.nodes[i].variance = (variance >> 8) as u8;
        (i, variance)
    }

    fn replace_neighbor(&mut self, neighbor: usize, old: usize, new: usize) {
        if neighbor != NONE {
            for link in &mut self.nodes[neighbor].neighbors {
                if *link == old { *link = new; break; }
            }
        }
    }

    /// 10038bf1: split one node and repair its side neighbors.
    fn split_one(&mut self, i: usize) {
        if self.nodes[i].split || self.nodes[i].children[0] == NONE { return; }
        let [l, r] = self.nodes[i].children;
        let [_, nl, nr] = self.nodes[i].neighbors;
        self.nodes[i].split = true;
        self.nodes[l].neighbors = [nl, r, NONE];
        self.nodes[r].neighbors = [nr, NONE, l];
        self.replace_neighbor(nl, i, l);
        self.replace_neighbor(nr, i, r);
    }

    /// 10038c9c: force the base neighbor first, then split the complete diamond.
    fn split(&mut self, i: usize) {
        if self.nodes[i].split || self.nodes[i].children[0] == NONE { return; }
        let base = self.nodes[i].neighbors[0];
        if base != NONE && self.nodes[base].neighbors[0] != i { self.split(base); }
        self.split_one(i);
        let base = self.nodes[i].neighbors[0];
        if base != NONE {
            self.split_one(base);
            let [l, r] = self.nodes[i].children;
            let [bl, br] = self.nodes[base].children;
            if bl != NONE {
                self.nodes[l].neighbors[2] = br;
                self.nodes[br].neighbors[1] = l;
                self.nodes[r].neighbors[1] = bl;
                self.nodes[bl].neighbors[2] = r;
            }
        }
    }

    fn select(&mut self, i: usize, camera: Point, radius: i32) {
        if self.nodes[i].children[0] == NONE { return; }
        if distance_squared(self.nodes[i].triangle, camera) < self.table.as_ref().unwrap().2[self.nodes[i].variance as usize] {
            self.split(i);
            let children = self.nodes[i].children;
            for child in children {
                if self.nodes[child].children[0] != NONE {
                    self.select(child, camera, radius);
                } else {
                    // 10031f3c: sum of vertices minus 3*camera, not a divided centroid.
                    let t = self.nodes[child].triangle;
                    let dx = t[0][0].wrapping_add(t[1][0]).wrapping_add(t[2][0]).wrapping_sub(camera[0].wrapping_mul(3));
                    let dz = t[0][1].wrapping_add(t[1][1]).wrapping_add(t[2][1]).wrapping_sub(camera[1].wrapping_mul(3));
                    self.nodes[child].high_quality = dx.wrapping_mul(dx).wrapping_add(dz.wrapping_mul(dz)) < radius * radius;
                }
            }
        }
    }

    /// Rebuilds real index lists only when native integer camera cell or preferences change.
    /// `indices[patch][submesh]` buffers are reused by the renderer.
    pub fn update(&mut self, eye: [f32; 3], quality: i32, radius: i32, indices: &mut [Vec<Vec<u32>>]) -> bool {
        let Some(p) = self.patches.first() else { return false; };
        let camera = [(eye[0] / p.cell_size) as i32, (-eye[2] / p.cell_size) as i32];
        let quality = quality.clamp(100, 200);
        let radius = radius.clamp(7, 44);
        if self.last == Some((camera, quality, radius)) { return false; }
        if self.table.as_ref().is_none_or(|t| (t.0, t.1) != (quality, radius)) {
            self.table = Some((quality, radius, thresholds(quality, radius)));
        }
        self.last = Some((camera, quality, radius));
        for node in &mut self.nodes { node.split = false; node.high_quality = false; node.neighbors = [NONE; 3]; }
        for (&r, &neighbors) in self.roots.iter().zip(&self.root_neighbors) { self.nodes[r].neighbors = neighbors; }
        for r in 0..self.roots.len() { self.select(self.roots[r], camera, radius); }
        for patch in indices.iter_mut() { for list in patch { list.clear(); } }
        for &r in &self.roots { self.emit(r, indices); }
        true
    }

    fn emit(&self, i: usize, indices: &mut [Vec<Vec<u32>>]) {
        let node = &self.nodes[i];
        if node.split {
            for child in node.children { self.emit(child, indices); }
            return;
        }
        let p = &self.patches[node.patch];
        let t = node.triangle.map(|v| [(v[0] - p.origin[0]) as usize, (v[1] - p.origin[1]) as usize]);
        if node.high_quality {
            let x = t.iter().map(|v| v[0]).min().unwrap();
            let z = t.iter().map(|v| v[1]).min().unwrap();
            let cell = &p.fine_cells[z * p.cells + x];
            indices[node.patch][cell.submesh].extend(t.map(|v| cell.vertices[(v[1] - z) * 2 + v[0] - x]));
        } else {
            indices[node.patch][p.coarse_submesh].extend(t.map(|v| p.lattice_start + (v[1] * (p.cells + 1) + v[0]) as u32));
        }
    }

    pub fn patches(&self) -> &[GroundPatch] { &self.patches }
}

fn midpoint(a: Point, b: Point) -> Point { [(a[0] + b[0]) / 2, (a[1] + b[1]) / 2] }
fn area([a, l, r]: [Point; 3]) -> i32 { ((l[0] - a[0]) * (r[1] - a[1]) - (l[1] - a[1]) * (r[0] - a[0])).abs() }

/// 10031d10 region mask: vertex distance outside a corner; squared line distance
/// outside an edge; zero inside. Native _ftol truncates edge distances.
fn distance_squared([a, l, r]: [Point; 3], p: Point) -> i32 {
    let cross = |a: Point, b: Point| p[1].wrapping_sub(a[1]).wrapping_mul(b[0] - a[0]).wrapping_sub(p[0].wrapping_sub(a[0]).wrapping_mul(b[1] - a[1]));
    let mask = usize::from(cross(a, l) >= 0) | (usize::from(cross(l, r) >= 0) << 1) | (usize::from(cross(r, a) >= 0) << 2);
    let vertex = |v: Point| {
        let dx = v[0].wrapping_sub(p[0]);
        let dz = v[1].wrapping_sub(p[1]);
        dx.wrapping_mul(dx).wrapping_add(dz.wrapping_mul(dz))
    };
    let edge = |a: Point, b: Point| {
        let c = cross(a, b);
        (c.wrapping_mul(c) as f64 / ((b[0] - a[0]).pow(2) + (b[1] - a[1]).pow(2)) as f64) as i32
    };
    match mask { 1 => vertex(r), 2 => vertex(a), 3 => edge(r, a), 4 => vertex(l), 5 => edge(l, r), 6 => edge(a, l), _ => 0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_thresholds_and_real_geometry() {
        assert_eq!(&thresholds(100, 7)[..5], &[50, 51, 54, 60, 74]);
        assert_eq!(&thresholds(200, 44)[..5], &[1937, 1941, 1961, 2065, 2592]);
        assert_eq!(thresholds(200, 44)[255], i32::MAX);
        let cells = 8;
        let mut heights = vec![0; 81];
        heights[4 * 9 + 4] = 4096;
        let patch = GroundPatch { mesh: 0, origin: [0, 0], cells, cell_size: 1.0, heights, lattice_start: 0, coarse_submesh: 0, fine_cells: (0..64).map(|_| GroundCell { submesh: 1, vertices: [81, 82, 83, 84], ..Default::default() }).collect(), ..Default::default() };
        let mut detail = GroundDetail::new(vec![patch]);
        assert_eq!(detail.nodes[detail.roots[0]].variance, 16, "native raw ushort residual propagated before >>8");
        assert_eq!(distance_squared([[0, 8], [0, 0], [8, 8]], [4, 4]), 0);
        assert_eq!(distance_squared([[0, 8], [0, 0], [8, 8]], [-2, 4]), 4);
        let mut indices = vec![vec![Vec::new(), Vec::new()]];
        assert!(detail.update([30.0, 0.0, -30.0], 100, 7, &mut indices));
        let low = indices[0].iter().map(Vec::len).sum::<usize>();
        assert!(detail.update([30.0, 0.0, -30.0], 200, 7, &mut indices));
        let quality_high = indices[0].iter().map(Vec::len).sum::<usize>();
        assert!(quality_high > low, "quality alone changes real geometry: {low} -> {quality_high}");
        assert!(detail.update([30.0, 0.0, -30.0], 200, 44, &mut indices));
        let radius_high = indices[0].iter().map(Vec::len).sum::<usize>();
        assert!(radius_high > quality_high, "radius alone changes real geometry: {quality_high} -> {radius_high}");
        assert!(detail.update([4.0, 0.0, -4.0], 200, 44, &mut indices));
        assert!(!indices[0][1].is_empty(), "native HQ-radius selects actual tile geometry");
        assert!(!detail.update([4.0, 0.0, -4.0], 200, 44, &mut indices));
    }

    #[test]
    fn native_forced_diamonds_leave_no_patch_cracks() {
        let patches = (0..2).map(|x| {
            let mut heights = vec![0; 81];
            if x == 0 { heights[4 * 9 + 7] = 4096; }
            GroundPatch { mesh: x, origin: [x as i32 * 8, 0], cells: 8, cell_size: 1.0, heights, lattice_start: 0, coarse_submesh: 0, fine_cells: (0..64).map(|_| GroundCell { submesh: 1, vertices: [81, 82, 83, 84], ..Default::default() }).collect(), ..Default::default() }
        }).collect();
        let mut detail = GroundDetail::new(patches);
        let mut indices = vec![vec![Vec::new(), Vec::new()]; 2];
        detail.update([-10.0, 0.0, -4.0], 100, 7, &mut indices);
        let mut edges = HashMap::new();
        fn collect(detail: &GroundDetail, i: usize, edges: &mut HashMap<(Point, Point), usize>) {
            let node = &detail.nodes[i];
            if node.split { for child in node.children { collect(detail, child, edges); } }
            else {
                let [a, l, r] = node.triangle;
                for (a, b) in [(a, l), (l, r), (r, a)] {
                    let key = if a < b { (a, b) } else { (b, a) };
                    *edges.entry(key).or_default() += 1;
                }
            }
        }
        for &root in &detail.roots { collect(&detail, root, &mut edges); }
        for ((a, b), count) in edges {
            let boundary = (a[0] == b[0] && (a[0] == 0 || a[0] == 16)) || (a[1] == b[1] && (a[1] == 0 || a[1] == 8));
            assert_eq!(count, if boundary { 1 } else { 2 }, "unpaired native triangle edge {a:?}--{b:?}");
        }
    }
}
