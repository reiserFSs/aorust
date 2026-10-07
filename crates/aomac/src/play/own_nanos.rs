//! The own nano programs and timed nano effects as the server announced them (docs/gui.md §11.12). Fed by `Zone::on_frame`, read by the Programs / NCU
//! windows (`hud_nano.rs`, `hud_ncu.rs`).
//!
//! * Programs (`N3Msg_GetNanoSpellList` [GC 0x1001747f] = `SimpleChar+0x1c0 -> +0x14`, a `std::list<int>`): replaced by `FullCharacterIIR_t`'s `+0x18`
//!   list (`FUN_10073a2f` [GC] line `FUN_10074488(*(*(dynel+0x1c0)+0x14), msg+0x18)`), then changed by the relayed `CharacterActionIIR_t` actions 0xcc / 0xcd
//!   (`ao_net::n3::nano::list_change`).
//! * Effects: CharacterAction 0x62 / 0xb1 adds (`GC 100512af`); BuffIIR kind 0 removes (`10050afd`).
//!   Login restores SimpleCharFullUpdate.effects timing (`10051b40`, `10051741`). The zone clock, not a GUI window, expires entries.

use ao_net::msg::Identity;
use ao_net::n3::nano::{self, ListChange, NANO_KIND};
use ao_net::n3::{misc::Misc, world::World, N3};
use ao_net::n3::dynel::Dynel;
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, Default)]
struct Metadata {
    cost: i32,
    rank: i32,
    families: [i32; 6],
    effect: i32,
}

impl Metadata {
    /// GC 1004e488 / 1005195a.
    fn conflicts(self, incoming: Self) -> bool {
        if self.rank > incoming.rank { return false; }
        if self.families[1..].iter().sum::<i32>() == 0 && incoming.families[1..].iter().sum::<i32>() == 0 {
            self.families[0] == incoming.families[0]
        } else {
            incoming.families.iter().any(|f| *f != 0 && self.families.contains(f))
        }
    }
}

/// One timed nano effect of the own character.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Buff {
    pub nano: i32,
    pub started: f32,
    pub total_cs: i32,
    pub source: i32,
    pub ncu_cost: i32,
}

impl Buff {
    pub fn remaining_cs(&self, now: f32) -> i32 {
        (self.total_cs - ((now.floor() - self.started) * 100.0) as i32).max(0)
    }
}

/// Retail active-entry visual handle changes, independent of the scaled NCU timer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum VisualEvent {
    Add { nano: i32, effect: i32, duration_cs: i32 },
    Remove { nano: i32 },
}

#[derive(Default, Debug)]
pub struct OwnNanos {
    /// The program list in server order.
    pub programs: Vec<i32>,
    pub buffs: Vec<Buff>,
    metadata: HashMap<i32, Option<Metadata>>,
    /// Successful new uploads awaiting the GUI's retail learned-nano signal consumers.
    learned: Vec<i32>,
    visuals: Vec<VisualEvent>,
    /// Seconds since the zone state was created (advanced by [`OwnNanos::tick`]); the clock of [`Buff::started`].
    pub time: f32,
    /// Incremented by every change of `programs` / `buffs`: the windows redraw when it moves.
    pub serial: u32,
}

impl OwnNanos {
    pub fn tick(&mut self, dt: f32) -> i32 {
        self.time += dt;
        // Retail timer getter clamps to zero; the server's BuffIIR removes expired effects.
        0
    }

    fn cost(&self) -> i32 {
        self.buffs.iter().map(|b| b.ncu_cost).sum()
    }

    fn metadata(&mut self, id: i32) -> Option<Metadata> {
        *self.metadata.entry(id).or_insert_with(|| {
            let load = || -> Result<Metadata, String> {
                let store = ao_rdb::RecordStore::open(&ao_gui::client_dir()).map_err(|e| e.to_string())?;
                let instance = u32::try_from(id).map_err(|e| e.to_string())?;
                let record = store.get(super::hud_nanodb::NANO_RDB_TYPE, instance).map_err(|e| e.to_string())?
                    .ok_or_else(|| format!("record {}:{id} not found", super::hud_nanodb::NANO_RDB_TYPE))?;
                let t = ao_formats::dynel_visual::parse_item_template(&record).map_err(|e| e.to_string())?;
                let flags = t.stat(0).unwrap_or(0);
                Ok(Metadata {
                    cost: if flags & 0x10000 != 0 && flags & (0x4000 | 0x8000 | 0x100) == 0 { t.stat(54).unwrap_or(0) } else { 0 },
                    rank: t.stat(551).unwrap_or(0),
                    families: [75, 546, 547, 548, 549, 550].map(|s| t.stat(s).unwrap_or(0)),
                    effect: t.stat(413).unwrap_or(1_234_567_890),
                })
            };
            match load() {
                Ok(metadata) => Some(metadata),
                Err(error) => {
                    eprintln!("nano {id}: metadata unavailable; NCU/conflict accounting unresolved: {error}");
                    None
                }
            }
        })
    }

