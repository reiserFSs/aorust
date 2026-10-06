//! Mission terminal messages. GC 0x1001620e/0x100c9b9a generate; 0x100cacdb alternatives;
//! 0x1001738d/0x100cab36 select. Dimension bytes: GameData 0x1000224b/0x100022fd.
use super::{quest::{self, Quest}, N3Header};
use crate::{msg::Identity, wire::{Reader, Writer}};
use anyhow::{ensure, Result};

pub const ALTERNATIVES: u32 = 0x5c436609;
pub const CREATE: u32 = 0x291f361b;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GenerateInfo {
    pub difficulty: u8,
    pub dimensions: [i8; 6],
    pub origin_type: u8,
    pub origin: Identity,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Alternative {
    pub quest: Quest,
    pub kind: u8,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Alternatives {
    pub difficulty: u8,
    pub dimensions: [i8; 6],
    pub seed: i32,
    pub origin_type: u8,
    pub origin: Identity,
    pub missions: Vec<Alternative>,
}

fn valid_dimensions(dimensions: &[i8; 6]) -> bool {
    dimensions.iter().all(|v| (-100..=100).contains(v))
}

fn header(w: &mut Writer, key: u32, own: Identity) {
    w.u32(key);
    own.write(w);
    w.u8(0);
}

/// UI difficulty 0..100 -> floor(difficulty * f32(0.10891088843345642) + 1), GC 0x1001620e.
pub fn generate(own: Identity, info: &GenerateInfo) -> Result<Vec<u8>> {
    ensure!(own.kind == 50000 && own.instance != 0, "invalid mission character");
    ensure!(info.difficulty <= 100 && valid_dimensions(&info.dimensions), "invalid mission sliders");
    ensure!((1..=8).contains(&info.origin_type) && info.origin != Identity::default(), "invalid mission origin");
    let mut w = Writer::default();
    header(&mut w, ALTERNATIVES, own);
    w.u8(4);
    w.u8((f32::from(info.difficulty) * 0.10891089 + 1.0).floor() as u8);
    for v in info.dimensions { w.u8(v as u8); }
    w.i32(0); // ACGQuestIIR constructor's seed; the response supplies the generated seed.
    w.u8(info.origin_type);
    info.origin.write(&mut w);
    w.u8(0); // no alternatives in a generation request
    Ok(w.0)
}

pub fn select(own: Identity, mission: Identity) -> Vec<u8> {
    let mut w = Writer::default();
    header(&mut w, CREATE, own);
    mission.write(&mut w);
    w.0
}

pub fn decode(h: &N3Header, r: &mut Reader<'_>) -> Result<Option<Alternatives>> {
    if h.msg_type != ALTERNATIVES { return Ok(None); }
    ensure!(h.target.kind == 50000 && h.target.instance != 0, "invalid mission character");
    ensure!(r.u8()? == 4, "invalid QuestAlternative version");
    let difficulty = r.u8()?;
    ensure!((1..=11).contains(&difficulty), "invalid mission difficulty");
    let mut dimensions = [0; 6];
    for v in &mut dimensions { *v = r.u8()? as i8; }
    ensure!(valid_dimensions(&dimensions), "invalid mission dimensions");
    let seed = r.i32()?;
    let origin_type = r.u8()?;
    let origin = Identity::read(r)?;
    ensure!((1..=8).contains(&origin_type) && origin != Identity::default(), "invalid mission origin");
    let count = r.u8()?;
    ensure!(count < 6, "too many mission alternatives");
    let mut missions = Vec::with_capacity(usize::from(count));
    for _ in 0..count {
        let id = Identity::read(r)?;
        missions.push(Alternative { quest: quest::quest_body(r, id)?, kind: r.u8()? });
    }
    Ok(Some(Alternatives { difficulty, dimensions, seed, origin_type, origin, missions }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn generation_and_selection_match_original_streams() {
        let own = Identity { kind: 50000, instance: 1 };
        let info = GenerateInfo { difficulty: 50, dimensions: [-100, 100, 0, -1, 1, 42], origin_type: 1, origin: Identity { kind: 51000, instance: 2 } };
        let p = generate(own, &info).unwrap();
        assert_eq!(&p[13..], &[4, 6, 156, 100, 0, 255, 1, 42, 0, 0, 0, 0, 1, 0, 0, 199, 56, 0, 0, 0, 2, 0]);
        let (h, mut r) = N3Header::parse(&p).unwrap();
        let m = decode(&h, &mut r).unwrap().unwrap();
        assert_eq!(m.dimensions, info.dimensions);
        assert!(m.missions.is_empty());
        let mut response = p.clone();
        response[34] = 1;
        response.extend(quest::tests::sample(15, "Terminal mission", 123));
        response.push(3);
        let (h, mut r) = N3Header::parse(&response).unwrap();
        let m = decode(&h, &mut r).unwrap().unwrap();
        assert_eq!((m.missions[0].quest.name.as_str(), m.missions[0].quest.icon, m.missions[0].kind), ("Terminal mission", 123, 3));
        for end in 35..response.len() {
            let (h, mut r) = N3Header::parse(&response[..end]).unwrap();
            assert!(decode(&h, &mut r).is_err());
        }
        for end in 13..p.len() { let (h, mut r) = N3Header::parse(&p[..end]).unwrap(); assert!(decode(&h, &mut r).is_err()); }
        let mut bad = p.clone(); bad[34] = 6;
        let (h, mut r) = N3Header::parse(&bad).unwrap(); assert!(decode(&h, &mut r).is_err());
        let mission = Identity { kind: 56003, instance: 9 };
        assert_eq!(&select(own, mission)[13..], &[0, 0, 218, 195, 0, 0, 0, 9]);
    }
}
