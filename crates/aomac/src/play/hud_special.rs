//! The own character's special-action list: what the Actions window (Ctrl+2, `specialaction_window`, `SpecialActionView_c`
//! GUI 0x100dc082) shows and what hotbar slots of kind 6 (`Identity_t {0xdeb0, template}`) refer to. Evidence: docs/gui.md §10.5.
//!
//! The list is **not server-filled**: the client builds it itself on the own character (Gamecode `FUN_10043005` for the base entries,
//! `FUN_1009e301` / `FUN_1009c07e` when a weapon is equipped, `FUN_10042c9f` the common "add"). Each entry (`SpecialAction_t`, a node of the
//! `std::list` at manager+0x84, 0x20 bytes after the node links) is `{ Identity template, Identity owner, Action_e current, Action_e key,
//! Action_e alt, flags }`; an entry with an `alt` is a *toggle* (Start / End Combat, Sit / Stand, Walk / Run, ...) and shows one of its two
//! actions, `FUN_10042da4(x)` = "x holds now, show the other one".

/// `FUN_100428ad` [GC 0x100428ad], the map `Action_e` -> `{template instance, true}` filled for the own character: the rdb 1000020 record
/// `{0xF4254, instance}` (`GetItemByTemplate` maps the Identity kind 0xdeb0 to the item template type, GC 0x17aae) that names and draws
/// the action. Instance 0 = filled at run time from the profession table `FUN_1003e278` (`DAT_102e2fc8`, all zero in the DLL image, no static
/// writer found: UNRESOLVED, so the alt-state actions 0x8a / 0x8b never get an entry here).
pub const ACTION_TEMPLATES: [(u32, u32); 28] = [
    (0x01, 0xc1aa), // Pick-Up
    (0x03, 0xc1a3), // Use
    (0x0b, 0xc1a5), // Start Combat
    (0x4e, 0xc1ab), // End Combat
    (0x11, 0xc1a2), // Walk
    (0x12, 0xc1a9), // Run
    (0x13, 0xc1a7), // Sneak
    (0x4f, 0x1412c), // Stop Sneaking
    (0x4c, 0xc1a8), // Sit
    (0x4d, 0xc1a6), // Stand
    (0x51, 0x14124), // Suspended Animation (camp)
    (0x52, 0x1412b), // Disrupt Suspended Animation
    (0x86, 0x1d9d7), // Search
    (0x97, 0x14127), // Aimed Shot
    (0x94, 0x14123), // Burst
    (0xa7, 0x14121), // Full Auto
    (0x96, 0x14120), // Fling Shot
    (0x8e, 0x14122), // Brawl
    (0x90, 0x14125), // Dimach
    (0x93, 0x14126), // Fast Attack
    (0x79, 0x1e159), // Bow special attack
    (0x92, 0x1f83a), // Sneak Attack
    (0x1e9, 0x32ba3), // Backstab
    (0x14, 0x39c7d), // Crawl
    (0x8d, 0x39c94), // Stop crawling (named "Crawl" in the record)
    (0x6e, 0x3bd0b), // Reload
    (0x8a, 0),
    (0x8b, 0),
];

/// The rdb 1000020 template instance of an action (`None`: the table holds 0 = not filled).
pub fn template_of(action: u32) -> Option<u32> {
    ACTION_TEMPLATES.iter().find(|t| t.0 == action).map(|t| t.1).filter(|&i| i != 0)
}

/// `ItemIconView_c` timer text (`FUN_1003eebe`, formats at GUI 0x101b03a4 / 0x101b03ac): `"%2ds"` up to 60 s, else `"%2dm"` of `secs / 60`.
pub fn timer_text(remaining_secs: i32) -> String {
    if remaining_secs < 0x3d { format!("{remaining_secs:2}s") } else { format!("{:2}m", remaining_secs / 60) }
}

