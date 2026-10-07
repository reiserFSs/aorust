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
use ao_net::n3::{dynel::Dynel, misc::Misc, world::World, Message, N3};
use ao_rdb::RecordStore;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

/// Stat `DamageType` (436).
pub const STAT_DAMAGE_TYPE: u32 = 0x1b4;
/// Stat `AmmoType` (420, `FUN_1009cc50` reads it as `0x1a4`).
pub const STAT_AMMO_TYPE: u32 = 0x1a4;
/// What `DummyWeapon_t` [GC 0x10082512] and `WeaponItem_t` [GC 0x1009b913] preset: 0x5b = "melee".
pub const DEFAULT_DAMAGE_TYPE: i32 = 0x5b;
/// The list key of the character's martial-arts / bare-hands item (`FUN_1006ac03` registers it as slot 0).
const BARE_HANDS_KEY: i32 = 100;
/// `CharacterActionIIR_t` id that empties a body slot (`identity_b.instance`).
pub const ACTION_UNWIELD: i32 = 0x61;
/// Explicit wield notification (`FUN_1006ad94`), distinct from initial weapon replication.
pub const ACTION_WIELD: i32 = 0x83;
/// The `CharacterActionIIR_t` 0x61 of the rifle's unwear (header = the own character 0x82e8, `identity_b = {0, 6}`), as received live.
#[cfg(test)]
pub const UNWIELD_SLOT_6: &str = "011d000a0001003700000001000082e85e4777700000c350000082e8000000006100000000000000000000000000000000000000060000";

/// Slots `FUN_10068072` answers: `0..=15`, `0x3d`, `0x3f`.
pub fn valid_slot(s: i32) -> bool {
    (0..0x10).contains(&s) || s == 0x3d || s == 0x3f
}

/// What the fight code reads of the item behind a slot: stat `DamageType` (log line), and for the attack sounds (`FUN_1009b4ac`, `FUN_1009cd68`,
/// `FUN_1009cc50`, docs/zone/combat-anim.md section 6) the record's sound multimap, stat `AmmoType` (420) and whether it is a wielded `WeaponItem_t`
/// (`object+0x14`) rather than a `DummyWeapon_t` (`object+0x10`).
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub dtype: i32,
    pub item_class: i32,
    pub sounds: Vec<(u32, Vec<u32>)>,
    pub animations: Vec<(u32, Vec<u32>)>,
    /// Authored muzzle, hit/tracer and successful-hit impact bindings (item event 10).
    pub effects: Vec<super::effects::Binding>,
    pub effect_type: i32,
    pub impact_effect_type: i32,
    pub ammo: i32,
    pub delay: i32,
    /// Effective `AttackRange` (287); ctor defaults are 20 wielded / 2 dummy.
    pub attack_range: i32,
    pub wielded: bool,
}

#[derive(Default)]
struct Arms {
    /// `ctrl+0x20` map: slot -> the item behind it.
    slots: BTreeMap<i32, Item>,
    /// `ctrl+0x10` map: special-attack key -> the same.
    specials: HashMap<i32, Item>,
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
    fn wield(&mut self, slot: i32, item: Item) {
        self.slots.insert(slot, item);
        if slot != 0 {
            self.slots.remove(&0);
        }
    }

    /// `FUN_1006a772` [GC 0x1006a772]: the slot is emptied; with nothing left below slot 16 the bare-hands item (key 100) is slot 0 again.
    fn unwield(&mut self, slot: i32) {
        self.slots.remove(&slot);
        if self.filled() == 0 {
            if let Some(t) = self.specials.get(&BARE_HANDS_KEY) {
                self.slots.insert(0, t.clone());
            }
        }
    }

