//! Lists the statel sound emitters of playfields and what their sound definitions resolve to
//! (`emitters <client_dir> <pf id>...`).
use std::path::PathBuf;

use ao_audio::Library;

fn main() {
    let mut a = std::env::args().skip(1);
    let dir = PathBuf::from(a.next().expect("client dir"));
    let lib = Library::load(&dir.join("cd_image/sound")).unwrap();
    let store = ao_rdb::RecordStore::open(&dir).unwrap();
    for id in a.map(|a| a.parse::<u32>().unwrap()) {
        let (_, r) = ao_formats::playfield::load_playfield_report(&store, &dir, id).unwrap();
        println!("playfield {id}: {} emitters", r.sounds.len());
        for e in r.sounds.iter().take(8) {
            let d = lib.sounds.get(e.sound_id);
            println!("  id {} r {} pos {:?} -> {:?}", e.sound_id, e.radius, e.pos.map(|v| v.round()), d.map(|d| (d.file.clone(), d.children.len(), d.prob, d.fade_out, d.min_dist, d.max_dist)));
        }
        let mut seen = std::collections::HashSet::new();
        for e in r.sounds.iter().filter(|e| seen.insert(e.sound_id)) {
            if let Some(d) = lib.sounds.get(e.sound_id).filter(|d| !d.children.is_empty()) {
                println!("  def {} flags {:#04x} play_all {} seq {} rand {} interval {}..{} dur {}..{} fade_out {}", d.id, d.flags, d.play_all, d.sequential, d.random_child, d.interval_min, d.interval_max, d.duration_min, d.duration_max, d.fade_out);
                for c in d.children.iter().filter_map(|c| lib.sounds.get(*c)) {
                    println!("    child {:?} interval {}..{} fade_out {} dur {}..{} vol {}", c.file, c.interval_min, c.interval_max, c.fade_out, c.duration_min, c.duration_max, c.vol_max);
                }
            }
        }
    }
}
