//! NPC / monster record (rdb 1040023, id = the `MonsterData` stat 0x167): `docs/zone/npc.md`.
//!
//! Gamecode.dll reads it in `FUN_1004dc30` (via `ResourceDatabase_t::GetBinaryStream(Identity{0xfde97, id})`, reader
//! `FUN_1004d919`) into a 0x34-byte `MonsterData` object that `n3VisualDynel` consults when a `SimpleChar` is "morphed".
//! All counts are stored as `(n + 1) * 1009` (`0x3f1`).

use super::{CatMesh, Rd};
use anyhow::{bail, ensure, Context, Result};
use ao_rdb::RecordStore;

/// rdb type of the record; the client requests `Identity{0xfde97, monster_data}`.
pub const NPC_TYPE: u32 = 1040023;
/// Stat ids of the override table the client reads with `FUN_1004d8e6(stat)` ([`NpcRecord::stat`]).
const STAT_MESH: u32 = 12;
const STAT_HEAD_MESH: u32 = 64;
/// Encoded-count multiplier (`0x3f1`).
const COUNT_MUL: u32 = 1009;

/// Animation roles of the NPC table: the `AbstractAnimID_e` keys, named after the clips the client's own humanoid
/// records file under them (`docs/zone/npc.md` § 3; the client has no symbol names for the enum).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum NpcAnim {
    Walk = 100,
    Run = 101,
    Crawl = 103,
    CrawlStart = 104,
    CrawlStop = 105,
    Idle = 0x78,
    ImpactBack = 126,
    ImpactChest = 127,
    ImpactHead = 128,
    ImpactLeftArm = 129,
    ImpactRightArm = 130,
    ImpactStomach = 132,
    Swim = 133,
    WalkLeft = 134,
    WalkRight = 135,
    WalkBack = 136,
    /// `spell-gen` / `spell-dir` / `spell-self` / `spell-sys` clips.
    SpellGeneral = 200,
    SpellDirected = 201,
    SpellSelf = 202,
    SpellSystem = 203,
    SitGroundStart = 213,
    SitGroundStop = 214,
    SitGround = 215,
    SitChairStart = 216,
    SitChairStop = 217,
    SitChair = 218,
    RunBack = 222,
    DieKnees = 500,
    DiePain = 501,
    DiePoison = 502,
    DieShot = 503,
    DieGround = 504,
    DieFloat = 505,
    /// Unarmed stance (`unarmed-start`, `idle-unarmed`, `unarmed-stop`) and its five attacks: what creatures fight with.
    UnarmedStart = 1030,
    UnarmedIdle = 1031,
    UnarmedStop = 1032,
    UnarmedAttack1 = 1033,
    UnarmedAttack2 = 1034,
    UnarmedAttack3 = 1035,
    UnarmedAttack4 = 1036,
    UnarmedAttack5 = 1037,
    /// Generic death: the fallback target of every `Die*` key (`die-pain` on the lizard).
    Die = 6000,
}

impl NpcAnim {
    /// The table key.
    pub fn key(self) -> u32 {
        self as u32
    }
}

/// A decoded NPC record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NpcRecord {
    /// `(stat id, value)` overrides in file order; the client looks a stat up linearly, first match wins.
    pub stats: Vec<(u32, i32)>,
    /// Animation multimap: `AbstractAnimID_e` key -> rdb 1010003 ids (ascending key; values in file order).
    pub anims: Vec<(u32, Vec<u32>)>,
    /// Sound multimap: `AbstractAnimID_e` key -> sound ids.
    pub sounds: Vec<(u32, Vec<u32>)>,
    /// Fourth list (`(i32, i32)` pairs; `FUN_1002b8b6`), empty in every real record surveyed.
    pub pairs: Vec<(i32, i32)>,
    /// Display name (`green lizard`).
    pub name: String,
    /// Bytes left after the name (never read by the client).
    pub trailing: usize,
}

fn count(r: &mut Rd, limit: u32) -> Result<usize> {
    let v = r.u32()?;
    ensure!(v % COUNT_MUL == 0 && v >= COUNT_MUL, "bad encoded count {v:#x}");
    let n = v / COUNT_MUL - 1;
    ensure!(n <= limit, "count {n} over {limit}");
    Ok(n as usize)
}

