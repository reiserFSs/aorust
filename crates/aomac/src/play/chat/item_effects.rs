//! Retail Gamecode.dll item text: 100236fa -> 10021270 -> 1002025b.
//! Opcode omissions below are the retail switch's empty/default branches, not generic effects.
use anyhow::{ensure, Result};
use ao_formats::{dynel_visual::{self, ItemTemplate}, screens::TextDb};
use ao_net::n3::spells::Spell;
use ao_rdb::RecordStore;
use crate::play::zone::Zone;
use super::super::log::{ldb_format, Arg};
use std::fmt::Write;

fn text(texts: &TextDb, category: u32, key: &str) -> String { texts.by_key(category, key).unwrap_or_default() }
fn id(texts: &TextDb, category: u32, value: i32) -> String { texts.by_id(category, value as u32).unwrap_or_default() }
fn format(texts: &TextDb, key: &str, args: &[Arg]) -> String { ldb_format(&text(texts, 1005, key), args) }
fn item(store: &RecordStore, value: i32) -> Result<ItemTemplate> {
    Ok(dynel_visual::item_template(store, value as u32)?.unwrap_or_else(|| ItemTemplate { kind: 0, stats: Vec::new(), name: None, sounds: Vec::new() }))
}
fn item_name(store: &RecordStore, value: i32) -> Result<String> { Ok(item(store, value)?.name.unwrap_or_default()) }
fn link(store: &RecordStore, value: i32) -> Result<String> {
    Ok(format!("<a href=\"itemid://53019/{}\">{}</a>", value as u32, item_name(store, value)?))
}
fn range(a: i32, b: i32) -> String {
    let (a,b) = (a.wrapping_abs(),b.wrapping_abs());
    if b == 0 { format!("{a}%") } else if a == b { a.to_string() } else { format!("{a}-{b}") }
}

