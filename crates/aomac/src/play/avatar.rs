//! The player's own character in the world: appearance from `SimpleCharFullUpdate`, animation clip choice and the
//! [`ao_scene::ActorFrame`] the renderer draws (always drawn, re-skinned every frame). Evidence: docs/zone/avatar.md.

use ao_formats::character::{
    actor::{ActorAssets, ActorRig, PlayerLook},
    head_table, CachedCharacter, CatAnim, ClothEntry, Role, Skin,
};
use ao_formats::screens::wire_breed_sex;
use ao_net::n3::dynel::SimpleCharFullUpdate;
use ao_rdb::RecordStore;
use ao_scene::{ActorFrame, Scene};
use anyhow::Result;
use glam::{Mat4, Vec3};
use std::{collections::HashMap, path::Path, sync::Arc};

/// Model key of the own avatar for `Renderer::add_actor_model` (`Host::actor_models`).
pub const MODEL_KEY: u64 = 0x4156_0000_0000_0001;

/// What the avatar should be doing; the movement state machine fills it in each frame.
#[derive(Clone, Debug, PartialEq)]
pub struct AvatarPose {
    pub role: Role,
    /// Current ground speed in m/s (`Vehicle_t+0x3c`); 0 for non-moving roles.
    pub speed: f32,
    /// Reference speed of the movement mode (`Vehicle_t+0x170`, [`ref_speed`]); the clip plays at `speed / ref_speed` times
    /// its calibrated rate ([`anim_rate`]).
    pub ref_speed: f32,
}

impl Default for AvatarPose {
    fn default() -> Self {
        Self::still(Role::Idle)
    }
}

impl AvatarPose {
    /// A role that does not move the character (idle, sit, emote): plays at the authored rate.
    pub fn still(role: Role) -> Self {
        Self { role, speed: 0.0, ref_speed: 1.0 }
    }
}

/// Reference speed `Vehicle_t+0x170` of a movement mode (`FUN_1006f4a2` @Gamecode: `FUN_100704e6` = mode, `FUN_100704ee` =
/// sub-mode): mode 3 with sub-mode 2 and mode 4 -> 3.0 (`_DAT_1015d69c`), mode 7 -> 7.0, mode 3 otherwise -> 5.0
/// (`_DAT_101574fc`), every other mode -> 1.5 (`_DAT_1015d76c`); mode 5 leaves the previous value (`None`).
#[cfg(test)]
pub fn ref_speed(mode: u32, sub_mode: u32) -> Option<f32> {
    match (mode, sub_mode) {
        (5, _) => None,
        (3, 2) | (4, _) => Some(3.0),
        (3, _) => Some(5.0),
        (7, _) => Some(7.0),
        _ => Some(1.5),
    }
}

/// Playback rate of a clip (`FUN_1006fb56` @Gamecode 0x1006fb56): `calibration * (100 / MonsterScale) * speed / ref_speed`,
/// capped at 1.3 (`_DAT_10160a8c`) when above it and the speed exceeds 4.0 m/s (`_DAT_10160a88`) unless `forced`; a rate that
/// is not positive leaves the authored rate 1.0 (the set-speed call is skipped).
pub fn anim_rate(calibration: f32, monster_scale_pct: f32, speed: f32, ref_speed: f32, forced: bool) -> f32 {
    let body = if monster_scale_pct == 0.0 { 1.0 } else { 100.0 / monster_scale_pct };
    let mut r = calibration * body * (speed / ref_speed);
    if r.is_nan() || r <= 0.0 {
        return 1.0;
    }
    if !forced && r > 1.3 && speed > 4.0 {
        r = 1.3;
    }
    r
}

/// `Setupf/animcalibration.txt`: `mesh anim factor` lines (`#` comments), the per-(body model, clip) movement calibration
/// factor of `AnimCalibrationControl_t` (constructed in `n3EngineClientAnarchy_t` ctor GC 0x10020f52, lookup `FUN_100017f7`:
/// 1.0 for a pair that is not in the table).
#[derive(Clone, Debug, Default)]
pub struct Calibration(HashMap<(u32, u32), f32>);

impl Calibration {
    pub fn load(client_dir: &Path) -> Self {
        std::fs::read_to_string(client_dir.join("Setupf/animcalibration.txt")).map(|t| Self::parse(&t)).unwrap_or_default()
    }

