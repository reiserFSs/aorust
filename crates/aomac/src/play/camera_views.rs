//! Scripted camera views: the playfield's `PointCameraAttractor_t` list as the client ranks, selects and steps through it
//! (`n3Camera_t` @N3 0x10020faa / 0x10021921 / `FUN_100220bb`, `PointCameraAttractor_t` vtable 0x1003e4f4). Everything is in
//! scene space (z negated); evidence and the unresolved parts: docs/zone/camera.md §6.

use ao_formats::playfield::{CameraAttractor, CameraViews};
use ao_render::Vec3;

/// The list is rebuilt every 10th frame (`DAT_1005c040` reset to 10 in `FUN_10022345` @N3 0x10022345); 60 Hz equivalent.
const LIST_PERIOD: f32 = 10.0 / 60.0;
/// Candidates must score below this (`_DAT_1003e2a0`).
const MAX_SCORE: f32 = 100_000.0;
/// Seconds an automatic selection waits after a selection (`+0x224` = `_DAT_1003e358`); the very first one is immediate
/// (`+0x220` starts at `_DAT_1003e244` = 20 s).
const SELECT_DELAY: f32 = 1.2;
const FIRST_TIMER: f32 = 20.0;
/// Height of the point the attractors are ranked from, above the character's feet (`(0, 1, 0)` added in `FUN_100220bb`).
const EYE: f32 = 1.0;

/// A selected attractor in scene space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub pos: Vec3,
    pub target: Vec3,
    pub range: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Entry {
    zone: usize,
    index: usize,
    score: f32,
}

pub struct Views {
    data: CameraViews,
    /// Zone of a scene position (`n3Playfield_t::GetZoneInstance`).
    zone_of: Box<dyn Fn([f32; 3]) -> usize>,
    list: Vec<Entry>,
    /// `n3Camera_t +0x21c`; `None` = -1 (the list changed).
    index: Option<usize>,
    /// What `CameraVehicle_t +0x1a0` points at.
    selected: Option<View>,
    /// `+0x220` / `+0x224`: time since the last automatic selection and the wait before the next.
    timer: f32,
    delay: f32,
    /// `+0x228`: the next refresh is skipped (the wait is still running).
    hold: bool,
    clock: f32,
}

fn flip(p: [f32; 3]) -> Vec3 {
    Vec3::new(p[0], p[1], -p[2])
}

impl Views {
    pub fn new(data: CameraViews, zone_of: Box<dyn Fn([f32; 3]) -> usize>) -> Self {
        Self { data, zone_of, list: Vec::new(), index: None, selected: None, timer: FIRST_TIMER, delay: SELECT_DELAY, hold: false, clock: 0.0 }
    }

    pub fn selected(&self) -> Option<View> {
        self.selected
    }

    fn view(&self, e: Entry) -> Option<View> {
        let a = self.data.attractors.get(e.zone)?.get(e.index)?;
        Some(View { pos: flip(a.pos), target: flip(a.target), range: a.range })
    }

    /// `SetCameraAttractor(vehicle, 0)` as `GetNextVisibleAttractor` does first.
    pub fn clear(&mut self) {
        self.selected = None;
    }

    /// `PointCameraAttractor_t` vtable +0xc (`FUN_10023c8a` + visibility `FUN_10023bfc`): `None` = not a candidate. Lower is
    /// better: the distance to the point, +10000 straight above/below it (< 0.5 m horizontally), +1000 per metre its authored
    /// target is farther than `range`, +100 when it lies in the direction of the camera's current spot (dot > 0.9), +50 when
    /// that spot is in the clear and the point lies beyond it, +10000 below y = 0.1.
    fn score(a: &CameraAttractor, p: Vec3, guide: Vec3, clear: &dyn Fn([f32; 3], [f32; 3]) -> bool) -> Option<f32> {
        // `+0x30`: disabled; the closed-door room test of `FUN_10023bfc` is not applied (doors count as open), the line of
        // sight is: `Space_i +0xc` reports a collision.
        let pos = flip(a.pos);
        if a.disabled() || !clear(pos.to_array(), p.to_array()) {
            return None;
        }
        let d = (pos - p).length();
        let horizontal = Vec3::new(pos.x - p.x, 0.0, pos.z - p.z).length();
        let mut s = if horizontal < 0.5 { 10_000.0 } else { d };
        let dt = (flip(a.target) - p).length();
        if dt > a.range {
            s += (dt - a.range) * 1000.0;
        }
        let g = guide - pos;
        if g.length() > 0.5 {
            let to_player = p - pos;
            if g.normalize().dot(to_player.normalize_or_zero()) > 0.9 {
                s += 100.0;
            }
            if clear(pos.to_array(), guide.to_array()) && (p - guide).dot(g) * g.dot(to_player) < 0.0 {
                s += 50.0;
            }
        }
        if pos.y < 0.1 {
            s += 10_000.0;
        }
        Some(s)
    }

