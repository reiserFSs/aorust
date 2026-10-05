//! Loads every playfield and prints ok/failed counts with grouped reasons and the slowest loads.
//!
//! `cargo run --release -p ao-formats --example pf_stats [client_dir] [id...]`

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Instant;

use ao_formats::playfield::{list_playfields, load_playfield_report};
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
                let dt = t.elapsed().as_secs_f64();
                if !only.is_empty() {
                    println!("{id} {name}: {dt:.2}s cells={} statels={} meshes={} instances={} missing={} failed={}", r.terrain_cells, r.statels, r.unique_meshes, scene.instances.len(), r.missing_meshes, r.failed_meshes);
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