/// `FUN_1007d63a` / `FUN_1007d6d5`: `n` x { key, sub-count, values } inserted into a `std::multimap` (stable per key).
fn multimap(r: &mut Rd) -> Result<Vec<(u32, Vec<u32>)>> {
    let n = count(r, 30000)?;
    let mut out: Vec<(u32, Vec<u32>)> = vec![];
    for _ in 0..n {
        let key = r.u32()?;
        let m = count(r, 30000)?;
        let vals = (0..m).map(|_| r.u32()).collect::<Result<Vec<_>>>()?;
        let at = out.partition_point(|e| e.0 <= key);
        match at.checked_sub(1).filter(|&i| out[i].0 == key) {
            Some(i) => out[i].1.extend(vals),
            None => out.insert(at, (key, vals)),
        }
    }
    Ok(out)
}

impl NpcRecord {
    pub fn parse(d: &[u8]) -> Result<Self> {
        let mut r = Rd::new(d);
        // Section markers `(tag, ?)`: read and dropped by the client.
        let marker = |r: &mut Rd| -> Result<()> { r.u32().map(drop).and_then(|_| r.u32().map(drop)) };
        marker(&mut r)?;
        let n = count(&mut r, 30000)?;
        let stats = (0..n).map(|_| Ok((r.u32()?, r.u32()? as i32))).collect::<Result<Vec<_>>>().context("stats")?;
        marker(&mut r)?;
        let anims = multimap(&mut r).context("animations")?;
        marker(&mut r)?;
        let sounds = multimap(&mut r).context("sounds")?;
        marker(&mut r)?;
        let n = count(&mut r, 30000)?;
        let pairs = (0..n).map(|_| Ok((r.u32()? as i32, r.u32()? as i32))).collect::<Result<Vec<_>>>().context("pairs")?;
        let len = r.u32()? as usize;
        let name = String::from_utf8_lossy(r.bytes(len).context("name")?).into_owned();
        Ok(Self { stats, anims, sounds, pairs, name, trailing: r.remaining() })
    }

    /// Reads and decodes record `id`.
    pub fn load(store: &RecordStore, id: u32) -> Result<Self> {
        let Some(b) = store.get(NPC_TYPE, id)? else { bail!("no NPC record {NPC_TYPE}/{id}") };
        Self::parse(&b).with_context(|| format!("decoding NPC record {id}"))
    }

    /// Stat override (`FUN_1004d8e6`): the first entry of `stat`.
    pub fn stat(&self, stat: u32) -> Option<i32> {
        self.stats.iter().find(|s| s.0 == stat).map(|s| s.1)
    }

    /// Model: rdb 1010002 id (stat `Mesh` = 12). `None` when the record has no mesh override (0 = absent in the client).
    pub fn mesh(&self) -> Option<u32> {
        self.stat(STAT_MESH).filter(|&m| m != 0).map(|m| m as u32)
    }

    /// Head mesh, rdb 1010001 id (stat `HeadMesh` = 64); humanoid NPCs only.
    pub fn head_mesh(&self) -> Option<u32> {
        self.stat(STAT_HEAD_MESH).filter(|&m| m != 0).map(|m| m as u32)
    }

    /// Clips filed under `key` (the client picks one at random).
    pub fn anim_variants(&self, key: u32) -> &[u32] {
        self.anims.iter().find(|e| e.0 == key).map_or(&[], |e| &e.1)
    }

    /// The clip for `role`; see [`anim_key`].
    pub fn anim(&self, role: NpcAnim) -> Option<u32> {
        anim_key(self, role.key())
    }
}

/// `FUN_10010ebe`: the clips of `key`, else of its fallback parent (`FUN_10010ad1`), …, else the first clip of the table.
/// The client picks one of them ([`pick_variant`]); the last resort is the single first value of the first key.
pub fn anim_key_variants(rec: &NpcRecord, key: u32) -> &[u32] {
    let mut k = key;
    loop {
        let v = rec.anim_variants(k);
        if !v.is_empty() {
            return v;
        }
        let next = fallback_key(k);
        if next == k || next == key || next == 0 {
            break;
        }
        k = next;
    }
    rec.anims.first().and_then(|e| e.1.get(..1)).unwrap_or(&[])
}

