//! Player-to-player trade (`TradeIIR_t`, Gamecode.dll): the one message both directions of the trade window use. Layouts and addresses: docs/zone/interact.md
//! ("Player trade").
//!
//! `TradeIIR_t` is an `n3InfoItemRemote_t` made by `FUN_1007a733 [GC]` (class name string, key `MapToKey("TradeIIR_t")` = `36284F6E`); the
//! "to be passed on" byte is **0** (`this[0xc] = 0` after the ctor). Body (`Write` `FUN_1007a674`, `Read` `FUN_1007a60b`): `i32 2` (the
//! version tag `DAT_101c069c`; a different value fails the read), `i8 op`, `Identity a` (`+0x1c`), `Identity b` (`+0x24`); an op above 10 fails the read.
//! The ops are the cases of the dispatcher `FUN_100674ab [GC]`.

use super::N3Header;
use crate::msg::Identity;
use crate::wire::{Reader, Writer};
use anyhow::{bail, Result};

/// `MapToKey("TradeIIR_t")`.
pub const TRADE: u32 = 0x3628_4F6E;
/// `DAT_101c069c`.
const VERSION: i32 = 2;

/// `N3Msg_TradeStart(target)` [GC 0x100190e0]: `a` = the other character, `b` zero. Received: the trade starts (`FUN_100663e4`).
pub const START: i8 = 0;
/// `N3Msg_TradeAccept` [GC 0x10015bfd] (all zero). Received: `FUN_10066598`.
pub const ACCEPT: i8 = 1;
/// `N3Msg_TradeAbort(flag)` [GC 0x10015ccf]: `a = {0, flag}`. Received: `FUN_100666a3` (`flag` = `a.instance != 0`: the cancel notice).
pub const ABORT: i8 = 2;
/// `N3Msg_TradeConfirm` [GC 0x10015c66] (all zero). Received: `FUN_100673a0`.
pub const CONFIRM: i8 = 3;
/// Received only: the trade went through (`FUN_100668d1`).
pub const COMPLETE: i8 = 4;
/// `N3Msg_TradeAddItem` [GC 0x100191f6]: `a` = the character, `b` = the item (`{0x68, bag slot}`). Received: `FUN_10066cf7`.
pub const ADD_ITEM: i8 = 5;
/// `N3Msg_TradeRemoveItem` [GC 0x1001721c]: as [`ADD_ITEM`]. Received: `FUN_10066faf`.
pub const REMOVE_ITEM: i8 = 6;
/// `N3Msg_TradeSetCash(cash)` [GC 0x10015dbc]: `a = {0, cash}`. Received: `FUN_100672c7` (the partner's cash, `cash >= 0`).
pub const SET_CASH: i8 = 7;
/// Received only, vending machines (`FUN_10067109` / `FUN_100671e8`): not used by the player trade.
pub const VENDING_ADD: i8 = 8;
pub const VENDING_REMOVE: i8 = 9;
/// Received only: `FUN_1006741b`, the accepted state of the window is reset (`GlobalSignals +0xe0 (0)`).
pub const RESET: i8 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Trade {
    pub op: i8,
    pub a: Identity,
    pub b: Identity,
}

impl Trade {
    /// The full N3 payload; `target` is the character the message is for / from (header identity, `+4` of the IIR).
    pub fn encode(&self, target: Identity) -> Vec<u8> {
        let mut w = Writer::default();
        w.u32(TRADE);
        target.write(&mut w);
        w.u8(0);
        w.i32(VERSION);
        w.u8(self.op as u8);
        self.a.write(&mut w);
        self.b.write(&mut w);
        w.0
    }
}

const ZERO: Identity = Identity { kind: 0, instance: 0 };

/// `N3Msg_TradeStart`.
pub fn start(own: Identity, other: Identity) -> Vec<u8> {
    Trade { op: START, a: other, b: ZERO }.encode(own)
}

/// `N3Msg_TradeAccept`.
pub fn accept(own: Identity) -> Vec<u8> {
    Trade { op: ACCEPT, a: ZERO, b: ZERO }.encode(own)
}

/// `N3Msg_TradeConfirm`.
pub fn confirm(own: Identity) -> Vec<u8> {
    Trade { op: CONFIRM, a: ZERO, b: ZERO }.encode(own)
}

/// `N3Msg_TradeAbort(flag)`: `true` from the decline button (`FUN_100df810`), `false` when the partner is on the ignore list.
pub fn abort(own: Identity, flag: bool) -> Vec<u8> {
    Trade { op: ABORT, a: Identity { kind: 0, instance: flag as i32 }, b: ZERO }.encode(own)
}

/// `N3Msg_TradeSetCash(cash)`.
pub fn set_cash(own: Identity, cash: i32) -> Vec<u8> {
    Trade { op: SET_CASH, a: Identity { kind: 0, instance: cash }, b: ZERO }.encode(own)
}

