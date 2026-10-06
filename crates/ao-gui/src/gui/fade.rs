//! `FadeGroupController_c` (GUI.dll ctor 0x1012d4e9, singleton 0x1012d6bf): the `fade_group` views of the control centre
//! (`Views/ControlCenter.xml`: the wing / bar docks and the two menus) dim to `CCFadeLow` while the pointer is elsewhere and brighten
//! to `CCFadeHigh` while it is over any view of their group. Evidence and the unresolved items: `docs/gui.md` §10.8.
//!
//! * Group = a map entry keyed by the `fade_group` string (registration: `View::SetFadeGroup` 0x1014ac6d -> `FUN_1012d48d`; the entry is
//!   created by `FUN_1012df8e` with the group constructor `FUN_1012d2fb`, which starts the current / from / to alpha at `CCFadeLow`).
//! * Per frame (`FUN_1012cfbf` -> `FUN_1012cf26`): a running fade does nothing until its start time has passed, then interpolates linearly
//!   `from + (to - from) * elapsed / duration`, ends exactly on `to`, and hands the value to every member view (`vtable+0x2c` =
//!   `View::SetAlpha`; the engine's `View::alpha`, multiplied down the tree).
//! * Pointer (`FUN_1012cffb`): the group of the hovered view is the first non-empty `fade_group` walking up the parents. When it differs
//!   from the hot group, the old hot group fades to `CCFadeLow` after `CCFadeDelay` seconds over 1 s (`FUN_1012cea2(low, delay, 1e6 us)`,
//!   asm 0x1012d12d) and the new one fades to `CCFadeHigh` at once over 0.2 s (200000 us poked into the entry).
//! * DValue observer (`FUN_1012d1f4`, observes `CCFadeLow` / `CCFadeHigh`): the changed value is stored, every group snaps to it
//!   (`FUN_1012cea2(value, 0, 0, 0, 0)`), then the hot group fades to `CCFadeHigh` over 0.2 s and all others to `CCFadeLow` over 1 s starting
//!   2 s later (a literal 2000000 us, not `CCFadeDelay`).

use super::*;
use std::collections::BTreeMap;

/// Defaults of `LoginPrefs.xml` until the application feeds the dvalues ([`Gui::set_fade_params`]).
const DEFAULT_LOW: f32 = 0.33;
const DEFAULT_HIGH: f32 = 0.85;
const DEFAULT_DELAY: f32 = 2.0;
/// Fade-in of the hot group (`piVar[0x12] = 200000` us).
const RISE: f64 = 0.2;
/// Fade-out of a group that lost the pointer (1000000 us).
const FALL: f64 = 1.0;
/// Start delay of the observer's fade-out (2000000 us).
const OBSERVER_DELAY: f64 = 2.0;

struct Group {
    cur: f32,
    from: f32,
    to: f32,
    /// Seconds; 0 = no fade running.
    dur: f64,
    start: f64,
}

impl Group {
    fn new(low: f32) -> Group {
        Group { cur: low, from: low, to: low, dur: 0.0, start: 0.0 }
    }
    /// `FUN_1012cea2` with a non-zero delay / duration.
    fn run(&mut self, to: f32, start: f64, dur: f64) {
        (self.from, self.to, self.start, self.dur) = (self.cur, to, start, dur);
    }
    /// `FUN_1012cea2` with all zeros: set at once.
    fn snap(&mut self, v: f32) {
        (self.cur, self.from, self.to, self.dur, self.start) = (v, v, v, 0.0, 0.0);
    }
}

pub(super) struct FadeCtl {
    low: f32,
    high: f32,
    delay: f32,
    hot: String,
    now: f64,
    groups: BTreeMap<String, Group>,
}

impl Default for FadeCtl {
    fn default() -> Self {
        FadeCtl { low: DEFAULT_LOW, high: DEFAULT_HIGH, delay: DEFAULT_DELAY, hot: String::new(), now: 0.0, groups: BTreeMap::new() }
    }
}

impl FadeCtl {
    /// The group entry of a registered member (created at `CCFadeLow`); returns its current alpha.
    pub(super) fn member(&mut self, group: &str) -> f32 {
        let low = self.low;
        self.groups.entry(group.to_string()).or_insert_with(|| Group::new(low)).cur
    }