/// Weapon `Can` flags (stat `0x1e` of an equipped item, `FUN_1009e301`) -> the special attack they grant, in the order the DLL tests them.
pub const WEAPON_FLAGS: [(i32, u32); 6] = [
    (0x800, 0x94),   // Burst
    (0x1000, 0x96),  // Fling Shot
    (0x2000, 0xa7),  // Full Auto
    (0x4000, 0x97),  // Aimed Shot
    (0x20000, 0x92), // Sneak Attack (+ Backstab 0x1e9 with the char flag below)
    (0x8000, 0x79),  // Bow special attack
];
/// `0x40000`: Fast Attack.
const FLAG_FAST_ATTACK: i32 = 0x40000;
/// Dynel stat-flag word `+0x138` bit 20, tested by `FUN_1009c07e` / `FUN_1009e301` for Backstab. UNRESOLVED GUESS: that word is stat `Flags`
/// (id 0, docs/zone/actions.md: "Flags stat switches"); the writer of bit 20 was not found.
const CHAR_FLAG_BACKSTAB: i32 = 1 << 20;
/// Equipment slots (`FUN_1009e301`: `slot < 0x30`).
pub const EQUIP_SLOTS: u32 = 0x30;

/// One `SpecialAction_t`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    /// `+0x14`: the action the entry was created with.
    pub key: u32,
    /// `+0x18`: the toggle partner (0 = none; Search 0x86 lists itself).
    pub alt: u32,
    /// `+0x10` / the identity: the action it shows now.
    pub shown: u32,
    /// Template supplied by `GC 10042e16` for dynamically registered actions.
    pub registered_template: Option<u32>,
    /// Dynamic owner identity instance, supplied independently of action/template by CharacterAction 0xb4.
    pub registered_owner: Option<i32>,
    /// Native +0x1c; bit 0 marks an action disabled independently of its progress timer.
    pub flags: u32,
}

impl Entry {
    pub fn template(&self) -> Option<u32> {
        self.registered_template.or_else(|| template_of(self.shown))
    }

    /// GC1004256c: dynamic owner kind0xc78e sends action0xb3, with the own character as identity_a.
    pub fn registered_action(&self, own: i32) -> Option<ao_net::n3::action::Outgoing> {
        if self.flags & 1 != 0 { return None; }
        self.registered_owner.map(|owner| ao_net::n3::action::Outgoing::Action(ao_net::n3::action::simple(
            0xb3, ao_net::msg::Identity { kind: 0xc350, instance: own },
            ao_net::msg::Identity { kind: self.shown as i32, instance: owner },
        )))
    }
}

/// What the own character is doing: the inputs of the toggles (`FUN_1006d196` movement-mode switch, `FUN_10068b7f` / `FUN_100593d3` fight
/// start / stop, `N3Msg_StartCamping` / `StopCamping`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OwnState {
    /// The own fight controller attacks.
    pub fighting: bool,
    /// Movement FSM mode (`movement::mode`).
    pub mode: u8,
    /// `Fsm::last_speed_mode == WALK` (`+0x30 == 2` in `FUN_1006d196`).
    pub last_speed_walk: bool,
    pub camping: bool,
    /// Serial and disabled value emitted by the actual Fly Apply handlers, not inferred from the mode.
    pub control_action_flags: (u32, bool),
}

/// `FUN_1006389c` [GC 0x1006389c]: `trunc(d * pct / 100.0 + d + 0.5)` with `pct = max(stat 0x17e, -50)`, except for the special attacks (Brawl 0x8e,
/// Dimach 0x90, Sneak Attack 0x92, Fast Attack 0x93, Burst 0x94, Fling Shot 0x96, Aimed Shot 0x97, Full Auto 0xa7, 0x243, 0x2a1) where `pct = 0`.
pub fn recharge_time(action: u32, duration: i32, pct: i32) -> i32 {
    let pct = if matches!(action, 0x8e | 0x90 | 0x92..=0x94 | 0x96 | 0x97 | 0xa7 | 0x243 | 0x2a1) { 0 } else { pct.max(-0x32) };
    (f64::from(duration.wrapping_mul(pct)) / 100.0 + f64::from(duration) + 0.5) as i32
}

/// One record of the recharge list (`FUN_10063c50`: `{+4 action, +8 total, +0xc remaining}` in seconds, `FUN_10064301` prints h:m:s).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Recharge {
    pub action: u32,
    pub total: i32,
    pub remaining: i32,
}

#[derive(Default)]
pub struct SpecialList {
    entries: Vec<Entry>,
    recharge: Vec<Recharge>,
    perk_recharge: Vec<Recharge>,
    control_action_flags_serial: u32,
    /// `(old template, new template)` of every identity change (`FUN_10042c40` emits it on GlobalSignals): hotbar slots follow it.
    changes: Vec<(u32, u32)>,
}

