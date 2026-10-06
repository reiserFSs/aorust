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
    /// Maximum speed of the current movement mode in m/s (`Vehicle_t+0x3c`); 0 for non-moving roles.
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
        // Fullupdate's base ClearAttractors only clears bookkeeping (DS 0x10071dd0), not the HeadMesh child.
        let skip_attractors = u.flags & ao_net::n3::dynel::flag::SET_DYNEL_800 != 0;
        let head = (!skip_attractors).then(|| u.attractors.iter().find(|a| a.place == 0).map(|a| a.mesh)).flatten().or(u.head_mesh).filter(|&h| h > 0).map(|h| h as u32);
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
            attachments: u.attractors.iter().filter(|a| !skip_attractors && a.place != 0 && a.mesh > 0).map(|a| (a.place, a.mesh as u32)).collect(),
            scale: if u.monster_scale > 0 { u.monster_scale as f32 / 100.0 } else { 1.0 },
        })
    }
}

/// Roles that play once and hold their last frame (the jump arcs, emotes, combat swings and death clips); every other clip
/// loops. [GUESS]: derived from the clip names, the original's per-clip loop flags were not traced (docs/zone/avatar.md §4).
fn one_shot(r: &Role, cast_loop: bool) -> bool {
    !cast_loop && matches!(r, Role::JumpStand | Role::JumpForward | Role::Emote(_) | Role::Clip(_))
}

/// The clip name of the weapon stance of `AnimSet` `set` for movement role `r`: the idle while not fighting is the equip routine's (`FUN_1009c858`:
/// rifle / bazooka list 0x29, others `idle-stand`), the fight idle is list 0x10 (`FUN_1003cad0`), walk / run are the rifle's constants 0x421 / 0x422
/// (`combat::anim::{peace_idle, fight_idle, wield_walk_run}`, docs/zone/combat-anim.md §4).
fn stance_clip(set: i32, r: &Role) -> Option<String> {
    use super::combat::anim::{anim_name, fight_idle, peace_idle, wield_walk_run};
    let id = match r {
        Role::Idle => peace_idle(Some(set)),
        Role::IdleCombat => fight_idle(Some(set))?,
        Role::Walk => wield_walk_run(Some(set))?.0,
        Role::Run => wield_walk_run(Some(set))?.1,
        _ => return None,
    };
    anim_name(id).map(|(n, _)| n.to_string())
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

/// The own-character fade (`FadeCharacter*` Char prefs). Consumer: `VisualCATMesh_t::RefreshAlpha` (DisplaySystem 0x10073b7f, run every frame by
/// `VisualCATMesh_t::RunFunction` 0x10074bb4); the prefs reach it through the changed callbacks `FUN_10072c3c/4d/5b/69` (registered with fire-now
/// in the constructor 0x100745e8) into `DAT_1015082c`, `DAT_100af880` (start), `DAT_100af884` (end) and `DAT_100af888` = `1 - endAlpha`.
/// Only the own character's mesh has the fade flag (`n3VisualDynel_t::SetCatMesh` N3 0x10019fb2 sets `VisualCATMesh_t+0x98` from
/// `n3Dynel_t::IsClientChar`, the constructor 0x10019365 stores it at `+0xc8`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fade {
    pub on: bool,
    pub start: f32,
    pub end: f32,
    pub end_alpha: f32,
}

impl Default for Fade {
    /// The registered defaults (`SetDefaultCharPrefs` GUI 0x1012447a): on, 1.5 m, 0.7 m, 0.15.
    fn default() -> Self {
        Self { on: true, start: 1.5, end: 0.7, end_alpha: 0.15 }
    }
}

/// The threshold `RefreshAlpha` compares the new factor against before it calls `SetAlpha` (`_DAT_10090a50`, the double 0.01).
const FADE_EPSILON: f32 = 0.01;

impl Fade {
    /// The current pref values.
    pub fn from_prefs(p: &super::dvalue::IndepPrefs) -> Self {
        use super::dvalue::Kind::Char;
        let d = Self::default();
        Self {
            on: p.get_int("FadeCharacter", Char).map_or(d.on, |v| v != 0),
            start: p.get_float("FadeCharacterStartDist", Char).unwrap_or(d.start),
            end: p.get_float("FadeCharacterEndDist", Char).unwrap_or(d.end),
            end_alpha: p.get_float("FadeCharacterEndAlpha", Char).unwrap_or(d.end_alpha),
        }
    }

