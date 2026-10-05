//! Audit of what the world loaders do not draw: texture references of every mesh that fail to resolve or resolve to
//! the database's red "Error" placeholder, statels without mesh, and submeshes without texture in every playfield.
//!
//! `cargo run --release -p ao-formats --example world_audit [client_dir]`

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;

use ao_formats::mesh::{texture_refs, MESH_LOW_TYPE, MESH_TYPE};
use ao_formats::playfield::{list_playfields, load_playfield_report};
use ao_formats::texture::decode_texture;
use ao_rdb::RecordStore;
use ao_scene::{Scene, Texture, TextureKey};

/// The PRK "Error" image (rdb 1010009/1 and look-alikes): yellow frame, red field.
fn is_error_image(t: &Texture) -> bool {
    let px = |x: u32, y: u32| {
        let i = ((y.min(t.height - 1) * t.width + x.min(t.width - 1)) * 4) as usize;
        [t.rgba[i], t.rgba[i + 1], t.rgba[i + 2]]
    };
    let yellow = |p: [u8; 3]| p[0] > 200 && p[1] > 200 && p[2] < 90;
    let red = |p: [u8; 3]| p[0] > 200 && p[1] < 70 && p[2] < 70;
    yellow(px(0, 0)) && yellow(px(t.width - 1, t.height - 1)) && red(px(t.width / 8, t.height / 2)) && red(px(t.width * 7 / 8, t.height / 2))
}

#[derive(Default)]
struct Counts {
    refs: usize,
    missing: BTreeSet<(u32, u32)>,
    broken: BTreeSet<(u32, u32)>,
    error: BTreeSet<(u32, u32)>,
}

fn classify(store: &RecordStore, k: TextureKey, c: &mut Counts, cache: &mut HashMap<TextureKey, u8>) {
    let st = *cache.entry(k).or_insert_with(|| match store.get(k.rdb_type, k.id) {
        Ok(Some(b)) => match decode_texture(&b) {
            Ok(t) if is_error_image(&t) => 3,
            Ok(_) => 0,
            Err(_) => 2,
        },
        _ => 1,
    });
    c.refs += 1;
    match st {
        1 => drop(c.missing.insert((k.rdb_type, k.id))),
        2 => drop(c.broken.insert((k.rdb_type, k.id))),
        3 => drop(c.error.insert((k.rdb_type, k.id))),
        _ => {}
    }
}

fn main() -> anyhow::Result<()> {
    let dir = std::env::args().nth(1).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var("HOME").unwrap()).join("Games/ProjectRubiKa/client"));
    let store = RecordStore::open(&dir)?;
    let mut cache = HashMap::new();
    let probe = decode_texture(&store.get(1_010_009, 1)?.unwrap())?;
    println!("detector self-check on rdb 1010009/1: {}", is_error_image(&probe));
    let mut error_meshes: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for ty in [MESH_TYPE, MESH_LOW_TYPE] {
        let mut c = Counts::default();
        let (mut meshes, mut with_bad, mut by_type) = (0usize, 0usize, BTreeMap::<u32, usize>::new());
        for id in store.ids(ty)? {
            let Some(refs) = texture_refs(&store, ty, id)? else { continue };
            meshes += 1;
            let mut bad = false;
            for k in refs {
                *by_type.entry(k.rdb_type).or_default() += 1;
                classify(&store, k, &mut c, &mut cache);
                if matches!(cache[&k], 1..=3) {
                    bad = true;
                    if ty == MESH_TYPE && cache[&k] == 3 {
                        error_meshes.entry(k.id).or_default().push(id);
                    }
                }
            }
            with_bad += usize::from(bad);
        }
        println!("mesh type {ty}: {meshes} records, {} texture refs by type {by_type:?}; records with an unusable texture: {with_bad}", c.refs);
        println!("  missing texture records: {} {:?}", c.missing.len(), c.missing.iter().take(10).copied().collect::<Vec<_>>());
        println!("  undecodable: {} {:?}", c.broken.len(), c.broken.iter().take(10).copied().collect::<Vec<_>>());
        println!("  'Error' placeholder images: {} {:?}", c.error.len(), c.error.iter().take(10).copied().collect::<Vec<_>>());
    }
    for (tex, ms) in error_meshes.iter().take(20) {
        println!("  error texture {tex} used by meshes {:?}", &ms[..ms.len().min(8)]);
    }

    // per playfield: what ends up on screen
    let (mut statels, mut no_mesh, mut missing_mesh, mut subs, mut subs_untextured, mut subs_error, mut tris_untextured) = (0usize, 0usize, 0usize, 0usize, 0usize, 0usize, 0usize);
    let mut untextured_pf: Vec<(usize, u32)> = Vec::new();
    let mut error_pf: Vec<(usize, u32)> = Vec::new();
    for (id, _) in list_playfields(&store)? {
        let Ok((scene, r)) = load_playfield_report(&store, &dir, id) else { continue };
        statels += r.statels;
        no_mesh += r.no_mesh_statels;
        missing_mesh += r.missing_meshes;
        if !r.missing_mesh_ids.is_empty() {
            println!("  playfield {id}: statel mesh records absent from rdb 1010001/1010026/any type: {:?}", r.missing_mesh_ids);
        }
        let (mut u, mut e) = (0usize, 0usize);
        count_subs(&scene, &mut cache, &mut subs, &mut u, &mut e, &mut tris_untextured);
        subs_untextured += u;
        subs_error += e;
        if u > 0 {
            untextured_pf.push((u, id));
        }
        if e > 0 {
            error_pf.push((e, id));
        }
    }
    println!("playfields: statels {statels}, mesh id 0: {no_mesh}, mesh record absent: {missing_mesh}; scene submeshes {subs}: untextured {subs_untextured} ({tris_untextured} tris), error-texture {subs_error}");
    untextured_pf.sort_by_key(|a| std::cmp::Reverse(*a));
    error_pf.sort_by_key(|a| std::cmp::Reverse(*a));
    println!("  playfields with untextured submeshes (count,id): {:?}", &untextured_pf[..untextured_pf.len().min(15)]);
    println!("  playfields with error-texture submeshes (count,id): {:?}", &error_pf[..error_pf.len().min(15)]);
    Ok(())
}

fn count_subs(scene: &Scene, cache: &mut HashMap<TextureKey, u8>, subs: &mut usize, untextured: &mut usize, error: &mut usize, tris: &mut usize) {
    let used: BTreeSet<usize> = scene.instances.iter().map(|i| i.mesh).collect();
    for &mi in &used {
        for s in &scene.meshes[mi].submeshes {
            *subs += 1;
            match s.texture {
                None => {
                    *untextured += usize::from(s.base_color[..3] == [1.0; 3]);
                    *tris += s.indices.len() / 3;
                }
                Some(k) => {
                    let st = *cache.entry(k).or_insert_with(|| scene.textures.get(&k).map_or(1, |t| if is_error_image(t) { 3 } else { 0 }));
                    *error += usize::from(st == 3);
                }
            }
        }
    }
}
