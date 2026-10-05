//! Real client data through the whole game audio chain, rendered offline (skips without the client).
use std::path::PathBuf;

use ao_audio::{Audio, PlayfieldAudio};
use ao_formats::playfield::load_playfield_report;
use ao_rdb::RecordStore;

fn client() -> Option<PathBuf> {
    let d = PathBuf::from(std::env::var_os("HOME")?).join("Games/ProjectRubiKa/client");
    d.join("cd_image/rdb.db").exists().then_some(d)
}

/// Runs `secs` of game time at 10 Hz and returns (max block RMS, last now_playing).
fn run(a: &Audio, secs: f32, cam: [f32; 3], day_time: f32) -> (f32, Option<String>) {
    let mut buf = vec![0f32; 4410 * 2];
    let (mut rms, mut np) = (0f32, None);
    for _ in 0..(secs * 10.0) as usize {
        a.update(0.1, cam, day_time);
        std::thread::sleep(std::time::Duration::from_millis(3)); // let the decoder threads run
        a.render(&mut buf);
        rms = rms.max((buf.iter().map(|s| s * s).sum::<f32>() / buf.len() as f32).sqrt());
        np = a.now_playing().or(np);
    }
    (rms, np)
}

#[test]
fn newland_city_music_and_ambience() {
    let Some(dir) = client() else { return };
    let store = RecordStore::open(&dir).unwrap();
    let (scene, report) = load_playfield_report(&store, &dir, 566).unwrap();
    let a = Audio::offline(&dir, 44100);
    a.set_playfield(Some(PlayfieldAudio::load(&store, 566, &report.sounds).unwrap()));
    let cam = scene.spawn.unwrap();
    // noon (3240): district 566 plays desert\Day (layer 0), ambience id 37
    let (rms, np) = run(&a, 6.0, cam, 3240.0);
    let np = np.expect("music started");
    assert!(np.to_ascii_lowercase().starts_with("d"), "desert day sample, got {np}");
    assert!(rms > 0.01, "music is audible: rms {rms}");
    eprintln!("566 noon: playing {np}, rms {rms:.4}, {}", a.status());
}

#[test]
fn ui_sounds_play() {
    let Some(dir) = client() else { return };
    let a = Audio::offline(&dir, 44100);
    assert!(!a.play_ui("SM_Sandy_CC_GUI_Select").is_empty());
    assert!(!a.play_ui("GUI_Mouseover").is_empty());
    let mut buf = vec![0f32; 44100 * 2 / 4];
    a.render(&mut buf);
    assert!(buf.iter().any(|s| s.abs() > 0.01));
    a.play_startup_music();
    let (_, np) = run(&a, 3.0, [0.0; 3], 0.0);
    assert!(np.is_some(), "startup music (mountain\\night)");
}