fn effect(s: &Spell, spells: &[Spell], amplified: bool, zone: &Zone, store: &RecordStore, texts: &TextDb) -> Result<String> {
    let stat = s.stat(0);
    let value = s.stat(39);
    let name = || id(texts,2003,stat);
    Ok(match s.function {
        0xcf14 | 0xcf35 => if stat == 360 { format(texts,"ModifyScale", &[Arg::N(value.wrapping_add(100))]) }
            else if matches!(stat,689|535|536|537) { format!("{} {value}%",name()) }
            else if value != 0 { format!("Modify {} {value}",name()) } else { String::new() },
        0xcf22 => format!("Set {} {value}",name()),
        0xcf29 => {
            let modifier = zone.stat(382).unwrap_or(0).max(-50);
            let adjusted = (f64::from(value) + f64::from(value.wrapping_mul(modifier))/100.0 + 0.5) as i32;
            let name = ao_formats::stats::name(stat as u32).unwrap_or("");
            if modifier == 0 { format!("{name} skill locked for {value} seconds") }
            else { format!("{name} skill locked for {value} ({adjusted}) seconds") }
        },
        0xcf1b => format(texts,"UploadNanoprogram", &[Arg::S(item_name(store,value)?)]),
        0xcf3b => format!("Cast {}",link(store,value)?),
        0xcf48 => "Spawn Item".into(),
        0xcf4a | 0xcf5f | 0xcf61 => {
            let prefix = match s.function { 0xcf4a => "Team Cast ".into(),0xcf5f => format!("AOE {}m ",s.stat(11)),_ => "Cast ".into() };
            format!("{prefix}{}",link(store,s.stat(88))?)
        },
        0xcf4c if s.stat(73) & 4 != 0 => "Root".into(),
        0xcf76 => {
            if matches!(stat,485..=487|539..=542|552..=559) { return Ok(String::new()); }
            let amount = if (467..=469).contains(&stat) {
                spells.iter().find(|other| other.stat(0)==stat+18).map_or(0,|other| other.stat(39))
            } else { value };
            let shown_stat = if (467..=469).contains(&stat) { value } else { stat };
            let shown_name = id(texts,2003,shown_stat);
            match shown_stat {
                339 => format(texts,"TempDamage", &[Arg::S(shown_name),Arg::S(super::super::log::damage_type_name(amount))]),
                368 => format(texts,"TempProfession", &[Arg::S(shown_name),Arg::S(id(texts,2004,amount))]),
                _ => format(texts,"TempStat", &[Arg::S(shown_name),Arg::N(amount)]),
            }
        },
        0xcf7d => format!("Taunt {value}"),
        0xcf7e => "Pacify".into(), 0xcf81 => "Fear".into(),0xcf82 => "Stun".into(),
        0xcf86 => "Wipe hate list".into(),0xcf87 => "Charm".into(),0xcfaf => "Spawn Pet".into(),
        0xcfaa => format(texts,"ResistNano", &[Arg::N(value),Arg::S(id(texts,2009,s.stat(152)))]),
        0xcfb7 => {
            let level = zone.stat(54).unwrap_or(0) as u16 as i32;
            let scaled = level.wrapping_mul(value)/200;
            if stat == 360 { format(texts,"ScaleAt200", &[Arg::N(level),Arg::N(scaled.wrapping_add(100)),Arg::N(value.wrapping_add(200)),Arg::N(value.wrapping_add(100))]) }
            else { format(texts,"SkillAt200", &[Arg::S(name()),Arg::N(level),Arg::N(scaled),Arg::N(value)]) }
        },
        0xcfb9 => format!("Reduce {} by {value} seconds",id(texts,2009,stat)),
        0xcfbe => {
            let target = item(store,s.stat(88))?;
            let desc = store.get(dynel_visual::ITEM_TEMPLATE_TYPE,s.stat(88) as u32)?
                .map(|raw| super::template_spells::data(&raw,0)).transpose()?.and_then(|d|d.description);
            let key = if desc.is_some() { "EnablesSpecial2" } else { "EnablesSpecial" };
            format(texts,key,&[Arg::S(target.name.unwrap_or_default()),Arg::S(desc.unwrap_or_default())])
        },
        0xcfc0 => format(texts,"BaseModification", &[Arg::S(name()),Arg::N(value)]),
        0xcfc3 => format!("Perk {} locked for {value} seconds",item_name(store,stat)?),
        0xcfc5 => format(texts,"FactionSet", &[Arg::S(id(texts,2014,stat)),Arg::N(value)]),
        0xcf0a | 0xcfcc => {
            let (a,b) = (s.stat(2),s.stat(37));
            match stat {
                27 => {
                    let healing = a >= 0 && b >= 0;
                    let mut line = text(texts,1005,if healing { "SpellHealing" } else { "SpellDamage" });
                    line.push(' '); line.push_str(&range(a,b));
                    let multiplier = zone.stat(if healing {535} else {536}).unwrap_or(100);
                    if b != 0 && multiplier != 100 && amplified {
                        let _ = write!(line,"({})",range(a.wrapping_abs().wrapping_mul(multiplier)/100,b.wrapping_abs().wrapping_mul(multiplier)/100));
                    }
                    line
                },
                214 => format!("{}{}",text(texts,1005,if a<0 || b<0 {"NanoReduce"} else {"NanoAdd"}),range(a,b)),
                560..=572 => format(texts,if a<0 || b<0 {"FactionReduce"} else {"FactionAdd"},&[Arg::S(id(texts,2014,stat)),Arg::S(range(a,b))]),
                471|472|585|586 => {
                    let area = ((s.stat(2) as u32 as f64).log10() as f32 / (2f64.log10() as f32) + 0.5) as i32
                        + match stat {472=>32,585=>64,586=>96,_=>0};
                    format!("{}{}",text(texts,1005,"MapArea"),id(texts,2007,area))
                },
                _ => String::new(),
            }
        },
        0xcfd8 => "Set Anchor".into(),0xcfd9 => "Recall Anchor".into(),
        0xcfe8 | 0xcfeb => format!("{} ({value}%): {}",if s.function==0xcfe8 {"Defensive proc"} else {"Offensive proc"},link(store,s.stat(10))?),
        0xcfed => text(texts,1005,"SolveQuest"),
        0xcff5 if value != 0 => format!("Modify {} {value}%",name()),
        0xd000 => format!("Clear lock {}",match s.stat(1) {1=>id(texts,2009,value),53019=>item_name(store,value)?,_=>"Unknown lock type".into()}),
        0xd001 => format(texts,"ModifyOrgCash", &[Arg::N(value),Arg::S(if s.stat(73)&1==0 {""} else if value<0 {" (to caster)"} else {" (from caster)"}.into())]),
        0xd002 => text(texts,1005,"DeleteQuest"),0xd003 => text(texts,1005,"FailQuest"),0xd004 => text(texts,1005,"SendMail"),
        0xd005 => format(texts,"EndFight", &[Arg::S(if s.stat(73)&1==0 {"self"} else {"attackers"}.into())]),
        0xd006 => text(texts,2011,"ActionSneak"),
        _ => String::new(),
    })
}

