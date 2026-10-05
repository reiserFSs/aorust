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

#[test]
fn statel_emitters_start_inside_radius_and_stop_after_leaving() {
    let Some(dir) = client() else { return };
    let store = RecordStore::open(&dir).unwrap();
    let (_, report) = load_playfield_report(&store, &dir, 566).unwrap();
    assert!(!report.sounds.is_empty());
    let a = Audio::offline(&dir, 44100);
    a.set_playfield(Some(PlayfieldAudio::load(&store, 566, &report.sounds).unwrap()));
    let far = [0.0, 5000.0, 0.0];
    run(&a, 2.0, far, 3240.0);
    assert_eq!(a.active_emitters(), 0);
    // on top of the machinery turbine (id 1000522882, r 25): its loop joins the mix
    let e = report.sounds.iter().find(|e| e.sound_id == 1000522882).expect("turbine emitter");
    run(&a, 2.0, e.pos, 3240.0);
    assert!(a.active_emitters() >= 1, "inside the radius");
    let loud = a.stats().voices.load(std::sync::atomic::Ordering::Relaxed);
    assert!(loud >= 2, "turbine loop + ambience/music voices: {loud}");
    // leave: 2 s hold + fade-out 2 s, then it is gone
    run(&a, 6.0, far, 3240.0);
    assert_eq!(a.active_emitters(), 0, "emitter stopped after leaving the radius");
}

#[test]
fn district_music_follows_day_period() {
    let Some(dir) = client() else { return };
    let store = RecordStore::open(&dir).unwrap();
    let layer_at = |pf: u32, day: f32| {
        let (scene, report) = load_playfield_report(&store, &dir, pf).unwrap();
        let a = Audio::offline(&dir, 44100);
        a.set_playfield(Some(PlayfieldAudio::load(&store, pf, &report.sounds).unwrap()));
        a.update(1.0, scene.spawn.unwrap(), day);
        a.music_layer()
    };
    // 566 Newland City: dawn/day/dusk = desert\Day, night = desert\Night (district 566 music ids, docs `## audio`)
    assert_eq!(layer_at(566, 3240.0).as_deref(), Some("desert\\Day"));
    assert_eq!(layer_at(566, 100.0).as_deref(), Some("desert\\Night"));
    assert_eq!(layer_at(566, 6000.0).as_deref(), Some("desert\\Night")); // 25 h
    assert_eq!(layer_at(152, 3240.0), None); // The Grid has no music
}

/// forest\Day pause track (anarchy.sws): 190 beats at 95 bpm of music (120 s), then 950 beats (600 s) of silence.
#[test]
fn pause_track_plays_then_pauses() {
    let Some(dir) = client() else { return };
    let a = Audio::offline(&dir, 44100);
    assert!(a.set_music_layer(Some("forest\\Day")));
    let (mut first_silence, mut resumed) = (None, None);
    for i in 0..8000 {
        let t = i as f32 * 0.1;
        a.update(0.1, [0.0; 3], 3240.0);
        let playing = a.now_playing().is_some();
        if !playing && first_silence.is_none() && t > 5.0 {
            first_silence = Some(t);
        }
        if playing && first_silence.is_some() && resumed.is_none() {
            resumed = Some(t);
        }
    }
    let (s, r) = (first_silence.expect("pause reached"), resumed.expect("music resumes"));
    assert!((118.0..135.0).contains(&s), "pause starts after ~120 s of music: {s}");
    assert!((r - s - 600.0).abs() < 6.0, "silence lasts ~600 s: {s} -> {r}");
}
