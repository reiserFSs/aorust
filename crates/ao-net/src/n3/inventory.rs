//! Item movement between the inventory pages (drag and drop, equip / unequip, drop, delete) and the server's answers.
//! Evidence: Gamecode.dll (GC) decompilation, details in docs/gui.md §11.12.
//!
//! **Slots.** The character's inventory is one vector (`FUN_1002a41a` reads it, `FUN_1002a200` swaps two cells): `0..0x10` weapon page,
//! `0x10..0x20` armor (clothes) page, `0x20..0x30` implant page, `0x30..0x40` social page, `0x40..` the bag. `FUN_10046d8e(slot)` maps a
//! slot to the `Identity_t::kind` of an item in it ([`slot_kind`]), `FUN_10046d50(slot)` to the `InventoryId_e` page ([`Page`]).
//!
//! **Client senders** (all `n3InfoItemRemote_t` subclasses *constructed* by the sender with their class name, hence unregistered keys
//! `MapToKey(name)`; to-be-passed-on byte **0**, header identity = the control dynel `{0xC350, char_id}`, sent with `SendIIRToObservers`):
//! * `N3Msg_MoveItemToInventory(item, page, slot)` [GC 0x10027a30] -> `ClientMoveItemToInventoryIIR_t` (`FUN_1001534a`, vtable 0x1015735c,
//!   `Write` `FUN_1001531e`): `Identity item; i32 slot` ([`move_item_to_inventory`]).
//! * `N3Msg_ContainerAddItem(container, item)` [GC 0x100282a8] -> `ClientContainerAddItemIIR_t` (`FUN_1001517e`, vtable 0x101572dc,
//!   `Write` `FUN_10015151`): `Identity container; Identity item` ([`container_add_item`]).
//! * `N3Msg_DropItem(item, pos)` [GC 0x10027c94] -> `DropTemplateIIR_t` 0x3A243F41 (`FUN_10072d7d`, `Write` `FUN_10072cfb`):
//!   `Identity item; f32 x, y, z` ([`drop_item`]).
//! * `N3Msg_DeleteItem(item, extra)` [GC 0x1001c7db] -> `CharacterActionIIR_t` action `0x70`, `identity_a = item`, `identity_b = extra`
//!   ([`delete_item`]).
//!
//! **Server messages** (decoded by [`decode`]): `ContainerAddItemIIR_t` 0x47537A24 (vtable 0x1015d134, read `FUN_10038ef6`, apply `FUN_10038f67`),
//! `ItemReplacedIIR_c` 0x3A223B50 (0x10154d74, `FUN_10003edc` / `FUN_10003f4d`), `InventoryUpdatedIIR_t` 0x485E7202 (0x10161058, `FUN_10074e16` /
//! `FUN_10074e49`), `InventoryUpdateIIR_t` 0x4E536976 (0x10155f9c, `FUN_100a0310` / `FUN_100a040e`).

use super::N3Header;
use super::world::{AcgItem, InventoryEntry};
use crate::msg::Identity;
use crate::wire::{Reader, Writer};
use anyhow::{ensure, Result};

pub const CONTAINER_ADD_ITEM: u32 = 0x4753_7A24; // ContainerAddItemIIR_t
pub const ITEM_REPLACED: u32 = 0x3A22_3B50; // ItemReplacedIIR_c
pub const INVENTORY_UPDATED: u32 = 0x485E_7202; // InventoryUpdatedIIR_t
pub const INVENTORY_UPDATE: u32 = 0x4E53_6976; // InventoryUpdateIIR_t
pub const DROP_TEMPLATE: u32 = 0x3A24_3F41; // DropTemplateIIR_t
/// `MapToKey("ClientMoveItemToInventoryIIR_t")` (string at GC 0x10157380).
pub const CLIENT_MOVE_ITEM: u32 = 0x5469_373F;
/// `MapToKey("ClientContainerAddItemIIR_t")` (string at GC 0x10157300).
pub const CLIENT_CONTAINER_ADD_ITEM: u32 = 0x1F4D_5F7E;

