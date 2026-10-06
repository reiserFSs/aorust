//! The weapon-slot objects of a character reduced to what a hit line needs: stat `DamageType` (0x1b4) of the item behind an
//! `AttackInfo` slot (docs/zone/combat-log.md §2.1.1).
//!
//! `FUN_1006a8f3` [GC 0x1006a8f3] looks the slot up with `FUN_10068072` [GC 0x10068072] (`ctrl+0x20` map slot -> slot object; a
//! non-zero `special` key `AttackInfo::unk_34` goes through `FUN_100686d0` [GC 0x100686d0], the `ctrl+0x10` map of the
//! special-attack items), `FUN_1009afde` [GC 0x1009afde] then reads stat `0x1b4` of `object+0x14` (a wielded `WeaponItem_t`)
//! or else `object+0x10` (the `DummyWeapon_t` of the `SpecialAttackWeaponIIR` item list). Every item constructor presets
//! `0x1b4 = 0x5b` (melee) before the rdb record and the message stats are applied ([`DEFAULT_DAMAGE_TYPE`]).

use super::state::CHAR_KIND;
use ao_formats::dynel_visual::{effective_stats, get, item_template, static_instance};
use ao_net::n3::{dynel::Dynel, misc::Misc, Message, N3};
use ao_rdb::RecordStore;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

/// Stat `DamageType` (436).
pub const STAT_DAMAGE_TYPE: u32 = 0x1b4;
/// What `DummyWeapon_t` [GC 0x10082512] and `WeaponItem_t` [GC 0x1009b913] preset: 0x5b = "melee".
pub const DEFAULT_DAMAGE_TYPE: i32 = 0x5b;
/// The list key of the character's martial-arts / bare-hands item (`FUN_1006ac03` registers it as slot 0).
const BARE_HANDS_KEY: i32 = 100;

/// Slots `FUN_10068072` answers: `0..=15`, `0x3d`, `0x3f`.
pub fn valid_slot(s: i32) -> bool {
    (0..0x10).contains(&s) || s == 0x3d || s == 0x3f
}

#[derive(Default)]
struct Arms {
    /// `ctrl+0x20` map: slot -> stat `0x1b4` of the item behind it.
    slots: BTreeMap<i32, i32>,
    /// `ctrl+0x10` map: special-attack key -> the same.
    specials: HashMap<i32, i32>,
    /// `FUN_10067c2b`: the item list was applied once (`ctrl+0x30 != 0`); later lists are ignored.
    listed: bool,
}

impl Arms {
    /// `FUN_10067fbe`: the filled slots below 16.
    fn filled(&self) -> usize {
        self.slots.range(..0x10).count()
    }

    /// `FUN_1006a700` [GC 0x1006a700]: a wielded weapon takes its slot; with the `0x800` stat flag clear (it is never set live,
    /// CharacterAction 0xa7, docs/zone/actions.md) the bare-hands slot 0 is dropped.
    fn wield(&mut self, slot: i32, dtype: i32) {
        self.slots.insert(slot, dtype);
        if slot != 0 {
            self.slots.remove(&0);
        }
    }

    /// `FUN_1006a772` [GC 0x1006a772]: the slot is emptied; with nothing left below slot 16 the bare-hands item (key 100) is slot 0 again.
    fn unwield(&mut self, slot: i32) {
        self.slots.remove(&slot);
        if self.filled() == 0 {
            if let Some(&t) = self.specials.get(&BARE_HANDS_KEY) {
                self.slots.insert(0, t);
            }
        }
    }

    /// `FUN_1006ac03` [GC 0x1006ac03] for one list entry `(key, stat 0x1b4 of its item)`; `npc` = `SimpleChar_t+0x21c`.
    fn list_entry(&mut self, npc: bool, key: i32, dtype: i32) {
        self.specials.entry(key).or_insert(dtype);
        let filled = self.filled();
        if npc {
            // creatures: the entries take the next free slots in list order (`FUN_1006a0d1(FUN_10067fbe(), item)`)
            if valid_slot(filled as i32) {
                self.slots.insert(filled as i32, dtype);
            }
        } else if key == BARE_HANDS_KEY && (filled == 0 || self.slots.contains_key(&0)) {
            self.slots.insert(0, dtype);
        }
    }
}

