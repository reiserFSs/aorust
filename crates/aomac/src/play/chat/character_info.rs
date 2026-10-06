//! Character Info HTML: GUI 0x100336fb (NPC), 0x10033db4 (player), rows 0x100310e3.
use crate::play::zone::Zone;
use ao_formats::screens::TextDb;
use ao_net::{msg::Identity, n3::info::InfoPacket};
use std::fmt::Write;

pub(super) fn row(out: &mut String, header: &str, value: &str, mode: u8) {
    if mode == 0 { out.push_str("<div indent=wrapped>"); }
    let _ = write!(out, "<font color=CCInfoHeader>{header}</font><font color=CCInfoText>{value}</font>");
    match mode { 0 => out.push_str("</div>"), 1 => out.push_str("<br>"), _ => {} }
}
fn label(texts: &TextDb, key: &str) -> String { texts.by_key(506, key).unwrap_or_default() }
fn entry(out: &mut String, texts: &TextDb, key: &str, value: &str) { row(out, &label(texts,key), value, 0); }
fn escape(s: &str) -> String { s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;") }

/// GC 0x10017496; GUI 0x100336fb compares the returned scalar against these exact constants.
fn danger(own: i32, target: i32, width: i32) -> &'static str {
    let con = if width == 1_234_567_890 || i64::from(target) < i64::from(own) - i64::from(width) { -1.0 } else { (i64::from(target) - i64::from(own) + i64::from(width)) as f32 / (2.0 * width as f32) };
    if con <= 0.0 { "KillingNoDanger" } else if con < 0.2 || con.is_nan() { "KillingEasy" } else if con < 0.49 { "KillingProbable" } else if con < 0.51 { "KillingMaybyPossible" } else if con < 0.7 { "KillingHard" } else if con < 0.9 { "KillingAlmostImpossible" } else { "KillingImpossible" }
}