    pub(super) fn alpha(&self, group: &str) -> Option<f32> {
        self.groups.get(group).map(|g| g.cur)
    }

    /// `CCFadeDelay` is read when the pointer leaves a group; a changed `CCFadeLow` / `CCFadeHigh` runs the observer.
    pub(super) fn set_params(&mut self, low: f32, high: f32, delay: f32) {
        self.delay = delay;
        if low != self.low {
            self.low = low;
            self.observe(low);
        }
        if high != self.high {
            self.high = high;
            self.observe(high);
        }
    }

    fn observe(&mut self, v: f32) {
        let (low, high, now) = (self.low, self.high, self.now);
        for (name, g) in &mut self.groups {
            g.snap(v);
            if *name == self.hot {
                g.run(high, now, RISE);
            } else {
                g.run(low, now + OBSERVER_DELAY, FALL);
            }
        }
    }

    /// The pointer moved onto a view of `group` ("" = none).
    pub(super) fn hover(&mut self, group: &str) {
        if group == self.hot {
            return;
        }
        let (low, high, now, delay) = (self.low, self.high, self.now, f64::from(self.delay));
        if let Some(g) = self.groups.get_mut(&self.hot) {
            g.run(low, now + delay, FALL);
        }
        self.hot = group.to_string();
        if let Some(g) = self.groups.get_mut(group) {
            g.run(high, now, RISE);
        }
    }

    /// `FUN_1012cf26` for every group.
    pub(super) fn tick(&mut self, dt: f32) {
        self.now += f64::from(dt);
        for g in self.groups.values_mut().filter(|g| g.dur != 0.0) {
            let t = self.now - g.start;
            if t > 0.0 {
                if t >= g.dur {
                    g.cur = g.to;
                    g.from = g.to;
                    g.dur = 0.0;
                } else {
                    g.cur = g.from + (g.to - g.from) * (t / g.dur) as f32;
                }
            }
        }
    }
}

impl Gui {
    /// `CCFadeLow`, `CCFadeHigh`, `CCFadeDelay` (`LoginPrefs.xml` dvalues): applied live, like the controller's observer.
    pub fn set_fade_params(&mut self, low: f32, high: f32, delay: f32) {
        self.fade.set_params(low, high, delay);
    }

    /// Current alpha of a fade group (None until a view registered it).
    pub fn fade_alpha(&self, group: &str) -> Option<f32> {
        self.fade.alpha(group)
    }

    /// `View::alpha` of the first view of that name (the fade groups write it).
    pub fn view_alpha(&self, w: WindowId, name: &str) -> Option<f32> {
        Some(self.tree.views[self.find(w, name)?].alpha)
    }

    /// `ViewSurface_c::SetAlpha` of the black surface behind the named view (see [`View::backdrop`]).
    pub fn set_backdrop(&mut self, w: WindowId, name: &str, alpha: f32) {
        if let Some(v) = self.find(w, name) {
            self.tree.views[v].backdrop = alpha.clamp(0.0, 1.0);
        }
    }

    /// Controller frame step: advances the fades and gives every member view its group's alpha.
    pub(super) fn tick_fade_groups(&mut self, dt: f32) {
        self.fade.tick(dt);
        for v in &mut self.tree.views {
            if !v.fade_group.is_empty() {
                v.alpha = self.fade.member(&v.fade_group);
            }
        }
    }

    /// `FUN_1012cffb`: the fade group of the view under the pointer. Every visible view counts (not only the interactive ones) except bare
    /// layout containers (`Kind::View` without a `fade_group`: the control centre's screen-sized splitter views would otherwise hide the
    /// docks); the topmost window with such a view under the pointer wins. UNRESOLVED: which view the original's mouse signal reports
    /// (its hit test was not decompiled).
    pub(super) fn fade_hover(&mut self) {
        let (x, y) = (self.mouse.x, self.mouse.y);
        let mut group = String::new();
        for (_, root, pos) in self.windows_top_down() {
            if let Some(v) = self.deepest_at(root, x - pos.0 as f32, y - pos.1 as f32, true, 0.0, 0.0) {
                let mut cur = Some(v);
                while let Some(c) = cur {
                    if !self.tree.views[c].fade_group.is_empty() {
                        group = self.tree.views[c].fade_group.clone();
                        break;
                    }
                    cur = self.tree.views[c].parent;
                }
                break;
            }
        }
        self.fade.hover(&group);
    }

