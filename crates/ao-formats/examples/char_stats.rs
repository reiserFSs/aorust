//! Survey of all character records: `cargo run --release -p ao-formats --example char_stats`.
//! `--bloom` surveys authored CAT bloom eligibility (not rendered pixels), without animation/player surveys.
//! Scope: authored bind pose, no wire texture/material overrides, rendering effect 0.
use ao_formats::character::*;
use ao_rdb::RecordStore;
use std::collections::{BTreeMap, HashMap};

fn main() -> anyhow::Result<()> {
    let store = RecordStore::open(&dirs_home().join("Games/ProjectRubiKa/client"))?;
    if std::env::args().any(|a| a == "--bloom") {
        return bloom_survey(&store);
    }
    if std::env::args().any(|a| a == "--list") {
        // id, skeleton root, bind-pose height, vertices, bones, material texture names
        for id in store.ids(CHAR_MESH_TYPE)? {
            let m = load_cat_mesh(&store, CHAR_MESH_TYPE, id)?;
            let v = m.submeshes.iter().flat_map(|s| &s.vertices);
            let (lo, hi) = v.clone().fold((f32::MAX, f32::MIN), |(l, h), v| (l.min(v.bind[1]), h.max(v.bind[1])));
            let tex: Vec<_> = m.materials.iter().map(|x| x.texture_name.as_str()).collect();
            println!("{id}\t{}\t{:.2}..{:.2}\t{}v\t{}b\t{:#010x}\t{}", m.root, lo, hi, v.count(), m.bones.len(), m.signature, tex.join(","));
        }
        return Ok(());
    }
    let mut sigs: HashMap<u32, Vec<u32>> = HashMap::new(); // skeleton hash -> mesh ids (1010002)
    for ty in [CHAR_MESH_TYPE, CHAR_MESH_LOW_TYPE] {
        let (mut ok, mut verts, mut tris, mut notex, mut parts, mut missing_tex) = (0, 0, 0, 0, 0, 0);
        let (mut nofit, mut unit_scale) = (0, 0);
        let mut fails: BTreeMap<String, Vec<u32>> = BTreeMap::new();
        for id in store.ids(ty)? {
            match load_cat_mesh(&store, ty, id) {
                Ok(m) => {
                    ok += 1;
                    if ty == CHAR_MESH_TYPE {
                        sigs.entry(m.signature).or_default().push(id);
                    }
                    verts += m.submeshes.iter().map(|s| s.vertices.len()).sum::<usize>();
                    tris += m.submeshes.iter().map(|s| s.indices.len() / 3).sum::<usize>();
                    parts += m.parts.len();
                    notex += m.parts.iter().filter(|p| p.texture == 0).count();
                    missing_tex += m.parts.iter().filter(|p| p.texture != 0 && store.get(1010004, p.texture).ok().flatten().is_none()).count();
                    unit_scale += m.bones.iter().filter(|b| b.scale != 1.0).count();
                    if std::env::args().any(|a| a == "--scaled") && m.bones.iter().any(|b| b.scale != 1.0) {
                        println!("scaled bones in {ty}/{id}: {:?}", m.bones.iter().filter(|b| b.scale != 1.0).take(3).map(|b| (b.name.as_str(), b.scale)).collect::<Vec<_>>());
                    }
                    nofit += usize::from(m.submeshes.is_empty());
                    // the full scene path (textures, skinning of the bind pose)
                    if let Err(e) = load_character_record(&store, ty, id, None) {
                        fails.entry(format!("scene: {e:#}")).or_default().push(id);
                    }
                }
                Err(e) => fails.entry(format!("{e:#}").split(": ").last().unwrap().to_string()).or_default().push(id),
            }
        }
        println!("type {ty}: decoded {ok}, verts {verts}, tris {tris}; texture parts {parts} (no texture id {notex}, id missing from 1010004 {missing_tex}); bones with scale != 1: {unit_scale}; no geometry {nofit}");
        for (k, v) in fails {
            println!("  FAIL x{} {k}: e.g. {:?}", v.len(), &v[..v.len().min(5)]);
        }
    }
    let (mut ok, mut dur, mut keys) = (0, 0.0f64, 0usize);
    let mut fails: BTreeMap<String, Vec<u32>> = BTreeMap::new();
    let mut anim_sigs: HashMap<u32, Vec<(u32, f32)>> = HashMap::new();
    let mut versions = BTreeMap::new();
    for id in store.ids(CHAR_ANIM_TYPE)? {
        match load_anim(&store, id) {
            Ok(a) => {
                ok += 1;
                dur += a.duration as f64;
                keys += a.tracks.iter().map(|t| t.rot.len() + t.trans.len()).sum::<usize>();
                *versions.entry(a.version).or_insert(0) += 1;
                anim_sigs.entry(a.signature).or_default().push((id, a.duration));
            }
            Err(e) => fails.entry(format!("{e:#}").split(": ").last().unwrap().to_string()).or_default().push(id),
        }
    }
    println!("type {CHAR_ANIM_TYPE}: decoded {ok}, {keys} keys, total {:.0} s; versions {versions:?}", dur / 1000.0);
    for (k, v) in fails {
        println!("  FAIL x{} {k}: e.g. {:?}", v.len(), &v[..v.len().min(5)]);
    }
    let orphans: usize = anim_sigs.iter().filter(|(s, _)| !sigs.contains_key(s)).map(|(_, v)| v.len()).sum();
    let unanimated = sigs.iter().filter(|(s, _)| !anim_sigs.contains_key(s)).map(|(_, v)| v.len()).sum::<usize>();
    println!("skeleton hashes: {} (meshes) / {} (anims); animations without a model {orphans}; models without animation {unanimated}", sigs.len(), anim_sigs.len());
    // posed decode over every mesh id with its first compatible clip
    let (mut posed, mut pfail) = (0, BTreeMap::<String, Vec<u32>>::new());
    for (sig, ids) in &sigs {
        let Some(&(anim, dur)) = anim_sigs.get(sig).and_then(|v| v.first()) else { continue };
        for &id in ids {
            match load_character_posed(&store, id, anim, dur / 2000.0) {
                Ok(_) => posed += 1,
                Err(e) => pfail.entry(format!("{e:#}")).or_default().push(id),
            }
        }
    }
    println!("posed decode (first compatible clip, mid-time): {posed} ok");
    for (k, v) in pfail {
        println!("  FAIL x{} {k}: e.g. {:?}", v.len(), &v[..v.len().min(5)]);
    }
    // limb detachment with each model's own clip set (walk, else the first clip) at 0.6 s; wrong-set clips of
    // the same skeleton hash have other bone lengths and are *not* expected to fit (see docs)
    let (mut checked, mut worst, mut bad) = (0, (0.0f32, 0u32), vec![]);
    for ids in sigs.values() {
        for &id in ids {
            let Ok(clips) = model_clips(&store, id) else { continue };
            let Some(&(_, clip)) = clips.iter().find(|c| c.0 == "walk").or(clips.first()) else { continue };
            let gap = pose_detachment(&store, id, clip, 0.6)?;
            checked += 1;
            if gap > worst.0 {
                worst = (gap, id);
            }
            if gap > 0.15 {
                bad.push((id, gap));
            }
        }
    }
    println!("limb detachment, own clip set: {checked} models checked, worst {:.3} m (model {}), above 0.15 m: {} {:?}", worst.0, worst.1, bad.len(), &bad[..bad.len().min(8)]);
    // name table coverage and the player tuple API
    let names = NameTable::load(&store)?;
    let named = |ty, ids: Vec<u32>| ids.iter().filter(|&&i| names.name(ty, i).is_some()).count();
    println!(
        "names: models {}/{}, clips {}/{}",
        named(CHAR_MESH_TYPE, store.ids(CHAR_MESH_TYPE)?),
        store.ids(CHAR_MESH_TYPE)?.len(),
        named(CHAR_ANIM_TYPE, store.ids(CHAR_ANIM_TYPE)?),
        store.ids(CHAR_ANIM_TYPE)?.len()
    );
    let (mut ok, mut total) = (0, 0);
    for breed in [Breed::Solitus, Breed::Opifex, Breed::Nanomage, Breed::Atrox] {
        for gender in [Gender::Male, Gender::Female] {
            let Ok(model) = player_model(&store, breed, gender) else { println!("{breed:?} {gender:?}: no model in the client"); continue };
            let roles = character_animations(&store, model)?;
            let heads = player_heads(&store, breed, gender, Skin::Caucasian)?;
            println!("{breed:?} {gender:?}: model {model}, {} heads, {} roles (walk {:?})", heads.len(), roles.len(), roles.iter().find(|r| r.0 == Role::Walk).map(|r| r.1));
            for (head, _) in &heads {
                total += 1;
                ok += usize::from(load_player_character(&store, breed, gender, *head, Some((Role::Idle, 0.0))).is_ok());
            }
        }
    }
    println!("players assembled over every head (idle pose): {ok}/{total}");
    // SetSkinData coverage: every slot of every player type, build and solitus ethnicity resolves its naked skin
    let (mut slots, mut found) = (0, 0);
    for (breed, gender) in [(Breed::Solitus, Gender::Male), (Breed::Solitus, Gender::Female), (Breed::Opifex, Gender::Male), (Breed::Opifex, Gender::Female), (Breed::Nanomage, Gender::Male), (Breed::Nanomage, Gender::Female), (Breed::Atrox, Gender::Male)] {
        for skin in if breed == Breed::Solitus { vec![Skin::Caucasian, Skin::Asian, Skin::African] } else { vec![Skin::Caucasian] } {
            for part in ClothPart::ALL {
                slots += 1;
                found += usize::from(names.id(1010011, &skin_texture_name(breed, gender, skin, part)).is_some());
            }
        }
    }
    println!("naked skin slots resolved by SetSkinData's name rule: {found}/{slots}");
    let (mut scenes, mut skinned) = (0, 0);
    for (breed, gender) in [(Breed::Solitus, Gender::Male), (Breed::Solitus, Gender::Female), (Breed::Opifex, Gender::Male), (Breed::Opifex, Gender::Female), (Breed::Nanomage, Gender::Male), (Breed::Nanomage, Gender::Female), (Breed::Atrox, Gender::Male)] {
        for build in 0..3 {
            scenes += 1;
            let p = Player::new(breed, gender, Skin::Caucasian, None);
            skinned += usize::from(load_player_build(&store, &p, build, None).is_ok_and(|s| s.textures.keys().filter(|k| k.rdb_type & 0x4000_0000 != 0).count() == 5));
        }
    }
    println!("player bodies (thin/normal/fat) with all five slots skin+default composited: {skinned}/{scenes}");
    Ok(())
}

