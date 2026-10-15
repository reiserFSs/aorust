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
    /// Effective Ammo (26), updated only by server weapon/reload messages.
    pub clip: i32,
    /// Stat `AttackDelay` (0x126 = 294).
    pub delay: i32,
    /// Stat `RechargeDelay` (0xd2 = 210): the item-info DPS cell's other half of the round time
    /// (`chat::item_info_combat::delay`, `recharge = true`).
    pub recharge: i32,
    /// Stat `InitiativeSkill` (440): which of the character's skills scales `delay`/`recharge` (`round_seconds`).
    pub initiative_skill: i32,
    /// Stat `AttackDelayCap` (523), present on some weapons.
    pub attack_cap: Option<i32>,
    /// Stat `RechargeDelayCap` (524).
    pub recharge_cap: Option<i32>,
    /// Effective `AttackRange` (287); ctor defaults are 20 wielded / 2 dummy.
    pub attack_range: i32,
    pub wielded: bool,
}

/// GC 1006772b/10067849: hundredths, current raw initiative and AggDef, the post-1200 initiative slope, individual item
/// caps, then the one-second floor. Shared by the item-info DPS cell (`chat::item_info_combat::delay`) and the Attack /
/// Reload HUD timer bars (`item_round_seconds`, `hud_stats::update_attack_timers`).
pub fn round_delay(raw: i32, cap: Option<i32>, initiative: i32, agg_def: i32, recharge: bool) -> f32 {
    let initiative = if initiative <= 1200 { initiative as f32 * 0.5 } else { (initiative - 1200) as f32 / 6.0 + 600.0 };
    let mut time = (raw as f32 + 75.0) - initiative * if recharge { 2.0 } else { 1.0 } / 3.0 - agg_def as f32;
    if let Some(cap) = cap {
        time = time.max(cap as f32);
    }
    time.max(100.0)
}