    /// A pair listed twice keeps its last line ([GUESS]: the table's insert policy was not traced).
    pub fn parse(text: &str) -> Self {
        let mut m = HashMap::new();
        for l in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
            let mut f = l.split_whitespace();
            if let (Some(Ok(a)), Some(Ok(b)), Some(Ok(c))) = (f.next().map(str::parse), f.next().map(str::parse), f.next().map(str::parse)) {
                m.insert((a, b), c);
            }
        }
        Self(m)
    }

    pub fn get(&self, model: u32, clip: u32) -> f32 {
        self.0.get(&(model, clip)).copied().unwrap_or(1.0)
    }
}

/// Everything `ActorRig::player` needs, read off the update (`SimpleChar_t` apply, GC 0x10077e13).
#[derive(Clone, Debug)]
pub struct AvatarLook {
    pub look: PlayerLook,
    /// `(AttractorPlace_e, rdb 1010001 mesh)` of the weapons / lights; the head (place 0) is `look.head`.
    pub attachments: Vec<(u8, u32)>,
    /// `MonsterScale / 100` (`SetBodyScale`).
    pub scale: f32,
}

impl AvatarLook {
    /// `skin_of_head` gives the skin race of a head mesh (creation head table), as the select-screen preview does.
    pub fn from_update(u: &SimpleCharFullUpdate, skin_of_head: impl FnOnce(u32) -> Skin) -> Result<Self> {
        let (breed, gender) = wire_breed_sex(u.breed as i32, u.sex as i32)?;
        // The head is delivered as stat HeadMesh and as attractor (place 0); the attractor wins like `CachedCharacter::head_mesh`.
        let head = u.attractors.iter().find(|a| a.place == 0).map(|a| a.mesh).or(u.head_mesh).filter(|&h| h > 0).map(|h| h as u32);
        // Worn cloth: page 0 only (`cloth[page * 5 + part]`, docs/zone/dynel.md §1.3); which page the renderer shows is not traced.
        let cloth = CachedCharacter {
            cloth: u.cloth.iter().filter(|c| c.page == 0).map(|c| ClothEntry { body_part: c.part(), texture: c.texture, ..Default::default() }).collect(),
            ..Default::default()
        };
        Ok(Self {
            look: PlayerLook {
                breed,
                gender,
                skin: head.map_or(Skin::Caucasian, skin_of_head),
                // Fatness 0 thin / 1 normal / 2 fat = `_thin` / `` / `_fat` models (GUI ChangeMesh 0x1011ab53); 3 is [GUESS] normal.
                build: if u.fatness == 0 { 0 } else if u.fatness == 2 { 2 } else { 1 },
                head,
                equipment: cloth.equipment(),
            },
            attachments: u.attractors.iter().filter(|a| a.place != 0 && a.mesh > 0).map(|a| (a.place, a.mesh as u32)).collect(),
            scale: if u.monster_scale > 0 { u.monster_scale as f32 / 100.0 } else { 1.0 },
        })
    }
}

/// Roles that play once and hold their last frame (the jump arcs, emotes, combat swings and death clips); every other clip
/// loops. [GUESS]: derived from the clip names, the original's per-clip loop flags were not traced (docs/zone/avatar.md §4).
fn one_shot(r: &Role) -> bool {
    matches!(r, Role::JumpStand | Role::JumpForward | Role::Emote(_) | Role::Clip(_))
}

/// Locomotion clips share a gait cycle: switching between them keeps the phase instead of restarting.
fn locomotion(r: &Role) -> bool {
    matches!(r, Role::Walk | Role::WalkBack | Role::WalkLeft | Role::WalkRight | Role::Run | Role::RunBack | Role::Sneak)
}

/// Clip to fall back to when the model's set lacks `r` (Atrox has fewer sets than the other breeds, creatures fewer still).
fn fallback(r: &Role) -> Option<Role> {
    match r {
        Role::RunBack => Some(Role::WalkBack),
        Role::Sneak | Role::WalkBack | Role::WalkLeft | Role::WalkRight | Role::Run => Some(Role::Walk),
        Role::IdleCombat | Role::SitChair | Role::SitGround | Role::Crawl | Role::JumpStand | Role::JumpForward => Some(Role::Idle),
        Role::Walk | Role::Idle => None,
        _ => Some(Role::Idle),
    }
}

