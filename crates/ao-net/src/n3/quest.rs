//! The own missions: `QuestFullUpdateIIR_t`'s `QuestList` (Gamecode `FUN_100abd64` 0x100abd64 -> per quest `FUN_100ab951` `Quest_t` stream reader, versions 7..=15),
//! its `RewardBox_t` (`FUN_10086b5e`), `QuestActionList` (`FUN_100ac789` / `FUN_100ac64a`) and `WorldPos_c` (GameData 0x1000ca52). Layouts are read from the
//! disassembly of those functions; no capture holds a mission, so the tests use bytes built from that layout (docs/gui.md §11.12). BinaryStream facts
//! (BinaryStream.dll): integers and floats are big-endian, `operator>>(char*)` reads bytes up to a NUL.

use super::world::{counted, fits, identities};
use crate::msg::Identity;
use crate::wire::Reader;
use anyhow::{bail, ensure, Result};

/// `GameData::ACGItem_t` as the quest streams read it (`operator>>` GameData 0x1000e9d7: four `i32`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Acg {
    pub low_id: i32,
    pub high_id: i32,
    pub level: i32,
    pub extra: i32,
}

/// `WorldPos_c` (GameData 0x1000ca52): the playfield identity, two `i32` (x, z as the world coordinates the class floats), a local `Vector3`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct WorldPos {
    pub playfield: Identity,
    pub x: i32,
    pub z: i32,
    pub local: [f32; 3],
}

/// `QuestAction_t` (`FUN_100ac64a`, 0x8c bytes in memory): an `i32`, five identities, four floats, an identity, four floats, an identity, two `i32`, an identity, a
/// `WorldPos_c`. The meaning of the individual fields is not resolved; the first action's position is what `N3Msg_GetQuestWorldPos` [GC 0x1001ab61] returns.
#[derive(Debug, Clone, PartialEq)]
pub struct QuestAction {
    pub id: i32,
    /// Identities at `+0x4, +0xc, +0x14, +0x1c, +0x24, +0x3c, +0x54, +0x64`, in stream order.
    pub identities: [Identity; 8],
    /// Floats at `+0x2c..+0x38` then `+0x44..+0x50`.
    pub floats: [f32; 8],
    /// `+0x5c`, `+0x60`.
    pub ints: [i32; 2],
    pub pos: WorldPos,
}

/// `RewardBox_t` (`FUN_10086b5e`, versions 3..=6).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RewardBox {
    pub version: i32,
    /// Box words `+0` .. `+0x18`: `[0]` and `[1]` from the first and third `i32`, `[2]`, `[5]`, `[6]` (version >= 4) and `[3]`, `[4]` (version >= 5).
    pub ints: [i32; 7],
    /// The two identity lists (`FUN_1002b8b6`, discarded by the client's box constructor) and the item list (`FUN_1004662a`).
    pub lists: [Vec<Identity>; 2],
    pub items: Vec<Acg>,
    pub item: Acg,
}

/// `Quest_t` as streamed (`FUN_100ab951`).
#[derive(Debug, Clone, PartialEq)]
pub struct Quest {
    /// The identity the list element starts with (kind 0xDAC3: `N3Msg_GetName` returns the quest's name buffer).
    pub id: Identity,
    pub version: i32,
    /// `+0xa4`.
    pub value_a4: i32,
    /// `+0x8`: the name (`operator>>(char*)`, NUL terminated); what `N3Msg_GetName(quest)` returns.
    pub name: String,
    /// `+0x88`: the description text (`i32` length 1..=0x1000 + bytes).
    pub description: String,
    pub identity_38: Identity,
    pub reward: RewardBox,
    pub identity_40: Identity,
    /// `+0x48`: `N3Msg_GetQuestIcon` [GC 0x173e0] (an rdb 1010008 image id, `FUN_1003eb30`).
    pub icon: i32,
    /// `+0x70`, `+0x74`.
    pub values_70: [i32; 2],
    pub actions: Vec<QuestAction>,
    /// `+0x60` and `+0x50` lists (`& 0x7ffffff`).
    pub list_60: Vec<i32>,
    pub list_50: Vec<i32>,
    /// Version >= 8: `+0xa8` (6 before).
    pub value_a8: i32,
    /// Version >= 10: `+0x4c`; >= 11: `+0xb0`; >= 12: `+0xb4` identity, `+0xbc`, `+0xc0`; >= 14: `+0xd4`.
    pub value_4c: i32,
    pub value_b0: i32,
    pub identity_b4: Identity,
    pub values_bc: [i32; 2],
    /// Version >= 13: `+0xc8` list of `(identity, i32)`.
    pub list_c8: Vec<(Identity, i32)>,
    pub value_d4: i32,
    /// Version >= 15: `FUN_100ab890`'s list of `(i32, i32)` (a `FactionList`).
    pub factions: Vec<(i32, i32)>,
}