    /// `FUN_100220bb`: rank the attractors of the character's zone and its neighbours, best first.
    fn refresh(&mut self, player: Vec3, guide: Vec3, clear: &dyn Fn([f32; 3], [f32; 3]) -> bool) {
        let p = player + Vec3::Y * EYE;
        let mut list = Vec::new();
        for zone in self.data.neighbours((self.zone_of)(player.to_array())) {
            for (index, a) in self.data.attractors.get(zone).into_iter().flatten().enumerate() {
                if let Some(score) = Self::score(a, p, guide, clear).filter(|s| *s < MAX_SCORE) {
                    list.push(Entry { zone, index, score });
                }
            }
        }
        list.sort_by(|a, b| a.score.total_cmp(&b.score));
        // The selection index survives only an unchanged set with an unchanged best entry and more than one member.
        let same_set = list.len() == self.list.len() && list.iter().all(|e| self.list.iter().any(|o| (o.zone, o.index) == (e.zone, e.index)));
        let same_first = list.first().map(|e| (e.zone, e.index)) == self.list.first().map(|e| (e.zone, e.index));
        let keep = same_set && (list.is_empty() || (same_first && list.len() != 1));
        self.list = list;
        if !keep {
            self.index = None;
        }
    }

    /// `FUN_10021921`: select the best entry. Returns whether the selection changed.
    fn select_first(&mut self) -> bool {
        let Some(&e) = self.list.first() else { return false };
        let v = self.view(e);
        self.index = Some(0);
        let changed = v != self.selected;
        self.selected = v;
        changed
    }

    /// Per-frame part of `FUN_10022345`: every 10th frame rebuild the list and, while nothing is selected by index, pick the
    /// best entry once the wait is over. `player` = feet (scene), `guide` = where the camera is (`GetCameraAttractorGuidePos`).
    pub fn tick(&mut self, dt: f32, player: Vec3, guide: Vec3, clear: &dyn Fn([f32; 3], [f32; 3]) -> bool) {
        self.timer += dt;
        self.clock += dt;
        if self.clock < LIST_PERIOD {
            return;
        }
        self.clock -= LIST_PERIOD;
        if !self.hold {
            self.refresh(player, guide, clear);
        }
        if self.index.is_none() {
            if self.timer <= self.delay {
                self.hold = true;
            } else {
                if self.select_first() {
                    self.timer = 0.0;
                    self.delay = SELECT_DELAY;
                }
                self.hold = false;
            }
        }
    }

