//! RDB item event lists: GC `FUN_1002b297`, `FUN_100a6c58`.
//! RDB BinaryStream integers are LE, unlike network BE. Read directly in LE so
//! SpellData string bytes remain intact (whole-record word swapping is invalid).
use anyhow::{bail, ensure, Result};
use ao_net::{n3::spells::{read_spell, Spell}, wire::Reader};

fn count(r: &mut Reader<'_>, max: usize) -> Result<usize> {
    let w = r.i32()?;
    ensure!(w > 0 && w % 1009 == 0, "invalid item list size word {w}");
    let n = (w / 1009 - 1) as usize;
    ensure!(n <= max, "item list count {n} exceeds {max}");
    Ok(n)
}

/// Decode the requested event, validating every element, including those after it.
/// Unsupported elements and malformed lists are errors, never absent modifiers.
pub(super) fn spells(record: &[u8], list: u32) -> Result<Vec<Spell>> {
    Ok(data(record, list)?.spells)
}

type SkillPairs = Vec<(u32, i32)>;

#[derive(Default, Clone)]
pub(super) struct Data {
    pub spells: Vec<Spell>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub pairs: Vec<(u32, u32, SkillPairs)>,
    pub criteria: Vec<(u32, u32, Vec<[i32; 3]>)>,
}

