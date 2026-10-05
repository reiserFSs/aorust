use ao_formats::mesh::{decode_record_into, MESH_LOW_TYPE, MESH_TYPE};
use ao_rdb::RecordStore;
use ao_scene::Scene;
use std::collections::BTreeMap;

fn main() -> anyhow::Result<()> {
    let store = RecordStore::open(&dirs_home().join("Games/ProjectRubiKa/client"))?;
    for ty in [MESH_TYPE, MESH_LOW_TYPE] {
        let (mut ok, mut empty, mut verts, mut tris) = (0usize, 0usize, 0usize, 0usize);
        let mut notex = 0usize;
        let (mut blends, mut two_sided, mut coloured_untex, mut tinted_tex, mut subs) = ([0usize; 4], 0usize, 0usize, 0usize, 0usize);
        let (mut agree, mut disagree, mut emissive, mut glow, mut glow_emissive) = (0usize, 0usize, 0usize, 0usize, 0usize);
        let mut specular = 0usize;
        let mut fails: BTreeMap<String, Vec<u32>> = BTreeMap::new();
        for id in store.ids(ty)? {
            let mut scene = Scene::default();
            match decode_record_into(&store, ty, id, &mut scene) {
                Ok(Some(i)) => {
                    let m = &scene.meshes[i];
                    if m.vertices.is_empty() { empty += 1 } else { ok += 1 }
                    verts += m.vertices.len();
                    for sub in &m.submeshes {
                        for t in sub.indices.as_chunks::<3>().0 {
                            let [a, b, c] = [0, 1, 2].map(|k| m.vertices[t[k] as usize]);
                            let e1: [f32; 3] = std::array::from_fn(|k| b.pos[k] - a.pos[k]);
                            let e2: [f32; 3] = std::array::from_fn(|k| c.pos[k] - a.pos[k]);
                            let cr = [e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]];
                            let d: f32 = (0..3).map(|k| cr[k] * (a.normal[k] + b.normal[k] + c.normal[k])).sum();
                            if d > 0.0 { agree += 1 } else if d < 0.0 { disagree += 1 }
                        }
                    }
                    tris += m.submeshes.iter().map(|s| s.indices.len() / 3).sum::<usize>();
                    notex += m.submeshes.iter().filter(|s| s.texture.is_none()).count();
                    for sub in &m.submeshes {
                        subs += 1;
                        blends[sub.blend as usize] += 1;
                        two_sided += usize::from(sub.two_sided);
                        let em = sub.emissive != [0.0; 3];
                        emissive += usize::from(em);
                        glow += usize::from(sub.glow_mask);
                        specular += usize::from(sub.specular != [0.0; 3] && sub.shininess > 0.0);
                        glow_emissive += usize::from(em && sub.glow_mask);
                        let tint = sub.base_color[..3] != [1.0; 3];
                        coloured_untex += usize::from(tint && sub.texture.is_none());
                        tinted_tex += usize::from(tint && sub.texture.is_some());
                    }
                }
                Ok(None) => {}
                Err(e) => fails.entry(format!("{:#}", e).split(": ").last().unwrap().to_string()).or_default().push(id),
            }
        }
        println!("  winding vs normals: CCW-agree {agree}, disagree {disagree}");
        println!("type {ty}: geometry {ok}, no-geometry {empty}, verts {verts}, tris {tris}, untextured submeshes {notex}");
        let [opaque, test, blend, add] = blends;
        println!("  submeshes {subs}: opaque {opaque}, alpha-test {test}, alpha-blend {blend}, additive {add}; two-sided {two_sided}; coloured untextured {coloured_untex}, tinted textured {tinted_tex}");
        println!("  emissive submeshes {emissive}, glow-mask submeshes {glow} (both {glow_emissive}), specular submeshes {specular}");
        for (k, v) in fails { println!("  FAIL x{} {k}: e.g. {:?}", v.len(), &v[..v.len().min(5)]); }
    }
    Ok(())
}
fn dirs_home() -> std::path::PathBuf { std::env::var_os("HOME").unwrap().into() }
