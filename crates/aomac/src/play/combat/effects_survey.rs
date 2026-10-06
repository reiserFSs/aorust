//! Explicit retail-data census; missing client data is an error, never a skipped pass.
use super::{bindings, Renderer, Templates};
use anyhow::{ensure, Context, Result};
use ao_rdb::RecordStore;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default)]
struct Census {
    records: usize,
    malformed: BTreeMap<u32, String>,
    total: usize,
    classes: BTreeMap<i32, BTreeSet<i32>>,
    missing: BTreeMap<i32, usize>,
    // Record, event (or template stat), spell function, authored effect ID.
    sources: BTreeSet<(u32, u32, u32, i32)>,
}

impl Census {
    fn add(&mut self, templates: &Templates, record: u32, event: u32, function: u32, effect: i32) {
        self.total += 1;
        self.sources.insert((record, event, function, effect));
        match templates.by_id.get(&effect) {
            Some(template) => { self.classes.entry(template.kind).or_default().insert(effect); }
            None => { *self.missing.entry(effect).or_default() += 1; }
        }
    }

    fn report(&self, label: &str) {
        eprintln!("{label}: records={} malformed={} total_bindings={} distinct_missing={} missing={:?}", self.records, self.malformed.len(), self.total, self.missing.len(), self.missing);
        for (kind, ids) in &self.classes {
            eprintln!("{label}: class={kind} supported={} distinct_effects={} ids={ids:?}", Renderer::supports(*kind), ids.len());
        }
        for (id, error) in &self.malformed { eprintln!("{label}: malformed record={id}: {error}"); }
        eprintln!("{label}: exact sources (record,event_or_stat,function,effect)={:?}", self.sources);
    }

    fn complete(&self, label: &str) -> Result<()> {
        ensure!(self.records > 0 && self.total > 0, "empty {label} census");
        ensure!(self.malformed.is_empty(), "{label}: {} malformed records; see census", self.malformed.len());
        ensure!(self.missing.is_empty(), "{label}: missing effect IDs {:?}", self.missing);
        Ok(())
    }
}

fn retail() -> Result<(RecordStore, Templates)> {
    let dir = ao_gui::client_dir();
    Ok((RecordStore::open(&dir)?, Templates::open(&dir)?))
}

fn weapons(store: &RecordStore, templates: &Templates) -> Result<Census> {
    let mut out = Census::default();
    for id in store.ids(1_000_020)? {
        out.records += 1;
        let record = store.get(1_000_020, id)?.with_context(|| format!("enumerated item {id} disappeared"))?;
        match crate::play::chat::item_template_spells(&record, 10) {
            Ok(spells) => {
                for binding in bindings(&spells) {
                    out.add(templates, id, 10, [0xcf49, 0xcf53, 0xcf54][binding.group as usize], binding.effect);
                }
            }
            Err(error) => { out.malformed.insert(id, format!("{error:#}")); }
        }
    }
    Ok(out)
}

fn nanos(store: &RecordStore, templates: &Templates) -> Result<Census> {
    let mut out = Census::default();
    for id in store.ids(crate::play::hud_nanodb::NANO_RDB_TYPE)? {
        out.records += 1;
        let record = store.get(crate::play::hud_nanodb::NANO_RDB_TYPE, id)?.with_context(|| format!("enumerated nano {id} disappeared"))?;
        let decoded = (|| -> Result<Vec<(u32, u32, i32)>> {
            let template = ao_formats::dynel_visual::parse_item_template(&record)?;
            let mut effects = Vec::new();
            // CharCastNano_t GC 1007b084 / 1007ac9a: start and completion visuals.
            for stat in [0x1ac, 0x19e, 0x169] {
                if let Some(effect) = template.stat(stat).filter(|&effect| effect > 0) {
                    effects.push((stat, 0, effect));
                }
            }
            // The shared parser validates sub <= 0x36. Enumerate every legal event,
            // not just the current GUI's event zero, so no visual spell is hidden.
            for event in 0..=0x36 {
                for spell in crate::play::chat::item_template_spells(&record, event)? {
                    let stat = match spell.function { 0xcf26 | 0xcfd4 => 0x27, 0xcf57 => 0x57, _ => continue };
                    let effect = spell.stat(stat);
                    if effect > 0 { effects.push((event, spell.function, effect)); }
                }
            }
            Ok(effects)
        })();
        match decoded {
            Ok(effects) => { for (event, function, effect) in effects { out.add(templates, id, event, function, effect); } }
            Err(error) => { out.malformed.insert(id, format!("{error:#}")); }
        }
    }
    Ok(out)
}

#[test]
#[ignore = "requires installed retail client; prints every authored effect class and ID"]
fn retail_effect_census() -> Result<()> {
    let (store, templates) = retail()?;
    let weapons = weapons(&store, &templates)?;
    let nanos = nanos(&store, &templates)?;
    weapons.report("weapon event10");
    nanos.report("nano visuals");
    // Print both inventories before failing, including malformed and missing records.
    let weapon_complete = weapons.complete("weapon event10");
    let nano_complete = nanos.complete("nano visuals");
    weapon_complete?;
    nano_complete
}

#[test]
#[ignore = "requires installed retail client; all weapon bindings must render"]
fn retail_weapon_classes_supported() -> Result<()> {
    let (store, templates) = retail()?;
    let census = weapons(&store, &templates)?;
    census.report("weapon event10");
    census.complete("weapon event10")?;
    let unsupported: BTreeMap<_, _> = census.classes.iter().filter(|(kind, _)| !Renderer::supports(**kind)).collect();
    ensure!(unsupported.is_empty(), "unsupported weapon effect classes and IDs: {unsupported:?}");
    Ok(())
}