fn cstr(r: &mut Reader) -> Result<String> {
    let mut b = vec![];
    loop {
        let c = r.u8()?;
        if c == 0 {
            return Ok(String::from_utf8_lossy(&b).into_owned());
        }
        ensure!(b.len() < 0x1000, "unterminated string");
        b.push(c);
    }
}

fn acg(r: &mut Reader) -> Result<Acg> {
    Ok(Acg { low_id: r.i32()?, high_id: r.i32()?, level: r.i32()?, extra: r.i32()? })
}

fn floats<const N: usize>(r: &mut Reader) -> Result<[f32; N]> {
    let mut v = [0.0; N];
    for f in &mut v {
        *f = r.f32()?;
    }
    Ok(v)
}

fn world_pos(r: &mut Reader) -> Result<WorldPos> {
    Ok(WorldPos { playfield: Identity::read(r)?, x: r.i32()?, z: r.i32()?, local: floats(r)? })
}

fn quest_action(r: &mut Reader) -> Result<QuestAction> {
    let id = r.i32()?;
    let mut ids = [Identity::default(); 8];
    let mut fl = [0.0f32; 8];
    // five identities, four floats, an identity, four floats, an identity (FUN_100ac64a)
    for i in ids.iter_mut().take(5) {
        *i = Identity::read(r)?;
    }
    fl[..4].copy_from_slice(&floats::<4>(r)?);
    ids[5] = Identity::read(r)?;
    fl[4..].copy_from_slice(&floats::<4>(r)?);
    ids[6] = Identity::read(r)?;
    let ints = [r.i32()?, r.i32()?];
    ids[7] = Identity::read(r)?;
    Ok(QuestAction { id, identities: ids, floats: fl, ints, pos: world_pos(r)? })
}

fn reward_box(r: &mut Reader) -> Result<RewardBox> {
    let version = r.i32()?;
    ensure!((3..=6).contains(&version), "RewardBox_t version {version}");
    let mut b = RewardBox { version, ..Default::default() };
    b.ints[0] = r.i32()?;
    r.i32()?; // read into a local and dropped
    b.ints[1] = r.i32()?;
    b.lists = [identities(r)?, identities(r)?];
    let (_, n) = counted(r)?;
    fits(n, 16, r)?;
    b.items = (0..n).map(|_| acg(r)).collect::<Result<_>>()?;
    if version >= 4 {
        b.ints[2] = r.i32()?;
        b.ints[5] = r.i32()?;
        b.ints[6] = r.i32()?;
    }
    if version >= 5 {
        b.ints[3] = r.i32()?;
        b.ints[4] = r.i32()?;
    }
    if version > 5 {
        b.item = acg(r)?;
    }
    Ok(b)
}

