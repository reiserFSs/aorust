//! Character information. GC reader 0x10045d02, request 0x10027271/0x1003f5fc.
//! Offset names preserve fields whose semantics are supplied by the original GUI getters.
use super::{action, grid::Destination, inventory, world::{AcgItem, CharacterAction}, N3Header};
use crate::{msg::Identity, wire::Reader};
use anyhow::{ensure, Result};

pub const INFO_PACKET: u32 = 0x4D38_242E;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InfoPacket {
    pub flags: u8,
    pub extended_flags: u8,
    pub v32: u8,
    pub v33: u8,
    pub v34: u8,
    pub v35: u8,
    pub v30: i16,
    pub v20: i32,
    pub v24: i32,
    pub v28: i32,
    pub v2c: i32,
    pub s48: String,
    pub s64: String,
    pub s80: String,
    pub s04: String,
    pub s9c: Option<String>,
    pub destinations: Vec<Destination>,
    pub v38: Option<i32>,
    pub items: Vec<AcgItem>,
    pub suppression: Option<(i32, u8)>,
    pub side_xp: Option<[i32; 12]>,
    pub v3c: i32,
    pub v40: i32,
    pub v44: i32,
    pub player_values: Option<[i32; 8]>,
}

impl InfoPacket {
    pub fn read(r: &mut Reader) -> Result<Self> {
        let flags = r.u8()?;
        let extended_flags = if flags & 0x40 != 0 { r.u8()? } else { 0 };
        let (v32, v33, v34) = (r.u8()?, r.u8()?, r.u8()?);
        if extended_flags == 0 { r.u8()?; }
        let v35 = r.u8()?;
        let v30 = r.i16()?;
        let (v20, v24, v28, v2c) = (r.i32()?, r.i32()?, r.i32()?, r.i32()?);
        let (s48, s64, s80, s04) = (r.str_i16()?, r.str_i16()?, r.str_i16()?, r.str_i16()?);
        let mut destinations = Vec::new();
        let (s9c, v38) = if flags & 1 != 0 {
            let name = r.str_i16()?;
            if flags & 2 != 0 { destinations = super::grid::read_destinations(r)?; }
            (Some(name), Some(r.i32()?))
        } else { (None, None) };
        let mut items = Vec::new();
        if flags & 4 != 0 {
            let (_, n) = super::world::counted(r)?;
            ensure!(n <= 0x7530, "InfoPacket item count {n}");
            for _ in 0..n { items.push(inventory::read_acg(r)?); }
        }
        let suppression = if flags & 8 != 0 { Some((r.i32()?, r.u8()?)) } else { None };
        let side_xp = if flags & 0x20 != 0 { Some(ints(r)?) } else { None };
        let (v3c, v40, v44) = (r.i32()?, r.i32()?, r.i32()?);
        let player_values = if flags & 0x10 == 0 { Some(ints(r)?) } else { None };
        Ok(Self { flags, extended_flags, v32, v33, v34, v35, v30, v20, v24, v28, v2c, s48, s64, s80, s04, s9c, destinations, v38, items, suppression, side_xp, v3c, v40, v44, player_values })
    }
}

fn ints<const N: usize>(r: &mut Reader) -> Result<[i32; N]> {
    let mut values = [0; N];
    for value in &mut values { *value = r.i32()?; }
    Ok(values)
}

/// `FUN_1003f5fc`: own header, requested identity A, action 0x69, zero param/B and empty text.
pub fn request(char_id: i32, target: Identity) -> Vec<u8> {
    action::character_action(char_id, &CharacterAction { action: 0x69, param: 0, identity_a: target, identity_b: Identity::default(), text: String::new() })
}
pub fn decode(h: &N3Header, r: &mut Reader) -> Result<Option<InfoPacket>> {
    if h.msg_type != INFO_PACKET { return Ok(None); }
    InfoPacket::read(r).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn request_and_legacy_response_wire() {
        let target = Identity { kind: 50000, instance: 77 };
        let bytes = request(42, target);
        assert_eq!(bytes, [0x5e,0x47,0x77,0x70,0,0,0xc3,0x50,0,0,0,42,0, 0,0,0,0x69, 0,0,0,0, 0,0,0xc3,0x50,0,0,0,77, 0,0,0,0,0,0,0,0,0,0]);
        assert_eq!(super::super::misc::key("InfoPacketIIR_t"), INFO_PACKET);
        // Legacy format inserts an obsolete byte after v34, not an extended-flags byte.
        let mut body = vec![0x10, 1, 2, 3, 0xff, 4, 0, 5];
        body.extend([0; 16]);
        body.extend([0; 8]);
        body.extend([0; 12]);
        let mut r = Reader::new(&body);
        let packet = InfoPacket::read(&mut r).unwrap();
        assert_eq!((packet.v32, packet.v33, packet.v34, packet.v35, packet.v30), (1,2,3,4,5));
        assert_eq!(r.remaining(), 0);
        for end in 0..body.len() { assert!(InfoPacket::read(&mut Reader::new(&body[..end])).is_err()); }
    }

    #[test]
    fn extended_response_optional_sections() {
        let mut w = crate::wire::Writer::default();
        for byte in [0x6f, 1, 2, 3, 4, 5] { w.u8(byte); }
        w.i16(125);
        for value in [11, 12, 13, 14] { w.i32(value); }
        for value in ["first", "last", "title", "description", "rank"] { w.str_i16(value); }
        w.i32(0x7e2); // one grid destination
        w.i32(100);
        Identity { kind: 50000, instance: 99 }.write(&mut w);
        w.str_i16("city");
        w.i32(20);
        w.i32(21);
        w.i32(22); // city id
        w.i32(0x7e2); // one ACG item, four wire words
        for value in [30, 0, 25, 0] { w.i32(value); }
        w.i32(3600);
        w.u8(2);
        for value in 0..12 { w.i32(value); }
        for value in [40, 41, 42] { w.i32(value); }
        for value in 50..58 { w.i32(value); }
        let mut r = Reader::new(&w.0);
        let packet = InfoPacket::read(&mut r).unwrap();
        assert_eq!(packet.s04, "description");
        assert_eq!(packet.s48, "first");
        assert_eq!(packet.destinations[0].name, "city");
        assert_eq!(packet.items[0], AcgItem { low_id: 30, high_id: 30, level: 25 });
        assert_eq!(packet.v38, Some(22));
        assert_eq!(packet.suppression, Some((3600, 2)));
        assert_eq!(packet.side_xp.unwrap()[11], 11);
        assert_eq!(packet.player_values.unwrap()[7], 57);
        assert_eq!(r.remaining(), 0);
        for end in 0..w.0.len() { assert!(InfoPacket::read(&mut Reader::new(&w.0[..end])).is_err()); }
    }
}