impl SpecialList {
    /// `FUN_10043005` on the own character: Use, Pick-Up, Attack / End Combat, Sit / Stand, Walk / Run, Sneak / Stop, Camp / Disrupt,
    /// Crawl / Stop, Search; then the initial toggle states (`FUN_10042da4` for 0x4e 0x4d 0x12 0x4f 0x8d 0x51 0x52) = Start Combat, Sit,
    /// Walk, Sneak, Crawl and Suspended Animation are shown. The alt-state pair 0x8a / 0x8b exists only with a profession table entry (not filled).
    pub fn new() -> Self {
        let mut l = SpecialList::default();
        for (a, alt) in [(3, 0), (1, 0), (0xb, 0x4e), (0x4c, 0x4d), (0x11, 0x12), (0x13, 0x4f), (0x51, 0x52), (0x14, 0x8d), (0x86, 0x86)] {
            l.add(a, alt);
        }
        for a in [0x4e, 0x4d, 0x12, 0x4f, 0x8d, 0x51, 0x52] {
            l.show_other(a);
        }
        l.changes.clear();
        l
    }

    /// `FUN_10042c9f`: an entry for `action` (skipped when the map has no template or the action is listed already).
    fn add(&mut self, action: u32, alt: u32) {
        if template_of(action).is_some() && !self.entries.iter().any(|e| e.key == action) {
            let flags = u32::from(action > 0x8d && self.recharge.iter().any(|lock| lock.action == action));
            self.entries.push(Entry { key: action, alt, shown: action, registered_template: None, registered_owner: None, flags });
        }
    }

    /// `GC 10042e16`: the server supplies the template; catalogue perk items are not action templates.
    pub fn register(&mut self, action: u32, owner: i32, template: u32) {
        if template == 0 { return; }
        if let Some(entry) = self.entries.iter_mut().find(|e| e.key == action) {
            entry.registered_template = Some(template);
            entry.registered_owner = Some(owner);
        } else {
            let flags = u32::from(action > 0x8d && self.recharge.iter().any(|lock| lock.action == action));
            self.entries.push(Entry { key: action, alt: 0, shown: action, registered_template: Some(template), registered_owner: Some(owner), flags });
        }
    }

    /// `FUN_10042da4(x)`: the entry that has `x` as key or alt (the last one found) shows its other action; the change is queued.
    pub fn show_other(&mut self, x: u32) {
        let Some(e) = self.entries.iter_mut().rev().find(|e| e.key == x || e.alt == x) else { return };
        let other = if e.key == x { e.alt } else { e.key };
        let (old, new) = (template_of(e.shown), template_of(other));
        e.shown = other;
        if let (Some(o), Some(n)) = (old, new) {
            if o != n {
                self.changes.push((o, n));
            }
        }
    }

    /// The toggles for the own state, in the order of the DLL's events: movement mode (`FUN_1006d196`: sneak end `FUN_1003b5f3` -> 0x4f, sneak
    /// start `FUN_1003b59b` -> 0x13, sit 0x4c / stand 0x4d, walk 0x11 / run 0x12 -- while sitting, sneaking or frozen the last speed mode
    /// decides), fight (0xb start / 0x4e stop), camp (0x51 / 0x52). Crawl enter / leave are GC1006e646 / 1006e3ff.
    pub fn apply_state(&mut self, s: OwnState) {
        use super::movement::mode;
        if self.control_action_flags_serial != s.control_action_flags.0 {
            for action in [0x4d, 0x4c, 0x11, 0x12, 0x13, 0x4f, 0x14, 0x8d] {
                self.set_disabled(action, s.control_action_flags.1);
            }
            self.control_action_flags_serial = s.control_action_flags.0;
        }
        let speed = |l: &mut Self| l.show_other(if s.last_speed_walk { 0x11 } else { 0x12 });
        match s.mode {
            mode::WALK | mode::RUN => {
                self.show_other(0x4f);
                self.show_other(if s.mode == mode::WALK { 0x11 } else { 0x12 });
                self.show_other(0x4d);
            }
            mode::SIT_GROUND => {
                self.show_other(0x4f);
                self.show_other(0x4c);
                speed(self);
            }
            mode::SNEAK => {
                self.show_other(0x13);
                self.show_other(0x4d);
                speed(self);
            }
            mode::FROZEN => speed(self),
            _ => {}
        }
        self.show_other(if s.mode == mode::CRAWL { 0x14 } else { 0x8d });
        self.show_other(if s.fighting { 0xb } else { 0x4e });
        self.show_other(if s.camping { 0x51 } else { 0x52 });
    }

