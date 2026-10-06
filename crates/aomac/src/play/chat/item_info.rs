//! Retail item HTML: GUI 100384f3 dispatch and 100368b4/100377f3/10037b0e/10037db3/10035dc5/10036543.
use anyhow::Result;
use ao_formats::{dynel_visual::{self, ItemTemplate}, screens::TextDb};
use ao_net::{msg::Identity, n3::{inventory as inv, world::AcgItem}};
use ao_rdb::RecordStore;
use crate::play::zone::Zone;
use super::{ItemRequest, template_spells::{self, Data}};
use std::fmt::Write;

const MISSING: i32 = 1_234_567_890;
fn label(texts: &TextDb, key: &str) -> String { texts.by_key(506,key).unwrap_or_default() }
fn row(out: &mut String, texts: &TextDb, key: &str, value: &str) { super::character::row(out,&label(texts,key),value,0); }
fn optional(out: &mut String,texts:&TextDb,key:&str,value:&str) { if !value.is_empty() { row(out,texts,key,value); } }
fn stat(t:&ItemTemplate,id:u32)->i32 { t.stat(id).unwrap_or(MISSING) }
fn escape(s:&str)->String { s.replace('&',"&amp;").replace('<',"&lt;").replace('>',"&gt;") }

// GC 1008058d -> 10003d5b; 1008070e copies Flags into DummyItem+4c.
fn fixture(t:&ItemTemplate)->bool {t.stat(30).is_some_and(|v|v!=MISSING&&v&1!=0)&&t.stat(0).is_some_and(|v|v!=MISSING&&v&0x1000!=0)}
fn runtime_quality(zone:&Zone,id:Identity,item:AcgItem,inventory:bool)->Option<i32> {zone.world.stat_of(id.kind,id.instance,701).or_else(||(id.kind==0xc788 || inventory).then_some(item.level))}

fn inventory_item(zone:&Zone,id:Identity,container:Identity)->Option<(AcgItem,i32)> {
    if let Some(e)=zone.inventory.get(&(id.instance as u32)).filter(|e|inv::item_identity(e.slot)==id || e.id==id && id!=Identity::default()) { return Some((e.item,i32::from(e.b))); }
    zone.containers.iter().filter(|((kind,instance),_)|container==Identity::default() || container==Identity{kind:*kind,instance:*instance}).find_map(|(_, (word,entries))|entries.iter().find(|e|inv::container_item_identity(*word,e.slot)==id || e.id==id && id!=Identity::default()).map(|e|(e.item,i32::from(e.b))))
}

pub(super) fn html(zone:&Zone,request:&ItemRequest,texts:&TextDb)->Result<Option<String>> {
    html_request(zone,request,texts,None)
}

pub(super) fn html_for_reference(zone:&Zone,item:AcgItem,price:Option<i32>,texts:&TextDb)->Result<Option<String>> {
    html_request(zone,&ItemRequest::Reference(item),texts,price)
}

