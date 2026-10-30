//! Region-limited admission invalidation (DESIGN section 8: history is dropped "for any pixel region whose admission set changed").
//!
//! Every frame the drawn actor items `(actor id, mesh index)` are compared with the previous frame's. An item present in exactly
//! one of the two is *changed*: its world bounding sphere (taken from the frame it existed in) is projected with BOTH the previous
//! and the current unjittered view-projection, unioned, padded (x1.25 plus [`PAD_PX`], covering its contact shadow and AO halo) and
//! the overlapping rects merged. TAA and the AO denoise treat pixels inside any rect as history-less; everything else keeps its
//! history. More than [`MAX_RECTS`] separate clusters, most of the screen covered, or a massive set change (zone change, mass
//! respawn) fall back to the full reset. Camera cuts, resize, the first frame and a TAA toggle stay full resets elsewhere.
use glam::{Mat4, Vec3, Vec4};
use std::collections::HashMap;

pub(super) const MAX_RECTS: usize = 16;
/// Added on every side after the growth: the actor's ground shadow and AO halo reach past its own bounds.
const PAD_PX: f32 = 24.0;
/// The rect grows by this factor about its centre. The sphere already carries `POSE_MARGIN` (1 m) around mesh 0 and the bounding
/// CUBE of it is up to 1.7x the sphere, so 1.25 (not more) is enough for the shadow footprint: a bigger factor makes a close
/// actor cover most of the screen, which is exactly the full reset this module exists to avoid.
const GROW: f32 = 1.25;
/// A set change is "massive" (full reset) when more than this many ACTORS changed AND they are more than half of all actors.
const MASSIVE_MIN: usize = 8;
/// Rects covering more than this share of the screen are not worth keeping: reset everything.
const MAX_COVER: f32 = 0.75;

/// GPU mirror (`struct Rects` in taa.wgsl / ao.wgsl): `count.x` rects of pixel bounds `(x0, y0, x1, y1)` on the full-resolution grid.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct RectUniform {
    pub count: [u32; 4],
    pub rects: [[f32; 4]; MAX_RECTS],
}

impl RectUniform {
    pub(super) fn new(rects: &[[f32; 4]]) -> Self {
        let mut out = Self::default();
        out.count[0] = rects.len().min(MAX_RECTS) as u32;
        out.rects[..rects.len().min(MAX_RECTS)].copy_from_slice(&rects[..rects.len().min(MAX_RECTS)]);
        out
    }
}

#[derive(Debug, PartialEq)]
pub(super) enum Change {
    /// Only these pixel rects (possibly none) lost their history.
    Rects(Vec<[f32; 4]>),
    /// Drop everything.
    Full,
}

type Key = (u32, usize);

#[derive(Default)]
pub(super) struct Tracker {
    prev: HashMap<Key, (Vec3, f32)>,
}

impl Tracker {
    /// `items`: `((actor, mesh), (world sphere centre, radius))` of this frame's drawn items. `prev_vp` / `vp`: previous / current
    /// UNJITTERED view-projection. Always records `items` as the new previous set.
    pub(super) fn update(&mut self, items: impl Iterator<Item = (Key, (Vec3, f32))>, prev_vp: Mat4, vp: Mat4, size: (u32, u32)) -> Change {
        let cur: HashMap<Key, (Vec3, f32)> = items.collect();
        let prev = std::mem::replace(&mut self.prev, cur);
        let cur = &self.prev;
        let changed: Vec<(Key, (Vec3, f32))> = prev.iter().filter(|(k, _)| !cur.contains_key(k)).map(|(k, s)| (*k, *s))
            .chain(cur.iter().filter(|(k, _)| !prev.contains_key(k)).map(|(k, s)| (*k, *s))).collect();
        if changed.is_empty() { return Change::Rects(Vec::new()); }
        // Massive = many ACTORS changed (an actor draws ~10 mesh items, which must not count as ten changes).
        let actors = |set: &HashMap<Key, (Vec3, f32)>| set.keys().map(|k| k.0).collect::<std::collections::HashSet<u32>>().len();
        let changed_actors = changed.iter().map(|(k, _)| k.0).collect::<std::collections::HashSet<u32>>().len();
        if changed_actors > MASSIVE_MIN && changed_actors * 2 > actors(&prev).max(actors(cur)) { return Change::Full; }
        let rects = changed.iter().map(|&(_, (c, r))| pad(union(sphere_rect(prev_vp, c, r, size), sphere_rect(vp, c, r, size)), size)).collect();
        merge(rects, size)
    }

    /// Forget the previous set (resize / first frame: the history is gone anyway).
    pub(super) fn clear(&mut self) { self.prev.clear(); }
}

