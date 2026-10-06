//! The dynel pick of the world click / hover (docs/gui.md §13.2): the camera's selection line against the bounding boxes of the
//! characters' current pose, ordered by distance.
//!
//! Original, evidence:
//! * `n3Playfield_t::RunCameraSelectionCollChecks` (N3 0x1000d8d1, every 0.1 s, skipped while the camera's mouse ray is zero) runs an
//!   `n3SelectionCollider_t` (a `LocalityWorker_i`, vftable 0x1003d2c8) over the camera's locality query (radius 200 m, `_DAT_1003ce54`).
//!   Its worker `FUN_1000fc2b` takes every `Vehicle_t` whose body is an `n3VisualDynel_t` and asks `FUN_10020a3c` whether the camera's
//!   `n3CameraCollLine_t` hits it: the client character only when the camera is third-person (`camera+0x1ec`), the dynel visible
//!   (`n3VisualDynel_t::IsVisible`), then `vtable+0x94(line, BoundingBoxTargeting, &distance)` = `n3VisualDynel_t::FineCollisionCheck`
//!   (N3 0x19452; slot 0x94 of every `SimpleChar_t`, `Corpse_t`, `Door_t`, … vftable of Gamecode).
//! * Character bodies are `VisualCATMesh_t`: `VisualCATMesh_t::IsLineIntersecting` (DisplaySystem 0x100736fe) → `RCATMesh_t::
//!   IsLineIntersecting` (randy31 0x10056786) with the `BoundingBoxTargeting` flag (LoginPrefs.xml default `true`). The line is moved
//!   into the model frame by the inverse of the mesh's world matrix (position, heading **and body scale**); with the flag set the hit is
//!   the ray against the axis aligned box `RCATMesh_t+0x1fc` (min) / `+0x208` (max) by `FUN_10050358` (the Graphics Gems "fast ray–box"
//!   candidate-plane test; an origin inside the box hits at distance 0), the distance `|hit − origin|` is **in model units** and a hit
//!   beyond the line length (`GetLengthOfViewcone`) is rejected. The box is `FUN_1005470d` over the **skinned vertices of the current
//!   pose** of the mesh's own parts (body; mounted heads and weapons are other meshes), refreshed when the pose version changes
//!   (`FUN_10055c1c` / `FUN_10055a23`). Without the flag the original tests every triangle instead (not ported: the pref is true).
//! * The hits are collected as `(vehicle, distance)` pairs and `std::sort`ed ascending by that distance (`FUN_1000fc8d` →
//!   `FUN_1000f8e5`, compare on the float at +4), then copied to the camera's hit list `+0x244`.
//! * Not ported, UNRESOLVED: an early-out sphere before the box (`RCATMesh_t` +0x1cc / +0x1d0 and the mesh data's `+0x5c→+0x14`
//!   radius; the writers of those fields were not traced, it only rejects rays that miss a sphere around the feet) and the 0.1 s
//!   refresh (the list here is built at the click / hover). Items, doors and corpses are `VisualMesh_t` / other `Vehicle_t` bodies
//!   that `Zone::dynels` does not hold (they cannot become the target here), so they are not picked.

use ao_render::Vec3;

/// A pickable character: the model-space box of its current pose and the actor transform (`ActorFrame::transform`, scene space).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PickBody {
    pub id: i32,
    /// `RCATMesh_t+0x1fc` / `+0x208`: min and max of the posed vertices, before the body scale.
    pub bounds: ([f32; 3], [f32; 3]),
    /// Scene-space position of the model origin (the feet).
    pub pos: [f32; 3],
    /// Scene-space heading about Y (`scene_yaw`).
    pub yaw: f32,
    /// Body scale (`MonsterScale / 100`), applied by the mesh's frame.
    pub scale: f32,
}

/// `FUN_1005470d`: the min / max corner of `positions`; `None` for an empty mesh.
pub fn bounds_of<'a>(positions: impl IntoIterator<Item = &'a [f32; 3]>) -> Option<([f32; 3], [f32; 3])> {
    let mut it = positions.into_iter();
    let first = *it.next()?;
    Some(it.fold((first, first), |(mut lo, mut hi), p| {
        for i in 0..3 {
            lo[i] = lo[i].min(p[i]);
            hi[i] = hi[i].max(p[i]);
        }
        (lo, hi)
    }))
}