fn html_request(zone:&Zone,request:&ItemRequest,texts:&TextDb,price:Option<i32>)->Result<Option<String>> {
    let store=RecordStore::open(&ao_gui::client_dir())?;
    let (item,id,shop,container,count)=match request {
        ItemRequest::Reference(item)=>(*item,Identity{kind:0xc788,instance:0},price.is_some(),Identity::default(),None),
        ItemRequest::Identity{id,shop,container}=> {
            if *shop {return Ok(None)}
            if let Some((item,count))=inventory_item(zone,*id,*container) { (item,*id,*shop,*container,Some(count)) }
            else {
                let template=if matches!(id.kind,0xcf1b|0xdeb0|1_000_020) { Some(id.instance) } else { zone.world.stat_of(id.kind,id.instance,23) };
                let Some(template)=template else {return Ok(None)};
                let kind=if id.kind==0xcf1b{crate::play::hud_nanodb::NANO_RDB_TYPE}else{dynel_visual::ITEM_TEMPLATE_TYPE};
                let Some(raw)=store.get(kind,template as u32)? else {return Ok(None)};
                let t=dynel_visual::parse_item_template(&raw)?;
                (AcgItem{low_id:template,high_id:template,level:t.stat(54).unwrap_or(1)},*id,*shop,*container,None)
            }
        }
        ItemRequest::Skill(_)=>return Ok(None),
    };
    let record_type=if id.kind==0xcf1b{crate::play::hud_nanodb::NANO_RDB_TYPE}else{dynel_visual::ITEM_TEMPLATE_TYPE};
    let Some(low)=store.get(record_type,item.low_id as u32)? else {return Ok(None)};
    let high_id=if item.high_id==0 {item.low_id}else{item.high_id};
    let high=if high_id==item.low_id {None}else{store.get(record_type,high_id as u32)?};
    if high_id!=item.low_id && high.is_none() {return Ok(None)}
    let high=high.as_deref().unwrap_or(&low);
    let a=dynel_visual::parse_item_template(&low)?;
    let b=dynel_visual::parse_item_template(high)?;
    let ql=if (1..=511).contains(&(item.level as u16 as i32)){item.level as u16 as i32}else{1};
    let mut template=if matches!(id.kind,0xcf1b|0xdeb0|1_000_020) {a.clone()}else{dynel_visual::interpolate_item_templates(&a,&b,item.level)?};
    if let Some(quality)=runtime_quality(zone,id,item,count.is_some()) {
        if let Some(s)=template.stats.iter_mut().find(|s|s.0==701){s.1=quality;}else{template.stats.push((701,quality));}
    }
    if let Some(price)=price {if let Some(s)=template.stats.iter_mut().find(|s|s.0==74){s.1=price;}else{template.stats.push((74,price));}}
    if let Some(count)=count { if let Some(s)=template.stats.iter_mut().find(|s|s.0==26) {s.1=count;} else {template.stats.push((26,count));} }
    for (s,v) in &mut template.stats { if *s!=701 {if let Some(value)=zone.world.stat_of(id.kind,id.instance,*s) {*v=value;}} }
    let data=interpolated_data(&low,high,ql,stat(&a,54),stat(&b,54))?;
    let sections=super::item_requirements::sections(&data,&template,id,zone,&store,texts)?;
    let spells=super::item_effects::interpolated_spells(&low,high,ql,0)?;
    let combat=super::item_info_combat::fields(&template,&spells,zone,texts)?;
    let mut out=String::new();
    render(&mut out,&template,&data,id,shop,container,zone,texts,&sections,&combat,&low,high,ql,&store)?;
    Ok(Some(out))
}