/// Pixel bounds `(x0, y0, x1, y1)` (y down) of the bounding cube of a sphere under `vp`; the whole screen when any corner is not in
/// front of the camera (the projection would be unbounded).
pub(super) fn sphere_rect(vp: Mat4, c: Vec3, r: f32, size: (u32, u32)) -> [f32; 4] {
    let (w, h) = (size.0 as f32, size.1 as f32);
    let (mut lo, mut hi) = (glam::Vec2::splat(f32::MAX), glam::Vec2::splat(f32::MIN));
    for corner in 0..8 {
        let o = Vec3::new(if corner & 1 == 0 { -r } else { r }, if corner & 2 == 0 { -r } else { r }, if corner & 4 == 0 { -r } else { r });
        let clip: Vec4 = vp * (c + o).extend(1.0);
        if clip.w <= 1e-3 { return [0.0, 0.0, w, h]; }
        let p = glam::Vec2::new((clip.x / clip.w * 0.5 + 0.5) * w, (0.5 - clip.y / clip.w * 0.5) * h);
        lo = lo.min(p);
        hi = hi.max(p);
    }
    [lo.x, lo.y, hi.x, hi.y]
}

fn union(a: [f32; 4], b: [f32; 4]) -> [f32; 4] { [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])] }

/// x1.5 about the centre, +[`PAD_PX`] per side, snapped outwards to whole pixels and clamped to the screen.
fn pad(r: [f32; 4], size: (u32, u32)) -> [f32; 4] {
    let (cx, cy) = ((r[0] + r[2]) * 0.5, (r[1] + r[3]) * 0.5);
    let (hx, hy) = ((r[2] - r[0]) * 0.5 * GROW + PAD_PX, (r[3] - r[1]) * 0.5 * GROW + PAD_PX);
    [(cx - hx).floor().max(0.0), (cy - hy).floor().max(0.0), (cx + hx).ceil().min(size.0 as f32), (cy + hy).ceil().min(size.1 as f32)]
}

fn overlaps(a: [f32; 4], b: [f32; 4]) -> bool { a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3] }