    /// Shift+F8 (`n3Camera_t::GetPreviousVisibleAttractor`): step back through the list, wrapping from the first entry to the
    /// last. An empty list clears the selection. [INFERENCE] "no index" (-1) steps like index 0: the client would index at -2.
    pub fn prev(&mut self, player: Vec3, guide: Vec3, clear: &dyn Fn([f32; 3], [f32; 3]) -> bool) {
        if self.list.is_empty() {
            self.refresh(player, guide, clear);
        }
        let Some(last) = self.list.len().checked_sub(1) else {
            self.selected = None;
            return;
        };
        let i = match self.index {
            None | Some(0) => last,
            Some(i) => i - 1,
        };
        self.index = Some(i);
        self.selected = self.view(self.list[i]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn att(pos: [f32; 3], target: [f32; 3], range: f32) -> CameraAttractor {
        CameraAttractor { pos, rot: [0.0, 0.0, 0.0, 1.0], target, range }
    }

    fn views(list: Vec<CameraAttractor>) -> Views {
        // one zone, server coordinates
        Views::new(CameraViews::new(vec![list], None, vec![]), Box::new(|_| 0))
    }

    const CLEAR: &dyn Fn([f32; 3], [f32; 3]) -> bool = &|_, _| true;
    /// Scene position of a server position.
    fn at(x: f32, y: f32, z: f32) -> Vec3 {
        Vec3::new(x, y, -z)
    }

    #[test]
    fn disabled_attractors_are_never_candidates() {
        let mut v = views(vec![att([10.0, 5.0, 0.0], [10.0, 5.0, 2.0], 2.0), att([20.0, 5.0, 0.0], [20.0, 5.0, 2.0], 1.9)]);
        v.refresh(at(12.0, 0.0, 3.0), at(0.0, 5.0, 0.0), CLEAR);
        assert_eq!(v.list.len(), 1);
        assert_eq!((v.list[0].zone, v.list[0].index), (0, 1));
    }

    #[test]
    fn ranking_prefers_near_and_penalises_far_targets_and_overhead_points() {
        let t = [8.0, 1.0, 0.5]; // an authored target next to the player
        let a = att([10.0, 5.0, 0.0], t, 1.0); // 4.5 m away
        let b = att([4.0, 5.0, 0.0], t, 1.0); // 5.7 m
        let c = att([3.0, 5.0, 0.0], [3.0, 1.0, 50.0], 1.0); // 6.4 m but its target is ~50 m from the player
        let d = att([8.0, 5.0, 0.0], t, 1.0); // straight above the player (horizontal 0): +10000
        let mut v = views(vec![a, b, c, d]);
        // the camera's current spot is the player's point: every attractor lies in its direction (+100 for all, no +50)
        v.refresh(at(8.0, 0.0, 0.0), at(8.0, 1.0, 0.0), CLEAR);
        let order: Vec<usize> = v.list.iter().map(|e| e.index).collect();
        assert_eq!(order, vec![0, 1, 3, 2]);
        let s: Vec<f32> = v.list.iter().map(|e| e.score).collect();
        assert!((s[0] - (20.0f32.sqrt() + 100.0)).abs() < 1e-3 && (s[2] - 10_100.0).abs() < 1e-2, "{s:?}");
        assert!(s[3] > 40_000.0);
    }

    #[test]
    fn a_blocked_line_of_sight_hides_the_attractor() {
        let mut v = views(vec![att([10.0, 5.0, 0.0], [10.0, 5.0, 2.0], 1.0)]);
        v.refresh(at(12.0, 0.0, 3.0), at(0.0, 5.0, 0.0), &|_, _| false);
        assert!(v.list.is_empty());
    }

    #[test]
    fn previous_steps_back_and_wraps() {
        let mut v = views(vec![att([10.0, 5.0, 0.0], [10.0, 5.0, 2.0], 1.0), att([14.0, 5.0, 0.0], [14.0, 5.0, 2.0], 1.0), att([30.0, 5.0, 0.0], [30.0, 5.0, 2.0], 1.0)]);
        let (p, g) = (at(12.0, 0.0, 3.0), at(0.0, 5.0, 0.0));
        v.prev(p, g, CLEAR); // no index: the last of the list
        assert_eq!(v.index, Some(2));
        assert_eq!(v.selected().unwrap().pos, at(30.0, 5.0, 0.0));
        v.prev(p, g, CLEAR);
        assert_eq!(v.index, Some(1));
        v.prev(p, g, CLEAR);
        v.prev(p, g, CLEAR); // 0 wraps to the end
        assert_eq!(v.index, Some(2));
        v.clear();
        assert!(v.selected().is_none());
    }

    #[test]
    fn nothing_to_select_clears() {
        let mut v = views(vec![]);
        v.selected = Some(View { pos: Vec3::ZERO, target: Vec3::ZERO, range: 1.0 });
        v.prev(Vec3::ZERO, Vec3::ZERO, CLEAR);
        assert!(v.selected().is_none());
    }

    #[test]
    fn the_best_entry_is_selected_automatically_after_the_first_refresh() {
        let mut v = views(vec![att([14.0, 5.0, 0.0], [14.0, 5.0, 2.0], 1.0), att([10.0, 5.0, 0.0], [10.0, 5.0, 2.0], 1.0)]);
        let (p, g) = (at(12.0, 0.0, 3.0), at(0.0, 5.0, 0.0));
        v.tick(0.05, p, g, CLEAR); // before the 10th frame: nothing yet
        assert!(v.selected().is_none());
        v.tick(0.12, p, g, CLEAR);
        let s = v.selected().expect("selected");
        assert_eq!(v.index, Some(0));
        assert!(s.pos == at(10.0, 5.0, 0.0) || s.pos == at(14.0, 5.0, 0.0));
        // an unchanged list keeps the index (more than one member)
        let before = v.index;
        v.tick(0.2, p, g, CLEAR);
        assert_eq!(v.index, before);
    }
}
