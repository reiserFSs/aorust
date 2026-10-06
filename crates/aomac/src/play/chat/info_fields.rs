//! Character field helpers traced from Gamecode.dll.
use crate::play::zone::Zone;
use ao_formats::screens::TextDb;
use ao_net::{msg::Identity, n3::info::InfoPacket};

/// GC 0x1003e877/0x1003e8d0: float exponentiation of pow(2500, 1/9),
/// multiplied by 20, truncated, then multiplied by 100. Equality advances rank.
const PVP_THRESHOLDS: [i32; 10] = [2000, 4700, 11300, 27100, 64700, 154400, 368400, 878700, 2096100, 4999900];
fn pvp_rank(score: i32) -> u32 { PVP_THRESHOLDS.partition_point(|&threshold| score >= threshold) as u32 }
fn stat(zone: &Zone, id: Identity, key: u32) -> i32 { zone.stat_of(id.instance, key).unwrap_or(0) }

/// GUI 0x10031068, Roman table 0x102643f8; values >=4000 produce an empty string.
fn roman(mut value: i32) -> String {
    let mut out = String::new();
    if value >= 4000 { return out; }
    for (number,text) in [(1000,"M"),(900,"CM"),(500,"D"),(400,"CD"),(100,"C"),(90,"XC"),(50,"L"),(40,"XL"),(10,"X"),(9,"IX"),(5,"V"),(4,"IV"),(1,"I")] {
        while value >= number { out.push_str(text); value -= number; }
    }
    out
}

/// GUI 0x10031694: TowerType gates this row, TowerLevel is rendered in Roman numerals.
fn tower_type_row(kind: i32, level: i32, texts: &TextDb) -> String {
    let mut out = String::new();
    if kind == 0 || kind == 1_234_567_890 { return out; }
    let value = if level == 1_234_567_890 { texts.by_key(506,"NotSet").unwrap_or_default() } else { roman(level) };
    super::character::row(&mut out,&texts.by_key(506,if kind == 1 { "ControllerType" } else { "TowerType" }).unwrap_or_default(),&value,1);
    out
}

/// GC 0x10021270 + 0x1002025b: the three opcode cases present in all tower
/// templates' event lists 24..26. Reuses Spell::stat and localized item stat names.
fn tower_modifiers(spells: &[ao_net::n3::spells::Spell], texts: &TextDb) -> anyhow::Result<String> {
    use crate::play::chat::log::{ldb_format, Arg};
    use ao_net::n3::spells::stat;
    anyhow::ensure!(spells.iter().all(|s| s.criteria.is_empty()),"tower spell has criteria");
    let mut out = String::new();
    let mut previous_target = -1;
    for spell in spells {
        let target = spell.stat(stat::TARGET);
        let changed_stat = spell.stat(stat::STAT);
        let name = || texts.by_id(2003,changed_stat as u32).unwrap_or_default();
        let value = spell.stat(stat::VALUE);
        let line = match spell.function {
            0xcf35 | 0xcf14 => {
                if changed_stat == 360 {
                    ldb_format(&texts.by_key(1005,"ModifyScale").unwrap_or_default(),&[Arg::N(value.wrapping_add(100))])
                } else if matches!(changed_stat,689 | 535 | 536 | 537) {
                    format!("{} {value}%",name())
                } else if value == 0 { continue; }
                else { format!("Modify {} {value}",name()) }
            }
            0xcf76 => ldb_format(&texts.by_key(1005,"TempStat").unwrap_or_default(),&[Arg::S(name()),Arg::N(value)]),
            0xcfaa => ldb_format(&texts.by_key(1005,"ResistNano").unwrap_or_default(),&[Arg::N(value),Arg::S(texts.by_id(2009,spell.stat(152) as u32).unwrap_or_default())]),
            function => anyhow::bail!("unsupported tower spell {function:#x}"),
        };
        if line.is_empty() { continue; }
        if target != 0 && target != previous_target {
            out.push_str(&texts.by_id(506,target as u32).unwrap_or_default());
            out.push('\n');
        }
        previous_target = target;
        out.push_str("  ");
        out.push_str(&line);
        let delay = spell.stat(4);
        let hits = spell.stat(3);
        if delay > 0 && hits > 1 {
            use std::fmt::Write;
            let _ = write!(out,", {hits} hits, {:.1}s delay",f64::from(delay)/100.0);
        }
        out.push('\n');
    }
    Ok(out)
}