pub(super) fn interpolated_spells(low: &[u8], high: &[u8], ql: i32, event: u32) -> Result<Vec<Spell>> {
    let a = dynel_visual::parse_item_template(low)?;
    let b = dynel_visual::parse_item_template(high)?;
    let aq = a.stat(54).unwrap_or(0);
    let bq = b.stat(54).unwrap_or(0);
    let low_spells = super::template_spells::spells(low,event)?;
    let high_spells = super::template_spells::spells(high,event)?;
    ensure!(low_spells.len()==high_spells.len(),"different item spell counts");
    let mut spells = if ql == aq {low_spells.clone()} else {high_spells.clone()};
    for ((out,a),b) in spells.iter_mut().zip(&low_spells).zip(&high_spells) {
        ensure!(a.function==b.function,"different item spell functions");
        super::tower_interpolation::interpolate_spell(out,a,b,aq,bq,ql)?;
    }
    Ok(spells)
}

// NanoItem::ReadBinary 1008622a sets Spell_c+0x24 bit 8 on every event.
// DummyItemBase::ReadBinary 1008070e leaves that constructor flag clear.
pub(super) fn event_text(spells: &[Spell], nano: bool, zone: &Zone, store: &RecordStore, texts: &TextDb) -> Result<String> {
    let mut out = String::new();
    let mut previous_target = -1;
    let mut previous_visible = true;
    let mut heading = String::new();
    for spell in spells {
        let target = spell.stat(32);
        if target != 0 {
            if target != previous_target { previous_target = target; heading = id(texts,506,target); }
            else if previous_visible { heading.clear(); }
        }
        let line = effect(spell,spells,nano,zone,store,texts)?;
        previous_visible = !line.is_empty();
        if line.is_empty() { continue; }
        if !heading.is_empty() { out.push_str(&heading); out.push('\n'); }
        out.push_str("  "); out.push_str(&line);
        if spell.stat(4)>0 && spell.stat(3)>1 { let _=write!(out,", {} hits, {:.1}s delay",spell.stat(3),f64::from(spell.stat(4))/100.0); }
        if !spell.criteria.is_empty() {
            out.push_str(" if \n");
            let criteria = criteria_text(&spell.criteria,zone,store,texts)?;
            for line in criteria.lines() { out.push_str("    "); out.push_str(line); out.push('\n'); }
        } else { out.push('\n'); }
    }
    Ok(out)
}

