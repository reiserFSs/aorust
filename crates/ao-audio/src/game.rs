//! The client's game audio rules driven from data (docs/formats.md `## audio`): district music selection, the
//! day-period ambience layers, statel sound emitters and `.sbf` sound definitions.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use ao_formats::playfield::{zone_locator, SoundEmitter, ZoneLocator};
use ao_rdb::RecordStore;

use crate::combat::CombatMusic;
use crate::district::Districts;
use crate::engine::Shared;
use crate::music::{MusicPlayer, Rng};
use crate::sbf::{sound_id, SoundDb, SoundDef};
use crate::sws::Project;

/// rdb type of the per playfield district table (id = playfield id).
const DISTRICTS: u32 = 1_000_014;

/// `GameTime_t` (Gamecode ctor @0x1000af71): 27 hours of 3600 s per day; dawn 7..9 h, dusk 21..23 h.
const HOUR: f32 = 3600.0;
/// The viewer's clock (`GameDayTime`, 0..6480 s) is the 27 hour day at 240 s per game hour.
const HOURS_PER_VIEWER_SECOND: f32 = 1.0 / 240.0;
/// Ambience cross-fade half width (`_DAT_1015d640`, Gamecode).
const K: f32 = 300.0;

/// Parsed client sound data shared by everything.
pub struct Library {
    pub project: Arc<Project>,
    pub sounds: Arc<SoundDb>,
}

impl Library {
    /// `sound_dir` = `<client>/cd_image/sound`.
    pub fn load(sound_dir: &Path) -> Result<Library> {
        let read = |rel: &str| std::fs::read(sound_dir.join(rel)).with_context(|| rel.to_string());
        let project = Project::parse(&read("music/env/anarchy.sws")?).context("anarchy.sws")?;
        let mut sounds = SoundDb::parse(&read("SourceFiles/SM_Sandy_Game_Dummy.sbf")?).context("SM_Sandy_Game_Dummy.sbf")?;
        sounds.merge(SoundDb::parse(&read("SourceFiles/SM_Sandy_Gui.sbf")?).context("SM_Sandy_Gui.sbf")?);
        Ok(Library { project: Arc::new(project), sounds: Arc::new(sounds) })
    }
}

/// Everything the audio needs from one playfield.
pub struct PlayfieldAudio {
    pub id: u32,
    districts: Option<Districts>,
    zones: ZoneLocator,
    emitters: Vec<SoundEmitter>,
}

impl PlayfieldAudio {
    /// `sounds` = `Report::sounds` of the scene load (statel sound emitters, scene space).
    pub fn load(store: &RecordStore, id: u32, sounds: &[SoundEmitter]) -> Result<PlayfieldAudio> {
        let districts = match store.get(DISTRICTS, id)? {
            Some(d) => Some(Districts::parse(&d).with_context(|| format!("district table of playfield {id}"))?),
            None => None,
        };
        Ok(PlayfieldAudio { id, districts, zones: zone_locator(store, id)?, emitters: sounds.to_vec() })
    }

    fn district(&self, cam: [f32; 3]) -> Option<&crate::district::District> {
        self.districts.as_ref()?.district(self.zones.zone_at(cam)?)
    }
}

/// Day period of the music module (`UpdateDayPeriod`, Gamecode @0x1000b1c2) from the game hour 0..27.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Period {
    Dawn,
    Day,
    Dusk,
    Night,
}

impl Period {
    pub fn at_hour(h: f32) -> Period {
        match h {
            h if h < 7.0 => Period::Night,
            h if h < 9.0 => Period::Dawn,
            h if h < 21.0 => Period::Day,
            h if h < 23.0 => Period::Dusk,
            _ => Period::Night,
        }
    }

    /// Slot of `DistrictData::music[]` (`music_ids[idx]` with idx = DayPeriod + 1, ours is 0 based).
    fn music_slot(self) -> usize {
        match self {
            Period::Dawn => 0,
            Period::Day => 1,
            Period::Dusk => 2,
            Period::Night => 3,
        }
    }
}

