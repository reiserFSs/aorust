//! The client's resource name table: rdb 1000010 id 1 maps `(rdb type, id)` to the original file name
//! (`athrox_run_01_01.ani`, `head_solitusmale03.abiff`, `hands_solitusmale_african_naked.png`, …).
//! See `docs/formats.md` § characters.

use anyhow::{ensure, Context, Result};
use ao_rdb::RecordStore;
use std::collections::HashMap;

const NAME_TABLE_TYPE: u32 = 1000010;

/// `(rdb type, id) <-> file name` for meshes (1010001), models (1010002), clips (1010003),
/// textures (1010004, 1010008, 1010011).
pub struct NameTable {
    by_id: HashMap<(u32, u32), String>,
    by_name: HashMap<(u32, String), u32>,
}

impl NameTable {
    pub fn load(store: &RecordStore) -> Result<Self> {
        let bytes = store.get(NAME_TABLE_TYPE, 1)?.context("no name table (rdb 1000010 id 1)")?;
        Self::parse(&bytes)
    }

    /// `u32 version (7)`, then sections `{ u32 rdb_type; u32 n; n × { u32 id; u32 len; char name[len] } }`
    /// up to the end of the record (a short trailer remains).
    pub fn parse(b: &[u8]) -> Result<Self> {
        let u32_at = |o: usize| -> Result<usize> {
            let s = b.get(o..o + 4).context("name table truncated")?;
            Ok(u32::from_le_bytes(s.try_into().unwrap()) as usize)
        };
        ensure!(u32_at(0)? == 7, "unknown name table version {}", u32_at(0)?);
        let (mut by_id, mut by_name) = (HashMap::new(), HashMap::new());
        let mut o = 4;
        while o + 8 <= b.len() {
            let (ty, n) = (u32_at(o)? as u32, u32_at(o + 4)?);
            o += 8;
            for _ in 0..n {
                let (id, len) = (u32_at(o)? as u32, u32_at(o + 4)?);
                let name = String::from_utf8_lossy(b.get(o + 8..o + 8 + len).context("name table truncated")?).into_owned();
                o += 8 + len;
                by_name.entry((ty, name.clone())).or_insert(id);
                by_id.insert((ty, id), name);
            }
        }
        Ok(Self { by_id, by_name })
    }

    pub fn name(&self, rdb_type: u32, id: u32) -> Option<&str> {
        self.by_id.get(&(rdb_type, id)).map(String::as_str)
    }

    pub fn id(&self, rdb_type: u32, name: &str) -> Option<u32> {
        // the lookup key owns its string: one small allocation per query
        self.by_name.get(&(rdb_type, name.to_string())).copied()
    }

    /// All `(id, name)` of one rdb type, unordered.
    pub fn entries(&self, rdb_type: u32) -> impl Iterator<Item = (u32, &str)> {
        self.by_id.iter().filter(move |((t, _), _)| *t == rdb_type).map(|((_, id), n)| (*id, n.as_str()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn section(ty: u32, ents: &[(u32, &str)]) -> Vec<u8> {
        let mut v = [ty, ents.len() as u32].iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<_>>();
        for (id, n) in ents {
            v.extend(id.to_le_bytes());
            v.extend((n.len() as u32).to_le_bytes());
            v.extend(n.as_bytes());
        }
        v
    }

    #[test]
    fn sections_and_trailer() {
        let mut b = 7u32.to_le_bytes().to_vec();
        b.extend(section(1010003, &[(10078, "athrox_walk_01_01.ani"), (9, "x.ani")]));
        b.extend(section(1010011, &[(9240, "hands_athroxmale_naked.png")]));
        b.extend([0; 4]); // the real record ends with a few stray bytes
        let t = NameTable::parse(&b).unwrap();
        assert_eq!(t.name(1010003, 10078), Some("athrox_walk_01_01.ani"));
        assert_eq!(t.id(1010011, "hands_athroxmale_naked.png"), Some(9240));
        assert_eq!(t.id(1010003, "hands_athroxmale_naked.png"), None);
        assert_eq!(t.entries(1010003).count(), 2);
        assert!(NameTable::parse(&b[..b.len() - 9]).is_err());
    }
}