/// `CharacterActionIIR_t` action of `N3Msg_DeleteItem`.
pub const ACTION_DELETE_ITEM: i32 = 0x70;
/// "Any free bag slot" destination of `ContainerAddItemIIR_t` (`FUN_10047d77`: `0x6f` -> first free slot from `0x40`).
pub const ANY_BAG_SLOT: i32 = 0x6f;

/// First bag slot; the bag of the own character has 30 of them (`SetMaxItemCount(0x1e)`, GUI 0x100cdb3a).
pub const BAG_FIRST: u32 = 0x40;
pub const BAG_SLOTS: u32 = 30;

/// `Identity_t::kind` of items by place (`FUN_10046d8e`, `N3Msg_IsItemPossibleToUnWear` GC 0x10026763, `N3Msg_UseItem` 0x100286f8).
pub const KIND_WEAPON_PAGE: i32 = 0x65;
pub const KIND_ARMOR_PAGE: i32 = 0x66;
pub const KIND_IMPLANT_PAGE: i32 = 0x67;
/// An item of the own bag (`instance` = the bag slot, `>= 0x40`; `FUN_1004b80a` rejects `< 0x40`).
pub const KIND_BAG: i32 = 0x68;
/// Bank item (`N3Msg_UseItem`: "Feedback_ItemCantBeUsedFromBank").
pub const KIND_BANK: i32 = 0x69;
/// An item inside a `Chest_t` (corpse 0xC76A, chest 0xC749, backpack): `instance = key * 0x10000 + slot`, `key` = the `word` of the container's last
/// `InventoryUpdateIIR_t` (chest `+0x1dc`; `FUN_1004af97` registers `key -> container identity`, `FUN_10048644` looks it up), `slot` = the index in the
/// container's inventory. Built by `FUN_1007de99` [GC] (the list `N3Msg_GetContainerInventoryList` returns for a chest) and read back by `FUN_1004b80a`,
/// `FUN_1004a7b3`, `FUN_10048829` (`instance >> 16` as signed short, `& 0xffff`).
pub const KIND_IN_CONTAINER: i32 = 0x6b;

/// The identity of the item in `slot` of the container whose last `InventoryUpdateIIR_t` carried `word` (`FUN_1007de99`).
pub fn container_item_identity(word: i32, slot: u32) -> Identity {
    Identity { kind: KIND_IN_CONTAINER, instance: word.wrapping_mul(0x10000).wrapping_add(slot as i32) }
}

/// An item of the social page.
pub const KIND_SOCIAL_PAGE: i32 = 0x73;

/// `InventoryId_e` (the `page` argument of `N3Msg_MoveItemToInventory`, `FUN_10046d50`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Bag = 1,
    Armor = 3,
    Weapon = 7,
    Implant = 8,
    Social = 0xe,
}

impl Page {
    /// `FUN_10046d50(slot)`.
    pub fn of_slot(slot: u32) -> Page {
        match slot {
            0..=0xf => Page::Weapon,
            0x10..=0x1f => Page::Armor,
            0x20..=0x2f => Page::Implant,
            0x30..=0x3f => Page::Social,
            _ => Page::Bag,
        }
    }

    /// Offset the page-relative slot gets before it is sent (`N3Msg_MoveItemToInventory`: armor `+0x10`, implants `+0x20`, social `+0x30`).
    pub fn base(self) -> u32 {
        match self {
            Page::Armor => 0x10,
            Page::Implant => 0x20,
            Page::Social => 0x30,
            Page::Weapon | Page::Bag => 0,
        }
    }
}

/// `FUN_10046d8e(slot)`: the identity kind of an item in `slot`.
pub fn slot_kind(slot: u32) -> i32 {
    match slot {
        0..=0xf => KIND_WEAPON_PAGE,
        0x10..=0x1f => KIND_ARMOR_PAGE,
        0x20..=0x2f => KIND_IMPLANT_PAGE,
        0x30..=0x3f => KIND_SOCIAL_PAGE,
        _ => KIND_BAG,
    }
}