/// The slot tables of every character, fed by the zone's N3 messages.
#[derive(Default)]
pub struct Armory {
    store: Option<RecordStore>,
    by: HashMap<i32, Arms>,
    /// Weapon dynel instance -> (holder, body slot).
    worn: HashMap<i32, (i32, i32)>,
}

impl Armory {
    /// With the client's rdb for the item templates (`dir` = client directory); [`Armory::default`] sees message stats only.
    pub fn open(dir: &Path) -> Self {
        Self { store: RecordStore::open(dir).ok(), ..Self::default() }
    }

    /// Forgets every character (a playfield change).
    pub fn clear(&mut self) {
        self.by.clear();
        self.worn.clear();
    }

    /// Stat `0x1b4` of an item: the rdb 1000020 record `template` under the message stats, else [`DEFAULT_DAMAGE_TYPE`].
    fn item_type(&self, template: Option<u32>, msg: &[(u32, i32)]) -> i32 {
        let tpl = template.zip(self.store.as_ref()).and_then(|(t, s)| item_template(s, t).ok().flatten());
        get(&effective_stats(tpl.as_ref(), msg), STAT_DAMAGE_TYPE).unwrap_or(DEFAULT_DAMAGE_TYPE)
    }

    /// `holder` wields weapon dynel `item` in body `slot` (`WeaponItemFullUpdateIIR_t`).
    pub fn wield(&mut self, holder: i32, item: i32, slot: i32, template: Option<u32>, msg: &[(u32, i32)]) {
        if !valid_slot(slot) {
            return;
        }
        let t = self.item_type(template, msg);
        self.by.entry(holder).or_default().wield(slot, t);
        self.worn.insert(item, (holder, slot));
    }

    /// `SpecialAttackWeaponIIR_t` of `holder`: items `(rdb template, key)` in list order.
    pub fn list(&mut self, holder: i32, npc: bool, items: &[(u32, i32)]) {
        if items.is_empty() || self.by.get(&holder).is_some_and(|a| a.listed) {
            return;
        }
        let types: Vec<i32> = items.iter().map(|&(t, _)| self.item_type(Some(t), &[])).collect();
        let a = self.by.entry(holder).or_default();
        a.listed = true;
        for (&(_, key), t) in items.iter().zip(types) {
            a.list_entry(npc, key, t);
        }
    }

    /// Weapon dynel `item` is gone (`n3ToClientQuit`): its slot is emptied (`FUN_1006a857`).
    pub fn unwield(&mut self, item: i32) {
        if let Some((holder, slot)) = self.worn.remove(&item) {
            if let Some(a) = self.by.get_mut(&holder) {
                a.unwield(slot);
            }
        }
    }

    /// Applies the messages that change a slot table; `npc` = the header character is an NPC.
    pub fn on_message(&mut self, m: &Message, npc: bool) {
        let who = m.header.target;
        match &m.body {
            N3::Dynel(Dynel::WeaponItemFullUpdate(w)) if w.parent.kind == CHAR_KIND => {
                let stats: Vec<(u32, i32)> = w.stats.iter().map(|&(i, v)| (i as u32, v)).collect();
                self.wield(w.parent.instance, who.instance, i32::from(w.byte_71), static_instance(&stats), &stats);
            }
            N3::Dynel(Dynel::SpecialAttackWeapon(s)) if who.kind == CHAR_KIND => {
                // read order `f0 f1 f3 f2`: `f0` low-QL item, `f3` the list key (`piVar5[5]`)
                let items: Vec<(u32, i32)> = s.attacks.iter().map(|e| (e.f0 as u32, e.f3 as i32)).collect();
                self.list(who.instance, npc, &items);
            }
            N3::Misc(Misc::ToClientQuit) if who.kind == CHAR_KIND => {
                self.by.remove(&who.instance);
            }
            N3::Misc(Misc::ToClientQuit) => self.unwield(who.instance),
            _ => {}
        }
    }

