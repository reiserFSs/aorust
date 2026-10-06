//! Nano effects on the own character's movement: the Features bit counters (`FUN_10044842` / `FUN_10044a07` [GC]), the crowd-control state machine
//! (`char+0x1f0`: fear / control ...), the input lock (`dynel+0x21d`) and the `PlayerVehicle_t` <-> `NPCVehicle_t` swap (`FUN_1005a71b`), driven by
//! `ApplySpellsIIR_t` spells ([`Movement::apply_spell`]). Evidence, addresses and what is not ported: docs/zone/movement.md §10.1.

use super::{id, mode, Movement};
use ao_net::n3::spells::{function, stat, Spell};

/// Stat `Features`.
const STAT_FEATURES: u32 = 0xE0;
/// `Flags` (stat 0) bit tested by `Beholder_t` vtable `+0x14` (`(char+0x4c & mask) != 0`, `FUN_10003d5b`) in the counter functions.
const FLAG_NO_FREEZE: i32 = 0x20000;

/// Features bits the counters toggle (`FUN_10044842` / `FUN_10044a07` read them).
pub mod bits {
    /// Movement allowed (`transition` `Frozen` / `LeaveFrozen` when it changes).
    pub const MOVE: u32 = 4;
    /// Turning allowed (fear removes it).
    pub const TURN: u32 = 2;
    /// Changing it switches the vehicle's falling off / on.
    pub const NO_FALL: u32 = 8;
    /// Grant while flying: leave fly.
    pub const NO_FLY: u32 = 0x200_0000;
    /// `FUN_100a8161` fear sets these (revoked again at the end).
    pub const FEAR: u32 = 0x500;
    /// `FUN_100a8246` (stun) sets this next to revoking `MOVE | TURN`.
    pub const STUN: u32 = 0x400;
}

/// State of the effects on the own character.
#[derive(Default)]
pub(super) struct Fx {
    /// `+0x44 + bit`: signed counters of the Features bits (all 0 at the start; a bit is set while its counter is positive after a grant,
    /// cleared when it is not positive after a revoke).
    counters: [i8; 32],
    /// `char+0x1f0` state: 0 none, 1 fear, 2 charm and 4 daze (NPC-only effects, never entered by the own character), 5 control.
    crowd: u8,
    /// The vehicle is an `NPCVehicle_t` (`FUN_1005a71b(1)`).
    npc_vehicle: bool,
}

impl Movement {
    /// `SetStat(Features)` with the value mirrored into [`super::Stats`] and the stat table (`FUN_10058ca8`: no stat hook).
    fn set_features(&mut self, v: u32) {
        self.stats.features = v as i32;
        self.write_stat(STAT_FEATURES, v as i32);
    }

    /// `FUN_10044842(mask)`: counter += 1 per bit, the bit is set while the counter is positive; follow-ups for the bits that changed.
    /// (The leading block for characters piloting a vehicle object, `dynel+0x2c8 != 0`, is not ported.)
    pub fn features_grant(&mut self, mask: u32) {
        let old = self.stats.features as u32;
        let mut new = old;
        for bit in (0..32).filter(|b| mask >> b & 1 != 0) {
            let c = &mut self.fx.counters[bit];
            *c = c.wrapping_add(1);
            if *c > 0 {
                new |= 1 << bit;
            }
        }
        let changed = old ^ new;
        self.set_features(new);
        if self.stats.flags & FLAG_NO_FREEZE == 0 {
            if changed & bits::MOVE != 0 {
                self.transition(id::LEAVE_FROZEN);
                if self.surface.in_liquid {
                    self.transition(id::SWITCH_SWIM); // `Vehicle +0x120`
                }
            }
        } else {
            self.set_features(new & !bits::MOVE);
        }
        if changed & bits::NO_FALL != 0 {
            self.disable_falling();
        }
        if changed & bits::NO_FLY != 0 && self.fsm.mode == mode::FLY {
            self.transition(id::LEAVE_FLY);
        }
    }

