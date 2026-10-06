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
    let mut buf = vec![0f32; 4410 * 2]; // the device drains the mixer: finished fades release their stream handles
    for i in 0..8000 {
        let t = i as f32 * 0.1;
        a.update(0.1, [0.0; 3], 3240.0);
        a.render(&mut buf);
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

/// mountain\day has authored transitions into mountain\night: after the request the day layer keeps playing until such a
/// transition fires, then the night layer plays (no 2 s fade logic: SIM `SignalEvent` with force 0).
#[test]
fn layer_change_uses_authored_transition() {
    let Some(dir) = client() else { return };
    let a = Audio::offline(&dir, 44100);
    assert!(a.set_music_layer(Some("mountain\\day")));
    for _ in 0..30 {
        a.update(0.1, [0.0; 3], 3240.0);
    }
    assert!(a.set_music_layer(Some("mountain\\night")));
    assert_eq!(a.music_layer().as_deref(), Some("mountain\\night"));
    let first = a.now_playing().unwrap();
    assert!(first.to_ascii_lowercase().starts_with("md"), "{first}");
    let mut switched = None;
    for i in 0..1200 {
        a.update(0.1, [0.0; 3], 3240.0);
        std::thread::sleep(std::time::Duration::from_micros(200));
        if let Some(n) = a.now_playing() {
            if n.to_ascii_lowercase().starts_with("mn") {
                switched = Some((i as f32 * 0.1, n));
                break;
            }
        }
    }
    assert!(switched.is_some(), "night layer reached through an authored transition (was {first})");
}

#[test]
fn weather_slot_prefs_and_keepalive() {
    let Some(dir) = client() else { return };
    let store = RecordStore::open(&dir).unwrap();
    let (scene, report) = load_playfield_report(&store, &dir, 566).unwrap();
    let a = Audio::offline(&dir, 44100);
    a.set_playfield(Some(PlayfieldAudio::load(&store, 566, &report.sounds).unwrap()));
    a.update(1.0, scene.spawn.unwrap(), 3240.0);
    assert_eq!(a.music_layer().as_deref(), Some("desert\\Day"));
    // rain slot of district 566 = desert\Night (docs: fog/rain/storm desert\Night x3)
    a.set_weather([0.8, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
    a.update(1.0, scene.spawn.unwrap(), 3240.0);
    assert_eq!(a.music_layer().as_deref(), Some("desert\\Night"));

    // prefs: muted music / FX
    let b = Audio::offline(&dir, 44100);
    b.set_prefs(&ao_audio::Prefs { music_on: false, ..Default::default() });
    b.play_startup_music();
    let (rms, _) = run(&b, 2.0, [0.0; 3], 0.0);
    assert!(rms < 1e-6, "music muted: {rms}");

    // keep-alive: ends 5 s (1 s duration + 4 s fade-out) after the last call, level falls over the last 4 s
    let c = Audio::offline(&dir, 44100);
    c.play_ui_keepalive("SM_Sandy_CC_Ambience");
    let voices = |a: &Audio| a.stats().voices.load(std::sync::atomic::Ordering::Relaxed);
    let mut buf = vec![0f32; 4410 * 2];
    let mut levels = Vec::new();
    for i in 0..80 {
        c.update(0.1, [0.0; 3], 0.0);
        if i < 20 {
            c.play_ui_keepalive("SM_Sandy_CC_Ambience"); // caller keeps calling for 2 s
        }
        c.render(&mut buf);
        levels.push(voices(&c));
    }
    assert_eq!(levels[10], 1);
    assert_eq!(levels[79], 0, "gone after the keep-alive expired");
}

/// Live day -> night at 566 (`desert\Day` -> `desert\Night`): the authored data has no Day -> Night transition, so the
/// request waits (SIM `FUN_10008d2e`, force 0); the Day track's pause clock (94 bpm, 188 beats = 120 s) runs from the
/// start of Day and is not restarted by the request, so the music falls silent at ~120 s, and a request that arrives in
/// that silence starts the requested layer's entry sample at once (`SignalEvent` -> `Play(NULL, true)`).
#[test]
fn day_to_night_follows_the_authored_pause_clock() {
    let Some(dir) = client() else { return };
    let a = Audio::offline(&dir, 44100);
    assert!(a.set_music_layer(Some("desert\\Day")));
    let mut buf = vec![0f32; 4410 * 2];
    let mut silent_at = None;
    for i in 0..1500 {
        let t = i as f32 * 0.1;
        if i == 300 {
            assert!(a.set_music_layer(Some("desert\\Night"))); // 30 s into Day
        }
        a.update(0.1, [0.0; 3], 3240.0);
        a.render(&mut buf);
        match a.now_playing() {
            Some(n) if silent_at.is_none() => assert!(n.to_ascii_lowercase().starts_with("dday"), "{t}: {n}"),
            None if silent_at.is_none() && t > 5.0 => silent_at = Some(t),
            _ => {}
        }
    }
    let s = silent_at.expect("Day span ends");
    assert!((118.0..130.0).contains(&s), "silence ~120 s after Day started (not 120 s after the request): {s}");
    assert!(a.now_playing().is_none(), "still silent at 150 s");
    assert!(a.set_music_layer(Some("desert\\Day")));
    assert!(a.now_playing().unwrap().to_ascii_lowercase().starts_with("dday"));
}

/// All 16 combat table layers exist in the real `anarchy.sws`, and the state overrides / restores the district layer.
#[test]
fn combat_music_overrides_the_district_layer() {
    use ao_audio::combat::LAYERS;
    use ao_audio::CombatSample;
    let Some(dir) = client() else { return };
    let project = ao_audio::sws::Project::parse(&std::fs::read(dir.join("cd_image/sound/music/env/anarchy.sws")).unwrap()).unwrap();
    for n in LAYERS {
        assert!(project.find_layer(n).is_some(), "layer {n}");
    }
    let store = RecordStore::open(&dir).unwrap();
    let (scene, report) = load_playfield_report(&store, &dir, 566).unwrap();
    let a = Audio::offline(&dir, 44100);
    a.set_playfield(Some(PlayfieldAudio::load(&store, 566, &report.sounds).unwrap()));
    let cam = scene.spawn.unwrap();
    a.update(1.0, cam, 3240.0);
    assert_eq!(a.music_layer().as_deref(), Some("desert\\Day"));
    // fight an equal enemy: state 10 -> battle\Neutral at the next 1 Hz evaluation
    let me = CombatSample { is_self: true, health_pct: 100, level: 50, ..Default::default() };
    let foe = CombatSample { is_self: false, health_pct: 100, level: 50, id: 9, ..Default::default() };
    a.set_combat_sample(&me);
    a.set_combat_sample(&foe);
    a.update(1.0, cam, 3240.0);
    assert_eq!(a.combat_state(), 10);
    assert_eq!(a.music_layer().as_deref(), Some("battle\\Neutral"));
    // override name replaces the table layer
    a.set_combat_music_override(Some("EP_01\\Battle_new"));
    a.set_combat_sample(&me);
    a.set_combat_sample(&foe);
    a.update(1.0, cam, 3240.0);
    assert_eq!(a.music_layer().as_deref(), Some("EP_01\\Battle_new"));
    a.set_combat_music_override(None);
    // the fight is over (no samples): state 0, district layer back
    a.update(1.0, cam, 3240.0);
    assert_eq!(a.combat_state(), 0);
    assert_eq!(a.music_layer().as_deref(), Some("desert\\Day"));
}

/// `PlayGameSound` of a door's open sound (`Door` template 41565 of 4582: 0xcfde8382): audible next to the door, silent beyond the
/// definition's maximum distance.
#[test]
fn game_sound_is_positional_by_distance_only() {
    let Some(dir) = client() else { return };
    let a = Audio::offline(&dir, 44100);
    let pos = [10.0, 2.0, -5.0];
    assert!(!a.play_game_sound(0xcfde8382, pos, [10.0, 2.0, -3.0]).is_empty());
    let mut buf = vec![0f32; 44100 / 2];
    a.render(&mut buf);
    assert!(buf.iter().any(|s| s.abs() > 0.001), "the door sound is audible next to the door");
    assert!(a.play_game_sound(0xcfde8382, pos, [500.0, 2.0, -5.0]).is_empty(), "nothing beyond the maximum distance");
    assert!(a.play_game_sound(0x1234_5678, pos, pos).is_empty(), "unknown ids are ignored");
}

/// The sound multimaps of the weapon records (rdb 1000020, `WeaponItem` 0xC74A; docs/zone/combat-anim.md section 6): the ids resolve in the sbf, the
/// martial-arts item of the bare-handed player has the swing / impact / swish sounds, and a variant sound plays for a creature material.
#[test]
fn weapon_sounds_resolve() {
    let Some(dir) = client() else { return };
    let store = RecordStore::open(&dir).unwrap();
    let lib = ao_audio::Library::load(&dir.join("cd_image/sound")).unwrap();
    let (mut weapons, mut with_sounds, mut ids, mut missing) = (0, 0, 0, 0);
    let mut keys = std::collections::BTreeSet::new();
    for id in store.ids(ao_formats::dynel_visual::ITEM_TEMPLATE_TYPE).unwrap() {
        let Some(t) = ao_formats::dynel_visual::item_template(&store, id).ok().flatten().filter(|t| t.kind == 0xC74A) else { continue };
        weapons += 1;
        with_sounds += usize::from(!t.sounds.is_empty());
        for (k, v) in &t.sounds {
            keys.insert(*k);
            for s in v {
                ids += 1;
                missing += usize::from(lib.sounds.get(*s).is_none());
            }
        }
    }
    eprintln!("weapon records {weapons}, with sounds {with_sounds}, sound ids {ids}, not in the sbf {missing}, keys {keys:?}");
    assert!(with_sounds > 16000 && ids > 8000, "{with_sounds} / {ids}");
    assert!(missing * 100 < ids, "{missing} of {ids} ids are not in the sbf");
    for k in [0xb, 0x1f] {
        assert!(keys.contains(&k), "key {k:#x}");
    }
    let martial = ao_formats::dynel_visual::item_template(&store, 43712).unwrap().unwrap();
    for (k, v) in &martial.sounds {
        assert!(v.iter().all(|s| lib.sounds.get(*s).is_some()), "key {k:#x}");
    }
    let a = Audio::offline(&dir, 44100);
    let at = [0.0; 3];
    // 0x7199b60d (martial-arts impact) only has variants: nothing without a material that has one, flesh (7) plays
    assert!(!a.play_game_sound_with(0x7199b60d, at, at, 7, 1).is_empty(), "the flesh variant plays");
}
