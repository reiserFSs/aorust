//! Replicated effects and named sounds. Retail layouts/apply: docs/zone/misc.md §16.
use super::{misc::Vec3, N3Header};
use crate::{msg::Identity, wire::Reader};
use anyhow::{bail, Result};

pub const GFX_TRIGGER: u32 = 0x7A22_2202;
pub const PLAY_SOUND: u32 = 0x455D_2938;

#[derive(Debug, Clone, PartialEq)]
pub struct GfxTrigger {
    pub effect: i32,
    pub placement: Placement,
}

/// Wire forms 1..6; the integer on character forms is passed unchanged to CreateEffect2.
#[derive(Debug, Clone, PartialEq)]
pub enum Placement {
    Position(Vec3),
    Character { character: Identity, argument: i32 },
    Positions { source: Vec3, target: Vec3 },
    PositionToCharacter { character: Identity, source: Vec3 },
    CharacterToPosition { character: Identity, target: Vec3, argument: i32 },
    /// Retail apply resolves `source` twice, despite reading a separate `target`.
    Characters { source: Identity, target: Identity, argument: i32 },
    /// Retail reads only form + effect and applies nothing for other form values.
    Unknown(i32),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaySound {
    pub name: String,
    pub identity: Identity,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Effects {
    GfxTrigger(GfxTrigger),
    PlaySound(PlaySound),
}

pub fn decode(h: &N3Header, r: &mut Reader) -> Result<Option<Effects>> {
    Ok(Some(match h.msg_type {
        GFX_TRIGGER => {
            let form = r.i32()?;
            let effect = r.i32()?;
            let placement = match form {
                1 => Placement::Position(Vec3::read(r)?),
                2 => Placement::Character { character: Identity::read(r)?, argument: r.i32()? },
                3 => Placement::Positions { source: Vec3::read(r)?, target: Vec3::read(r)? },
                4 => Placement::PositionToCharacter { character: Identity::read(r)?, source: Vec3::read(r)? },
                5 => Placement::CharacterToPosition { character: Identity::read(r)?, target: Vec3::read(r)?, argument: r.i32()? },
                6 => Placement::Characters { source: Identity::read(r)?, target: Identity::read(r)?, argument: r.i32()? },
                _ => Placement::Unknown(form),
            };
            Effects::GfxTrigger(GfxTrigger { effect, placement })
        }
        PLAY_SOUND => {
            let length = r.i32()?;
            if !(1..=1000).contains(&length) {
                bail!("PlaySound filename length {length} outside 1..=1000");
            }
            // Retail appends NUL itself and assigns using strlen: embedded NUL ends the name.
            let bytes = r.bytes(length as usize)?;
            let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
            let name = String::from_utf8_lossy(&bytes[..end]).into_owned();
            Effects::PlaySound(PlaySound { name, identity: Identity::read(r)? })
        }
        _ => return Ok(None),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::Writer;

    #[test]
    fn retail_layouts_and_every_truncated_prefix() {
        let character = Identity { kind: 50000, instance: 42 };
        let other = Identity { kind: 50000, instance: 43 };
        let a = Vec3 { x: 1.0, y: 2.0, z: 3.0 };
        let b = Vec3 { x: 4.0, y: 5.0, z: 6.0 };
        let placements = [
            Placement::Position(a),
            Placement::Character { character, argument: -1 },
            Placement::Positions { source: a, target: b },
            Placement::PositionToCharacter { character, source: a },
            Placement::CharacterToPosition { character, target: a, argument: -1 },
            Placement::Characters { source: character, target: other, argument: -1 },
        ];
        // Synthetic fixtures follow GC reader 100396bd/writer 1003977b, not a live capture.
        for (i, placement) in placements.into_iter().enumerate() {
            let mut w = Writer::default();
            w.i32(i as i32 + 1);
            w.i32(123);
            match &placement {
                Placement::Position(v) => v.write(&mut w),
                Placement::Character { character, argument } => { character.write(&mut w); w.i32(*argument); }
                Placement::Positions { source, target } => { source.write(&mut w); target.write(&mut w); }
                Placement::PositionToCharacter { character, source } => { character.write(&mut w); source.write(&mut w); }
                Placement::CharacterToPosition { character, target, argument } => { character.write(&mut w); target.write(&mut w); w.i32(*argument); }
                Placement::Characters { source, target, argument } => { source.write(&mut w); target.write(&mut w); w.i32(*argument); }
                Placement::Unknown(_) => unreachable!(),
            }
            let h = N3Header { msg_type: GFX_TRIGGER, target: character, flag: 0 };
            let mut r = Reader::new(&w.0);
            assert_eq!(decode(&h, &mut r).unwrap(), Some(Effects::GfxTrigger(GfxTrigger { effect: 123, placement })));
            assert_eq!(r.remaining(), 0);
            for n in 0..w.0.len() {
                assert!(decode(&h, &mut Reader::new(&w.0[..n])).is_err(), "form {} prefix {n}", i + 1);
            }
        }
        let h = N3Header { msg_type: PLAY_SOUND, target: character, flag: 0 };
        let mut w = Writer::default();
        w.i32(8);
        w.bytes(b"foo.wav\0");
        other.write(&mut w);
        assert_eq!(decode(&h, &mut Reader::new(&w.0)).unwrap(), Some(Effects::PlaySound(PlaySound { name: "foo.wav".into(), identity: other })));
        for n in 0..w.0.len() {
            assert!(decode(&h, &mut Reader::new(&w.0[..n])).is_err());
        }
        for length in [-1i32, 0, 1001, i32::MAX] {
            assert!(decode(&h, &mut Reader::new(&length.to_be_bytes())).is_err());
        }
        assert_eq!(super::super::misc::key("GfxTriggerIIR_t"), GFX_TRIGGER);
        assert_eq!(super::super::misc::key("PlaySoundIIR_c"), PLAY_SOUND);
        for bytes in [&b"a\0b"[..], &b"abc"[..]] {
            let mut w = Writer::default();
            w.i32(bytes.len() as i32);
            w.bytes(bytes);
            other.write(&mut w);
            let mut r = Reader::new(&w.0);
            let Some(Effects::PlaySound(sound)) = decode(&h, &mut r).unwrap() else { panic!("sound") };
            assert_eq!(sound.name, if bytes.contains(&0) { "a" } else { "abc" });
            assert_eq!(r.remaining(), 0);
        }
    }
}