    /// `FUN_1003f121(identity)`: the entry currently showing template `instance`.
    pub fn find(&self, instance: u32) -> Option<&Entry> {
        self.entries.iter().find(|e| e.template() == Some(instance))
    }

    /// The entries in list order (the Actions window shows them in this order).
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The identity changes since the last call.
    pub fn take_changes(&mut self) -> Vec<(u32, u32)> {
        std::mem::take(&mut self.changes)
    }

    /// Weapon-derived entries (`FUN_1009e301`, run for every item put into an equipment slot `< 0x30`): `items` = `(Can flags (stat 0x1e), stat 0x1a)`
    /// of each equipped item in slot order; `char_flags` = stat `Flags`. A missing item removes its entries again (the DLL's removal path
    /// was not traced: UNRESOLVED, so this recomputes the set from the present equipment instead).
    pub fn sync_equipment(&mut self, items: &[(i32, i32)], char_flags: i32) {
        let derived = |a: u32| matches!(a, 0x94 | 0x96 | 0xa7 | 0x97 | 0x92 | 0x1e9 | 0x79 | 0x93 | 0x6e);
        self.entries.retain(|e| !derived(e.key));
        for &(flags, reload) in items {
            if flags == 0x499602d2u32 as i32 {
                continue; // INVALID marker: `uVar6 != 0x499602d2`
            }
            for (bit, action) in WEAPON_FLAGS {
                if flags & bit != 0 {
                    self.add(action, 0);
                    if action == 0x92 && char_flags & CHAR_FLAG_BACKSTAB != 0 {
                        self.add(0x1e9, 0);
                    }
                }
            }
            if flags & FLAG_FAST_ATTACK != 0 {
                self.add(0x93, 0);
            }
            if reload != -1 {
                self.add(0x6e, 0);
            }
        }
    }

    /// Project the authoritative native lock vectors; the Zone owns their shared second-pulse clock.
    /// Skill locks are keyed by Stat_e; perk locks have already been resolved through catalogue word 4.
    pub fn sync_recharge(&mut self, skill: impl Iterator<Item = Recharge>, perks: impl Iterator<Item = Recharge>) {
        self.recharge.clear();
        self.recharge.extend(skill);
        self.perk_recharge.clear();
        self.perk_recharge.extend(perks);
    }

    /// `N3Msg_GetActionProgress` [GC 0x100275f7]: `(remaining / total, remaining seconds)` while recharging.
    pub fn progress(&self, action: u32) -> Option<(f32, i32)> {
        let locks = if action >= 1000 { &self.perk_recharge } else if action >= 0x8e { &self.recharge } else { return None };
        locks.iter().find(|r| r.action == action).map(|r| (r.remaining as f32 / r.total as f32, r.remaining))
    }

    /// GC1003f0c3 / 1003f065 update every entry currently showing this action.
    pub fn set_disabled(&mut self, action: u32, disabled: bool) {
        for entry in self.entries.iter_mut().filter(|entry| entry.shown == action) {
            if disabled { entry.flags |= 1; } else { entry.flags &= !1; }
        }
    }

