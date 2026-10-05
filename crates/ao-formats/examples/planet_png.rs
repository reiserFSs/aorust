//! `cargo run --release -p ao-formats --example planet_png -- <client dir> <index rel> <level> <out.png>`:
//! writes the composed canvas of a planet map level (survey tool for `planetmap`).
use ao_formats::planetmap::PlanetMap;

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let m = PlanetMap::load(std::path::Path::new(&a[1]), &a[2])?;
    let level: usize = a[3].parse()?;
    let c = m.canvas(level)?;
    image::save_buffer(&a[4], &c.rgba, c.width, c.height, image::ColorType::Rgba8)?;
    println!("{} level {level}: {}x{} ({} playfields)", m.index.name, c.width, c.height, m.coords.len());
    Ok(())
}
