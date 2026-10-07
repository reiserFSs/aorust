//! Combat / death / rebirth / victory music selection, a pure port of `SandyInterface_t` (SandyInterface.dll):
//! `CombatUpdate` @0x10005c42 (accumulator), `Frameprocess` @0x10003f61 (timer + call) and `ProcessCombatMusic`
//! @0x10001eef (state decision). Docs: formats.md `### Combat music`.
//!
//! Frame order: the game feeds [`CombatMusic::combat_update`] for every character (`Gamecode FUN_10059736`, from the
//! character `Run` `FUN_1005b016`, every frame), then calls [`CombatMusic::update`] once (the module `FrameProcess`).

/// Layer name per state 1..=16 (`LoadSimProject` @0x10005ffa fills `this+0xc8..0x104` by `CProject::FindLayerID`).
pub const LAYERS: [&str; 16] = [
    "battle\\NeutralRebirth",
    "battle\\ClanRebirth",
    "battle\\OmniRebirth",
    "battle\\NeutralDeath",
    "battle\\ClanDeath",
    "battle\\OmniDeath",
    "battle\\NeutralVictory",
    "battle\\NeutralVictory",
    "battle\\NeutralVictory",
    "battle\\Neutral",
    "battle\\WinningMedium",
    "battle\\WinningSlightlyBig",
    "battle\\WinningGreatlyBig",
    "battle\\LosingMedium",
    "battle\\LosingSlightlyBig",
    "battle\\LosingBadlyBig",
];

/// Lock after a rebirth (`_DAT_1000a274`), a death (`_DAT_1000a270`) and a victory (`_DAT_1000a26c`), seconds.
pub const REBIRTH_LOCK: f32 = 25.0;
pub const DEATH_LOCK: f32 = 5.0;
pub const VICTORY_LOCK: f32 = 20.0;

/// The `CombatUpdate(isSelf, a, b, side, id, flag, level)` arguments.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CombatSample {
    /// The controlled (local) character (`dynel+0x140 != 0`), otherwise an opponent.
    pub is_self: bool,
    /// `a`: health percent (see [`health_percent`]); 0 = dead.
    pub health_pct: i32,
    /// `b`: stat 421 (0x1a5); stored by the client but never used by a decision.
    pub enemy_metric: i32,
    /// `side`: stat 33 (0 neutral, 1 clan, 2 omni); other values count as 0.
    pub side: i32,
    /// `id`: `dynel+0x18`, the character id (opponent identity).
    pub id: i32,
    /// `flag`: byte `dynel+0x21c`.
    pub flag: bool,
    /// `level`: stat 54.
    pub level: i32,
}

/// `a` of `FUN_10059736`: `Life * 100 / MaxHealth` truncated (`_ftol`), at most 100, 1 when alive but rounded to 0.
pub fn health_percent(life: i32, max_health: i32) -> i32 {
    let mut p = if max_health == 0 { 0 } else { (f64::from(life) * 100.0 / f64::from(max_health)) as i32 };
    p = p.min(100);
    if p < 1 && life > 0 {
        p = 1;
    }
    p
}

/// What `FUN_10059736` reads from one character each frame.
#[derive(Clone, Copy, Debug, Default)]
pub struct CharInfo {
    /// `dynel+0x140`: the controlled character.
    pub is_local: bool,
    /// stat 27 Life / stat 1 MaxHealth.
    pub life: i32,
    pub max_health: i32,
    /// Fight controller (`dynel+0x1d4`) exists and its fight state (`+0x44`) is not 1 (= fighting).
    pub fighting: bool,
    /// The fight target (`controller+0x4c`) is a character (identity type 50000) whose `+0x140` is set, i.e. it attacks us.
    pub target_is_local: bool,
    pub side: i32,
    pub id: i32,
    pub flag: bool,
    pub level: i32,
    pub metric: i32,
}

