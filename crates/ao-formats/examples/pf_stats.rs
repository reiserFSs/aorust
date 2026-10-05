//! Loads every playfield and prints ok/failed counts with grouped reasons and the slowest loads.
//!
//! `cargo run --release -p ao-formats --example pf_stats [client_dir] [id...]`

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Instant;

use ao_formats::playfield::{floor_below, support_below, list_playfields, load_playfield_report, scene_bounds};
use ao_rdb::RecordStore;

fn main() -> anyhow::Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let dir = if args.first().is_some_and(|a| a.contains('/')) {
        PathBuf::from(args.remove(0))
    } else {
        PathBuf::from(std::env::var("HOME")?).join("Games/ProjectRubiKa/client")
    };
    let store = RecordStore::open(&dir)?;
    let all = list_playfields(&store)?;
    println!("playfields listed: {}", all.len());
    let only: Vec<u32> = args.iter().filter_map(|a| a.parse().ok()).collect();
    let (mut ok, mut failed) = (0usize, 0usize);
    let mut reasons: BTreeMap<String, Vec<u32>> = BTreeMap::new();
    let mut slow = Vec::new();
    let (mut statels, mut instances, mut mesh_fail, mut missing) = (0usize, 0usize, 0usize, 0usize);
    let mut mesh_errors: BTreeMap<String, usize> = BTreeMap::new();
    let mut spawn_bad: Vec<(u32, String)> = Vec::new();
    let mut empty: Vec<u32> = Vec::new();
    let mut lit: Vec<(usize, u32)> = Vec::new();
    let mut light_total = 0usize;
    let (mut sounds, mut fogs, mut no_mesh) = (0usize, 0usize, 0usize);
    let mut sound_ids = std::collections::BTreeSet::new();
    for (id, name) in &all {
        if !only.is_empty() && !only.contains(id) {
            continue;
        }
        let t = Instant::now();
        match load_playfield_report(&store, &dir, *id) {
            Ok((scene, r)) => {
                ok += 1;
                statels += r.statels;
                instances += scene.instances.len();
                mesh_fail += r.failed_meshes;
                missing += r.missing_meshes;
                if let Some(e) = r.first_mesh_error {
                    *mesh_errors.entry(e).or_default() += 1;
                }
                light_total += scene.lights.len();
                sounds += r.sounds.len();
                fogs += r.fogs.len();
                no_mesh += r.no_mesh_statels;
                sound_ids.extend(r.sounds.iter().map(|e| e.sound_id));
                if !scene.lights.is_empty() {
                    lit.push((scene.lights.len(), *id));
                }
                if let Some(l) = scene.lights.iter().find(|l| !(l.range > 0.0 && l.pos.iter().chain(&l.color).all(|v| v.is_finite()))) {
                    spawn_bad.push((*id, format!("{name}: invalid light {l:?}")));
                }
                if scene.instances.is_empty() {
                    empty.push(*id); // records without rooms: nothing to place a camera in
                } else if let Some(why) = spawn_problem(&scene) {
                    spawn_bad.push((*id, format!("{name}: {why}")));
                }
                let dt = t.elapsed().as_secs_f64();
                if !only.is_empty() {
                    println!("{id} {name}: {dt:.2}s cells={} statels={} meshes={} instances={} verts={} missing={} failed={} lights={}", r.terrain_cells, r.statels, r.unique_meshes, scene.instances.len(), scene.meshes.iter().map(|m| m.vertices.len()).sum::<usize>(), r.missing_meshes, r.failed_meshes, scene.lights.len());
                }
                slow.push((dt, *id, name.clone(), r.terrain_cells, r.statels));
            }
            Err(e) => {
                failed += 1;
                reasons.entry(format!("{e:#}")).or_default().push(*id);
            }
        }
    }
    println!("ok={ok} failed={failed} statels={statels} instances={instances} statels-without-mesh={missing} mesh-decode-failures={mesh_fail}");
    println!("spawn checks: {} ok, {} failed, {} empty playfields (no rooms/statels in the data: {empty:?})", ok - spawn_bad.len() - empty.len(), spawn_bad.len(), empty.len());
    lit.sort_by_key(|a| std::cmp::Reverse(a.0));
    println!("statel sound emitters {sounds} ({} distinct ids), fog volumes {fogs}, mesh-0 statels {no_mesh}", sound_ids.len());
    println!("lights: {light_total} in {} playfields; most: {:?}", lit.len(), &lit[..lit.len().min(8)]);
    if !only.is_empty() {
        println!("per playfield (count, id): {lit:?}");
    }
    for (id, why) in &spawn_bad {
        println!("SPAWN FAIL {id} {why}");
    }
    for (reason, ids) in &reasons {
        println!("FAILED x{}: {reason}  ids={:?}", ids.len(), &ids[..ids.len().min(12)]);
    }
    for (e, n) in &mesh_errors {
        println!("mesh error x{n}: {e}");
    }
    slow.sort_by(|a, b| b.0.total_cmp(&a.0));
    for (dt, id, name, cells, statels) in slow.iter().take(5) {
        println!("slowest: {id} {name}: {dt:.2}s ({cells} terrain cells, {statels} statels)");
    }
    Ok(())
}

/// Spawn must exist, lie inside the scene bounds, have terrain/floor below it (0.5..30 m) and look at a different point.
fn spawn_problem(scene: &ao_scene::Scene) -> Option<String> {
    let (Some(s), Some(at)) = (scene.spawn, scene.spawn_look_at) else { return Some("no spawn/look-at".into()) };
    let Some((lo, hi)) = scene_bounds(scene) else { return Some("empty scene".into()) };
    if (0..3).any(|i| s[i] < lo[i] || s[i] > hi[i] + if i == 1 { 50.0 } else { 0.0 }) {
        return Some(format!("spawn {s:?} outside bounds {lo:?}..{hi:?}"));
    }
    // terrain/room floor first; only a spawn hovering over floating statels (no ground there) may stand on their tops
    let floor = floor_below(scene, s).filter(|f| s[1] - f <= 30.0).or_else(|| support_below(scene, s));
    match floor {
        None => Some(format!("no floor below spawn {s:?}")),
        Some(f) if s[1] - f < 0.5 || s[1] - f > 30.0 => Some(format!("spawn {s:?} is {:.1} m above the floor", s[1] - f)),
        _ if (at[0] - s[0]).hypot(at[2] - s[2]) < 1.0 => Some(format!("look-at {at:?} is straight above/below spawn {s:?}")),
        _ => None,
    }
}
