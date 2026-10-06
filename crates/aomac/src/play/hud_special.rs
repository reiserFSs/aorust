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

/// The action whose template is `instance` (inverse of [`template_of`]; the `N3Msg_GetCorrectActionID` loop over the map `FUN_1003f1c0`).
pub fn action_of_template(instance: u32) -> Option<u32> {
    ACTION_TEMPLATES.iter().find(|t| t.1 == instance && instance != 0).map(|t| t.0)
}

/// Stat 588 of a template record = its menu category (`special_action_category` of `ActionMenu/CommandMenu.xml`; the constants
/// `AM_CATEGORY_PERKS` 1, `AM_CATEGORY_NORMAL` 2, `AM_CATEGORY_ATTACK` 3 are registered by `ControlCenterModule_c::InitialiseMessage` GUI 0x1006a968).
pub const STAT_CATEGORY: u32 = 588;

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
}

/// One record of the recharge list (`FUN_10063c50`: `{+4 action, +8 total, +0xc remaining}` in seconds, `FUN_10064301` prints h:m:s).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Recharge {
    pub action: u32,
    pub total: f32,
    pub remaining: f32,
}

#[derive(Default)]
pub struct SpecialList {
    entries: Vec<Entry>,
    recharge: Vec<Recharge>,
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
            self.entries.push(Entry { key: action, alt, shown: action });
        }
    }

    /// The list in creation order (what the Actions window and `GetSpecialActionList` give).
    pub fn entries(&self) -> &[Entry] {
        &self.entries
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
    /// decides), fight (0xb start / 0x4e stop), camp (0x51 / 0x52). Crawl (mode 5: 0x14, leaving 0x8d) is an [INFERENCE] from the sneak pair.
    pub fn apply_state(&mut self, s: OwnState) {
        use super::movement::mode;
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

    /// `N3Msg_GetCorrectActionID` [GC 0x1002733c] / `FUN_1003f1c0`: the template a slot holding `instance` has to show now (the entry whose
    /// key or alt is that template's action; unchanged when there is none).
    pub fn correct_template(&self, instance: u32) -> u32 {
        let Some(a) = action_of_template(instance) else { return instance };
        self.entries.iter().rev().find(|e| e.key == a || e.alt == a).and_then(|e| template_of(e.shown)).unwrap_or(instance)
    }

    /// `FUN_1003f121(identity)`: the entry currently showing template `instance`.
    pub fn find(&self, instance: u32) -> Option<&Entry> {
        self.entries.iter().find(|e| template_of(e.shown) == Some(instance))
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

    /// Starts / replaces the recharge record of `action`.
    #[allow(dead_code)] // no producer yet: the message that fills the client's recharge list is not decoded (docs/gui.md §10.5)
    pub fn set_recharge(&mut self, action: u32, total: f32, remaining: f32) {
        self.recharge.retain(|r| r.action != action);
        self.recharge.push(Recharge { action, total, remaining });
    }

    pub fn tick(&mut self, dt: f32) {
        for r in &mut self.recharge {
            r.remaining -= dt;
        }
        self.recharge.retain(|r| r.remaining > 0.0);
    }

    /// `N3Msg_GetSpecialActionState` [GC 0x100272b2] / `FUN_1003f156`: the action is unavailable (the slot is greyed with the timer) while
    /// it has a recharge record; unknown identities are available (`FUN_1004af3b` path, not modelled).
    pub fn unavailable(&self, instance: u32) -> bool {
        self.find(instance).is_some_and(|e| self.progress(e.shown).is_some())
    }

    /// `N3Msg_GetActionProgress` [GC 0x100275f7]: `(remaining / total, remaining seconds)` while recharging.
    pub fn progress(&self, action: u32) -> Option<(f32, i32)> {
        self.recharge.iter().find(|r| r.action == action).map(|r| (r.remaining / r.total, r.remaining.ceil() as i32))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shown(l: &SpecialList) -> Vec<u32> {
        l.entries().iter().map(|e| e.shown).collect()
    }

    /// Initial list (`FUN_10043005` + the seven `FUN_10042da4` calls): every toggle shows its first action.
    #[test]
    fn initial_list_shows_the_first_action_of_each_toggle() {
        let l = SpecialList::new();
        assert_eq!(shown(&l), [3, 1, 0xb, 0x4c, 0x11, 0x13, 0x51, 0x14, 0x86]);
        assert_eq!(l.entries()[2], Entry { key: 0xb, alt: 0x4e, shown: 0xb });
    }

    /// Sit shows Stand, release shows Sit; a hotbar slot holding Sit follows (`N3Msg_GetCorrectActionID`) and the change is queued once.
    #[test]
    fn toggles_follow_the_state_and_are_found_by_template() {
        let mut l = SpecialList::new();
        assert_eq!(l.correct_template(0xc1a8), 0xc1a8);
        l.show_other(0x4c); // sitting
        assert_eq!(l.take_changes(), [(0xc1a8, 0xc1a6)]);
        assert_eq!(l.correct_template(0xc1a8), 0xc1a6, "the Sit slot now shows Stand");
        assert_eq!(l.correct_template(0xc1a6), 0xc1a6);
        assert_eq!(l.find(0xc1a6).map(|e| e.key), Some(0x4c));
        assert!(l.find(0xc1a8).is_none(), "the identity of Sit is no longer listed (PerformSpecialAction would be ActionIsNotAvailable)");
        l.show_other(0x4d);
        assert_eq!(l.correct_template(0xc1a6), 0xc1a8);
        l.show_other(0x4d);
        assert!(l.take_changes().len() == 1, "a repeated state is not a change");
        // camping: StartCamping -> FUN_10042da4(0x51) shows 0x52
        l.show_other(0x51);
        assert_eq!(l.correct_template(0x14124), 0x1412b);
    }

    /// The map `Action_e` -> template is a bijection on the filled entries (the hotbar derives `Action_e` from it).
    #[test]
    fn action_template_map_round_trips() {
        for (a, t) in ACTION_TEMPLATES {
            if t != 0 {
                assert_eq!(action_of_template(t), Some(a), "{a:#x}");
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
        let keys: Vec<u32> = l.entries().iter().map(|e| e.key).collect();
        assert_eq!(&keys[9..], [0x94, 0x96, 0x6e]);
        l.sync_equipment(&[(0x20000, -1)], 0);
        assert_eq!(l.entries().iter().map(|e| e.key).skip(9).collect::<Vec<_>>(), [0x92]);
        l.sync_equipment(&[(0x20000, -1)], CHAR_FLAG_BACKSTAB);
        assert!(l.entries().iter().any(|e| e.key == 0x1e9));
        l.sync_equipment(&[], 0);
        assert_eq!(l.entries().len(), 9);
    }

    /// Recharge: unavailable while a record exists, progress is `remaining / total`, the icon timer text is seconds below a minute.
    #[test]
    fn recharge_greys_the_slot_and_counts_down() {
        let mut l = SpecialList::new();
        l.sync_equipment(&[(0x800, -1)], 0);
        let burst = 0x14123;
        assert!(!l.unavailable(burst));
        l.set_recharge(0x94, 20.0, 15.0);
        assert!(l.unavailable(burst));
        assert_eq!(l.progress(0x94), Some((0.75, 15)));
        l.tick(10.0);
        assert_eq!(l.progress(0x94), Some((0.25, 5)));
        l.tick(6.0);
        assert!(!l.unavailable(burst));
        assert_eq!(timer_text(5), " 5s");
        assert_eq!(timer_text(60), "60s");
        assert_eq!(timer_text(61), " 1m");
        assert_eq!(timer_text(150), " 2m");
    }
}