/// `FUN_10059736`: the `CombatUpdate` call of one character this frame, or `None`.
/// Local char with MaxHealth: a self sample. Local char without: nothing. Other char: needs MaxHealth, a fight
/// controller in a fight state; when it targets us its health percent is reported, when it targets something else
/// only its death is reported (`a = 0`, Life == 0) so that a dead opponent is dropped.
pub fn char_sample(c: &CharInfo) -> Option<CombatSample> {
    if c.max_health == 0 {
        return None;
    }
    let mk = |is_self, health_pct| CombatSample { is_self, health_pct, enemy_metric: c.metric, side: c.side, id: c.id, flag: c.flag, level: c.level };
    if c.is_local {
        return Some(mk(true, health_percent(c.life, c.max_health)));
    }
    if !c.fighting {
        return None;
    }
    if c.target_is_local {
        Some(mk(false, health_percent(c.life, c.max_health)))
    } else if c.life == 0 {
        Some(mk(false, 0))
    } else {
        None
    }
}

#[derive(Clone, Debug)]
pub struct CombatMusic {
    /// `m_nCombatMusicPreferences` (`BattlemusicMode`, 0..=3).
    pref: i32,
    // own character (`this+0x78..0x88`)
    health: i32,
    side: i32,
    level: i32,
    // strongest opponent of this frame (`+0x8c..0xa0`)
    e_health: i32,
    e_id: i32,
    e_level: i32,
    e_flag: bool,
    /// `+0x94`: 0 = none, 1..=16.
    state: u8,
    /// `+0xa4`, `+0xb0`: opponent id / health of the last processing.
    last_id: i32,
    last_e_health: i32,
    /// `+0xa8`: `CombatUpdate` calls since the last `Frameprocess`.
    count: u32,
    /// `+0xac`: own health of the last processing.
    prev_health: i32,
    /// `+0xc0`: lock seconds left.
    timer: f32,
    /// `SetCombatMusicOverride`: layer name replacing the whole table.
    override_name: Option<String>,
}

impl CombatMusic {
    /// Constructor values of `SandyInterface_t` (@0x10002b64): own health and previous health 100, the rest 0.
    pub fn new(pref: i32) -> CombatMusic {
        CombatMusic {
            pref,
            health: 100,
            side: 0,
            level: 0,
            e_health: 0,
            e_id: 0,
            e_level: 0,
            e_flag: false,
            state: 0,
            last_id: 0,
            last_e_health: 0,
            count: 0,
            prev_health: 100,
            timer: 0.0,
            override_name: None,
        }
    }

    /// A new session keeps the user's configuration, not the old character's combat snapshot.
    pub(crate) fn reset(&mut self) {
        let override_name = self.override_name.take();
        *self = Self::new(self.pref);
        self.override_name = override_name;
    }

    /// `SetStaticBattleMusicMode(int)` @0x100019fc (GUI `SlotPrefBattlemusicModeChanged`).
    pub fn set_pref(&mut self, pref: i32) {
        self.pref = pref;
    }

    pub fn pref(&self) -> i32 {
        self.pref
    }

    /// `SetCombatMusicOverride(name)` @0x100021c1 (tweak `CombatMusicOverride`): while the state is 1..=16 this layer
    /// replaces the table layer; `None` / a name that is no layer restore the table.
    pub fn set_override(&mut self, name: Option<&str>) {
        self.override_name = name.map(str::to_owned);
    }

    pub fn override_name(&self) -> Option<&str> {
        self.override_name.as_deref()
    }

    /// Music state 0..=16 (`this+0x94`); 0 = the district music plays.
    pub fn state(&self) -> u8 {
        self.state
    }