/// Volume of the ambience layer of `period` at time-of-day `t` seconds (game seconds, 0..97200): a trapezoid
/// (`FUN_100ba221`): night 0,0,dawnS-K,dawnS; dawn dawnS-K,dawnS,dawnE-K,dawnE; day dawnE-K,dawnE,duskS-K,duskS;
/// dusk duskS-K,duskS,duskE-K,duskE. The client forces `t = 1` once `t > duskE - K`.
pub fn ambience_level(period: Period, t: f32) -> f32 {
    let (dawn_s, dawn_e, dusk_s, dusk_e) = (7.0 * HOUR, 9.0 * HOUR, 21.0 * HOUR, 23.0 * HOUR);
    let t = if t > dusk_e - K { 1.0 } else { t };
    let (a, b, c, d) = match period {
        Period::Night => (0.0, 0.0, dawn_s - K, dawn_s),
        Period::Dawn => (dawn_s - K, dawn_s, dawn_e - K, dawn_e),
        Period::Day => (dawn_e - K, dawn_e, dusk_s - K, dusk_s),
        Period::Dusk => (dusk_s - K, dusk_s, dusk_e - K, dusk_e),
    };
    if t < a || t >= d {
        0.0
    } else if t < b {
        (t - a) / (b - a)
    } else if t <= c {
        1.0
    } else {
        1.0 - (t - c) / (d - c)
    }
}

/// One running ambience layer: a keep-alive looping parent plus its timed child one-shots.
struct Ambient {
    voice: u64,
    timers: Vec<f32>,
}

/// State of one statel emitter (`PlayGameSound` called every frame while the camera is inside).
#[derive(Default)]
struct EmitterState {
    started: bool,
    /// looping voice of the definition's own file
    parent: u64,
    /// currently playing child one-shot
    child: u64,
    last_child: usize,
    /// per child: seconds until it fires (timed children of definitions with neither `play_all` nor `random_child`)
    timers: Vec<f32>,
    /// seconds since the camera was last inside the radius
    idle: f32,
    /// seconds until the next probability roll (`prob < 100` sounds roll at most once per second)
    gate: f32,
}

/// Duration override of statel emitter calls (`StatelSoundRun`, N3 @0x10024e97): the sound is kept alive 2 s after the
/// last call, then fades out over the definition's fade-out time.
const EMITTER_HOLD: f32 = 2.0;

/// `SandyInterface_t::PlaySample` distance rule (@0x10002d98): with a non-zero position, nothing plays beyond the max
/// distance and the level falls linearly from 1 at `min` to 0 at `max`. A caller radius replaces the max distance and
/// scales the min: `min = (def.min / def.max) * radius` when `def.min < def.max`, else `radius`.
pub fn attenuation(d: f32, def_min: f32, def_max: f32, radius: Option<f32>) -> f32 {
    let (min, max) = match radius {
        Some(r) if r > 0.0 => (if def_min < def_max { def_min / def_max * r } else { r }, r),
        _ => (def_min, def_max),
    };
    if d > max {
        0.0
    } else if d > min {
        ((max - min) - (d - min)) / (max - min)
    } else {
        1.0
    }
}

/// Mutable audio world state (one per [`crate::Audio`]).
pub(crate) struct Runtime {
    pub lib: Library,
    pub music: MusicPlayer,
    /// `SandyInterface_t` combat music state (`CombatUpdate`/`Frameprocess`/`ProcessCombatMusic`).
    pub combat: CombatMusic,
    pf: Option<PlayfieldAudio>,
    /// seconds since the last once-per-second music/district evaluation
    eval: f32,
    ambient: HashMap<(u16, Period), Ambient>,
    emitters: Vec<EmitterState>,
    /// ambience sound id of the camera's district
    want: Option<u16>,
    /// voices whose caller stopped re-triggering: (hold seconds left, voice, fade-out seconds)
    dying: Vec<(f32, u64, f32)>,
    rng: Rng,
    /// `Total_FX` (master x FX x mutes).
    pub fx: f32,
    /// Weather state floats s0..s6 (rain, fog, cloud, wind, sand, fallout R, fallout G storms); all 0 = clear.
    pub weather: [f32; 7],
    /// Module flag +0xbc: land-control areas of a district with a land-control level use `Landcontrol_neutral`.
    pub land_control: bool,
    /// Keep-alive UI sounds by sound id: (voice, seconds left, base level, fade-out).
    keepalive: HashMap<u32, (u64, f32, f32, f32)>,
}

