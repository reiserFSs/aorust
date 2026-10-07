//! The character-select appearance cache `prefs/CharacterViewer.xml` of `CharacterViewerModule_c` [GUI.dll], see
//! `docs/formats.md` § characters → *Appearance cache*.
//!
//! * read once by the module's constructor (`LoadCache` 0x100065a4), rewritten on `SlotShuttingDown` / `SlotConfigurationSaved`
//!   after `UpdateCache` (0x100069ed) stored the live visuals of the active `CharacterID` (`SaveCache` 0x100060c0), and on destruction;
//! * the file is a `Message` dumped by `Message::DumpToXML` [Utils.dll 0x1000a0e4]: root `<Archive code="0">`, every value an
//!   element named after its type with `name`/`value`, nested messages `<Archive code="0" name="…">`, several values of one
//!   name wrapped in `<Array name="…">` (the children carry no `name`).

use super::player::{ClothPart, Equipment};
use anyhow::{bail, ensure, Result};
use std::{collections::BTreeMap, path::Path};

/// One `Cloth` message: a worn texture of a body slot (`ClothData_t`, 0x14 bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClothEntry {
    /// [`ClothPart`] index.
    pub body_part: i32,
    /// rdb 1010004 texture drawn over the skin (layer 2); 0 = nothing.
    pub texture: i32,
    /// Second/env texture (layer 3); 0 = none.
    pub env_texture: i32,
    pub alpha_mode: i32,
    pub priority: i32,
}

/// One `Mesh` message: a static mesh on an attractor (`AttractorMeshData_t`, 0xc bytes); attractor 0 is the head.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MeshEntry {
    pub attractor: i8,
    pub flags: i8,
    /// rdb 1010001 mesh.
    pub mesh_id: i32,
    pub texture_id: i32,
}

/// One `Character` message (`CharacterViewerData_t` + the character id).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CachedCharacter {
    /// `CharacterData_t.id` (the `CharacterID` DValue).
    pub id: i32,
    /// `_time64` of the capture; a newer entry wins when the file is merged.
    pub time: i32,
    /// Resolved body model, rdb 1010002 (thin/fat/robe variants are distinct models).
    /// Ordinary humanoids resolve breed/sex/build rather than using PRK's placeholder Mesh stat 0xc.
    pub mesh_id: i32,
    /// Stat 0x40: head mesh, rdb 1010001.
    pub head_id: i32,
    /// Stat 4 (`Breed_e`: 1 solitus … 4 atrox).
    pub breed: i32,
    /// Stat 0x3b (`BreedSex_e`: 2 male, 3 female).
    pub sex: i32,
    /// Stat 0x2f.
    pub fatness: i32,
    /// Stat 0x21 (faction).
    pub side: i32,
    pub cloth: Vec<ClothEntry>,
    pub meshes: Vec<MeshEntry>,
}

impl CachedCharacter {
    /// What the select screen puts over the skin: every `Cloth` entry in order via `SetCATTexture(part, texture, 2)`
    /// (`CharacterViewer_c::Update` 0x100054cd), so the last entry of a slot wins and texture 0 clears it.
    pub fn equipment(&self) -> Equipment {
        let mut e = Equipment::default();
        for c in &self.cloth {
            if let Some(part) = ClothPart::ALL.get(usize::try_from(c.body_part).unwrap_or(usize::MAX)) {
                e.0[*part as usize] = u32::try_from(c.texture).ok().filter(|&t| t != 0);
            }
        }
        e
    }

    /// Stores `equipment` as the live visuals do (`FUN_1004b521` @Gamecode 0x1004b521): one `Cloth` entry per slot in
    /// [`ClothPart`] order, texture 0 for an empty slot, alpha mode and priority 0.
    pub fn set_equipment(&mut self, equipment: &Equipment) {
        self.cloth = ClothPart::ALL
            .iter()
            .map(|p| ClothEntry { body_part: *p as i32, texture: equipment.0[*p as usize].map_or(0, |t| t as i32), ..Default::default() })
            .collect();
    }

    /// The head mesh the preview mounts: the attractor-0 `Mesh` entry (`CCCharacter_t::ChangeHead`), else `head_id`.
    pub fn head_mesh(&self) -> u32 {
        self.meshes.iter().find(|m| m.attractor == 0).map_or(self.head_id, |m| m.mesh_id).max(0) as u32
    }
}

/// All cached characters by id (the module's `std::map`, saved in ascending id order).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ViewerCache(pub BTreeMap<i32, CachedCharacter>);

impl ViewerCache {
    /// The client's file name inside its `prefs` directory.
    pub const FILE: &'static str = "CharacterViewer.xml";

