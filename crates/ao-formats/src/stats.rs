//! Stat tables of the original client: id → internal name (`Gamecode.dll`), the skill-window groups
//! (`GUI.dll` `SkillWindow`), display names / descriptions (`text.mdb`) and `data/ipdist.xml`.
//! Evidence: `docs/gui.md` §11.

pub mod skills;

use crate::screens::TextDb;
use anyhow::{Context, Result};
use std::{collections::HashMap, path::Path, sync::LazyLock};

/// `fStatToString` table (`n3EngineClientAnarchy_t::N3Msg_GetStatNameMap`, filled by `FUN_1002f009` [GC 0x1002f009]; 521 distinct ids,
/// id 0x85 and 0x2b2 are inserted twice with the same name). Extracted from the DLL's code by `StatTab.java`
/// (pairs `MOV [EBP-4], id` / `CALL 0x1008a3e8` (map operator[]) / `MOV [EAX], &name`), committed as `data/stat_names.txt`.
const NAMES: &str = include_str!("../data/stat_names.txt");

pub fn name(id: u32) -> Option<&'static str> {
    static MAP: LazyLock<HashMap<u32, &'static str>> = LazyLock::new(|| {
        NAMES
            .lines()
            .filter(|l| !l.starts_with('#'))
            .filter_map(|l| {
                let (id, n) = l.split_once(' ')?;
                Some((id.parse().ok()?, n))
            })
            .collect()
    });
    MAP.get(&id).copied()
}

/// Reverse lookup by internal name.
pub fn id_of(name: &str) -> Option<u32> {
    NAMES.lines().filter(|l| !l.starts_with('#')).find_map(|l| {
        let (id, n) = l.split_once(' ')?;
        (n == name).then(|| id.parse().ok()).flatten()
    })
}

// Stat ids the interface reads by number (`N3Msg_GetSkill(Stat_e, ..)` callers in GUI.dll; names from the table above).
pub const LIFE: u32 = 1; // MaxHealth
pub const BREED: u32 = 4;
pub const HEALTH: u32 = 27;
pub const IP: u32 = 53; // `FUN_100fc18e` → `N3Msg_GetSkill(0x35, 0)` = remaining IP
pub const LEVEL: u32 = 54;
pub const SEX: u32 = 59;
pub const PROFESSION: u32 = 60;
pub const CASH: u32 = 61;
pub const SIDE: u32 = 33;
/// The server marks "no value" with this (`FullCharacter` stat application skips it).
pub const INVALID: i32 = 0x499602D2;

/// One group of the skill window: view/group/button name prefix (`<prefix>_view`, `<prefix>_group`, button `<prefix>`),
/// `text.mdb` category 10010 key of its label (`#10010:n` in `Skills.xml`), the stats in the original's order.
pub struct SkillGroup {
    pub prefix: &'static str,
    pub label: u32,
    pub stats: &'static [u16],
}

/// Built by `FUN_100fb596` [GUI 0x100fb596] (`SkillWindow` setup): 11 groups by `FUN_100ff7cc` (names via `LDBface::GetText(0x271a=10010, n)`),
/// then `FUN_100ff558(11)` sizes the stat vectors and `FUN_10064bee` pushes the 75 ids (ECX = `window+0x8c + 0x10·group`, decoded from the asm).
/// Group 10 "disabled" holds only the two deprecated skills; the window's `disabled_group`/button are shown only when they have non-zero IP.
pub const SKILL_GROUPS: [SkillGroup; 11] = [
    SkillGroup { prefix: "abilities", label: 0, stats: &[16, 17, 18, 19, 20, 21] },
    SkillGroup { prefix: "body", label: 1, stats: &[152, 132, 155, 154, 153, 168, 145] },
    SkillGroup { prefix: "meleew", label: 2, stats: &[102, 103, 106, 107, 105, 104, 100, 101, 118, 120] },
    SkillGroup { prefix: "melees", label: 3, stats: &[146, 142, 147, 144, 143] },
    SkillGroup { prefix: "rangedw", label: 4, stats: &[112, 111, 114, 116, 115, 113, 133, 109, 110, 134, 119] },
    SkillGroup { prefix: "rangeds", label: 5, stats: &[150, 151, 148, 167, 121, 108] },
    SkillGroup { prefix: "nanocast", label: 6, stats: &[127, 128, 129, 122, 131, 130, 149] },
    SkillGroup { prefix: "exploring", label: 7, stats: &[139, 166, 117, 156, 137] },
    SkillGroup { prefix: "combatheal", label: 8, stats: &[136, 164, 162, 135, 123, 124] },
    SkillGroup { prefix: "traderepair", label: 9, stats: &[125, 126, 157, 163, 158, 160, 141, 165, 161, 159] },
    SkillGroup { prefix: "disabled", label: 9999, stats: &[138, 140] },
];

