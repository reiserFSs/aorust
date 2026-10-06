//! Drag and drop of items between the wear window and the inventory (docs/gui.md §11.12). The rules are the client's:
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

/// `N3Msg_IsItemPossibleToWear` without the checks that need state we do not have (equipment lock `FUN_1002654d`, the wearer's skill requirements
/// `vtable+0x5c`, implant access `FUN_10044b6e(0x10)`; the server decides those): `ItemClass` in {1, 2, 3, 5}, the `Placement` bit of the slot id, and the
/// deck slot rule of `FUN_10047be5` for weapons.
/// UNRESOLVED: the decompilation drops the `class` argument (`param_3`) the callers pass, so whether the original also compares it to the item's class is
/// unknown; we do (weapon tab: class 1, armor: 2, implants: 3 or 5, social: any wearable class), the server validates anyway.
pub fn can_wear(info: &Info, tab: usize, id: u32, deck: i32) -> bool {
    let Some(class) = info.stat(STAT_ITEM_CLASS) else { return false };
    if !matches!(class, 1 | 2 | 3 | 5) || !(1..=16).contains(&id) {
        return false;
    }
    let on_tab = match tab {
        0 => class == 1,
        1 => class == 2,
        2 => matches!(class, 3 | 5),
        3 => true,
        _ => false,
    };
    let fits = info.stat(STAT_PLACEMENT).is_some_and(|p| p & (1 << id) != 0);
    // `FUN_10047be5(class 1, slot, ..)`: the deck slots 9..14 need `ComputerLiteracy`/deck (`GetSkill(0x2d) > slot - 9`)
    let deck_ok = !(tab == 0 && class == 1 && (9..15).contains(&id)) || deck > id as i32 - 9;
    on_tab && fits && deck_ok
}

/// Where a double click wears a bag item (`N3Msg_UseItem`): `Can & 8 == 0`, class 1 -> weapon page, 2 -> armor page (`+0x10`), 3 / 5 -> implants
/// (`+0x20`), slot id = `DefaultPos`; returns (tab, slot id).
pub fn default_wear(info: &Info) -> Option<(usize, u32)> {
    if info.stat(STAT_CAN).unwrap_or(0) & 8 != 0 {
        return None;
    }
    let id = info.stat(STAT_DEFAULT_POS)?;
    let tab = match info.stat(STAT_ITEM_CLASS)? {
        1 => 0,
        2 => 1,
        3 | 5 => 2,
        _ => return None,
    };
    (1..=16).contains(&id).then_some((tab, id as u32))
}

/// `N3Msg_JoinItems` preconditions we can check: same template, both stackable (`Can & 0x200`) and the same quality level.
pub fn can_join(a: &Info, b: &Info, same_template: bool, level_a: i32, level_b: i32) -> bool {
    same_template && a.stat(STAT_CAN).unwrap_or(0) & 0x200 != 0 && b.stat(STAT_CAN).unwrap_or(0) & 0x200 != 0 && level_a == level_b
}

/// What a finished drag asks for.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// `ClientMoveItemToInventoryIIR_t` (equip, unequip, to the bag).
    Move { item: Identity, slot: i32 },
    /// `CharacterActionIIR_t` `0x35`: stack `from` onto `onto`.
    Join { from: Identity, onto: Identity },
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
}

pub struct Drag {
    pub slot: u32,
    pub ghost: WindowId,
}

/// Double click time (`GUIColors`/input config value not located: UNRESOLVED, the usual 0.4 s).
pub const DOUBLE_CLICK: f32 = 0.4;

#[cfg(test)]
mod tests {
    use super::*;

    fn info(stats: &[(u32, i32)]) -> Info {
        Info { name: String::new(), count: 1, icon: None, stats: stats.to_vec() }
    }

    #[test]
    fn wearing_follows_class_and_placement() {
        let helmet = info(&[(STAT_ITEM_CLASS, 2), (STAT_PLACEMENT, 1 << 2), (STAT_DEFAULT_POS, 2)]);
        assert!(can_wear(&helmet, 1, 2, 0));
        assert!(!can_wear(&helmet, 1, 3, 0), "wrong slot");
        assert!(!can_wear(&helmet, 0, 2, 0), "armor on the weapon tab");
        assert!(can_wear(&helmet, 3, 2, 0), "social mirrors the clothes slots");
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
        let s = info(&[(STAT_CAN, 0x200)]);
        assert!(can_join(&s, &s, true, 5, 5));
        assert!(!can_join(&s, &s, true, 5, 6));
        assert!(!can_join(&s, &s, false, 5, 5));
        assert!(!can_join(&s, &info(&[]), true, 5, 5));
    }
}
