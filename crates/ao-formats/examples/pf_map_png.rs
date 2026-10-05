//! `cargo run --release -p ao-formats --example pf_map_png -- <client dir> <playfield id> <max px> <out.png>`:
//! top-down ground image of a playfield (survey tool for `topdown`).
use ao_formats::{playfield::load_playfield_report, topdown};

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let dir = std::path::Path::new(&a[1]);
    let store = ao_rdb::RecordStore::open(dir)?;
    let t = std::time::Instant::now();
    let (scene, rep) = load_playfield_report(&store, dir, a[2].parse()?)?;
    let load = t.elapsed();
    let m = topdown::render(&scene, rep.ground, a[3].parse()?).ok_or_else(|| anyhow::anyhow!("no map"))?;
    image::save_buffer(&a[4], &m.rgba, m.width, m.height, image::ColorType::Rgba8)?;
    println!("{}x{} px, {:.2} m/px, origin {:?}, load {load:?}, total {:?}", m.width, m.height, m.mpp, m.origin, t.elapsed());
    Ok(())
}
