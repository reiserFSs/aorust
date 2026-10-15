//! Drag and drop of items between the wear window and the inventory (docs/gui.md §11.11). The rules are the client's:
//! `InventoryView_c` slots `FUN_100cbab4` (drag begin), `FUN_100cbc21` (drop on the bag), `FUN_100ca1e7` (double click), `WearView_c`
//! drop `FUN_100e1920`, and the checks of `N3Msg_IsItemPossibleToWear` [GC 0x10026adb], `N3Msg_IsItemPossibleToUnWear` [GC 0x10026763],
//! `N3Msg_MoveItemToInventory` [GC 0x10027a30], `N3Msg_UseItem` [GC 0x100286f8]. Nothing changes locally: the server answers a move with a
//! `ContainerAddItemIIR_t` ([`ao_net::n3::inventory`]) that updates `Zone::inventory`.

use super::items::Info;
use ao_gui::WindowId;
use ao_net::msg::Identity;
use ao_net::n3::inventory::Page;

/// `Can` (30): bit `0x8` of it sends `UseItem` on a bag item down the "use" path instead of the "wear" path (`FUN_100286f8`); `0x200` = stackable
/// (`N3Msg_JoinItems`); `0x1000000` = cannot be split.
pub const STAT_CAN: u32 = 30;
/// `ItemClass` (76): 1 weapon, 2 armor / clothes, 3 implant (5 also goes to the implants in `UseItem`), 6 social (`MoveItemToInventory` passes 6 for the social page).
pub const STAT_ITEM_CLASS: u32 = 76;
/// `DefaultPos` (88): the slot id a double click wears the item into (`GetSkill(item, 0x58)` in `N3Msg_UseItem`).
pub const STAT_DEFAULT_POS: u32 = 88;
/// `Placement` (298): bit `1 << slot id` = the item fits slot `id` of its page (`FUN_1008010f`, `slot - 1 < 0x10`).
pub const STAT_PLACEMENT: u32 = 298;

/// A cell of one of the two windows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    /// Wear window tab `tab`, slot id `id` (1..15, the cell's id) = inventory slot `slot`.
    Wear { tab: usize, id: u32, slot: u32 },
    /// Inventory grid cell (column, row).
    Bag { cell: (usize, usize) },
}

/// `InventoryId_e` and the item class the wear tabs pass to `N3Msg_IsItemPossibleToWear` (`FUN_100e1920`: 1 / 2 / 3 / 6).
pub fn tab_page(tab: usize) -> Option<(Page, i32)> {
    match tab {
        0 => Some((Page::Weapon, 1)),
        1 => Some((Page::Armor, 2)),
        2 => Some((Page::Implant, 3)),
        3 => Some((Page::Social, 6)),
        _ => None,
    }
}

/// Structural portion of `N3Msg_IsItemPossibleToWear`; `Items::can_equip`
/// additionally checks ACG-resolved wearer requirements and implant access.
/// Owned-item class matching is explicit in GC `1004959a`.
#[cfg(test)]
pub fn can_wear(info: &Info, tab: usize, id: u32, deck: i32) -> bool {
    can_wear_stats(&info.stats, tab, id, deck)
}

pub(super) fn can_wear_stats(stats: &[(u32, i32)], tab: usize, id: u32, deck: i32) -> bool {
    let stat = |id| stats.iter().find(|s| s.0 == id).map(|s| s.1);
    let Some(class) = stat(STAT_ITEM_CLASS) else { return false };
    if !matches!(class, 1 | 2 | 3 | 5) || !(1..=16).contains(&id) {
        return false;
    }
    let on_tab = match tab {
        0 => class == 1,
        1 => class == 2,
        2 => matches!(class, 3 | 5),
        3 => matches!(class, 1 | 2),
        _ => false,
    };
    let placement = stat(STAT_PLACEMENT).unwrap_or(0);
    // Social armor loses hand bits; weapons map left/right hand to fingers.
    let placement = if tab != 3 { placement } else if class == 1 {
        (if placement & 0x100 != 0 { 0x8000 } else { 0 }) | (if placement & 0x40 != 0 { 0x2000 } else { 0 })
    } else { placement & 0xffff5fff_u32 as i32 };
    let fits = placement & (1 << id) != 0;
    // `FUN_10047be5(class 1, slot, ..)`: the deck slots 9..14 need `ComputerLiteracy`/deck (`GetSkill(0x2d) > slot - 9`)
    let deck_ok = !(tab == 0 && class == 1 && (9..15).contains(&id)) || deck > id as i32 - 9;
    on_tab && fits && deck_ok
}