    /// Stat `0x1b4` of the item `FUN_1006a8f3` finds for an `AttackInfo` of `holder` with this `slot` / `special` key; `None` = the slot
    /// object does not exist (the original then skips the hit entirely, this port cannot tell it from a table it never saw filled).
    pub fn damage_type(&self, holder: i32, slot: i32, special: i32) -> Option<i32> {
        let a = self.by.get(&holder)?;
        if special != 0 { a.specials.get(&special) } else { a.slots.get(&slot) }.copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MELEE: i32 = 0x5b;
    const PROJECTILE: i32 = 0x5a;
    const ENERGY: i32 = 0x5c;

    /// The own character of the capture: martial-arts item `43712` under key 100, specials `144`, `142`, `1`.
    fn player() -> Armory {
        let mut a = Armory::default();
        a.list(1, false, &[(43712, 100), (43713, 144), (43714, 142), (43715, 1)]);
        a
    }

    #[test]
    fn bare_hands_are_the_martial_arts_item_in_slot_0() {
        let a = player();
        assert_eq!(a.damage_type(1, 0, 0), Some(MELEE));
        assert_eq!(a.damage_type(1, 6, 0), None, "no weapon in the right hand");
        assert_eq!(a.damage_type(1, 0, 144), Some(MELEE), "a special-attack key selects its item");
        assert_eq!(a.damage_type(1, 0, 77), None);
    }

    #[test]
    fn a_wielded_weapon_replaces_the_bare_hands_and_unwielding_restores_them() {
        let mut a = player();
        a.wield(1, 500, 6, None, &[(STAT_DAMAGE_TYPE, PROJECTILE)]);
        assert_eq!(a.damage_type(1, 6, 0), Some(PROJECTILE));
        assert_eq!(a.damage_type(1, 0, 0), None, "FUN_1006a700 drops slot 0 (flag 0x800 clear)");
        a.wield(1, 501, 8, None, &[(STAT_DAMAGE_TYPE, ENERGY)]);
        assert_eq!(a.damage_type(1, 8, 0), Some(ENERGY));
        a.unwield(500);
        assert_eq!((a.damage_type(1, 6, 0), a.damage_type(1, 0, 0)), (None, None), "the left hand still holds one");
        a.unwield(501);
        assert_eq!(a.damage_type(1, 0, 0), Some(MELEE));
    }

    #[test]
    fn a_weapon_record_without_the_stat_keeps_the_item_default() {
        let mut a = Armory::default();
        a.wield(1, 500, 6, None, &[]);
        assert_eq!(a.damage_type(1, 6, 0), Some(DEFAULT_DAMAGE_TYPE));
        a.wield(1, 501, 5, None, &[(STAT_DAMAGE_TYPE, 0)]);
        assert_eq!(a.damage_type(1, 5, 0), Some(0), "an explicit 0 stays 0 here; FUN_1009afde turns it into 0x5a");
        a.wield(1, 502, 99, None, &[]);
        assert_eq!(a.damage_type(1, 99, 0), None, "slot 99 is not a weapon slot");
    }

    #[test]
    fn creature_attacks_take_the_next_free_slots_in_list_order() {
        let mut a = Armory::default();
        a.wield(7, 500, 6, None, &[(STAT_DAMAGE_TYPE, PROJECTILE)]);
        a.list(7, true, &[(11, 0x4252_4157), (12, 0x4449_4954)]);
        assert_eq!(a.damage_type(7, 6, 0), Some(PROJECTILE));
        // the wielded weapon fills slot 6 (below 16), so the first list entry goes to slot 1, the second to 2
        assert_eq!((a.damage_type(7, 1, 0), a.damage_type(7, 2, 0)), (Some(MELEE), Some(MELEE)));
        assert_eq!(a.damage_type(7, 0, 0), None);
        // a second list is ignored (`FUN_10067c2b`)
        a.list(7, true, &[(13, 1)]);
        assert_eq!(a.damage_type(7, 3, 0), None);
        let mut b = Armory::default();
        b.list(8, true, &[(11, 5), (12, 6)]);
        assert_eq!((b.damage_type(8, 0, 0), b.damage_type(8, 1, 0), b.damage_type(8, 2, 0)), (Some(MELEE), Some(MELEE), None));
    }

    /// Real item records (rdb 1000020, stat 436), skipped without the client.
    #[test]
    fn real_records() {
        let dir = std::path::PathBuf::from(std::env::var_os("HOME").unwrap()).join("Games/ProjectRubiKa/client");
        if !dir.join("cd_image/rdb.db").exists() {
            return;
        }
        let mut a = Armory::open(&dir);
        // unarmed: the martial-arts item (record 43712 has no stat 436: the item default); its special-attack siblings alike
        a.list(1, false, &[(43712, 100), (43713, 144)]);
        assert_eq!((a.damage_type(1, 0, 0), a.damage_type(1, 0, 144)), (Some(MELEE), Some(MELEE)));
        // right hand: Solar-Powered Pistol (projectile); left hand: Dull E-Blade (a 2H blade that deals energy damage)
        a.wield(1, 500, 6, Some(121567), &[]);
        a.wield(1, 501, 8, Some(122159), &[]);
        assert_eq!((a.damage_type(1, 6, 0), a.damage_type(1, 8, 0)), (Some(PROJECTILE), Some(ENERGY)));
        // a baseball bat is melee, unless the message carries its own stat
        a.wield(2, 502, 6, Some(121564), &[]);
        a.wield(2, 503, 8, Some(121564), &[(STAT_DAMAGE_TYPE, ENERGY)]);
        assert_eq!((a.damage_type(2, 6, 0), a.damage_type(2, 8, 0)), (Some(MELEE), Some(ENERGY)));
        // the creatures' innate attacks: "Monster Melee Primary Wpn" / "Generic Monster Distance Weapon"
        a.list(3, true, &[(56180, 1), (44007, 2)]);
        assert_eq!((a.damage_type(3, 0, 0), a.damage_type(3, 1, 0)), (Some(MELEE), Some(PROJECTILE)));
    }

    /// The capture: 117 of the 134 `AttackInfo` find a slot object (12 have no target, the rest arrive before the attacker's
    /// `SpecialAttackWeapon` list, which the original drops too); with the real rdb the items behind the slots have four different types.
    #[test]
    fn capture_attack_slots_resolve() {
        use ao_net::frame::{Frame, PT_N3};
        let frames: Vec<Frame> = include_str!("../../../../../docs/captures/zone_ithaca.rec")
            .lines()
            .filter_map(|l| {
                let mut p = l.split(' ');
                let (_, dir, hex) = (p.next()?, p.next()?, p.next()?);
                let b: Vec<u8> = (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap()).collect();
                (dir == "<").then(|| Frame::decode_with(&b, false).ok().flatten().map(|(f, _)| f)).flatten()
            })
            .filter(|f| f.ptype == PT_N3)
            .collect();
        let dir = std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join("Games/ProjectRubiKa/client"));
        let mut a = dir.as_deref().filter(|d| d.join("cd_image/rdb.db").exists()).map_or_else(Armory::default, Armory::open);
        let mut npcs = std::collections::HashSet::new();
        let (mut hits, mut found, mut types) = (0, 0, std::collections::BTreeSet::new());
        for f in &frames {
            let Ok(m) = ao_net::n3::decode(f) else { continue };
            let h = m.header.target;
            if let N3::Dynel(Dynel::SimpleCharFullUpdate(u)) = &m.body {
                if u.is_npc() {
                    npcs.insert(h.instance);
                }
            }
            a.on_message(&m, npcs.contains(&h.instance));
            if let N3::Misc(Misc::AttackInfo(i)) = &m.body {
                hits += 1;
                if let Some(t) = a.damage_type(h.instance, i.slot, i.unk_34) {
                    found += 1;
                    types.insert(t);
                }
            }
        }
        assert_eq!(hits, 134);
        assert_eq!(found, 117);
        assert!(types.iter().all(|&t| (0x5a..=0x61).contains(&t)), "{types:x?}");
        if a.store.is_some() {
            assert!(types.contains(&0x5a) && types.contains(&0x5b) && types.len() > 2, "{types:x?}");
        }
    }
}