    /// `FUN_1006ac03` [GC 0x1006ac03] for one list entry `(key, stat 0x1b4 of its item)`; `npc` = `SimpleChar_t+0x21c`.
    fn list_entry(&mut self, npc: bool, key: i32, item: Item) {
        self.specials.entry(key).or_insert_with(|| item.clone());
        let filled = self.filled();
        if npc {
            // creatures: the entries take the next free slots in list order (`FUN_1006a0d1(FUN_10067fbe(), item)`)
            if valid_slot(filled as i32) {
                self.slots.insert(filled as i32, item);
            }
        } else if key == BARE_HANDS_KEY && (filled == 0 || self.slots.contains_key(&0)) {
            self.slots.insert(0, item);
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
    /// Replicated bag weapons remain resolvable for the subsequent action 0x83.
    bag: HashMap<i32, Item>,
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
        self.bag.clear();
    }

    /// The item of the rdb 1000020 record `template` under the message stats: stat `0x1b4` else [`DEFAULT_DAMAGE_TYPE`], stat 420 else the
    /// `WeaponItem_t` default -1 (`FUN_1009b913`), and the record's sounds.
    fn item(&self, template: Option<u32>, msg: &[(u32, i32)], wielded: bool) -> Item {
        let tpl = template.zip(self.store.as_ref()).and_then(|(t, s)| item_template(s, t).ok().flatten());
        let stats = effective_stats(tpl.as_ref(), msg);
        let record = template.zip(self.store.as_ref()).and_then(|(t, s)| s.get(ao_formats::dynel_visual::ITEM_TEMPLATE_TYPE, t).ok().flatten());
        let animations = record.as_ref().map(|r| ao_formats::dynel_visual::animation_map(r)).unwrap_or_default();
        let effects = record.as_ref().and_then(|r| super::super::chat::item_template_spells(r, 10).ok()).map(|s| super::effects::bindings(&s)).unwrap_or_default();
        Item { item_class: get(&stats, 0x37).unwrap_or(0), dtype: get(&stats, STAT_DAMAGE_TYPE).unwrap_or(DEFAULT_DAMAGE_TYPE), ammo: get(&stats, STAT_AMMO_TYPE).unwrap_or(-1), delay: get(&stats, 0x126).unwrap_or(200), attack_range: get(&stats, 0x11f).unwrap_or(if wielded { 20 } else { 2 }), effect_type: get(&stats, 413).unwrap_or(0), impact_effect_type: get(&stats, 414).unwrap_or(49999), sounds: tpl.map(|t| t.sounds).unwrap_or_default(), animations, effects, wielded }
    }

    /// `holder` wields weapon dynel `item` in body `slot` (`WeaponItemFullUpdateIIR_t`).
    pub fn wield(&mut self, holder: i32, item: i32, slot: i32, template: Option<u32>, msg: &[(u32, i32)]) {
        let t = self.item(template, msg, true);
        if !valid_slot(slot) {
            self.bag.insert(item, t);
            return;
        }
        self.bag.remove(&item);
        self.by.entry(holder).or_default().wield(slot, t);
        self.worn.insert(item, (holder, slot));
    }

    /// `SpecialAttackWeaponIIR_t` of `holder`: items `(rdb template, key)` in list order.
    pub fn list(&mut self, holder: i32, npc: bool, items: &[(u32, i32)]) {
        if items.is_empty() || self.by.get(&holder).is_some_and(|a| a.listed) {
            return;
        }
        let types: Vec<Item> = items.iter().map(|&(t, _)| self.item(Some(t), &[], false)).collect();
        let a = self.by.entry(holder).or_default();
        a.listed = true;
        for (&(_, key), t) in items.iter().zip(types) {
            a.list_entry(npc, key, t);
        }
    }

    /// Weapon dynel `item` is gone (`n3ToClientQuit`): its slot is emptied (`FUN_1006a857`).
    pub fn unwield(&mut self, item: i32) {
        self.bag.remove(&item);
        if let Some((holder, slot)) = self.worn.remove(&item) {
            if let Some(a) = self.by.get_mut(&holder) {
                a.unwield(slot);
            }
        }
    }

    /// `CharacterActionIIR_t` 0x61 (`FUN_1006a857` -> `FUN_1006a772`, docs/zone/actions.md): `holder`'s body `slot` is emptied. The weapon dynel stays
    /// alive (it goes back to the bag: its `WeaponItemFullUpdate` then names the bag slot), so no `n3ToClientQuit` follows.
    pub fn unwield_slot(&mut self, holder: i32, slot: i32) {
        let bag = &mut self.bag;
        let mut arms = self.by.get_mut(&holder);
        self.worn.retain(|item, worn| {
            if *worn != (holder, slot) {
                return true;
            }
            if let Some(record) = arms.as_mut().and_then(|a| a.slots.remove(&slot)) {
                bag.insert(*item, record);
            }
            false
        });
        if let Some(a) = self.by.get_mut(&holder) {
            a.unwield(slot);
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
            N3::World(World::CharacterAction(a)) if who.kind == CHAR_KIND && a.action == ACTION_UNWIELD => self.unwield_slot(who.instance, a.identity_b.instance),
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
        if special != 0 { a.specials.get(&special) } else { a.slots.get(&slot) }.map(|i| i.dtype)
    }

    /// The item behind `holder`'s body `slot` (`FUN_10068072` [GC 0x10068072]): what the attack notes read (`FUN_100688f9`).
    pub fn slot_item(&self, holder: i32, slot: i32) -> Option<&Item> {
        self.by.get(&holder)?.slots.get(&slot)
    }

    /// GC 0x10068969: hands 8/6, or bare hands 0 only when neither is occupied.
    /// GC 0x1009a68e: percentage bonus, truncate to integer, upper limit 40.
    pub fn in_attack_range(&self, holder: i32, distance: f32, bonus: i32) -> bool {
        let slots = if self.slot_item(holder, 8).is_some() || self.slot_item(holder, 6).is_some() { &[8, 6][..] } else { &[0][..] };
        slots.iter().filter_map(|&slot| self.slot_item(holder, slot)).any(|item| {
            let range = (item.attack_range as f64 + item.attack_range as f64 * bonus as f64 * 0.01) as i32;
            range.min(40) as f32 > distance
        })
    }

    /// Resolve an explicit wield notification against the replicated weapon dynel, not an outgoing inventory request.
    pub fn worn_item(&self, item: i32) -> Option<(i32, i32, &Item)> {
        let &(holder, slot) = self.worn.get(&item)?;
        Some((holder, slot, self.slot_item(holder, slot)?))
    }

    pub fn weapon_item(&self, item: i32) -> Option<&Item> {
        self.bag.get(&item).or_else(|| self.worn_item(item).map(|(_, _, item)| item))
    }


    /// Lists of the special's own item, or the bare-hands item for an ordinary unarmed attack.
    pub fn item_animation(&self, holder: i32, special: i32, key: u16, pick: u32) -> Option<(u16, u16)> {
        let a = self.by.get(&holder)?;
        let item = if special == 0 { a.slots.get(&0)? } else { a.specials.get(&special)? };
        let (resolved_key, values) = item.animations.iter().find(|e| e.0 == u32::from(key)).or_else(|| item.animations.iter().find(|e| e.0 == 0xb))?;
        Some((u16::try_from(*values.get(pick as usize % values.len().max(1))?).ok()?, u16::try_from(*resolved_key).ok()?))
    }

    pub fn swing_delay(&self, holder: i32) -> Option<i32> {
        let a = self.by.get(&holder)?;
        [6, 8, 0].iter().find_map(|s| a.slots.get(s)).map(|i| i.delay)
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
    fn attack_range_follows_hand_slots_bonus_and_strict_boundary() {
        let mut a = player();
        assert!(a.in_attack_range(1, 1.99, 0));
        assert!(!a.in_attack_range(1, 2.0, 0));
        a.wield(1, 500, 6, None, &[(0x11f, 10)]);
        assert!(a.in_attack_range(1, 11.99, 20));
        assert!(!a.in_attack_range(1, 12.0, 20));
        assert!(!a.in_attack_range(1, 10.0, 9), "percentage result truncates to 10");
        a.wield(1, 501, 8, None, &[(0x11f, 100)]);
        assert!(a.in_attack_range(1, 39.99, 0));
        assert!(!a.in_attack_range(1, 40.0, 0));
        a.unwield_slot(1, 8);
        a.unwield_slot(1, 6);
        assert!(a.in_attack_range(1, 1.99, 0));
        assert!(!a.in_attack_range(1, 2.0, 0));
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
    fn slot_unwear_retains_the_weapon_dynel_until_quit() {
        let mut a = player();
        a.wield(1, 500, 6, None, &[(STAT_DAMAGE_TYPE, PROJECTILE)]);
        a.unwield_slot(1, 6);
        assert!(a.worn_item(500).is_none());
        assert!(a.slot_item(1, 6).is_none());
        assert_eq!(a.weapon_item(500).unwrap().dtype, PROJECTILE);
        assert_eq!(a.damage_type(1, 0, 0), Some(MELEE));
        a.unwield_slot(1, 6);
        assert_eq!(a.weapon_item(500).unwrap().dtype, PROJECTILE);
        a.unwield(500);
        assert!(a.weapon_item(500).is_none());
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
        a.list(1, false, &[(43712, 100), (42033, 144), (70292, 142)]);
        assert_eq!((a.damage_type(1, 0, 0), a.damage_type(1, 0, 144)), (Some(MELEE), Some(MELEE)));
        assert_eq!(a.item_animation(1, 144, 0x24, 0), Some((163, 0xb)));
        assert_eq!(a.item_animation(1, 142, 0x23, 0), Some((1036, 0xb)));
        assert_eq!((0..4).map(|n| a.item_animation(1, 0, 0xb, n).unwrap()).collect::<Vec<_>>(), [(1034, 0xb), (1035, 0xb), (1037, 0xb), (1033, 0xb)]);
        // right hand: Solar-Powered Pistol (projectile); left hand: Dull E-Blade (a 2H blade that deals energy damage)
        a.wield(1, 500, 6, Some(121567), &[]);
        a.wield(1, 501, 8, Some(122159), &[]);
        assert_eq!((a.damage_type(1, 6, 0), a.damage_type(1, 8, 0)), (Some(PROJECTILE), Some(ENERGY)));
        let (bare, pistol) = (a.slot_item(1, 0), a.slot_item(1, 6).unwrap());
        eprintln!("bare hands {bare:?}\npistol {pistol:?}");
        assert!(bare.is_none(), "the pistol replaced the bare hands (slot 0)");
        assert!(pistol.wielded && !pistol.sounds.is_empty() && pistol.sounds.iter().any(|s| s.0 == 0xb), "the wielded weapon's record sounds: swing / shot 0xb");
        assert_eq!(pistol.effects, [
            super::super::effects::Binding { group: 0, attractor: 0, effect: 2000, note: 0, color: 0xffffffd7 },
            super::super::effects::Binding { group: 1, attractor: 0, effect: 2601, note: 0, color: 0xffffffd7 },
            super::super::effects::Binding { group: 2, attractor: 0, effect: 62002, note: 0, color: 0xffffffd7 },
        ]);
        a.wield(3, 504, 6, Some(121569), &[]);
        let rifle = a.slot_item(3, 6).unwrap();
        assert_eq!(rifle.effects.iter().map(|b| (b.group, b.effect)).collect::<Vec<_>>(), [(0,2005), (1,2750), (2,62002)]);
        let mut b = Armory::open(&dir);
        b.list(1, false, &[(43712, 100)]);
        let m = b.slot_item(1, 0).unwrap();
        assert!(!m.wielded, "the martial-arts item is a DummyWeapon (`object+0x10`)");
        assert_eq!(m.sounds, [(0xb, vec![0xc1080179]), (0x1f, vec![0x7199b60d]), (0x73, vec![0x01d88720]), (0x74, vec![0xc1080179])]);
        // a baseball bat is melee, unless the message carries its own stat
        a.wield(2, 502, 6, Some(121564), &[]);
        a.wield(2, 503, 8, Some(121564), &[(STAT_DAMAGE_TYPE, ENERGY)]);
        assert_eq!((a.damage_type(2, 6, 0), a.damage_type(2, 8, 0)), (Some(MELEE), Some(ENERGY)));
        // A separate creature has no wielded rifle occupying a slot before its innate attack list.
        // "Monster Melee Primary Wpn" / "Generic Monster Distance Weapon"
        a.list(4, true, &[(56180, 1), (44007, 2)]);
        assert_eq!((a.damage_type(4, 0, 0), a.damage_type(4, 1, 0)), (Some(MELEE), Some(PROJECTILE)));
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

    /// The newcomer rifle of the wear capture (`zone_wear_rifle_borealis.rec`: `WeaponItemFullUpdate` of item 121569 in body slot 6, projectile
    /// 0x5a) takes the right hand; the unwear is the server's `CharacterAction` 0x61 for slot 6 (the weapon update that follows only names the bag
    /// slot 0x41) and the bare hands (martial-arts item, melee) are slot 0 again.
    #[test]
    fn captured_wear_and_unwear_of_the_rifle() {
        use ao_net::frame::{Frame, PT_N3};
        let dir = std::path::PathBuf::from(std::env::var_os("HOME").unwrap()).join("Games/ProjectRubiKa/client");
        if !dir.join("cd_image/rdb.db").exists() {
            return;
        }
        let decode = |hex: &str| {
            let b: Vec<u8> = (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap()).collect();
            let f = Frame::decode_with(&b, false).unwrap().unwrap().0;
            assert_eq!(f.ptype, PT_N3);
            ao_net::n3::decode(&f).unwrap()
        };
        let rec: Vec<&str> = include_str!("../../../../../docs/captures/zone_wear_rifle_borealis.rec").lines().filter(|l| l.split(' ').nth(1) == Some("<")).map(|l| l.split(' ').nth(2).unwrap()).collect();
        let weapon_updates: Vec<Message> = rec.iter().map(|h| decode(h)).filter(|m| matches!(m.body, N3::Dynel(Dynel::WeaponItemFullUpdate(_)))).collect();
        // wear (slot 6), unwear (bag slot 0x41), wear again
        assert_eq!(weapon_updates.len(), 3);
        let own = 0x82e8;
        let mut a = Armory::open(&dir);
        a.list(own, false, &[(43712, BARE_HANDS_KEY)]);
        assert_eq!((a.damage_type(own, 0, 0), a.damage_type(own, 6, 0)), (Some(MELEE), None));
        a.on_message(&weapon_updates[0], false);
        assert_eq!((a.damage_type(own, 6, 0), a.damage_type(own, 0, 0)), (Some(PROJECTILE), None));
        a.on_message(&decode(UNWIELD_SLOT_6), false);
        assert_eq!((a.damage_type(own, 6, 0), a.damage_type(own, 0, 0)), (None, Some(MELEE)));
        a.on_message(&weapon_updates[1], false);
        assert_eq!(a.damage_type(own, 6, 0), None, "slot 0x41 is the bag, not a hand");
        a.on_message(&weapon_updates[2], false);
        assert_eq!(a.damage_type(own, 6, 0), Some(PROJECTILE));
    }
}
