//! Grid / whompah / shuttle destinations (`GridDestinationSelectIIR_t`, `GridSelectedIIR_t`, Gamecode.dll): the server offers a list of
//! destinations, the client answers with the chosen one. Layouts and addresses: docs/zone/interact.md ("Grid / whompah / shuttle").
//!
//! Both classes derive from `n3InfoItemRemote_t`; the N3 header target is the client character and the "to be passed on" byte is **0**
//! (`ClearToBePassedOn`, `FUN_10128fd9` / `FUN_10129398`). There is no version word: the body starts right away.
//!
//! * `GridDestinationSelectIIR_t` body (`FUN_101290a0` read / `FUN_101290cc` write, `+0x18` list, `+0x2c` token): `i32 (count + 1) * 0x3f1`
//!   (`FUN_10128f1e` rejects a value that is not a multiple of 0x3f1 or whose quotient is not 1..=0x7531), `count` entries (`FUN_10128b07` /
//!   `FUN_10128aac`, 0x30 bytes in memory), then the [`Token`].
//! * Entry: `i32 playfield` (`+0`), [`Identity`] (`+4`), `i16`-length string (`+0xc`, `FUN_100388e2` / `FUN_1003889d`), `i32` (`+0x2c`), `i32` (`+0x28`).
//! * Token (`Token_c`, vtable 0x10171690: write `FUN_1013d0f3`, read `FUN_1013d12d`): byte `'a'`, `i32` length, the bytes.
//! * `GridSelectedIIR_t` body (`FUN_101293a4` / `FUN_101293ec`): the token (`+0x18`), `i32` (`+0x24`), `i32` (`+0x28`), [`Identity`] (`+0x2c`).

use super::N3Header;
use crate::msg::Identity;
use crate::wire::{Reader, Writer};
use anyhow::{bail, Result};

/// `GridDestinationSelectIIR_t` (server -> client); `Activate` `FUN_10128fd9`.
pub const DESTINATION_SELECT: u32 = 0x0639_474D;
/// `GridSelectedIIR_t` (client -> server), built by `N3Msg_GridDestinationSelected` [GC 0x1001817b].
pub const SELECTED: u32 = 0x3A32_2A4A;

/// `FUN_10128f1e`: the count word is `(entries + 1) * 0x3f1`.
const COUNT_UNIT: i32 = 0x3f1;
/// `FUN_10128f1e`: at most 0x7530 entries (`quotient - 1 < 0x7531`).
const MAX_ENTRIES: i32 = 0x7530;

/// One destination (`GridDestination_t`, 0x30 bytes in memory).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Destination {
    /// `+0`: the playfield id (`N3Msg_GetPFName`, and the second argument of `N3Msg_GridDestinationSelected`).
    pub playfield: i32,
    /// `+4`: sent back as the third argument of `N3Msg_GridDestinationSelected`.
    pub target: Identity,
    /// `+0xc`: the "Location" column.
    pub name: String,
    /// `+0x2c` (second `i32` of the entry on the wire); the client's window never reads it.
    pub v2c: i32,
    /// `+0x28` (last `i32` of the entry on the wire); the client's window never reads it.
    pub v28: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Grid {
    DestinationSelect { destinations: Vec<Destination>, token: Vec<u8> },
    /// `index` = position in the list (the row's `Variant`), `playfield` = the entry's `+0`, `target` its identity; `index == -1` with zeros cancels.
    Selected { index: i32, playfield: u32, target: Identity, token: Vec<u8> },
}

impl Grid {
    pub fn key(&self) -> u32 {
        match self {
            Grid::DestinationSelect { .. } => DESTINATION_SELECT,
            Grid::Selected { .. } => SELECTED,
        }
    }