/// `FUN_10050358` + the distance of `RCATMesh_t::IsLineIntersecting`: the entry distance of the ray (`dir` need not be unit, the
/// result is in units of the space the arguments are in) into the box, `0` for an origin inside, `None` for a miss, a box behind
/// the origin or an entry beyond `max_len`.
pub fn ray_box(origin: [f32; 3], dir: [f32; 3], min: [f32; 3], max: [f32; 3], max_len: f32) -> Option<f32> {
    // per axis: the candidate plane (`Some`) when the origin is outside the slab, nothing when inside
    let plane: [Option<f32>; 3] = std::array::from_fn(|i| if origin[i] < min[i] { Some(min[i]) } else if origin[i] > max[i] { Some(max[i]) } else { None });
    if plane.iter().all(Option::is_none) {
        return Some(0.0);
    }
    // t of each candidate plane (the original stores -1 where the direction is 0 or the slab contains the origin)
    let t: [f32; 3] = std::array::from_fn(|i| match plane[i] {
        Some(p) if dir[i] != 0.0 => (p - origin[i]) / dir[i],
        _ => -1.0,
    });
    let far = (1..3).fold(0, |m, i| if t[m] < t[i] { i } else { m });
    if t[far] < 0.0 {
        return None;
    }
    let mut hit = [0.0; 3];
    for i in 0..3 {
        hit[i] = if i == far {
            plane[i]?
        } else {
            let c = origin[i] + dir[i] * t[far];
            if c < min[i] || c > max[i] {
                return None;
            }
            c
        };
    }
    let d = (0..3).map(|i| (hit[i] - origin[i]).powi(2)).sum::<f32>().sqrt();
    (d <= max_len).then_some(d)
}

impl PickBody {
    /// The ray's entry distance into this body's box in **model units** (what the original sorts by); `max_len` is the line
    /// length in scene units (`GetLengthOfViewcone`).
    pub fn hit(&self, origin: Vec3, dir: Vec3, max_len: f32) -> Option<f32> {
        let k = self.scale.max(f32::EPSILON);
        let (s, c) = self.yaw.sin_cos();
        // inverse of `ActorFrame::transform`: columns (c, 0, -s)·k, (0, 1, 0)·k, (s, 0, c)·k and the position
        let to_model = |v: Vec3| [(v.x * c - v.z * s) / k, v.y / k, (v.x * s + v.z * c) / k];
        let o = to_model(origin - Vec3::from(self.pos));
        let d = to_model(dir);
        let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        if len == 0.0 {
            return None;
        }
        // the original normalises the model-space direction and compares with the model-space line length
        ray_box(o, d.map(|x| x / len), self.bounds.0, self.bounds.1, max_len / k)
    }
}