/// `text.mdb` categories keyed by stat id (`LDBface::GetText(cat, id)`): 2000 internal names, 2001 long descriptions, 2002 long names,
/// 2003 short names. The `StatRow` constructor [GUI 0x100fe956] reads 2003 (`GetText(0x7d3, stat)`).
pub const CAT_STAT_DESC: u32 = 2001;
pub const CAT_STAT_LONG: u32 = 2002;
pub const CAT_STAT_SHORT: u32 = 2003;

/// Short display name used on skill rows ("Body Dev.").
pub fn short_name(db: &TextDb, id: u32) -> String {
    db.by_id(CAT_STAT_SHORT, id).unwrap_or_else(|| name(id).unwrap_or("?").to_string())
}

/// Long name ("Body Development").
pub fn long_name(db: &TextDb, id: u32) -> String {
    db.by_id(CAT_STAT_LONG, id).unwrap_or_else(|| short_name(db, id))
}

pub fn description(db: &TextDb, id: u32) -> Option<String> {
    db.by_id(CAT_STAT_DESC, id)
}

/// `N3Msg_GetPVPScoreForRank(rank)` [GC 0x100279d1 → `FUN_1003e877`]: `ftol(round_f32(2500^(1/9))^(rank-1) * 20) * 100`
/// (`_CIpow` of the doubles at GC 0x1015d720 = 2500.0 and 0x1015d718 = 1/9 stored as float, `FUN_1003ddae` integer power, 0x1015d710 = 20.0).
pub fn pvp_score_for_rank(rank: i32) -> i32 {
    let base = 2500f64.powf(f64::from(0.111_111_11_f32)) as f32;
    let mut p = 1f32;
    for _ in 0..(rank - 1).max(0) {
        p *= base;
    }
    (f64::from(p) * 20.0) as i32 * 100
}

/// `N3Msg_GetPVPRank(.., stat)` [GC 0x10027995 → `FUN_1003e8d0`]: the loop counts `r` from 1 while the value reaches `pvp_score_for_rank(r)` (at most 10)
/// and returns `r - 1`: the highest rank whose score the value reaches (0 = unranked).
pub fn pvp_rank(value: i32) -> i32 {
    let mut r = 1;
    while r < 0xb && value >= pvp_score_for_rank(r) {
        r += 1;
    }
    r - 1
}

// ---------------------------------------------------------------------------------------------------------------
// data/ipdist.xml
// ---------------------------------------------------------------------------------------------------------------

/// One `<stat>` of a profession: priority of the (single) level range. Stats are keyed by display name ("Body Dev.").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DistStat {
    pub name: String,
    pub min: u32,
    pub max: u32,
    /// `pri` (0 = not raised … 3 = highest); absent attribute = 0.
    pub pri: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DistProfession {
    pub name: String,
    pub stats: Vec<DistStat>,
}

/// Parses `cd_image/data/ipdist.xml` (a bare sequence of `<profession name>` elements, no root).
pub fn parse_ipdist(src: &str) -> Vec<DistProfession> {
    let attr = |tag: &str, key: &str| -> Option<String> {
        let i = tag.find(&format!("{key}=\""))? + key.len() + 2;
        Some(tag[i..i + tag[i..].find('"')?].replace("&amp;", "&"))
    };
    let mut out: Vec<DistProfession> = vec![];
    for tag in src.split('<').skip(1) {
        let tag = tag.split('>').next().unwrap_or("");
        if tag.starts_with("profession ") {
            out.push(DistProfession { name: attr(tag, "name").unwrap_or_default(), stats: vec![] });
        } else if tag.starts_with("stat ") {
            if let Some(p) = out.last_mut() {
                p.stats.push(DistStat { name: attr(tag, "name").unwrap_or_default(), min: 1, max: 220, pri: 0 });
            }
        } else if tag.starts_with("levelrange") {
            if let Some(s) = out.last_mut().and_then(|p| p.stats.last_mut()) {
                let n = |k| attr(tag, k).and_then(|v| v.parse().ok());
                s.min = n("min").unwrap_or(s.min);
                s.max = n("max").unwrap_or(s.max);
                s.pri = n("pri").unwrap_or(0);
            }
        }
    }
    out
}