impl Runtime {
    pub fn new(sh: &Arc<Shared>, lib: Library, seed: u64) -> Runtime {
        let music = MusicPlayer::new(sh.clone(), lib.project.clone(), seed);
        Runtime { lib, music, combat: CombatMusic::new(3), pf: None, eval: 1.0, ambient: HashMap::new(), emitters: Vec::new(), want: None, dying: Vec::new(), rng: Rng(seed.rotate_left(17) | 1), fx: 1.0, weather: [0.0; 7], land_control: false, keepalive: HashMap::new() }
    }

    /// Emitters currently inside the camera's radius (or in their 2 s hold).
    pub fn active_emitters(&self) -> usize {
        self.emitters.iter().filter(|e| e.started).count()
    }

    /// Plays a definition once, non-positionally, at `Total_FX`.
    pub fn play(&mut self, sh: &Shared, def: &SoundDef) -> Vec<u64> {
        let db = self.lib.sounds.clone();
        play_def(sh, &db, def, self.fx, &mut self.rng, None)
    }

    /// A positional one-shot (`PlayGameSound(id, pos, material, ..., size)`: doors, fight sounds): the definition once at `Total_FX` times the
    /// distance level of `attenuation` for the listener `d` metres away; nothing beyond the definition's maximum distance. `material` is the game
    /// material argument (`FabricType` of the struck creature, 7 = flesh of a player; 0 = none) and `size` the impact size 0 / 1 / 2 (1 = plain):
    /// a definition with material variants also starts the one `PlaySample` picks ([`variant_of`]).
    pub fn play_at(&mut self, sh: &Shared, def: &SoundDef, d: f32, material: i32, size: i32) -> Vec<u64> {
        let level = attenuation(d, def.min_dist, def.max_dist, None);
        if level <= 0.0 {
            return Vec::new();
        }
        let db = self.lib.sounds.clone();
        let variant = variant_of(def, material, size).and_then(|v| db.get(v));
        play_def(sh, &db, def, self.fx * level, &mut self.rng, variant)
    }

    /// SI PlaySoundCommand @100071ed: caller volume/radius/probability, material 0, size 1.
    pub fn play_effect_at(&mut self, sh: &Shared, def: &SoundDef, d: f32, volume: f32, radius: f32, probability: i32) -> Vec<u64> {
        if probability != 100 && (self.rng.next() % 200) as i32 >= probability { return Vec::new(); }
        let level = attenuation(d, def.min_dist, def.max_dist, Some(radius)) * volume;
        if level <= 0.0 { return Vec::new(); }
        let db = self.lib.sounds.clone();
        let variant = variant_of(def, 0, 1).and_then(|v| db.get(v));
        play_def(sh, &db, def, self.fx * level, &mut self.rng, variant)
    }

    /// `PlaySample` keep-alive (`SM_Sandy_CC_Ambience`, ...): each call sets the level and re-arms the sound to
    /// `fade_out + duration`; `update` ends it `T` seconds after the last call, fading linearly over its last
    /// `fade_out` seconds (`FrameProcessSound` @SI 0x10003b70).
    pub fn keepalive(&mut self, sh: &Shared, def: &SoundDef) {
        self.keepalive_scaled(sh, def, 1.0);
    }

    /// [`Runtime::keepalive`] with the per-call volume of `PlaySample(handle, .., volume, ..)` (weather: rain/wind levels).
    pub fn keepalive_scaled(&mut self, sh: &Shared, def: &SoundDef, scale: f32) {
        let level = def.vol_max * self.fx * scale;
        let t = def.fade_out + def.duration_max;
        if let Some(k) = self.keepalive.get_mut(&def.id) {
            if sh.mixer().is_playing(k.0) {
                *k = (k.0, t, level, def.fade_out);
                sh.mixer().set_gain(k.0, level);
                return;
            }
        }
        if let Some(p) = def.file.as_deref().and_then(|f| sh.resolve(f)) {
            let voice = sh.play_sample(&p, level, true, def.priority);
            if voice != 0 {
                self.keepalive.insert(def.id, (voice, t, level, def.fade_out));
            }
        }
    }

    pub fn set_playfield(&mut self, sh: &Shared, pf: Option<PlayfieldAudio>) {
        self.stop_ambience(sh, 0.0);
        self.emitters = pf.as_ref().map(|p| p.emitters.iter().map(|_| EmitterState::default()).collect()).unwrap_or_default();
        self.pf = pf;
        self.eval = 1.0; // evaluate immediately
    }