    /// The full N3 payload: header (`target` = the client character, flag byte 0) and the body.
    pub fn encode(&self, target: Identity) -> Vec<u8> {
        let mut w = Writer::default();
        w.u32(self.key());
        target.write(&mut w);
        w.u8(0);
        match self {
            Grid::DestinationSelect { destinations, token } => {
                w.i32((destinations.len() as i32 + 1) * COUNT_UNIT);
                for d in destinations {
                    w.i32(d.playfield);
                    d.target.write(&mut w);
                    w.str_i16(&d.name);
                    w.i32(d.v2c);
                    w.i32(d.v28);
                }
                write_token(&mut w, token);
            }
            Grid::Selected { index, playfield, target, token } => {
                write_token(&mut w, token);
                w.i32(*index);
                w.u32(*playfield);
                target.write(&mut w);
            }
        }
        w.0
    }
}

/// `N3Msg_GridDestinationSelected(index, playfield, target)` [GC 0x1001817b]: `GridSelectedIIR_t(own, stored token, index, playfield, target)`.
pub fn selected(own: Identity, token: &[u8], index: i32, playfield: u32, target: Identity) -> Vec<u8> {
    Grid::Selected { index, playfield, target, token: token.to_vec() }.encode(own)
}

/// The window's close path (`FUN_100ff939` [GUI]) while it is still open: `N3Msg_GridDestinationSelected(-1, 0, Identity(0, 0))`.
pub fn cancel(own: Identity, token: &[u8]) -> Vec<u8> {
    selected(own, token, -1, 0, Identity::default())
}

fn write_token(w: &mut Writer, token: &[u8]) {
    w.u8(b'a');
    w.i32(token.len() as i32);
    w.bytes(token);
}

/// `FUN_1013d12d`: anything but `'a'` throws; the length is an unsigned `new[]` size.
fn read_token(r: &mut Reader) -> Result<Vec<u8>> {
    let tag = r.u8()?;
    if tag != b'a' {
        bail!("grid token tag {tag:#x}");
    }
    let n = r.i32()?;
    if n < 0 {
        bail!("grid token length {n}");
    }
    Ok(r.bytes(n as usize)?.to_vec())
}

/// Decode a grid body; `Ok(None)` when `h.msg_type` is not a grid message.
pub fn decode(h: &N3Header, r: &mut Reader) -> Result<Option<Grid>> {
    Ok(Some(match h.msg_type {
        DESTINATION_SELECT => {
            Grid::DestinationSelect { destinations: read_destinations(r)?, token: read_token(r)? }
        }
        SELECTED => {
            let token = read_token(r)?;
            let index = r.i32()?;
            let playfield = r.u32()?;
            Grid::Selected { index, playfield, target: Identity::read(r)?, token }
        }
        _ => return Ok(None),
    }))
}