    fn deepest_at(&self, id: ViewId, x: f32, y: f32, is_root: bool, ox: f32, oy: f32) -> Option<ViewId> {
        let v = &self.tree.views[id];
        if !v.visible {
            return None;
        }
        let (l, t) = if is_root { (0.0, 0.0) } else { (ox + v.frame.l, oy + v.frame.t) };
        let (r, b) = (l + v.frame.width() + 1.0, t + v.frame.height() + 1.0);
        if x < l || x >= r || y < t || y >= b {
            return None;
        }
        let opaque = !matches!(v.kind, Kind::View) || !v.fade_group.is_empty();
        v.children.iter().rev().find_map(|c| self.deepest_at(*c, x, y, false, l, t)).or(opaque.then_some(id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(c: &mut FadeCtl, secs: f32) {
        for _ in 0..(secs * 1000.0).round() as u32 {
            c.tick(0.001);
        }
    }

    #[test]
    fn hover_brightens_then_dims_after_delay() {
        let mut c = FadeCtl::default();
        assert_eq!(c.member("g"), 0.33); // registered groups start at CCFadeLow
        c.hover("g");
        step(&mut c, 0.1);
        assert!((c.alpha("g").unwrap() - 0.59).abs() < 0.01, "half way after 0.1 s of 0.2 s");
        step(&mut c, 0.11);
        assert_eq!(c.alpha("g"), Some(0.85));
        c.hover("");
        step(&mut c, 1.9);
        assert_eq!(c.alpha("g"), Some(0.85)); // CCFadeDelay = 2 s not over
        step(&mut c, 0.6);
        assert!((c.alpha("g").unwrap() - 0.59).abs() < 0.01, "half way through the 1 s fall");
        step(&mut c, 0.6);
        assert_eq!(c.alpha("g"), Some(0.33));
    }

    #[test]
    fn moving_between_groups_swaps_them() {
        let mut c = FadeCtl::default();
        c.member("a");
        c.member("b");
        c.hover("a");
        step(&mut c, 0.3);
        c.hover("b");
        step(&mut c, 0.3);
        assert_eq!((c.alpha("a"), c.alpha("b")), (Some(0.85), Some(0.85)), "a waits CCFadeDelay, b rose");
        step(&mut c, 2.8);
        assert_eq!((c.alpha("a"), c.alpha("b")), (Some(0.33), Some(0.85)));
    }

    #[test]
    fn params_apply_live() {
        let mut c = FadeCtl::default();
        c.member("a");
        c.member("b");
        c.hover("a");
        step(&mut c, 0.3);
        c.set_params(0.1, 0.85, 2.0); // low changed: everything snaps to it, the hot group rises again
        assert_eq!((c.alpha("a"), c.alpha("b")), (Some(0.1), Some(0.1)));
        step(&mut c, 0.25);
        assert_eq!((c.alpha("a"), c.alpha("b")), (Some(0.85), Some(0.1)));
        c.set_params(0.1, 0.6, 2.0); // high changed: snap to 0.6, b falls after the observer's fixed 2 s
        assert_eq!((c.alpha("a"), c.alpha("b")), (Some(0.6), Some(0.6)));
        step(&mut c, 1.9);
        assert_eq!(c.alpha("b"), Some(0.6));
        step(&mut c, 1.2);
        assert_eq!((c.alpha("a"), c.alpha("b")), (Some(0.6), Some(0.1)));
        // the delay is read when the pointer leaves
        c.set_params(0.1, 0.6, 5.0);
        c.hover("b");
        step(&mut c, 4.9);
        assert_eq!(c.alpha("a"), Some(0.6));
        step(&mut c, 1.2);
        assert_eq!(c.alpha("a"), Some(0.1));
    }
}