/// `N3Msg_TradeAddItem(own, item)`.
pub fn add_item(own: Identity, item: Identity) -> Vec<u8> {
    Trade { op: ADD_ITEM, a: own, b: item }.encode(own)
}

/// `N3Msg_TradeRemoveItem(character, item)`.
pub fn remove_item(own: Identity, character: Identity, item: Identity) -> Vec<u8> {
    Trade { op: REMOVE_ITEM, a: character, b: item }.encode(own)
}

/// Decode a `TradeIIR_t` body; `Ok(None)` when `h.msg_type` is not [`TRADE`].
pub fn decode(h: &N3Header, r: &mut Reader) -> Result<Option<Trade>> {
    if h.msg_type != TRADE {
        return Ok(None);
    }
    let version = r.i32()?;
    if version != VERSION {
        bail!("TradeIIR_t version {version}");
    }
    let op = r.u8()? as i8;
    // `FUN_1007a60b` returns `10 < (uint)op`: a failed read
    if !(0..=RESET).contains(&op) {
        bail!("TradeIIR_t op {op}");
    }
    Ok(Some(Trade { op, a: Identity::read(r)?, b: Identity::read(r)? }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::n3::outgoing::message_key;

    const OWN: Identity = Identity { kind: 0xC350, instance: 0x82e8 };
    const OTHER: Identity = Identity { kind: 0xC350, instance: 4711 };

    fn parse(b: &[u8]) -> Result<Option<Trade>> {
        let (h, mut r) = N3Header::parse(b)?;
        decode(&h, &mut r)
    }

    #[test]
    fn key_is_the_class_name_hash() {
        assert_eq!(message_key("TradeIIR_t"), TRADE);
    }

    #[test]
    fn every_op_round_trips_and_truncation_fails() {
        for t in [
            Trade { op: START, a: OTHER, b: ZERO },
            Trade { op: ABORT, a: Identity { kind: 0, instance: 1 }, b: ZERO },
            Trade { op: ADD_ITEM, a: OWN, b: Identity { kind: 0x68, instance: 0x41 } },
            Trade { op: SET_CASH, a: Identity { kind: 0, instance: 123_456 }, b: ZERO },
            Trade { op: RESET, a: ZERO, b: ZERO },
        ] {
            let b = t.encode(OWN);
            let (h, mut r) = N3Header::parse(&b).unwrap();
            assert_eq!((h.msg_type, h.target, h.flag), (TRADE, OWN, 0));
            assert_eq!(decode(&h, &mut r).unwrap(), Some(t));
            assert_eq!(r.remaining(), 0);
            for cut in 13..b.len() {
                assert!(parse(&b[..cut]).is_err(), "truncated at {cut}");
            }
        }
    }

    /// The bytes the original writes (`FUN_1007a674`): key, header identity, flag 0, `i32 2`, `i8 op`, two identities.
    #[test]
    fn client_messages_wire_layout() {
        let head = [0x36, 0x28, 0x4F, 0x6E, 0, 0, 0xC3, 0x50, 0, 0, 0x82, 0xe8, 0, 0, 0, 0, 2];
        let want = |op: u8, a: [u8; 8], b: [u8; 8]| [&head[..], &[op], &a, &b].concat();
        let z = [0; 8];
        assert_eq!(start(OWN, OTHER), want(0, [0, 0, 0xC3, 0x50, 0, 0, 0x12, 0x67], z));
        assert_eq!(accept(OWN), want(1, z, z));
        assert_eq!(abort(OWN, true), want(2, [0, 0, 0, 0, 0, 0, 0, 1], z));
        assert_eq!(confirm(OWN), want(3, z, z));
        assert_eq!(set_cash(OWN, 300), want(7, [0, 0, 0, 0, 0, 0, 1, 0x2c], z));
        let item = Identity { kind: 0x68, instance: 0x40 };
        assert_eq!(add_item(OWN, item), want(5, [0, 0, 0xC3, 0x50, 0, 0, 0x82, 0xe8], [0, 0, 0, 0x68, 0, 0, 0, 0x40]));
        assert_eq!(remove_item(OWN, OWN, item), want(6, [0, 0, 0xC3, 0x50, 0, 0, 0x82, 0xe8], [0, 0, 0, 0x68, 0, 0, 0, 0x40]));
    }

    #[test]
    fn bad_version_op_and_other_messages_are_rejected() {
        let good = start(OWN, OTHER);
        let mut b = good.clone();
        b[16] = 3;
        assert!(parse(&b).is_err());
        let mut b = good.clone();
        b[17] = 11;
        assert!(parse(&b).is_err());
        b[17] = 0xff; // sign-extended -1: `10 < (uint)-1`
        assert!(parse(&b).is_err());
        let b = super::super::outgoing::char_in_play(1);
        assert_eq!(parse(&b).unwrap(), None);
    }
}