pub(super) fn html(zone: &Zone, id: Identity, packet: &InfoPacket, texts: &TextDb) -> anyhow::Result<String> {
    let stat = |s| zone.skill_value_of(id.instance, s).unwrap_or(1_234_567_890);
    let name = zone.dynels.get(&id.instance).map(|d| d.name.as_str()).unwrap_or("");
    let npc = zone.dynels.get(&id.instance).is_some_and(|d| d.npc);
    let mut out = String::new();
    if npc {
        let rarity = ["CCItemUnknown", "CCItemTrash", "CCItemNormal", "CCItemExotic", "CCItemQuest", "CCItemSocial"].get(stat(0x2b0) as usize).copied().unwrap_or("CCItemUnknown");
        let _ = write!(out, "<font color={rarity}>{}</font><br>", escape(name));
        row(&mut out, "Level: ", &stat(0x36).to_string(), 0);
        let key = danger(zone.skill_value(0x36).unwrap_or(1_234_567_890), stat(0x36), zone.skill_value(0x113).unwrap_or(1_234_567_890));
        let mut value = label(texts,key);
        if stat(0xcc) > 30 { value.push_str(&label(texts,"AttackOnSight")); }
        entry(&mut out,texts,"DangerLevel", &value);
        if let Some(faction) = super::fields::faction_html(zone,id,texts) { let _ = write!(out,"<font color=CCInfoText>{faction}</font><br>"); }
        else if stat(0x21) != 3 { entry(&mut out,texts,"Alignment", &texts.by_id(2005,stat(0x21) as u32).unwrap_or_default()); }
        out.push_str("<br>");
        if !packet.s04.is_empty() { let _ = write!(out,"<font color=CCInfoText>{}</font><br>",packet.s04); }
        out.push_str(&super::fields::tower_html(zone,id,packet,texts)?);
    } else {
        out.push_str("<font color=CCInfoHeadline>");
        let title = super::fields::title_name(zone,id,packet,texts);
        if !title.is_empty() { let _ = write!(out, "{} ", escape(&title)); }
        if !packet.s48.is_empty() { let _ = write!(out, "{} ", escape(&packet.s48)); }
        let _ = write!(out, "\"{}\"", escape(name));
        if !packet.s64.is_empty() { let _ = write!(out, " {}", escape(&packet.s64)); }
        out.push_str("</font>\n");
        let shadow = stat(0x214);
        // GC 0x1001601a / 0x10035c99: active ShadowBreed replaces the ordinary breed name.
        let breed_name = if shadow != 0 && shadow != 1_234_567_890 {
            const SHADOW: [&str;36] = ["Luminous","Splendent","Metoric","Balanced","Casual","Uncertain","Dusky","Gloomy","Nocturnal","Radient","Splendid","Blazing","Unsettled","Undetermined","Undecided","Benighted","Noctivagant","Noctivagous","Cloudless","Loustrous","Shiny","Doubtful","Dubious","Undefinable","Shady","Adumbrated","Darkened","Vivid","Bright","Scintillant","Vague","Indeterminated","Ambiguous","Murky","Dark","Black"];
            SHADOW.get(shadow.wrapping_sub(1) as usize).map(|s|s.to_string()).unwrap_or_else(||format!("Missing ShadowBreed: {shadow}"))
        } else {
            let breed = stat(4);
            ["Nothing", "Solitus", "Opifex", "Nanomage", "Atrox", "Special", "Monster", "human monster"].get(breed as usize).map(|s| s.to_string()).unwrap_or_else(||format!("Missing breed: {breed}"))
        };
        entry(&mut out,texts,"Breed", &breed_name);
        entry(&mut out,texts,"Gender", &texts.by_id(1005, (stat(0x3b) + 100) as u32).unwrap_or_else(|| texts.by_key(1005,"Error").unwrap_or_default()));
        let visual_prof = stat(0x170);
        let title_level = stat(0x25);
        entry(&mut out,texts,"ProfessionTitle", &format!("{} (TitleLevel {title_level})",texts.by_id(1060,(i64::from(visual_prof)*100+i64::from(title_level)) as u32).unwrap_or_default()));
        let profession = if visual_prof == 0 || visual_prof == 255 { label(texts,"NotChosenYet") } else { texts.by_id(2004,visual_prof as u32).unwrap_or_default() };
        entry(&mut out,texts,"Profession", &profession);
        let level = stat(0x36);
        if level != 1_234_567_890 {
            row(&mut out,&label(texts,"Level"),&level.to_string(),2);
            if (201..=220).contains(&level) { out.push_str("&nbsp;");row(&mut out,&label(texts,"ShadowLevel"),&(level-200).to_string(),1); } else { out.push_str("<br>"); }
        }
        for (key,s) in [("NumInvadersKilled",0x267),("KilledByInvaders",0x268)] { entry(&mut out,texts,key,&stat(s).to_string()); }
        entry(&mut out,texts,"DefenderRank",&texts.by_id(1169,(i64::from(stat(0xa9))*100) as u32).unwrap_or_default());
        let side = stat(0x21);
        entry(&mut out,texts,"Alignment",&if side == 3 { label(texts,"NotChosenYet") } else { texts.by_id(2005,side as u32).unwrap_or_default() });
        if id.instance as u32 == zone.char_id && (side == 1 || side == 2) { entry(&mut out,texts,"Tokens",&stat(if side == 2 {0x4b} else {0x3e}).to_string()); }
        entry(&mut out,texts,"SideXpBonus",&format!("{:.02}%",packet.v30 as u16 as f32 / 100.0));
        if let Some(org) = zone.org_names.get(&stat(5)) { entry(&mut out,texts,if side == 2 {"Detachment"} else {"Clan"},org); }
        if let Some(title) = packet.s9c.as_ref().filter(|s| !s.is_empty()) {
            entry(&mut out,texts,if side == 2 {"DetachmentTitle"} else {"ClanTitle"},title);
            if !packet.destinations.is_empty() {
                let mut names = String::new();
                for d in &packet.destinations { names.push_str(&d.name); names.push_str("<br>"); }
                entry(&mut out,texts,"MastersOf",&names);
            }
            if let Some((_,city)) = packet.v38.filter(|&city| city != 0).and_then(|city|ao_formats::map_areas::area(city as u32)) {
                entry(&mut out,texts,"OwnersOfCity",city);
            }
        }
        out.push_str("<br>");
        if id.instance as u32 == zone.char_id {
            if stat(0x45) > 5 { row(&mut out,&texts.by_key(10004,"VeteranPoints").unwrap_or_default(),&stat(0x44).to_string(),0); }
            if stat(0x185) & 0x10 != 0 { row(&mut out,"Victory Points: ",&stat(0x29d).to_string(),0); }
            row(&mut out,"Data Fragments: ",&stat(0x2b7).to_string(),0);
            row(&mut out,"Freelancers Inc. Tokens: ",&stat(0x2b8).to_string(),0);
            let mut specs = Vec::new();
            for (i,key) in ["First", "Second", "Third", "Fourth"].iter().enumerate() { if stat(0xb6) & (1<<i) != 0 { specs.push(texts.by_key(110,&format!("Feedback_{key}SpecializationCompleted")).unwrap_or_default()); } }
            entry(&mut out,texts,"Specializations",&specs.join("\n"));
        }
        out.push_str(&super::fields::pvp_rows(zone,id,texts));
        out.push_str(&super::fields::tower_html(zone,id,packet,texts)?);
        if id.instance as u32 != zone.char_id { let _ = write!(out,"<br><a href=\"chatcmd:///inspect {}\">Inspect Equipment</a>",id.instance); }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn original_rows_and_consider_boundaries() {
        let mut out=String::new();row(&mut out,"Level: ","12",0);
        assert_eq!(out,"<div indent=wrapped><font color=CCInfoHeader>Level: </font><font color=CCInfoText>12</font></div>");
        assert_eq!(danger(10,0,10),"KillingNoDanger");
        assert_eq!(danger(10,10,10),"KillingMaybyPossible");
        assert_eq!(danger(10,18,10),"KillingImpossible");
        assert_eq!(danger(10,10,0),"KillingEasy");
    }
}
