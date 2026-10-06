//! The own nano programs and timed nano effects as the server announced them (docs/gui.md §11.12). Fed by `Zone::on_frame`, read by the Programs / NCU
//! windows (`hud_nano.rs`, `hud_ncu.rs`).
//!
//! * Programs (`N3Msg_GetNanoSpellList` [GC 0x1001747f] = `SimpleChar+0x1c0 -> +0x14`, a `std::list<int>`): replaced by `FullCharacterIIR_t`'s `+0x18`
//!   list (`FUN_10073a2f` [GC] line `FUN_10074488(*(*(dynel+0x1c0)+0x14), msg+0x18)`), then changed by the relayed `CharacterActionIIR_t` actions 0xcc / 0xcd
//!   (`ao_net::n3::nano::list_change`).
//! * Timed effects (the NCU window's rows, `N3Msg_GetNanoTemplateInfoList` = `+0x20`): `BuffIIR_c` [GC 0x1007213b] kind 0 with the nano id; the client then
//!   runs `FUN_10050afd` (adds the entry, raises `CurrentNCU` 0xb4 by the nano's NCU). The entry's start time is the game time of the message
//!   (`N3Msg_GetBuffCurrentTime` [GC 0x10017831] = `FUN_1004eb5a`: `entry+0xc + entry+8 - gametime*100`), its total the nano record's `TimeExist` (stat 8, 1/100 s;
//!   `FUN_10085fd4`).

use ao_net::msg::Identity;
use ao_net::n3::nano::{self, ListChange, NANO_KIND};
use ao_net::n3::{misc::Misc, world::World, N3};

/// One timed nano effect of the own character.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Buff {
    pub nano: i32,
    /// [`OwnNanos::time`] when the effect started.
    pub started: f32,
}

#[derive(Default, Debug)]
pub struct OwnNanos {
    /// The program list in server order.
    pub programs: Vec<i32>,
    pub buffs: Vec<Buff>,
    /// Successful new uploads awaiting the GUI's retail learned-nano signal consumers.
    learned: Vec<i32>,
    /// Seconds since the zone state was created (advanced by [`OwnNanos::tick`]); the clock of [`Buff::started`].
    pub time: f32,
    /// Incremented by every change of `programs` / `buffs`: the windows redraw when it moves.
    pub serial: u32,
}

impl OwnNanos {
    pub fn tick(&mut self, dt: f32) {
        self.time += dt;
    }

    pub fn take_learned(&mut self) -> Vec<i32> {
        std::mem::take(&mut self.learned)
    }

    /// Applies a message addressed to the own character (`header` identity == `own`).
    pub fn on_message(&mut self, who: Identity, own: Identity, body: &N3) {
        if who != own {
            return;
        }
        match body {
            N3::World(World::FullCharacter(c)) => {
                self.programs = c.list_18.clone();
                self.serial += 1;
            }
            N3::World(World::CharacterAction(a)) => match nano::list_change(a) {
                // `FUN_1004fbbc`: only when not in the list yet; the record check (`FUN_100a45ba`) is the windows' (an unknown id has no row)
                Some(ListChange::Learned(id)) if !self.programs.contains(&id) => {
                    self.programs.push(id);
                    self.learned.push(id);
                    self.serial += 1;
                }
                Some(ListChange::Forgotten(id)) if self.programs.contains(&id) => {
                    self.programs.retain(|p| *p != id);
                    self.buffs.retain(|b| b.nano != id);
                    self.serial += 1;
                }
                _ => {}
            },
            // `FUN_1007219c`: acts only for kind 0 with a nano identity (`FUN_10050afd` replaces an entry of the same nano)
            N3::Misc(Misc::Buff(b)) if b.kind == 0 => {
                if let Some(id) = b.nano.filter(|n| n.kind == NANO_KIND) {
                    self.buffs.retain(|x| x.nano != id.instance);
                    self.buffs.push(Buff { nano: id.instance, started: self.time });
                    self.serial += 1;
                }
            }
            _ => {}
        }
    }

    /// Drops the effects `expired` names (the NCU window decides from the records' durations).
    pub fn remove_buffs(&mut self, expired: &[i32]) {
        if !expired.is_empty() {
            self.buffs.retain(|b| !expired.contains(&b.nano));
            self.serial += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ao_net::n3::action::simple;
    use ao_net::n3::misc;

    fn me() -> Identity {
        Identity { kind: 0xC350, instance: 7 }
    }

    fn action(a: i32, id: i32) -> N3 {
        N3::World(World::CharacterAction(simple(a, Identity::default(), nano::nano(id))))
    }

    #[test]
    fn list_follows_the_server() {
        let mut n = OwnNanos::default();
        let fc = ao_net::n3::world::FullCharacter { list_18: vec![5, 6], ..Default::default() };
        n.on_message(me(), me(), &N3::World(World::FullCharacter(Box::new(fc))));
        assert_eq!(n.programs, [5, 6]);
        assert!(n.take_learned().is_empty(), "login list is not an upload signal");
        n.on_message(me(), me(), &action(nano::action::LEARNED, 9));
        n.on_message(me(), me(), &action(nano::action::LEARNED, 9));
        assert_eq!(n.programs, [5, 6, 9]);
        assert_eq!(n.take_learned(), [9], "duplicate replies do not repeat the GUI signal");
        n.on_message(me(), me(), &action(nano::action::FORGOTTEN, 5));
        assert_eq!(n.programs, [6, 9]);
        // somebody else's actions are not ours
        n.on_message(Identity { kind: 0xC350, instance: 8 }, me(), &action(nano::action::LEARNED, 1));
        assert_eq!(n.programs, [6, 9]);
    }

    #[test]
    fn buffs_start_at_the_message_time() {
        let mut n = OwnNanos::default();
        n.tick(2.0);
        let buff = |kind| N3::Misc(Misc::Buff(misc::Buff { kind, nano: Some(nano::nano(163449)), rest: vec![] }));
        n.on_message(me(), me(), &buff(1));
        assert!(n.buffs.is_empty());
        n.on_message(me(), me(), &buff(0));
        n.tick(1.0);
        n.on_message(me(), me(), &buff(0));
        assert_eq!(n.buffs, [Buff { nano: 163449, started: 3.0 }]);
        n.remove_buffs(&[163449]);
        assert!(n.buffs.is_empty());
    }
}