/// GUI 0x100336fb (NPC first item) / 0x10033db4 (player personal tower list).
pub(super) fn tower_html(zone: &Zone, id: Identity, packet: &InfoPacket, texts: &TextDb) -> anyhow::Result<String> {
    let npc = zone.dynels.get(&id.instance).is_some_and(|d| d.npc);
    let kind = zone.skill_value_of(id.instance,388).unwrap_or(1_234_567_890);
    if packet.items.is_empty() || (npc && (kind == 0 || kind == 1_234_567_890)) { return Ok(String::new()); }
    let store = ao_rdb::RecordStore::open(&ao_gui::client_dir())?;
    let mut out = String::new();
    for item in packet.items.iter().take(if npc { 1 } else { usize::MAX }) {
        let Some(tower) = super::tower_interpolation::interpolated(&store,item)? else { continue };
        let type_row = tower_type_row(tower.template.stat(388).unwrap_or(1_234_567_890),tower.template.stat(75).unwrap_or(1_234_567_890),texts);
        if npc {
            out.push_str(&type_row);
            for (index,label) in ["VicinityFriendModifier","VicinityHostileModifier","PersonalModifier"].into_iter().enumerate() {
                let modifiers = tower_modifiers(&tower.lists[index],texts)?;
                if !modifiers.is_empty() { super::character::row(&mut out,&texts.by_key(506,label).unwrap_or_default(),&modifiers,0); }
            }
        } else {
            let modifiers = tower_modifiers(&tower.lists[2],texts)?;
            if modifiers.is_empty() { continue; }
            super::character::row(&mut out,&texts.by_key(506,"Tower").unwrap_or_default(),tower.template.name.as_deref().unwrap_or(""),2);
            out.push_str("<br>");
            out.push_str(&type_row);
            out.push_str("<font color=CCInfoText>");
            out.push_str(&modifiers);
            out.push_str("</font><br>");
        }
    }
    if !npc && !out.is_empty() {
        let mut wrapped = String::from("<br>");
        super::character::row(&mut wrapped,&texts.by_key(506,"PersonalTowerModifiers").unwrap_or_default(),&out,1);
        out = wrapped;
    }
    Ok(out)
}

/// GC 0x1001aebe / 0x10035c99; range table is rdb 1000037 records 1..12,
/// read by 0x100c1f61 (inclusive i32 bounds), visited in ascending record order.
pub(super) fn faction_html(zone: &Zone, id: Identity, texts: &TextDb) -> Option<String> {
    if !zone.dynels.get(&id.instance).is_some_and(|d| d.npc) { return None; }
    const NAMES: [&str; 12] = ["The Sentinels","Omni-Med","Gaia","Omni-Trans","Vanguards","Guardian of Shadow","The Followers of the Temple","The Assertive Operators","The Unredeemed","The Devoted","The Benign Conservers","The Redeemed"];
    const BOUNDS: [(i32,i32); 12] = [(-50000,-30001),(-30000,-18001),(-18000,-10001),(-10000,-5001),(-5000,-1001),(-1000,-1),(0,999),(1000,4999),(5000,9999),(10000,17999),(18000,29999),(30000,50000)];
    const DESC: [&str; 12] = ["Dreaded and hated by","Hated enemy to","Enemy to","Wanted by","Disliked by","Indifferent to","Neutral to","Liked by","Ally to","Friend to","A close friend to","Like Family to"];
    let mut selected = None;
    let mut lowest = 999_999_999;
    for key in 561..=572 {
        let value = stat(zone,id,key);
        if value < 0 && value <= lowest { lowest = value; selected = Some(key); }
    }
    let key = selected?;
    let value = zone.stat(key).unwrap_or(0);
    let desc = BOUNDS.iter().position(|&(lo,hi)| lo <= value && value <= hi).map(|i| DESC[i]).unwrap_or("Missing ExamineDesc: -1");
    let format = texts.by_key(1005,"FactionStanding").unwrap_or_default();
    let mut out = crate::play::chat::log::ldb_format(&format, &[crate::play::chat::log::Arg::S(NAMES[(key-561) as usize].to_string()),crate::play::chat::log::Arg::S(desc.to_string())]);
    out.push_str(&texts.by_key(1005,match stat(zone,id,59) { 2 => "EndSexMale", 3 => "EndSexFemale", _ => "EndSexNeutral" }).unwrap_or_default());
    Some(out)
}