    fn stop_ambience(&mut self, sh: &Shared, fade: f32) {
        let mut m = sh.mixer();
        for (_, a) in self.ambient.drain() {
            m.fade(a.voice, 0.0, fade, true);
        }
    }

    /// Per frame: `cam` scene-space camera position, `day_time` the viewer clock (0..6480 s).
    pub fn update(&mut self, sh: &Shared, dt: f32, cam: [f32; 3], day_time: f32) {
        self.combat.update(dt);
        let hours = day_time.rem_euclid(6480.0) * HOURS_PER_VIEWER_SECOND;
        let period = Period::at_hour(hours);
        self.eval += dt;
        if self.pf.is_some() && self.eval >= 1.0 {
            self.eval = 0.0;
            let id = self.evaluate_district(period, cam);
            self.set_ambience_district(id);
        }
        self.music.tick(dt);
        self.keepalive.retain(|_, (voice, left, level, fade)| {
            *left -= dt;
            let mut m = sh.mixer();
            if *left <= 0.0 {
                m.stop(*voice);
                return false;
            }
            if *fade > *left {
                m.set_gain(*voice, *level * *left / *fade);
            }
            true
        });
        self.dying.retain_mut(|(hold, voice, fade)| {
            *hold -= dt;
            if *hold <= 0.0 {
                sh.mixer().fade(*voice, 0.0, *fade, true);
            }
            *hold > 0.0
        });
        self.tick_ambience(sh, dt, hours * HOUR);
        self.tick_emitters(sh, dt, cam);
    }

    /// The 1 Hz music module (`FUN_100b6d67`): district of the camera's zone -> `music[DayPeriod]` layer.
    /// Returns the district's ambience sound id.
    fn evaluate_district(&mut self, period: Period, cam: [f32; 3]) -> Option<u16> {
        let Some(d) = self.pf.as_ref().and_then(|p| p.district(cam)) else {
            // [INFERENCE] a combat state also applies without a district
            self.music.signal(self.combat_layer().flatten());
            return None;
        };
        // FUN_100b6d67: storm (max s3..s6 > 0.4) = slot 7, rain (s0 > 0.4) = 6, fog (s1 > 0.4) = 5, else the day period
        let w = &self.weather;
        let slot = if w[3..].iter().cloned().fold(0.0, f32::max) > 0.4 {
            6
        } else if w[0] > 0.4 {
            5
        } else if w[1] > 0.4 {
            4
        } else {
            period.music_slot()
        };
        let mut layer = d.music[slot] as usize;
        let sound_id = d.sound_id;
        if self.land_control && d.lc_lvl.0 != 0 {
            layer = self.lib.project.find_layer("Landcontrol_neutral").unwrap_or(0xffff);
        }
        let district = (layer != 0xffff && layer < self.lib.project.layers.len()).then_some(layer);
        self.music.signal(self.combat_layer().unwrap_or(district));
        Some(sound_id)
    }

    /// `PlayMusic` (SI @0x100062e4): while the combat state is 1..=16 the layer is the `SetCombatMusicOverride` layer
    /// (when `FindLayerID` finds it and it is not layer 0) or the table layer of the state (`None` = not found = silence).
    /// Returns `None` outside combat so that the requested (district) layer plays.
    pub fn combat_layer(&self) -> Option<Option<usize>> {
        let table = self.combat.layer_name()?;
        let p = &self.lib.project;
        let ov = self.combat.override_name().and_then(|n| p.find_layer(n)).filter(|&l| l != 0);
        Some(ov.or_else(|| p.find_layer(table)))
    }

    /// A district change fades the old id's layers out (4 s, the sound definitions' fade-out).
    fn set_ambience_district(&mut self, id: Option<u16>) {
        let stale: Vec<_> = self.ambient.keys().filter(|(i, _)| Some(*i) != id).copied().collect();
        for k in stale {
            if let Some(a) = self.ambient.remove(&k) {
                // keep-alive sound whose caller stopped: full level for its duration (1 s), then the 4 s fade-out
                self.dying.push((1.0, a.voice, 4.0));
            }
        }
        self.want = id;
    }

