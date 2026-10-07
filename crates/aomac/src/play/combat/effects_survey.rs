//! Explicit retail-data census; unexpected missing client data is an error.
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
        let native_rejected_count = self.malformed.values().filter(|error| native_rejected(error)).count();
        eprintln!("{label}: native_rejected={native_rejected_count} unresolved_parser_failures={}", self.malformed.len() - native_rejected_count);
        for (id, error) in &self.malformed {
            let category = if native_rejected(error) { "native_rejected" } else { "malformed" };
            eprintln!("{label}: {category} record={id}: {error}");
        }
        eprintln!("{label}: exact sources (record,event_or_stat,function,effect)={:?}", self.sources);
    }

    fn complete(&self, label: &str) -> Result<()> {
        ensure!(self.records > 0 && self.total > 0, "empty {label} census");
        ensure!(self.malformed.is_empty(), "{label}: {} malformed records; see census", self.malformed.len());
        ensure!(self.missing.is_empty(), "{label}: missing effect IDs {:?}", self.missing);
        Ok(())
    }
}

fn native_rejected(error: &str) -> bool {
    // GC1002b297 has no type23 dispatch branch; this exact authored element is
    // rejected by retail too. Do not generalize to other parser failures.
    error == "unsupported item element (0x17, 0x25)"
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

fn persistent_buffs(store: &RecordStore, templates: &Templates) -> Result<Census> {
    let mut out = Census::default();
    for id in store.ids(crate::play::hud_nanodb::NANO_RDB_TYPE)? {
        out.records += 1;
        let record = store.get(crate::play::hud_nanodb::NANO_RDB_TYPE, id)?.with_context(|| format!("enumerated nano {id} disappeared"))?;
        match ao_formats::dynel_visual::parse_item_template(&record) {
            Ok(template) => {
                if let Some(effect) = template.stat(413).filter(|&effect| effect > 0) {
                    out.add(templates, id, 413, 0, effect);
                }
            }
            Err(error) => { out.malformed.insert(id, format!("{error:#}")); }
        }
    }
    Ok(out)
}

#[test]
#[ignore = "requires installed retail client; census of persistent stat413 buff effects"]
fn retail_persistent_buff_census() -> Result<()> {
    let (store, templates) = retail()?;
    let census = persistent_buffs(&store, &templates)?;
    census.report("persistent buff stat413");
    ensure!(census.records > 0 && census.total > 0, "empty persistent buff stat413 census");
    ensure!(census.malformed.is_empty(), "persistent buff stat413: malformed records {:?}", census.malformed);
    // Installed gfxtweak gaps documented in docs/zone/combat-anim.md §7.5; never substitute artwork.
    ensure!(census.missing.keys().copied().collect::<BTreeSet<_>>() == BTreeSet::from([16451, 39606, 39745]),
        "persistent buff stat413: changed missing effect IDs {:?}", census.missing);
    let unsupported: BTreeMap<_, _> = census.classes.iter().filter(|(kind, _)| !Renderer::supports(**kind)).collect();
    ensure!(unsupported.is_empty(), "unsupported persistent buff classes and IDs: {unsupported:?}");
    Ok(())
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

// Keep dependency slots aligned with the native controllers, including deferred
// emissions and destructor impacts: successful spawn alone cannot discover those.
fn dependencies(t: &super::Template) -> Result<Vec<(usize, i32)>> {
    if t.kind == 3001 && t.words.get(10) == Some(&4) {
        return Ok(vec![(usize::MAX, 11762)]);
    }
    let slots: Vec<usize> = match t.kind {
        1001 | 1011 | 3007 => vec![10, 11],
        1002 => vec![10, 11, 26],
        1003 => vec![36],
        1010 => (27..33).collect(),
        1022 => vec![15],
        2007 | 2010 => (0..10).collect(),
        3013 | 3036 => vec![2],
        3014 => vec![28, 29],
        3026 => vec![11, 14],
        3029 => vec![10],
        3034 => {
            let mut at = 21;
            super::buff303x::Curve::parse(t, &mut at, true)?;
            super::buff303x::Curve::parse(t, &mut at, false)?;
            vec![at]
        }
        3004 => {
            let count = t.word(1)? as usize;
            ensure!(count <= 4096, "sequencer count exceeds bounded storage");
            (0..count).map(|i| 2 + i * 3).collect()
        }
        _ => Vec::new(),
    };
    slots.into_iter().filter_map(|slot| {
        let value = if matches!(t.kind, 2007 | 2010 | 3004 | 3034) {
            Ok(t.words.get(slot).copied().unwrap_or(0) as i32)
        } else { t.word(slot).map(|v| v as i32) };
        match value {
            Ok(0) => None,
            Ok(-1) if matches!(t.kind, 2007 | 2010) && slot == 9 => None,
            other => Some(other.map(|id| (slot, id))),
        }
    }).collect()
}

fn all_item_spells(store: &RecordStore, templates: &Templates) -> Result<Census> {
    let mut out = Census::default();
    for id in store.ids(1_000_020)? {
        out.records += 1;
        let record = store.get(1_000_020, id)?.with_context(|| format!("enumerated item {id} disappeared"))?;
        let decoded = (|| -> Result<()> {
            for event in 0..=0x36 {
                let spells = crate::play::chat::item_template_spells(&record, event)?;
                for spell in &spells {
                    let stat = match spell.function {
                        0xcf26 | 0xcfd4 => 0x27,
                        0xcf57 => 0x57,
                        0xcf49 | 0xcf53 | 0xcf54 => 0x57,
                        _ => continue,
                    };
                    let effect = spell.stat(stat);
                    if effect > 0 { out.add(templates, id, event, spell.function, effect); }
                }
            }
            Ok(())
        })();
        if let Err(error) = decoded { out.malformed.insert(id, format!("{error:#}")); }
    }
    Ok(out)
}

fn reachable(census: &Census, graph: &BTreeMap<i32, Vec<(usize, i32)>>) -> BTreeSet<i32> {
    let mut pending: Vec<_> = census.sources.iter().map(|source| source.3).collect();
    let mut seen = BTreeSet::new();
    while let Some(id) = pending.pop() {
        if seen.insert(id) {
            if let Some(children) = graph.get(&id) {
                pending.extend(children.iter().map(|child| child.1));
            }
        }
    }
    seen
}

const CREATIONS: [super::Creation; 15] = {
    use super::Creation::*;
    [Unlocated, Vector, Matrix, RConnector, HitLocation, Dynel, Tracer,
        VectorVector, VectorMatrix, VectorConnector, VectorDynel, DynelVector,
        DynelMatrix, DynelConnector, DynelDynel]
};

fn creation_fixture(creation: super::Creation) -> super::EffectConfig {
    use super::Creation::*;
    let source_dynel = matches!(creation, Dynel | DynelVector | DynelMatrix | DynelConnector | DynelDynel);
    let target_dynel = matches!(creation, VectorDynel | DynelDynel);
    super::EffectConfig {
        creation,
        hit_location: matches!(creation, HitLocation | Tracer).then_some((glam::Vec3::ZERO, glam::Vec3::Z * 20.0)),
        resource_connector: matches!(creation, RConnector | VectorConnector | DynelConnector).then_some(glam::Mat4::IDENTITY),
        source_identity: source_dynel.then_some((50000, 1)),
        target_identity: target_dynel.then_some((50000, 2)),
        source_appearance: source_dynel.then_some([1, 1, 0, 100]),
        target_appearance: target_dynel.then_some([1, 1, 0, 100]),
        track_source: source_dynel,
        ..Default::default()
    }
}

#[test]
#[ignore = "requires installed retail client; exhaustive authored configuration and reachability inventory"]
fn retail_authored_effect_census() -> Result<()> {
    let (store, templates) = retail()?;
    let mut renderer = Renderer::open(&ao_gui::client_dir())?;
    // Installed Newland supplies the real polygon and terrain resource needed
    // by Fence/terrain constructors; this remains a constructor-only probe.
    renderer.set_collision(Some(std::rc::Rc::new(std::cell::RefCell::new(
        ao_formats::playfield::collision::Collision::load(&store, 566)?))));
    let mut graph = BTreeMap::new();
    let mut failures = BTreeMap::new();
    let mut unavailable_mesh_metadata = BTreeMap::new();
    let mut classes: BTreeMap<i32, usize> = BTreeMap::new();
    let mut context_counts: BTreeMap<(i32, String, &'static str), usize> = BTreeMap::new();
    let mut ids: Vec<_> = templates.by_id.keys().copied().collect();
    ids.sort_unstable();
    eprintln!("authored: records={}", ids.len());
    eprintln!("authored: probe=synthetic_creation_fixtures scope=spawn_and_configuration_not_live_frame_behavior");
    for id in ids {
        let template = &templates.by_id[&id];
        *classes.entry(template.kind).or_default() += 1;
        graph.insert(id, dependencies(template)
            .with_context(|| format!("incomplete dependency reachability for effect{id}"))?);
        if matches!(template.kind, 3025 | 3027) {
            let meshes = (|| -> Result<Vec<u32>> {
                if template.kind == 3025 {
                    let mesh = super::tracer_meshes::TracerMesh::new(template, &renderer.store,
                        &renderer.names, glam::Mat4::IDENTITY, &mut renderer.mesh_resources,
                        super::EffectConfig::default())?;
                    Ok(vec![mesh.record_id()])
                } else {
                    let mut meshes = Vec::new();
                    for name in super::legacy302x::MParticle::resource_names(template)? {
                        if let Some(mesh) = renderer.names.id(ao_formats::mesh::MESH_TYPE, name) {
                            meshes.push(mesh);
                        } else {
                            let reason = format!("missing MParticle mesh {name}");
                            eprintln!("mesh_dependency_unavailable: parent={id} class={} reason={reason}", template.kind);
                            unavailable_mesh_metadata.entry(id).and_modify(|old: &mut String| {
                                old.push_str("; "); old.push_str(&reason);
                            }).or_insert(reason);
                        }
                    }
                    Ok(meshes)
                }
            })();
            match meshes {
                Ok(meshes) => for mesh in meshes {
                    let Some(bytes) = renderer.store.get(ao_formats::mesh::MESH_TYPE, mesh)? else {
                        let reason = format!("missing mesh effect ABIFF {mesh}");
                        eprintln!("mesh_dependency_unavailable: parent={id} class={} reason={reason}", template.kind);
                        unavailable_mesh_metadata.entry(id).and_modify(|old: &mut String| {
                            old.push_str("; "); old.push_str(&reason);
                        }).or_insert(reason);
                        continue;
                    };
                    // A present malformed resource is an unresolved parser error,
                    // unlike an absent asset whose child controls cannot instantiate.
                    for attr in ao_formats::mesh::mesh_effect_attrs(&bytes)? {
                        eprintln!("mesh_dependency: parent={id} mesh={mesh} frame={} child={}",
                            attr.frame, attr.effect);
                        graph.entry(id).or_default().push((usize::MAX, attr.effect));
                    }
                },
                Err(error) if error.to_string().starts_with("missing native effect mesh ")
                    || error.to_string().starts_with("missing MParticle mesh ") => {
                    let reason = format!("{error:#}");
                    eprintln!("mesh_dependency_unavailable: parent={id} class={} reason={reason}", template.kind);
                    unavailable_mesh_metadata.insert(id, reason);
                }
                Err(error) => anyhow::bail!("incomplete mesh reachability for effect{id}: {error:#}"),
            }
        }
        // Class zero is body-profile data; all factory contexts return native null.
        if template.kind == 0 {
            // Three breeds have both sexes; Atrox has only its male six-word row.
            if let Err(error) = (0..42).try_for_each(|slot| template.float(slot).map(|_| ())) {
                failures.insert(id, format!("body profile: {error:#}"));
            }
        }
        for creation in CREATIONS {
            let (status, error) = if !creation.constructs(template.kind) {
                ("native_null", None)
            } else {
                let config = creation_fixture(creation);
                for identity in [config.source_identity, config.target_identity].into_iter().flatten() {
                    let matrix = if identity.1 == 1 { glam::Mat4::IDENTITY }
                        else { glam::Mat4::from_translation(glam::Vec3::Z * 20.0) };
                    renderer.prepare_anchors(identity, |_, _| Some(matrix));
                    renderer.set_source_runtime(identity, 1.0, 1, None, 0, true);
                }
                match renderer.spawn_configured(
                    super::Binding { group: 0, attractor: 0, effect: id, note: 0, color: 0 },
                    glam::Mat4::IDENTITY, glam::Vec3::Z * 20.0, config,
                ) {
                    Ok(0) => ("native_null", None),
                    Ok(_) => ("constructed", None),
                    Err(error) => ("configuration_error", Some(format!("{error:#}"))),
                }
            };
            *context_counts.entry((template.kind, format!("{creation:?}"), status)).or_default() += 1;
            if let Some(error) = &error {
                let message = format!("{creation:?}: {error}");
                failures.entry(id).and_modify(|old| { old.push_str("; "); old.push_str(&message); }).or_insert(message);
            }
            eprintln!("authored_context: id={id} class={} creation={creation:?} eligible={} status={status} error={error:?}",
                template.kind, creation.constructs(template.kind));
            renderer.clear();
        }
        eprintln!("authored: id={id} class={} words={:?} configuration_error={:?}",
            template.kind, template.words, failures.get(&id));
    }
    eprintln!("authored: class_record_counts={classes:?} configuration_failures={}", failures.len());
    for ((class, creation, status), count) in context_counts {
        eprintln!("authored_context_counts: class={class} creation={creation} status={status} records={count}");
    }
    for (parent, children) in &graph {
        for &(slot, child) in children {
            eprintln!("dependency: parent={parent} slot={slot} child={child} missing={}",
                child != 49999 && !templates.by_id.contains_key(&child));
        }
    }
    let mut malformed = 0;
    let mut reachable_union = BTreeSet::new();
    for (label, census) in [
        ("weapon event10", weapons(&store, &templates)?),
        ("nano visuals", nanos(&store, &templates)?),
        ("all item effect spells", all_item_spells(&store, &templates)?),
        ("persistent buff stat413", persistent_buffs(&store, &templates)?),
    ] {
        census.report(label);
        malformed += census.malformed.values().filter(|error| !native_rejected(error)).count();
        for id in reachable(&census, &graph) {
            reachable_union.insert(id);
            eprintln!("{label}: reachable_effect={id} class={:?} missing={} configuration_error={:?}",
                templates.by_id.get(&id).map(|t| t.kind),
                id != 49999 && !templates.by_id.contains_key(&id), failures.get(&id));
        }
    }
    for (class, authored) in &classes {
        let reached = reachable_union.iter().filter(|id| {
            templates.by_id.get(id).is_some_and(|template| template.kind == *class)
        }).count();
        let failed = failures.keys().filter(|id| templates.by_id[id].kind == *class).count();
        eprintln!("coverage: class={class} authored={authored} reachable={reached} configuration_failures={failed} supported={}", Renderer::supports(*class));
    }
    eprintln!("coverage: reachable_missing={:?}", reachable_union.iter()
        .filter(|id| **id != 49999 && !templates.by_id.contains_key(id)).collect::<Vec<_>>());
    eprintln!("coverage: unavailable_mesh_metadata={unavailable_mesh_metadata:?}");
    // Unsupported and missing artwork are inventory results; undecoded source
    // records prevent an exhaustive reachability claim and must fail explicitly.
    ensure!(malformed == 0, "incomplete reachability: {malformed} malformed source records; see exact census errors");
    Ok(())
}

#[test]
fn census_native_rejection_is_exact() {
    assert!(native_rejected("unsupported item element (0x17, 0x25)"));
    assert!(!native_rejected("unsupported item element (0x0, 0x16)"));
    assert!(!native_rejected("spell 0x0: version 0 (format 4)"));
}

#[test]
fn census_creation_fixtures_match_native_overloads() {
    use super::Creation::*;
    let hit = creation_fixture(HitLocation);
    assert!(hit.hit_location.is_some());
    assert!(hit.source_identity.is_none());
    let connector = creation_fixture(RConnector);
    assert!(connector.resource_connector.is_some());
    assert!(connector.source_identity.is_none());
    let dynels = creation_fixture(DynelDynel);
    assert!(dynels.source_identity.is_some() && dynels.target_identity.is_some());
    assert!(dynels.track_source);
    assert_eq!(CREATIONS.iter().filter(|creation| creation.constructs(2013)).count(), 1);
    assert!(HitLocation.constructs(2013) && !Matrix.constructs(2013));
}

#[test]
fn census_reachability_includes_deferred_impact_and_cycles() -> Result<()> {
    let t = super::Template { kind: 3026, words: {
        let mut words = vec![0; 15]; words[11] = 2; words[14] = 3; words
    }};
    let graph = BTreeMap::from([(1, dependencies(&t)?), (2, vec![(0, 1)]), (3, vec![(0, 99)])]);
    let census = Census { sources: BTreeSet::from([(7, 10, 0, 1)]), ..Default::default() };
    assert_eq!(reachable(&census, &graph), BTreeSet::from([1, 2, 3, 99]));
    assert!(dependencies(&super::Template { kind: 3026, words: vec![] })?.is_empty());
    assert!(dependencies(&super::Template { kind: 3004, words: vec![0, 4097] }).is_err());
    for (kind, slots) in [(2010, vec![0, 9]), (3013, vec![2]), (3014, vec![28, 29]), (3036, vec![2])] {
        let mut words = vec![0; 30];
        for &slot in &slots { words[slot] = 42; }
        assert_eq!(dependencies(&super::Template { kind, words })?,
            slots.into_iter().map(|slot| (slot, 42)).collect::<Vec<_>>());
    }
    let mut words = vec![0; 24];
    words[23] = 71360;
    assert_eq!(dependencies(&super::Template { kind: 3034, words })?, vec![(23, 71360)]);
    let mut words = vec![0; 11];
    words[10] = 4;
    assert_eq!(dependencies(&super::Template { kind: 3001, words })?, vec![(usize::MAX, 11762)]);
    let mut words = vec![0; 10];
    words[9] = u32::MAX;
    assert!(dependencies(&super::Template { kind: 2010, words })?.is_empty());
    Ok(())
}