    /// `FUN_10044a07(mask)`: counter -= 1 per bit, the bit is cleared when the counter is not positive; losing `MOVE` stops the character and
    /// freezes it (`FUN_10059ae5(1)`, `Transition(0x17)`), losing `NO_FALL` re-enables falling.
    pub fn features_revoke(&mut self, mask: u32) {
        let old = self.stats.features as u32;
        let mut new = old;
        for bit in (0..32).filter(|b| mask >> b & 1 != 0) {
            let c = &mut self.fx.counters[bit];
            *c = c.wrapping_sub(1);
            if *c < 1 {
                new &= !(1 << bit);
            }
        }
        let changed = old ^ new;
        self.set_features(new);
        if changed & bits::MOVE != 0 && self.stats.flags & FLAG_NO_FREEZE == 0 {
            self.stop_if_moving();
            self.transition(id::SWITCH_FROZEN);
        }
        if changed & bits::NO_FALL != 0 {
            self.enable_falling();
        }
    }

    /// `dynel+0x21d` (`FUN_10058d6c` sets it, `FUN_10058d7d` clears it; only for a player): while set `PlayerVehicle` vtable `[0x24]`
    /// (`FUN_10070fd0`) is false and every movement action is refused.
    fn set_input_lock(&mut self, locked: bool) {
        self.controllable = !locked;
    }

    /// The own character's input is locked (fear, control).
    pub fn input_locked(&self) -> bool {
        !self.controllable
    }

    /// `FUN_1005a71b(npc)`: the `PlayerVehicle_t` becomes an `NPCVehicle_t` (or back). A character that sits / sleeps / lounges / crawls is stood up
    /// first (`Transition` 0x29 + 0x25 for WaitState 0xF, 0x2a + 0x25 for 0x10, 0x28 for 0xE, else 0x25), then a fresh vehicle takes the pose:
    /// no velocity and no key inputs.
    fn swap_vehicle(&mut self, npc: bool) {
        if self.fx.npc_vehicle == npc {
            return;
        }
        let stand: &[u8] = match self.stats.wait_state {
            0xF => &[id::LEAVE_SLEEP, id::LEAVE_SIT],
            0x10 => &[id::LEAVE_LOUNGE, id::LEAVE_SIT],
            0xE => &[id::LEAVE_CRAWL],
            _ => &[id::LEAVE_SIT],
        };
        for t in stand {
            self.transition(*t);
        }
        self.vel = [0.0; 2];
        self.in_fwd = 0.0;
        self.in_strafe = 0.0;
        self.in_turn = 0.0;
        self.in_elev = 0.0;
        self.fx.npc_vehicle = npc;
    }

    /// The vehicle is an `NPCVehicle_t` (the key inputs have no effect on it).
    pub fn npc_vehicle(&self) -> bool {
        self.fx.npc_vehicle
    }

    /// The crowd-control state (`char+0x1f0`).
    pub fn crowd_state(&self) -> u8 {
        self.fx.crowd
    }

    /// `FUN_100458fa(new)`: a request for state `new` ends every state above it; true when no state is left.
    fn crowd_release(&mut self, new: u8) -> bool {
        let s = self.fx.crowd;
        if new < s {
            match s {
                1 => self.fear_end(),
                5 => self.control_end(),
                // states 2 / 4 only exist for NPCs (`FUN_10045894` / `FUN_100458ce` just reset the state for a player)
                2 | 4 => self.fx.crowd = 0,
                _ => {}
            }
        }
        self.fx.crowd == 0
    }

    /// `FUN_10045971`: fear starts: input lock, `NPCVehicle`, `Features` -TURN, +0x500.
    fn fear_start(&mut self) {
        if self.fx.crowd != 1 && self.crowd_release(1) {
            self.set_input_lock(true);
            self.swap_vehicle(true);
            self.features_revoke(bits::TURN);
            self.features_grant(bits::FEAR);
            self.fx.crowd = 1;
        }
    }