/// Merges overlapping rects to a fixpoint; [`Change::Full`] when more than [`MAX_RECTS`] clusters remain or they cover too much.
fn merge(mut rects: Vec<[f32; 4]>, size: (u32, u32)) -> Change {
    rects.retain(|r| r[2] > r[0] && r[3] > r[1]);
    'again: loop {
        for i in 0..rects.len() {
            for j in i + 1..rects.len() {
                if overlaps(rects[i], rects[j]) {
                    rects[i] = union(rects[i], rects[j]);
                    rects.swap_remove(j);
                    continue 'again;
                }
            }
        }
        break;
    }
    let area: f32 = rects.iter().map(|r| (r[2] - r[0]) * (r[3] - r[1])).sum();
    if rects.len() > MAX_RECTS || area > MAX_COVER * size.0 as f32 * size.1 as f32 { Change::Full } else { Change::Rects(rects) }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: (u32, u32) = (1000, 500);

    fn vp() -> Mat4 {
        Mat4::perspective_rh(1.0, 2.0, 0.1, 100.0) * Mat4::look_to_rh(Vec3::ZERO, Vec3::NEG_Z, Vec3::Y)
    }

    #[test]
    fn sphere_projects_around_its_screen_position() {
        let r = sphere_rect(vp(), Vec3::new(0.0, 0.0, -10.0), 1.0, SIZE);
        let (cx, cy) = ((r[0] + r[2]) * 0.5, (r[1] + r[3]) * 0.5);
        assert!((cx - 500.0).abs() < 1e-2 && (cy - 250.0).abs() < 1e-2, "{r:?}");
        // tan(fov/2) = 0.5463; the cube's near face (9 m away) spans 2 x 250 / (9 x 0.5463) = 101.7 px.
        assert!((r[3] - r[1] - 101.7).abs() < 0.2, "{r:?}");
        assert_eq!(sphere_rect(vp(), Vec3::new(0.0, 0.0, 0.5), 1.0, SIZE), [0.0, 0.0, 1000.0, 500.0], "straddling the camera plane = everything");
        let left = sphere_rect(vp(), Vec3::new(-4.0, 0.0, -10.0), 1.0, SIZE);
        assert!(left[2] < 500.0, "left of the view axis stays left");
    }

    #[test]
    fn padding_grows_by_a_quarter_plus_margin_and_clamps() {
        assert_eq!(pad([100.0, 100.0, 180.0, 140.0], SIZE), [140.0 - 50.0 - 24.0, 120.0 - 25.0 - 24.0, 140.0 + 50.0 + 24.0, 120.0 + 25.0 + 24.0]);
        assert_eq!(pad([0.0, 0.0, 10.0, 10.0], SIZE)[..2], [0.0, 0.0]);
        assert_eq!(pad([990.0, 490.0, 1000.0, 500.0], SIZE)[2..], [1000.0, 500.0]);
    }

    #[test]
    fn overlapping_rects_merge_and_too_many_clusters_fall_back() {
        let Change::Rects(r) = merge(vec![[0.0, 0.0, 10.0, 10.0], [5.0, 5.0, 20.0, 20.0], [100.0, 100.0, 110.0, 110.0]], SIZE) else { panic!() };
        assert_eq!(r.len(), 2);
        assert!(r.contains(&[0.0, 0.0, 20.0, 20.0]));
        let many = (0..MAX_RECTS).map(|i| [i as f32 * 50.0, 0.0, i as f32 * 50.0 + 10.0, 10.0]).collect::<Vec<_>>();
        assert!(matches!(merge(many.clone(), SIZE), Change::Rects(r) if r.len() == MAX_RECTS));
        let mut over = many;
        over.push([900.0, 400.0, 910.0, 410.0]);
        assert_eq!(merge(over, SIZE), Change::Full, "more than MAX_RECTS clusters");
        assert_eq!(merge(vec![[0.0, 0.0, 1000.0, 400.0]], SIZE), Change::Full, "covers most of the screen");
    }

    fn item(actor: u32, x: f32) -> (Key, (Vec3, f32)) { ((actor, 0), (Vec3::new(x, 0.0, -10.0), 1.0)) }

    #[test]
    fn only_items_in_exactly_one_frame_are_changed() {
        let mut t = Tracker::default();
        let base: Vec<_> = (0..20).map(|i| item(i, (i as f32 - 10.0) * 0.5)).collect();
        t.update(base.iter().copied(), vp(), vp(), SIZE);
        assert_eq!(t.update(base.iter().copied(), vp(), vp(), SIZE), Change::Rects(vec![]), "identical set keeps every pixel");
        let mut moved = base.clone();
        moved[3].1 .0.x += 0.3;
        assert_eq!(t.update(moved.iter().copied(), vp(), vp(), SIZE), Change::Rects(vec![]), "a moving actor is motion vectors' job");
        let mut added = moved.clone();
        added.push(item(99, 3.0));
        let Change::Rects(r) = t.update(added.iter().copied(), vp(), vp(), SIZE) else { panic!() };
        assert_eq!(r.len(), 1);
        let own = pad(sphere_rect(vp(), Vec3::new(3.0, 0.0, -10.0), 1.0, SIZE), SIZE);
        assert_eq!(r[0], own, "entering actor: its own padded rect");
        let Change::Rects(r) = t.update(moved.iter().copied(), vp(), vp(), SIZE) else { panic!() };
        assert_eq!(r, vec![own], "leaving actor: the rect of the frame it was drawn in");
    }

    #[test]
    fn rect_covers_both_camera_poses() {
        let mut t = Tracker::default();
        t.update(std::iter::empty(), vp(), vp(), SIZE);
        let shifted = Mat4::perspective_rh(1.0, 2.0, 0.1, 100.0) * Mat4::look_to_rh(Vec3::new(1.0, 0.0, 0.0), Vec3::NEG_Z, Vec3::Y);
        let Change::Rects(r) = t.update([item(1, 0.0)].into_iter(), vp(), shifted, SIZE) else { panic!() };
        let (a, b) = (sphere_rect(vp(), Vec3::new(0.0, 0.0, -10.0), 1.0, SIZE), sphere_rect(shifted, Vec3::new(0.0, 0.0, -10.0), 1.0, SIZE));
        assert!(r[0][0] <= a[0].min(b[0]) && r[0][2] >= a[2].max(b[2]), "{:?} {a:?} {b:?}", r[0]);
    }

    #[test]
    fn massive_set_change_is_a_full_reset() {
        // One actor with 12 mesh items entering is ONE changed actor, not twelve.
        let mut t = Tracker::default();
        t.update((0..30).map(|i| item(i, i as f32 * 0.1)), vp(), vp(), SIZE);
        let crowd = (0..30).map(|i| item(i, i as f32 * 0.1)).chain((0..12).map(|m| ((900, m), (Vec3::new(0.0, 0.0, -10.0), 1.0))));
        assert!(matches!(t.update(crowd, vp(), vp(), SIZE), Change::Rects(r) if r.len() == 1), "a 12-mesh actor popping in is one region");
        let mut t = Tracker::default();
        t.update((0..30).map(|i| item(i, 0.0)), vp(), vp(), SIZE);
        assert_eq!(t.update((100..130).map(|i| item(i, 0.0)), vp(), vp(), SIZE), Change::Full, "zone change: everything swapped");
        let mut t = Tracker::default();
        t.update((0..30).map(|i| item(i, i as f32 * 0.1)), vp(), vp(), SIZE);
        let mut next: Vec<_> = (0..30).map(|i| item(i, i as f32 * 0.1)).collect();
        for (k, slot) in next.iter_mut().take(5).enumerate() { *slot = item(500 + k as u32, k as f32); }
        assert!(matches!(t.update(next.into_iter(), vp(), vp(), SIZE), Change::Rects(_)), "5 of 30 churning (an ordinary crowd) stays regional");
    }

    #[test]
    fn rect_uniform_matches_the_shader_layout() {
        assert_eq!(std::mem::size_of::<RectUniform>(), 16 + 16 * MAX_RECTS);
        let u = RectUniform::new(&[[1.0, 2.0, 3.0, 4.0]]);
        assert_eq!((u.count[0], u.rects[0]), (1, [1.0, 2.0, 3.0, 4.0]));
    }
}
