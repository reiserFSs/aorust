//! Nano programs: the own program list, the `CharacterActionIIR_t` senders of the Programs / NCU windows and the server's list updates.
//! Evidence: docs/gui.md §11.12 (GUI.dll `NanoView_c` `FUN_100d496c`, `NCUView_c` `FUN_100d6dd6`; Gamecode.dll `N3Msg_CastNanoSpell` 0x1001b54b,
//! `N3Msg_DeleteNano` 0x1001b652, `N3Msg_RemoveBuff` 0x1001be60, the apply switch `FUN_1005d0d8`).

use super::action::{character_action, simple};
use super::world::CharacterAction;
use crate::msg::Identity;

/// `Identity_t` kind of a nano program (the instance is the id of the rdb 1040005 record; `FUN_100d4783` builds `{0xcf1b, id}`).
pub const NANO_KIND: i32 = 0xCF1B;

/// `CharacterActionIIR_t` action ids of the nano windows.
pub mod action {
    /// `N3Msg_CastNanoSpell(nano, target)` [GC 0x1001b54b]: `FUN_1007253f(hdr, a = target, param 0, 0x13, b = nano, "")`.
    pub const CAST: i32 = 0x13;
    /// `N3Msg_RemoveBuff(nano)` [GC 0x1001be60]: `FUN_1007253f(hdr, a = {0,0}, 0, 0x41, b = nano, "")`.
    pub const REMOVE_BUFF: i32 = 0x41;
    /// `N3Msg_DeleteNano(nano)` [GC 0x1001b652]: `FUN_1007253f(hdr, a = nano, 0, 0xd3, b = {0,0}, "")`.
    pub const DELETE: i32 = 0xd3;
    /// Server -> client, apply case 0x56 [GC 0x1005e7e6]: `FUN_1004fbbc(identity_b.instance)` appends the nano to the own program list when its record
    /// exists and it is not in the list yet (then a feedback line is printed). The action's own name is not in the client (no enum symbols).
    pub const LEARNED: i32 = 0xcc;
    /// Server -> client, apply case 0x57 [GC 0x1005e846]: `FUN_1004f415(identity_b.instance)` removes the nano from the own program list.
    pub const FORGOTTEN: i32 = 0xcd;
}

/// The identity of nano program `id`.
pub fn nano(id: i32) -> Identity {
    Identity { kind: NANO_KIND, instance: id }
}

/// `N3Msg_CastNanoSpell(nano, target)` of character `char_id` (action 0x13; the Programs window's double click, the hotbar's nano slot).
pub fn cast_nano(char_id: i32, nano_id: i32, target: Identity) -> Vec<u8> {
    character_action(char_id, &simple(action::CAST, target, nano(nano_id)))
}

/// `N3Msg_RemoveBuff(nano)`: the NCU window's double click on an active timed nano.
pub fn remove_buff(char_id: i32, nano_id: i32) -> Vec<u8> {
    character_action(char_id, &simple(action::REMOVE_BUFF, Identity::default(), nano(nano_id)))
}

/// `N3Msg_DeleteNano(nano)`: forgets a nano program.
pub fn delete_nano(char_id: i32, nano_id: i32) -> Vec<u8> {
    character_action(char_id, &simple(action::DELETE, nano(nano_id), Identity::default()))
}

/// A change of the own program list sent by the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListChange {
    Learned(i32),
    Forgotten(i32),
}

/// The program-list change a relayed `CharacterActionIIR_t` of the own character carries (`identity_b.instance` is the nano id).
pub fn list_change(a: &CharacterAction) -> Option<ListChange> {
    match a.action {
        action::LEARNED => Some(ListChange::Learned(a.identity_b.instance)),
        action::FORGOTTEN => Some(ListChange::Forgotten(a.identity_b.instance)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::n3::action::parse_character_action;

    fn hex(s: &str) -> Vec<u8> {
        let s: String = s.split_whitespace().collect();
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    /// Layouts from the disassembly of the three senders (argument order of `FUN_1007253f`: header, identity a, param, action, identity b, text).
    #[test]
    fn sender_bytes() {
        let target = Identity { kind: 0xC350, instance: 0x6584 };
        assert_eq!(
            cast_nano(0x6584, 163449, target),
            hex("5e477770 0000c350 00006584 00  00000013 00000000 0000c350 00006584 0000cf1b 00027e79 0000")
        );
        assert_eq!(remove_buff(0x6584, 163449), hex("5e477770 0000c350 00006584 00  00000041 00000000 00000000 00000000 0000cf1b 00027e79 0000"));
        assert_eq!(delete_nano(0x6584, 163449), hex("5e477770 0000c350 00006584 00  000000d3 00000000 0000cf1b 00027e79 00000000 00000000 0000"));
    }

    #[test]
    fn list_changes_are_read_from_identity_b() {
        let learned = |a: i32| {
            let (_, ca) = parse_character_action(&character_action(1, &simple(a, Identity::default(), nano(77)))).unwrap();
            list_change(&ca)
        };
        assert_eq!(learned(action::LEARNED), Some(ListChange::Learned(77)));
        assert_eq!(learned(action::FORGOTTEN), Some(ListChange::Forgotten(77)));
        assert_eq!(learned(action::CAST), None);
    }
}