// ConvertCriteria 1001e153. Numeric comparisons adjust strict-greater bounds
// before printing; named enumerations bypass the numeric operator suffix.
fn criterion(c: [i32;3], store: &RecordStore, texts: &TextDb) -> Result<String> {
    let [stat,value,op] = c;
    let key = |key: &str| text(texts,1005,key);
    let feed = |key: &str,args: &[Arg]| format(texts,key,args);
    let ref_key = match op {
        32=>"MustNotHaveItemEquipped",33=>"MustHaveItemEquipped",
        35=>"MustHaveFormula",36=>"MustNotHaveFormula",
        91=>"AffectedBy",101=>"NotAffectedBy",108=>"MustNotHaveUnique",109=>"MustHaveUnique",
        127=>"MustHaveNCUFor",128=>"MustNotHaveNCUFor",_=>"",
    };
    if !ref_key.is_empty() { return Ok(feed(ref_key,&[Arg::S(item_name(store,value)?)])); }
    let plain = match op {
        44=>"IsNPC",80=>"MustBeInOrgAndNW",111=>"NoVehicleEquipped",
        118=>"MustBeCastersPet",119=>"IsInGracePeriod",120=>"IsInLCAreaRange",
        121=>"MustBeInRaid",123=>"InDuel",124=>"MustBeAbleToTeleport",
        125=>"MustNotHaveAnythingWorn",135=>"MustAlliedCombat",136=>"MustNotAlliedCombat",_=>"",
    };
    if !plain.is_empty() { return Ok(key(plain)); }
    if op == 70 || op == 112 { return Ok(text(texts,100,"MustBeFlying")); }
    if matches!(op,92|102) {
        return Ok(feed(if op==92 {"AffectedBy"} else {"NotAffectedBy"},&[Arg::S(id(texts,2009,value))]));
    }
    if matches!(op,93|103) {
        return Ok(format!("{}{}",text(texts,100,if op==93 {"MustHavePerk"} else {"MustNotHavePerk"}),item_name(store,value)?));
    }
    if matches!(op,94|97) {
        return Ok(feed(if op==94 {"PerkMustBeLocked"} else {"PerkCannotBeLocked"},&[Arg::S(item_name(store,value)?)]));
    }
    if op == 106 {
        return Ok(if value==1 {key("MustHaveAvailableInvSlot")} else {feed("MustHaveAvailableInvSlots",&[Arg::N(value)])});
    }
    if op == 117 { return Ok(text(texts,110,"Feedback_MustHaveQuest")); }
    if op == 50 {
        if matches!((stat,value),(1,3)|(3,1)) { return Ok(text(texts,110,"Feedback_TargetMustBeSelf")); }
        return Ok(ldb_format(&text(texts,110,"Feedback_MustBeSameAs"),&[Arg::S(id(texts,2015,value)),Arg::S(id(texts,2015,stat))]));
    }
    if matches!(op,138..=143) {
        let key = ["Feedback_TargetMustBeInTeamWith","Feedback_TargetMustNotBeInTeamWith","Feedback_TargetMustBeInRaidWith","Feedback_TargetMustNotBeInRaidWith","Feedback_TargetMustBeInOrgWith","Feedback_TargetMustNotBeInOrgWith"][(op-138) as usize];
        return Ok(ldb_format(&text(texts,110,key),&[Arg::S(id(texts,2015,value))]));
    }
    if stat == 0 {
        return Ok(key(match op {3=>"OperatorOr",4=>"OperatorAnd",24=>"OperatorUnequal",42=>"OperatorNot",_=>return Ok(String::new())}));
    }
    let named = match stat {
        4 => Some(match value {0=>"nothing",1=>"solitus",2=>"opifex",3=>"nanomage",4=>"athrox",5=>"special",6=>"monster",7=>"human monster",_=>return Ok(format!("Missing breed: {value}"))}.into()),
        59 => Some(match value {0=>"none",1=>"uni",2=>"male",3=>"female",_=>return Ok(format!("Missing sex: {value}"))}.into()),
        33 => Some(id(texts,2005,value)),
        60|368 => {
            if value==0 { return Ok(String::new()); }
            let mut name=id(texts,2004,value);
            if stat==368 { name.insert_str(0,&key("VisualProfession")); }
            Some(name)
        },
        5 => return Ok(if value!=0 {String::new()} else {key(match op {85=>"Must_inYourOrganization",0=>"WithoutOrganization",24=>"MustBeInOrganization",_=>return Ok(String::new())})}),
        6 => return Ok(key(if (value!=0) ^ (op==24) {"MustBeInTeam"} else {"CannotBeInTeam"})),
        72 => return Ok(String::new()),
        173 => return Ok(if value==7 {key(if op==24 {"CannotBeFlying"} else {"MustBeFlying"})} else {"Unsupported CurrentMovementMode.".into()}),
        182 => return Ok(key(if value&8!=0 {"4th_Specialization"} else if value&4!=0 {"3rd_Specialization"} else if value&2!=0 {"2nd_Specialization"} else if value&1!=0 {"1st_Specialization"} else {return Ok(String::new())})),
        213 => return Ok(format!("{}{}",key("CriteriaTeamSide"),id(texts,2005,value))),
        274 if !matches!(op,22|107) => return Ok(key(match value {1=>"CannotWieldWeapons",2=>"WhileWieldingMelee",4=>"WhileWieldingDistance",5=>"NotWieldingMelee",6=>"MustMeleeAndDistance",7=>"MustMelee_Distance_MA",_=>return Ok(String::new())})),
        274|690 if matches!(op,22|107) => {
            let mut names=String::new();
            for bit in 0..=16 { if value as u32 & (1<<bit)!=0 {if !names.is_empty(){names.push_str(", ");} names.push_str(&id(texts,2013,bit));} }
            return Ok(if names.is_empty(){names}else{feed(match (stat,op) {(274,22)=>"MustHaveEquippedWeapon",(274,_)=>"MustNotHaveEquippedWeapon",(_,22)=>"RHMustHaveEquippedWeapon",_=>"RHMustNotHaveEquippedWeapon"},&[Arg::S(names)])});
        },
        349 if value&16!=0 && matches!(op,22|107) => return Ok(key(if op==22 {"XPGainDisabled"} else {"XPGainNotDisabled"})),
        355 => {
            let bit=[(16,500),(8,502),(4,504),(2,506),(1,508),(64,510)].into_iter().find(|(mask,_)|value&mask!=0);
            return Ok(bit.map_or_else(String::new,|(_,base)|id(texts,1005,base+i32::from(op!=107))));
        },
        360 => return Ok(feed("Scale",&[Arg::N(value.wrapping_add(100))])),
        389 => {
            let mut names=Vec::new();
            for (bit,name) in ["Notum Wars","Shadowlands","Shadowlands pre-order","Alien Invasion","Alien Invasion pre-order","Lost Eden","Lost Eden pre-order","Legacy of the Xan","Legacy of the Xan pre-order","Mail sending abilities","Special edition mech"].into_iter().enumerate() {
                if value as u32 & (1<<bit)!=0 {names.push(name);}
            }
            if value as u32 & 0xfffff800 !=0 {names.push("one or more unknown expansions are set");}
            return Ok(feed("MustHaveExpansion",&[Arg::S(names.join(", "))]));
        },
        397 => return Ok(format!("{}{}",key(if op==107 {"CriteriaTargetTypeNot"} else {"CriteriaTargetType"}),id(texts,2007,value))),
        410 => return Ok(key(if value==0 {"CannotBeFighting"} else {"MustBeFighting"})),
        430 => return Ok(if value==2 {key("MustBeOnGround")} else {String::new()}),
        431 => return Ok(if value==35 {key("CannotBeMonster")} else {format!("Unsupported SelectedTarget {}",value as u32)}),
        434 => return Ok(if value==10 {key("MustBeInCombat")} else {String::new()}),
        438 => return Ok(key(match (op,value&3) {(22,1|3)=>"IndoorsOnly",(107,1|3)=>"NotIndoorsOnly",(_,2)=>"OutdoorsOnly",_=>return Ok(String::new())})),
        455 => {
            let name=match (value,op==0) {(0,true)=>"MustBePlayer",(94,true)=>"MustBeCharmPet",(95,true)=>"MustBeRobotPet",(96,true)=>"MustHealingPet",(97,true)=>"MustBeCombatPet",(98,true)=>"MustPsychosisPet",(157,true)=>"MustControlTower",(0,false)=>"MustNotBePlayer",(94,false)=>"MustNotBeCharmPet",(95,false)=>"MustNotBeRobotPet",(96,false)=>"MustNotHealingPet",(97,false)=>"MustNotBeCombatPet",(98,false)=>"MustNotPsychosisPet",(157,false)=>"MustNotControlTower",_=>return Ok(feed("MustUnsupported",&[Arg::N(value)]))};
            return Ok(key(name));
        },
        471|472|585|586 => return Ok(String::new()),
        531 => return Ok(key(if value&1!=0 {"OnlyInShadowlands"} else {"OnlyOnRubi-ka"})),
        660 if value&1!=0 && matches!(op,22|107) => return Ok(key(if op==22 {"FreePlayer"} else {"PayingPlayer"})),
        359|690 => return Ok(key(match value {0 if op==0=>"NormalShape",0=>"MustBePolymorphed",17655=>"MonsterTypeLeet",17710=>"MonsterTypePitLizard",30348=>"MonsterTypeWolf",30356=>"MonsterTypeSabretooth",30365=>"MonsterTypeReet",204168..=204170=>"MustHaveThisManta",_=>"MustBeTransformed"})),
        _=>None,
    };
    if let Some(mut name)=named {
        if op==24 {name.insert_str(0,&key("OperatorNotOf"));}
        return Ok(name);
    }
    if matches!(stat,65..=67|198|256|257|303|432|544|545|617..=619|685|686|692..=694) {
        // The retail masks combine matching bit positions, including multi-bit masks.
        let ordinal=[0xaaaaaaaa_u32,0xcccccccc,0xf0f0f0f0,0xff00ff00,0xffff0000].into_iter().enumerate().fold(0,|n,(bit,mask)|n|if value as u32 & mask!=0 {1<<bit}else{0});
        let mut result=feed("CompletedMission",&[Arg::S(id(texts,2017,stat.wrapping_mul(1000).wrapping_add(ordinal)))]);
        if op==107 {result.insert_str(0,&key("OperatorNotOf"));}
        return Ok(result);
    }
    let stat_name=id(texts,2003,stat);
    let suffix=match op {0=>format!(" {} {value}",key("Op_Equal")),1=>format!(" {} {value}",key("Op_Less")),2=>format!(" {} {}",key("Op_Larger"),value.wrapping_add(1)),3=>format!(" {} {value}",key("Op_Or")),4=>format!(" {} {value}",key("Op_And")),22=>format!("{}{value}",key("MustHave_NL")),24=>format!(" {} {value}",key("Op_Unequal")),73=>feed("BaseFrom",&[Arg::S(format!("{stat_name}{}",value.wrapping_add(1)))]),74=>feed("BaseBelow",&[Arg::S(format!("{stat_name}{value}"))]),_=>format!(" Op: {op} Val: {value}")};
    Ok(if matches!(op,73|74){suffix}else{format!("{stat_name}{suffix}")})
}