/// Where a double click wears a bag item (`N3Msg_UseItem`): `Can & 8 == 0`, class 1 -> weapon page, 2 -> armor page (`+0x10`), 3 / 5 -> implants
/// (`+0x20`), slot id = `DefaultPos`; returns (tab, slot id).
#[cfg(test)]
pub fn default_wear(info: &Info) -> Option<(usize, u32)> {
    default_wear_stats(&info.stats)
}

pub(super) fn default_wear_stats(stats: &[(u32, i32)]) -> Option<(usize, u32)> {
    let stat = |id| stats.iter().find(|s| s.0 == id).map(|s| s.1);
    if stat(STAT_CAN).unwrap_or(0) & 8 != 0 {
        return None;
    }
    let id = stat(STAT_DEFAULT_POS)?;
    let tab = match stat(STAT_ITEM_CLASS)? {
        1 => 0,
        2 => 1,
        3 | 5 => 2,
        _ => return None,
    };
    (1..=16).contains(&id).then_some((tab, id as u32))
}

/// `N3Msg_JoinItems` preconditions we can check: same template, both stackable (`Can & 0x200`) and the same quality level.
pub fn can_join(can_a: i32, can_b: i32, same_template: bool, level_a: i32, level_b: i32) -> bool {
    same_template && can_a & 0x200 != 0 && can_b & 0x200 != 0 && level_a == level_b
}

/// GC 1001bc8a: the count must leave at least one item, and Can bit 0x1000000 forbids splitting.
pub fn can_split(can: i32, total: i32, count: i32) -> bool {
    can & 0x1000000 == 0 && count > 0 && count < total
}

/// What a finished drag asks for.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// `ClientMoveItemToInventoryIIR_t` (equip, unequip, to the bag).
    Move { item: Identity, slot: i32 },
    /// `CharacterActionIIR_t` `0x35`: stack `from` onto `onto`.
    Join { from: Identity, onto: Identity },
    /// `CharacterActionIIR_t` `0x34`: split off `count` units.
    Split { item: Identity, count: i32 },
    /// Only the client side grid changes.
    MoveCell { from: (usize, usize), to: (usize, usize) },
    /// `DropTemplateIIR_t` at the player's position.
    Drop { item: Identity },
    /// A bag item is used (`GenericCmd_t`, `N3Msg_UseItem`).
    Use { item: Identity },
    /// A bag item released over a world object (`FUN_100cb081` [GUI]): `N3Msg_UseItemOnCharacter` (`GenericCmd_t` 0x20) for a character,
    /// `N3Msg_UseItemOnItem` (`GenericCmd_t` 5) for any other object.
    UseOn { item: Identity, target: Identity },
}

/// `EquipDelay` (stat 0xd3 / 211, `stat_names.txt`). `N3Msg_MoveItemToInventory` [GC 0x10027a30] calls `FUN_100474a9`/`FUN_1004aed0`
/// (docs/zone/avatar.md §"Clothing is not a weapon notification"): it reads this stat of the moved item and, when wearing into an
/// occupied weapon/armor/implant slot, of the item already there, sums them (centiseconds) and registers the client-local item-action
/// timer `FUN_10047fde`/`FUN_1003d7ce` (the same mechanism `docs/zone/interact.md` §8.8 documents for timed item use).
pub const STAT_EQUIP_DELAY: u32 = 211;

/// `FUN_100474a9` [GC]: whether moving an item whose own current slot is `item_slot` to absolute `slot` registers the wear-delay timer at
/// all (weapon/armor/implant page, `0..0x30`, either as the destination (wearing) or the item's own current slot (unwearing); the social
/// page and bag-to-bag moves are never timed), and if so `FUN_1004aed0`'s summed duration in seconds (the moved item's own `EquipDelay`,
/// plus the occupant's when wearing over one).
#[cfg(test)]
pub fn wear_delay(moved: &Info, item_slot: i32, slot: i32, occupant: Option<&Info>) -> Option<f32> {
    wear_delay_values(moved.stat(STAT_EQUIP_DELAY).unwrap_or(0), item_slot, slot, occupant.and_then(|o| o.stat(STAT_EQUIP_DELAY)).unwrap_or(0))
}

