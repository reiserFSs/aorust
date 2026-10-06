//! Quest itemid pages: GUI FUN_1003565d, dispatched by FUN_100384f3 for kind 0xdac3.
//! This branch has no criteria/action table: actions only supply the remaining-time deadline.
use crate::play::zone::Zone;
use ao_formats::{dynel_visual::{item_template, interpolate_item_templates}, screens::TextDb};
use ao_net::{msg::Identity, n3::quest::{Acg, Quest}};
use ao_rdb::RecordStore;
use anyhow::{ensure, Result};
use std::fmt::Write;

fn text(texts: &TextDb, key: &str) -> String { texts.by_key(506, key).unwrap_or_default() }
fn numbers(mut template: String, values: &[i32]) -> String {
    for value in values { template = template.replacen("%d", &value.to_string(), 1); }
    template
}
fn attribute(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('\n', "<br>").replace('"', "&quot;")
}

/// GC CanReceiveSK 0x10018215; N3 RDBPlayfield_t::ReadBlob 0x1001c115 stores +0x50
/// from the version >=9 resource word at file +0x34 (bit 1 enables SK).
fn receives_sk(zone: &Zone, store: Option<&RecordStore>) -> Result<bool> {
    let Some((store, pf)) = store.zip(zone.playfield) else { return Ok(false) };
    let Some(raw) = store.get(1_000_001, pf)? else { return Ok(false) };
    ensure!(raw.len() >= 4, "truncated quest playfield resource");
    let version = u32::from_le_bytes(raw[..4].try_into()?);
    if version <= 8 { return Ok(false); }
    ensure!(raw.len() >= 0x38, "truncated quest playfield flags");
    Ok(u32::from_le_bytes(raw[0x34..0x38].try_into()?) & 2 != 0)
}

/// GC 0x1006263f: faction-dependent factor rounded to f32 before multiplying XP.
fn shadowknowledge(zone: &Zone, xp: i32) -> i32 {
    let faction = match zone.stat(0x21) { Some(1) => 0x23c, Some(2) => 0x239, _ => 0x236 };
    let factor = (f64::from(zone.stat(faction).unwrap_or(0)) * 0.0010000000474974513 / 50000.0) as f32;
    (f64::from(factor) * f64::from(xp)).trunc().max(0.0) as i32
}

fn reward_icon(store: Option<&RecordStore>, item: &Acg) -> Result<Option<(String, i32)>> {
    let Some(store) = store else { return Ok(None) };
    let (Ok(low), Ok(high)) = (u32::try_from(item.low_id), u32::try_from(item.high_id)) else { return Ok(None) };
    let Some(low) = item_template(store, low)? else { return Ok(None) };
    let item = if item.low_id == item.high_id {
        low
    } else {
        let Some(high) = item_template(store, high)? else { return Ok(None) };
        interpolate_item_templates(&low, &high, item.level)?
    };
    Ok(item.stat(0x4f).filter(|&icon| icon != 0).map(|icon| (item.name.unwrap_or_default(), icon)))
}

pub(super) fn html(zone: &Zone, id: Identity, texts: &TextDb, store: Option<&RecordStore>) -> Result<Option<String>> {
    let Some(q) = zone.quests.get(&id).or_else(|| zone.mission_alternatives.get(&id)) else { return Ok(None) };
    let remaining = zone.server_now().and_then(|now| q.remaining_secs(now));
    let sk_level = zone.stat(0x36).is_some_and(|level| (200..=220).contains(&level));
    let xp = if sk_level && q.reward.ints[1] > 0 { receives_sk(zone, store)?.then(|| (shadowknowledge(zone, q.reward.ints[1]), true)) } else { Some((q.reward.ints[1], false)) };
    render(q, remaining, xp, |key| text(texts,key), |item| reward_icon(store,item)).map(Some)
}