    /// The opacity factor of `RefreshAlpha` for a squared distance between the head attractor and the camera: 1 beyond `start`; inside it
    /// a linear ramp in the distance down to `1 - endAlpha` at `min(end, start)`, which is kept closer than that. (At `endAlpha` 0.15 the
    /// character is never more than 15 % transparent; the option is called "Max transparency level".)
    pub fn factor(&self, dist_sq: f32) -> f32 {
        if !self.on || dist_sq >= self.start * self.start {
            return 1.0;
        }
        let k = 1.0 - self.end_alpha;
        let end = self.end.min(self.start);
        if dist_sq > end * end {
            (1.0 - k) * ((dist_sq.sqrt() - end) / (self.start - end)) + k
        } else {
            k
        }
    }
}

/// The cached factor of `VisualCATMesh_t+0x94` (starts at 1): `RefreshAlpha(false)` takes a new value only when it moved by more than 0.01.
#[derive(Clone, Copy, Debug)]
pub struct Fader(f32);

impl Default for Fader {
    fn default() -> Self {
        Self(1.0)
    }
}

impl Fader {
    /// One `RunFunction`: returns the opacity to draw with.
    pub fn step(&mut self, fade: &Fade, dist_sq: f32) -> f32 {
        let f = fade.factor(dist_sq);
        if (f - self.0).abs() > FADE_EPSILON {
            self.0 = f;
        }
        self.0
    }
}

