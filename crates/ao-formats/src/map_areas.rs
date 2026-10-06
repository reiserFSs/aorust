//! Which playfield maps a character owns: the table of `FUN_100324d2` (`Gamecode.dll` 0x100324d2, `data/map_areas.txt`, 128 entries of
//! `playfield id -> area index (-1 = every character) + name`) and the rule of `FUN_10057ca5` (0x10057ca5). Evidence: `docs/gui.md` §12.2.

use std::{collections::HashMap, sync::LazyLock};

const TABLE: &str = include_str!("../data/map_areas.txt");

/// Stats the rule reads: `GmLevel`, `MapAreaPart1..4` (areas 0-31, 32-63, 64-95, 96-127).
pub const GM_LEVEL: u32 = 215;
pub const MAP_AREA_PARTS: [u32; 4] = [471, 472, 585, 586];
/// The one playfield `FUN_10057ca5` always refuses (0x1f7c, "Abandoned Research Facility").
pub const NO_MAP_PLAYFIELD: u32 = 8060;

/// `(area index, name)` of a playfield (`FUN_100366ce` / `FUN_10036714`); index -1 = no area bit is needed.
pub fn area(playfield: u32) -> Option<(i32, &'static str)> {
    static MAP: LazyLock<HashMap<u32, (i32, &'static str)>> = LazyLock::new(|| {
        TABLE
            .lines()
            .filter(|l| !l.starts_with('#'))
            .filter_map(|l| {
                let mut f = l.splitn(3, '\t');
                Some((f.next()?.parse().ok()?, (f.next()?.parse().ok()?, f.next()?)))
            })
            .collect()
    });
    MAP.get(&playfield).copied()
}

/// `FUN_10057ca5`: does the character own the map of `playfield` (`None`: no playfield)? `stat` is the character's stat lookup.
pub fn owns_map(playfield: Option<u32>, stat: impl Fn(u32) -> i32) -> bool {
    if stat(GM_LEVEL) != 0 {
        return true;
    }
    let Some(pf) = playfield.filter(|&pf| pf != NO_MAP_PLAYFIELD) else { return false };
    match area(pf) {
        Some((idx, _)) if idx >= 0 => stat(MAP_AREA_PARTS[(idx as usize / 32).min(3)]) as u32 >> (idx % 32) & 1 != 0,
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_has_128_entries_of_the_dll() {
        assert_eq!(TABLE.lines().filter(|l| !l.starts_with('#')).count(), 128);
        assert_eq!(area(505), Some((-1, "Avalon")));
        assert_eq!(area(4561), Some((70, "a shop in Jobe")));
        assert_eq!(area(4001), Some((79, "Jobe Research")));
        assert_eq!(area(4328), Some((115, "Caina")));
        assert_eq!(area(8060), Some((-1, "Abandoned Research Facility")));
        assert_eq!(area(1), None);
    }

    #[test]
    fn ownership_rule() {
        let none = |_| 0;
        // outdoor Rubi-Ka maps and playfields outside the table are always owned; 8060 and "no playfield" never
        assert!(owns_map(Some(505), none));
        assert!(owns_map(Some(1234), none));
        assert!(!owns_map(Some(8060), none));
        assert!(!owns_map(None, none));
        // Jobe Research = area 79: bit 15 of MapAreaPart3 (585)
        assert!(!owns_map(Some(4001), none));
        assert!(owns_map(Some(4001), |s| if s == 585 { 1 << 15 } else { 0 }));
        assert!(!owns_map(Some(4001), |s| if s == 471 { 1 << 15 } else { 0 }));
        // area 115 = bit 19 of MapAreaPart4 (586), also the sign bit works: area 31 would be bit 31 of part 1
        assert!(owns_map(Some(4328), |s| if s == 586 { 1 << 19 } else { 0 }));
        // GmLevel owns everything, even 8060 and no playfield
        assert!(owns_map(Some(8060), |s| (s == GM_LEVEL) as i32));
        assert!(owns_map(None, |s| (s == GM_LEVEL) as i32));
    }
}
