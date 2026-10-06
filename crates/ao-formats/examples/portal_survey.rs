//! Lists every collision surface record (rdb 1000013) that carries a teleportal polygon: playfield, zone, polygon size and
//! bounds, destination bits (`n3SurfaceResource_t` +0x40: playfield in the low 16 bits).
//!
//! `cargo run --release -p ao-formats --example portal_survey [client_dir]`

use std::path::PathBuf;

use ao_formats::playfield::collision::kd;
use ao_rdb::RecordStore;

fn main() -> anyhow::Result<()> {
    let dir = match std::env::args().nth(1) {
        Some(d) => PathBuf::from(d),
        None => PathBuf::from(std::env::var("HOME")?).join("Games/ProjectRubiKa/client"),
    };
    let store = RecordStore::open(&dir)?;
    let (mut records, mut portals) = (0, 0);
    for id in store.ids(kd::SURFACE_TYPE)? {
        let Some((version, data)) = store.get_versioned(kd::SURFACE_TYPE, id)? else { continue };
        let s = kd::parse(version, &data)?;
        records += 1;
        if s.portal.is_empty() && s.portal_dest.is_none() {
            continue;
        }
        portals += 1;
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        for p in &s.portal {
            for i in 0..3 {
                lo[i] = lo[i].min(p[i]);
                hi[i] = hi[i].max(p[i]);
            }
        }
        println!(
            "pf {:5} zone {:4} v{version} pts {:2} x {:.1}..{:.1} y {:.1}..{:.1} z {:.1}..{:.1} dest {:#010x} (pf {})",
            id >> 16,
            id & 0xffff,
            s.portal.len(),
            lo[0],
            hi[0],
            lo[1],
            hi[1],
            lo[2],
            hi[2],
            s.portal_dest.unwrap_or(0),
            s.portal_dest.unwrap_or(0) & 0xffff
        );
    }
    println!("{records} surface records, {portals} with a teleportal");
    Ok(())
}