    /// `LoadCache`: a character is merged only if all eight `Int32` fields exist (incomplete ones are skipped, as are
    /// incomplete `Cloth`/`Mesh` children); an entry replaces an existing one of the same id unless that is newer.
    /// Not well-formed XML is an error (the client's `LoadFromFile` fails and the cache stays empty).
    pub fn parse(xml: &str) -> Result<Self> {
        let root = parse_xml(xml)?;
        let mut cache = ViewerCache::default();
        for ch in named(&root, "Character") {
            let int = |n: &str| find_int(ch, n, "Int32");
            let (Some(id), Some(time), Some(mesh_id), Some(head_id), Some(breed), Some(sex), Some(fatness), Some(side)) =
                (int("ID"), int("Time"), int("MeshID"), int("HeadID"), int("Breed"), int("Sex"), int("Fatness"), int("Side"))
            else {
                continue;
            };
            if cache.0.get(&id).is_some_and(|old| (old.time as u32) > time as u32) {
                continue;
            }
            let cloth = named(ch, "Cloth")
                .into_iter()
                .filter_map(|c| {
                    let i = |n: &str| find_int(c, n, "Int32");
                    Some(ClothEntry { alpha_mode: i("AlphaMode")?, body_part: i("BodyPart")?, env_texture: i("EnvTextureId")?, priority: i("Priority")?, texture: i("TextureID")? })
                })
                .collect();
            let meshes = named(ch, "Mesh")
                .into_iter()
                .filter_map(|m| {
                    Some(MeshEntry {
                        attractor: find_int(m, "Attractor", "Int8")? as i8,
                        flags: find_int(m, "Flags", "Int8")? as i8,
                        mesh_id: find_int(m, "MeshID", "Int32")?,
                        texture_id: find_int(m, "TextureID", "Int32")?,
                    })
                })
                .collect();
            cache.0.insert(id, CachedCharacter { id, time, mesh_id, head_id, breed, sex, fatness, side, cloth, meshes });
        }
        Ok(cache)
    }

    /// `SaveCache`: `Message::SaveToFile` text (TinyXML layout, no XML declaration).
    pub fn to_xml(&self) -> String {
        let mut out = String::from("<Archive code=\"0\">\n");
        let list: Vec<&CachedCharacter> = self.0.values().collect();
        group(&mut out, 1, "Character", &list, |o, d, c, name| {
            archive(o, d, name, |o, d| {
                for (n, v) in [("ID", c.id), ("Time", c.time), ("MeshID", c.mesh_id), ("HeadID", c.head_id), ("Breed", c.breed), ("Sex", c.sex), ("Fatness", c.fatness), ("Side", c.side)] {
                    value(o, d, "Int32", Some(n), v);
                }
                let cloth: Vec<&ClothEntry> = c.cloth.iter().collect();
                group(o, d, "Cloth", &cloth, |o, d, e, name| {
                    archive(o, d, name, |o, d| {
                        for (n, v) in [("AlphaMode", e.alpha_mode), ("BodyPart", e.body_part), ("EnvTextureId", e.env_texture), ("Priority", e.priority), ("TextureID", e.texture)] {
                            value(o, d, "Int32", Some(n), v);
                        }
                    })
                });
                let meshes: Vec<&MeshEntry> = c.meshes.iter().collect();
                group(o, d, "Mesh", &meshes, |o, d, m, name| {
                    archive(o, d, name, |o, d| {
                        value(o, d, "Int8", Some("Attractor"), m.attractor as i32);
                        value(o, d, "Int8", Some("Flags"), m.flags as i32);
                        value(o, d, "Int32", Some("MeshID"), m.mesh_id);
                        value(o, d, "Int32", Some("TextureID"), m.texture_id);
                    })
                });
            })
        });
        out.push_str("</Archive>\n");
        out
    }

    /// `UpdateCache` for the active character: stores `c` unconditionally (no time check).
    pub fn update(&mut self, c: CachedCharacter) {
        self.0.insert(c.id, c);
    }

    /// The cache of `dir/CharacterViewer.xml`; missing or unreadable = empty, like the client.
    pub fn load(dir: &Path) -> Self {
        std::fs::read_to_string(dir.join(Self::FILE)).ok().and_then(|s| Self::parse(&s).ok()).unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        std::fs::write(dir.join(Self::FILE), self.to_xml())
    }
}

/// `count == 1` values are written directly, more than one inside an `<Array name>` whose children have no name.
fn group<T>(out: &mut String, depth: usize, name: &str, items: &[&T], mut item: impl FnMut(&mut String, usize, &T, Option<&str>)) {
    let pad = "    ".repeat(depth);
    match items {
        [] => {}
        [one] => item(out, depth, one, Some(name)),
        many => {
            out.push_str(&format!("{pad}<Array name=\"{name}\">\n"));
            for m in many {
                item(out, depth + 1, m, None);
            }
            out.push_str(&format!("{pad}</Array>\n"));
        }
    }
}