    /// Keeps the four `SM_Sandy_Env_Background{Night,Dawn,Day,Dusk}_<id>` layers of the district at their trapezoid
    /// level; children are independent timed one-shots at the parent's level.
    fn tick_ambience(&mut self, sh: &Shared, dt: f32, t: f32) {
        let Some(id) = self.want else { return };
        for (name, period) in [("Night", Period::Night), ("Dawn", Period::Dawn), ("Day", Period::Day), ("Dusk", Period::Dusk)] {
            let level = ambience_level(period, t) * self.fx;
            let key = (id, period);
            let def = self.lib.sounds.get(sound_id(&format!("SM_Sandy_Env_Background{name}_{id}"))).cloned();
            let Some(def) = def else { continue };
            if level <= 0.0 {
                if let Some(a) = self.ambient.remove(&key) {
                    sh.mixer().fade(a.voice, 0.0, def.fade_out.max(0.001), true);
                }
                continue;
            }
            let vol = def.vol_max * level;
            let alive = self.ambient.get(&key).is_some_and(|a| sh.mixer().is_playing(a.voice));
            if !alive {
                let Some(path) = def.file.as_deref().and_then(|f| sh.resolve(f)) else { continue };
                let voice = sh.play_sample(&path, vol, true, def.priority);
                if voice == 0 {
                    continue;
                }
                let timers = def.children.iter().map(|c| self.lib.sounds.get(*c).map_or(f32::MAX, |c| self.rng.unit() * (c.interval_max - c.interval_min).max(0.0) + c.interval_min)).collect();
                self.ambient.insert(key, Ambient { voice, timers });
                continue;
            }
            let a = self.ambient.get_mut(&key).unwrap();
            sh.mixer().set_gain(a.voice, vol);
            for (i, tm) in a.timers.iter_mut().enumerate() {
                *tm -= dt;
                if *tm <= 0.0 {
                    let Some(child) = self.lib.sounds.get(def.children[i]) else { *tm = f32::MAX; continue };
                    // re-armed with a fresh random interval of the child's own range
                    *tm = child.interval_min + self.rng.unit() * (child.interval_max - child.interval_min).max(0.0);
                    if let Some(p) = child.file.as_deref().and_then(|f| sh.resolve(f)) {
                        sh.play_sample(&p, vol, false, child.priority);
                    }
                }
            }
        }
    }

    /// `EvaluateStatelSoundFog` (N3 @0x10024eff, every frame): inside an emitter's radius `PlayGameSound(id, pos, 2 s,
    /// ..., radius)` is called each frame: a looping sound whose level tracks the camera distance
    /// (`radius`, linear), not restarted while it plays; 2 s after the last call it fades out. Children follow the
    /// definition's flags: `random_child` = one child at a time, re-picked (never the same twice) when it ends,
    /// `play_all` = all at start, otherwise timed one-shots per child interval [INFERENCE: flag semantics from the
    /// SandyInterface play list, see docs].
    fn tick_emitters(&mut self, sh: &Shared, dt: f32, cam: [f32; 3]) {
        let Some(pf) = &self.pf else { return };
        // dungeon rooms' emitters run only while the room is enabled: the camera's room (N3 `RunFunction` @0x100260d2, docs)
        let cam_room = pf.zones.zone_at(cam);
        for (e, st) in pf.emitters.iter().zip(&mut self.emitters) {
            let off = e.room.is_some_and(|r| Some(r as usize) != cam_room);
            let d = ((e.pos[0] - cam[0]).powi(2) + (e.pos[1] - cam[1]).powi(2) + (e.pos[2] - cam[2]).powi(2)).sqrt();
            let Some(def) = self.lib.sounds.get(e.sound_id) else { continue };
            if d >= e.radius || off {
                if st.started {
                    st.idle += dt;
                    if st.idle >= EMITTER_HOLD {
                        let mut m = sh.mixer();
                        for v in [st.parent, st.child] {
                            m.fade(v, 0.0, def.fade_out.max(0.001), true);
                        }
                        *st = EmitterState::default();
                    }
                }
                continue;
            }
            st.idle = 0.0;
            let level = attenuation(d, def.min_dist, def.max_dist, Some(e.radius)) * (def.vol_min + self.rng.unit() * (def.vol_max - def.vol_min)) * self.fx;
            if !st.started {
                st.gate -= dt;
                if st.gate > 0.0 {
                    continue;
                }
                if def.prob != 100 {
                    st.gate = 1.0;
                    if self.rng.next() % 200 >= def.prob as u32 {
                        continue;
                    }
                }
                st.started = true;
                st.last_child = usize::MAX;
                st.timers = def.children.iter().map(|c| self.lib.sounds.get(*c).map_or(f32::MAX, |c| c.interval_min + self.rng.unit() * (c.interval_max - c.interval_min).max(0.0))).collect();
                // (the parent loop is started below, where it is also retried)
                if def.play_all {
                    for c in def.children.iter().filter_map(|c| self.lib.sounds.get(*c)) {
                        if let Some(p) = c.file.as_deref().and_then(|f| sh.resolve(f)) {
                            sh.play_sample(&p, level, false, c.priority);
                        }
                    }
                }
            }
            // `AllocateChannel` found nothing (all 13 handles busy, none stealable): not played, asked again next frame
            // while the sound is active
            if st.parent == 0 {
                if let Some(p) = def.file.as_deref().and_then(|f| sh.resolve(f)) {
                    st.parent = sh.play_sample(&p, level, true, def.priority);
                }
            }
            {
                let mut m = sh.mixer();
                m.set_gain(st.parent, level);
                m.set_gain(st.child, level);
            }
            if def.random_child && !def.children.is_empty() && !sh.mixer().is_playing(st.child) {
                let n = def.children.len();
                let mut i = self.rng.next() as usize % n;
                if i == st.last_child {
                    i = (i + 1) % n;
                }
                st.last_child = i;
                if let Some((p, pr)) = self.lib.sounds.get(def.children[i]).and_then(|c| Some((sh.resolve(c.file.as_deref()?)?, c.priority))) {
                    st.child = sh.play_sample(&p, level, false, pr);
                }
            } else if !def.play_all && !def.random_child {
                for (i, tm) in st.timers.iter_mut().enumerate() {
                    *tm -= dt;
                    if *tm <= 0.0 {
                        let Some(c) = self.lib.sounds.get(def.children[i]) else { *tm = f32::MAX; continue };
                        *tm = c.interval_min + self.rng.unit() * (c.interval_max - c.interval_min).max(0.0);
                        if let Some(p) = c.file.as_deref().and_then(|f| sh.resolve(f)) {
                            sh.play_sample(&p, level, false, c.priority);
                        }
                    }
                }
            }
        }
    }
}