/// Total round time (seconds) of `item`: `round_delay(AttackDelay) + round_delay(RechargeDelay)`, both initiative-scaled
/// against the own character's current raw stats (`stats`, `Zone::stats`). `recharge_init` follows stat 118 unless
/// `initiative_skill` already lies in 0..=1200 (`chat::item_info_combat::fields`, `FUN_10067849`).
pub fn item_round_seconds(item: &Item, stats: &HashMap<u32, i32>) -> f32 {
    use ao_formats::stats::INVALID;
    let agg_def = stats.get(&51).copied().unwrap_or(INVALID);
    let init = item.initiative_skill;
    let attack_init = stats.get(&(init as u32)).copied().unwrap_or(INVALID);
    let recharge_init = stats.get(&(if (0..=1200).contains(&init) { init } else { 118 } as u32)).copied().unwrap_or(INVALID);
    (round_delay(item.delay, item.attack_cap, attack_init, agg_def, false)
        + round_delay(item.recharge, item.recharge_cap, recharge_init, agg_def, true)) * 0.01
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
        let invalid = ao_formats::stats::INVALID;
        Item { item_class: get(&stats, 0x37).unwrap_or(0), dtype: get(&stats, STAT_DAMAGE_TYPE).unwrap_or(DEFAULT_DAMAGE_TYPE), ammo: get(&stats, STAT_AMMO_TYPE).unwrap_or(-1), clip: get(&stats, 26).unwrap_or(invalid), delay: get(&stats, 0x126).unwrap_or(200), recharge: get(&stats, 0xd2).unwrap_or(200), initiative_skill: get(&stats, 440).unwrap_or(invalid), attack_cap: get(&stats, 523).filter(|&v| v != invalid), recharge_cap: get(&stats, 524).filter(|&v| v != invalid), attack_range: get(&stats, 0x11f).unwrap_or(if wielded { 20 } else { 2 }), effect_type: get(&stats, 413).unwrap_or(0), impact_effect_type: get(&stats, 414).unwrap_or(49999), sounds: tpl.map(|t| t.sounds).unwrap_or_default(), animations, effects, wielded }
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
            // The server never replicates the own character's already-worn weapon dynel at zone-in the way it does for other
            // characters (docs/zone/static.md §4; live capture 2026-10-12, Aomacfixr: zero `WeaponItemFullUpdateIIR_t` of the own
            // character in 587 zone-in frames). `FullCharacterIIR_t::rest` was empty, so `inventory` (`FUN_1002a41a`) is the one
            // zone-in source left: its elements carry the same `(holder, item id, slot, rdb template)` a live update would, with
            // no stat overrides. `wield` itself routes anything outside `valid_slot` into the resolvable bag map.
            N3::World(World::FullCharacter(c)) if who.kind == CHAR_KIND => {
                for e in &c.inventory {
                    self.wield(who.instance, e.id.instance, e.slot as i32, Some(e.item.low_id as u32), &[]);
                }
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
        if special != 0 { a.specials.get(&special) } else { a.slots.get(&slot) }.map(|i| i.dtype)
    }

    /// The item behind `holder`'s body `slot` (`FUN_10068072` [GC 0x10068072]): what the attack notes read (`FUN_100688f9`).
    pub fn slot_item(&self, holder: i32, slot: i32) -> Option<&Item> {
        self.by.get(&holder)?.slots.get(&slot)
    }

    /// GC 0x10068969: hands 8/6, or bare hands 0 only when neither is occupied -- every wielded hand, for `in_attack_range`
    /// and the per-hand Attack timer bar (`item_round_seconds`, `hud_stats::update_attack_timers`).
    pub fn attack_slots(&self, holder: i32) -> Vec<i32> {
        let hands: Vec<i32> = [8, 6].into_iter().filter(|&s| self.slot_item(holder, s).is_some()).collect();
        if hands.is_empty() { vec![0] } else { hands }
    }

    /// GC 0x1009a68e: percentage bonus, truncate to integer, upper limit 40.
    pub fn in_attack_range(&self, holder: i32, distance: f32, bonus: i32) -> bool {
        self.attack_slots(holder).iter().filter_map(|&slot| self.slot_item(holder, slot)).any(|item| {
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

    /// GC10069274: set the named weapon's nonnegative Ammo; return raw RechargeDelay.
    pub fn reload(&mut self, weapon: ao_net::msg::Identity, clip: i32) -> Option<i32> {
        if weapon.kind != ao_net::n3::dynel::KIND_WEAPON_ITEM { return None; }
        let item = if let Some(&(holder, slot)) = self.worn.get(&weapon.instance) {
            self.by.get_mut(&holder)?.slots.get_mut(&slot)?
        } else {
            self.bag.get_mut(&weapon.instance)?
        };
        if item.clip < 0 { return None; }
        item.clip = clip;
        Some(item.recharge)
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
    fn full_character_inventory_seeds_the_own_weapon_at_zone_in() {
        use ao_net::msg::Identity;
        use ao_net::n3::world::{AcgItem, FullCharacter, InventoryEntry};
        use ao_net::n3::{Message, N3Header};
        let own = 33588;
        let msg = |inventory: Vec<InventoryEntry>| Message {
            header: N3Header { msg_type: 0, target: Identity { kind: CHAR_KIND, instance: own }, flag: 0 },
            sender: 1,
            body: N3::World(World::FullCharacter(Box::new(FullCharacter { inventory, ..FullCharacter::default() }))),
        };
        // the right-hand weapon-page entry (slot 6) wields exactly like a live `WeaponItemFullUpdateIIR_t` would
        let mut a = Armory::default();
        a.on_message(&msg(vec![InventoryEntry {
            slot: 6, a: 0, b: 1,
            id: Identity { kind: 0xc74a, instance: 900 },
            item: AcgItem { low_id: 121569, high_id: 121569, level: 1 },
        }]), false);
        assert!(a.slot_item(own, 6).is_some(), "the own right-hand slot resolves without a live WeaponItemFullUpdate");
        assert!(a.in_attack_range(own, 19.99, 0), "the wielded attack range, not the bare-hands fallback of 2");
        assert!(!a.in_attack_range(own, 20.0, 0));
        // a bag entry (slot 0x40, outside `valid_slot`) is not an attack slot: it stays resolvable for a
        // later explicit wield (CharacterAction 0x83), not inserted into `slots`
        let mut b = Armory::default();
        b.on_message(&msg(vec![InventoryEntry {
            slot: 0x40, a: 0, b: 1,
            id: Identity { kind: 0xc74a, instance: 901 },
            item: AcgItem { low_id: 121569, high_id: 121569, level: 1 },
        }]), false);
        assert!(b.slot_item(own, 6).is_none(), "a bagged weapon is not worn");
        assert!(b.weapon_item(901).is_some(), "but it is resolvable for a later explicit wield");
    }

    /// Real `FullCharacterIIR_t` (Aomacfixr zone-in, 2026-10-12, own char 33588): the rifle (template 121569) is in
    /// bag slot 0x40, not currently worn. Exercises the real wire parse against the gating this fix relies on.
    #[test]
    fn real_zone_in_full_character_inventory_does_not_falsely_wield_a_bagged_item() {
        let line = include_str!("../../../../../docs/captures/zone_own_full_character_ithaca.rec");
        let hex = line.split_whitespace().nth(2).unwrap();
        let bytes: Vec<u8> = (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap()).collect();
        let (frame, _) = ao_net::frame::Frame::decode_with(&bytes, false).unwrap().unwrap();
        let m = ao_net::n3::decode(&frame).unwrap();
        let mut a = Armory::default();
        a.on_message(&m, false);
        assert_eq!(a.slot_item(33588, 6), None, "not currently worn: the rifle sits in the bag (slot 0x40)");
        assert!(a.weapon_item(24763172).is_some(), "but the bag item is resolvable for a later explicit wield");
    }

    /// The captured Solar rifle carries ammo count zero, not an exhausted conventional clip.
    /// Its template's self-supplied ammunition is pinned separately below; docs/zone/combat-timers.md §3.
    #[test]
    fn captured_solar_rifle_keeps_zero_ammo_across_wear_unwear() {
        let mut updates = 0;
        for line in include_str!("../../../../../docs/captures/zone_wear_rifle_borealis.rec").lines() {
            let mut fields = line.split_whitespace();
            let _ms = fields.next().unwrap();
            let direction = fields.next().unwrap();
            let hex = fields.next().unwrap();
            if direction != "<" { continue; }
            let bytes: Vec<u8> = (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap()).collect();
            let (frame, _) = ao_net::frame::Frame::decode_with(&bytes, false).unwrap().unwrap();
            let m = ao_net::n3::decode(&frame).unwrap();
            assert_ne!(m.header.msg_type, 0x2651_5e61, "wear/unwear does not report a reload");
            if let N3::Dynel(Dynel::WeaponItemFullUpdate(w)) = m.body {
                assert_eq!((m.header.target.instance, w.parent.instance), (13948944, 33512));
                assert_eq!(w.stats.iter().find(|&&(id, _)| id == 23).map(|&(_, value)| value), Some(121569));
                assert_eq!(w.stats.iter().find(|&&(id, _)| id == 26).map(|&(_, value)| value), Some(0));
                updates += 1;
            }
        }
        assert_eq!(updates, 3, "wear, unwear and re-wear all preserve the same ammo count");
    }

    #[test]
    fn solar_assault_rifle_has_self_supplied_ammo_and_no_finite_clip() {
        let dir = std::path::PathBuf::from(std::env::var_os("HOME").unwrap()).join("Games/ProjectRubiKa/client");
        if !dir.join("cd_image/rdb.db").exists() { return; }
        let store = RecordStore::open(&dir).unwrap();
        let template = item_template(&store, 121569).unwrap().unwrap();
        assert_eq!(template.name.as_deref(), Some("Solar-Powered Assault Rifle"));
        assert_eq!(template.stat(420), Some(10), "AmmoType: SelfSupplied, not conventional bullets");
        assert_eq!(template.stat(212), Some(-1), "AmmoMag: no finite shots-per-clip limit");
        assert_eq!(template.stat(26), None, "current count comes from replication, not the static record");
        assert_eq!((template.stat(294), template.stat(210)), (Some(100), Some(150)));
        let mut arms = Armory::open(&dir);
        arms.wield(33512, 13948944, 6, Some(121569), &[(26, 0)]);
        let item = arms.slot_item(33512, 6).unwrap();
        assert_eq!((item.ammo, item.delay, item.recharge), (10, 100, 150));
    }

    /// Retained live subset: the self-supplied rifle kills a Beach Leet without reporting clip consumption.
    #[test]
    fn captured_solar_rifle_hits_report_no_clip_decrement() {
        let own = 33588;
        let target = 0xf99f3;
        let mut weapons = 0;
        let mut hits = Vec::new();
        let mut lifecycle = Vec::new();
        for line in include_str!("../../../../../docs/captures/zone_tab_rifle_fixr_ithaca.rec").lines() {
            let mut fields = line.split_whitespace();
            let _ms = fields.next().unwrap();
            let direction = fields.next().unwrap();
            let hex = fields.next().unwrap();
            if direction != "<" { continue; }
            let bytes: Vec<u8> = (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap()).collect();
            let (frame, _) = ao_net::frame::Frame::decode_with(&bytes, false).unwrap().unwrap();
            let m = ao_net::n3::decode(&frame).unwrap();
            // This is a selected subset, not evidence of absence throughout the full live session.
            assert_ne!(m.header.msg_type, 0x2651_5e61, "the retained subset contains no Reload message");
            match m.body {
                N3::Dynel(Dynel::WeaponItemFullUpdate(w)) => {
                    assert_eq!((m.header.target.instance, w.parent.instance, w.byte_71), (24763172, own, 6));
                    assert_eq!(w.stats.iter().find(|&&(id, _)| id == 23).map(|&(_, value)| value), Some(121569));
                    assert_eq!(w.stats.iter().find(|&&(id, _)| id == 26).map(|&(_, value)| value), Some(0));
                    weapons += 1;
                }
                N3::Misc(Misc::AttackInfo(hit)) if m.header.target.instance == own => {
                    assert_eq!((hit.slot, hit.value_20, hit.other.instance), (6, -1, target));
                    hits.push(hit.damage);
                    lifecycle.push("hit");
                }
                N3::Misc(Misc::StopFight(_)) if m.header.target.instance == own => lifecycle.push("stop"),
                N3::World(World::CharacterAction(action)) if m.header.target.instance == target && action.action == 0x63 => lifecycle.push("death"),
                N3::Misc(Misc::ToClientQuit) if m.header.target.instance == target => lifecycle.push("quit"),
                _ => {}
            }
        }
        assert_eq!(weapons, 2);
        assert_eq!(hits, [11, 2], "two server-confirmed slot-6 hits, not animation-note counts");
        assert_eq!(lifecycle, ["hit", "hit", "stop", "death", "quit"]);
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

    /// GC 0x10068969: bare hands while unarmed, both hands once either is wielded, the other hand added once it is too.
    #[test]
    fn attack_slots_follow_wielded_hands() {
        let mut a = player();
        assert_eq!(a.attack_slots(1), vec![0]);
        a.wield(1, 500, 6, None, &[(STAT_DAMAGE_TYPE, PROJECTILE)]);
        assert_eq!(a.attack_slots(1), vec![6]);
        a.wield(1, 501, 8, None, &[(STAT_DAMAGE_TYPE, ENERGY)]);
        assert_eq!(a.attack_slots(1), vec![8, 6]);
        a.unwield(500);
        assert_eq!(a.attack_slots(1), vec![8]);
    }

    /// `round_delay` reproduces `chat::item_info_combat::delay`'s fixture exactly (same formula, different input shape).
    #[test]
    fn round_delay_matches_the_item_info_dps_cell() {
        assert_eq!(round_delay(500, None, 1200, 100, false), 275.0);
        assert!((round_delay(500, None, 1800, 100, false) - (475.0 - 700.0 / 3.0)).abs() < 0.0001);
        assert_eq!(round_delay(500, None, 1200, 100, true), 100.0);
        assert_eq!(round_delay(500, Some(250), 1200, 100, true), 250.0);
    }

    #[test]
    fn item_round_seconds_sums_attack_and_recharge_in_hundredths() {
        let mut a = player();
        // AttackDelay 294 = 500, RechargeDelay 210 = 500, no caps, no InitiativeSkill (440 absent -> INVALID, both
        // lookups miss): the raw formula with `initiative = INVALID` behaves like an absurdly high initiative value,
        // so pin the item with an explicit InitiativeSkill the test stats table actually carries.
        a.wield(1, 500, 6, None, &[(0x126, 500), (0xd2, 500), (440, 100)]);
        let mut stats = HashMap::new();
        stats.insert(100, 1200);
        stats.insert(51, 0);
        let item = a.slot_item(1, 6).unwrap();
        assert_eq!(item.initiative_skill, 100);
        let seconds = item_round_seconds(item, &stats);
        // round_delay(500, None, 1200, 0, false) = 575 - 400 = 175 (= (500+75) - 1200*0.5/3*1 - 0); recharge uses
        // the same 1200 stat value (0..=1200) -> round_delay(500, None, 1200, 0, true) = 575 - 800/3 = 308.3333
        let expected = (round_delay(500, None, 1200, 0, false) + round_delay(500, None, 1200, 0, true)) * 0.01;
        assert!((seconds - expected).abs() < 0.0001);
    }
}