fn archive(out: &mut String, depth: usize, name: Option<&str>, body: impl FnOnce(&mut String, usize)) {
    let pad = "    ".repeat(depth);
    out.push_str(&format!("{pad}<Archive code=\"0\"{}>\n", name.map_or(String::new(), |n| format!(" name=\"{n}\""))));
    body(out, depth + 1);
    out.push_str(&format!("{pad}</Archive>\n"));
}

fn value(out: &mut String, depth: usize, ty: &str, name: Option<&str>, v: i32) {
    let pad = "    ".repeat(depth);
    out.push_str(&format!("{pad}<{ty}{} value=\"{v}\" />\n", name.map_or(String::new(), |n| format!(" name=\"{n}\""))));
}

struct Element {
    tag: String,
    attrs: Vec<(String, String)>,
    children: Vec<Element>,
}

impl Element {
    fn attr(&self, k: &str) -> Option<&str> {
        self.attrs.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
    }
}

/// `Message::LoadFromXML`: the children named `name`, with the children of `Array` elements of that name flattened in.
fn named<'a>(e: &'a Element, name: &str) -> Vec<&'a Element> {
    let mut v = vec![];
    for c in e.children.iter().filter(|c| c.attr("name") == Some(name)) {
        if c.tag.eq_ignore_ascii_case("Array") {
            v.extend(c.children.iter());
        } else {
            v.push(c);
        }
    }
    v
}

/// `Message::FindInt32/FindInt8`: the first element of that type and name.
fn find_int(e: &Element, name: &str, ty: &str) -> Option<i32> {
    named(e, name).into_iter().find(|c| c.tag == ty)?.attr("value")?.trim().parse().ok()
}

