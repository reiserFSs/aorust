//! Survey of rdb 1040023 (NPC records): parse rate, mesh / animation existence and skeleton compatibility, key usage.
//! `cargo run --release -p ao-formats --example npc_stats [id…]` (ids: dump those records' words instead).
use ao_formats::character::*;
use ao_rdb::RecordStore;
use std::collections::{BTreeMap, HashMap};

fn main() -> anyhow::Result<()> {
    let dir = std::path::PathBuf::from(std::env::var("HOME")?).join("Games/ProjectRubiKa/client");
    let store = RecordStore::open(&dir)?;
    let ids: Vec<u32> = std::env::args().skip(1).filter_map(|a| a.parse().ok()).collect();
    if !ids.is_empty() {
        for id in ids {
            let d = store.get(NPC_TYPE, id)?.expect("no record");
            let rec = NpcRecord::parse(&d)?;
            println!("{id} len {} name {:?} stats {:x?} pairs {:?} trailing {}", d.len(), rec.name, rec.stats, rec.pairs, rec.trailing);
            let nt = NameTable::load(&store)?;
            for (k, v) in &rec.anims {
                let n: Vec<_> = v.iter().map(|a| format!("{a}={}", nt.name(1010003, *a).unwrap_or("?"))).collect();
                println!("  anim {k:#x} ({k}) -> {}", n.join(", "));
            }
            for (k, v) in &rec.sounds {
                println!("  sound {k:#x} ({k}) -> {v:x?}");
            }
        }
        return Ok(());
    }
    let names = NameTable::load(&store)?;
    let all = store.ids(NPC_TYPE)?;
    let (mut parsed, mut no_mesh, mut mesh_ok, mut head_ok, mut head_n) = (0, 0, 0, 0, 0);
    let (mut anim_n, mut anim_ok, mut sig_ok, mut pairs_n, mut tail_n, mut unnamed) = (0, 0, 0, 0, 0, 0);
    let mut tails: BTreeMap<usize, usize> = BTreeMap::new();
    let mut skeleton: HashMap<u32, Option<u32>> = HashMap::new(); // mesh id -> signature
    let mut anim_sig: HashMap<u32, Option<u32>> = HashMap::new();
    let mut keys: BTreeMap<u32, (usize, HashMap<String, usize>)> = BTreeMap::new();
    let mut skel_keys: BTreeMap<u32, (usize, usize)> = BTreeMap::new(); // key -> (anims, compatible)
    let mut stat_use: BTreeMap<u32, usize> = BTreeMap::new();
    for &id in &all {
        let rec = match NpcRecord::load(&store, id) {
            Ok(r) => r,
            Err(e) => {
                println!("FAIL {id}: {e:#}");
                continue;
            }
        };
        parsed += 1;
        pairs_n += !rec.pairs.is_empty() as usize;
        unnamed += rec.name.is_empty() as usize;
        if rec.trailing > 0 {
            tail_n += 1;
            *tails.entry(rec.trailing).or_default() += 1;
        }
        for s in &rec.stats {
            *stat_use.entry(s.0).or_default() += 1;
        }
        let Some(mesh) = rec.mesh() else {
            no_mesh += 1;
            continue;
        };
        let sig = *skeleton.entry(mesh).or_insert_with(|| load_cat_mesh(&store, CHAR_MESH_TYPE, mesh).ok().map(|m| m.signature));
        mesh_ok += sig.is_some() as usize;
        if let Some(h) = rec.head_mesh() {
            head_n += 1;
            head_ok += names.name(1010001, h).is_some() as usize;
        }
        for (key, vals) in &rec.anims {
            for &a in vals {
                anim_n += 1;
                let asig = *anim_sig.entry(a).or_insert_with(|| load_anim(&store, a).ok().map(|x| x.signature));
                anim_ok += asig.is_some() as usize;
                let ok = asig.is_some() && asig == sig;
                sig_ok += ok as usize;
                if !ok {
                    println!("MISMATCH npc {id} {:?} mesh {mesh} key {key:#x} anim {a} {:?}", rec.name, names.name(1010003, a));
                }
                let e = skel_keys.entry(*key).or_default();
                e.0 += 1;
                e.1 += ok as usize;
                let k = keys.entry(*key).or_default();
                k.0 += 1;
                let n = names.name(1010003, a).unwrap_or("?");
                // `<model>_<clip>[NN].ani` -> `<clip>`; model names may hold `-`/`_`, so keep what follows the last `_`/`-` that precedes a letter run
                let stem = n.trim_end_matches(".ani").trim_end_matches(|c: char| c.is_ascii_digit() || c == ' ' || c == '_');
                let clip = stem.rsplit(['_', '-', ' ']).next().unwrap_or(stem);
                *k.1.entry(clip.to_ascii_lowercase()).or_default() += 1;
            }
        }
    }
    let pct = |a: usize, b: usize| if b == 0 { 100.0 } else { 100.0 * a as f64 / b as f64 };
    println!("records {} parsed {parsed} ({:.2}%)", all.len(), pct(parsed, all.len()));
    println!("with mesh stat {} / no mesh override {no_mesh}", parsed - no_mesh);
    println!("mesh exists in 1010002: {mesh_ok}/{} ({:.2}%)", parsed - no_mesh, pct(mesh_ok, parsed - no_mesh));
    println!("head mesh exists in 1010001: {head_ok}/{head_n}");
    println!("anim ids {anim_n}: exist in 1010003 {anim_ok} ({:.2}%), skeleton signature == model {sig_ok} ({:.2}%)", pct(anim_ok, anim_n), pct(sig_ok, anim_n));
    println!("non-empty 4th list {pairs_n}, unnamed {unnamed}, records with trailing bytes {tail_n} {tails:?}");
    println!("stat ids in overrides: {stat_use:?}");
    for (k, (n, h)) in &keys {
        let mut v: Vec<_> = h.iter().collect();
        v.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        let top: Vec<String> = v.iter().take(4).map(|(s, c)| format!("{s}:{c}")).collect();
        let (a, c) = skel_keys[k];
        println!("key {k:#06x} ({k}) n={n} compat {c}/{a}: {}", top.join(" "));
    }
    assert_eq!(parsed, all.len(), "every record parses");
    assert_eq!(mesh_ok, parsed - no_mesh, "every mesh exists");
    assert_eq!(anim_ok, anim_n, "every anim exists");
    Ok(())
}