/// `SandyInterfaceModule_t::MapGameMaterialToSoundMaterial` [SI 0x10007490]: the game material (`FabricType` 1..=17, `ImpactEffectType`) to the sound
/// material index of the 46 `SoundDef::variants`; 0 and everything above 17 is 3.
pub fn sound_material(game: i32) -> i32 {
    match game {
        1..=11 => game,
        12 => 15,
        13 => 7,
        14 => 5,
        15 => 12,
        16 => 13,
        17 => 14,
        _ => 3,
    }
}

/// The variant sound `PlaySample` [SI 0x10002d98] starts next to a definition that has variants (`def+0x264`: any of the 46 ids non-zero): with
/// `m = sound_material(material)`, a `size` of 0 / 2 first prefers `variants[m + 30]` / `variants[m + 15]` when that one exists (size 1 = plain, `m`);
/// then `variants[m]`, else `variants[0]`, for `m` in 1..=45. `None`: no variants, `m` out of range or the slot empty.
pub fn variant_of(def: &SoundDef, material: i32, size: i32) -> Option<u32> {
    if def.variants.iter().all(|&v| v == 0) {
        return None;
    }
    let mut m = sound_material(material) as usize;
    if size != 1 && m != 0 {
        let shifted = match size {
            0 => m + 30,
            2 => m + 15,
            _ => m,
        };
        if def.variants.get(shifted).is_some_and(|&v| v != 0) {
            m = shifted;
        }
    }
    if !(1..=45).contains(&m) {
        return None;
    }
    [def.variants[m], def.variants[0]].into_iter().find(|&v| v != 0)
}