    #[cfg(test)]
    fn add(&mut self, buff: Buff, replace: bool) {
        self.add_visual(buff, replace, buff.total_cs);
    }

    fn add_visual(&mut self, buff: Buff, replace: bool, duration_cs: i32) {
        let meta = self.metadata(buff.nano);
        if replace {
            let metadata = &self.metadata;
            let visuals = &mut self.visuals;
            self.buffs.retain(|b| {
                let remove = b.nano == buff.nano || match (metadata.get(&b.nano).copied().flatten(), meta) {
                    (Some(existing), Some(incoming)) => existing.conflicts(incoming),
                    _ => false,
                };
                if remove { visuals.push(VisualEvent::Remove { nano: b.nano }); }
                !remove
            });
        }
        self.buffs.push(Buff { ncu_cost: meta.map_or(buff.ncu_cost, |m| m.cost), ..buff });
        if let Some(metadata) = meta {
            self.visuals.push(VisualEvent::Add { nano: buff.nano, effect: metadata.effect, duration_cs });
        }
        self.serial += 1;
    }

    pub fn take_learned(&mut self) -> Vec<i32> {
        std::mem::take(&mut self.learned)
    }

    pub fn take_visuals(&mut self) -> Vec<VisualEvent> {
        std::mem::take(&mut self.visuals)
    }

    /// Applies a message addressed to the own character (`header` identity == `own`).
    pub fn on_message(&mut self, who: Identity, own: Identity, body: &N3, duration_percent: i32) -> i32 {
        if who != own {
            return 0;
        }
        let before = self.cost();
        match body {
            N3::World(World::FullCharacter(c)) => {
                self.programs = c.list_18.clone();
                self.serial += 1;
            }
            N3::World(World::CharacterAction(a)) if !matches!(a.action, 0x62 | 0xb1) => match nano::list_change(a) {
                // `FUN_1004fbbc`: only when not in the list yet; the record check (`FUN_100a45ba`) is the windows' (an unknown id has no row)
                Some(ListChange::Learned(id)) if !self.programs.contains(&id) => {
                    self.programs.push(id);
                    self.learned.push(id);
                    self.serial += 1;
                }
                Some(ListChange::Forgotten(id)) if self.programs.contains(&id) => {
                    self.programs.retain(|p| *p != id);
                    // Forgetting a program does not remove its already-running effect.
                    self.serial += 1;
                }
                _ => {}
            },
            N3::World(World::CharacterAction(a)) if matches!(a.action, 0x62 | 0xb1) && a.identity_a.kind == NANO_KIND => {
                self.add_visual(Buff {
                    nano: a.identity_a.instance,
                    source: a.identity_b.kind,
                    started: self.time.floor(),
                    total_cs: ((i64::from(a.identity_b.instance) * i64::from(duration_percent)) / 100).max(0) as i32,
                    ..Default::default()
                }, true, a.identity_b.instance);
            }
            N3::Dynel(Dynel::SimpleCharFullUpdate(u)) => {
                self.visuals.extend(self.buffs.iter().map(|b| VisualEvent::Remove { nano: b.nano }));
                self.buffs.clear();
                for e in &u.effects {
                    if e.source.kind == NANO_KIND {
                        self.add_visual(Buff { nano: e.source.instance, started: self.time.floor() - (e.b - e.c) as f32 / 100.0,
                            total_cs: e.b, ..Default::default() }, false, e.c.max(1));
                    }
                }
                self.serial += 1;
            }
            N3::Misc(Misc::Buff(b)) if b.kind == 0 => {
                if let Some(id) = b.nano.filter(|n| n.kind == NANO_KIND) {
                    let len = self.buffs.len();
                    self.buffs.retain(|b| {
                        if b.nano == id.instance {
                            self.visuals.push(VisualEvent::Remove { nano: b.nano });
                            false
                        } else { true }
                    });
                    if self.buffs.len() != len { self.serial += 1; }
                }
            }
            _ => {}
        }
        self.cost() - before
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
        n.on_message(me(), me(), &N3::World(World::FullCharacter(Box::new(fc))), 100);
        assert_eq!(n.programs, [5, 6]);
        assert!(n.take_learned().is_empty(), "login list is not an upload signal");
        n.on_message(me(), me(), &action(nano::action::LEARNED, 9), 100);
        n.on_message(me(), me(), &action(nano::action::LEARNED, 9), 100);
        assert_eq!(n.programs, [5, 6, 9]);
        assert_eq!(n.take_learned(), [9], "duplicate replies do not repeat the GUI signal");
        n.on_message(me(), me(), &action(nano::action::FORGOTTEN, 5), 100);
        assert_eq!(n.programs, [6, 9]);
        // somebody else's actions are not ours
        n.on_message(Identity { kind: 0xC350, instance: 8 }, me(), &action(nano::action::LEARNED, 1), 100);
        assert_eq!(n.programs, [6, 9]);
    }