/// The `[loopstart, loopend]` span of a clip (event markers of the CAT clip, e.g. walk 733..1733 ms of 2433): the clip is
/// `intro, loop, outro`; while the role holds, playback wraps from `loopend` back to `loopstart`.
fn loop_span(a: &CatAnim) -> Option<(f32, f32)> {
    let at = |n: &str| a.events.iter().find(|e| e.1 == n).map(|e| e.0 as f32);
    let (s, e) = (at("loopstart")?, at("loopend").unwrap_or(a.duration));
    (s < e && e <= a.duration).then_some((s, e))
}

/// Clip time for `ms` of playback: one-shots clamp to the last key; clips with a loop span play the intro once and then
/// repeat the span; the others wrap over their whole length.
pub fn clip_time(a: &CatAnim, ms: f32, once: bool) -> f32 {
    if a.duration <= 0.0 {
        return 0.0;
    }
    if once {
        return ms.min(a.duration);
    }
    match loop_span(a) {
        Some((s, e)) if ms >= e => s + (ms - s).rem_euclid(e - s),
        Some(_) => ms,
        None => ms.rem_euclid(a.duration),
    }
}

/// Phase (0..1) of `ms` inside the loop span, or of the whole clip without one.
fn phase(a: &CatAnim, ms: f32) -> f32 {
    let (s, e) = loop_span(a).unwrap_or((0.0, a.duration));
    ((clip_time(a, ms, false) - s) / (e - s)).clamp(0.0, 1.0)
}

pub struct Avatar {
    id: u32,
    rig: ActorRig,
    assets: ActorAssets,
    scale: f32,
    calibration: Calibration,
    pose: AvatarPose,
    clip: Option<Arc<CatAnim>>,
    /// rdb 1010003 id of `clip`.
    clip_id: u32,
    /// Playback rate of the current clip ([`anim_rate`]).
    rate: f32,
    /// `ItemDelay` of the weapon of the swing clip that plays ([`Avatar::set_swing_delay`]).
    swing_delay: Option<i32>,
    /// Milliseconds into the current clip.
    ms: f32,
    transform: Mat4,
}

impl Avatar {
    /// Builds the rig from the own dynel's update. `id` is the dynel instance id (`ActorFrame::id`).
    pub fn new(store: &RecordStore, client_dir: &Path, id: u32, u: &SimpleCharFullUpdate) -> Result<Self> {
        let assets = ActorAssets::new(store)?;
        let (breed, gender) = wire_breed_sex(u.breed as i32, u.sex as i32)?;
        let heads = head_table(store, breed, gender, 2)?;
        let l = AvatarLook::from_update(u, |h| heads.iter().find(|e| e.mesh == h).map_or(Skin::Caucasian, |e| e.skin))?;
        let rig = ActorRig::player(store, &assets, &l.look, &l.attachments)?;
        let mut a = Self { id, rig, assets, scale: l.scale, calibration: Calibration::load(client_dir), pose: AvatarPose::default(), clip: None, clip_id: 0, rate: 1.0, swing_delay: None, ms: 0.0, transform: Mat4::IDENTITY };
        a.set_pose(store, AvatarPose::default())?;
        a.set_transform([u.pos[0], u.pos[1], -u.pos[2]], u.yaw().map_or(0.0, |y| -y));
        Ok(a)
    }

    /// The model for `Host::actor_models` (key [`MODEL_KEY`]).
    pub fn model(&self) -> &Scene {
        self.rig.model()
    }

    /// Switches the clip when the role changes; a change between locomotion clips keeps the gait phase, any other restarts.
    pub fn set_pose(&mut self, store: &RecordStore, pose: AvatarPose) -> Result<()> {
        if pose.role != self.pose.role || self.clip.is_none() {
            let mut role = pose.role.clone();
            let clips = self.assets.clips(store, self.rig.model_id)?;
            let (clip, id) = loop {
                let name = role.clip_name();
                if let Some(&(_, id)) = clips.iter().find(|c| c.0 == name) {
                    break (Some(self.assets.anim(store, id)?), id);
                }
                match fallback(&role) {
                    Some(r) => role = r,
                    None => break (None, 0),
                }
            };
            self.ms = match (&self.clip, &clip) {
                (Some(old), Some(new)) if locomotion(&self.pose.role) && locomotion(&pose.role) && old.duration > 0.0 && new.duration > 0.0 => {
                    let (s, e) = loop_span(new).unwrap_or((0.0, new.duration));
                    s + phase(old, self.ms) * (e - s)
                }
                _ => 0.0,
            };
            self.clip = clip;
            self.clip_id = id;
        }
        self.rate = anim_rate(self.calibration.get(self.rig.model_id, self.clip_id), self.scale * 100.0, pose.speed, pose.ref_speed, false);
        // `FUN_1006a239`: a weapon swing is sped up so its first note lands within the weapon's ItemDelay
        if let (Some(d), Some(a)) = (self.swing_delay, &self.clip) {
            self.rate *= super::combat::anim::swing_speed_scale(a.events.first().map_or(0.0, |e| e.0 as f32), d);
        }
        self.pose = pose;
        Ok(())
    }