    /// Table layer name of the current state (`None` for state 0). The override name is applied by the caller,
    /// which knows whether it is a layer (see [`CombatMusic::override_name`]).
    pub fn layer_name(&self) -> Option<&'static str> {
        (1..=16).contains(&self.state).then(|| LAYERS[self.state as usize - 1])
    }

    /// `SandyInterface_t::CombatUpdate` @0x10005c42.
    pub fn combat_update(&mut self, s: &CombatSample) {
        let side = if (0..=2).contains(&s.side) { s.side } else { 0 };
        if s.is_self {
            self.side = side;
            self.level = s.level;
            self.health = s.health_pct;
        } else if s.id == self.last_id && s.health_pct == 0 {
            // the opponent of the last processing is dead: it stays the opponent with health 0
            self.e_health = 0;
            self.e_level = s.level;
            self.e_id = s.id;
            self.e_flag = s.flag;
        } else {
            // (+0x90 = `b` is stored with a level upgrade, but nothing ever reads it)
            let take = s.health_pct != 0 && (self.e_level < s.level || (self.e_level == s.level && s.health_pct > self.e_health));
            if take {
                self.e_health = s.health_pct;
                self.e_flag = s.flag;
                self.e_level = s.level;
                self.e_id = s.id;
            }
        }
        self.count += 1;
    }

    /// `SandyInterface_t::Frameprocess` @0x10003f61 (music part): count down the lock, process when it is over,
    /// clear the per-frame counter.
    pub fn update(&mut self, dt: f32) {
        if self.timer > 0.0 {
            self.timer -= dt;
        }
        if self.timer <= 0.0 {
            self.process();
        }
        self.count = 0;
    }

    /// `SandyInterface_t::ProcessCombatMusic` @0x10001eef.
    fn process(&mut self) {
        let n = self.count;
        if n == 0 || self.pref == 0 {
            self.state = 0;
            return;
        }
        let side = self.side as u8;
        let dead = self.health == 0;
        if (self.prev_health == 0) != dead {
            // dead -> alive: rebirth; alive -> dead: death
            let (state, lock) = if dead { (side + 4, DEATH_LOCK) } else { (side + 1, REBIRTH_LOCK) };
            self.timer = lock;
            self.state = state;
        } else {
            self.state = self.fight_state(n);
        }
        self.prev_health = self.health;
        self.health = 0;
        if n > 1 {
            self.last_id = self.e_id;
            self.last_e_health = self.e_health;
        }
        self.e_health = 0;
        self.e_level = 0;
    }

    /// `LAB_10001f65`: the battle states 0, 7..=9, 10..=16 from own level `+0x84` vs the opponent's `+0x9c`.
    fn fight_state(&mut self, n: u32) -> u8 {
        if n < 2 {
            return 0;
        }
        let (enemy, own) = (i64::from(self.e_level.max(0)), i64::from(self.level.max(0)));
        // r: own/enemy "danger" ratio in percent: 100 = equal, > 100 the opponent is stronger, < 100 weaker
        let r = if own < enemy {
            200 - own * 100 / enemy
        } else if enemy < own {
            enemy * 100 / own
        } else {
            100
        };
        if own != enemy && (r < 25 || (r < 100 && self.pref < 3)) {
            return 0;
        }
        if r < 150 && self.pref < 2 {
            return 0;
        }
        if r >= 100 || !self.e_flag {
            if self.last_e_health != 0 && self.e_health == 0 {
                self.timer = VICTORY_LOCK;
                return self.side as u8 + 7;
            }
            let d = self.health - self.e_health;
            let big = u8::from(r > 133);
            if d <= -60 {
                return 14 + 2 * big;
            } else if d <= -30 {
                return 14 + big;
            } else if d >= 60 {
                return 11 + 2 * big;
            } else if d >= 30 {
                return 11 + big;
            }
        }
        10
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn me(health: i32, level: i32, side: i32) -> CombatSample {
        CombatSample { is_self: true, health_pct: health, side, level, flag: true, ..Default::default() }
    }
    fn foe(id: i32, health: i32, level: i32) -> CombatSample {
        CombatSample { is_self: false, health_pct: health, id, level, ..Default::default() }
    }
    /// One game frame: feed the samples, then `Frameprocess(dt)`.
    fn frame(m: &mut CombatMusic, dt: f32, s: &[CombatSample]) -> u8 {
        for x in s {
            m.combat_update(x);
        }
        m.update(dt);
        m.state()
    }

    #[test]
    fn table_has_16_battle_layers() {
        assert_eq!(LAYERS.len(), 16);
        let mut m = CombatMusic::new(3);
        assert_eq!(m.layer_name(), None);
        m.state = 10;
        assert_eq!(m.layer_name(), Some("battle\\Neutral"));
        m.state = 16;
        assert_eq!(m.layer_name(), Some("battle\\LosingBadlyBig"));
    }

    #[test]
    fn health_percent_rules() {
        assert_eq!(health_percent(50, 100), 50);
        assert_eq!(health_percent(150, 100), 100);
        assert_eq!(health_percent(1, 1000), 1); // alive but rounded to 0
        assert_eq!(health_percent(0, 1000), 0);
    }

    #[test]
    fn idle_is_state_zero_and_no_updates_clear_state() {
        let mut m = CombatMusic::new(3);
        assert_eq!(frame(&mut m, 0.016, &[me(100, 10, 0)]), 0); // alone: n < 2
        assert_eq!(frame(&mut m, 0.016, &[]), 0);
    }

    #[test]
    fn equal_enemy_is_neutral_battle_and_health_buckets() {
        // (own health, enemy health, enemy level, expected state), own level 50 neutral
        let table = [
            (100, 100, 50, 10), // equal, d = 0
            (100, 71, 50, 10),  // d = 29
            (100, 70, 50, 11),  // d = 30
            (100, 41, 50, 11),  // d = 59
            (100, 40, 50, 11),  // d = 60, r = 100
            (100, 40, 70, 11),  // d = 60, r = 200 - 71 = 129 (not > 133)
            (100, 40, 90, 13),  // d = 60, r = 200 - 55 = 145 > 133
            (100, 70, 90, 12),  // d = 30, r = 145
            (70, 100, 50, 14),  // d = -30
            (40, 100, 50, 14),  // d = -60 equal level: medium
            (40, 100, 90, 16),  // d = -60, r 145
            (70, 100, 90, 15),  // d = -30, r 145
        ];
        for (i, (mine, theirs, lvl, want)) in table.into_iter().enumerate() {
            let mut m = CombatMusic::new(3);
            let got = frame(&mut m, 0.016, &[me(mine, 50, 0), foe(7, theirs, lvl)]);
            assert_eq!(got, want, "row {i}");
        }
    }

    #[test]
    fn strongest_opponent_wins() {
        let mut m = CombatMusic::new(3);
        // higher level replaces; equal level only with more health; weaker ignored
        let s = frame(&mut m, 0.016, &[me(100, 50, 0), foe(1, 100, 50), foe(2, 20, 50), foe(3, 100, 10), foe(4, 90, 70)]);
        assert_eq!(m.e_id, 4);
        assert_eq!(s, 10); // r = 200 - 71 = 129, d = 10
    }

    #[test]
    fn death_and_rebirth_with_locks() {
        let mut m = CombatMusic::new(3);
        assert_eq!(frame(&mut m, 0.25, &[me(0, 50, 1), foe(7, 100, 50)]), 5); // clan death, 5 s lock
        for _ in 0..19 {
            assert_eq!(frame(&mut m, 0.25, &[me(0, 50, 1)]), 5); // locked: not processed
        }
        // lock over: still dead, no opponent (n < 2) -> 0
        assert_eq!(frame(&mut m, 0.25, &[me(0, 50, 1)]), 0);
        // alive again after having been dead -> omni rebirth, 25 s lock
        assert_eq!(frame(&mut m, 0.25, &[me(100, 50, 2)]), 3);
        for _ in 0..99 {
            assert_eq!(frame(&mut m, 0.25, &[me(100, 50, 2), foe(7, 100, 50)]), 3);
        }
        assert_eq!(frame(&mut m, 0.25, &[me(100, 50, 2), foe(7, 100, 50)]), 10);
    }

    #[test]
    fn neutral_rebirth_and_omni_death() {
        let mut m = CombatMusic::new(3);
        assert_eq!(frame(&mut m, 0.1, &[me(0, 50, 2)]), 6);
        let mut m = CombatMusic::new(3);
        assert_eq!(frame(&mut m, 0.1, &[me(0, 50, 9)]), 4); // side out of range -> 0
    }

    #[test]
    fn victory_when_the_opponent_disappears() {
        let mut m = CombatMusic::new(3);
        assert_eq!(frame(&mut m, 0.1, &[me(100, 50, 2), foe(7, 80, 50)]), 10);
        // the opponent dies: id == last id, health 0
        assert_eq!(frame(&mut m, 0.1, &[me(100, 50, 2), foe(7, 0, 50)]), 9); // omni side -> 2 + 7
        assert_eq!(m.layer_name(), Some("battle\\NeutralVictory"));
        for _ in 0..79 {
            assert_eq!(frame(&mut m, 0.25, &[me(100, 50, 2)]), 9);
        }
        // 20 s over, nobody fights: back to 0
        assert_eq!(frame(&mut m, 0.25, &[me(100, 50, 2)]), 0);
    }

    #[test]
    fn pref_gating() {
        // (pref, enemy level, expected) own level 100: enemy 100 -> r 100, 40 -> r 40, 20 -> r 20 (< 25), 250 -> r 200 - 40 = 160
        let table = [
            (0, 100, 0),
            (1, 100, 0),
            (2, 100, 10),
            (3, 100, 10),
            (1, 250, 10), // r = 160 >= 150: even pref 1
            (2, 40, 0),   // r = 40 < 100 needs pref 3
            (3, 40, 10),
            (3, 20, 0), // r < 25
            (1, 180, 0), // r = 200 - 55 = 145 < 150 needs pref 2
            (2, 180, 10),
        ];
        for (i, (pref, lvl, want)) in table.into_iter().enumerate() {
            let mut m = CombatMusic::new(pref);
            assert_eq!(frame(&mut m, 0.016, &[me(100, 100, 0), foe(7, 100, lvl)]), want, "row {i}");
        }
    }

    #[test]
    fn flag_keeps_weaker_opponents_neutral() {
        // r < 100 with the opponent flag set -> plain battle\Neutral even with a big health difference
        let mut m = CombatMusic::new(3);
        let mut f = foe(7, 10, 60);
        f.flag = true;
        assert_eq!(frame(&mut m, 0.016, &[me(100, 100, 0), f]), 10);
        let mut m = CombatMusic::new(3);
        f.flag = false;
        assert_eq!(frame(&mut m, 0.016, &[me(100, 100, 0), f]), 11);
    }

    #[test]
    fn char_sample_selection() {
        let base = CharInfo { max_health: 100, life: 50, level: 7, id: 5, ..Default::default() };
        // local: self sample
        let s = char_sample(&CharInfo { is_local: true, ..base }).unwrap();
        assert!(s.is_self && s.health_pct == 50);
        assert!(char_sample(&CharInfo { is_local: true, max_health: 0, ..base }).is_none());
        // other: idle -> nothing; fighting us -> its health; fighting others -> only death
        assert!(char_sample(&base).is_none());
        let s = char_sample(&CharInfo { fighting: true, target_is_local: true, ..base }).unwrap();
        assert!(!s.is_self && s.health_pct == 50 && s.level == 7);
        assert!(char_sample(&CharInfo { fighting: true, ..base }).is_none());
        let s = char_sample(&CharInfo { fighting: true, life: 0, ..base }).unwrap();
        assert_eq!(s.health_pct, 0);
    }
}
