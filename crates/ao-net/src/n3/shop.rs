//! `ShopUpdateIIR_t` (Gamecode.dll): the stock list of a vending machine. Layout and addresses: docs/zone/interact.md ("Vending machines / shops").
//!
//! An `n3InfoItemRemote_t` made by `FUN_100a0fb4` [GC] (class name string, key `MapToKey("ShopUpdateIIR_t")` = `58362220`), vftable 0x10166c88: read slot 7
//! `FUN_100a0ef9` -> `FUN_1009a55e`, write slot 8 `FUN_100a0f18` -> `FUN_100995ee`, `Activate` slot 2 `FUN_100a0f2c` ([`ShopUpdate`] is only valid for a
//! `VendingMachine_t` header target; it then runs `FUN_1009a4b2`). Body: `i32 (n + 1) * 0x3f1`, then `n` x (`i32 low_id`, `i32 high_id`, `i32 ql`) = the
//! arguments of `GameData::ACGItem_t::ACGItem_t(low, high, ql)`. `FUN_1009a55e` accepts only a word that is a positive multiple of 0x3f1 with
//! `quotient - 1 < 1000`; anything else clears the stream's state (`BinaryStream::clear(4)`), i.e. the message is malformed. The flag byte of the header is
//! the usual 0.

use super::world::AcgItem;
use super::N3Header;
use crate::msg::Identity;
use crate::wire::{Reader, Writer};
use anyhow::{bail, Result};

/// `MapToKey("ShopUpdateIIR_t")`.
pub const SHOP_UPDATE: u32 = 0x5836_2220;
/// `FUN_1009a55e`: the count word is `(entries + 1) * 0x3f1`.
const COUNT_UNIT: i32 = 0x3f1;
/// `FUN_1009a55e`: `quotient - 1 < 1000`.
const MAX_ENTRIES: i32 = 999;

/// The stock of the machine named by the N3 header target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShopUpdate {
    pub items: Vec<AcgItem>,
}

impl ShopUpdate {
    /// The full N3 payload: header (`machine` = the vending machine, flag byte 0) and the body.
    pub fn encode(&self, machine: Identity) -> Vec<u8> {
        let mut w = Writer::default();
        w.u32(SHOP_UPDATE);
        machine.write(&mut w);
        w.u8(0);
        w.i32((self.items.len() as i32 + 1) * COUNT_UNIT);
        for i in &self.items {
            w.i32(i.low_id);
            w.i32(i.high_id);
            w.i32(i.level);
        }
        w.0
    }
}

/// Decode a `ShopUpdateIIR_t` body; `Ok(None)` when `h.msg_type` is another message.
pub fn decode(h: &N3Header, r: &mut Reader) -> Result<Option<ShopUpdate>> {
    if h.msg_type != SHOP_UPDATE {
        return Ok(None);
    }
    let v = r.i32()?;
    if v <= 0 || v % COUNT_UNIT != 0 || v / COUNT_UNIT - 1 > MAX_ENTRIES {
        bail!("shop update count word {v:#x}");
    }
    let n = v / COUNT_UNIT - 1;
    let mut items = Vec::with_capacity(n as usize);
    for _ in 0..n {
        items.push(AcgItem { low_id: r.i32()?, high_id: r.i32()?, level: r.i32()? });
    }
    Ok(Some(ShopUpdate { items }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::n3::outgoing::message_key;

    const MACHINE: Identity = Identity { kind: 0xC75B, instance: 0x4b };

    fn parse(b: &[u8]) -> Result<Option<ShopUpdate>> {
        let (h, mut r) = N3Header::parse(b)?;
        decode(&h, &mut r)
    }

    #[test]
    fn key_is_the_class_name_hash() {
        assert_eq!(message_key("ShopUpdateIIR_t"), SHOP_UPDATE);
    }

    #[test]
    fn round_trip_and_truncation() {
        for items in [vec![], vec![AcgItem { low_id: 0x469bb, high_id: 0x469bb, level: 1 }, AcgItem { low_id: 7, high_id: 9, level: 200 }]] {
            let m = ShopUpdate { items };
            let b = m.encode(MACHINE);
            assert_eq!(parse(&b).unwrap(), Some(m));
            for cut in 13..b.len() {
                assert!(parse(&b[..cut]).is_err(), "truncated at {cut}");
            }
        }
    }

    #[test]
    fn bad_count_words_are_rejected() {
        let good = ShopUpdate { items: vec![AcgItem::default()] }.encode(MACHINE);
        for word in [0i32, -0x3f1, 0x3f1 * 1001, 0x3f1 + 1] {
            let mut b = good.clone();
            b[13..17].copy_from_slice(&word.to_be_bytes());
            assert!(parse(&b).is_err(), "{word:#x}");
        }
        let mut b = good;
        b[0] ^= 1;
        assert_eq!(parse(&b).unwrap(), None);
    }
}
