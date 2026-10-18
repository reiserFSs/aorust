//! CPU-only acceptance survey of installed native steep-water source geometry.
use ao_formats::playfield::{list_playfields, load_playfield_at, wildlife_geometry};
use ao_rdb::RecordStore;

fn steep_triangle(points: [[f32; 3]; 3]) -> Option<(f32, [f32; 3], f32)> {
    let [a, b, c] = points;
    let u = std::array::from_fn::<_, 3, _>(|i| b[i] - a[i]);
    let v = std::array::from_fn::<_, 3, _>(|i| c[i] - a[i]);
    let n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
    let area = n.iter().map(|x| x * x).sum::<f32>().sqrt();
    let drop = a[1].max(b[1]).max(c[1]) - a[1].min(b[1]).min(c[1]);
    (area > 0.0 && n[1].abs() < 0.9 * area && drop > 0.0)
        .then(|| (area, std::array::from_fn(|i| (a[i] + b[i] + c[i]) / 3.0), drop))
}

#[test]
#[ignore = "requires installed client; surveys native playfields and loads a representative steep-water source"]
fn native_steep_water_source_loads() -> anyhow::Result<()> {
    let dir = std::path::PathBuf::from(std::env::var_os("HOME").expect("HOME"))
        .join("Games/ProjectRubiKa/client");
    let store = RecordStore::open(&dir)?;
    let mut largest = None;
    for (id, name) in list_playfields(&store)? {
        let geometry = wildlife_geometry(&store, id)?;
        let best = geometry.water_triangles.iter()
            .filter(|(_, kind)| matches!(ao_scene::liquid::style(*kind), Some(0 | 4)))
            .filter_map(|(points, kind)| steep_triangle(*points).map(|t| (t, *points, *kind)))
            .max_by(|a, b| a.0.0.total_cmp(&b.0.0));
        if let Some(((area, center, drop), points, kind)) = best {
            let foot = points.into_iter().min_by(|a, b| a[1].total_cmp(&b[1])).unwrap();
            let pool = geometry.water_triangles.iter().any(|(p, k)| {
                matches!(ao_scene::liquid::style(*k), Some(0 | 4))
                    && p.iter().all(|v| (v[1] - foot[1]).abs() < 1.0)
                    && p.iter().any(|v| (v[0] - foot[0]).hypot(v[2] - foot[2]) < 60.0)
            });
            let representative = (2.0..=80.0).contains(&drop) && pool;
            println!("PF {id} {name:?}: kind={kind} style={:?} twice_area={area} native_centroid={center:?} drop={drop} nearby_foot_pool={pool} vertices={points:?}", ao_scene::liquid::style(kind));
            if largest.as_ref().is_none_or(|(_, old_rank)| (representative, area) > *old_rank) {
                largest = Some((id, (representative, area)));
            }
        }
    }
    let (id, _) = largest.expect("installed native playfields must contain steep style0/4 water");
    let scene = load_playfield_at(&store, &dir, id, 2648.69)?;
    let best = scene.liquids.iter().filter(|s| matches!(s.style, 0 | 4))
        .flat_map(|s| s.triangles().map(move |t| (s.mesh, t.map(|v| v.pos))))
        .filter_map(|(mesh, points)| steep_triangle(points).map(|t| (t, mesh, points)))
        .max_by(|a, b| a.0.0.total_cmp(&b.0.0))
        .expect("full scene must retain native steep-water source");
    println!("LOADED PF {id}: mesh={} twice_area={} scene_centroid={:?} drop={} scene_vertices={:?}",
        best.1, best.0.0, best.0.1, best.0.2, best.2);
    Ok(())
}