pub(super) fn wear_delay_values(moved: i32, item_slot: i32, slot: i32, occupant: i32) -> Option<f32> {
    let worn_page = |s: i32| (0..0x30).contains(&s);
    if !worn_page(slot) && !worn_page(item_slot) {
        return None;
    }
    let mut cs = i64::from(moved.max(0));
    if worn_page(slot) {
        cs += i64::from(occupant.max(0));
    }
    (cs > 0 && cs != 1_234_567_890).then(|| cs as f32 / 100.0)
}

/// Native kind6 countdown, gating later moves while active (GC1004b9c9).
/// The server's `ContainerAddItemIIR_t` still owns actual inventory changes.
#[derive(Default)]
pub struct WearTimers {
    pending: Option<(f32, f32)>,
}

impl WearTimers {
    pub fn start(&mut self, seconds: f32) {
        // 10047fde refuses a second kind6 entry until the first is removed.
        self.pending.get_or_insert((seconds, seconds));
    }

    pub fn tick(&mut self, dt: f32) {
        if let Some((remaining, _)) = self.pending.as_mut() {
            *remaining -= dt;
            if *remaining <= 0.0 { self.pending = None; }
        }
    }

    /// Native kind6 is a singleton. `(remaining / total, total)` also feeds
    /// the optional timer bar (minimum visible duration is a GUI policy).
    pub fn active(&self) -> Option<(f32, f32)> {
        self.pending.map(|(remaining, total)| ((remaining / total).clamp(0.0, 1.0), total))
    }
}

/// Press / drag state.
#[derive(Default)]
pub struct Dnd {
    pub press: Option<(Place, f32, f32)>,
    pub drag: Option<Drag>,
    /// Drop target under the pointer while dragging (`true` = the drop would be accepted); repaints the windows.
    pub hover: Option<(Place, bool)>,
    /// Last click for the double click: where, and the [`Dnd::clock`] time.
    pub last_click: Option<(Place, f32)>,
    pub clock: f32,
    /// Items released over a window that is not an inventory place: `(slot, x, y)`, drained by [`super::HudStats::take_drops`].
    pub dropped: Vec<(u32, f32, f32)>,
    /// The world object under the pointer when the button went up (`InputConfig_t+0xb0`, set by `Play::interact_mouse`); `None` = the ground.
    pub world_under: Option<Identity>,
    /// GUI 10040f22/100412a0: Ctrl-left horizontal count selection before starting a split drag.
    pub split: Option<Split>,
    pub pointer_warp: Option<(f32, f32)>,
}

pub struct Drag {
    pub slot: u32,
    pub ghost: WindowId,
    pub count: i32,
}

pub struct Split {
    pub place: Place,
    pub anchor: (f32, f32),
    pub count: i32,
    pub popup: Option<WindowId>,
}

/// Double click time (`GUIColors`/input config value not located: UNRESOLVED, the usual 0.4 s).
pub const DOUBLE_CLICK: f32 = 0.4;

#[cfg(test)]
mod tests {
    use super::*;

    fn info(stats: &[(u32, i32)]) -> Info {
        Info { name: String::new(), icon: None, stats: stats.to_vec() }
    }