pub(super) fn read_destinations(r: &mut Reader) -> Result<Vec<Destination>> {
    let v = r.i32()?;
    if v % COUNT_UNIT != 0 || !(1..=MAX_ENTRIES + 1).contains(&(v / COUNT_UNIT)) {
        bail!("grid destination count word {v:#x}");
    }
    let n = v / COUNT_UNIT - 1;
    let mut destinations = Vec::with_capacity((n as usize).min(1024));
    for _ in 0..n {
        let playfield = r.i32()?;
        let target = Identity::read(r)?;
        let name = r.str_i16()?;
        let (v2c, v28) = (r.i32()?, r.i32()?);
        destinations.push(Destination { playfield, target, name, v2c, v28 });
    }
    Ok(destinations)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::n3::outgoing::message_key;

    const OWN: Identity = Identity { kind: 0xC350, instance: 0x82e8 };

    fn list() -> Grid {
        Grid::DestinationSelect {
            destinations: vec![
                Destination { playfield: 4582, target: Identity { kind: 0x9C50, instance: 4582 }, name: "Shuttle".into(), v2c: 1, v28: 2 },
                Destination { playfield: 560, target: Identity { kind: 0x9C50, instance: 560 }, name: String::new(), v2c: -1, v28: 0 },
            ],
            token: vec![1, 2, 3, 0xff],
        }
    }

    fn round(g: Grid) {
        let b = g.encode(OWN);
        let (h, mut r) = N3Header::parse(&b).unwrap();
        assert_eq!((h.msg_type, h.target, h.flag), (g.key(), OWN, 0));
        assert_eq!(decode(&h, &mut r).unwrap(), Some(g));
        assert_eq!(r.remaining(), 0);
        for cut in 13..b.len() {
            let (h, mut r) = N3Header::parse(&b[..cut]).unwrap();
            assert!(decode(&h, &mut r).is_err(), "truncated at {cut}");
        }
    }

    #[test]
    fn keys_are_the_class_name_hashes() {
        assert_eq!(message_key("GridDestinationSelectIIR_t"), DESTINATION_SELECT);
        assert_eq!(message_key("GridSelectedIIR_t"), SELECTED);
    }

    #[test]
    fn messages_round_trip() {
        round(list());
        round(Grid::DestinationSelect { destinations: vec![], token: vec![] });
        round(Grid::Selected { index: 1, playfield: 560, target: Identity { kind: 0x9C50, instance: 560 }, token: vec![9; 40] });
    }

    /// The bytes `N3Msg_GridDestinationSelected` writes: header, token (`'a'`, length, bytes), index, playfield, identity.
    #[test]
    fn selected_wire_layout() {
        let b = selected(OWN, &[0xAA, 0xBB], 3, 560, Identity { kind: 0x9C50, instance: 560 });
        let mut want = SELECTED.to_be_bytes().to_vec();
        want.extend([0, 0, 0xC3, 0x50, 0, 0, 0x82, 0xe8, 0]);
        want.extend([b'a', 0, 0, 0, 2, 0xAA, 0xBB]);
        want.extend([0, 0, 0, 3, 0, 0, 2, 0x30, 0, 0, 0x9C, 0x50, 0, 0, 2, 0x30]);
        assert_eq!(b, want);
        // the close path: (-1, 0, Identity(0, 0))
        let c = cancel(OWN, &[]);
        let mut want = SELECTED.to_be_bytes().to_vec();
        want.extend([0, 0, 0xC3, 0x50, 0, 0, 0x82, 0xe8, 0, b'a', 0, 0, 0, 0, 0xff, 0xff, 0xff, 0xff]);
        want.extend([0; 12]);
        assert_eq!(c, want);
    }

    #[test]
    fn list_wire_layout() {
        let b = list().encode(OWN);
        // count word (2 + 1) * 0x3f1, then the first entry
        assert_eq!(&b[13..17], &(3 * 0x3f1i32).to_be_bytes());
        assert_eq!(&b[17..21], &4582i32.to_be_bytes());
        assert_eq!(&b[29..31], &[0, 7]);
        assert_eq!(&b[31..38], b"Shuttle");
        assert_eq!(&b[38..46], &[0, 0, 0, 1, 0, 0, 0, 2]);
    }

    #[test]
    fn bad_lists_and_tokens_are_rejected() {
        for word in [0i32, 0x3f1 + 1, 0x3f1 * 0x7533, -0x3f1] {
            let mut b = list().encode(OWN);
            b[13..17].copy_from_slice(&word.to_be_bytes());
            let (h, mut r) = N3Header::parse(&b).unwrap();
            assert!(decode(&h, &mut r).is_err(), "{word:#x}");
        }
        let mut b = Grid::DestinationSelect { destinations: vec![], token: vec![7] }.encode(OWN);
        let tag = 13 + 4;
        b[tag] = b'b';
        let (h, mut r) = N3Header::parse(&b).unwrap();
        assert!(decode(&h, &mut r).is_err());
        b[tag] = b'a';
        b[tag + 1..tag + 5].copy_from_slice(&(-1i32).to_be_bytes());
        let (h, mut r) = N3Header::parse(&b).unwrap();
        assert!(decode(&h, &mut r).is_err());
        let b = super::super::outgoing::char_in_play(1);
        let (h, mut r) = N3Header::parse(&b).unwrap();
        assert_eq!(decode(&h, &mut r).unwrap(), None);
    }
}