/// GUI 0x10034d8d..0x100351a0: rank labels in LDB 2010, followed by raw kill/death counts.
pub(super) fn pvp_rows(zone: &Zone, id: Identity, texts: &TextDb) -> String {
    let mut out = String::new();
    for (label, score, offset, counters) in [
        ("DuelRank",684,2000, &[("DuelKills",674),("DuelDeaths",675)][..]),
        ("SoloRank",682,(i64::from(zone.skill_value_of(id.instance,60).unwrap_or(1_234_567_890))*50) as u32, &[("SoloKills",678)][..]),
        ("TeamRank",683,1000, &[("TeamKills",680)][..]),
    ] {
        let rank = pvp_rank(stat(zone,id,score));
        let name = texts.by_id(2010,offset.wrapping_add(rank)).unwrap_or_default();
        super::character::row(&mut out,&texts.by_key(506,label).unwrap_or_default(),&format!("{name} ({rank})"),0);
        for &(label,key) in counters {
            super::character::row(&mut out,&texts.by_key(506,label).unwrap_or_default(),&zone.skill_value_of(id.instance,key).unwrap_or(1_234_567_890).to_string(),0);
        }
    }
    out
}
/// GC 0x1001b328: title selection in PvPOptions bits 7..8; all three maximum ranks override it.
pub(super) fn title_name(zone: &Zone, id: Identity, packet: &InfoPacket, texts: &TextDb) -> String {
    if zone.dynels.get(&id.instance).is_none_or(|d| d.npc) { return String::new(); }
    let duel = pvp_rank(stat(zone,id,684));
    let solo = pvp_rank(stat(zone,id,682));
    let team = pvp_rank(stat(zone,id,683));
    if duel == 10 && solo == 10 && team == 10 { return texts.by_key(2010,"GrandMaster").unwrap_or_default(); }
    let (rank,key) = match (stat(zone,id,673) >> 7) & 3 {
        0 => return packet.s80.clone(),
        1 => (duel,2000 + duel),
        2 => (solo,(i64::from(stat(zone,id,60))*50 + i64::from(solo)) as u32),
        _ => (team,1000 + team),
    };
    if rank == 0 { String::new() } else { texts.by_id(2010,key).unwrap_or_default() }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rank_thresholds_advance_on_equality() {
        assert_eq!(pvp_rank(-1),0);
        for (i,threshold) in PVP_THRESHOLDS.into_iter().enumerate() {
            assert_eq!(pvp_rank(threshold-1),i as u32);
            assert_eq!(pvp_rank(threshold),i as u32+1);
        }
        assert_eq!(pvp_rank(i32::MAX),10);
    }
    #[test]
    fn roman_type_boundaries_match_client() {
        assert_eq!(roman(0),"");
        assert_eq!(roman(4),"IV");
        assert_eq!(roman(1999),"MCMXCIX");
        assert_eq!(roman(3999),"MMMCMXCIX");
        assert_eq!(roman(4000),"");
    }
    #[test]
    fn faction_uses_later_equal_minimum_and_own_relationship() {
        let client = ao_gui::client_dir();
        let Ok(texts) = TextDb::load(&client) else { return };
        let id = Identity { kind: 50000, instance: 2 };
        let mut zone = Zone::default();
        zone.dynels.insert(2,crate::play::zone::DynelState { name: String::new(),pos:[0.0;3],yaw:None,npc:true,side:0,level:1,health:1,max_health:1 });
        zone.character_stats.insert(2,[(561,-1),(572,-1)].into());
        zone.stats.insert(572,30000);
        let result = faction_html(&zone,id,&texts).unwrap();
        assert!(result.contains("The Redeemed"),"{result}");
        assert!(result.contains("Like Family to"),"{result}");
        zone.character_stats.get_mut(&2).unwrap().insert(561,-2);
        assert!(faction_html(&zone,id,&texts).unwrap().contains("The Sentinels"));
        zone.character_stats.get_mut(&2).unwrap().clear();
        assert!(faction_html(&zone,id,&texts).is_none());
    }
    #[test]
    fn tower_spell_text_reuses_localized_stat_and_target() {
        let Ok(texts) = TextDb::load(&ao_gui::client_dir()) else { return };
        let spell = ao_net::n3::spells::Spell { function: 0xcf35,stats:[(0,91),(39,2),(32,2)].into(),..Default::default() };
        let result = tower_modifiers(&[spell.clone(),spell],&texts).unwrap();
        assert_eq!(result,format!("{}\n  Modify {} 2\n  Modify {} 2\n",texts.by_id(506,2).unwrap_or_default(),texts.by_id(2003,91).unwrap_or_default(),texts.by_id(2003,91).unwrap_or_default()));
        assert!(tower_modifiers(&[ao_net::n3::spells::Spell { function:0, ..Default::default() }],&texts).is_err());
    }
}