/// One `Quest_t` body (after the list element's identity).
fn quest_body(r: &mut Reader, id: Identity) -> Result<Quest> {
    let version = r.i32()?;
    if !(7..=15).contains(&version) {
        bail!("Quest_t version {version}");
    }
    r.i32()?;
    r.i32()?;
    let value_a4 = r.i32()?;
    let name = cstr(r)?;
    let len = r.i32()?;
    ensure!((1..=0x1000).contains(&len), "Quest_t description of {len} bytes");
    let description = String::from_utf8_lossy(r.bytes(len as usize)?).trim_end_matches('\0').to_owned();
    let identity_38 = Identity::read(r)?;
    let reward = reward_box(r)?;
    let identity_40 = Identity::read(r)?;
    let icon = r.i32()?;
    let values_70 = [r.i32()?, r.i32()?];
    let (_, n) = counted(r)?;
    fits(n, 4, r)?;
    let actions = (0..n).map(|_| quest_action(r)).collect::<Result<Vec<_>>>()?;
    identities(r)?; // FUN_1002b8b6 into a local vector
    let list = |r: &mut Reader| -> Result<Vec<i32>> {
        let n = r.i32()?;
        ensure!((0..=0x7530).contains(&n), "list of {n}");
        fits(n as usize, 4, r)?;
        (0..n).map(|_| Ok(r.i32()? & 0x7ffffff)).collect()
    };
    let list_60 = list(r)?;
    let list_50 = list(r)?;
    // `{Identity, i32 len, bytes}` records read into a stack buffer and dropped
    let n = r.i32()?;
    ensure!((0..=0x7530).contains(&n), "record list of {n}");
    for _ in 0..n {
        Identity::read(r)?;
        let l = r.i32()?;
        ensure!((0..=0x180).contains(&l), "record text of {l} bytes");
        r.bytes(l as usize)?;
    }
    let value_a8 = if version >= 8 { r.i32()? } else { 6 };
    if version >= 9 {
        identities(r)?;
    }
    let value_4c = if version >= 10 { r.i32()? } else { 0 };
    let value_b0 = if version >= 11 { r.i32()? } else { 0 };
    let (identity_b4, values_bc) = if version >= 12 { (Identity::read(r)?, [r.i32()?, r.i32()?]) } else { (Identity::default(), [0, 0]) };
    let mut list_c8 = vec![];
    if version >= 13 {
        let n = r.i32()?;
        ensure!((0..=0x7530).contains(&n), "list_c8 of {n}");
        for _ in 0..n {
            list_c8.push((Identity::read(r)?, r.i32()?));
        }
    }
    let value_d4 = if version >= 14 { r.i32()? } else { 0 };
    let mut factions = vec![];
    if version >= 15 {
        let (_, n) = counted(r)?;
        fits(n, 8, r)?;
        for _ in 0..n {
            factions.push((r.i32()?, r.i32()?));
        }
    }
    Ok(Quest {
        id,
        version,
        value_a4,
        name,
        description,
        identity_38,
        reward,
        identity_40,
        icon,
        values_70,
        actions,
        list_60,
        list_50,
        value_a8,
        value_4c,
        value_b0,
        identity_b4,
        values_bc,
        list_c8,
        value_d4,
        factions,
    })
}