/// The identity the client uses for the item in `slot` (`{slot_kind, slot}`).
pub fn item_identity(slot: u32) -> Identity {
    Identity { kind: slot_kind(slot), instance: slot as i32 }
}

fn header(w: &mut Writer, key: u32, char_id: i32) {
    w.u32(key);
    Identity { kind: super::outgoing::DYNEL_CHAR, instance: char_id }.write(w);
    w.u8(0);
}

/// `ClientMoveItemToInventoryIIR_t`: move `item` to absolute inventory slot `slot` (page offset already added).
pub fn move_item_to_inventory(char_id: i32, item: Identity, slot: i32) -> Vec<u8> {
    let mut w = Writer::default();
    header(&mut w, CLIENT_MOVE_ITEM, char_id);
    item.write(&mut w);
    w.i32(slot);
    w.0
}

/// `N3Msg_GetItem(item)` [GC 0x10027beb] -> `ClientGetItemIIR_t` (`FUN_10015258`, `Write` = one `Identity`): pick the world item up. The sender
/// first refuses with `Feedback_InventoryFull` when no bag slot `0x40..` is free (`FUN_1002a1b0(0x40) == -1`).
pub fn get_item(char_id: i32, item: Identity) -> Vec<u8> {
    let mut w = Writer::default();
    header(&mut w, super::outgoing::message_key("ClientGetItemIIR_t"), char_id);
    item.write(&mut w);
    w.0
}

/// `ClientContainerAddItemIIR_t`: add `item` to `container` (a backpack / chest / the own character).
pub fn container_add_item(char_id: i32, container: Identity, item: Identity) -> Vec<u8> {
    let mut w = Writer::default();
    header(&mut w, CLIENT_CONTAINER_ADD_ITEM, char_id);
    container.write(&mut w);
    item.write(&mut w);
    w.0
}

/// `DropTemplateIIR_t`: drop `item` at the playfield position `pos` (x, y up, z).
pub fn drop_item(char_id: i32, item: Identity, pos: [f32; 3]) -> Vec<u8> {
    let mut w = Writer::default();
    header(&mut w, DROP_TEMPLATE, char_id);
    item.write(&mut w);
    pos.iter().for_each(|&p| w.f32(p));
    w.0
}

/// `N3Msg_DeleteItem`: `CharacterActionIIR_t` action `0x70` with `identity_a = item`, `identity_b = extra`, `param` 0, empty text.
pub fn delete_item(char_id: i32, item: Identity, extra: Identity) -> Vec<u8> {
    super::action::character_action(char_id, &super::action::simple(ACTION_DELETE_ITEM, item, extra))
}

/// `CharacterActionIIR_t` actions of `N3Msg_SplitItem` [GC 0x1001bc8a] and `N3Msg_JoinItems` [GC 0x1001baa1].
pub const ACTION_SPLIT_ITEM: i32 = 0x34;
pub const ACTION_JOIN_ITEMS: i32 = 0x35;

/// `N3Msg_JoinItems(target, dragged)`: `identity_a` = the item that stays (the one under the drop, `FUN_100cbc21` passes it first),
/// `identity_b` = the dragged item that is merged into it.
pub fn join_items(char_id: i32, target: Identity, dragged: Identity) -> Vec<u8> {
    super::action::character_action(char_id, &super::action::simple(ACTION_JOIN_ITEMS, target, dragged))
}

/// `N3Msg_SplitItem(item, count)`: `identity_a` = item, `identity_b` = `{0, count}`.
pub fn split_item(char_id: i32, item: Identity, count: i32) -> Vec<u8> {
    super::action::character_action(char_id, &super::action::simple(ACTION_SPLIT_ITEM, item, Identity { kind: 0, instance: count }))
}