pub struct Avatar {
    id: u32,
    rig: ActorRig,
    /// What `rig` was built from (full update plus [`Avatar::set_appearance`] deltas).
    look: PlayerLook,
    attachments: Vec<(u8, u32)>,
    assets: ActorAssets,
    scale: f32,
    calibration: Calibration,
    pose: AvatarPose,
    clip: Option<Arc<CatAnim>>,
    /// rdb 1010003 id of `clip`.
    clip_id: u32,
    /// AbstractAnimID passed to calibration (GC 0x1006fb56), not the RDB clip id.
    calibration_id: u32,
    /// Playback rate of the current clip ([`anim_rate`]).
    rate: f32,
    /// `ItemDelay` of the weapon of the swing clip that plays ([`Avatar::set_swing_delay`]).
    swing_delay: Option<i32>,
    /// The clip that plays is a weapon swing: its animation notes fire ([`Avatar::take_notes`]).
    swinging: bool,
    /// Playback rate factor of a hit-reaction clip ([`Avatar::set_clip_scale`]).
    clip_scale: Option<f32>,
    cast_loop: bool,
    /// Bit `i` = event `i` of the clip has fired its note (cleared when a clip starts).
    note_fired: u32,
    notes: Vec<u32>,
    /// `AnimSet` of the wielded weapon ([`Avatar::set_stance`]).
    stance: Option<i32>,
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
        let mut a = Self { id, rig, look: l.look, attachments: l.attachments, assets, scale: l.scale, calibration: Calibration::load(client_dir), pose: AvatarPose::default(), clip: None, clip_id: 0, calibration_id: 0, rate: 1.0, swing_delay: None, swinging: false, clip_scale: None, cast_loop: false, note_fired: 0, notes: Vec::new(), stance: None, ms: 0.0, transform: Mat4::IDENTITY };
        a.set_pose(store, AvatarPose::default())?;
        a.set_transform([u.pos[0], u.pos[1], -u.pos[2]], u.yaw().map_or(0.0, |y| -y));
        Ok(a)
    }

    /// The model for `Host::actor_models` (key [`MODEL_KEY`]).
    pub fn model(&self) -> &Scene {
        self.rig.model()
    }

    /// Apply cloth deltas and replace attractors (`AppearanceUpdateIIR_c::Activate`, GC 0x10071679).
    /// Returns whether the caller must upload the rebuilt model; keeps the current animation.
    pub fn set_appearance(&mut self, store: &RecordStore, appearance: &ao_net::n3::world::AppearanceUpdate) -> Result<bool> {
        let head = appearance.attractors.iter().find(|a| a.a == 0 && a.b > 0).map(|a| a.b as u32);
        let mut attachments: Vec<(u8, u32)> = appearance.attractors.iter().filter(|a| a.a != 0 && a.b > 0).map(|a| (a.a, a.b as u32)).collect();
        attachments.sort_unstable();
        let mut old = self.attachments.clone();
        old.sort_unstable();
        let mut equipment = self.look.equipment;
        for c in appearance.cloth.iter().filter(|c| c.c == 0) {
            if let Some(slot) = usize::try_from(c.id).ok().and_then(|i| equipment.0.get_mut(i)) {
                *slot = (c.b > 0).then_some(c.b as u32);
            }
        }
        if head == self.look.head && attachments == old && equipment.0 == self.look.equipment.0 {
            return Ok(false);
        }
        let look = PlayerLook { head, equipment, ..self.look.clone() };
        self.rig = ActorRig::player(store, &self.assets, &look, &attachments)?;
        self.look = look;
        self.attachments = attachments;
        Ok(true)
    }

    /// A different one-shot list can resolve to the same role; restart its clock and notes explicitly.
    pub fn restart_clip(&mut self) {
        self.ms = 0.0;
        self.note_fired = 0;
    }

    /// Switches the clip when the role changes; a change between locomotion clips keeps the gait phase, any other restarts.
    pub fn set_pose(&mut self, store: &RecordStore, pose: AvatarPose) -> Result<()> {
        if pose.role != self.pose.role || self.clip.is_none() {
            let mut role = pose.role.clone();
            let clips = self.assets.clips(store, self.rig.model_id)?;
            // a wielder: the weapon's stance clip over the movement clip while the model's set has it, else the plain role
            let mut stance = self.stance.and_then(|set| stance_clip(set, &pose.role)).filter(|n| clips.iter().any(|c| c.0 == *n));
            let (clip, id, calibration_id) = loop {
                let name = stance.take().unwrap_or_else(|| role.clip_name());
                if let Some(&(_, id)) = clips.iter().find(|c| c.0 == name) {
                    let abstract_id = super::combat::anim::ANIMS.iter().find(|a| a.1 == name).map_or(0, |a| u32::from(a.0));
                    break (Some(self.assets.anim(store, id)?), id, abstract_id);
                }
                match fallback(&role) {
                    Some(r) => role = r,
                    None => break (None, 0, 0),
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
            self.calibration_id = calibration_id;
            self.note_fired = 0;
        }
        self.rate = anim_rate(self.calibration.get(self.rig.model_id, self.calibration_id), self.scale * 100.0, pose.speed, pose.ref_speed, false);
        // `FUN_1006a239`: a weapon swing is sped up so its first note lands within the weapon's ItemDelay
        if let (Some(d), Some(a)) = (self.swing_delay, &self.clip) {
            self.rate *= super::combat::anim::swing_speed_scale(a.events.first().map_or(0.0, |e| e.0 as f32), d);
        }
        if let Some(k) = self.clip_scale {
            self.rate *= k;
        }
        self.pose = pose;
        Ok(())
    }

    pub fn set_cast_loop(&mut self, looping: bool) {
        self.cast_loop = looping;
    }
    /// Cast Play calls SetTime(0, total) even when its authored clip is unchanged (GC 100108be).
    pub fn restart_cast_clip(&mut self) {
        self.ms = 0.0;
        self.note_fired = 0;
    }


    fn one_shot(&self) -> bool {
        one_shot(&self.pose.role, self.cast_loop)
    }

    /// Rate factor of the one-shot clip that plays (a hit reaction, `Dynels::react_to_hit`); set every frame before [`Avatar::set_pose`].
    pub fn set_clip_scale(&mut self, scale: Option<f32>) {
        self.clip_scale = scale;
    }

    /// `ItemDelay` (centiseconds) of the weapon whose swing clip is playing (`None`: no swing speed scale); set before [`Avatar::set_pose`].
    pub fn set_swing_delay(&mut self, delay: Option<i32>) {
        self.swing_delay = delay;
    }

    /// Whether the one-shot clip that plays is a weapon swing (set every frame before [`Avatar::set_pose`]).
    pub fn set_swinging(&mut self, swinging: bool) {
        self.swinging = swinging;
    }

    /// The notes (`combat::notes`) the swing clip reached since the last call.
    pub fn take_notes(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.notes)
    }

    /// The weapon stance (`AnimSet` of the weapon in the first hand slot, `None` = nothing wielded): idle / walk / run play the weapon's lists
    /// 0x10 / 0x2a / 0x2b ([`stance_clip`]) instead of the unarmed clips, the way [`Dynels`](super::dynels::Dynels) draws a wielding character.
    pub fn set_stance(&mut self, set: Option<i32>) {
        if set != self.stance {
            self.stance = set;
            self.clip = None; // the next `set_pose` picks the clip again
        }
    }

    /// Whether a one-shot clip (jump) has played to its end.
    pub fn finished(&self) -> bool {
        self.one_shot() && self.clip.as_ref().is_none_or(|a| self.ms >= a.duration)
    }

    /// Scene position (feet) and rotation about +Y ([`super::zone::scene_yaw`] of the server heading).
    pub fn set_transform(&mut self, scene_pos: [f32; 3], scene_yaw: f32) {
        self.transform = Mat4::from_translation(Vec3::from(scene_pos)) * Mat4::from_rotation_y(scene_yaw) * Mat4::from_scale(Vec3::splat(self.scale));
    }

    /// Advances the clip by `dt` seconds at the pose's speed.
    pub fn update(&mut self, dt: f32) {
        self.ms += dt * 1000.0 * self.rate;
        if let (true, Some(a), Role::Clip(_)) = (self.swinging, &self.clip, &self.pose.role) {
            self.notes.extend(super::combat::notes::fire(&a.events, self.ms.min(a.duration), &mut self.note_fired));
        }
        // keep the counter bounded; looping clips wrap by themselves, one-shots stop at the end
        if let Some(a) = &self.clip {
            if !self.one_shot() && a.duration > 0.0 && self.ms > 4.0 * a.duration {
                self.ms = clip_time(a, self.ms, false);
            }
        }
    }

    /// The actor to draw this frame (CPU-skinned pose, never frustum culled).
    pub fn frame(&self) -> ActorFrame {
        let clip = self.clip.as_ref().map(|a| (&**a, clip_time(a, self.ms, self.one_shot())));
        let (skin, parts) = self.rig.pose(clip);
        ActorFrame { id: self.id, model: MODEL_KEY, transform: self.transform.to_cols_array_2d(), parts, skin: Some(skin), always: true, alpha: 1.0 }
    }

    /// Current effect anchor in world scene space, including heading and body scale.
    pub fn effect_anchor(&self, id: i32) -> Option<[[f32; 4]; 4]> {
        let clip = self.clip.as_ref().map(|a| (&**a, clip_time(a, self.ms, self.one_shot())));
        self.rig.effect_anchor(id, clip).map(|m| (self.transform * Mat4::from_cols_array_2d(&m)).to_cols_array_2d())
    }

    pub fn weapon_effect_anchor(&self, place: u8) -> Option<[[f32; 4]; 4]> {
        let clip = self.clip.as_ref().map(|a| (&**a, clip_time(a, self.ms, self.one_shot())));
        self.rig.weapon_effect_anchor(place, clip).map(|m| (self.transform * Mat4::from_cols_array_2d(&m)).to_cols_array_2d())
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
        let clip = self.clip.as_ref().map(|a| (&**a, clip_time(a, self.ms, self.one_shot())));
        self.rig.head_attractor(clip).map(|p| p[1] * self.scale)
    }

    /// Scene position of the head attractor (`Attractor01_head`) in the current pose, the point `RefreshAlpha` measures the camera distance
    /// from (attractor translation x body scale, through the CAT frame's world matrix); the feet for a model without one (the identity
    /// attractor matrix `RefreshAlpha` starts from).
    pub fn head_position(&self) -> Vec3 {
        let clip = self.clip.as_ref().map(|a| (&**a, clip_time(a, self.ms, self.one_shot())));
        self.transform.transform_point3(self.rig.head_attractor(clip).map_or(Vec3::ZERO, Vec3::from))
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
    fn fullupdate_keeps_head_mesh_and_honors_attractor_skip() {
        let mut u = own_update();
        let head = u.head_mesh.filter(|&h| h > 0).unwrap() as u32;
        u.attractors.clear();
        assert_eq!(AvatarLook::from_update(&u, |_| Skin::Caucasian).unwrap().look.head, Some(head));
        u.attractors.push(ao_net::n3::dynel::AttractorMesh { place: 0, mesh: 7, field: 0, byte: 4 });
        u.attractors.push(ao_net::n3::dynel::AttractorMesh { place: 1, mesh: 7796, field: 0, byte: 2 });
        assert_eq!(AvatarLook::from_update(&u, |_| Skin::Caucasian).unwrap().look.head, Some(7));
        u.flags |= ao_net::n3::dynel::flag::SET_DYNEL_800;
        let look = AvatarLook::from_update(&u, |_| Skin::Caucasian).unwrap();
        assert_eq!(look.look.head, Some(head));
        assert!(look.attachments.is_empty());
    }
    #[test]
    fn captured_appearance_cloth_reaches_live_avatar_and_rebuild_snapshot() {
        let Some(dir) = client() else { return };
        let store = RecordStore::open(&dir).unwrap();
        let mut u = own_update();
        u.cloth.push(ao_net::n3::dynel::ClothData { raw: 1, texture: 154207, page: 0, extra: None });
        u.flags |= ao_net::n3::dynel::flag::SET_DYNEL_800;
        let mut avatar = Avatar::new(&store, &dir, 7, &u).unwrap();
        let mut zone = super::super::zone::Zone::new(0x82e8);
        zone.own_update = Some(Box::new(u));
        let rec = include_str!("../../../../docs/captures/zone_wear_rifle_borealis.rec");
        let frame = rec.lines().filter_map(|l| {
            let mut p = l.split(' ');
            let (_, direction, hex) = (p.next()?, p.next()?, p.next()?);
            if direction != "<" { return None; }
            let bytes: Vec<u8> = (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap()).collect();
            Frame::decode_with(&bytes, false).ok().flatten().map(|(f, _)| f)
        }).find(|f| matches!(n3::decode(f).map(|m| m.body), Ok(N3::World(n3::world::World::Appearance(_))))).unwrap();
        zone.on_frame(&frame);
        let appearance = zone.own_events.iter().find_map(|e| match e {
            super::super::zone::OwnEvent::Appearance(a) => Some(a),
            _ => None,
        }).unwrap();
        assert!(appearance.cloth.iter().any(|c| c.id == 1 && c.c == 0 && c.b == 0), "capture clears body cloth");
        let clip = (avatar.clip_id, avatar.ms);
        assert!(avatar.set_appearance(&store, appearance).unwrap());
        assert_eq!(avatar.look.equipment.0[1], None);
        assert_eq!((avatar.clip_id, avatar.ms), clip);
        assert!(!avatar.set_appearance(&store, appearance).unwrap());
        let rebuilt = AvatarLook::from_update(zone.own_update.as_ref().unwrap(), |_| Skin::Caucasian).unwrap();
        assert_eq!(rebuilt.look.equipment.0, avatar.look.equipment.0);
        let mut delta = appearance.clone();
        delta.cloth = vec![n3::world::ClothData { id: 1, packed: 1, b: 154207, c: 0, ..Default::default() }];
        assert!(avatar.set_appearance(&store, &delta).unwrap());
        assert_eq!(avatar.look.equipment.0[1], Some(154207));
        delta.cloth.clear();
        assert!(!avatar.set_appearance(&store, &delta).unwrap(), "unnamed cloth remains equipped");
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
        let casting = Role::Clip("spell-sus".into());
        assert!(!one_shot(&casting, true), "cast-start loop must not clamp at the last CAT frame");
        assert!(one_shot(&casting, false), "release restores one-shot playback");
        let a = CatAnim { source_id: 0, root: String::new(), events: vec![(200, "loopstart".into())], version: 0, duration: 1000.0, signature: 0, param: 0.0, tracks: vec![] };
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

    /// `RefreshAlpha` (DisplaySystem 0x10073b7f): 1 beyond the start distance, linear ramp to `1 - endAlpha` at the end distance, constant inside;
    /// the end distance is clamped to the start; off = 1; the cached factor only follows moves above 0.01.
    #[test]
    fn fade_factor_curve_matches_refresh_alpha() {
        let f = Fade::default();
        let at = |d: f32| f.factor(d * d);
        assert_eq!(at(5.0), 1.0);
        assert_eq!(at(1.5), 1.0, "d^2 < start^2 is strict");
        assert!((at(1.1) - (0.15 * (1.1 - 0.7) / 0.8 + 0.85)).abs() < 1e-6);
        assert!((at(1.49) - 1.0).abs() < 0.01);
        assert!((at(0.7) - 0.85).abs() < 1e-6 && (at(0.3) - 0.85).abs() < 1e-6 && at(0.0) == 0.85);
        assert_eq!(Fade { on: false, ..f }.factor(0.0), 1.0);
        // end beyond start behaves as end == start: a step to 1 - endAlpha at the start distance
        let s = Fade { end: 3.0, ..f };
        assert_eq!((s.factor(1.4 * 1.4), s.factor(1.6 * 1.6)), (0.85, 1.0));
        let full = Fade { end_alpha: 1.0, ..f };
        assert_eq!(full.factor(0.0), 0.0, "100 % max transparency");
        // hysteresis
        let mut r = Fader::default();
        assert_eq!(r.step(&f, 0.0), 0.85);
        assert_eq!(r.step(&f, 0.75 * 0.75), 0.85, "0.85 -> 0.8625 is below 0.01");
        assert!((r.step(&f, 0.9 * 0.9) - (0.15 * 0.2 / 0.8 + 0.85)).abs() < 1e-6);
    }

    /// The prefs reach the curve: defaults of `SetDefaultCharPrefs`, a changed Char pref is read back.
    #[test]
    fn fade_reads_the_char_prefs() {
        use super::super::dvalue::{IndepPrefs, Kind};
        let mut p = IndepPrefs::with_defaults();
        assert_eq!(Fade::from_prefs(&p), Fade::default());
        p.set_int("FadeCharacter", 0, Kind::Char);
        p.set_float("FadeCharacterStartDist", 4.0, Kind::Char);
        p.set_float("FadeCharacterEndDist", 2.0, Kind::Char);
        p.set_float("FadeCharacterEndAlpha", 0.5, Kind::Char);
        assert_eq!(Fade::from_prefs(&p), Fade { on: false, start: 4.0, end: 2.0, end_alpha: 0.5 });
    }

    #[test]
    fn retail_authored_gait_durations_use_milliseconds_and_abstract_calibration_keys() {
        let Some(dir) = client() else { return };
        let store = RecordStore::open(&dir).unwrap();
        let mut assets = ActorAssets::new(&store).unwrap();
        let calibration = Calibration::load(&dir);
        for (id, abstract_id, duration, start, end, speed, reference, factor) in [
            (10191, 0x64, 2433.0, 733.0, 1733.0, 1.5, 1.5, 0.90),
            (10194, 0x65, 4000.0, 1166.0, 1933.0, 5.0, 5.0, 1.15),
        ] {
            let clip = assets.anim(&store, id).unwrap();
            assert_eq!(clip.duration, duration);
            assert_eq!(loop_span(&clip), Some((start, end)));
            assert_eq!(calibration.get(5907, id), factor);
            assert_eq!(calibration.get(5907, abstract_id), 1.0, "retail loader does not remap RDB keys");
            let rate = anim_rate(calibration.get(5907, abstract_id), 100.0, speed, reference, false);
            let seconds_per_cycle = (end - start) / (1000.0 * rate);
            assert!((clip_time(&clip, start + seconds_per_cycle * 1000.0 * rate, false) - start).abs() < 0.001);
            eprintln!("CAT {id}: authored {duration}ms, loop {start}..{end}, rate {rate}, cycle {seconds_per_cycle:.6}s, stride {:.6}m", speed * seconds_per_cycle);
        }
    }

    /// The head attractor world position is the feet + head height straight up (idle), in the actor's frame.
    #[test]
    fn head_position_agrees_with_head_height() {
        let Some(dir) = client() else { return };
        let store = RecordStore::open(&dir).unwrap();
        let mut a = Avatar::new(&store, &dir, 7, &own_update()).unwrap();
        a.set_transform([10.0, 20.0, 30.0], 0.0);
        let p = a.head_position();
        assert!((p.y - 20.0 - a.head_height().unwrap()).abs() < 1e-4, "{p:?}");
        assert!((p.x - 10.0).hypot(p.z - 30.0) < 1.0, "{p:?}");
    }

    /// Screenshots of the own avatar at several camera distances with the fade applied: `AVATAR_FADE_SHOT=<dir>` writes
    /// `fade_<prefs>_<d>.png` (default prefs and a 100 % `FadeCharacterEndAlpha`; `d` = camera distance from the head attractor).
    #[test]
    fn fade_screenshots() {
        let (Some(dir), Some(out)) = (client(), std::env::var_os("AVATAR_FADE_SHOT")) else { return };
        let store = RecordStore::open(&dir).unwrap();
        let u = own_update();
        let mut a = Avatar::new(&store, &dir, 7, &u).unwrap();
        let pos = super::super::zone::scene_pos(u.pos);
        let yaw = super::super::zone::scene_yaw(u.yaw().unwrap_or(0.0));
        a.set_transform(pos, yaw);
        let scene = ao_formats::playfield::load_playfield(&store, &dir, 4604).unwrap();
        let head = a.head_position();
        let f = super::super::zone::scene_forward(u.yaw().unwrap_or(0.0));
        for (name, fade) in [("off", Fade { on: false, ..Fade::default() }), ("default", Fade::default()), ("full", Fade { end_alpha: 1.0, ..Fade::default() })] {
            for d in [2.0f32, 1.2, 0.9, 0.5] {
                let eye = [head.x + f[0] * d, head.y + 0.1, head.z + f[2] * d];
                let dist_sq = (Vec3::from(eye) - head).length_squared();
                let mut frame = a.frame();
                frame.alpha = Fader::default().step(&fade, dist_sq);
                eprintln!("{name} d={d} alpha={:.3}", frame.alpha);
                let png = std::path::Path::new(&out).join(format!("fade_{name}_{d}.png"));
                ao_render::render_to_png_actors(&scene, &[(MODEL_KEY, a.model().clone())], vec![frame], eye, [head.x, head.y - 0.2, head.z], 500, 400, &png, 0.0).unwrap();
            }
        }
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

    /// Stance clips of the AnimSets (docs/zone/combat-anim.md §4): the idle out of a fight is the equip routine's (rifle `idle-2h`, bazooka list 0x29,
    /// else `idle-stand`), the fight idle list 0x10, walk / run only the rifle's 0x421 / 0x422; other roles and sets without lists: none.
    #[test]
    fn stance_clip_names() {
        use super::super::combat::anim::anim_name;
        assert_eq!(stance_clip(3, &Role::Idle).as_deref(), Some("idle-2h"));
        assert_eq!(stance_clip(3, &Role::IdleCombat).as_deref(), Some("idle-rifle"));
        assert_eq!(stance_clip(3, &Role::Walk), anim_name(0x421).map(|n| n.0.to_string()));
        assert_eq!(stance_clip(3, &Role::Run), anim_name(0x422).map(|n| n.0.to_string()));
        assert_eq!(stance_clip(3, &Role::Sneak), None);
        assert_eq!(stance_clip(1, &Role::Idle).as_deref(), Some("idle-stand"), "a blade keeps the plain idle out of a fight");
        assert_eq!(stance_clip(1, &Role::IdleCombat).as_deref(), Some("idle-blade"));
        assert_eq!((stance_clip(1, &Role::Walk), stance_clip(8, &Role::Run)), (None, None), "only the rifle overrides walk / run");
        assert_eq!(stance_clip(4, &Role::IdleCombat), None, "AnimSet 4: the lists live in the item record");
    }
    #[test]
    fn nano_cast_loop_restarts_and_release_finishes() {
        let Some(dir) = client() else { return };
        let store = RecordStore::open(&dir).unwrap();
        let mut avatar = Avatar::new(&store, &dir, 7, &own_update()).unwrap();
        let start = AvatarPose::still(Role::Clip("spell-sus".into()));
        avatar.set_cast_loop(true);
        avatar.set_pose(&store, start.clone()).unwrap();
        assert!(avatar.clip.is_some(), "retail cast-start clip must resolve");
        avatar.update(30.0);
        assert!(!avatar.finished(), "cast-start remains a loop");
        assert!(avatar.effect_anchor(2000).is_some(), "cast connector samples the loop pose");
        avatar.restart_cast_clip();
        avatar.set_pose(&store, start).unwrap();
        assert_eq!(avatar.ms, 0.0, "a new Play restarts the same cast clip");
        avatar.set_cast_loop(false);
        avatar.set_pose(&store, AvatarPose::still(Role::Clip("spell-dir".into()))).unwrap();
        assert!(avatar.clip.is_some(), "retail cast-release clip must resolve");
        avatar.update(30.0);
        assert!(avatar.finished(), "release plays once");
    }


    /// `AppearanceUpdateIIR_c` of a wear (docs/captures/zone_wear_rifle_borealis.rec: attractors `{0, head}` + `{1, 0x3ddf}`) mounts the weapon mesh
    /// in the right hand, the unwear's list (head only) takes it away again; an identical list rebuilds nothing. A wielder's idle / walk / run use the
    /// weapon's stance clips (rifle: `idle-rifle` 0x3fd, 2H walk / run 0x421 / 0x422), and the plain ones return when it is gone.
    #[test]
    fn worn_weapon_mounts_its_mesh_and_switches_the_stance() {
        let Some(dir) = client() else { return };
        let store = RecordStore::open(&dir).unwrap();
        let mut a = Avatar::new(&store, &dir, 7, &own_update()).unwrap();
        let head = a.look.head.unwrap();
        let appearance = |list: &[(u8, u32)]| ao_net::n3::world::AppearanceUpdate { cloth: Vec::new(), attractors: list.iter().map(|&(a, b)| ao_net::n3::world::Attractor { a, b: b as i32, c: 0, d: 0 }).collect(), visual_flags: 31, extra: 0 };
        let plain = (a.model().meshes.len(), a.frame().parts.len());
        assert!(!a.set_appearance(&store, &appearance(&[(0, head)])).unwrap(), "the login list is unchanged");
        assert!(a.set_appearance(&store, &appearance(&[(1, 0x3ddf), (0, head)])).unwrap());
        assert_eq!((a.model().meshes.len(), a.frame().parts.len()), (plain.0 + 1, plain.1 + 1), "the rifle is one more mounted mesh");
        assert!(!a.set_appearance(&store, &appearance(&[(0, head), (1, 0x3ddf)])).unwrap(), "same set, other order");
        let idle = a.clip_id;
        a.set_stance(Some(3));
        a.set_pose(&store, AvatarPose::still(Role::Idle)).unwrap();
        let clips = a.assets.clips(&store, a.rig.model_id).unwrap();
        let named = |n: &str| clips.iter().find(|c| c.0 == n).map(|c| c.1);
        // out of a fight a rifle wielder stands in `idle-2h` (list 0x29, `FUN_1009c858`) when the model's set has it, else plain
        assert_eq!(Some(a.clip_id), named("idle-2h").or(Some(idle)));
        // fighting with a weapon plays the weapon idle (list 0x10), not the raised fists
        a.set_pose(&store, AvatarPose::still(Role::IdleCombat)).unwrap();
        assert_eq!(Some(a.clip_id), named("idle-rifle"));
        assert_ne!(a.clip_id, idle);
        // the fight start's draw (list 0x1a), the fight stop's holster (list 0x1b) and the unwear's gesture (0x6d) are clips of the model's set that play once
        for n in ["rifle-start", "rifle-stop", "wield"] {
            a.set_pose(&store, AvatarPose::still(Role::Clip(n.into()))).unwrap();
            assert_eq!(Some(a.clip_id), named(n), "{n}");
            assert!(!a.finished(), "{n}");
            a.update(30.0);
            assert!(a.finished(), "{n} plays once");
        }
        a.set_pose(&store, AvatarPose::still(Role::IdleCombat)).unwrap();
        // the rifle's walk constant 0x421 when the model's set has it, else the plain walk
        let walk = stance_clip(3, &Role::Walk).and_then(|n| named(&n));
        a.set_pose(&store, AvatarPose { role: Role::Walk, speed: 1.5, ref_speed: 1.5 }).unwrap();
        assert_eq!(Some(a.clip_id), walk.or(named("walk")));
        a.set_stance(None);
        a.set_pose(&store, AvatarPose::still(Role::Idle)).unwrap();
        assert_eq!(a.clip_id, idle);
        assert!(a.set_appearance(&store, &appearance(&[(0, head)])).unwrap());
        assert_eq!((a.model().meshes.len(), a.frame().parts.len()), plain);
        if let Some(png) = std::env::var_os("AVATAR_SHOT_RIFLE") {
            a.set_appearance(&store, &appearance(&[(1, 0x3ddf), (0, head)])).unwrap();
            a.set_stance(Some(3));
            a.set_pose(&store, AvatarPose::still(Role::Idle)).unwrap();
            a.update(0.3);
            let u = own_update();
            let (pos, yaw) = (super::super::zone::scene_pos(u.pos), super::super::zone::scene_yaw(u.yaw().unwrap_or(0.0)));
            a.set_transform(pos, yaw);
            let scene = ao_formats::playfield::load_playfield(&store, &dir, 4604).unwrap();
            let f = super::super::zone::scene_forward(yaw);
            let at = [pos[0], pos[1] + 1.0, pos[2]];
            let eye = [at[0] + f[0] * 2.5, at[1] + 0.3, at[2] + f[2] * 2.5];
            ao_render::render_to_png_actors(&scene, &[(MODEL_KEY, a.model().clone())], vec![a.frame()], eye, at, 900, 600, std::path::Path::new(&png), 0.0).unwrap();
        }
    }
}
