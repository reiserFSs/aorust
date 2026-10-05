//! Lists the submeshes of mesh records: `mesh_subs <id>...`
use ao_rdb::RecordStore;
fn main() -> anyhow::Result<()> {
    let store = RecordStore::open(&std::path::PathBuf::from(std::env::var("HOME")?).join("Games/ProjectRubiKa/client"))?;
    for id in std::env::args().skip(1).filter_map(|a| a.parse().ok()) {
        let s = ao_formats::mesh::load_mesh(&store, id)?;
        let m = &s.meshes[0];
        println!("mesh {id}: {} verts", m.vertices.len());
        for (i, sub) in m.submeshes.iter().enumerate() {
            println!("  sub {i}: tris {} tex {:?} blend {:?} base {:?}", sub.indices.len() / 3, sub.texture.map(|k| (k.rdb_type, k.id)), sub.blend, sub.base_color);
        }
    }
    Ok(())
}
