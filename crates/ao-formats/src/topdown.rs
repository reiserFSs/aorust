//! Top-down ground image of a playfield scene: the picture the playfield map window shows.
//!
//! The client's `PFMapGroundRenderer_c` (GUI.dll vftable 0x101b0a78, ctor `FUN_10046b79`, per-room/ground textures held by
//! `MapData_c` 0x101b0848) renders the ground (`AnarchyGround_t`) or the dungeon rooms with a camera looking straight
//! down. We do the same on the CPU from the loaded [`Scene`]: every up-facing, opaque, non-sky triangle is drawn with its
//! texture, the highest surface per pixel wins; ceilings face down and never show (docs/formats.md *Playfield map image*).
//! UNRESOLVED: the original's lighting/colour modulation of the map texture (not read) — texels are shown unlit.

use ao_scene::{Blend, Scene};

/// Server-space axes: `X` = scene x, `Z` = −scene z (north = +`Z` = up in the image).
#[derive(Clone, Debug)]
pub struct GroundMap {
    pub width: u32,
    pub height: u32,
    /// RGBA, alpha 0 where no surface was drawn.
    pub rgba: Vec<u8>,
    /// Server `X` of the left edge and `Z` of the top edge.
    pub origin: [f32; 2],
    /// Metres per pixel.
    pub mpp: f32,
    /// Per pixel, which `ground` instance (index from `ground.start`) drew it; [`NO_OWNER`] where nothing was drawn. In a
    /// dungeon the instances are the room shells, so this is the room of every pixel.
    pub owner: Vec<u16>,
}

pub const NO_OWNER: u16 = u16::MAX;

impl GroundMap {
    /// Image position (pixels, may lie outside) of server coordinates `(x, z)`.
    pub fn to_px(&self, x: f32, z: f32) -> [f32; 2] {
        [(x - self.origin[0]) / self.mpp, (self.origin[1] - z) / self.mpp]
    }
}

fn mat_apply(m: &[[f32; 4]; 4], p: [f32; 3]) -> [f32; 3] {
    // column-major, M * v
    [0, 1, 2].map(|r| m[0][r] * p[0] + m[1][r] * p[1] + m[2][r] * p[2] + m[3][r])
}

struct Tri {
    p: [[f32; 3]; 3],
    uv: [[f32; 2]; 3],
    sub: (usize, usize),
    owner: u16,
}

