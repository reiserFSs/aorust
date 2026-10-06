//! Server messages that place or push the own character without being a `CharDCMove` (which the client drops for its own
//! dynel, docs/zone/movement.md §3.1): `SetPosIIR_c`, `ImpulseIIR_c`, `ResurrectIIR_t`. Layouts and addresses: docs/zone/movement.md §10.
//! None of them is in the live captures; the tests build frames with the encoders below.
//!
//! Like [`super::action::parse_social_body`] the parsers take the bytes after the 13-byte N3 header (`N3::Unknown`).

use crate::msg::Identity;
use crate::wire::{Reader, Writer};
use anyhow::{bail, Result};

/// `SetPosIIR_c` (vtable [GC 0x101612e0], read `FUN_10076df2`, apply `FUN_10076e5a`).
pub const SET_POS: u32 = 0x195E496E;
/// `ImpulseIIR_c` (vtable [GC 0x10160fe0], read `FUN_10074731`, apply `FUN_100745bb`).
pub const IMPULSE: u32 = 0x5F4A4C6C;
/// `ResurrectIIR_t` (vtable [GC 0x10161290], read [GC 0x1007696a], apply `FUN_100769bd`).
pub const RESURRECT: u32 = 0x445F2A0B;

/// `SetPosIIR_c`: `Vec3 pos` (object `+0x18`), `u8` (`+0x24`: `UpdateLastAllowedPosition` bookkeeping), `i32` (`+0x28`: for the client
/// char non-zero prints `Feedback_CrowdLimiting`), `u8` (`+0x2c`: full stop if the dynel is moving, `FUN_10059ae5(1)`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SetPos {
    pub pos: [f32; 3],
    pub last_allowed: bool,
    pub crowd_limit: i32,
    pub stop: bool,
}

/// One element of `ImpulseIIR_c` (24 bytes: `Identity`, `Vec3`, `f32`): `Vehicle_t::Impulse(delta, time)` on that dynel's vehicle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Push {
    pub target: Identity,
    /// Horizontal displacement of the landing point (`x`, `z`; the `y` part is overwritten by the vehicle's own height, Vehicle.dll 0x1000cd61).
    pub delta: [f32; 3],
    /// Flight time in seconds (`BallisticPath_t` duration).
    pub time: f32,
}

/// `ResurrectIIR_t`: `SetStat(Health 0x1b, health)`, `SetStat(CurrentNano 0xd6, nano)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resurrect {
    pub health: i32,
    pub nano: i32,
}

fn vec3(r: &mut Reader) -> Result<[f32; 3]> {
    Ok([r.f32()?, r.f32()?, r.f32()?])
}

fn done(r: &Reader, what: &str) -> Result<()> {
    if r.remaining() != 0 {
        bail!("{} trailing bytes after {what}", r.remaining());
    }
    Ok(())
}

pub fn parse_set_pos(body: &[u8]) -> Result<SetPos> {
    let mut r = Reader::new(body);
    let m = SetPos { pos: vec3(&mut r)?, last_allowed: r.u8()? != 0, crowd_limit: r.i32()?, stop: r.u8()? != 0 };
    done(&r, "SetPos")?;
    Ok(m)
}

pub fn parse_impulse(body: &[u8]) -> Result<Vec<Push>> {
    let mut r = Reader::new(body);
    let n = r.i32()?;
    if n < 0 || n as usize > r.remaining() / 24 {
        bail!("Impulse element count {n} does not fit {} bytes", r.remaining());
    }
    let v = (0..n).map(|_| Ok(Push { target: Identity::read(&mut r)?, delta: vec3(&mut r)?, time: r.f32()? })).collect::<Result<_>>()?;
    done(&r, "Impulse")?;
    Ok(v)
}

pub fn parse_resurrect(body: &[u8]) -> Result<Resurrect> {
    let mut r = Reader::new(body);
    let m = Resurrect { health: r.i32()?, nano: r.i32()? };
    done(&r, "Resurrect")?;
    Ok(m)
}

fn header(msg: u32, target: Identity) -> Writer {
    let mut w = Writer::default();
    w.u32(msg);
    target.write(&mut w);
    w.u8(1);
    w
}

/// Full N3 payload of a `SetPosIIR_c` for `target` (tests, server emulation).
pub fn set_pos(target: Identity, m: &SetPos) -> Vec<u8> {
    let mut w = header(SET_POS, target);
    m.pos.iter().for_each(|&f| w.f32(f));
    w.u8(m.last_allowed as u8);
    w.i32(m.crowd_limit);
    w.u8(m.stop as u8);
    w.0
}

/// Full N3 payload of an `ImpulseIIR_c` whose header dynel is `target`.
pub fn impulse(target: Identity, pushes: &[Push]) -> Vec<u8> {
    let mut w = header(IMPULSE, target);
    w.i32(pushes.len() as i32);
    for p in pushes {
        p.target.write(&mut w);
        p.delta.iter().for_each(|&f| w.f32(f));
        w.f32(p.time);
    }
    w.0
}

/// Full N3 payload of a `ResurrectIIR_t`.
pub fn resurrect(target: Identity, m: &Resurrect) -> Vec<u8> {
    let mut w = header(RESURRECT, target);
    w.i32(m.health);
    w.i32(m.nano);
    w.0
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: Identity = Identity { kind: 50000, instance: 7 };

    #[test]
    fn set_pos_round_trip() {
        let m = SetPos { pos: [1.0, 2.0, 3.0], last_allowed: true, crowd_limit: 2, stop: true };
        let p = set_pos(ME, &m);
        assert_eq!(&p[..4], &SET_POS.to_be_bytes());
        assert_eq!(parse_set_pos(&p[13..]).unwrap(), m);
        assert!(parse_set_pos(&p[13..p.len() - 1]).is_err());
        assert!(parse_set_pos(&[p.clone(), vec![0]].concat()[13..]).is_err());
    }

    #[test]
    fn impulse_round_trip_and_bounds() {
        let v = [Push { target: ME, delta: [4.0, 0.0, -2.0], time: 0.8 }, Push { target: Identity { kind: 50000, instance: 9 }, delta: [0.0; 3], time: 1.0 }];
        let p = impulse(ME, &v);
        assert_eq!(parse_impulse(&p[13..]).unwrap(), v);
        // a count larger than the payload is refused before allocating
        let mut bad = p[13..].to_vec();
        bad[..4].copy_from_slice(&0x7fff_ffffi32.to_be_bytes());
        assert!(parse_impulse(&bad).is_err());
        assert!(parse_impulse(&p[13..p.len() - 3]).is_err());
    }

    #[test]
    fn resurrect_round_trip() {
        let m = Resurrect { health: 120, nano: 40 };
        assert_eq!(parse_resurrect(&resurrect(ME, &m)[13..]).unwrap(), m);
    }
}