/// `FUN_10010ebe` with the first variant (what `FUN_1004570c` returns when the key has one value).
pub fn anim_key(rec: &NpcRecord, key: u32) -> Option<u32> {
    anim_key_variants(rec, key).first().copied()
}

/// `FUN_1003c802` (the animation holder's resolver `FUN_1003c8b7`, reached by every state / stance / attack start): the clips of the
/// first key of the parent chain that has any; unlike [`anim_key_variants`] it has no "first entry of the table" last resort
/// and also stops at parent 0 (the name rule that follows it applies to human skeletons only).
pub fn holder_variants(rec: &NpcRecord, key: u32) -> &[u32] {
    let mut k = key;
    loop {
        let v = rec.anim_variants(k);
        if !v.is_empty() {
            return v;
        }
        let next = fallback_key(k);
        if next == k || next == 0 || next == key {
            return &[];
        }
        k = next;
    }
}

/// `FUN_1004570c`: one value of a multimap key. A single value is returned without touching the RNG; several are
/// `values[rand() % count]` (`rand` = the CRT's, [`CrtRand::rand`]).
pub fn pick_variant(values: &[u32], rand: &mut impl FnMut() -> u32) -> Option<u32> {
    match values {
        [] => None,
        [v] => Some(*v),
        v => Some(v[rand() as usize % v.len()]),
    }
}

/// The Visual C++ 2010 CRT `rand()` (`MSVCR100.dll`, which every client DLL imports - `FUN_1004570c` calls it): per-thread
/// `holdrand = holdrand * 214013 + 2531011; (holdrand >> 16) & 0x7fff`. A thread that never called `srand` starts at 1.
/// `srand` callers found: `FUN_1011bb4a` [GC] = `srand(_time64())` (an ID-range remapper that runs on a playfield load) and
/// `GfxVisualNano2::ProcessStuff` [DS 0x1001975e] = `srand(0x2a)` / `srand(effect seed)` while a particle effect is generated
/// (not modelled: it makes the real sequence depend on the visible effects).
#[derive(Clone, Debug)]
pub struct CrtRand(u32);

impl CrtRand {
    /// `srand(seed)`.
    pub const fn new(seed: u32) -> Self {
        Self(seed)
    }

    pub fn rand(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(214013).wrapping_add(2531011);
        (self.0 >> 16) & 0x7fff
    }
}

/// `FUN_10010ad1`: the parent of an animation key (0 = none).
pub fn fallback_key(key: u32) -> u32 {
    match key {
        0x3fc => 0x3f2,
        0x3fd => 0x3f3,
        0x3fe => 0x3f4,
        0x3ff => 0x3f5,
        0x41e | 0xb2 => 0x78,
        0x421 | 0xb3 | 0x66 => 100,
        0x422 | 0xb4 => 0x65,
        0x424 => 0x3fd,
        0x426 => 0x3ff,
        0x7e | 0x80..=0x84 => 0x7f,
        // 0xc4 / 0xc5 -> 0x86 / 0x87 only while the client char has stat 0x296 (not modelled): 0.
        0xcc | 500..=0x1f9 => 6000,
        _ => 0,
    }
}

/// One `TextureData_t` of the wire `textures[]` list (`ao_net::n3::dynel::TextureData`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextureOverride<'a> {
    /// Material name (matched exactly against `CatMesh::parts[].name`).
    pub material: &'a str,
    /// rdb 1010004 id replacing `Part::texture` (0 = keep the model's).
    pub texture: u32,
    /// rdb 1010004 id replacing `Part::env_texture` (0 = keep).
    pub env_texture: u32,
    /// Third int; the client stores `5` when non-zero (`AlphaMode`), else 0.
    pub alpha_mode: u32,
}