pub fn load_ipdist(client_dir: &Path) -> Result<Vec<DistProfession>> {
    let p = client_dir.join("cd_image/data/ipdist.xml");
    let b = std::fs::read(&p).with_context(|| format!("reading {}", p.display()))?;
    Ok(parse_ipdist(&b.iter().map(|&c| c as char).collect::<String>()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_has_the_names_the_zone_docs_use() {
        assert_eq!(name(16), Some("Strength"));
        assert_eq!(name(27), Some("Health"));
        assert_eq!(name(54), Some("Level"));
        assert_eq!(name(0), Some("Flags"));
        assert_eq!(name(26), Some("Energy"));
        assert_eq!(id_of("IP"), Some(IP));
        assert_eq!(NAMES.lines().filter(|l| !l.starts_with('#')).count(), 521);
    }

    #[test]
    fn pvp_scores_follow_the_client_formula() {
        let s: Vec<i32> = (1..=11).map(pvp_score_for_rank).collect();
        assert_eq!(s, [2000, 4700, 11300, 27100, 64700, 154400, 368400, 878700, 2096100, 5000000, 11926600]);
        assert_eq!((pvp_rank(0), pvp_rank(1999), pvp_rank(2000), pvp_rank(4699), pvp_rank(5_000_000), pvp_rank(i32::MAX)), (0, 0, 1, 1, 10, 10));
        assert_eq!(pvp_rank(4700), 2);
    }

    #[test]
    fn groups_cover_the_75_skills_once() {
        let mut all: Vec<u16> = SKILL_GROUPS.iter().flat_map(|g| g.stats.iter().copied()).collect();
        assert_eq!(all.len(), 75);
        all.sort();
        all.dedup();
        assert_eq!(all.len(), 75);
        assert!(all.iter().all(|&s| name(s as u32).is_some()));
    }

    #[test]
    fn ipdist_parses() {
        let p = parse_ipdist("<profession name=\"A\">\n <stat name=\"Strength\">\n  <levelrange min=\"1\" max=\"220\" pri=\"3\"/>\n </stat>\n <stat name=\"X\">\n  <levelrange min=\"1\" max=\"220\" />\n </stat>\n</profession>");
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].stats[0], DistStat { name: "Strength".into(), min: 1, max: 220, pri: 3 });
        assert_eq!(p[0].stats[1].pri, 0);
    }

    #[test]
    fn real_client_names_and_ipdist() {
        let dir = std::env::var_os("AO_CLIENT_DIR").map(std::path::PathBuf::from).unwrap_or_else(|| {
            std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join("Games/ProjectRubiKa/client")
        });
        let Ok(db) = TextDb::load(&dir) else { return };
        assert_eq!(short_name(&db, 152), "Body Dev.");
        assert_eq!(long_name(&db, 152), "Body Development");
        assert_eq!(db.by_id(10010, 1).as_deref(), Some("Body & Defense"));
        let dist = load_ipdist(&dir).unwrap();
        assert_eq!(dist.len(), 14);
        // every ipdist stat name is the short display name of one of the 75 skills
        let names: Vec<String> = SKILL_GROUPS.iter().flat_map(|g| g.stats).map(|&s| short_name(&db, s as u32)).collect();
        for p in &dist {
            assert_eq!(p.stats.len(), 75, "{}", p.name);
            for s in &p.stats {
                // "Parry" is the pre-rename name of 145 "Deflect" (text.mdb 2003); the file still uses it (docs/gui.md §11).
                assert!(s.name == "Parry" || names.contains(&s.name), "{} / {}", p.name, s.name);
            }
        }
    }
}