/// Every body hit by the line `origin + dir · [0, max_len]`, nearest (model-space distance) first; ties by id (the original's
/// `std::sort` is unstable there).
pub fn hits(bodies: &[PickBody], origin: Vec3, dir: Vec3, max_len: f32) -> Vec<i32> {
    let mut v: Vec<(f32, i32)> = bodies.iter().filter_map(|b| b.hit(origin, dir, max_len).map(|d| (d, b.id))).collect();
    v.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    v.into_iter().map(|h| h.1).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const LO: [f32; 3] = [-0.4, 0.0, -0.3];
    const HI: [f32; 3] = [0.4, 1.8, 0.3];

    #[test]
    fn ray_enters_the_box_at_the_near_face() {
        let d = ray_box([0.0, 1.0, 5.0], [0.0, 0.0, -1.0], LO, HI, 100.0).unwrap();
        assert!((d - 4.7).abs() < 1e-5, "{d}");
        // an origin inside hits at 0, a box behind the origin or beside the ray does not
        assert_eq!(ray_box([0.0, 1.0, 0.0], [0.0, 0.0, 1.0], LO, HI, 100.0), Some(0.0));
        assert_eq!(ray_box([0.0, 1.0, 5.0], [0.0, 0.0, 1.0], LO, HI, 100.0), None);
        assert_eq!(ray_box([0.5, 1.0, 5.0], [0.0, 0.0, -1.0], LO, HI, 100.0), None);
        assert_eq!(ray_box([0.0, 2.5, 5.0], [0.0, 0.0, -1.0], LO, HI, 100.0), None);
        // beyond the line length
        assert_eq!(ray_box([0.0, 1.0, 5.0], [0.0, 0.0, -1.0], LO, HI, 4.0), None);
        // a diagonal ray through the top face
        let d = ray_box([0.0, 4.0, 0.0], [0.0, -1.0, 0.0], LO, HI, 100.0).unwrap();
        assert!((d - 2.2).abs() < 1e-5, "{d}");
    }

    #[test]
    fn bounds_are_the_vertex_extremes() {
        let p = [[1.0, 2.0, 3.0], [-1.0, 5.0, 0.5], [0.0, 0.0, 9.0]];
        assert_eq!(bounds_of(&p), Some(([-1.0, 0.0, 0.5], [1.0, 5.0, 9.0])));
        assert_eq!(bounds_of(&[]), None);
    }

    fn body(id: i32, pos: [f32; 3], yaw: f32, scale: f32) -> PickBody {
        PickBody { id, bounds: (LO, HI), pos, yaw, scale }
    }

    #[test]
    fn body_scale_and_heading_move_the_box() {
        let o = Vec3::new(0.0, 1.0, 10.0);
        let dir = -Vec3::Z;
        // unscaled: the front face is at z = 0.3
        assert!((body(1, [0.0; 3], 0.0, 1.0).hit(o, dir, 800.0).unwrap() - 9.7).abs() < 1e-4);
        // scale 2: the model-space distance is half the scene distance (box front at z = 0.6)
        assert!((body(1, [0.0; 3], 0.0, 2.0).hit(o, dir, 800.0).unwrap() - 4.7).abs() < 1e-4);
        // a quarter turn swaps the box's x / z extents: the front face is now at z = 0.4
        assert!((body(1, [0.0; 3], std::f32::consts::FRAC_PI_2, 1.0).hit(o, dir, 800.0).unwrap() - 9.6).abs() < 1e-4);
        // x = 0.35 is inside the unrotated box's x extent (±0.4) but outside the rotated one (±0.3)
        let side = Vec3::new(0.35, 1.0, 10.0);
        assert!(body(1, [0.0; 3], 0.0, 1.0).hit(side, dir, 800.0).is_some());
        assert!(body(1, [0.0; 3], std::f32::consts::FRAC_PI_2, 1.0).hit(side, dir, 800.0).is_none());
    }

    #[test]
    fn hits_are_ordered_by_model_distance() {
        let o = Vec3::new(0.0, 1.0, 20.0);
        let bodies = [body(3, [0.0, 0.0, -5.0], 0.0, 1.0), body(1, [0.0, 0.0, 5.0], 0.0, 1.0), body(2, [3.0, 0.0, 0.0], 0.0, 1.0)];
        assert_eq!(hits(&bodies, o, -Vec3::Z, 800.0), vec![1, 3]);
        assert_eq!(hits(&bodies, o, -Vec3::Z, 16.0), vec![1]);
    }

    /// Real data: the box of a real creature rig (the Surf Lizard, model 22773) over its bind pose and over a run-less pose: the live
    /// box is the same `bounds_of` over `ActorRig::pose`. It stands on the ground, is non-degenerate and a ray through its middle hits
    /// the front face; a 20 % larger body scale grows the box with it.
    #[test]
    fn real_creature_mesh_box() {
        use ao_formats::character::actor::ActorRig;
        let Some(dir) = std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join("Games/ProjectRubiKa/client")).filter(|d| d.join("cd_image/rdb.db").exists()) else { return };
        let store = ao_rdb::RecordStore::open(&dir).unwrap();
        let rig = ActorRig::new(&store, 22773, None, &Default::default(), &Default::default(), &[]).unwrap();
        let (verts, _) = rig.pose(None);
        let (lo, hi) = bounds_of(verts.iter().map(|v| &v.pos)).unwrap();
        eprintln!("surf lizard bind box {lo:?} {hi:?}");
        let ext = [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]];
        assert!(ext.iter().all(|e| (0.1..8.0).contains(e)), "{ext:?}");
        assert!(lo[1].abs() < 0.3, "stands on the ground: {}", lo[1]);
        let (cx, cy) = ((lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0);
        let b = PickBody { id: 1, bounds: (lo, hi), pos: [0.0; 3], yaw: 0.0, scale: 1.0 };
        let d = b.hit(Vec3::new(cx, cy, 20.0), -Vec3::Z, 800.0).unwrap();
        assert!((d - (20.0 - hi[2])).abs() < 1e-3, "{d}");
        assert!(b.hit(Vec3::new(hi[0] + 0.5, cy, 20.0), -Vec3::Z, 800.0).is_none(), "beside the box");
        let big = PickBody { scale: 1.2, ..b };
        assert!(big.hit(Vec3::new(cx * 1.2, cy * 1.2, 20.0), -Vec3::Z, 800.0).unwrap() < d, "a larger body reaches further");
    }
}
