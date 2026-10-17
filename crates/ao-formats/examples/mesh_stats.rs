//! `--bloom`: whole archive population eligibility through the normal statel/attachment decoder.
//! No placement overrides, inherited texture, actor overrides, visibility, fog or pixel-threshold claim.
use ao_formats::mesh::{decode_record_into, MESH_LOW_TYPE, MESH_TYPE};
use ao_rdb::RecordStore;
use ao_scene::Scene;
use std::collections::BTreeMap;

fn main() -> anyhow::Result<()> {
    let store = RecordStore::open(&dirs_home().join("Games/ProjectRubiKa/client"))?;
    if std::env::args().any(|a| a == "--bloom") {
        return bloom_survey(&store);
    }
    for ty in [MESH_TYPE, MESH_LOW_TYPE] {
        let (mut ok, mut empty, mut verts, mut tris) = (0usize, 0usize, 0usize, 0usize);
        let mut notex = 0usize;
        let (mut blends, mut two_sided, mut subs) = ([0usize; 4], 0usize, 0usize);
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
                    }
                }
                Ok(None) => {}
                Err(e) => fails.entry(format!("{:#}", e).split(": ").last().unwrap().to_string()).or_default().push(id),
            }
        }
        println!("  winding vs normals: CCW-agree {agree}, disagree {disagree}");
        println!("type {ty}: geometry {ok}, no-geometry {empty}, verts {verts}, tris {tris}, untextured submeshes {notex}");
        let [opaque, test, blend, add] = blends;
        println!("  submeshes {subs}: opaque {opaque}, alpha-test {test}, alpha-blend {blend}, additive {add}; two-sided {two_sided}");
        println!("  emissive submeshes {emissive}, glow-mask submeshes {glow} (both {glow_emissive}), specular submeshes {specular}");
        for (k, v) in fails { println!("  FAIL x{} {k}: e.g. {:?}", v.len(), &v[..v.len().min(5)]); }
    }
    Ok(())
}
fn bloom_survey(store: &RecordStore) -> anyhow::Result<()> {
    use ao_formats::character::NameTable;
    use ao_scene::Blend;
    let names = NameTable::load(store)?;
    let categories = ["old_generic_emissive", "fullbright_emissive", "glow_mask", "unlit", "additive", "before_union", "after_union"];
    let (mut total_records, mut total_submeshes) = ([0usize; 7], [0usize; 7]);
    let mut failures = 0;
    println!("archive bloom eligibility: decode_record_into statel/attachment path; no placement texture/inherited state or actor overrides");
    println!("effective_linear=decoded material emissive; old generic channels >0, fullbright channels >=1 exactly; prelit/liquid excluded from generic");
    println!("before_union=old_generic_emissive|glow_mask|unlit|additive; after_union=fullbright_emissive|glow_mask|unlit|additive");
    println!("authored eligibility only: no visibility/fog/lighting/saturation or bloom-threshold claim; empty index lists excluded");
    for ty in [MESH_TYPE, MESH_LOW_TYPE] {
        let mut ids = store.ids(ty)?;
        ids.sort_unstable();
        let (mut records, mut submeshes) = ([0usize; 7], [0usize; 7]);
        let (mut decoded, mut empty, mut subs, mut failed) = (0, 0, 0, 0);
        for id in &ids {
            let mut scene = Scene::default();
            let index = match decode_record_into(store, ty, *id, &mut scene) {
                Ok(Some(index)) => index,
                Ok(None) => {
                    println!("FAIL\t{ty}/{id}\tname={:?}\tlisted record missing", names.name(ty, *id));
                    failed += 1;
                    continue;
                }
                Err(e) => {
                    println!("FAIL\t{ty}/{id}\tname={:?}\t{e:#}", names.name(ty, *id));
                    failed += 1;
                    continue;
                }
            };
            decoded += 1;
            let mut record = [false; 7];
            let mut record_subs = 0;
            for (si, s) in scene.meshes[index].submeshes.iter().enumerate().filter(|(_, s)| !s.indices.is_empty()) {
                subs += 1;
                record_subs += 1;
                let additive_blend = matches!(s.blend, Blend::Additive | Blend::AdditiveOne | Blend::SourceColorOne);
                let helper = !s.prelit && !s.liquid && s.lighting_enabled && !additive_blend;
                let generic = helper && s.emissive.iter().any(|&c| c > 0.0);
                let fullbright_channels = s.emissive.map(|c| c >= 1.0);
                let fullbright = helper && fullbright_channels.iter().any(|&c| c);
                let glow = helper && s.glow_mask;
                let unlit = !s.lighting_enabled && !s.liquid;
                let additive = additive_blend && !s.liquid;
                let explicit = glow || unlit || additive;
                let after = fullbright || explicit;
                let flags = [generic, fullbright, glow, unlit, additive, generic || explicit, after];
                for (i, yes) in flags.into_iter().enumerate() {
                    record[i] |= yes;
                    submeshes[i] += usize::from(yes);
                }
                if flags[5] || s.emissive.iter().any(|&c| c > 0.0) {
                    let reasons: Vec<_> = [(fullbright, "fullbright_emissive"), (glow, "alpha_glow_mask"), (unlit, "native_unlit"), (additive, "authored_additive")]
                        .into_iter().filter_map(|(yes, why)| yes.then_some(why)).collect();
                    let texture_name = s.texture.and_then(|key| names.name(key.rdb_type, key.id));
                    println!("material\t{ty}/{id}\tname={:?}\tsubmesh={si}\ttexture={:?}\ttexture_name={texture_name:?}\teffective_linear={:?}\tfullbright_channels={fullbright_channels:?}\told_generic={generic}\tafter={after}\twhy={}\tblend={:?}\tprelit={}\tliquid={}\tlighting={}\tglow_mask={}",
                        names.name(ty, *id), s.texture, s.emissive, reasons.join("+"), s.blend, s.prelit, s.liquid, s.lighting_enabled, s.glow_mask);
                    if after {
                        println!("remaining\t{ty}/{id}\tname={:?}\tsubmesh={si}\ttexture_name={texture_name:?}\twhy={}\teffective_linear={:?}", names.name(ty, *id), reasons.join("+"), s.emissive);
                    }
                }
            }
            empty += usize::from(record_subs == 0);
            for (i, yes) in record.into_iter().enumerate() {
                records[i] += usize::from(yes);
            }
        }
        println!("population\t{ty}\tauthored={}\tdecoded={decoded}\tfailed={failed}\tempty={empty}\tsubmeshes={subs}", ids.len());
        for (i, category) in categories.iter().enumerate() {
            println!("summary\t{ty}\t{category}\trecords={}\tsubmeshes={}", records[i], submeshes[i]);
            total_records[i] += records[i];
            total_submeshes[i] += submeshes[i];
        }
        failures += failed;
    }
    for (i, category) in categories.iter().enumerate() {
        println!("total\t{category}\trecords={}\tsubmeshes={}", total_records[i], total_submeshes[i]);
    }
    anyhow::ensure!(failures == 0, "{failures} archive records failed; bloom population survey is incomplete");
    Ok(())
}

fn dirs_home() -> std::path::PathBuf { std::env::var_os("HOME").unwrap().into() }