/// `FUN_10070247` (DisplaySystem `AsyncCATMesh::SetCATTextures`): per list entry the part named `material` gets `texture`
/// (layer 1) and `env_texture` (layer 3); the last entry for a part wins. Returns `(part index, override)` ascending.
pub fn texture_overrides(mesh: &CatMesh, list: &[TextureOverride]) -> Vec<(usize, TextureOverride<'static>)> {
    let mut out: Vec<(usize, TextureOverride<'static>)> = vec![];
    for t in list {
        for i in (0..mesh.parts.len()).filter(|&i| mesh.parts[i].name == t.material) {
            let merged = |old: Option<&TextureOverride>| TextureOverride {
                material: "",
                texture: if t.texture != 0 { t.texture } else { old.map_or(0, |o| o.texture) },
                env_texture: if t.env_texture != 0 { t.env_texture } else { old.map_or(0, |o| o.env_texture) },
                alpha_mode: if t.texture != 0 { t.alpha_mode } else { old.map_or(0, |o| o.alpha_mode) },
            };
            match out.iter().position(|e| e.0 == i) {
                Some(k) => out[k].1 = merged(Some(&out[k].1)),
                None => out.push((i, merged(None))),
            }
        }
    }
    out.sort_by_key(|e| e.0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::character::Part;

    /// rdb 1040023 / 22794 (Surf Lizard, "green lizard"), verbatim.
    const LIZARD: &str = "0f00000017000000881f00000c000000f5580000a50100000400000002000000a08601000000000000400000e000000007800000c50000000100000029000000000000000e00000013000000f246000064000000e2070000ff58000065000000e2070000ff58000078000000e2070000f95800007f000000e2070000f7580000c8000000e207000096d90000c9000000e207000096d90000ca000000e207000096d90000cb000000e207000096d9000006040000e2070000fd58000007040000e2070000fa58000008040000e2070000fe58000009040000e207000096d900000a040000e2070000fb5800000b040000e2070000fb5800000c040000e2070000fb5800000d040000e2070000fb58000070170000e2070000f858000014000000050000005b2b00000f000000e2070000aadaeced1e000000e2070000937b4dac1f000000e2070000b379cdad26000000e2070000fb2183bc36000000e2070000fa64028973000000e20700002087d80174000000e2070000790108c175000000e20700008171a84176000000e207000060e1480177000000e2070000ab1934f4060000001c000000f10300000c000000677265656e206c697a61726400000000000000000000000000";

    fn bytes(h: &str) -> Vec<u8> {
        (0..h.len() / 2).map(|i| u8::from_str_radix(&h[2 * i..2 * i + 2], 16).unwrap()).collect()
    }

    #[test]
    fn surf_lizard() {
        let r = NpcRecord::parse(&bytes(LIZARD)).unwrap();
        assert_eq!(r.name, "green lizard");
        assert_eq!(r.mesh(), Some(22773));
        assert_eq!(r.head_mesh(), None);
        assert_eq!(r.stat(0x1a5), Some(4));
        assert_eq!(r.stat(0xe0), Some(0x8007));
        assert_eq!(r.anim(NpcAnim::Walk), Some(22783));
        assert_eq!(r.anim(NpcAnim::Run), Some(22783));
        assert_eq!(r.anim(NpcAnim::Idle), Some(22777));
        assert_eq!(r.anim(NpcAnim::ImpactChest), Some(22775));
        assert_eq!(r.anim(NpcAnim::Die), Some(22776));
        assert_eq!(r.anims.len(), 17);
        assert_eq!(r.sounds.len(), 10);
        assert_eq!(r.sounds[1], (0x1e, vec![0xac4d7b93]));
        assert_eq!((r.pairs.len(), r.trailing), (0, 13));
    }

    #[test]
    fn fallback_chain() {
        let r = NpcRecord::parse(&bytes(LIZARD)).unwrap();
        // no key 126 (back impact) -> 0x7e -> 0x7f; DieShot 503 -> 500..=0x1f9 -> 6000
        assert_eq!(r.anim(NpcAnim::ImpactBack), Some(22775));
        assert_eq!(r.anim(NpcAnim::DieShot), Some(22776));
        // swim 133 has no parent: first clip of the table (key 0x64)
        assert_eq!(r.anim(NpcAnim::Swim), Some(22783));
        assert_eq!(fallback_key(0xb4), 0x65);
        assert_eq!(fallback_key(0x3ff), 0x3f5);
    }

    #[test]
    fn malformed_is_err_not_panic() {
        let good = bytes(LIZARD);
        // the 13 bytes after the name are never read, so every prefix shorter than the name's end must fail
        for n in 0..good.len() - 13 {
            assert!(NpcRecord::parse(&good[..n]).is_err(), "prefix {n}");
        }
        let mut bad = good.clone();
        bad[8] ^= 1; // stat count no longer (n+1)*1009
        assert!(NpcRecord::parse(&bad).is_err());
        let mut bad = good;
        bad[0x1ac] = 0xff; // name length over the record
        assert!(NpcRecord::parse(&bad).is_err());
    }

    fn cat(names: &[&str]) -> CatMesh {
        CatMesh {
            root: String::new(),
            parts: names.iter().map(|n| Part { name: n.to_string(), texture: 1, env_texture: 2, alpha_aux: 0 }).collect(),
            signature: 0,
            materials: vec![],
            spheres: vec![],
            bones: vec![],
            submeshes: vec![],
            col_spheres: vec![],
            attractors: vec![],
        }
    }

    #[test]
    fn texture_data_replaces_named_part() {
        // wire `lizard_green` -> 22768 on mesh 22773's only material; human cloth parts are named arms/feet/hands/legs/body
        let m = cat(&["arms", "body"]);
        let t = |material, texture, env_texture, alpha_mode| TextureOverride { material, texture, env_texture, alpha_mode };
        let o = texture_overrides(&m, &[t("body", 7, 0, 5), t("nothing", 9, 9, 0), t("body", 0, 8, 0)]);
        assert_eq!(o.len(), 1);
        assert_eq!((o[0].0, o[0].1.texture, o[0].1.env_texture, o[0].1.alpha_mode), (1, 7, 8, 5));
    }
}

#[cfg(test)]
mod variant_tests {
    use super::*;

    fn rec(anims: Vec<(u32, Vec<u32>)>) -> NpcRecord {
        NpcRecord { stats: vec![], anims, sounds: vec![], pairs: vec![], name: String::new(), trailing: 0 }
    }

    /// The classic MSVC CRT sequence after `srand(1)` (also the state of a thread that never seeded).
    #[test]
    fn crt_rand_sequence() {
        let mut r = CrtRand::new(1);
        assert_eq!([r.rand(), r.rand(), r.rand(), r.rand()], [41, 18467, 6334, 26500]);
    }

    /// `FUN_1004570c`: one value never touches the RNG, several are `rand() % count`.
    #[test]
    fn pick_uses_rand_only_for_several_values() {
        let calls = std::cell::Cell::new(0);
        let mut rand = || {
            calls.set(calls.get() + 1);
            7
        };
        assert_eq!(pick_variant(&[], &mut rand), None);
        assert_eq!(pick_variant(&[9], &mut rand), Some(9));
        assert_eq!(calls.get(), 0);
        assert_eq!(pick_variant(&[10, 11, 12], &mut rand), Some(11)); // 7 % 3 = 1
        assert_eq!(calls.get(), 1);
    }

    /// Both resolvers take the variants of the first key of the parent chain; only `FUN_10010ebe` falls back to the table's first value.
    #[test]
    fn chain_variants() {
        // idle 0x78 has two variants, walk 100 one; imp-back 0x7e -> 0x7f is absent -> not in the chain of 0x78
        let r = rec(vec![(100, vec![5]), (0x78, vec![1, 2]), (0xb2, vec![])]);
        assert_eq!(holder_variants(&r, 0xb2), &[1, 2]); // hover idle -> idle-stand
        assert_eq!(anim_key_variants(&r, 0xb2), &[1, 2]);
        assert_eq!(holder_variants(&r, 0x7e), &[] as &[u32]); // no parent hit, no table fallback
        assert_eq!(anim_key_variants(&r, 0x7e), &[5]); // first value of the first key (100)
        assert_eq!(anim_key(&r, 0x78), Some(1));
        let mut seq = [0, 1].into_iter();
        let mut rand = || seq.next().unwrap();
        assert_eq!(pick_variant(holder_variants(&r, 0x78), &mut rand), Some(1));
        assert_eq!(pick_variant(holder_variants(&r, 0x78), &mut rand), Some(2));
    }
}