    #[test]
    fn wearing_follows_class_and_placement() {
        let helmet = info(&[(STAT_ITEM_CLASS, 2), (STAT_PLACEMENT, 1 << 2), (STAT_DEFAULT_POS, 2)]);
        assert!(can_wear(&helmet, 1, 2, 0));
        assert!(!can_wear(&helmet, 1, 3, 0), "wrong slot");
        assert!(!can_wear(&helmet, 0, 2, 0), "armor on the weapon tab");
        assert!(can_wear(&helmet, 3, 2, 0), "social mirrors the clothes slots");
        let social_weapon = info(&[(STAT_ITEM_CLASS, 1), (STAT_PLACEMENT, 0x140)]);
        assert!(can_wear(&social_weapon, 3, 13, 0));
        assert!(can_wear(&social_weapon, 3, 15, 0));
        assert!(!can_wear(&social_weapon, 3, 6, 0), "social weapon maps hands to fingers");
        let implant = info(&[(STAT_ITEM_CLASS, 3), (STAT_PLACEMENT, 1 << 2)]);
        assert!(!can_wear(&implant, 3, 2, 0), "social accepts only weapons and clothing");
        assert_eq!(default_wear(&helmet), Some((1, 2)));
        let deck_weapon = info(&[(STAT_ITEM_CLASS, 1), (STAT_PLACEMENT, 1 << 10)]);
        assert!(!can_wear(&deck_weapon, 0, 10, 0));
        assert!(can_wear(&deck_weapon, 0, 10, 2));
        assert!(!can_wear(&info(&[(STAT_ITEM_CLASS, 4), (STAT_PLACEMENT, 0xffff)]), 1, 1, 9), "class 4 never wears");
        assert!(!can_wear(&info(&[]), 1, 1, 9));
        // `Can & 8` items are used, not worn
        assert_eq!(default_wear(&info(&[(STAT_CAN, 8), (STAT_ITEM_CLASS, 2), (STAT_DEFAULT_POS, 2)])), None);
        assert_eq!(default_wear(&info(&[(STAT_ITEM_CLASS, 5), (STAT_DEFAULT_POS, 4)])), Some((2, 4)));
    }

    #[test]
    fn tabs_map_to_pages() {
        assert_eq!(tab_page(1), Some((Page::Armor, 2)));
        assert_eq!(tab_page(2).map(|p| p.0.base()), Some(0x20));
        assert_eq!(tab_page(4), None);
    }

    #[test]
    fn join_needs_stackable_same_level() {
        assert!(can_join(0x200, 0x200, true, 5, 5));
        assert!(!can_join(0x200, 0x200, true, 5, 6));
        assert!(!can_join(0x200, 0x200, false, 5, 5));
        assert!(!can_join(0x200, 0, true, 5, 5));
    }

    #[test]
    fn split_count_must_leave_one_and_respect_cannot_split() {
        assert!(can_split(0x200, 10, 4));
        for count in [-1, 0, 10, 11] { assert!(!can_split(0x200, 10, count)); }
        assert!(!can_split(0x1000200, 10, 4));
    }

    /// `N3Msg_MoveItemToInventory`'s `FUN_100474a9`/`FUN_1004aed0`: weapon/armor/implant wear sums the moved item's `EquipDelay` with the
    /// slot's current occupant (a weapon swap); unwearing out of those pages times only the unworn item; the social page and bag moves
    /// (both ends `>= 0x30`) are never timed.
    #[test]
    fn wear_delay_follows_gamecodes_summed_centiseconds() {
        let belt = info(&[(STAT_EQUIP_DELAY, 2000)]);
        let old_weapon = info(&[(STAT_EQUIP_DELAY, 500)]);
        // wearing into an empty armor slot (implants: 0x20..0x2f, NCU belt page): only the moved item's delay
        assert_eq!(wear_delay(&belt, 0x40, 0x22, None), Some(20.0));
        // wearing over an occupied weapon slot (0..0xf): both items' delays sum
        assert_eq!(wear_delay(&old_weapon, 0x40, 3, Some(&belt)), Some(25.0));
        // unwearing (source < 0x30, dest the "any free bag slot" >= 0x30): only the unworn item's delay, no occupant lookup
        assert_eq!(wear_delay(&belt, 0x22, 0x6f, Some(&old_weapon)), Some(20.0));
        // social page (0x30..0x3f) and plain bag-to-bag moves: never timed
        assert_eq!(wear_delay(&belt, 0x40, 0x31, None), None);
        assert_eq!(wear_delay(&belt, 0x40, 0x41, None), None);
        // no EquipDelay stat at all: no timer
        assert_eq!(wear_delay(&info(&[]), 0x40, 3, None), None);
    }

    #[test]
    fn wear_timers_count_down_and_clear() {
        let mut t = WearTimers::default();
        assert_eq!(t.active(), None);
        t.start(2.0);
        assert_eq!(t.active(), Some((1.0, 2.0)));
        t.tick(1.5);
        assert_eq!(t.active(), Some((0.25, 2.0)));
        // A second registration cannot reset an already-active kind6 timer.
        t.start(2.0);
        assert_eq!(t.active(), Some((0.25, 2.0)));
        t.tick(0.5);
        assert_eq!(t.active(), None, "elapsed timers clear themselves");
    }
}