/// Renders the `ground` instances of `scene` (`Report::ground`) from above into an image of at
/// most `max_px` pixels along its longer side; `None` when nothing faces up.
pub fn render(scene: &Scene, ground: std::ops::Range<usize>, max_px: u32) -> Option<GroundMap> {
    let mut tris: Vec<Tri> = Vec::new();
    let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
    for (n, inst) in scene.instances.iter().take(ground.end).skip(ground.start).enumerate() {
        let Some(mesh) = scene.meshes.get(inst.mesh) else { continue };
        let world: Vec<[f32; 3]> = mesh.vertices.iter().map(|v| mat_apply(&inst.transform, v.pos)).collect();
        for (si, sm) in mesh.submeshes.iter().enumerate() {
            if sm.sky_fog || !matches!(sm.blend, Blend::Opaque | Blend::AlphaTest) {
                continue;
            }
            for t in sm.indices.chunks_exact(3) {
                let ix = [t[0] as usize, t[1] as usize, t[2] as usize];
                if ix.iter().any(|&i| i >= world.len()) {
                    continue;
                }
                let p = ix.map(|i| world[i]);
                // CCW seen from +Y = the winding of the terrain quads (`terrain.rs`); cross.y > 0 faces up
                let (a, b) = ([p[1][0] - p[0][0], p[1][2] - p[0][2]], [p[2][0] - p[0][0], p[2][2] - p[0][2]]);
                if a[1] * b[0] - a[0] * b[1] <= 0.0 {
                    continue;
                }
                for q in &p {
                    lo = [lo[0].min(q[0]), lo[1].min(-q[2])];
                    hi = [hi[0].max(q[0]), hi[1].max(-q[2])];
                }
                tris.push(Tri { p, uv: ix.map(|i| mesh.vertices[i].uv), sub: (inst.mesh, si), owner: n.min(NO_OWNER as usize - 1) as u16 });
            }
        }
    }
    if tris.is_empty() || hi[0] <= lo[0] || hi[1] <= lo[1] {
        return None;
    }
    let mpp = ((hi[0] - lo[0]).max(hi[1] - lo[1]) / max_px.max(1) as f32).max(0.05);
    let (w, h) = (((hi[0] - lo[0]) / mpp).ceil().max(1.0) as u32, ((hi[1] - lo[1]) / mpp).ceil().max(1.0) as u32);
    let mut map = GroundMap { width: w, height: h, rgba: vec![0; (w * h * 4) as usize], origin: [lo[0], hi[1]], mpp, owner: vec![NO_OWNER; (w * h) as usize] };
    let mut depth = vec![f32::MIN; (w * h) as usize];
    for t in &tris {
        let sm = &scene.meshes[t.sub.0].submeshes[t.sub.1];
        let tex = sm.texture.and_then(|k| scene.textures.get(&k));
        let s = t.p.map(|q| [(q[0] - lo[0]) / mpp, (hi[1] + q[2]) / mpp]);
        let (x0, x1) = (s.iter().map(|q| q[0]).fold(f32::MAX, f32::min).floor().max(0.0) as u32, (s.iter().map(|q| q[0]).fold(f32::MIN, f32::max).ceil() as u32).min(w));
        let (y0, y1) = (s.iter().map(|q| q[1]).fold(f32::MAX, f32::min).floor().max(0.0) as u32, (s.iter().map(|q| q[1]).fold(f32::MIN, f32::max).ceil() as u32).min(h));
        let det = (s[1][1] - s[2][1]) * (s[0][0] - s[2][0]) + (s[2][0] - s[1][0]) * (s[0][1] - s[2][1]);
        if det.abs() < 1e-9 {
            continue;
        }
        for y in y0..y1 {
            for x in x0..x1 {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let l0 = ((s[1][1] - s[2][1]) * (px - s[2][0]) + (s[2][0] - s[1][0]) * (py - s[2][1])) / det;
                let l1 = ((s[2][1] - s[0][1]) * (px - s[2][0]) + (s[0][0] - s[2][0]) * (py - s[2][1])) / det;
                let l2 = 1.0 - l0 - l1;
                if l0 < -1e-4 || l1 < -1e-4 || l2 < -1e-4 {
                    continue;
                }
                let height = l0 * t.p[0][1] + l1 * t.p[1][1] + l2 * t.p[2][1];
                let di = (y * w + x) as usize;
                if height <= depth[di] {
                    continue;
                }
                let mut c = [sm.base_color[0], sm.base_color[1], sm.base_color[2], 1.0];
                if let Some(tx) = tex {
                    let (u, v) = (l0 * t.uv[0][0] + l1 * t.uv[1][0] + l2 * t.uv[2][0], l0 * t.uv[0][1] + l1 * t.uv[1][1] + l2 * t.uv[2][1]);
                    let (tu, tv) = ((u.rem_euclid(1.0) * tx.width as f32) as u32 % tx.width, (v.rem_euclid(1.0) * tx.height as f32) as u32 % tx.height);
                    let o = ((tv * tx.width + tu) * 4) as usize;
                    if tx.rgba[o + 3] < 128 && sm.blend == Blend::AlphaTest {
                        continue;
                    }
                    for k in 0..3 {
                        c[k] *= tx.rgba[o + k] as f32 / 255.0;
                    }
                }
                depth[di] = height;
                map.owner[di] = t.owner;
                for k in 0..3 {
                    map.rgba[di * 4 + k] = (c[k].clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                }
                map.rgba[di * 4 + 3] = 255;
            }
        }
    }
    Some(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ao_scene::{Instance, Mesh, Submesh, Vertex, IDENTITY};

    fn quad(y: f32, up: bool, color: [f32; 4]) -> Mesh {
        let v = |x: f32, z: f32| Vertex { pos: [x, y, z], ..Default::default() };
        // CCW seen from +Y: (0,0) (0,1) (1,0) in (x, z) is cross.y > 0? built so that `up` flips the winding
        let idx = if up { vec![0, 1, 2, 2, 1, 3] } else { vec![0, 2, 1, 1, 2, 3] };
        let mut sm = Submesh::new(idx, None);
        sm.base_color = color;
        Mesh { vertices: vec![v(0.0, 0.0), v(1.0, 0.0), v(0.0, -1.0), v(1.0, -1.0)].into_iter().map(|mut q| {
            q.pos[0] *= 10.0;
            q.pos[2] *= 10.0;
            q
        }).collect(), submeshes: vec![sm] }
    }

    #[test]
    fn floor_shows_ceiling_does_not() {
        // floor at y=0 (red), ceiling above it with the opposite winding (green): only the floor is visible
        let mut s = Scene::default();
        s.meshes = vec![quad(0.0, true, [1.0, 0.0, 0.0, 1.0]), quad(3.0, false, [0.0, 1.0, 0.0, 1.0])];
        s.instances = vec![Instance { mesh: 0, transform: IDENTITY }, Instance { mesh: 1, transform: IDENTITY }];
        let m = render(&s, 0..usize::MAX, 10).unwrap();
        assert_eq!((m.width, m.height), (10, 10));
        assert_eq!(&m.rgba[(5 * 10 + 5) * 4..][..4], &[255, 0, 0, 255]);
        // scene z in [-10, 0] is server Z in [0, 10]: the image top is Z = 10
        assert_eq!(m.origin, [0.0, 10.0]);
        assert_eq!(m.to_px(5.0, 5.0), [5.0, 5.0]);
    }

    #[test]
    fn empty_scene_has_no_map() {
        assert!(render(&Scene::default(), 0..0, 64).is_none());
    }
}