/// A one-shot non-positional sound definition (UI, children): probability gate, randomised volume, the file and
/// its children per the definition's flags (`play_all`, or one `random_child`).
pub(crate) fn play_def(sh: &Shared, db: &SoundDb, def: &SoundDef, fx: f32, rng: &mut Rng, variant: Option<&SoundDef>) -> Vec<u64> {
    if def.prob != 100 && rng.next() % 200 >= def.prob as u32 {
        return Vec::new();
    }
    let vol = (def.vol_min + rng.unit() * (def.vol_max - def.vol_min)) * fx;
    let mut out = Vec::new();
    // `PlaySample` starts the material variant first, at the definition's level (`SandyInterface_t::PlaySample` @0x10002d98, section "variants")
    if let Some(v) = variant {
        out.extend(play_def(sh, db, v, vol, rng, None));
    }
    let mut one = |d: &SoundDef| {
        if let Some(p) = d.file.as_deref().and_then(|f| sh.resolve(f)) {
            let id = sh.play_sample(&p, vol, false, d.priority);
            if id != 0 {
                out.push(id);
            }
        }
    };
    one(def);
    if def.play_all {
        for c in def.children.iter().filter_map(|c| db.get(*c)) {
            one(c);
        }
    } else if def.random_child && !def.children.is_empty() {
        if let Some(c) = db.get(def.children[rng.next() as usize % def.children.len()]) {
            one(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trapezoids_and_attenuation() {
        let h = HOUR;
        assert_eq!(ambience_level(Period::Night, 0.0), 1.0);
        assert_eq!(ambience_level(Period::Night, 7.0 * h - K), 1.0);
        assert!((ambience_level(Period::Night, 7.0 * h - K / 2.0) - 0.5).abs() < 1e-6);
        assert!((ambience_level(Period::Dawn, 7.0 * h - K / 2.0) - 0.5).abs() < 1e-6);
        assert_eq!(ambience_level(Period::Day, 12.0 * h), 1.0);
        assert_eq!(ambience_level(Period::Day, 12.0 * h) + ambience_level(Period::Night, 12.0 * h), 1.0);
        // forced to t = 1 late in the day: the night layer is back at full level, dusk is gone
        assert_eq!(ambience_level(Period::Night, 23.0 * h - K / 2.0), 1.0);
        assert_eq!(ambience_level(Period::Dusk, 23.0 * h - K / 2.0), 0.0);
        assert_eq!(Period::at_hour(6.99), Period::Night);
        assert_eq!(Period::at_hour(7.0), Period::Dawn);
        assert_eq!(Period::at_hour(12.0), Period::Day);
        assert_eq!(Period::at_hour(21.5), Period::Dusk);
        assert_eq!(Period::at_hour(26.0), Period::Night);
        // sbf default 0..15 m, caller radius 40 m: linear 1 -> 0 over the radius
        assert_eq!(attenuation(0.0, 0.0, 15.0, Some(40.0)), 1.0);
        assert!((attenuation(10.0, 0.0, 15.0, Some(40.0)) - 0.75).abs() < 1e-6);
        assert_eq!(attenuation(41.0, 0.0, 15.0, Some(40.0)), 0.0);
        // Native impact71345 passes radius120, not the definition's ordinary15m.
        assert_eq!(attenuation(60.0, 0.0, 15.0, Some(120.0)), 0.5);
        assert_eq!(attenuation(121.0, 0.0, 15.0, Some(120.0)), 0.0);
        assert!((attenuation(7.5, 0.0, 15.0, None) - 0.5).abs() < 1e-6);
        // min >= max: full level inside the radius, silent beyond
        assert_eq!(attenuation(10.0, 15.0, 15.0, Some(20.0)), 1.0);
    }

    fn def_with_variants(v: &[(usize, u32)]) -> SoundDef {
        let mut variants = vec![0; 46];
        for &(i, id) in v {
            variants[i] = id;
        }
        SoundDef { id: 1, flags: 1, file: None, vol_min: 0.5, vol_max: 0.5, min_dist: 0.0, max_dist: 15.0, fade_in: 0.0, fade_out: 0.0, duration_min: 0.0, duration_max: 0.0, prob: 100, children: vec![], interval_min: 0.0, interval_max: 0.0, play_all: false, sequential: false, random_child: false, priority: 1, variants }
    }

    /// `MapGameMaterialToSoundMaterial` [SI 0x10007490] (jump table @0x100074e3) and the variant choice of `PlaySample` [SI 0x10002d98].
    #[test]
    fn game_material_maps_to_a_variant_slot() {
        let map: Vec<i32> = (0..=18).map(sound_material).collect();
        assert_eq!(map, [3, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 15, 7, 5, 12, 13, 14, 3]);
        let d = def_with_variants(&[(0, 100), (7, 107), (22, 122), (37, 137), (3, 103)]);
        assert_eq!(variant_of(&d, 7, 1), Some(107), "size 1: variants[m]");
        assert_eq!(variant_of(&d, 7, 2), Some(122), "size 2 prefers variants[m + 15]");
        assert_eq!(variant_of(&d, 7, 0), Some(137), "size 0 prefers variants[m + 30]");
        assert_eq!(variant_of(&d, 5, 2), Some(100), "no slot 20 / 5: variants[0]");
        assert_eq!(variant_of(&d, 0, 1), Some(103), "material 0 is 3 for PlayGameSound");
        assert_eq!(variant_of(&d, 99, 0), Some(103), "unknown materials are 3 too (size 0: slot 33 is empty)");
        assert_eq!(variant_of(&def_with_variants(&[]), 7, 1), None, "a definition without variants plays alone");
        assert_eq!(variant_of(&def_with_variants(&[(0, 100)]), 7, 1), Some(100));
    }

    fn client() -> Option<std::path::PathBuf> {
        let d = std::path::PathBuf::from(std::env::var_os("HOME")?).join("Games/ProjectRubiKa/client");
        d.join("cd_image/rdb.db").exists().then_some(d)
    }

    fn runtime(dir: &std::path::Path) -> (Arc<Shared>, Runtime) {
        let sh = Shared::new(dir.join("cd_image/sound"), 44100);
        let lib = Library::load(&sh.root).unwrap();
        let rt = Runtime::new(&sh, lib, 1);
        (sh, rt)
    }

    /// `AllocateChannel` with every handle busy and nothing stealable returns NULL; the emitter's next frame asks again
    /// (it used to be armed once and never retried while the camera stayed inside).
    #[test]
    fn emitter_retries_when_the_pool_is_full() {
        use crate::decode::Pcm;
        use crate::mixer::{Source, VoiceDesc};
        let Some(dir) = client() else { return };
        let store = ao_rdb::RecordStore::open(&dir).unwrap();
        let (_, report) = ao_formats::playfield::load_playfield_report(&store, &dir, 566).unwrap();
        let (sh, mut rt) = runtime(&dir);
        let pf = PlayfieldAudio::load(&store, 566, &report.sounds).unwrap();
        let idx = pf.emitters.iter().position(|e| e.sound_id == 1000522882).expect("turbine emitter");
        let pos = pf.emitters[idx].pos;
        rt.set_playfield(&sh, Some(pf));
        let silence = Arc::new(Pcm { rate: 44100, channels: 1, samples: vec![0.0; 44100] });
        let pool: Vec<u64> = (0..13).map(|_| sh.mixer().play(VoiceDesc { priority: Some(0), looping: true, ..VoiceDesc::new(Source::Sample(silence.clone())) })).collect();
        assert!(pool.iter().all(|v| *v != 0));
        for _ in 0..20 {
            rt.tick_emitters(&sh, 1.0, pos);
        }
        assert!(rt.emitters[idx].started);
        assert_eq!(rt.emitters[idx].parent, 0, "pool full: not played");
        sh.mixer().stop(pool[0]);
        rt.tick_emitters(&sh, 0.1, pos);
        assert_ne!(rt.emitters[idx].parent, 0, "retried once a handle is free");
    }

    /// Ambience child one-shots are first armed with the child's own interval (re-arming already did).
    #[test]
    fn ambience_children_first_arm_uses_their_own_interval() {
        let Some(dir) = client() else { return };
        let (sh, mut rt) = runtime(&dir);
        let mut checked = 0;
        for id in 0u16..120 {
            let Some(def) = rt.lib.sounds.get(sound_id(&format!("SM_Sandy_Env_BackgroundDay_{id}"))).cloned() else { continue };
            if def.children.is_empty() || def.file.as_deref().and_then(|f| sh.resolve(f)).is_none() {
                continue;
            }
            rt.want = Some(id);
            rt.ambient.clear();
            rt.tick_ambience(&sh, 0.1, 15.0 * HOUR);
            let Some(a) = rt.ambient.get(&(id, Period::Day)) else { continue };
            for (tm, c) in a.timers.iter().zip(&def.children) {
                let Some(c) = rt.lib.sounds.get(*c) else { continue };
                assert!((c.interval_min..=c.interval_max.max(c.interval_min)).contains(tm), "id {id}: {tm} outside the child's {}..{}", c.interval_min, c.interval_max);
                checked += (c.interval_min != def.interval_min || c.interval_max != def.interval_max) as usize;
            }
            sh.mixer().stop_all();
        }
        assert!(checked > 0, "some ambience child has an interval different from its parent's");
    }
}