/// Body of `InventoryUpdateIIR_t` (`FUN_100a0310`): the contents of a container the server (re)sends. `capacity` (`+0x14` of the
/// `NewInventory_t` made by `FUN_1002ae7b`) and `kind` (`+0x24`) are the first two words; the element list is the one of `FullCharacterIIR_t`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventoryUpdate {
    pub capacity: i32,
    pub kind: i32,
    pub entries: Vec<InventoryEntry>,
    /// The container the list belongs to (`+0x1c`; a `Chest_t` or `SimpleChar_t` dynel).
    pub container: Identity,
    /// `+0x24`, passed to `FUN_1004775e` (the `0x6e` trade / overflow path) and stored in the chest (`+0x1dc`).
    pub word: i32,
    /// `+0x28`: clears the chest's flag `0x40` and re-announces it when set (`FUN_100a040e`).
    pub flag: bool,
}

/// A decoded server inventory message; the header target is the character the message is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InventoryMsg {
    /// `ContainerAddItemIIR_t`: wire order `item, container, slot` (`FUN_10038ef6` reads `+0x20`, `+0x18`, `+0x28`; apply `FUN_1004ad44(container, item, slot)`).
    ContainerAdd { item: Identity, container: Identity, slot: i32 },
    /// `ItemReplacedIIR_c`: the item at `slot` (< 0x40) became `new` (`FUN_1004cf08`; `old` is only compared).
    ItemReplaced { slot: i32, old: AcgItem, new: AcgItem },
    /// `InventoryUpdatedIIR_t`: an `i32` the GUI is told about (`FUN_10074e49` emits it on the global inventory signal for the own dynel).
    Updated(i32),
    Update(InventoryUpdate),
}

/// `GameData::operator>>(ACGItem_t)` [GameData 0x1000e9d7], as in [`InventoryEntry`].
fn read_acg(r: &mut Reader) -> Result<AcgItem> {
    let (low_id, high_id, mut level) = (r.i32()?, r.i32()?, r.i32()?);
    r.i32()?;
    if level > 0x1ff {
        level &= 0x1ff;
    }
    Ok(AcgItem { low_id, high_id: if high_id == 0 { low_id } else { high_id }, level })
}