fn bloom_survey(store: &RecordStore) -> anyhow::Result<()> {
    use ao_scene::Blend;
    let names = NameTable::load(store)?;
    let categories = ["raw_material_emissive", "effective_generic_emissive", "glow_mask", "unlit", "additive", "before_union", "after_union", "fullbright_emissive"];
    println!("bloom eligibility: authored bind pose, effect=0, no wire texture overrides; no visibility/fog/lighting/saturation or bloom-threshold claim");
    println!("raw_gamma is CAT emissive; material_linear is max(raw,0)^2.2; effective_linear=material_linear here");
    println!("live overrides (not surveyed): part_materials[mesh].emissive.or(actor.emissive).unwrap_or(material); explicit zero replaces, never adds (ao-render actors.rs)");
    println!("before_union=effective_generic_emissive|glow_mask|unlit|additive; after_union=fullbright_emissive|glow_mask|unlit|additive");
    println!("old generic accepts effective linear channels >0; fullbright_emissive retains only channels >=1, with no tolerance");
    println!("counts: records count once per category; submeshes are raw CAT groups, batches are assembled unique-material draws; empty index lists excluded");
    let (mut total_records, mut total_submeshes, mut total_batches) = ([0usize; 8], [0usize; 8], [0usize; 8]);
    let mut failures = 0;
    for ty in [CHAR_MESH_TYPE, CHAR_MESH_LOW_TYPE] {
        let mut ids = store.ids(ty)?;
        ids.sort_unstable();
        let (mut records, mut submeshes, mut batches) = ([0usize; 8], [0usize; 8], [0usize; 8]);
        let (mut decoded, mut raw_count, mut batch_count, mut failed) = (0, 0, 0, 0);
        for id in &ids {
            let loaded = load_cat_mesh(store, ty, *id).and_then(|cat| load_character_record(store, ty, *id, None).map(|scene| (cat, scene)));
            let (cat, scene) = match loaded {
                Ok(loaded) => loaded,
                Err(e) => {
                    println!("FAIL\t{ty}/{id}\tname={:?}\t{e:#}", names.name(ty, *id));
                    failed += 1;
                    continue;
                }
            };
            decoded += 1;
            raw_count += cat.submeshes.iter().filter(|s| !s.indices.is_empty()).count();
            // assemble() merges CAT groups by material, retaining first-use order.
            let mut materials = Vec::new();
            for s in cat.submeshes.iter().filter(|s| !s.indices.is_empty()) {
                if !materials.contains(&s.material) {
                    materials.push(s.material);
                }
            }
            let rendered: Vec<_> = scene.meshes.iter().flat_map(|m| &m.submeshes).collect();
            anyhow::ensure!(materials.len() == rendered.len(), "CAT assembly material count mismatch for {ty}/{id}");
            let mut record = [false; 8];
            let mut remaining = Vec::new();
            for (material, s) in materials.into_iter().zip(rendered) {
                batch_count += 1;
                let raw = &cat.materials[material as usize];
                let effective = s.emissive;
                let additive_blend = matches!(s.blend, Blend::Additive | Blend::AdditiveOne | Blend::SourceColorOne);
                let generic = !s.prelit && !s.liquid && s.lighting_enabled
                    && !additive_blend && effective.iter().any(|&c| c > 0.0);
                let glow = s.glow_mask && !s.prelit && !s.liquid && s.lighting_enabled && !additive_blend;
                let unlit = !s.lighting_enabled && !s.liquid;
                let additive = additive_blend && !s.liquid;
                let fullbright_channels = effective.map(|c| c >= 1.0);
                let fullbright = generic && fullbright_channels.iter().any(|&channel| channel);
                let explicit = glow || unlit || additive;
                let after = fullbright || explicit;
                let flags = [raw.emissive.iter().any(|&c| c > 0.0), generic, glow, unlit, additive, generic || explicit, after, fullbright];
                let multiplicity = cat.submeshes.iter().filter(|sm| sm.material == material && !sm.indices.is_empty()).count();
                for (i, yes) in flags.into_iter().enumerate() {
                    if yes {
                        record[i] = true;
                        submeshes[i] += multiplicity;
                        batches[i] += 1;
                    }
                }
                if flags[0] || flags[5] {
                    let reasons: Vec<_> = [(glow, "alpha_glow_mask"), (unlit, "native_unlit"), (additive, "authored_additive"), (fullbright, "fullbright_emissive")]
                        .into_iter().filter_map(|(yes, why)| yes.then_some(why)).collect();
                    println!("material\t{ty}/{id}\tname={:?}\tmaterial={material}:{:?}\ttexture={:?}\tflags={:#x}\tdefault_alpha_mode={}\topacity={}\traw_gamma={:?}\tmaterial_linear={:?}\teffective_linear={effective:?}\told_generic={generic}\tfullbright_channels={fullbright_channels:?}\tafter={after}\twhy={}\tblend={:?}\traw_submeshes={multiplicity}",
                        names.name(ty, *id), raw.name, s.texture, raw.flags, default_alpha_mode(raw.flags), raw.opacity, raw.emissive, s.emissive, reasons.join("+"), s.blend);
                    if after {
                        remaining.push(format!("{material}:{}:{}:effective_linear={effective:?}:fullbright_channels={fullbright_channels:?}", raw.name, reasons.join("+")));
                    }
                }
            }
            println!("record\t{ty}/{id}\tname={:?}\troot={:?}\traw_emissive={}\teffective_generic={}\tglow_mask={}\tunlit={}\tadditive={}\tbefore={}\tafter={}\tfullbright_emissive={}",
                names.name(ty, *id), cat.root, record[0], record[1], record[2], record[3], record[4], record[5], record[6], record[7]);
            if !remaining.is_empty() {
                println!("remaining\t{ty}/{id}\tname={:?}\troot={:?}\twhy={}", names.name(ty, *id), cat.root, remaining.join(";"));
            }
            for (i, yes) in record.into_iter().enumerate() {
                records[i] += usize::from(yes);
            }
        }
        println!("population\t{ty}\tauthored={}\tdecoded={decoded}\tfailed={failed}\traw_submeshes={raw_count}\tmaterial_batches={batch_count}", ids.len());
        for (i, category) in categories.iter().enumerate() {
            println!("summary\t{ty}\t{category}\trecords={}\tsubmeshes={}\tbatches={}", records[i], submeshes[i], batches[i]);
            total_records[i] += records[i];
            total_submeshes[i] += submeshes[i];
            total_batches[i] += batches[i];
        }
        failures += failed;
    }
    for (i, category) in categories.iter().enumerate() {
        println!("total\t{category}\trecords={}\tsubmeshes={}\tbatches={}", total_records[i], total_submeshes[i], total_batches[i]);
    }
    anyhow::ensure!(failures == 0, "{failures} CAT records failed; bloom population survey is incomplete");
    Ok(())
}

fn dirs_home() -> std::path::PathBuf {
    std::path::PathBuf::from(std::env::var_os("HOME").unwrap())
}