fn render(q: &Quest, remaining: Option<u32>, xp: Option<(i32,bool)>, label: impl Fn(&str)->String,
    mut icon: impl FnMut(&Acg)->Result<Option<(String,i32)>>) -> Result<String> {
    let mut out = format!("<font color=CCItemUnknown>{}</font><br>", q.name);
    super::character::row(&mut out, &label("Description"), &q.description.replace("\\n", "<br>"), 0);
    out.push_str("<br>");
    if let Some(seconds) = remaining.filter(|&seconds| seconds > 0) {
        let minutes = seconds / 60;
        let value = numbers(label("DayHourMinute"), &[(minutes / 1440) as i32, (minutes / 60 % 24) as i32, (minutes % 60) as i32]);
        super::character::row(&mut out, &label("RealtimeLeft"), &value, 0);
    }
    if q.is_team() { super::character::row(&mut out, &label("TeamMissionRules"), &label("ThisIsaTeamMission"), 0); }
    out.push_str(&label("Reward"));
    let mut has_reward = false;
    if q.reward.ints[0] > 0 {
        has_reward = true;
        super::character::row(&mut out, &label("Cash"), &numbers(label("NumCredits"), &[q.reward.ints[0]]), 0);
    }
    if q.reward.ints[1] > 0 {
        if let Some((value, sk)) = xp {
            has_reward = true;
            super::character::row(&mut out, &label("Experience"), &numbers(label(if sk { "IT_QuestSKReward" } else { "NumPoints" }), &[value]), 0);
        }
    }
    // GC 0x1001b481: a nonzero single reward ACG replaces the list, not supplements it.
    let single = q.reward.item.low_id != 0 || q.reward.item.high_id != 0 || q.reward.item.level != 0;
    let items = if single { std::slice::from_ref(&q.reward.item) } else { q.reward.items.as_slice() };
    if !items.is_empty() {
        has_reward = true;
        out.push_str(&label("ItemReward"));
        for item in items {
            if let Some((name, image)) = icon(item)? {
                let _ = write!(out, "<a href=itemref://{}/{}/{} title=\"{}\"><img src=rdb://{}></a> ", item.low_id,item.high_id,item.level,attribute(&name),image);
            }
        }
    }
    if !has_reward { out.push_str(&label("NoReward")); }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quest_page_preserves_retail_order_and_reward_actions() {
        let mut q = Quest { name:"Mission".into(), description:"first\\nsecond".into(), value_a4:1<<8, ..Quest::default() };
        q.reward.ints[0] = 25; q.reward.ints[1] = 50;
        q.reward.items.push(Acg { low_id:1, high_id:2, level:3, extra:0 });
        let labels = |key:&str| match key { "DayHourMinute"=>"%d/%d/%d".into(), "NumCredits"|"NumPoints"|"IT_QuestSKReward"=>"%d".into(), _=>format!("[{key}]") };
        let out = render(&q,Some(90060),Some((50,false)),labels,|_|Ok(Some(("A<&\"".into(),42)))).unwrap();
        assert!(out.contains("first<br>second")); assert!(out.contains("1/1/1"));
        assert!(out.find("[Description]") < out.find("[RealtimeLeft]"));
        assert!(out.find("[TeamMissionRules]") < out.find("[Reward]"));
        assert!(out.contains("href=itemref://1/2/3 title=\"A&lt;&amp;&quot;\"><img src=rdb://42>"));
        assert!(!out.contains("[NoReward]"));
        q.reward = Default::default();
        assert!(render(&q,None,None,labels,|_|Ok(None)).unwrap().ends_with("[NoReward]"));
        q.reward.items.push(Acg { low_id:1, ..Default::default() });
        q.reward.item = Acg { low_id:9, high_id:10, level:11, extra:0 };
        let out = render(&q,None,None,labels,|a|Ok(Some((a.low_id.to_string(),42)))).unwrap();
        assert!(out.contains("itemref://9/10/11")); assert!(!out.contains("itemref://1/"));
    }
}