pub fn decode(h: &N3Header, r: &mut Reader) -> Result<Option<InventoryMsg>> {
    Ok(match h.msg_type {
        CONTAINER_ADD_ITEM => {
            let (item, container, slot) = (Identity::read(r)?, Identity::read(r)?, r.i32()?);
            ensure!(r.remaining() == 0, "ContainerAddItemIIR_t: {} trailing bytes", r.remaining());
            Some(InventoryMsg::ContainerAdd { item, container, slot })
        }
        ITEM_REPLACED => {
            let slot = r.i32()?;
            let (old, new) = (read_acg(r)?, read_acg(r)?);
            ensure!(r.remaining() == 0, "ItemReplacedIIR_c: {} trailing bytes", r.remaining());
            Some(InventoryMsg::ItemReplaced { slot, old, new })
        }
        INVENTORY_UPDATED => {
            let v = r.i32()?;
            ensure!(r.remaining() == 0, "InventoryUpdatedIIR_t: {} trailing bytes", r.remaining());
            Some(InventoryMsg::Updated(v))
        }
        INVENTORY_UPDATE => {
            let (capacity, kind) = (r.i32()?, r.i32()?);
            let entries = super::world::read_inventory(r)?;
            let (container, word, flag) = (Identity::read(r)?, r.i32()?, r.i32()? != 0);
            ensure!(r.remaining() == 0, "InventoryUpdateIIR_t: {} trailing bytes", r.remaining());
            Some(InventoryMsg::Update(InventoryUpdate { capacity, kind, entries, container, word, flag }))
        }
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::n3::outgoing::message_key;

    fn hex(s: &str) -> Vec<u8> {
        let s: String = s.split_whitespace().collect();
        (0..s.len() / 2).map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap()).collect()
    }

    #[test]
    fn keys_are_the_class_name_hashes() {
        assert_eq!(message_key("ClientMoveItemToInventoryIIR_t"), CLIENT_MOVE_ITEM);
        assert_eq!(message_key("ClientContainerAddItemIIR_t"), CLIENT_CONTAINER_ADD_ITEM);
        assert_eq!(message_key("ContainerAddItemIIR_t"), CONTAINER_ADD_ITEM);
        assert_eq!(message_key("ItemReplacedIIR_c"), ITEM_REPLACED);
        assert_eq!(message_key("InventoryUpdatedIIR_t"), INVENTORY_UPDATED);
        assert_eq!(message_key("InventoryUpdateIIR_t"), INVENTORY_UPDATE);
        assert_eq!(message_key("DropTemplateIIR_t"), DROP_TEMPLATE);
    }

    #[test]
    fn get_item_is_header_plus_one_identity() {
        let p = get_item(0x6584, Identity { kind: 0xCF1B, instance: 0x27E79 });
        assert_eq!(p.len(), 4 + 8 + 1 + 8);
        assert_eq!(p[..4], message_key("ClientGetItemIIR_t").to_be_bytes());
        assert_eq!(p[4..], [0, 0, 0xc3, 0x50, 0, 0, 0x65, 0x84, 0, 0, 0, 0xcf, 0x1b, 0, 2, 0x7e, 0x79][..]);
    }

    #[test]
    fn slot_tables_follow_fun_10046d8e_and_fun_10046d50() {
        let kinds: Vec<_> = [0, 0xf, 0x10, 0x1f, 0x20, 0x2f, 0x30, 0x3f, 0x40, 0x5d].iter().map(|&s| slot_kind(s)).collect();
        assert_eq!(kinds, [0x65, 0x65, 0x66, 0x66, 0x67, 0x67, 0x73, 0x73, 0x68, 0x68]);
        let pages: Vec<_> = [0, 0xf, 0x10, 0x1f, 0x20, 0x2f, 0x30, 0x3f, 0x40, 0x5d].iter().map(|&s| Page::of_slot(s) as i32).collect();
        assert_eq!(pages, [7, 7, 3, 3, 8, 8, 0xe, 0xe, 1, 1]);
        assert_eq!(item_identity(0x45), Identity { kind: 0x68, instance: 0x45 });
        assert_eq!(Page::Armor.base() + 5, 0x15);
        assert_eq!(Page::Social.base() + 3, 0x33);
    }

    #[test]
    fn client_message_bytes() {
        // header: key, {0xC350, 0x6584}, pass-on 0
        let b = move_item_to_inventory(0x6584, item_identity(0x12), 0x41);
        assert_eq!(b, hex("5469373f 0000c350 00006584 00  00000066 00000012 00000041"));
        let b = container_add_item(0x6584, Identity { kind: 0xc350, instance: 0x6584 }, item_identity(0x40));
        assert_eq!(b, hex("1f4d5f7e 0000c350 00006584 00  0000c350 00006584 00000068 00000040"));
        let b = drop_item(0x6584, item_identity(0x41), [1.0, 2.0, -3.0]);
        assert_eq!(b, hex("3a243f41 0000c350 00006584 00  00000068 00000041 3f800000 40000000 c0400000"));
        let b = delete_item(0x6584, item_identity(0x42), Identity { kind: 0, instance: 0 });
        assert_eq!(b, hex("5e477770 0000c350 00006584 00  00000070 00000000 00000068 00000042 00000000 00000000 0000"));
        let b = join_items(0x6584, item_identity(0x42), item_identity(0x43));
        assert_eq!(b, hex("5e477770 0000c350 00006584 00  00000035 00000000 00000068 00000042 00000068 00000043 0000"));
        let b = split_item(0x6584, item_identity(0x42), 5);
        assert_eq!(b, hex("5e477770 0000c350 00006584 00  00000034 00000000 00000068 00000042 00000000 00000005 0000"));
    }

    fn parse(b: &[u8]) -> Result<Option<InventoryMsg>> {
        let (h, mut r) = N3Header::parse(b)?;
        decode(&h, &mut r)
    }

    #[test]
    fn server_messages_decode() {
        // wire order item, container, slot
        let b = hex("47537a24 0000c350 00006584 00  00000068 00000041 0000c350 00006584 00000042");
        assert_eq!(
            parse(&b).unwrap(),
            Some(InventoryMsg::ContainerAdd { item: item_identity(0x41), container: Identity { kind: 0xc350, instance: 0x6584 }, slot: 0x42 })
        );
        let b = hex(
            "3a223b50 0000c350 00006584 00  00000011  00000010 00000010 00000019 00000000  00000020 00000000 000002ff 00000000",
        );
        assert_eq!(
            parse(&b).unwrap(),
            Some(InventoryMsg::ItemReplaced { slot: 0x11, old: AcgItem { low_id: 16, high_id: 16, level: 25 }, new: AcgItem { low_id: 32, high_id: 32, level: 0xff } })
        );
        assert_eq!(parse(&hex("485e7202 0000c350 00006584 00 00000005")).unwrap(), Some(InventoryMsg::Updated(5)));
        assert!(parse(&hex("485e7202 0000c350 00006584 00 00000005")[..16]).is_err());
        // InventoryUpdateIIR_t: capacity, kind, list (size word (n+1)*0x3f1: one element), container, word, flag
        let b = hex(
            "4e536976 0000c750 00000001 00  00000015 00000002  000007e2  00000041 0001 0003 00000068 00000041  00000010 00000011 00000019 00000000  0000c750 00000001 00000007 00000001",
        );
        let Some(InventoryMsg::Update(u)) = parse(&b).unwrap() else { panic!() };
        assert_eq!((u.capacity, u.kind, u.word, u.flag), (0x15, 2, 7, true));
        assert_eq!(u.container, Identity { kind: 0xc750, instance: 1 });
        assert_eq!(u.entries.len(), 1);
        assert_eq!((u.entries[0].slot, u.entries[0].item.low_id, u.entries[0].item.high_id), (0x41, 0x10, 0x11));
        // every truncation is an error, trailing data too
        for n in 13..b.len() {
            assert!(parse(&b[..n]).is_err(), "{n}");
        }
        assert!(parse(&[b.clone(), vec![0]].concat()).is_err());
        assert_eq!(parse(&hex("00000001 0000c350 00000001 00")).unwrap(), None);
    }

    /// Own kill on Ithaca (`docs/captures/zone_loot_own_kill_ithaca.rec`): the corpse's `InventoryUpdateIIR_t` and the take the original client sends
    /// (`FUN_100ca1e7` -> `MoveItemToInventory(item)` = `N3Msg_MoveItemToInventory(item, 1, 0x6f)`, the item built by `FUN_1007de99`).
    #[test]
    fn corpse_item_identity_is_kind_6b_with_the_update_word_as_key() {
        let b = hex(
            "4e536976 0000c350 0000830e 01 00000015 00000002 00000bd3
             00000000 00a1 0001 09000001 0044b2c5 0000a690 0000a690 00000001 00000000
             00000001 00a1 0001 09000001 0044b2c6 0003ca03 0003ca03 00000001 00000000
             0000c76a 00000e22 00000070 00000001",
        );
        let Some(InventoryMsg::Update(u)) = parse(&b).unwrap() else { panic!() };
        assert_eq!((u.container, u.word, u.flag), (Identity { kind: 0xc76a, instance: 0xe22 }, 0x70, true));
        // the entry's own `id` (kind 0x09000001) is NOT what the client sends back
        assert_eq!(u.entries[0].id, Identity { kind: 0x0900_0001, instance: 0x44b2c5 });
        let ids: Vec<_> = u.entries.iter().map(|e| container_item_identity(u.word, e.slot)).collect();
        assert_eq!(ids, [Identity { kind: 0x6b, instance: 0x0070_0000 }, Identity { kind: 0x6b, instance: 0x0070_0001 }]);
        let take = move_item_to_inventory(0x830e, ids[1], ANY_BAG_SLOT);
        assert_eq!(take, hex("5469373f 0000c350 0000830e 00 0000006b 00700001 0000006f"));
        // the key is a signed short (`*(short *)(id + 6)`)
        assert_eq!(container_item_identity(-1, 2).instance >> 16, -1);
    }
}