pub(super) fn data(record: &[u8], list: u32) -> Result<Data> {
    let mut r = Reader::little_endian(record);
    let kind = r.u32()?;
    ensure!(kind >= 10000, "invalid item kind {kind}");
    let elements = r.i32()?;
    ensure!(elements > 0 && elements as usize <= r.remaining() / 8, "invalid item element count {elements}");
    let mut result = Data::default();
    for index in 0..elements {
        let (ty, sub) = (r.u32()?, r.u32()?);
        ensure!(ty <= 0x32 && sub <= 0x36, "invalid item element ({ty}, {sub})");
        ensure!(index != 0 || (ty, sub) == (15, 23), "item starts without stat list");
        match ty {
            2 => {
                // FUN_100a6c58 permits n < 1000, not n <= 1000.
                let n = count(&mut r, 999)?;
                ensure!(n <= r.remaining() / 32, "spell list does not fit item record");
                for _ in 0..n {
                    let spell = read_spell(&mut r)?;
                    if sub == list { result.spells.push(spell); }
                }
            }
            4 | 19 => {
                // FUN_1002e21e / 1002e123 / 1002e036: skill key/value pairs.
                if ty == 4 {
                    let max = r.remaining() / 8;
                    let n = count(&mut r, max)?;
                    for _ in 0..n {
                        let key = r.u32()?;
                        let m = count(&mut r, 10000)?;
                        let mut pairs = Vec::with_capacity(m);
                        for _ in 0..m { pairs.push((r.u32()?, r.i32()?)); }
                        result.pairs.retain(|entry|entry.1!=key);
                        result.pairs.push((sub, key, pairs));
                    }
                } else {
                    let n = count(&mut r, 10000)?;
                    r.bytes(n * 8)?;
                }
            }
            6 if matches!(sub, 9 | 17 | 27 | 41) => {
                // FUN_1002b8b6: sized Identity_t pairs.
                let n = count(&mut r, 30000)?;
                r.bytes(n * 8)?;
            }
            15 if matches!(sub, 23 | 24 | 43) => {
                let max = r.remaining() / 8;
                let n = count(&mut r, max)?;
                r.bytes(n * 8)?;
            }
            21 => {
                let name = r.u16()? as usize;
                let description = r.u16()? as usize;
                result.name = Some(r.bytes(name)?.iter().map(|&b| b as char).collect());
                result.description = Some(r.bytes(description)?.iter().map(|&b| b as char).collect());
            }
            14 | 18 | 20 => {
                // FUN_1007d59f / 1007d6d5: key -> sized integer list.
                let max = r.remaining() / 8;
                let n = count(&mut r, max)?;
                for _ in 0..n {
                    r.u32()?;
                    let m = count(&mut r, 30000)?;
                    r.bytes(m * 4)?;
                }
            }
            22 => {
                // FUN_1008a007: attribute key -> sized Criterion_t triples.
                let n = count(&mut r, 998)?;
                for _ in 0..n {
                    let key = r.u32()?;
                    let m = count(&mut r, 30000)?;
                    let mut criteria = Vec::with_capacity(m);
                    for _ in 0..m {
                        let stat = r.i32()?;
                        let value = r.i32()?;
                        let op = r.u32()?;
                        ensure!(op < 0x91, "invalid item criterion operator {op}");
                        criteria.push([stat, value, op as i32]);
                    }
                    result.criteria.retain(|entry|entry.1!=key);
                    result.criteria.push((sub, key, criteria));
                }
            }
            3 => {} // FUN_1002b297 consumes no payload for this element.
            _ => bail!("unsupported item element ({ty:#x}, {sub:#x})"),
        }
    }
    // FUN_1002b297 stops after element count; derived item readers may own a trailer.
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rdb_reader_preserves_unaligned_string_bytes() {
        let mut bytes = 3i32.to_le_bytes().to_vec();
        bytes.extend(b"abc");
        bytes.extend(42u32.to_le_bytes());
        let mut r = Reader::little_endian(&bytes);
        assert_eq!(r.str_i32(3).unwrap(), "abc");
        assert_eq!(r.u32().unwrap(), 42);
        assert_eq!(r.remaining(), 0);
    }

    #[test]
    fn bounded_item_event_list() {
        let words: [u32; 17] = [0xc73d, 2, 15, 23, 1009, 2, 24, 2018, 53045, 0, 4, 0, 1, 0, 2, 9, 108];
        let mut bytes: Vec<_> = words.into_iter().flat_map(u32::to_le_bytes).collect();
        bytes.extend(2i32.to_le_bytes());
        let s = spells(&bytes, 24).unwrap();
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].stat(0), 108);
        assert_eq!(s[0].stat(39), 2);
        assert!(spells(&bytes, 25).unwrap().is_empty());
        for n in 0..bytes.len() { assert!(spells(&bytes[..n], 24).is_err()); }
        for word in [0u32, 1008, 1009 * 1001, u32::MAX] {
            let mut malformed = bytes.clone();
            malformed[28..32].copy_from_slice(&word.to_le_bytes());
            assert!(spells(&malformed, 24).is_err());
        }
    }

    #[test]
    fn item_metadata_keeps_description_pairs_and_criteria() {
        let mut bytes: Vec<_> = [0xc73d_u32,4,15,23,1009,21,33].into_iter().flat_map(u32::to_le_bytes).collect();
        bytes.extend(1u16.to_le_bytes());
        bytes.extend(4u16.to_le_bytes());
        bytes.extend([b'N',0xe9,b'\\',b'n',b'D']);
        bytes.extend([4_u32,4,2018,12,2018,108,2,22,5,2018,0,2018,108,2,2].into_iter().flat_map(u32::to_le_bytes));
        let parsed=data(&bytes,u32::MAX).unwrap();
        assert_eq!(parsed.name.as_deref(),Some("N"));
        assert_eq!(parsed.description.as_deref(),Some("é\\nD"));
        assert_eq!(parsed.pairs,vec![(4,12,vec![(108,2)])]);
        assert_eq!(parsed.criteria,vec![(5,0,vec![[108,2,2]])]);
        for end in 0..bytes.len() { assert!(data(&bytes[..end],u32::MAX).is_err()); }
    }

    #[test]
    fn original_tower_item_record() {
        let Some(home) = std::env::var_os("HOME") else { return };
        let dir = std::path::PathBuf::from(home).join("Games/ProjectRubiKa/client");
        if !dir.join("cd_image/rdb.db").exists() { return; }
        let store = ao_rdb::RecordStore::open(&dir).unwrap();
        let record = store.get(1000020, 201534).unwrap().unwrap();
        for (event, stat) in [(24, 91), (26, 92)] {
            let s = spells(&record, event).unwrap();
            assert_eq!(s.len(), 1);
            assert_eq!(s[0].function, 0xcf35);
            assert_eq!((s[0].stat(0), s[0].stat(39), s[0].stat(32)), (stat, 1, 2));
        }
        assert!(spells(&record, 25).unwrap().is_empty());
        // This record also exercises animation/sound maps, skills and identities.
        let record = store.get(1000020, 205411).unwrap().unwrap();
        for event in [24, 25, 26] {
            spells(&record, event).unwrap();
        }
    }
}