/// The reverse/postfix criterion stream rendered by CriteriaAndOrNot 10020590.
/// Iterative evaluation avoids unbounded recursive descent on malformed RDB data.
pub(super) fn criteria_text(criteria: &[[i32;3]], _zone: &Zone, store: &RecordStore, texts: &TextDb) -> Result<String> {
    let mut stack: Vec<(String,usize)> = Vec::new();
    let apply = |c: &[i32;3]| c[0]==0 && matches!(c[2],18|19|21|26|100|110);
    let target=criteria.iter().rev().find(|c|apply(c)).map_or(19,|c|c[2]);
    let mut out=id(texts,1005,target+1000);
    if !criteria.is_empty(){out.push('\n');}
    for &c in criteria {
        if apply(&c) {continue;}
        if c[0]==0 && matches!(c[2],3|4) {
            let (right,rn)=stack.pop().ok_or_else(||anyhow::anyhow!("criterion operator without right operand"))?;
            let (left,ln)=stack.pop().ok_or_else(||anyhow::anyhow!("criterion operator without left operand"))?;
            let join=criterion(c,store,texts)?;
            let indent=|s:String,n:usize| if n>1 {s.lines().map(|l|format!("  {l}")).collect::<Vec<_>>().join("\n")} else {s};
            let left=indent(left,ln); let right=indent(right,rn);
            stack.push((format!("{left}\n{join}\n{right}"),ln+rn+usize::from(ln>1)));
        } else if c[0]==0 && c[2]==42 {
            let (operand,n)=stack.pop().ok_or_else(||anyhow::anyhow!("criterion NOT without operand"))?;
            let join=criterion(c,store,texts)?;
            let body=if n>1 {format!("\n{}",operand.lines().map(|l|format!("  {l}")).collect::<Vec<_>>().join("\n"))}else{format!(" {operand}")};
            stack.push((format!("{join}{body}"),n));
        } else {stack.push((format!("  {}",criterion(c,store,texts)?),1));}
    }
    for (index,(s,_)) in stack.into_iter().rev().enumerate() {if index!=0{out.push('\n');}out.push_str(&s);}
    if !criteria.is_empty(){out.push('\n');}
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retail_modifier_ranges_keep_sign_and_collapse_equal_bounds() {
        assert_eq!(range(-12,-28),"12-28");
        assert_eq!(range(18,18),"18");
        assert_eq!(range(i32::MIN,0),"-2147483648%");
    }
}