    /// `FUN_10045849`: fear ends.
    fn fear_end(&mut self) {
        if self.fx.crowd == 1 {
            self.set_input_lock(false);
            self.swap_vehicle(false);
            self.features_grant(bits::TURN);
            self.features_revoke(bits::FEAR);
            self.fx.crowd = 0;
        }
    }

    /// `FUN_1004593b`: state 5: input lock and `NPCVehicle` without Features changes.
    fn control_start(&mut self) {
        if self.fx.crowd != 5 && self.crowd_release(5) {
            self.set_input_lock(true);
            self.swap_vehicle(true);
            self.fx.crowd = 5;
        }
    }

    /// `FUN_10045821`.
    fn control_end(&mut self) {
        if self.fx.crowd == 5 {
            self.set_input_lock(false);
            self.swap_vehicle(false);
            self.fx.crowd = 0;
        }
    }

    /// The effect of one `ApplySpellsIIR_t` spell on the own character (`FUN_100a59f5`'s `switch` on the function id, handlers `FUN_100a8161`,
    /// `FUN_100a81eb`, `FUN_100a8246`, `FUN_100a767e`, `FUN_100a7723`); `apply` = false is the undo (`Spell_c+0x24` bit 0). Functions the
    /// movement does not react to are ignored. Returns whether the spell is one the movement reacts to.
    pub fn apply_spell(&mut self, s: &Spell, apply: bool) -> bool {
        let start = apply && s.stat(stat::EFFECT_OR_END) != 1;
        match s.function {
            function::FEAR if start => self.fear_start(),
            function::FEAR => self.fear_end(),
            function::CONTROL if start => self.control_start(),
            function::CONTROL => self.control_end(),
            function::STUN if apply => {
                self.stop_if_moving();
                self.features_revoke(bits::MOVE | bits::TURN);
                self.features_grant(bits::STUN);
            }
            function::STUN => {
                self.features_grant(bits::MOVE | bits::TURN);
                self.features_revoke(bits::STUN);
            }
            function::FEATURES_GRANT | function::FEATURES_REVOKE => {
                let mask = s.stat(stat::FEATURES_MASK) as u32;
                if apply == (s.function == function::FEATURES_GRANT) {
                    self.features_grant(mask);
                } else {
                    self.features_revoke(mask);
                }
            }
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ao_net::n3::spells::spell;

    fn fresh() -> Movement {
        Movement::new([0.0; 3], 0.0, 0)
    }

    #[test]
    fn counters_follow_fun_10044842_and_fun_10044a07() {
        let mut m = fresh();
        // a revoke of a bit with counter 0 clears it (counter -1): the character freezes
        m.action(id::FORWARD_START, 0.0);
        assert_eq!(m.fsm().fwd, 2);
        m.features_revoke(bits::MOVE);
        assert_eq!(m.stats().features & 4, 0);
        assert_eq!((m.fsm().mode, m.fsm().fwd), (mode::FROZEN, 1), "FullStop then Transition(0x17)");
        assert_eq!(m.take_stat_writes().last(), Some(&(0xE0, 0)));
        m.take_outgoing();
        m.action(id::FORWARD_START, 1.0);
        assert!(m.take_outgoing().is_empty(), "Frozen refuses the move");
        // a grant brings the counter to 0: not positive, the bit stays cleared and nothing leaves Frozen
        m.features_grant(bits::MOVE);
        assert_eq!((m.stats().features & 4, m.fsm().mode), (0, mode::FROZEN));
        // the next grant makes it 1: bit set, `Transition(0x26)` leaves Frozen
        m.features_grant(bits::MOVE);
        assert_eq!((m.stats().features & 4, m.fsm().mode), (4, mode::RUN));
        // the Flags bit 0x20000 vetoes bit 4 (the grant clears it again)
        m.set_stats(|s| s.flags |= FLAG_NO_FREEZE);
        m.features_revoke(bits::MOVE);
        m.features_revoke(bits::MOVE);
        m.features_grant(bits::MOVE);
        m.features_grant(bits::MOVE);
        m.features_grant(bits::MOVE);
        assert_eq!(m.stats().features & 4, 0);
        // bit 8 toggles falling
        let mut m = fresh();
        assert!(m.falling_enabled);
        m.features_grant(bits::NO_FALL);
        assert!(!m.falling_enabled && m.stats().features & 8 != 0);
        m.features_revoke(bits::NO_FALL);
        assert!(m.falling_enabled && m.stats().features & 8 == 0);
    }

    #[test]
    fn fear_locks_input_swaps_vehicle_and_releases() {
        let mut m = fresh();
        m.action(id::SWITCH_SIT_GROUND, 0.0);
        assert_eq!(m.fsm().mode, mode::SIT_GROUND);
        m.take_outgoing();
        let fear = spell(function::FEAR, &[]);
        assert!(m.apply_spell(&fear, true));
        assert_eq!((m.crowd_state(), m.input_locked(), m.npc_vehicle()), (1, true, true));
        assert_ne!(m.fsm().mode, mode::SIT_GROUND, "the vehicle swap stands the character up");
        assert_eq!(m.stats().features & 0x500, 0x500, "+0x500 granted");
        assert_eq!(m.stats().features & 2, 0, "-2 revoked");
        m.action(id::FORWARD_START, 0.0);
        assert!(m.take_outgoing().is_empty(), "no key reaches the locked character");
        // a repeated fear does nothing, control (5) is refused while fear (1) holds? no: 5 > 1 releases fear first
        assert!(m.apply_spell(&fear, true));
        assert_eq!(m.crowd_state(), 1);
        assert!(m.apply_spell(&spell(function::CONTROL, &[]), true));
        assert_eq!((m.crowd_state(), m.stats().features & 0x500), (5, 0), "release(5) ended the fear: -0x500, the lock stays");
        assert!(m.input_locked() && m.npc_vehicle());
        assert!(m.apply_spell(&spell(function::CONTROL, &[]), false));
        assert_eq!((m.crowd_state(), m.input_locked(), m.npc_vehicle()), (0, false, false));
        m.action(id::FORWARD_START, 1.0);
        assert_eq!(m.take_outgoing().len(), 1, "controllable again");
        // stat 0x42 == 1 ends an effect that "applies"
        let end = spell(function::FEAR, &[(stat::EFFECT_OR_END, 1)]);
        assert!(m.apply_spell(&fear, true) && m.crowd_state() == 1);
        assert!(m.apply_spell(&end, true));
        assert_eq!((m.crowd_state(), m.input_locked()), (0, false));
    }

    #[test]
    fn stun_and_feature_spells() {
        let mut m = fresh();
        m.action(id::FORWARD_START, 0.0);
        assert!(m.apply_spell(&spell(function::STUN, &[]), true));
        assert_eq!((m.fsm().mode, m.stats().features & 0x406), (mode::FROZEN, 0x400));
        // undo: the MOVE / TURN counters are at -1 -> 0, still clear (counter arithmetic of FUN_10044842)
        assert!(m.apply_spell(&spell(function::STUN, &[]), false));
        assert_eq!(m.stats().features & 0x406, 0);
        let grant = spell(function::FEATURES_GRANT, &[(stat::FEATURES_MASK, 8)]);
        let revoke = spell(function::FEATURES_REVOKE, &[(stat::FEATURES_MASK, 8)]);
        let mut m = fresh();
        m.apply_spell(&grant, true);
        assert_eq!(m.stats().features & 8, 8);
        m.apply_spell(&grant, false);
        assert_eq!(m.stats().features & 8, 0);
        m.apply_spell(&revoke, true);
        m.apply_spell(&revoke, false);
        assert_eq!(m.stats().features & 8, 0, "revoke then its undo (grant) leaves the counter at 0: not set");
        assert!(!m.apply_spell(&spell(0xCF22, &[]), true), "ModifyStat is not a movement effect");
    }
}