    /// `ItemDelay` (centiseconds) of the weapon whose swing clip is playing (`None`: no swing speed scale); set before [`Avatar::set_pose`].
    pub fn set_swing_delay(&mut self, delay: Option<i32>) {
        self.swing_delay = delay;
    }

    /// Whether a one-shot clip (jump) has played to its end.
    pub fn finished(&self) -> bool {
        one_shot(&self.pose.role) && self.clip.as_ref().is_none_or(|a| self.ms >= a.duration)
    }

    /// Scene position (feet) and rotation about +Y ([`super::zone::scene_yaw`] of the server heading).
    pub fn set_transform(&mut self, scene_pos: [f32; 3], scene_yaw: f32) {
        self.transform = Mat4::from_translation(Vec3::from(scene_pos)) * Mat4::from_rotation_y(scene_yaw) * Mat4::from_scale(Vec3::splat(self.scale));
    }

    /// Advances the clip by `dt` seconds at the pose's speed.
    pub fn update(&mut self, dt: f32) {
        self.ms += dt * 1000.0 * self.rate;
        // keep the counter bounded; looping clips wrap by themselves, one-shots stop at the end
        if let Some(a) = &self.clip {
            if !one_shot(&self.pose.role) && a.duration > 0.0 && self.ms > 4.0 * a.duration {
                self.ms = clip_time(a, self.ms, false);
            }
        }
    }

    /// The actor to draw this frame (CPU-skinned pose, never frustum culled).
    pub fn frame(&self) -> ActorFrame {
        let clip = self.clip.as_ref().map(|a| (&**a, clip_time(a, self.ms, one_shot(&self.pose.role))));
        let (skin, parts) = self.rig.pose(clip);
        ActorFrame { id: self.id, model: MODEL_KEY, transform: self.transform.to_cols_array_2d(), parts, skin: Some(skin), always: true }
    }

    /// `n3Dynel_t::GetBodyCollSphereRadi` (N3 0x10004dd3): the model's torso sphere radius (`VisualCATMesh_t::GetTorsoSphereRadi`), 0.5 when
    /// negative, times the body scale (`n3VisualDynel_t::UpdateCollision` N3 0x19be4).
    pub fn body_radius(&self) -> f32 {
        let r = self.rig.cat().torso_sphere.radius;
        (if r < 0.0 { ao_formats::playfield::collision::DEFAULT_BODY_RADIUS } else { r }) * self.scale
    }

    /// Body height in metres including `monster_scale` (eye height / name tag).
    #[cfg(test)]
    pub fn height(&self) -> f32 {
        self.rig.height() * self.scale
    }