/// Elements and attributes only (comments and declarations skipped), the subset TinyXML wrote.
fn parse_xml(src: &str) -> Result<Element> {
    fn skip_ws(s: &[u8], i: &mut usize) {
        while *i < s.len() && s[*i].is_ascii_whitespace() {
            *i += 1;
        }
    }
    fn skip_to(s: &[u8], i: &mut usize, end: &str) -> Result<()> {
        match s[*i..].windows(end.len()).position(|w| w == end.as_bytes()) {
            Some(p) => {
                *i += p + end.len();
                Ok(())
            }
            None => bail!("xml: missing {end}"),
        }
    }
    fn name(s: &[u8], i: &mut usize) -> String {
        let st = *i;
        while *i < s.len() && !s[*i].is_ascii_whitespace() && !matches!(s[*i], b'=' | b'>' | b'/' | b'<') {
            *i += 1;
        }
        String::from_utf8_lossy(&s[st..*i]).into_owned()
    }
    fn element(s: &[u8], i: &mut usize, depth: usize) -> Result<Element> {
        ensure!(depth < 64, "xml: nesting too deep");
        *i += 1; // '<'
        let mut e = Element { tag: name(s, i), attrs: vec![], children: vec![] };
        ensure!(!e.tag.is_empty(), "xml: empty tag");
        loop {
            skip_ws(s, i);
            match s.get(*i) {
                Some(b'/') => {
                    ensure!(s.get(*i + 1) == Some(&b'>'), "xml: bad empty tag");
                    *i += 2;
                    return Ok(e);
                }
                Some(b'>') => {
                    *i += 1;
                    break;
                }
                Some(_) => {
                    let k = name(s, i);
                    skip_ws(s, i);
                    ensure!(!k.is_empty() && s.get(*i) == Some(&b'='), "xml: bad attribute");
                    *i += 1;
                    skip_ws(s, i);
                    let q = *s.get(*i).filter(|c| matches!(c, b'"' | b'\'')).ok_or_else(|| anyhow::anyhow!("xml: unquoted attribute"))?;
                    *i += 1;
                    let st = *i;
                    while *i < s.len() && s[*i] != q {
                        *i += 1;
                    }
                    ensure!(*i < s.len(), "xml: unterminated attribute");
                    let v = String::from_utf8_lossy(&s[st..*i]).replace("&quot;", "\"").replace("&apos;", "'").replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&");
                    *i += 1;
                    e.attrs.push((k, v));
                }
                None => bail!("xml: unterminated tag"),
            }
        }
        loop {
            // text content is not used by the format
            while *i < s.len() && s[*i] != b'<' {
                *i += 1;
            }
            ensure!(*i < s.len(), "xml: unterminated element {}", e.tag);
            if s[*i..].starts_with(b"<!--") {
                skip_to(s, i, "-->")?;
            } else if s[*i..].starts_with(b"</") {
                *i += 2;
                let close = name(s, i);
                ensure!(close == e.tag, "xml: </{close}> closes <{}>", e.tag);
                skip_ws(s, i);
                ensure!(s.get(*i) == Some(&b'>'), "xml: bad closing tag");
                *i += 1;
                return Ok(e);
            } else {
                e.children.push(element(s, i, depth + 1)?);
            }
        }
    }
    let (s, mut i) = (src.as_bytes(), 0);
    loop {
        skip_ws(s, &mut i);
        ensure!(i < s.len(), "xml: no root element");
        if s[i..].starts_with(b"<?") {
            skip_to(s, &mut i, "?>")?;
        } else if s[i..].starts_with(b"<!--") {
            skip_to(s, &mut i, "-->")?;
        } else {
            return element(s, &mut i, 0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> CachedCharacter {
        let mut c = CachedCharacter { id: 7, time: 1_700_000_000, mesh_id: 5907, head_id: 40681, breed: 1, sex: 2, fatness: 1, side: 2, ..Default::default() };
        let mut e = Equipment::default();
        e.wear(ClothPart::Body, 154207);
        e.wear(ClothPart::Legs, 154200);
        c.set_equipment(&e);
        c.meshes = vec![MeshEntry { attractor: 0, flags: 4, mesh_id: 40681, texture_id: 0 }];
        c
    }

    #[test]
    fn roundtrips_and_equipment_follows_the_cloth_entries() {
        let mut cache = ViewerCache::default();
        cache.update(sample());
        let mut other = sample();
        other.id = 3;
        other.meshes.clear();
        cache.update(other);
        let xml = cache.to_xml();
        assert!(xml.starts_with("<Archive code=\"0\">\n    <Array name=\"Character\">\n        <Archive code=\"0\">\n            <Int32 name=\"ID\" value=\"3\" />"));
        assert!(xml.contains("<Archive code=\"0\" name=\"Mesh\">") && xml.contains("<Int8 name=\"Attractor\" value=\"0\" />"), "single value is not wrapped in an Array");
        let back = ViewerCache::parse(&xml).unwrap();
        assert_eq!(back, cache);
        let c = &back.0[&7];
        assert_eq!(c.cloth.len(), 5);
        let e = c.equipment();
        assert_eq!((e.0[ClothPart::Body as usize], e.0[ClothPart::Legs as usize], e.0[ClothPart::Hands as usize]), (Some(154207), Some(154200), None));
        assert_eq!(c.head_mesh(), 40681);
    }

    #[test]
    fn load_skips_incomplete_entries_and_keeps_the_newer_one() {
        let xml = r#"<Archive code="0">
            <Array name="Character">
              <Archive code="0"><Int32 name="ID" value="1" /><Int32 name="Time" value="10" /></Archive>
              <Archive code="0">
                <Int32 name="ID" value="2" /><Int32 name="Time" value="20" /><Int32 name="MeshID" value="5900" /><Int32 name="HeadID" value="40098" />
                <Int32 name="Breed" value="4" /><Int32 name="Sex" value="2" /><Int32 name="Fatness" value="1" /><Int32 name="Side" value="0" />
                <Array name="Cloth">
                  <Archive code="0"><Int32 name="AlphaMode" value="0" /><Int32 name="BodyPart" value="1" /><Int32 name="EnvTextureId" value="0" /><Int32 name="Priority" value="0" /><Int32 name="TextureID" value="5" /></Archive>
                  <Archive code="0"><Int32 name="BodyPart" value="4" /></Archive>
                  <Archive code="0"><Int32 name="AlphaMode" value="0" /><Int32 name="BodyPart" value="1" /><Int32 name="EnvTextureId" value="0" /><Int32 name="Priority" value="0" /><Int32 name="TextureID" value="0" /></Archive>
                </Array>
              </Archive>
              <Archive code="0">
                <Int32 name="ID" value="2" /><Int32 name="Time" value="5" /><Int32 name="MeshID" value="1" /><Int32 name="HeadID" value="1" />
                <Int32 name="Breed" value="1" /><Int32 name="Sex" value="2" /><Int32 name="Fatness" value="1" /><Int32 name="Side" value="0" />
              </Archive>
            </Array>
          </Archive>"#;
        let c = ViewerCache::parse(xml).unwrap();
        assert_eq!(c.0.keys().copied().collect::<Vec<_>>(), [2], "id 1 lacks fields");
        let ch = &c.0[&2];
        assert_eq!((ch.time, ch.mesh_id, ch.breed), (20, 5900, 4), "the older duplicate does not replace");
        assert_eq!(ch.cloth.len(), 2, "the entry without its fields is skipped");
        assert_eq!(ch.equipment().0[ClothPart::Body as usize], None, "the last entry of a slot wins, texture 0 clears");
        assert!(ViewerCache::parse("<Archive").is_err());
        assert!(ViewerCache::parse("<a><b></a>").is_err());
        assert!(ViewerCache::parse(&"<a>".repeat(100)).is_err());
    }
}
