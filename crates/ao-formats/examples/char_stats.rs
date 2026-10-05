//! Survey of all character records: `cargo run --release -p ao-formats --example char_stats`.
use ao_formats::character::*;
use ao_rdb::RecordStore;
use std::collections::{BTreeMap, HashMap};

fn main() -> anyhow::Result<()> {
    let store = RecordStore::open(&dirs_home().join("Games/ProjectRubiKa/client"))?;
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
    Ok(())
}

fn dirs_home() -> std::path::PathBuf {
    std::path::PathBuf::from(std::env::var_os("HOME").unwrap())
}