    /// Height of the head attractor over the feet in the current pose, times the body scale: the camera look target
    /// (`FUN_10020af1` N3, docs/zone/camera.md §3). `None` for models without a head attractor.
    pub fn head_height(&self) -> Option<f32> {
        let clip = self.clip.as_ref().map(|a| (&**a, clip_time(a, self.ms, one_shot(&self.pose.role))));
        self.rig.head_attractor(clip).map(|p| p[1] * self.scale)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ao_net::{
        frame::Frame,
        n3::{self, dynel::Dynel, N3},
    };

    fn own_update() -> SimpleCharFullUpdate {
        let rec = include_str!("../../../../docs/captures/zone_newchar_ithaca.rec");
        rec.lines()
            .filter_map(|l| {
                let mut p = l.split(' ');
                let (_, dir, hex) = (p.next()?, p.next()?, p.next()?);
                let b: Vec<u8> = (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap()).collect();
                (dir == "<").then(|| Frame::decode_with(&b, false).ok().flatten().map(|(f, _)| f)).flatten()
            })
            .filter(|f| f.ptype == ao_net::frame::PT_N3)
            .find_map(|f| match n3::decode(&f).ok()?.body {
                N3::Dynel(Dynel::SimpleCharFullUpdate(u)) if u.name == "Aomacvolk" => Some(*u),
                _ => None,
            })
            .expect("own update in the capture")
    }

    #[test]
    fn look_of_the_captured_character() {
        let u = own_update();
        let l = AvatarLook::from_update(&u, |_| Skin::Caucasian).unwrap();
        assert_eq!((u.breed, u.sex), (1, 2), "Aomacvolk is a male solitus");
        assert_eq!((l.look.breed, l.look.gender, l.look.build), (ao_formats::character::Breed::Solitus, ao_formats::character::Gender::Male, 1));
        assert_eq!(l.look.head, u.head_mesh.map(|h| h as u32));
        assert!(l.look.head.is_some());
        assert!(l.attachments.is_empty(), "{:?}", l.attachments);
        assert_eq!(l.scale, 1.0);
        assert_eq!(l.look.equipment.0, [None; 5]);
    }

    #[test]
    fn weapons_and_cloth_come_from_the_update() {
        let mut u = own_update();
        u.fatness = 2;
        u.monster_scale = 150;
        u.attractors.push(ao_net::n3::dynel::AttractorMesh { place: 1, mesh: 7796, field: 0, byte: 2 });
        u.cloth.push(ao_net::n3::dynel::ClothData { raw: 1, texture: 154207, page: 0, extra: None });
        u.cloth.push(ao_net::n3::dynel::ClothData { raw: 2, texture: 99, page: 1, extra: None });
        let l = AvatarLook::from_update(&u, |_| Skin::Asian).unwrap();
        assert_eq!((l.look.build, l.scale, l.look.skin), (2, 1.5, Skin::Asian));
        assert_eq!(l.attachments, vec![(1, 7796)]);
        assert_eq!(l.look.equipment.0, [None, Some(154207), None, None, None]);
    }

    #[test]
    fn animation_rate_follows_the_original_formula() {
        // calibration 1.1, normal body, running at the reference speed
        assert_eq!(anim_rate(1.1, 100.0, 5.0, 5.0, false), 1.1);
        // a half-size body (MonsterScale 50) plays its clips twice as fast
        assert_eq!(anim_rate(1.0, 50.0, 3.0, 3.0, false), 2.0);
        // standing still / reversed: authored rate
        assert_eq!(anim_rate(1.0, 100.0, 0.0, 5.0, false), 1.0);
        // above 4 m/s the rate is capped at 1.3 unless forced
        assert_eq!(anim_rate(1.0, 100.0, 8.0, 5.0, false), 1.3);
        assert_eq!(anim_rate(1.0, 100.0, 8.0, 5.0, true), 1.6);
        assert_eq!(anim_rate(1.0, 100.0, 3.0, 1.5, false), 2.0, "slow movers are not capped");
        assert_eq!(ref_speed(3, 1), Some(5.0));
        assert_eq!(ref_speed(3, 2), Some(3.0));
        assert_eq!(ref_speed(5, 0), None);
    }

    #[test]
    fn calibration_file_parses() {
        let c = Calibration::parse("# c\n\n5900\t9382\t1.10\n5900 9386 0.99\n5900 9382 1.2\nbad line\n");
        assert_eq!((c.get(5900, 9386), c.get(5900, 1), c.get(1, 9382)), (0.99, 1.0, 1.0));
        assert_eq!(c.get(5900, 9382), 1.2);
        let Some(dir) = client() else { return };
        let real = Calibration::load(&dir);
        assert_eq!(real.get(5900, 9382), 1.10, "athrox_male sneakcool");
    }

    #[test]
    fn loop_markers_and_one_shots() {
        let a = CatAnim { root: String::new(), events: vec![(200, "loopstart".into())], version: 0, duration: 1000.0, signature: 0, param: 0.0, tracks: vec![] };
        assert_eq!(clip_time(&a, 500.0, false), 500.0);
        assert_eq!(clip_time(&a, 1100.0, false), 300.0, "wraps to loopstart, not 0");
        let c = CatAnim { events: vec![(200, "loopstart".into()), (600, "loopend".into())], ..a.clone() };
        assert_eq!(clip_time(&c, 500.0, false), 500.0);
        assert_eq!(clip_time(&c, 700.0, false), 300.0, "wraps at loopend");
        assert_eq!(phase(&c, 400.0), 0.5);
        assert_eq!(clip_time(&a, 5000.0, true), 1000.0);
        let b = CatAnim { events: vec![], ..a };
        assert_eq!(clip_time(&b, 1100.0, false), 100.0);
    }

    fn client() -> Option<std::path::PathBuf> {
        let d = std::path::PathBuf::from(std::env::var_os("HOME")?).join("Games/ProjectRubiKa/client");
        d.join("cd_image/rdb.db").exists().then_some(d)
    }

    /// The camera look target: the head attractor of the solitus male is at the face, bobs while running, and scales with the body.
    #[test]
    fn head_attractor_height_is_the_camera_look_target() {
        let Some(dir) = client() else { return };
        let store = RecordStore::open(&dir).unwrap();
        let mut a = Avatar::new(&store, &dir, 7, &own_update()).unwrap();
        let idle = a.head_height().expect("players have a head attractor");
        eprintln!("head attractor height (idle) {idle:.3} m, body {:.3} m", a.height());
        assert!(idle > 1.4 && idle < 1.8 && idle < a.height(), "{idle}");
        a.set_pose(&store, AvatarPose { role: Role::Run, speed: 5.0, ref_speed: 5.0 }).unwrap();
        let mut ys = Vec::new();
        for _ in 0..30 {
            a.update(0.02);
            ys.push(a.head_height().unwrap());
        }
        let (lo, hi) = ys.iter().fold((f32::MAX, f32::MIN), |(l, h), &y| (l.min(y), h.max(y)));
        assert!(hi - lo > 0.005 && hi - lo < 0.3, "run bob {lo}..{hi}");
        a.scale = 2.0;
        assert!((a.head_height().unwrap() / ys[29] - 2.0).abs() < 1e-3);
    }

    /// Real model: roles pick different clips, a pose moves the body, the frame is transformed by position + heading.
    /// With `AVATAR_SHOT=<png>` it also renders the character in the Arrival Hall (4604) offscreen, 3 m in front of it.
    #[test]
    fn avatar_poses_and_renders_in_the_arrival_hall() {
        let Some(dir) = client() else { return };
        let store = RecordStore::open(&dir).unwrap();
        let u = own_update();
        let mut a = Avatar::new(&store, &dir, 7, &u).unwrap();
        let idle = a.frame();
        a.set_pose(&store, AvatarPose { role: Role::Run, speed: 5.0, ref_speed: 5.0 }).unwrap();
        a.update(0.2);
        let run = a.frame();
        let (i, r) = (idle.skin.as_ref().unwrap(), run.skin.as_ref().unwrap());
        assert_eq!(i.len(), r.len());
        assert!(i.iter().zip(r).any(|(x, y)| x.pos != y.pos), "run moved nothing");
        assert!(a.height() > 1.4 && a.height() < 2.2, "{}", a.height());
        // jump plays once and reports it
        a.set_pose(&store, AvatarPose::still(Role::JumpStand)).unwrap();
        assert!(!a.finished());
        a.update(30.0);
        assert!(a.finished());

        if let Some(png) = std::env::var_os("AVATAR_SHOT") {
            let pos = super::super::zone::scene_pos(u.pos);
            a.set_pose(&store, AvatarPose { role: Role::Run, speed: 5.0, ref_speed: 5.0 }).unwrap();
            a.update(0.15);
            let yaw = u.yaw().unwrap_or(0.0);
            a.set_transform(pos, super::super::zone::scene_yaw(yaw));
            let scene = ao_formats::playfield::load_playfield(&store, &dir, 4604).unwrap();
            // the camera stands in front of the avatar and looks back at it
            let f = super::super::zone::scene_forward(yaw);
            let at = [pos[0], pos[1] + 1.0, pos[2]];
            let eye = [at[0] + f[0] * 3.0, at[1] + 0.3, at[2] + f[2] * 3.0];
            ao_render::render_to_png_actors(&scene, &[(MODEL_KEY, a.model().clone())], vec![a.frame()], eye, at, 900, 600, std::path::Path::new(&png), 0.0).unwrap();
        }
    }
}