    /// Synthetic supplementary lifecycle: duration scaling, refresh, timer zero and authoritative expiry removal.
    #[test]
    fn synthetic_lifecycle() {
        let mut n = OwnNanos::default();
        n.metadata.insert(1, Some(Metadata { cost: 3, effect: 1070, ..Default::default() }));
        let add = |op| N3::World(World::CharacterAction(simple(op, nano::nano(1), Identity { kind: 42, instance: 1000 })));
        n.tick(2.9);
        assert_eq!(n.on_message(me(), me(), &add(0x62), 150), 3);
        assert_eq!(n.buffs[0].started, 2.0);
        assert_eq!(n.buffs[0].total_cs, 1500);
        assert_eq!(n.take_visuals(), [VisualEvent::Add { nano: 1, effect: 1070, duration_cs: 1000 }], "visuals receive authored duration, not the NCU duration multiplier");
        n.tick(1.0);
        assert_eq!(n.on_message(me(), me(), &add(0xb1), 100), 0);
        assert_eq!(n.buffs.len(), 1);
        assert_eq!(n.take_visuals(), [
            VisualEvent::Remove { nano: 1 },
            VisualEvent::Add { nano: 1, effect: 1070, duration_cs: 1000 },
        ], "refresh terminates the old handle before creating its replacement");
        n.tick(20.0);
        assert_eq!(n.buffs[0].remaining_cs(n.time), 0);
        assert!(n.take_visuals().is_empty(), "timer zero is not an authoritative visual removal");
        let remove = N3::Misc(Misc::Buff(misc::Buff { kind: 0, nano: Some(nano::nano(1)), rest: vec![] }));
        assert_eq!(n.on_message(me(), me(), &remove, 100), -3);
        assert_eq!(n.on_message(me(), me(), &remove, 100), 0);
        assert!(n.buffs.is_empty());
        assert_eq!(n.take_visuals(), [VisualEvent::Remove { nano: 1 }], "duplicate removal emits no second termination");
    }

    #[test]
    fn missing_metadata_does_not_conflict_with_unrelated_effects() {
        let mut n = OwnNanos::default();
        n.metadata.insert(1, None);
        n.metadata.insert(2, None);
        n.add(Buff { nano: 1, total_cs: 100, ..Default::default() }, true);
        n.add(Buff { nano: 2, total_cs: 100, ..Default::default() }, true);
        assert_eq!(n.buffs.len(), 2);
        n.add(Buff { nano: 1, total_cs: 200, ..Default::default() }, true);
        assert_eq!(n.buffs.len(), 2);
        assert_eq!(n.buffs.iter().find(|b| b.nano == 1).unwrap().total_cs, 200);
    }

    /// Synthetic supplementary conflict families/priority from GC 1004e488.
    #[test]
    fn synthetic_conflict_priority_and_families() {
        let old = Metadata { rank: 4, families: [7, 0, 0, 0, 0, 0], ..Default::default() };
        assert!(!old.conflicts(Metadata { rank: 3, ..old }));
        assert!(old.conflicts(Metadata { rank: 4, ..old }));
        assert!(!old.conflicts(Metadata { families: [8, 0, 0, 0, 0, 0], ..old }));
        let multiple = Metadata { families: [8, 7, 0, 0, 0, 0], ..old };
        assert!(old.conflicts(multiple));
        assert!(Metadata::default().conflicts(Metadata::default()), "zero primary family is compared too");
        let mut nanos = OwnNanos::default();
        nanos.metadata.insert(1, Some(Metadata { effect: 1070, ..old }));
        nanos.metadata.insert(2, Some(Metadata { effect: 1070, ..multiple }));
        nanos.add(Buff { nano: 1, total_cs: 1000, ..Default::default() }, true);
        nanos.take_visuals();
        nanos.add(Buff { nano: 2, total_cs: 2000, ..Default::default() }, true);
        assert_eq!(nanos.take_visuals(), [
            VisualEvent::Remove { nano: 1 },
            VisualEvent::Add { nano: 2, effect: 1070, duration_cs: 2000 },
        ]);
    }
}