/// `QuestList` (`FUN_100abd64`): the size word was consumed by the caller (`count` quests follow, each an identity and a `Quest_t`).
pub fn quest_list(r: &mut Reader, count: usize) -> Result<Vec<Quest>> {
    fits(count, 8, r)?;
    (0..count)
        .map(|_| {
            let id = Identity::read(r)?;
            quest_body(r, id)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::Writer;

    const UNIT: u32 = 0x3F1;

    fn ident(w: &mut Writer, kind: i32, inst: i32) {
        Identity { kind, instance: inst }.write(w);
    }

    fn list_word(w: &mut Writer, n: u32) {
        w.u32((n + 1) * UNIT);
    }

    /// A version-15 quest assembled from the reader's field order.
    pub(crate) fn sample(version: i32, name: &str, icon: i32) -> Vec<u8> {
        let mut w = Writer::default();
        ident(&mut w, 0xDAC3, 77);
        w.i32(version);
        w.i32(1);
        w.i32(2);
        w.i32(3);
        w.bytes(name.as_bytes());
        w.u8(0);
        let d = b"Kill the rats";
        w.i32(d.len() as i32);
        w.bytes(d);
        ident(&mut w, 1, 2); // +0x38
        // RewardBox v6: 3 ints, two identity lists, an item list, 3 + 2 ints, an item
        w.i32(6);
        w.i32(10);
        w.i32(11);
        w.i32(12);
        list_word(&mut w, 1);
        ident(&mut w, 5, 6);
        list_word(&mut w, 0);
        list_word(&mut w, 1);
        for v in [100, 101, 102, 103] {
            w.i32(v);
        }
        for v in [20, 21, 22, 23, 24] {
            w.i32(v);
        }
        for v in [200, 201, 202, 203] {
            w.i32(v);
        }
        ident(&mut w, 3, 4); // +0x40
        w.i32(icon);
        w.i32(70);
        w.i32(74);
        // one QuestAction
        list_word(&mut w, 1);
        w.i32(9);
        for i in 0..5 {
            ident(&mut w, 0x100 + i, i);
        }
        for f in [1.0f32, 2.0, 3.0, 4.0] {
            w.f32(f);
        }
        ident(&mut w, 0x200, 6);
        for f in [5.0f32, 6.0, 7.0, 8.0] {
            w.f32(f);
        }
        ident(&mut w, 0x300, 7);
        w.i32(31);
        w.i32(32);
        ident(&mut w, 0x400, 8);
        ident(&mut w, 0xC79C, 4605); // WorldPos: playfield, x, z, local
        w.i32(1500);
        w.i32(2500);
        for f in [10.0f32, 20.0, 30.0] {
            w.f32(f);
        }
        list_word(&mut w, 0); // identities (local)
        w.i32(1); // list_60
        w.i32(0x1000_0007);
        w.i32(0); // list_50
        w.i32(0); // records
        if version >= 8 {
            w.i32(5);
        }
        if version >= 9 {
            list_word(&mut w, 0);
        }
        if version >= 10 {
            w.i32(40);
        }
        if version >= 11 {
            w.i32(50);
        }
        if version >= 12 {
            ident(&mut w, 9, 9);
            w.i32(60);
            w.i32(61);
        }
        if version >= 13 {
            w.i32(1);
            ident(&mut w, 8, 8);
            w.i32(80);
        }
        if version >= 14 {
            w.i32(90);
        }
        if version >= 15 {
            list_word(&mut w, 1);
            w.i32(1);
            w.i32(2);
        }
        w.0
    }

    #[test]
    fn quest_follows_the_streamed_layout() {
        let bytes = sample(15, "Rat Hunt", 4242);
        let mut r = Reader::new(&bytes);
        let q = quest_list(&mut r, 1).unwrap();
        assert_eq!(r.remaining(), 0);
        let q = &q[0];
        assert_eq!((q.id, q.version, q.name.as_str(), q.icon), (Identity { kind: 0xDAC3, instance: 77 }, 15, "Rat Hunt", 4242));
        assert_eq!(q.description, "Kill the rats");
        assert_eq!(q.reward.item, Acg { low_id: 200, high_id: 201, level: 202, extra: 203 });
        assert_eq!(q.reward.items.len(), 1);
        assert_eq!(q.actions.len(), 1);
        let a = &q.actions[0];
        assert_eq!((a.id, a.floats[4], a.pos.playfield.instance, a.pos.x, a.pos.local), (9, 5.0, 4605, 1500, [10.0, 20.0, 30.0]));
        assert_eq!((q.list_60.as_slice(), q.value_a8, q.value_4c, q.value_b0, q.value_d4), (&[7][..], 5, 40, 50, 90));
        assert_eq!((q.identity_b4.kind, q.values_bc, q.list_c8.len(), q.factions.clone()), (9, [60, 61], 1, vec![(1, 2)]));
    }

    #[test]
    fn older_versions_end_earlier() {
        for v in 7..=14 {
            let bytes = sample(v, "x", 1);
            let mut r = Reader::new(&bytes);
            quest_list(&mut r, 1).unwrap();
            assert_eq!(r.remaining(), 0, "v{v}");
        }
    }

    #[test]
    fn malformed_quests_are_errors() {
        let bytes = sample(15, "Rat Hunt", 1);
        for n in 0..bytes.len() {
            assert!(quest_list(&mut Reader::new(&bytes[..n]), 1).is_err(), "{n}");
        }
        let mut bad = sample(15, "Rat Hunt", 1);
        bad[11] = 16; // version 16
        assert!(quest_list(&mut Reader::new(&bad), 1).is_err());
    }
}