fn interpolated_data(low:&[u8],high:&[u8],ql:i32,aq:i32,bq:i32)->Result<Data> {
    let a=template_spells::data(low,u32::MAX)?;
    if std::ptr::eq(low,high) {return Ok(a)}
    let b=template_spells::data(high,u32::MAX)?;
    let (mut out,other,from,to)=if (ql-aq).abs()<(ql-bq).abs(){(a,b,aq,bq)}else{(b,a,bq,aq)};
    if aq==bq{return Ok(out)}
    for (_,key,pairs) in &mut out.pairs {
        let Some(other)=other.pairs.iter().find(|e|e.1==*key).map(|e|&e.2) else {anyhow::bail!("different item skill lists")};
        anyhow::ensure!(pairs.len()==other.len(),"different item skill counts");
        for (out,b) in pairs.iter_mut().zip(other) {
            anyhow::ensure!(out.0==b.0,"different item skill keys");
            out.1=dynel_visual::interpolate_acg_value(out.1,b.1,from,to,ql);
        }
    }
    for (_,key,criteria) in &mut out.criteria {
        let Some(other)=other.criteria.iter().find(|e|e.1==*key).map(|e|&e.2) else {anyhow::bail!("different item criterion lists")};
        anyhow::ensure!(criteria.len()==other.len(),"different item criterion counts");
        for (out,b) in criteria.iter_mut().zip(other) {
            anyhow::ensure!(out[0]==b[0],"different item criterion keys");
            out[1]=dynel_visual::interpolate_acg_value(out[1],b[1],from,to,ql);
        }
    }
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
fn render(out:&mut String,t:&ItemTemplate,data:&Data,id:Identity,shop:bool,container:Identity,zone:&Zone,texts:&TextDb,s:&super::item_requirements::Sections,c:&super::item_info_combat::Fields,low:&[u8],high:&[u8],ql:i32,store:&RecordStore)->Result<()> {
    let class=match id.kind {0xcf1b=>6,0xdeb0=>7,_=>match stat(t,76){1=>1,2=>2,3|5=>3,_=>4}};
    if matches!(class,1..=3) && container==Identity::default() && s.overequip!=0 {
        out.push_str(&super::super::log::ldb_format(&label(texts,"PercentEffective"),&[super::super::log::Arg::N((4-s.overequip)*25)]));
    }
    title(out,t,data,id,zone,texts,class!=6 && class!=7);
    if class==6 || class==7 {
        if class==6 { nano(out,t,c,id,zone,texts); }
        if c.radius!=0 {row(out,texts,"Radius",&format!("{}m",c.radius));}
        range(out,t,texts);
        optional(out,texts,"Damage",&c.damage);
        speed(out,t,texts,true);
        cooldowns(out,t,texts);
        optional(out,texts,"AtkSkills",&s.attack);
        optional(out,texts,"DefSkills",&s.defend);
        modifiers(out,low,high,ql,class==6,zone,store,texts,&[14,0],"Modifier")?;
        modifiers(out,low,high,ql,class==6,zone,store,texts,&[20],"OnDeath")?;
        modifiers(out,low,high,ql,class==6,zone,store,texts,&[28],"OnCancel")?;
        optional(out,texts,"SkillToUse",&s.use_skill);
        optional(out,texts,"Requirements",&s.useby);
        description(out,data,texts);
        return Ok(());
    }
    flags(out,t,texts);
    temporary(out,t,texts);
    if container==Identity::default() && stat(t,30)!=MISSING && stat(t,30)&0x40!=0 {out.push_str(&label(texts,"ItemPvPLootable"));}
    if class==1 {ammo(out,t,id,texts);}
    row(out,texts,"Rarity",&texts.by_id(2016,if stat(t,688)==MISSING{0}else{stat(t,688)as u32}).unwrap_or_default());
    let quality=stat(t,701);
    if quality!=0 && quality!=MISSING {row(out,texts,"QualityLevel",&if stat(t,0)!=MISSING && stat(t,0)&0x20!=0{label(texts,"SPECIAL")}else{quality.to_string()});}
    if shop && stat(t,74)!=0 && stat(t,74)!=MISSING {row(out,texts,"Price",&crate::play::hud::group(stat(t,74)));}
    if (class==2 || class==4) && fixture(t) {row(out,texts,"FixtureHeader",&label(texts,"FixtureInfo"));}
    optional(out,texts,"Requirements",&s.useby);
    if class==1 {weapon(out,t,texts,c);}
    optional(out,texts,"Loc",&s.location);
    if class==1 {range(out,t,texts);weapon_skills(out,t,texts);optional(out,texts,"AtkSkills",&s.attack);optional(out,texts,"DefSkills",&s.defend);}
    if class==4 && stat(t,362)!=MISSING && stat(t,61)!=MISSING {
        let cost=zone.skill_value(54).unwrap_or(MISSING).wrapping_mul(stat(t,61));
        row(out,texts,"InsuranceCost",&super::super::log::ldb_format(&label(texts,"NumCredits"),&[super::super::log::Arg::N(cost)]));
    }
    modifiers(out,low,high,ql,false,zone,store,texts,&[14,0],"Modifier")?;
    optional(out,texts,"SkillToUse",&s.use_skill);
    if class==1 && stat(t,436)!=MISSING {row(out,texts,"DamageType",&texts.by_id(2003,stat(t,436)as u32).unwrap_or_default());}
    // GUI 10032d8d's Fabric row requires InputConfig debug mode 0x38, not normal play.
    if class==4 {
        out.push_str(&super::item_info_building::html(t,store,texts)?);
        for (event,key) in [(24,"VicinityFriendModifier"),(25,"VicinityHostileModifier"),(26,"PersonalModifier")] {modifiers(out,low,high,ql,false,zone,store,texts,&[event],key)?;}
        out.push_str(&super::fields::tower_type_row(stat(t,388),stat(t,75),texts));
        charges(out,t,id,texts);
    }
    description(out,data,texts);
    if let Some(nano)=c.crystal {
        if let Some(raw)=store.get(crate::play::hud_nanodb::NANO_RDB_TYPE,nano)? {
            let template=dynel_visual::parse_item_template(&raw)?;
            let data=template_spells::data(&raw,u32::MAX)?;
            let sections=super::item_requirements::sections(&data,&template,Identity{kind:0xcf1b,instance:nano as i32},zone,store,texts)?;
            let combat=super::item_info_combat::fields(&template,&template_spells::spells(&raw,0)?,zone,texts)?;
            out.push_str(&label(texts,"ItemHasFormula"));
            render(out,&template,&data,Identity{kind:0xcf1b,instance:nano as i32},false,container,zone,texts,&sections,&combat,&raw,&raw,stat(&template,54),store)?;
        }
    }
    Ok(())
}

fn title(out:&mut String,t:&ItemTemplate,data:&Data,id:Identity,zone:&Zone,texts:&TextDb,temporary:bool) {
    let rarity=["CCItemUnknown","CCItemTrash","CCItemNormal","CCItemExotic","CCItemQuest","CCItemSocial"].get(stat(t,688)as usize).copied().unwrap_or("CCItemUnknown");
    let _=write!(out,"<font color={rarity}>");
    if temporary && !zone.world.on_ground(id) && stat(t,8)>0 && stat(t,8)!=MISSING {out.push_str(&label(texts,"Temporary"));}
    out.push_str(&escape(zone.world.name_of(id.kind,id.instance).or(data.name.as_deref()).or(t.name.as_deref()).unwrap_or("")));
    out.push_str("</font><br>");
}
fn flags(out:&mut String,t:&ItemTemplate,texts:&TextDb) {
    let flags=stat(t,0); if flags==MISSING{return}
    let mut value=String::new();
    for (bit,key) in [(0x4000000,"NODROP"),(0x8000000,"UNIQUE"),(0x40000,"PlayshiftReq")] {if flags&bit!=0{value.push_str(&label(texts,key));}}
    if !value.is_empty(){let _=write!(out,"<div indent=wrapped><font color=CCInfoText>{value}</font></div>");}
}
fn temporary(out:&mut String,t:&ItemTemplate,texts:&TextDb) {
    let time=stat(t,8); if time<=0 || time==MISSING{return}
    let seconds=time/100;
    row(out,texts,"Temporary",&if seconds<61{format!("{seconds}s")}else if seconds<3601{format!("{}m",seconds/60)}else{format!("{}h",seconds/3600)});
}
fn description(out:&mut String,data:&Data,texts:&TextDb) {if let Some(d)=&data.description {row(out,texts,"Description",&d.replace("\\n","<br>"));}}
fn range(out:&mut String,t:&ItemTemplate,texts:&TextDb) {if stat(t,287)!=MISSING{row(out,texts,"Range",&format!("{}m",stat(t,287)));}}
fn speed(out:&mut String,t:&ItemTemplate,texts:&TextDb,nano:bool) {
    let a=stat(t,294);let b=stat(t,210);
    let a=(a!=MISSING && a!=0 && !(nano&&a==10)).then_some(a);
    let b=(b!=MISSING && b!=0 && !(nano&&b==10)).then_some(b);
    use super::super::log::{ldb_format,Arg};
    let value=match(a,b){(Some(a),Some(b))=>ldb_format(&label(texts,"SpeedAtRe"),&[Arg::F(a as f32/100.0),Arg::F(b as f32/100.0)]),(Some(a),None)=>ldb_format(&label(texts,"SpeedAt"),&[Arg::F(a as f32/100.0)]),(None,Some(b))=>ldb_format(&label(texts,"SpeedRe"),&[Arg::F(b as f32/100.0)]),_=>return};
    row(out,texts,"Speed",&value);
}
fn cooldowns(out:&mut String,t:&ItemTemplate,texts:&TextDb) {
    let cooldown=stat(t,254);if cooldown!=MISSING && cooldown>=100{row(out,texts,"LocalCooldown",&format!("{}.00s",cooldown/100));}
    for (index,id) in [75,546,547,548,549,550].into_iter().enumerate(){let line=stat(t,id);let time=stat(t,643+index as u32);if line!=MISSING&&line>0&&time!=MISSING&&time>=100{row(out,texts,"NanolineCooldown",&format!("{} for {}.00s",texts.by_id(2009,line as u32).unwrap_or_default(),time/100));}}
}
fn nano(out:&mut String,t:&ItemTemplate,c:&super::item_info_combat::Fields,id:Identity,zone:&Zone,texts:&TextDb) {
    let raw=t.stat(407).unwrap_or(1);
    let max=zone.skill_value(221).unwrap_or(MISSING);
    out.push_str(&label(texts,"NanoCost"));
    let color=if max<c.nano_cost{"CCRed"}else{"CCInfoText"};
    let raw_color=if max<raw{"CCRed"}else{"CCInfoText"};
    let _=write!(out,"<font color={color}>{}</font><font color={raw_color}> ({raw})</font>",c.nano_cost);
    if stat(t,54)!=999 {row(out,texts,"NCUcost",&stat(t,54).to_string());}
    let school=stat(t,405);
    let value=if (1..=5).contains(&school){texts.by_key(10000,["NanoSchool_Combat","NanoSchool_Medical","NanoSchool_Prot","NanoSchool_Psi","NanoSchool_Space"][(school-1)as usize]).unwrap_or_default()}else{format!("unknown {}",school.wrapping_sub(1))};
    row(out,texts,"School",&value);
    row(out,texts,"StackingOrder",&stat(t,551).to_string());
    for stat_id in [75,546,547,548,549,550]{let line=stat(t,stat_id);if line!=MISSING&&line!=0&&line!=id.instance{row(out,texts,"Nanoline",&texts.by_id(2009,line as u32).unwrap_or_default());}}
    let seconds=c.duration/100;
    row(out,texts,"Duration",&format!("{:02}:{:02}:{:02}",seconds/3600,(seconds/60)%60,seconds%60));
}
fn ammo(out:&mut String,t:&ItemTemplate,id:Identity,texts:&TextDb) {
    let kind=stat(t,420);if matches!(kind,-1|0|MISSING){return}
    let count=stat(t,26);let max=stat(t,212);
    if id.kind!=0xc788 && count!=-1 && count!=MISSING && max!=-1 && max!=MISSING {
        let color=if count==0{"CCRed"}else if count==max{"CCGreen"}else if count<max*30/100{"CCYellow"}else{"CCWhite"};
        let _=write!(out,"<div indent=wrapped><font color={color}>");
        let args=if stat(t,30)&0x200!=0 {vec![super::super::log::Arg::N(count)]}else{vec![super::super::log::Arg::N(count),super::super::log::Arg::N(max)]};
        out.push_str(&super::super::log::ldb_format(&label(texts,if stat(t,30)&0x200!=0{"AmmoCntOf"}else{"AmmoCnt"}),&args));
        if stat(t,30)&0x200!=0 && stat(t,30)&0x1000000==0{out.push_str(&label(texts,"Splitable"));}
        out.push_str("</font></div>");
    } else if max!=-1 && max!=MISSING {row(out,texts,"MaxAmmo",&max.to_string());}
}
fn weapon(out:&mut String,t:&ItemTemplate,texts:&TextDb,c:&super::item_info_combat::Fields) {
    let ammo=stat(t,420);
    if !matches!(ammo,-1|0|MISSING){let key=match ammo{1=>"EnergyAmmo",2=>"Bullets",3=>"FlamethrowerAmmo",4=>"ShotgunShells",5=>"Arrows",6=>"GrenadeAmmo",7=>"BloodAmmo",8=>"BazookaAmmo",10=>"SelfSupplied",_=>""};row(out,texts,"Ammotype",&if key.is_empty(){format!("Error {ammo}")}else{label(texts,key)});}
    optional(out,texts,"Damage",&c.damage);optional(out,texts,"Dps",&c.dps);
    speed(out,t,texts,false);
    if stat(t,211)!=MISSING{row(out,texts,"EquipDelay",&format!("{:.02}s",stat(t,211)as f64/100.0));}
    let initiative=match stat(t,440){118=>Some("InitiativeMelee"),119=>Some("InitiativeRanged"),120=>Some("InitiativePhysical"),149=>Some("InitiativeNano"),_=>None};
    if let Some(key)=initiative{row(out,texts,"Initiative",&label(texts,key));}
}
fn weapon_skills(out:&mut String,t:&ItemTemplate,texts:&TextDb) {
    for (id,key) in [(101,"DualWield"),(134,"DualWield"),(100,"MAcombinedAttack")] {if stat(t,id)!=MISSING{row(out,texts,key,&format!("{} {}",texts.by_id(2003,id).unwrap_or_default(),stat(t,id)));}}
    let mut specials=String::new();for(bit,key)in [(0x800,"Burst"),(0x1000,"FlingShot"),(0x2000,"FullAuto"),(0x4000,"Snipe"),(0x8000,"BowSpecial"),(0x20000,"SneakAttack"),(0x40000,"FastAttack")] {if stat(t,30)!=MISSING && stat(t,30)&bit!=0{specials.push_str(&label(texts,key));}}
    optional(out,texts,"Special",&specials);
    if stat(t,538)!=MISSING && stat(t,538)!=0{row(out,texts,"MaxBeneficialSkill",&stat(t,538).to_string());}
}
fn charges(out:&mut String,t:&ItemTemplate,id:Identity,texts:&TextDb) {
    let n=stat(t,26);
    if id.kind!=0xc788&&n>=0&&n!=MISSING {
        let mut value=n.to_string();
        if stat(t,30)!=MISSING && stat(t,30)&0x200!=0{value.push_str(&label(texts,"Stackable"));}
        if stat(t,30)!=MISSING && stat(t,30)&0x1000000==0{value.push_str(&label(texts,"Splitable"));}
        row(out,texts,"Charges",&value);
    }
}
#[allow(clippy::too_many_arguments)]
fn modifiers(out:&mut String,low:&[u8],high:&[u8],ql:i32,nano:bool,zone:&Zone,store:&RecordStore,texts:&TextDb,events:&[u32],key:&str)->Result<()> {
    let mut value=String::new();
    for &event in events {
        let spells=super::item_effects::interpolated_spells(low,high,ql,event)?;
        value.push_str(&super::item_effects::event_text(&spells,nano,zone,store,texts)?);
    }
    optional(out,texts,key,&value);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn title_and_description_keep_retail_html_and_literal_line_breaks() {
        let texts=TextDb::parse(b"MMDB\0\0\0\0".to_vec()).unwrap();
        let mut template=ItemTemplate{kind:0xc73d,stats:vec![(688,2)],name:None,sounds:vec![]};
        let data=Data{name:Some("A & B".into()),description:Some("first\\n<font color=CCRed>last</font>".into()),..Default::default()};
        let mut out=String::new();
        title(&mut out,&template,&data,Identity::default(),&Zone::new(7),&texts,false);
        description(&mut out,&data,&texts);
        assert_eq!(out,"<font color=CCItemNormal>A &amp; B</font><br><div indent=wrapped><font color=CCInfoHeader></font><font color=CCInfoText>first<br><font color=CCRed>last</font></font></div>");
        assert!(!fixture(&template));
        template.stats.extend([(0,0x1000),(30,1)]);
        assert!(fixture(&template));
        template.stats.pop();
        assert!(!fixture(&template));
    }
    #[test]
    fn runtime_slot_identity_resolves_without_entry_identity() {
        let mut zone=Zone::new(7);
        let item=AcgItem{low_id:1,high_id:2,level:3};
        zone.inventory.insert(0x16,ao_net::n3::world::InventoryEntry{slot:0x16,a:0,b:1,id:Identity::default(),item});
        assert_eq!(inventory_item(&zone,inv::item_identity(0x16),Identity::default()),Some((item,1)));
        assert!(inventory_item(&zone,Identity{kind:inv::KIND_WEAPON_PAGE,instance:0x16},Identity::default()).is_none());
        for level in [0,512] {
            let item=AcgItem{level,..item};
            assert_eq!(runtime_quality(&zone,Identity{kind:0xc788,instance:0},item,false),Some(level));
            assert_eq!(runtime_quality(&zone,inv::item_identity(0x16),item,true),Some(level));
        }
    }
}
