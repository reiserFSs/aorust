//! `n3TeleportIIR_t` 0x43197D22 (server -> client; `IsAllowedFromClient` is false): the server moves a dynel, within the
//! playfield or to another one. Docs: docs/zone/world.md §10.2.
//!
//! Read order (`ReadSubClass` [N3 0x10029e7f]): position `f32 x3` (`+0x18`), rotation `f32 x4` (`+0x24`), `PlayfieldProxy_t`
//! (`+0x34`, `FUN_10038402`: `u8 'a'`, `Identity playfield`, `i32 attribute`, `i32 exit_door`, `Identity exit_door_id`),
//! `Identity` (`+0x4c`), `Identity` (`+0x54`), `i32 len` (<= 1000, else the read fails) and `len` raw bytes (copied into the
//! message's own `BinaryStream` at `+0x5c`).

use super::N3Header;
use crate::msg::{Identity, PlayfieldProxy};
use crate::wire::{Reader, Writer};
use anyhow::{bail, Result};

pub const TELEPORT: u32 = 0x43197D22;

/// Largest trailing blob `ReadSubClass` accepts (`< 0x3e9`).
const MAX_BLOB: i32 = 1000;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Teleport {
    /// The dynel moved is the header's target.
    pub target: Identity,
    pub pos: [f32; 3],
    /// Quaternion `x, y, z, w` as `SetRelPosRot(Vector3_t, Quaternion_t)` [N3] takes it (memory order).
    pub rot: [f32; 4],
    /// Destination playfield. `Activate` [N3 0x10029f87] tests `exit_door_id.instance` (`+0x48`): `0` = the dynel is placed
    /// in its own playfield, anything else = a playfield change (for the own character `n3EngineClient_t::StartTeleport`).
    pub proxy: PlayfieldProxy,
    /// `+0x4c` / `+0x54`: read but never used by `Activate` (names unknown).
    pub unused: [Identity; 2],
    pub blob: Vec<u8>,
}

impl Teleport {
    /// `Activate` branch: the destination is another playfield (`+0x48 != 0`).
    pub fn is_zone_change(&self) -> bool {
        self.proxy.exit_door_id.instance != 0
    }

    /// The N3 payload (header with `target` and the pass-on `flag`, then the body in `ReadSubClass` order).
    pub fn encode(&self, flag: u8) -> Vec<u8> {
        let mut w = Writer::default();
        w.u32(TELEPORT);
        self.target.write(&mut w);
        w.u8(flag);
        self.pos.iter().chain(&self.rot).for_each(|&v| w.f32(v));
        w.u8(b'a');
        self.proxy.playfield.write(&mut w);
        w.i32(self.proxy.attribute);
        w.i32(self.proxy.exit_door);
        self.proxy.exit_door_id.write(&mut w);
        self.unused.iter().for_each(|i| i.write(&mut w));
        w.i32(self.blob.len() as i32);
        w.bytes(&self.blob);
        w.0
    }
}

pub fn decode(h: &N3Header, r: &mut Reader) -> Result<Option<Teleport>> {
    if h.msg_type != TELEPORT {
        return Ok(None);
    }
    let pos = [r.f32()?, r.f32()?, r.f32()?];
    let rot = [r.f32()?, r.f32()?, r.f32()?, r.f32()?];
    let tag = r.u8()?;
    if tag != b'a' {
        bail!("Invalid playfieldproxy version {tag:#x}");
    }
    let proxy = PlayfieldProxy { playfield: Identity::read(r)?, attribute: r.i32()?, exit_door: r.i32()?, exit_door_id: Identity::read(r)? };
    let unused = [Identity::read(r)?, Identity::read(r)?];
    let len = r.i32()?;
    if !(0..=MAX_BLOB).contains(&len) {
        bail!("n3TeleportIIR_t blob of {len} bytes");
    }
    Ok(Some(Teleport { target: h.target, pos, rot, proxy, unused, blob: r.bytes(len as usize)?.to_vec() }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(dest: i32, blob: &[u8]) -> Vec<u8> {
        let proxy = PlayfieldProxy { playfield: Identity { kind: 0xC79C, instance: 4582 }, exit_door_id: Identity { kind: 0x9C50, instance: dest }, ..Default::default() };
        let t = Teleport { target: Identity { kind: 50000, instance: 25988 }, pos: [927.0, 23.5, 742.7], rot: [0.0, 0.5, 0.0, 0.866], proxy, unused: Default::default(), blob: blob.to_vec() };
        t.encode(1)
    }

    fn parse(b: &[u8]) -> Result<Option<Teleport>> {
        let (h, mut r) = N3Header::parse(b)?;
        decode(&h, &mut r)
    }

    #[test]
    fn key_is_the_class_name_hash() {
        let key = "n3TeleportIIR_t".bytes().enumerate().fold(0u32, |k, (i, c)| k ^ ((c as u32) << ((i & 3) * 8)));
        assert_eq!(key, TELEPORT);
    }

    #[test]
    fn in_place_and_zone_change_follow_exit_door_id_instance() {
        let t = parse(&msg(0, &[])).unwrap().unwrap();
        assert!(!t.is_zone_change());
        assert_eq!((t.target.instance, t.pos, t.rot[3]), (25988, [927.0, 23.5, 742.7], 0.866));
        let t = parse(&msg(4604, &[1, 2, 3])).unwrap().unwrap();
        assert!(t.is_zone_change() && t.proxy.playfield.instance == 4582 && t.blob == [1, 2, 3]);
        assert_eq!(t.encode(1), msg(4604, &[1, 2, 3]));
    }

    #[test]
    fn malformed_bodies_are_errors() {
        let ok = msg(0, &[9]);
        for cut in 13..ok.len() {
            assert!(parse(&ok[..cut]).is_err(), "cut at {cut}");
        }
        let mut bad_tag = ok.clone();
        bad_tag[13 + 28] = b'b';
        assert!(parse(&bad_tag).is_err());
        assert!(parse(&msg(0, &[0; 1001])).is_err());
    }
}