    /// GC1003f156: stat-lock membership takes precedence for 142..999; otherwise perk-lock
    /// membership or the entry's flag word disables the action. Expiry is not tested here.
    pub fn disabled(&self, action: u32) -> bool {
        if (0x8e..1000).contains(&action) {
            self.recharge.iter().any(|lock| lock.action == action)
        } else {
            self.perk_recharge.iter().any(|lock| lock.action == action)
                || self.entries.iter().any(|entry| entry.shown == action && entry.flags != 0)
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn registered_perk_template_is_authoritative() {
        let mut l = SpecialList::new();
        l.register(10111, 111, 213010);
        l.register(10111, 111, 213010);
        let e = l.find(213010).unwrap();
        assert_eq!((e.shown, e.template()), (10111, Some(213010)));
        assert_eq!(l.entries().iter().filter(|e| e.key == 10111).count(), 1);
        assert!(l.find(211656).is_none());
    }

    use super::*;

    fn shown(l: &SpecialList) -> Vec<u32> {
        l.entries.iter().map(|e| e.shown).collect()
    }

    /// Initial list (`FUN_10043005` + the seven `FUN_10042da4` calls): every toggle shows its first action.
    #[test]
    fn initial_list_shows_the_first_action_of_each_toggle() {
        let l = SpecialList::new();
        assert_eq!(shown(&l), [3, 1, 0xb, 0x4c, 0x11, 0x13, 0x51, 0x14, 0x86]);
        assert_eq!(l.entries[2], Entry { key: 0xb, alt: 0x4e, shown: 0xb, registered_template: None, registered_owner: None, flags: 0 });
    }

    /// Sit shows Stand, release shows Sit; a hotbar slot holding Sit follows the queued change and the change is queued once.
    #[test]
    fn toggles_follow_the_state_and_are_found_by_template() {
        let mut l = SpecialList::new();
        l.show_other(0x4c); // sitting
        assert_eq!(l.take_changes(), [(0xc1a8, 0xc1a6)]);
        assert_eq!(l.find(0xc1a6).map(|e| e.key), Some(0x4c));
        assert!(l.find(0xc1a8).is_none(), "the identity of Sit is no longer listed (PerformSpecialAction would be ActionIsNotAvailable)");
        l.show_other(0x4d);
        l.show_other(0x4d);
        assert_eq!(l.take_changes(), [(0xc1a6, 0xc1a8)], "a repeated state is not a change");
        // camping: StartCamping -> FUN_10042da4(0x51) shows 0x52
        l.show_other(0x51);
        assert!(l.find(0x1412b).is_some());
    }

    /// The map `Action_e` -> template is a bijection on the filled entries (the hotbar derives `Action_e` from it).
    #[test]
    fn action_template_map_round_trips() {
        for (a, t) in ACTION_TEMPLATES {
            if t != 0 {
                assert_eq!(template_of(a), Some(t));
            }
        }
        assert_eq!(template_of(0x8a), None);
    }

    /// `FUN_1009e301`: a Burst + Fling Shot weapon with a clip adds three entries; Backstab needs the char flag; removing the weapon removes them.
    #[test]
    fn equipment_adds_the_weapon_specials() {
        let mut l = SpecialList::new();
        l.sync_equipment(&[(0x800 | 0x1000, 5)], 0);
        let keys: Vec<u32> = l.entries.iter().map(|e| e.key).collect();
        assert_eq!(&keys[9..], [0x94, 0x96, 0x6e]);
        l.sync_equipment(&[(0x20000, -1)], 0);
        assert_eq!(l.entries.iter().map(|e| e.key).skip(9).collect::<Vec<_>>(), [0x92]);
        l.sync_equipment(&[(0x20000, -1)], CHAR_FLAG_BACKSTAB);
        assert!(l.entries.iter().any(|e| e.key == 0x1e9));
        l.sync_equipment(&[], 0);
        assert_eq!(l.entries.len(), 9);
    }

    /// `FUN_100655d3`: a second record of the same action extends the first; `FUN_1006389c`: percent stat except for special attacks, floor -50.
    #[test]
    fn recharge_feed_extends_and_scales() {
        assert_eq!(recharge_time(0x94, 20, 50), 20, "special attacks ignore stat 0x17e");
        assert_eq!(recharge_time(0x11, 20, 50), 30);
        assert_eq!(recharge_time(0x11, 20, -90), 10, "floored at -50 %");
        assert_eq!(recharge_time(0x11, 7, -10), 6, "7 - 0.7 + 0.5 = 6.8, truncated");
        assert_eq!(recharge_time(0x11, 100_000_000, 50), 107_050_327, "native IMUL wraps before conversion to double");
        let mut l = SpecialList::new();
        l.sync_recharge([Recharge { action: 0x88, total: 60, remaining: 50 }].into_iter(), std::iter::empty());
        assert_eq!(l.progress(0x88), None, "Search's stat lock is not an icon action-progress timer");
    }

    /// Recharge: unavailable while a record exists, progress is `remaining / total`, the icon timer text is seconds below a minute.
    #[test]
    fn recharge_greys_the_slot_and_counts_down() {
        let mut l = SpecialList::new();
        l.sync_equipment(&[(0x800, -1)], 0);
        assert_eq!(l.progress(0x94), None);
        l.sync_recharge([Recharge { action: 0x94, total: 20, remaining: 15 }].into_iter(), std::iter::empty());
        assert_eq!(l.progress(0x94), Some((0.75, 15)));
        l.sync_recharge([Recharge { action: 0x94, total: 20, remaining: 5 }].into_iter(), std::iter::empty());
        assert_eq!(l.progress(0x94), Some((0.25, 5)));
        l.sync_recharge(std::iter::empty(), std::iter::empty());
        assert_eq!(l.progress(0x94), None);
        assert_eq!(timer_text(5), " 5s");
        assert_eq!(timer_text(60), "60s");
        assert_eq!(timer_text(61), " 1m");
        assert_eq!(timer_text(150), " 2m");
    }

    #[test]
    fn native_lock_ranges_and_entry_flags_are_independent_of_timer_text() {
        let mut list = SpecialList::new();
        list.register(10111, 111, 213010);
        list.sync_recharge(
            [Recharge { action: 141, total: 10, remaining: 5 }, Recharge { action: 142, total: 10, remaining: 5 },
                Recharge { action: 999, total: 10, remaining: 5 }, Recharge { action: 10111, total: 99, remaining: 98 }].into_iter(),
            [Recharge { action: 999, total: 50, remaining: 40 }, Recharge { action: 10111, total: 30, remaining: 29 }].into_iter(),
        );
        assert_eq!(list.progress(141), None);
        assert_eq!(list.progress(142), Some((0.5, 5)));
        assert_eq!(list.progress(999), Some((0.5, 5)));
        assert_eq!(list.progress(10111), Some((29.0 / 30.0, 29)), "perk identity resolves through catalogue action, not the stat-lock vector");
        list.set_disabled(10111, true);
        list.set_disabled(142, true);
        list.sync_recharge(std::iter::empty(), std::iter::empty());
        assert!(list.disabled(10111), "native perk lock sets the entry flag independently");
        assert!(!list.disabled(142), "stat-range actions ignore entry flags when their lock expires");
        assert_eq!(list.progress(10111), None, "flag-only disability never fabricates a zero-second timer");
        list.set_disabled(10111, false);
        assert!(!list.disabled(10111));
    }

    #[test]
    fn flight_action_flags_follow_apply_events_not_mode_inference() {
        use super::super::movement::{id, Movement};
        let mut movement = Movement::new([0.0; 3], 0.0, 100);
        let state = |movement: &Movement| OwnState {
            mode: movement.fsm().mode, control_action_flags: movement.control_action_flags(), ..OwnState::default()
        };
        let mut list = SpecialList::new();
        assert!(movement.transition(id::SWITCH_FLY));
        list.apply_state(state(&movement));
        assert!(list.disabled(0x4c));
        assert!(list.disabled(0x11));
        assert!(!list.disabled(0xb));
        list.set_disabled(0x4c, false);
        list.apply_state(state(&movement));
        assert!(!list.disabled(0x4c), "an unchanged Fly mode cannot overwrite a later native flag clear");
        assert!(movement.transition(id::SWITCH_FLY));
        list.apply_state(state(&movement));
        assert!(list.disabled(0x4c), "a repeated Apply event must still write flags");
        assert!(movement.transition(id::SWITCH_FROZEN));
        list.apply_state(state(&movement));
        assert!(list.disabled(0x4c), "Frozen entered from Fly does not call the LeaveFly flag handler");
        assert!(movement.transition(id::LEAVE_FROZEN));
        assert!(movement.transition(id::SWITCH_FLY));
        assert!(movement.transition(id::LEAVE_FLY));
        list.apply_state(state(&movement));
        assert!(!list.disabled(0x4c));
        assert!(!list.disabled(0x11));
        assert!(!list.disabled(0x13));
        assert!(!list.disabled(0x14));
    }
}
