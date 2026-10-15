//! The player's own character in the world: appearance from `SimpleCharFullUpdate`, animation clip choice and the
//! [`ao_scene::ActorFrame`] the renderer draws (always drawn, re-skinned every frame). Evidence: docs/zone/avatar.md.

use ao_formats::character::{
    actor::{ActorAssets, ActorRig, PlayerLook},
    animation_fade_ms, head_table, CachedCharacter, CatAnim, ClothEntry, Role, Skin,
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

/// GC 1006d330 subtracts the double at 101606a8 from the enter interval duration.
pub(super) const POSTURE_IDLE_LEAD_MS: f32 = 500.0;

/// GC 100108be mode 1 plays the intro interval, falling back to the entire clip.
pub(super) fn posture_interval_end(clip: &CatAnim) -> f32 {
    clip.events.iter().find(|(ms, name)| name == "loopstart" && *ms > 0 && (*ms as f32) <= clip.duration)
        .map_or(clip.duration, |(ms, _)| *ms as f32)
}

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

/// `Gamecode.dll 0x1004b521` (decompiled): for each of the 5 body parts (0 hands .. 4 legs) it scans pages 2, 1, 0 in that order through a dense
/// `page*5+part` array and takes the first (highest-page) nonzero slot, independently per part; a part with no nonzero page across any page falls
/// back to bare skin (`ClothData_t::GetName`, caller `FUN_1004b5ab`). Pages are independent layers of the *same* part, not a filter that drops
/// entries belonging to a different part: two cloth entries on different parts (e.g. part 1 page 0, part 2 page 1) are both visible, each at its
/// own part slot; the page only matters when two entries share one part (the higher page then wins).
pub(super) fn visible_cloth(cloth: &[(i32, i32)]) -> impl Iterator<Item = (i32, i32)> + '_ {
    (0..5).filter_map(move |part| {
        (0..3).rev().find_map(|page| cloth.iter().find(|&&(index, texture)| index == page * 5 + part && texture > 0).map(|&(_, texture)| (part, texture)))
    })
}

impl AvatarLook {
    /// `skin_of_head` gives the skin race of a head mesh (creation head table), as the select-screen preview does.
    pub fn from_update(u: &SimpleCharFullUpdate, skin_of_head: impl FnOnce(u32) -> Skin) -> Result<Self> {
        let (breed, gender) = wire_breed_sex(u.breed as i32, u.sex as i32)?;
        // Fullupdate's base ClearAttractors only clears bookkeeping (DS 0x10071dd0), not the HeadMesh child.
        let skip_attractors = u.flags & ao_net::n3::dynel::flag::SET_DYNEL_800 != 0;
        let head = (!skip_attractors).then(|| u.attractors.iter().find(|a| a.place == 0).map(|a| a.mesh)).flatten().or(u.head_mesh).filter(|&h| h > 0).map(|h| h as u32);
        // GC 0x1004b521 resolves the three cloth pages before uploading textures.
        let pages: Vec<_> = u.cloth.iter().map(|c| (c.page * 5 + c.part(), c.texture)).collect();
        let cloth = CachedCharacter {
            cloth: visible_cloth(&pages).map(|(body_part, texture)| ClothEntry { body_part, texture, ..Default::default() }).collect(),
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

/// Movement playback uses interval mode 2 (GC 0x1006be27 -> 0x1003c392 -> 0x100108be):
/// start at `loopstart`, loop to `loopend`; no separate run-start clip or inherited gait phase.
fn movement_loop(r: &Role) -> bool {
    matches!(r, Role::Idle | Role::SitGround | Role::Walk | Role::WalkBack | Role::WalkLeft | Role::WalkRight | Role::Run | Role::RunBack | Role::Swim | Role::IdleSwim)
        || matches!(r, Role::Clip(name) if name == "turn-left" || name == "turn-right")
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
pub(super) fn loop_span(a: &CatAnim) -> Option<(f32, f32)> {
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

/// DS 0x10073f23's infinite-count predecessor path extends a short remaining interval by
/// ceil(200 / interval) whole cycles before clamping. Positive-count fades use the shared helper.
pub(super) fn body_fade_ms(old: &CatAnim, ms: f32, old_once: bool, old_movement_loop: bool, new_once: bool) -> f32 {
    let (start, end) = if old_movement_loop { loop_span(old).unwrap_or((0.0, old.duration)) } else { (0.0, old.duration) };
    let duration = end - start;
    if !old_once && !new_once && duration > 0.0 {
        let mut remaining = duration - (ms - start).rem_euclid(duration);
        if remaining < 200.0 {
            remaining += (200.0 / duration).ceil() * duration;
        }
        remaining.clamp(200.0, 300.0)
    } else {
        animation_fade_ms(duration, ms - start)
    }
}

/// Mark the intro's notes consumed too: retail's first/next note interval begins at loopstart.
fn body_note_mask(clip: &CatAnim, role: &Role) -> u32 {
    let start = if movement_loop(role) { loop_span(clip).map_or(0.0, |s| s.0) } else { return 0 };
    clip.events.iter().enumerate().filter(|(_, (ms, _))| (*ms as f32) < start).fold(0, |mask, (i, _)| mask | 1u32 << (i & 31))
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

struct ActionClip {
    clip: Arc<CatAnim>,
    ms: f32,
    duration: f32,
    posture: bool,
    before_body: bool,
    layer: u8,
    priority: i32,
    fade_ms: f32,
    layers: u32,
    note_fired: u32,
    rate: f32,
    key: Option<u16>,
    slot: i32,
    #[cfg(test)]
    role: Option<Role>,
}

impl ActionClip {
    fn take_notes(&mut self) -> impl Iterator<Item = super::combat::notes::FiredNote> + '_ {
        let slot = self.slot;
        super::combat::notes::fire(&self.clip.events, self.ms.min(self.duration), &mut self.note_fired)
            .filter_map(move |id| attributed_note(id, slot))
    }
    fn finish_notes(&mut self) -> impl Iterator<Item = super::combat::notes::FiredNote> + '_ {
        let slot = self.slot;
        super::combat::notes::finish(&self.clip.events, &mut self.note_fired)
            .filter_map(move |id| attributed_note(id, slot))
    }
    fn sample(&self) -> ao_formats::character::AnimLayer<'_> {
        ao_formats::character::AnimLayer {
            clip: &self.clip, ms: self.ms.min(self.duration), layers: self.layers,
            blend: ao_formats::character::animation_blend(self.ms, self.duration, self.fade_ms),
        }
    }
}

fn attributed_note(note: u32, slot: i32) -> Option<super::combat::notes::FiredNote> {
    use super::combat::notes::{id, FiredNote};
    (slot >= 0 || (note != id::ATTACK && !(id::ATTACK_EFFECT_1..=id::ATTACK_EFFECT_4).contains(&note)))
        .then_some(FiredNote { id: note, slot })
}

pub struct Avatar {
    id: u32,
    rig: ActorRig,
    /// Last submitted body pose shared with the renderer, retained for input picking.
    current_skin: std::cell::RefCell<Option<Arc<Vec<ao_scene::Vertex>>>>,
    current_bounds: std::cell::Cell<Option<([f32; 3], [f32; 3])>>,
    current_parts: std::cell::RefCell<Vec<[[f32; 4]; 4]>>,
    sample_pose: std::cell::Cell<bool>,
    cadence: ao_formats::character::AnimationCadence,
    pose_cache: std::cell::RefCell<ao_formats::character::actor::PoseCache>,
    anim_set: &'static str,
    /// What `rig` was built from (full update plus [`Avatar::set_appearance`] deltas).
    look: PlayerLook,
    attachments: Vec<(u8, u32)>,
    cloth_pages: Vec<(i32, i32)>,
    monster_data: i32,
    monster: Option<ao_formats::character::NpcRecord>,
    monster_look: super::dynels::CharLook,
    // ponytail: own variants use a private CRT stream; share the scene stream if
    // multi-variant own morph/effect ordering needs exact retail interleaving.
    monster_rand: ao_formats::character::CrtRand,
    assets: ActorAssets,
    scale: f32,
    calibration: Calibration,
    pose: AvatarPose,
    clip: Option<Arc<CatAnim>>,
    actions: Vec<ActionClip>,
    body_priority: i32,
    body_layers: u32,
    /// rdb 1010003 id of `clip`.
    clip_id: u32,
    /// AbstractAnimID passed to calibration (GC 0x1006fb56), not the RDB clip id.
    calibration_id: u32,
    /// Playback rate of the current clip ([`anim_rate`]).
    rate: f32,
    /// Playback rate factor of a hit-reaction clip ([`Avatar::set_clip_scale`]).
    clip_scale: Option<f32>,
    cast_loop: bool,
    /// Playback semantics installed on the current node, separate from the next cast configuration.
    body_once: bool,
    /// Bit `i` = event `i` of the clip has fired its note (cleared when a clip starts).
    note_fired: u32,
    /// Local clip time ([`clip_time`]) as of the last [`Avatar::update`], to detect a looping clip's wrap and re-arm
    /// `note_fired` for its next cycle (the own walk/run/jump clip's own step/land notes, not just the swing overlays).
    body_phase: f32,
    /// `Vehicle_t+0x52` (`FUN_10045069`'s own guard): no step/swim/land note plays while airborne (`Player::set_airborne`).
    airborne: bool,
    notes: Vec<super::combat::notes::FiredNote>,
    /// `AnimSet` of the wielded weapon ([`Avatar::set_stance`]).
    stance: Option<i32>,
    /// Milliseconds into the current clip.
    ms: f32,
    /// SetAnimationDelay freezes only the new idle node, not the enter or preceding idle.
    body_delay_ms: f32,
    /// Body-layer crossfade (`VisualCATMesh_t::SetAnimation` DS 0x10073f23: `CATAnimBlend_t` anim1 = previous clip, anim2 =
    /// new one, blended over `body_fade_ms` on the incoming clip's clock): the role's previous clip, kept alive while
    /// `prev_fade_elapsed < prev_fade_ms`.
    prev_clip: Option<Arc<CatAnim>>,
    prev_ms: f32,
    prev_rate: f32,
    /// The outgoing node keeps its playback count while the new role fades in.
    prev_once: bool,
    prev_fade_elapsed: f32,
    prev_fade_ms: f32,
    transform: Mat4,
}

impl Avatar {
    /// Builds the rig from the own dynel's update. `id` is the dynel instance id (`ActorFrame::id`). `assets` is
    /// reused across zone changes by the caller ([`super::player::Player`]): its `rest`/`clips`/`anims` caches are
    /// a pure function of the client's own data, independent of the avatar's current look, so a fresh `ActorAssets`
    /// only needs building once per session, not once per `Player::new` (measured live: resolving the own model's
    /// rest pose dominated the ~120-140 ms per-zone-change stall, `zone-preload-proposal.md` §2.1).
    pub fn new(store: &RecordStore, client_dir: &Path, id: u32, u: &SimpleCharFullUpdate, assets: ActorAssets) -> Result<Self> {
        let (breed, gender) = wire_breed_sex(u.breed as i32, u.sex as i32)?;
        let heads = head_table(store, breed, gender, 2)?;
        let l = AvatarLook::from_update(u, |h| heads.iter().find(|e| e.mesh == h).map_or(Skin::Caucasian, |e| e.skin))?;
        let rig = ActorRig::player(store, &assets, &l.look, &l.attachments)?;
        let mut a = Self { id, rig, current_skin: Default::default(), current_bounds: Default::default(), current_parts: Default::default(), sample_pose: std::cell::Cell::new(true), cadence: Default::default(), pose_cache: Default::default(), look: l.look, attachments: l.attachments, cloth_pages: u.cloth.iter().map(|c| (c.page * 5 + c.part(), c.texture)).collect(), monster_data: 0, monster: None, monster_look: super::dynels::CharLook::from_update(u), monster_rand: ao_formats::character::CrtRand::new(1), assets, anim_set: super::combat::anim::clip_set(u.breed, u.sex), scale: l.scale, calibration: Calibration::load(client_dir), pose: AvatarPose::default(), clip: None, actions: Vec::new(), body_priority: 0, body_layers: 3, clip_id: 0, calibration_id: 0, rate: 1.0, clip_scale: None, cast_loop: false, body_once: false, note_fired: 0, body_phase: 0.0, airborne: false, notes: Vec::new(), stance: None, ms: 0.0, body_delay_ms: 0.0, prev_clip: None, prev_ms: 0.0, prev_rate: 1.0, prev_once: false, prev_fade_elapsed: 0.0, prev_fade_ms: 0.0, transform: Mat4::IDENTITY };
        a.set_pose(store, AvatarPose::default())?;
        a.set_transform([u.pos[0], u.pos[1], -u.pos[2]], u.yaw().map_or(0.0, |y| -y));
        Ok(a)
    }

    /// Reclaims the warmed `ActorAssets` ([`Avatar::new`]) so the caller can hand it back to the next zone's
    /// avatar build instead of discarding it with the rest of the dropped [`super::player::Player`].
    pub(super) fn into_assets(self) -> ActorAssets {
        self.assets
    }

    /// The model for `Host::actor_models` (key [`MODEL_KEY`]).
    pub fn model(&self) -> &Scene {
        self.rig.model()
    }

    fn invalidate_model(&mut self) {
        *self.current_skin.get_mut() = None;
        self.current_bounds.set(None);
        self.current_parts.get_mut().clear();
        *self.pose_cache.get_mut() = Default::default();
        self.sample_pose.set(true);
    }

    /// GC 10059e6a -> 10052147 -> 10051d2d: MonsterData replaces the actor,
    /// not an attractor. The caller applies the MechData guard before this call.
    pub fn set_monster_data(&mut self, store: &RecordStore, monster_data: i32) -> Result<bool> {
        if self.monster_data == monster_data { return Ok(false); }
        let monster = if monster_data != 0 {
            ao_formats::character::NpcRecord::lookup(store, monster_data as u32)?
        } else { None };
        let rig = if let Some(rec) = &monster {
            super::dynels::npc_rig(store, &self.assets, &self.monster_look, rec, self.look.head, &self.attachments)?
        } else {
            ActorRig::player(store, &self.assets, &self.look, &self.attachments)?
        };
        self.rig = rig;
        self.monster_data = monster_data;
        self.monster = monster;
        self.invalidate_model();
        // Never blend clips belonging to different CAT skeletons.
        self.clip = None;
        self.prev_clip = None;
        self.actions.clear();
        self.notes.clear();
        self.set_pose(store, self.pose.clone())?;
        Ok(true)
    }

    #[cfg(test)]
    pub(in crate::play) fn monster_visual(&self) -> (i32, u32, u32) {
        (self.monster_data, self.rig.model_id, self.clip_id)
    }

    /// Effective native MonsterScale(kind 3) / 100, supplied by the stat owner.
    pub fn set_body_scale(&mut self, scale: f32) {
        if self.scale == scale { return; }
        let ratio = scale / self.scale;
        for axis in [&mut self.transform.x_axis, &mut self.transform.y_axis, &mut self.transform.z_axis] {
            *axis *= ratio;
        }
        self.scale = scale;
    }

    /// Apply cloth deltas and replace attractors (`AppearanceUpdateIIR_c::Activate`, GC 0x10071679).
    /// Returns whether the caller must upload the rebuilt model; keeps the current animation.
    pub fn set_appearance(&mut self, store: &RecordStore, appearance: &ao_net::n3::world::AppearanceUpdate) -> Result<bool> {
        let head = appearance.attractors.iter().find(|a| a.a == 0 && a.b > 0).map(|a| a.b as u32);
        let mut attachments: Vec<(u8, u32)> = appearance.attractors.iter().filter(|a| a.a != 0 && a.b > 0).map(|a| (a.a, a.b as u32)).collect();
        attachments.sort_unstable();
        let mut old = self.attachments.clone();
        old.sort_unstable();
        for c in &appearance.cloth {
            let index = c.c * 5 + c.id;
            self.cloth_pages.retain(|p| p.0 != index);
            if c.b > 0 { self.cloth_pages.push((index, c.b)); }
        }
        let mut equipment = self.look.equipment;
        equipment.0.fill(None);
        for (part, texture) in visible_cloth(&self.cloth_pages) {
            equipment.0[part as usize] = Some(texture as u32);
        }
        if head == self.look.head && attachments == old && equipment.0 == self.look.equipment.0 {
            return Ok(false);
        }
        let look = PlayerLook { head, equipment, ..self.look.clone() };
        self.monster_look.apply_appearance(appearance);
        self.rig = if let Some(rec) = &self.monster {
            super::dynels::npc_rig(store, &self.assets, &self.monster_look, rec, head, &attachments)?
        } else {
            ActorRig::player(store, &self.assets, &look, &attachments)?
        };
        self.invalidate_model();
        self.look = look;
        self.attachments = attachments;
        Ok(true)
    }

    fn action_clip(&mut self, id: u16) -> Option<(u32, super::combat::anim::Layer)> {
        use super::combat::anim::{resolve_clip, Layer, ANIMS};
        if let Some(rec) = &self.monster {
            let variants = ao_formats::character::holder_variants(rec, u32::from(id));
            let clip = ao_formats::character::pick_variant(variants, &mut || self.monster_rand.rand())?;
            let layer = ANIMS.iter().find(|a| a.0 == id).map_or(Layer::Body, |a| a.2);
            Some((clip, layer))
        } else {
            resolve_clip(&self.assets.names, self.anim_set, id, false).map(|(clip, _, layer)| (clip, layer))
        }
    }

    /// Abstract-id action playback; explicit `Layer_e` overrides the table's layer.
    pub fn play_action(&mut self, store: &RecordStore, id: u16, layer: Option<u8>, priority: i32) -> Result<bool> {
        use super::combat::anim::Layer;
        let Some((clip_id, default_layer)) = self.action_clip(id) else { return Ok(false) };
        let clip = self.assets.anim(store, clip_id)?;
        let layer = layer.unwrap_or(match default_layer { Layer::Body => 0, Layer::Upper => 1 });
        self.start_action(clip, layer, priority, 1.0, (None, -1, None));
        Ok(true)
    }

    pub fn play_swing(&mut self, store: &RecordStore, role: Role, delay: Option<i32>, key: u16, slot: i32) -> Result<()> {
        if self.actions.iter().any(|a| a.key == Some(key)) {
            return Ok(());
        }
        let name = role.clip_name();
        let Some((id, _, _)) = super::combat::anim::ANIMS.iter().find(|(_, clip_name, _)| *clip_name == name) else { return Ok(()) };
        let Some((clip_id, _)) = self.action_clip(*id) else { return Ok(()) };
        let clip = self.assets.anim(store, clip_id)?;
        let rate = delay.map_or(1.0, |delay| super::combat::anim::swing_speed_scale(clip.events.first().map_or(0.0, |e| e.0 as f32), delay));
        self.start_action(clip, 0, -1, rate, (Some(key), slot, Some(role)));
        Ok(())
    }

    fn start_action(&mut self, clip: Arc<CatAnim>, layer: u8, priority: i32, rate: f32, playback: (Option<u16>, i32, Option<Role>)) {
        let (key, slot, role) = playback;
        #[cfg(not(test))]
        let _ = role;
        let previous = self.actions.iter().rev().find(|a| a.priority == priority);
        let fade_ms = previous.map_or(200.0, |a| ao_formats::character::animation_fade_ms(a.duration, a.ms));
        let at = self.actions.partition_point(|a| a.priority >= priority);
        self.actions.insert(at, ActionClip { duration: clip.duration, posture: false, before_body: false, clip, ms: 0.0, layer, priority, fade_ms, layers: 3, note_fired: 0, rate, key, slot, #[cfg(test)] role });
        // DS 100702d4 refreshes masks on clip load, not on node expiration.
        self.refresh_masks();
    }

    /// GC 1006d330: enqueue the infinite mode-2 idle first, then a priority-0 mode-1 enter.
    /// DS 100737db/10072eeb freezes the idle clock; the finite enter keeps its own fade/lifetime.
    pub fn play_posture(&mut self, store: &RecordStore, id: u16, idle: AvatarPose) -> Result<()> {
        let Some((clip_id, _)) = self.action_clip(id) else { return Ok(()) };
        let clip = self.assets.anim(store, clip_id)?;
        let duration = posture_interval_end(&clip);
        let idle_fade = self.actions.iter().rev().find(|action| action.priority == self.body_priority && !action.before_body)
            .map(|action| animation_fade_ms(action.duration, action.ms));
        self.set_pose(store, idle)?;
        if let Some(fade) = idle_fade { self.prev_fade_ms = fade; }
        self.body_delay_ms = (duration - POSTURE_IDLE_LEAD_MS).max(0.0);
        let idle_duration = self.clip.as_ref().map_or(0.0, |idle| loop_span(idle).map_or(idle.duration, |(start, end)| end - start));
        let fade_ms = animation_fade_ms(idle_duration, 0.0);
        self.start_action(clip, 0, 0, 1.0, (None, -1, None));
        let action = self.actions.iter_mut().rev().find(|action| action.priority == 0).unwrap();
        action.duration = duration;
        action.fade_ms = fade_ms;
        action.posture = true;
        Ok(())
    }

    #[cfg(test)]
    pub fn posture_active(&self) -> bool {
        self.actions.iter().any(|action| action.posture)
    }
    pub fn set_body_priority(&mut self, priority: i32) {
        if priority != self.body_priority {
            self.body_priority = priority;
            self.body_layers = 3;
            self.refresh_masks();
        }
    }

    fn refresh_masks(&mut self) {
        use ao_formats::character::animation_layer_mask;
        let mut state = (i32::MAX, 0, 0);
        let at = self.actions.partition_point(|a| a.priority > self.body_priority || (a.priority == self.body_priority && a.before_body));
        let (before, after) = self.actions.split_at_mut(at);
        for action in before {
            action.layers = animation_layer_mask(&mut state, action.priority, u32::from(action.layer), action.layers);
        }
        self.body_layers = animation_layer_mask(&mut state, self.body_priority, 0, self.body_layers);
        for action in after {
            action.layers = animation_layer_mask(&mut state, action.priority, u32::from(action.layer), action.layers);
        }
    }



    /// Switches clips at the movement handler's authored loop start (GC 0x1006be27 / 0x100108be).
    pub fn set_pose(&mut self, store: &RecordStore, pose: AvatarPose) -> Result<()> {
        // Movement retains Hover while flying: GC 1006be27 moving mode 7
        // leaves the flight-entry clip unchanged, not a B3/B4 fallback.
        if pose.role != self.pose.role || self.clip.is_none() {
            let (clip, id, calibration_id) = if let Some(rec) = &self.monster {
                let name = pose.role.clip_name();
                let Some(key) = super::combat::anim::ANIMS.iter().find(|a| a.1 == name).map(|a| u32::from(a.0)) else {
                    self.pose = pose;
                    return Ok(());
                };
                let variants = ao_formats::character::holder_variants(rec, key);
                let id = ao_formats::character::pick_variant(variants, &mut || self.monster_rand.rand());
                match id {
                    Some(id) => (Some(self.assets.anim(store, id)?), id, key),
                    None => {
                        self.pose = pose;
                        return Ok(()); // native holder failure leaves the installed clip unchanged
                    }
                }
            } else {
                let mut role = pose.role.clone();
                let clips = self.assets.clips(store, self.rig.model_id)?;
                // A wielder overrides movement only while the model has the stance clip.
                let mut stance = self.stance.and_then(|set| stance_clip(set, &pose.role)).filter(|n| clips.iter().any(|c| c.0 == *n));
                loop {
                    let name = stance.take().unwrap_or_else(|| role.clip_name());
                    if let Some(&(_, id)) = clips.iter().find(|c| c.0 == name) {
                        let abstract_id = super::combat::anim::ANIMS.iter().find(|a| a.1 == name).map_or(0, |a| u32::from(a.0));
                        break (Some(self.assets.anim(store, id)?), id, abstract_id);
                    }
                    match fallback(&role) {
                        Some(r) => role = r,
                        None => break (None, 0, 0),
                    }
                }
            };
            let old_ms = self.ms;
            let old_rate = self.rate;
            // An idle that has not acquired any weight is not the visible predecessor.
            let old_clip = if self.prev_clip.is_some() && self.prev_fade_elapsed == 0.0 { None } else { self.clip.clone() };
            self.body_delay_ms = 0.0;
            for action in &mut self.actions {
                if action.priority == self.body_priority { action.before_body = true; }
            }
            self.ms = clip.as_ref().filter(|_| movement_loop(&pose.role)).and_then(|a| loop_span(a)).map_or(0.0, |s| s.0);
            // body-layer crossfade (DS 0x10073f23): keep the outgoing clip blended underneath while it fades.
            if let Some(old) = old_clip {
                self.prev_once = self.one_shot();
                self.prev_fade_ms = body_fade_ms(&old, old_ms, self.prev_once, movement_loop(&self.pose.role), one_shot(&pose.role, self.cast_loop));
                self.prev_fade_elapsed = 0.0;
                self.prev_ms = old_ms;
                self.prev_rate = old_rate;
                self.prev_clip = Some(old);
            }
            self.clip = clip;
            self.clip_id = id;
            self.calibration_id = calibration_id;
            self.note_fired = self.clip.as_ref().map_or(0, |a| body_note_mask(a, &pose.role));
            self.body_phase = self.ms;
            self.body_layers = 3;
            self.refresh_masks();
        }
        self.rate = anim_rate(self.calibration.get(self.rig.model_id, self.calibration_id), self.scale * 100.0, pose.speed, pose.ref_speed, false);
        if let Some(k) = self.clip_scale {
            self.rate *= k;
        }
        self.body_once = one_shot(&pose.role, self.cast_loop);
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
        self.body_phase = 0.0;
    }


    fn one_shot(&self) -> bool {
        self.body_once
    }

    /// Rate factor of the one-shot clip that plays (a hit reaction, `Dynels::react_to_hit`); set every frame before [`Avatar::set_pose`].
    pub fn set_clip_scale(&mut self, scale: Option<f32>) {
        self.clip_scale = scale;
    }

    /// `Vehicle_t+0x52` (`FUN_10045069`): no step/swim/land note plays while airborne.
    pub fn set_airborne(&mut self, airborne: bool) {
        self.airborne = airborne;
    }


    /// The notes (`combat::notes`) the swing clip reached since the last call.
    pub fn take_notes(&mut self) -> Vec<super::combat::notes::FiredNote> {
        std::mem::take(&mut self.notes)
    }

    #[cfg(test)]
    pub fn swing_role(&self) -> Option<&Role> {
        self.actions.iter().rev().find_map(|a| a.role.as_ref())
    }

    /// Weapon stance of the first wielded hand: peace idle and rifle-only walk/run overrides
    /// (`stance_clip`, GC 0x1009c858); fight idle independently uses list 0x10.
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
        self.sample_pose.set(true);
        let elapsed = dt * 1000.0 * self.rate;
        let delay = self.body_delay_ms.min(elapsed.abs());
        self.body_delay_ms -= delay;
        let body_elapsed = elapsed.signum() * (elapsed.abs() - delay);
        self.ms += body_elapsed;
        if self.prev_clip.is_some() {
            self.prev_ms += dt * 1000.0 * self.prev_rate;
            self.prev_fade_elapsed += body_elapsed.abs();
            if self.prev_fade_elapsed >= self.prev_fade_ms {
                self.prev_clip = None;
            }
        }
        for action in &mut self.actions {
            action.ms += dt * 1000.0 * action.rate;
            self.notes.extend(action.take_notes());
            if action.ms > action.duration && !action.posture {
                self.notes.extend(action.finish_notes());
            }
        }
        self.actions.retain(|a| a.ms <= a.duration);
        // the own walk/run/jump clip's own notes (`FUN_1003c036`'s per-frame holder walk, not just the swing overlays):
        // `fire` needs the looping clip's current cycle position, and a wrap re-arms `note_fired` for the next cycle.
        if let Some(clip) = self.clip.clone() {
            let local = clip_time(&clip, self.ms, self.one_shot());
            if local < self.body_phase {
                self.note_fired = body_note_mask(&clip, &self.pose.role);
            }
            let airborne = self.airborne;
            use super::combat::notes::id as note;
            self.notes.extend(super::combat::notes::fire(&clip.events, local, &mut self.note_fired)
                .filter_map(|id| attributed_note(id, -1))
                .filter(|n| !(airborne && matches!(n.id, note::STEP | note::LAND | note::SWIM))));
            self.body_phase = local;
        }
        // keep the counter bounded; looping clips wrap by themselves, one-shots stop at the end
        if let Some(a) = &self.clip {
            if !self.one_shot() && a.duration > 0.0 && self.ms > 4.0 * a.duration {
                self.ms = clip_time(a, self.ms, false);
            }
        }
    }

    fn layers(&self) -> impl Clone + Iterator<Item = ao_formats::character::AnimLayer<'_>> {
        use ao_formats::character::AnimLayer;
        let prev_body = self.prev_clip.as_ref().map(|a| AnimLayer { clip: a, ms: clip_time(a, self.prev_ms, self.prev_once), layers: self.body_layers, blend: 1.0 });
        let body_blend = if self.prev_clip.is_some() { (self.prev_fade_elapsed / self.prev_fade_ms).clamp(0.0, 1.0) } else { 1.0 };
        let body = self.clip.as_ref().map(|a| AnimLayer { clip: a, ms: clip_time(a, self.ms, self.one_shot()), layers: self.body_layers, blend: body_blend });
        let high = self.actions.partition_point(|a| a.priority > self.body_priority);
        let at = self.actions.partition_point(|a| a.priority > self.body_priority || (a.priority == self.body_priority && a.before_body));
        let (higher, rest) = self.actions.split_at(high);
        let (older, after) = rest.split_at(at - high);
        higher.iter().map(ActionClip::sample).chain(prev_body).chain(older.iter().map(ActionClip::sample))
            .chain(body).chain(after.iter().map(ActionClip::sample))
    }

    /// Own CAT mesh uses the same native camera-distance refresh policy as remote meshes.
    pub fn set_animation_refresh(&mut self, smooth: bool, camera: Vec3, previously_visible: bool) {
        let distance_sq = self.transform.w_axis.truncate().distance_squared(camera);
        self.sample_pose.set(self.cadence.sample(smooth, distance_sq, self.rig.bounding_radius, previously_visible));
    }

    fn refresh_pose(&self) {
        if self.sample_pose.replace(false) || self.current_skin.borrow().is_none() {
            let (skin, parts) = self.rig.pose_composed_cached(self.layers(), &mut self.pose_cache.borrow_mut());
            self.current_bounds.set(super::hud_pick::bounds_of(skin.iter().map(|vertex| &vertex.pos)));
            *self.current_skin.borrow_mut() = Some(Arc::new(skin));
            *self.current_parts.borrow_mut() = parts;
        }
    }

    /// Embedded worn lights use the posed avatar transform, independently of body alpha.
    /// The caller applies CAT visibility: DisplaySystem 0x10073693 detaches hidden frames
    /// before Randy 0x1005798b -> 0x100577d3 can process their mounted lights.
    pub fn lights(&self) -> impl Iterator<Item = ao_scene::Light> + '_ {
        if self.rig.has_mount_lights() {
            self.refresh_pose();
        }
        self.rig.mount_lights(&self.current_parts.borrow()).into_iter().map(|light| ao_scene::Light {
            pos: self.transform.transform_point3(Vec3::from(light.pos)).to_array(),
            spot: light.spot.map(|spot| ao_scene::Spot { dir: self.transform.transform_vector3(Vec3::from(spot.dir)).to_array(), ..spot }),
            ..light
        })
    }

    /// The actor to draw this frame (CPU-skinned pose, never frustum culled).
    pub fn frame(&self) -> ActorFrame {
        self.refresh_pose();
        ActorFrame { id: self.id, model: MODEL_KEY, transform: self.transform.to_cols_array_2d(), parts: self.current_parts.borrow().clone(), part_attractors: self.rig.part_attractors(), skin: self.current_skin.borrow().as_ref().map(Arc::clone), always: true, alpha: 1.0, ..Default::default() }
    }

    /// Native camera selection against the current own body pose, not mounted meshes.
    pub fn pick_hit(&self, ray: &super::hud_target::Ray, bounding_box: bool) -> Option<f32> {
        let pose = self.current_skin.borrow();
        let vertices = pose.as_ref()?;
        let bounds = self.current_bounds.get()?;
        let inverse = self.transform.inverse();
        let origin = inverse.transform_point3(ray.origin);
        let direction = inverse.transform_vector3(ray.dir);
        let length = ray.len * direction.length();
        let body = super::hud_pick::PickBody { id: self.id as i32, bounds, pos: [0.0; 3], yaw: 0.0, scale: 1.0 };
        if bounding_box {
            body.hit(origin, direction, length)
        } else {
            body.hit_triangles(origin, direction, length, vertices, self.rig.body_triangles())
        }
    }

    /// Current effect anchor in world scene space, including heading and body scale.
    pub fn effect_anchor(&self, id: i32) -> Option<[[f32; 4]; 4]> {
        let matrix = if self.current_skin.borrow().is_some() { self.rig.cached_effect_anchor(id, &self.pose_cache.borrow()) } else { self.rig.effect_anchor_composed(id, self.layers()) };
        matrix.map(|m| (self.transform * Mat4::from_cols_array_2d(&m)).to_cols_array_2d())
    }

    pub fn weapon_effect_anchor(&self, place: u8) -> Option<[[f32; 4]; 4]> {
        let matrix = if self.current_skin.borrow().is_some() { self.rig.cached_weapon_effect_anchor(place, &self.pose_cache.borrow()) } else { self.rig.weapon_effect_anchor_composed(place, self.layers()) };
        matrix.map(|m| (self.transform * Mat4::from_cols_array_2d(&m)).to_cols_array_2d())
    }

    /// `n3Dynel_t::GetBodyCollSphereRadi` (N3 0x10004dd3): the model's torso sphere radius (`VisualCATMesh_t::GetTorsoSphereRadi`), 0.5 when
    /// negative, times the body scale (`n3VisualDynel_t::UpdateCollision` N3 0x19be4).
    pub fn body_radius(&self) -> f32 {
        let r = self.rig.cat().torso_sphere.radius;
        (if r < 0.0 { ao_formats::playfield::collision::DEFAULT_BODY_RADIUS } else { r }) * self.scale
    }

    /// The visual dynel's body scale (`N3 GetBodyScale`, 0x10019622, dynel +0xac).
    pub fn body_scale(&self) -> f32 { self.scale }

    /// GC Spiral2 dynel ctor 0x10111dbc; DS torso getter 0x10072c22.
    /// Unlike collision radius this preserves the CAT's authored negative radius.
    pub fn effect_torso_factor(&self) -> f32 {
        (self.rig.cat().torso_sphere.radius as f64 / 0.25075000524520874 * self.scale as f64) as f32
    }

    /// Body height in metres including `monster_scale` (eye height / name tag).
    #[cfg(test)]
    pub fn height(&self) -> f32 {
        self.rig.height() * self.scale
    }

    /// Scaled local head attractor (`N3 FUN_10020af1`), before the avatar heading.
    pub fn head_local(&self) -> Option<Vec3> {
        self.rig.head_attractor_composed(self.layers()).map(|p| Vec3::from(p) * self.scale)
    }

    /// Animated head height over the feet.
    #[cfg(test)]
    pub fn head_height(&self) -> Option<f32> {
        self.head_local().map(|p| p.y)
    }

    /// Scene position of the head attractor (`Attractor01_head`) in the current pose, the point `RefreshAlpha` measures the camera distance
    /// from (attractor translation x body scale, through the CAT frame's world matrix); the feet for a model without one (the identity
    /// attractor matrix `RefreshAlpha` starts from).
    #[cfg(test)]
    pub fn head_position(&self) -> Vec3 {
        self.transform.transform_point3(self.rig.head_attractor_composed(self.layers()).map_or(Vec3::ZERO, Vec3::from))
    }
}

#[cfg(test)]
pub(super) mod tests {
    pub(in crate::play) struct PostureReference {
        old: Arc<CatAnim>,
        old_ms: f32,
        old_rate: f32,
        idle: Arc<CatAnim>,
        enter: Arc<CatAnim>,
        duration: f32,
        delay: f32,
        idle_start: f32,
        idle_fade: f32,
        enter_fade: f32,
    }

    pub(in crate::play) fn posture_reference(avatar: &mut Avatar, store: &RecordStore, id: u16, idle: &Role) -> PostureReference {
        let (enter, _, _) = super::super::combat::anim::resolve_clip(&avatar.assets.names, avatar.anim_set, id, false).unwrap();
        let enter = avatar.assets.anim(store, enter).unwrap();
        let clips = avatar.assets.clips(store, avatar.rig.model_id).unwrap();
        let idle_id = clips.iter().find(|(name, _)| *name == idle.clip_name()).unwrap().1;
        let idle = avatar.assets.anim(store, idle_id).unwrap();
        let old = avatar.clip.clone().unwrap();
        let (old_start, old_end) = loop_span(&old).unwrap_or((0.0, old.duration));
        let old_span = old_end - old_start;
        let mut remaining = old_span - (avatar.ms - old_start).rem_euclid(old_span);
        if remaining < 200.0 { remaining += (200.0 / old_span).ceil() * old_span; }
        let (idle_start, idle_end) = loop_span(&idle).unwrap_or((0.0, idle.duration));
        let duration = enter.events.iter().find(|(_, name)| name == "loopstart").map_or(enter.duration, |(ms, _)| *ms as f32);
        PostureReference { old, old_ms: avatar.ms, old_rate: avatar.rate, idle, enter, duration,
            delay: (duration - 500.0).max(0.0), idle_start, idle_fade: remaining.clamp(200.0, 300.0),
            enter_fade: (idle_end - idle_start).clamp(200.0, 300.0) }
    }

    impl PostureReference {
        fn sample(&self, avatar: &Avatar, frame_no: i32) -> Vec<ao_scene::Vertex> {
            use ao_formats::character::AnimLayer;
            let elapsed = (frame_no + 1) as f32 * (1000.0 / 60.0);
            let idle_elapsed = (elapsed - self.delay).max(0.0);
            let old = (idle_elapsed < self.idle_fade).then_some(AnimLayer {
                clip: &self.old, ms: clip_time(&self.old, self.old_ms + elapsed * self.old_rate, false), layers: 3, blend: 1.0,
            });
            let idle = AnimLayer { clip: &self.idle, ms: clip_time(&self.idle, self.idle_start + idle_elapsed, false),
                layers: 3, blend: (idle_elapsed / self.idle_fade).clamp(0.0, 1.0) };
            let enter = (elapsed <= self.duration).then_some(AnimLayer {
                clip: &self.enter, ms: elapsed.min(self.duration), layers: 3,
                blend: if elapsed <= self.enter_fade { elapsed / self.enter_fade } else { (self.duration - elapsed) / self.enter_fade }.clamp(0.0, 1.0),
            });
            avatar.rig.pose_composed(old.into_iter().chain(Some(idle)).chain(enter)).0
        }
    }
    /// Samples real CAT geometry against the independent 60 Hz retail queue timeline.
    /// The baseline caller recreates old transient scheduling; its outgoing sampler wraps.
    pub(in crate::play) fn posture_sample(avatar: &Avatar, before: bool, frame_no: i32, reference: Option<&PostureReference>) -> (ActorFrame, Vec<ao_scene::Vertex>, String) {
        let posture = avatar.actions.iter().rev().find(|action| action.posture);
        let expected = if let Some(reference) = reference {
            assert!((avatar.prev_fade_ms - reference.idle_fade).abs() < 1e-4, "idle fade must come from the preceding configured CAT interval at enqueue");
            if let Some(action) = posture {
                assert!((action.fade_ms - reference.enter_fade).abs() < 1e-4, "enter fade must come from the queued idle interval at elapsed zero");
            }
            reference.sample(avatar, frame_no)
        } else {
            let expected_layers = avatar.layers().enumerate().map(|(i, layer)| {
                if i == 0 && avatar.prev_clip.is_some() && avatar.prev_once {
                    ao_formats::character::AnimLayer { ms: avatar.prev_ms.min(layer.clip.duration), ..layer }
                } else { layer }
            });
            avatar.rig.pose_composed(expected_layers).0
        };
        let mut frame = avatar.frame();
        if before {
            let old_layers = avatar.layers().enumerate().map(|(i, layer)| {
                if i == 0 && avatar.prev_clip.is_some() {
                    ao_formats::character::AnimLayer { ms: clip_time(layer.clip, avatar.prev_ms, false), ..layer }
                } else {
                    layer
                }
            });
            frame.skin = Some(Arc::new(avatar.rig.pose_composed(old_layers).0));
        }
        let trace = format!("{},{:.4},{:.4},{},{:.4},{},{:.4},{:.4},{},{:.4},{:.4},{:.4},{:.4}",
            avatar.clip_id, avatar.ms, avatar.clip.as_ref().map_or(0.0, |a| a.duration),
            avatar.prev_clip.as_ref().map_or(0, |a| a.source_id), avatar.prev_ms, avatar.prev_once,
            avatar.prev_fade_elapsed, avatar.prev_fade_ms, posture.map_or(0, |a| a.clip.source_id),
            posture.map_or(0.0, |a| a.ms), posture.map_or(0.0, |a| a.duration),
            posture.map_or(0.0, |a| a.sample().blend), avatar.body_delay_ms);
        (frame, expected, trace)
    }
    pub(in crate::play) fn posture_clock(avatar: &Avatar) -> (u32, f32) {
        (avatar.clip_id, avatar.ms)
    }
    pub(in crate::play) fn posture_duration(avatar: &mut Avatar, store: &RecordStore, id: u16, before: bool) -> f32 {
        let (id, _, _) = super::super::combat::anim::resolve_clip(&avatar.assets.names, avatar.anim_set, id, false).unwrap();
        let clip = avatar.assets.anim(store, id).unwrap();
        if before { clip.duration } else { posture_interval_end(&clip) }
    }

    pub(in crate::play) fn posture_baseline_idle(avatar: &mut Avatar) {
        // The original ground idle started at zero rather than the queued mode-2 interval.
        if avatar.pose.role == Role::SitGround {
            avatar.ms = 1000.0 / 60.0;
            avatar.sample_pose.set(true);
        }
    }

    #[test]
    fn own_yalmaha_shape_replaces_rig_and_restores_humanoid() {
        let Some(dir) = client() else { return };
        let store = RecordStore::open(&dir).unwrap();
        let mut avatar = Avatar::new(&store, &dir, 7, &own_update(), ActorAssets::new(&store).unwrap()).unwrap();
        let humanoid = avatar.rig.model_id;
        let humanoid_signature = avatar.rig.cat().signature;
        assert!(avatar.set_monster_data(&store, 99470).unwrap());
        assert_eq!(avatar.rig.model_id, 42370);
        assert_ne!(avatar.rig.cat().signature, humanoid_signature);
        assert_eq!(avatar.clip_id, 99429);
        assert_eq!(avatar.clip.as_ref().unwrap().signature, avatar.rig.cat().signature);
        assert!(avatar.prev_clip.is_none());
        let frame = avatar.frame();
        assert_eq!(frame.model, MODEL_KEY);
        assert!(!avatar.model().meshes.is_empty());
        assert!(frame.parts.iter().flatten().flatten().all(|v| v.is_finite()));
        let has_head = avatar.look.head.is_some() && avatar.rig.cat().attractors.iter().any(|a| a.name.ends_with("_head"));
        assert_eq!(frame.part_attractors.contains(&Some(0)), has_head);
        assert!(!avatar.set_monster_data(&store, 99470).unwrap());
        avatar.set_pose(&store, AvatarPose { role: Role::Run, speed: 7.0, ref_speed: 7.0 }).unwrap();
        assert_eq!(avatar.clip_id, 99435);
        avatar.set_pose(&store, AvatarPose::still(Role::Hover)).unwrap();
        assert_eq!(avatar.clip_id, 99429, "B2 holder falls back to authored idle; no Yalm hover clip");
        avatar.update(0.1);
        let (clip, clock) = (avatar.clip_id, avatar.ms);
        avatar.set_pose(&store, AvatarPose { role: Role::Hover, speed: 7.0, ref_speed: 7.0 }).unwrap();
        assert_eq!((avatar.clip_id, avatar.ms), (clip, clock), "moving mode 7 must not start a clip");
        avatar.set_body_scale(0.2);
        assert_eq!(avatar.body_scale(), 0.2);
        assert!((avatar.transform.x_axis.truncate().length() - 0.2).abs() < 1e-5);
        assert!(avatar.set_monster_data(&store, 0).unwrap());
        assert_eq!(avatar.rig.model_id, humanoid);
        assert_eq!(avatar.rig.cat().signature, humanoid_signature);
        assert!(avatar.prev_clip.is_none(), "different skeletons must never crossfade");
        assert_eq!(avatar.clip.as_ref().unwrap().signature, humanoid_signature);
        assert!(!avatar.frame().parts.is_empty(), "humanoid mounts must be restored");
        assert!(avatar.set_monster_data(&store, 202286).unwrap());
        assert_eq!(avatar.monster_visual().0, 202286);
        assert_eq!(avatar.rig.model_id, humanoid, "native null record lookup retains the humanoid resolver");
    }
    use super::*;
    use ao_net::{
        frame::Frame,
        n3::{self, dynel::Dynel, N3},
    };

    #[test]
    fn own_interrupted_loop_keeps_installed_playback_count() {
        let Some(dir) = client() else { return };
        let store = RecordStore::open(&dir).unwrap();
        let mut avatar = Avatar::new(&store, &dir, 7, &own_update(), ActorAssets::new(&store).unwrap()).unwrap();
        avatar.set_cast_loop(true);
        avatar.set_pose(&store, AvatarPose::still(Role::Clip("ground-start".into()))).unwrap();
        let duration = avatar.clip.as_ref().unwrap().duration;
        avatar.update((duration + 100.0) / 1000.0);
        avatar.set_cast_loop(false);
        avatar.set_pose(&store, AvatarPose::still(Role::Idle)).unwrap();
        assert!(!avatar.prev_once, "new cast configuration must not reclassify the outgoing looping node");
        let previous = avatar.layers().next().unwrap();
        assert_eq!(previous.ms, clip_time(previous.clip, duration + 100.0, false));
    }

    #[test]
    fn own_delayed_sit_reversal_preserves_visible_pose() {
        use ao_formats::character::AnimLayer;
        let Some(dir) = client() else { return };
        let store = RecordStore::open(&dir).unwrap();
        let mut avatar = Avatar::new(&store, &dir, 7, &own_update(), ActorAssets::new(&store).unwrap()).unwrap();
        avatar.update(1.0);
        avatar.play_posture(&store, 0xd5, AvatarPose::still(Role::SitGround)).unwrap();
        avatar.update(0.1);
        assert!(avatar.body_delay_ms > 0.0 && avatar.prev_fade_elapsed == 0.0);
        let before = avatar.rig.pose_composed(avatar.layers()).0;
        let standing = avatar.prev_clip.clone().unwrap();
        let standing_ms = avatar.prev_ms;
        let old_enter = avatar.actions.last().unwrap();
        let old_clip = old_enter.clip.clone();
        let old_ms = old_enter.ms;
        let old_duration = old_enter.duration;
        let old_fade = old_enter.fade_ms;
        avatar.play_posture(&store, 0xd6, AvatarPose::still(Role::Idle)).unwrap();
        let switched = avatar.rig.pose_composed(avatar.layers()).0;
        let delta = |a: &[ao_scene::Vertex], b: &[ao_scene::Vertex]| a.iter().zip(b)
            .map(|(a, b)| Vec3::from(a.pos).distance(Vec3::from(b.pos))).fold(0.0_f32, f32::max);
        assert!(delta(&before, &switched) < 1e-5, "a zero-time reversal must not expose the delayed seated idle");
        let new_enter = avatar.actions.last().unwrap();
        let new_clip = new_enter.clip.clone();
        let new_fade = new_enter.fade_ms;
        let dt = 1000.0 / 60.0;
        // Absolute one-frame bound from the two authored enters, unchanged standing
        // clock, and retail envelopes; no test-object layer list is used.
        let old_weight = if old_ms + dt <= old_fade { (old_ms + dt) / old_fade } else { (old_duration - old_ms - dt) / old_fade }.clamp(0.0, 1.0);
        let expected = avatar.rig.pose_composed([
            AnimLayer { clip: &standing, ms: clip_time(&standing, standing_ms + dt, false), layers: 3, blend: 1.0 },
            AnimLayer { clip: &old_clip, ms: old_ms + dt, layers: 3, blend: old_weight },
            AnimLayer { clip: &new_clip, ms: dt, layers: 3, blend: (dt / new_fade).clamp(0.0, 1.0) },
        ].into_iter()).0;
        avatar.update(1.0 / 60.0);
        let actual = avatar.rig.pose_composed(avatar.layers()).0;
        assert!(delta(&actual, &expected) < 1e-4);
        assert!(delta(&switched, &actual) <= delta(&switched, &expected) + 1e-4);
    }

    #[test]
    fn unlit_hidden_avatar_does_not_sample_a_pose_for_lights() {
        let Some(dir) = client() else { return };
        let store = RecordStore::open(&dir).unwrap();
        let mut update = own_update();
        update.attractors.clear();
        let avatar = Avatar::new(&store, &dir, 7, &update, ActorAssets::new(&store).unwrap()).unwrap();
        assert!(avatar.current_skin.borrow().is_none());
        assert!(avatar.lights().next().is_none());
        assert!(avatar.current_skin.borrow().is_none(), "unlit hidden bodies must not skin just to collect lights");
        assert!(avatar.sample_pose.get(), "light collection must leave the deferred body refresh intact");
    }

    #[test]
    fn smooth_animation_toggle_changes_native_refresh_not_clip_time() {
        let Some(dir) = client() else { return };
        let store = RecordStore::open(&dir).unwrap();
        let mut avatar = Avatar::new(&store, &dir, 7, &own_update(), ActorAssets::new(&store).unwrap()).unwrap();
        let radius = avatar.rig.bounding_radius;
        let camera = avatar.transform.w_axis.truncate() + Vec3::X * radius * 50.0;
        let first = avatar.frame().skin.unwrap();
        let head_anchor = avatar.effect_anchor(2002).expect("authored head attractor");
        for _ in 0..6 {
            avatar.update(0.02);
            avatar.set_animation_refresh(false, camera, true);
            assert!(Arc::ptr_eq(&first, &avatar.frame().skin.unwrap()), "distant disabled-smooth CAT holds six raster frames");
            assert_eq!(avatar.effect_anchor(2002), Some(head_anchor), "native effects use the retained skeleton too");
        }
        avatar.update(0.02);
        avatar.set_animation_refresh(true, camera, true);
        assert!(!Arc::ptr_eq(&first, &avatar.frame().skin.unwrap()), "next native countdown expiry resamples at the current clock");
        let next = avatar.frame().skin.unwrap();
        assert!(Arc::ptr_eq(&next, &avatar.frame().skin.unwrap()), "effect source reuses this frame's posed geometry");
    }

    #[test]
    fn own_body_pick_uses_posed_triangles_and_world_transform() {
        let Some(dir) = client() else { return };
        let store = RecordStore::open(&dir).unwrap();
        let mut avatar = Avatar::new(&store, &dir, 7, &own_update(), ActorAssets::new(&store).unwrap()).unwrap();
        avatar.set_transform([3.0, 2.0, -4.0], 0.7);
        let actor = avatar.frame();
        let vertices = actor.skin.as_ref().unwrap();
        assert!(Arc::ptr_eq(vertices, avatar.current_skin.borrow().as_ref().unwrap()), "renderer and targeting share the same posed allocation");
        let (lo, hi) = super::super::hud_pick::bounds_of(vertices.iter().map(|v| &v.pos)).unwrap();
        let mut actual = false;
        let mut empty_box = false;
        for x in 1..20 {
            for y in 1..20 {
                let origin = Vec3::new(lo[0] + (hi[0] - lo[0]) * x as f32 / 20.0, lo[1] + (hi[1] - lo[1]) * y as f32 / 20.0, hi[2] + 5.0);
                let ray = super::super::hud_target::Ray {
                    origin: avatar.transform.transform_point3(origin),
                    dir: avatar.transform.transform_vector3(-Vec3::Z).normalize(),
                    len: 100.0,
                };
                assert!(avatar.pick_hit(&ray, true).is_some());
                let hit = avatar.pick_hit(&ray, false);
                actual |= hit.is_some();
                empty_box |= hit.is_none();
            }
        }
        assert!(actual, "body triangles are reachable through the transformed own-avatar ray");
        assert!(empty_box, "bounding-box targeting must not be mistaken for body geometry");
    }

    #[test]
    fn own_action_notes_do_not_replay_retained_weapon_effects() {
        use super::super::combat::notes::id;
        let clip = CatAnim { source_id: 0, root: String::new(), events: vec![
            (0, "attack".into()), (0, "attack_effect_1".into()),
            (0, "attack_effect_4".into()), (0, "swish_punch".into()),
        ], version: 0, duration: 1000.0, signature: 0, param: 0.0, tracks: vec![] };
        let mut action = ActionClip { clip: Arc::new(clip), ms: 100.0, duration: 1000.0, posture: false, before_body: false, layer: 1, priority: -1, fade_ms: 200.0, layers: 3, note_fired: 0, rate: 1.0, key: None, slot: -1, role: None };
        assert_eq!(action.take_notes().map(|note| note.id).collect::<Vec<_>>(), vec![id::SWISH_PUNCH]);
        assert_eq!(action.take_notes().count(), 0, "authored notes still fire only once");
    }

    #[test]
    fn own_swing_history_preserves_slots_through_replacement_and_death() {
        let Some(dir) = client() else { return };
        let store = RecordStore::open(&dir).unwrap();
        let mut avatar = Avatar::new(&store, &dir, 7, &own_update(), ActorAssets::new(&store).unwrap()).unwrap();
        let clip = |time| Arc::new(CatAnim { source_id: 0, root: String::new(),
            events: vec![(time, "attack".into())], version: 0, duration: 1000.0,
            signature: 0, param: 0.0, tracks: vec![] });
        avatar.start_action(clip(300), 0, -1, 1.0, (Some(0x1a), 7, None));
        avatar.update(0.05);
        avatar.start_action(clip(100), 0, -1, 1.0, (Some(0xb), 0, None));
        avatar.play_swing(&store, Role::Clip("unarmed-rswing".into()), None, 0x1a, 7).unwrap();
        assert_eq!(avatar.actions.len(), 2, "any active node suppresses its list key");
        avatar.update(0.1);
        assert_eq!(avatar.take_notes(), vec![super::super::combat::notes::FiredNote { id: 0xb, slot: 0 }]);
        avatar.set_body_priority(-3);
        avatar.set_pose(&store, AvatarPose::still(Role::Clip("die-knees".into()))).unwrap();
        avatar.update(0.15);
        assert_eq!(avatar.take_notes(), vec![super::super::combat::notes::FiredNote { id: 0xb, slot: 7 }], "older special survives replacement and death");
        assert_eq!(avatar.actions.len(), 2);
        avatar.start_action(clip(2000), 0, -1, 1.0, (Some(0x19), 8, None));
        avatar.update(1.1);
        assert_eq!(avatar.take_notes(), vec![super::super::combat::notes::FiredNote { id: 0xb, slot: 8 }], "removal flushes pending attack markers");
        assert!(avatar.actions.is_empty());
    }

    #[test]
    fn own_social_actions_use_retail_clip_candidates() {
        let Some(dir) = client() else { return };
        let store = RecordStore::open(&dir).unwrap();
        let mut avatar = Avatar::new(&store, &dir, 7, &own_update(), ActorAssets::new(&store).unwrap()).unwrap();
        for (id, name) in [(6, "backflip"), (0x12, "eat"), (0x29, "puke")] {
            let expected = super::super::combat::anim::resolve_clip(&avatar.assets.names, avatar.anim_set, id, false).expect("retail social clip");
            assert_eq!(expected.1, id, "{name} should not need a fallback");
            assert!(avatar.play_action(&store, id, Some(0), -2).unwrap(), "{name}");
            assert_eq!(avatar.actions.last().unwrap().clip.source_id, expected.0);
        }
    }

    #[test]
    fn own_upper_action_preserves_locomotion_and_composes_effect_anchors() {
        let Some(dir) = client() else { return };
        let store = RecordStore::open(&dir).unwrap();
        let mut avatar = Avatar::new(&store, &dir, 7, &own_update(), ActorAssets::new(&store).unwrap()).unwrap();
        let walk = AvatarPose { role: Role::Walk, speed: 1.5, ref_speed: 1.5 };
        avatar.set_pose(&store, walk.clone()).unwrap();
        avatar.update(0.2);
        let body_id = avatar.clip_id;
        let body_ms = avatar.ms;
        assert!(avatar.play_action(&store, 107, None, -2).unwrap());
        assert_eq!((avatar.clip_id, avatar.ms), (body_id, body_ms));
        avatar.set_pose(&store, walk).unwrap();
        avatar.update(0.1);
        assert!(avatar.ms > body_ms, "locomotion clock continues under the action");
        let layers: Vec<_> = avatar.layers().collect();
        assert_eq!(layers.len(), 2);
        assert_eq!((layers[0].layers, layers[1].layers), (3, 3));
        assert_eq!(layers[0].clip.source_id, avatar.clip.as_ref().unwrap().source_id);
        assert_eq!(layers[1].blend, 0.5, "retail first action fades in over 200 ms");
        let local = avatar.rig.effect_anchor_composed(2000, avatar.layers()).expect("retail hand attractor");
        let expected = (avatar.transform * Mat4::from_cols_array_2d(&local)).to_cols_array_2d();
        assert_eq!(avatar.effect_anchor(2000), Some(expected), "effect samples the rendered composition");
        let root=avatar.transform.to_cols_array_2d();
        assert!(avatar.effect_anchor(i32::MAX).is_none(),"raw connector lookup remains absent");
        assert!(avatar.rig.weapon_effect_anchor_composed(1,avatar.layers()).is_none(),"unarmed fixture has no weapon geometry");
        assert_eq!(avatar.weapon_effect_anchor(1).or_else(||avatar.effect_anchor(0)),Some(root),"missing weapon geometry locator uses the actual visual root");
        assert!(avatar.play_action(&store, 107, Some(0), -1).unwrap());
        assert_eq!(avatar.actions[0].layer, 0, "explicit override beats table upper layer");
        assert_eq!(avatar.actions[0].priority, -1);
        assert_eq!(avatar.actions[1].priority, -2);
        assert_eq!(avatar.actions.len(), 2, "positive-count history survives for fade-out");
        let expected_fade = ao_formats::character::animation_fade_ms(avatar.actions[0].clip.duration, avatar.actions[0].ms);
        assert!(avatar.play_action(&store, 107, Some(0), -1).unwrap());
        assert_eq!(avatar.actions.len(), 3, "same-priority positive-count predecessor remains");
        assert_eq!(avatar.actions[1].fade_ms, expected_fade);
        avatar.update(30.0);
        assert!(avatar.actions.is_empty(), "actions expire at their authored durations");
        assert!(avatar.play_action(&store, 107, None, -1).unwrap());
        assert!(avatar.play_action(&store, 107, Some(0), -2).unwrap());
        assert_eq!(avatar.actions[1].layers, 2, "higher upper node excludes the lower node");
        avatar.actions[0].ms = avatar.actions[0].clip.duration + 1.0;
        avatar.update(0.0);
        assert_eq!(avatar.actions.len(), 1);
        assert_eq!(avatar.actions[0].layers, 2, "expiry does not reset the stored exclusion mask");
    }

    /// `Avatar::update` must fire the base locomotion clip's own notes (`FUN_1003c036`'s per-frame holder walk), not just
    /// the action overlays: a walk clip's `step` markers fire repeatedly as it loops, and the jump-landing clip's `land`
    /// marker (`jump-land-idle`, played once by `Player` on the airborne->grounded edge) fires once.
    #[test]
    fn base_clip_reports_its_own_step_and_land_notes() {
        let Some(dir) = client() else { return };
        let store = RecordStore::open(&dir).unwrap();
        let mut avatar = Avatar::new(&store, &dir, 7, &own_update(), ActorAssets::new(&store).unwrap()).unwrap();
        avatar.set_pose(&store, AvatarPose { role: Role::Walk, speed: 1.5, ref_speed: 1.5 }).unwrap();
        let mut steps = 0;
        for _ in 0..200 {
            avatar.update(0.05);
            steps += avatar.take_notes().iter().filter(|n| n.id == super::super::combat::notes::id::STEP).count();
        }
        assert!(steps >= 2, "a 10 s walk must cross several step markers more than once (got {steps})");
        avatar.set_pose(&store, AvatarPose::still(Role::Clip("jump-land-idle".into()))).unwrap();
        let mut landed = 0;
        while !avatar.finished() {
            avatar.update(0.05);
            landed += avatar.take_notes().iter().filter(|n| n.id == super::super::combat::notes::id::LAND).count();
        }
        assert_eq!(landed, 1, "the jump-landing clip's land marker fires exactly once");
    }

    /// `Vehicle_t+0x52` (`FUN_10045069`'s own guard): no step note while airborne, even while the walk clip keeps looping.
    #[test]
    fn step_notes_are_silent_while_airborne() {
        let Some(dir) = client() else { return };
        let store = RecordStore::open(&dir).unwrap();
        let mut avatar = Avatar::new(&store, &dir, 7, &own_update(), ActorAssets::new(&store).unwrap()).unwrap();
        avatar.set_pose(&store, AvatarPose { role: Role::Walk, speed: 1.5, ref_speed: 1.5 }).unwrap();
        avatar.set_airborne(true);
        let mut steps = 0;
        for _ in 0..200 {
            avatar.update(0.05);
            steps += avatar.take_notes().iter().filter(|n| n.id == super::super::combat::notes::id::STEP).count();
        }
        assert_eq!(steps, 0, "airborne suppresses every step note, however many loops the clip crosses");
    }

    /// Movement is an infinite-count node: authored loop interval, linear blend, clip-rate clock.
    #[test]
    fn own_role_change_crossfades_body_pose_instead_of_popping() {
        let Some(dir) = client() else { return };
        let store = RecordStore::open(&dir).unwrap();
        let mut avatar = Avatar::new(&store, &dir, 7, &own_update(), ActorAssets::new(&store).unwrap()).unwrap();
        let idle_clip = avatar.clip.clone().expect("retail idle clip resolves");
        avatar.set_pose(&store, AvatarPose { role: Role::Run, speed: 2.5, ref_speed: 5.0 }).unwrap();
        let run = avatar.clip.as_ref().unwrap();
        let start = loop_span(run).expect("retail run has loop markers").0;
        assert!(start > 0.0);
        assert_eq!(avatar.ms, start, "GC 100108be mode 2 skips the intro, not a run-start transition");
        assert_eq!(avatar.prev_clip.as_ref().unwrap().source_id, idle_clip.source_id);
        assert_eq!(avatar.prev_fade_ms, body_fade_ms(&idle_clip, 0.0, false, true, false));
        let layers: Vec<_> = avatar.layers().collect();
        assert_eq!(layers.len(), 2);
        assert_eq!((layers[0].clip.source_id, layers[0].blend), (idle_clip.source_id, 1.0));
        assert_eq!((layers[1].ms, layers[1].blend), (start, 0.0));
        let half = avatar.prev_fade_ms / 2.0;
        avatar.update(half / 1000.0 / avatar.rate);
        assert!((avatar.layers().last().unwrap().blend - 0.5).abs() < 1e-6, "DS 10072562 is linear; 10072fde scales its clock by incoming rate");
        assert!((avatar.ms - start - half).abs() < 1e-3);
        avatar.update((half + 1.0) / 1000.0 / avatar.rate);
        assert!(avatar.prev_clip.is_none());
        assert_eq!(avatar.layers().count(), 1);
        assert_eq!(avatar.layers().next().unwrap().blend, 1.0);
        avatar.set_pose(&store, AvatarPose { role: Role::Walk, speed: 1.5, ref_speed: 1.5 }).unwrap();
        assert_eq!(avatar.ms, loop_span(avatar.clip.as_ref().unwrap()).unwrap().0, "walk/run changes restart the named interval too, never inherit gait phase");
    }

    #[test]
    fn own_movement_clips_match_retail_animsets() {
        let Some(dir) = client() else { return };
        let store = RecordStore::open(&dir).unwrap();
        let mut assets = ActorAssets::new(&store).unwrap();
        // Name table 1000010/1 and CAT headers 1010003: set, stand, two-hand idle,
        // bazooka idle, ordinary run, rifle run, authored run-loop starts.
        for (breed, sex, stand, two_h, bazooka, run, rifle_run, start, rifle_start) in [
            (1, 2, 10173, 21626, 265784, 10194, 81717, 1166.0, 1166.0),
            (1, 3, 10135, 13576, 265786, 10146, 81716, 1166.0, 566.0),
            (4, 2, 9992, 21475, 265782, 9386, 81715, 1166.0, 1166.0),
        ] {
            let mut u = own_update();
            u.breed = breed;
            u.sex = sex;
            u.head_mesh = None;
            u.attractors.clear();
            let mut avatar = Avatar::new(&store, &dir, 7, &u, assets).unwrap();
            for (set, idle, run, start) in [
                (None, stand, run, start),
                (Some(0), stand, run, start),
                (Some(3), two_h, rifle_run, rifle_start),
                (Some(6), stand, run, start),
                (Some(8), bazooka, run, start),
            ] {
                avatar.set_stance(set);
                avatar.set_pose(&store, AvatarPose::still(Role::Idle)).unwrap();
                assert_eq!(avatar.clip_id, idle, "{breed}/{sex} AnimSet {set:?}: peace idle");
                avatar.set_pose(&store, AvatarPose { role: Role::Run, speed: 5.0, ref_speed: 5.0 }).unwrap();
                assert_eq!(avatar.clip_id, run, "{breed}/{sex} AnimSet {set:?}: run");
                assert_eq!(avatar.ms, start);
                let clip = avatar.clip.as_ref().unwrap();
                for (i, (ms, name)) in clip.events.iter().enumerate().filter(|(_, (ms, _))| (*ms as f32) < start) {
                    assert_ne!(avatar.note_fired & (1 << (i & 31)), 0, "intro note {name}@{ms} must not fire on loop entry");
                }
            }
            assets = avatar.into_assets();
        }
    }

    /// Optional, headless-only before/after frames; baseline reproduces 5eca9fb's zero-phase start.
    #[test]
    fn idle_to_run_before_after_frame_sequence() {
        let Some(output) = std::env::var_os("AVATAR_RUN_SEQUENCE") else { return };
        let Some(dir) = client() else { return };
        let store = RecordStore::open(&dir).unwrap();
        let output = std::path::PathBuf::from(output);
        std::fs::create_dir_all(&output).unwrap();
        let u = own_update();
        let pos = super::super::zone::scene_pos(u.pos);
        let yaw = u.yaw().unwrap_or(0.0);
        let f = super::super::zone::scene_forward(yaw);
        let at = [pos[0], pos[1] + 1.0, pos[2]];
        let eye = [at[0] + f[0] * 3.0, at[1] + 0.3, at[2] + f[2] * 3.0];
        let scene = ao_formats::playfield::load_playfield(&store, &dir, 4604).unwrap();
        for (stance, label) in [(None, "unarmed"), (Some(3), "rifle"), (Some(8), "bazooka")] {
            for before in [true, false] {
                let mut avatar = Avatar::new(&store, &dir, 7, &u, ActorAssets::new(&store).unwrap()).unwrap();
                avatar.set_stance(stance);
                avatar.set_pose(&store, AvatarPose::still(Role::Idle)).unwrap();
                avatar.update(1.0);
                let idle = avatar.clip.clone().unwrap();
                let idle_ms = avatar.ms;
                let version = if before { "before" } else { "after" };
                let models = [(MODEL_KEY, avatar.model().clone())];
                let png = output.join(format!("{label}-{version}-idle.png"));
                ao_render::render_to_png_actors(&scene, &models, vec![avatar.frame()], eye, at, 900, 600, &png, 0.0).unwrap();
                avatar.set_pose(&store, AvatarPose { role: Role::Run, speed: 5.0, ref_speed: 5.0 }).unwrap();
                if before {
                    avatar.ms = 0.0;
                    avatar.prev_fade_ms = animation_fade_ms(idle.duration, idle_ms);
                }
                eprintln!("{label}-{version}: set={} idle={}/{}ms run={} interval={:?} initial={} fade={} rate={}", avatar.anim_set, idle.source_id, idle.duration, avatar.clip_id, loop_span(avatar.clip.as_ref().unwrap()), avatar.ms, avatar.prev_fade_ms, avatar.rate);
                let mut last = 0;
                for ms in [0, 16, 60, 120, 200, 300, 400] {
                    let previous = if before { avatar.prev_clip.clone() } else { None };
                    avatar.update((ms - last) as f32 / 1000.0);
                    if before {
                        // 5eca9fb advanced the fade in wall time, independently of the clip rate.
                        avatar.prev_fade_elapsed = ms as f32;
                        avatar.prev_clip = previous.filter(|_| avatar.prev_fade_elapsed < avatar.prev_fade_ms);
                    }
                    let png = output.join(format!("{label}-{version}-{ms:03}.png"));
                    ao_render::render_to_png_actors(&scene, &models, vec![avatar.frame()], eye, at, 900, 600, &png, 0.0).unwrap();
                    last = ms;
                }
            }
        }
    }

    pub(in crate::play) fn own_update() -> SimpleCharFullUpdate {
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
        let mut avatar = Avatar::new(&store, &dir, 7, &u, ActorAssets::new(&store).unwrap()).unwrap();
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
        u.cloth.push(ao_net::n3::dynel::ClothData { raw: 1, texture: 777, page: 1, extra: None }); // same part, higher page: wins over the page-0 entry
        u.cloth.push(ao_net::n3::dynel::ClothData { raw: 2, texture: 99, page: 0, extra: None }); // a different part: visible independently, not merged with part 1
        let l = AvatarLook::from_update(&u, |_| Skin::Asian).unwrap();
        assert_eq!((l.look.build, l.scale, l.look.skin), (2, 1.5, Skin::Asian));
        assert_eq!(l.attachments, vec![(1, 7796)]);
        assert_eq!(l.look.equipment.0, [None, Some(777), Some(99), None, None]);
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
        assert_eq!(clip_time(&a, 5000.0, true), 1000.0);
        let b = CatAnim { events: vec![], ..a };
        assert_eq!(clip_time(&b, 1100.0, false), 100.0);
        assert_eq!(body_fade_ms(&c, 550.0, false, true, false), 300.0, "50 ms left in a 400 ms loop is extended by a cycle, then clamped");
        assert_eq!(body_fade_ms(&c, 350.0, false, true, false), 250.0, "250 ms left requires no extra cycle");
        assert_eq!(body_fade_ms(&c, 550.0, false, true, true), 200.0, "positive-count successor has no infinite-cycle extension");
        let short = CatAnim { duration: 100.0, ..b };
        assert_eq!(body_fade_ms(&short, 90.0, false, true, false), 210.0, "ceil(200 / 100) cycles plus the 10 ms remainder");
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
        let mut a = Avatar::new(&store, &dir, 7, &own_update(), ActorAssets::new(&store).unwrap()).unwrap();
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
        let mut a = Avatar::new(&store, &dir, 7, &u, ActorAssets::new(&store).unwrap()).unwrap();
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
        let mut a = Avatar::new(&store, &dir, 7, &own_update(), ActorAssets::new(&store).unwrap()).unwrap();
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

    /// The idle clip keeps playing and looping while the character stands still: `Player::frame` calls `Avatar::update`
    /// unconditionally every frame and `anim_rate` falls back to the authored rate 1.0 at zero speed (not 0), so the body
    /// pose must keep changing over several seconds of idle instead of freezing on its first sampled frame; confirmed with
    /// an offscreen render sequence (`AVATAR_IDLE_SEQ=<dir>`, not part of this test) showing the authored breathing /
    /// weight-shift motion directly.
    #[test]
    fn own_idle_clip_keeps_advancing_and_wraps_over_several_seconds() {
        let Some(dir) = client() else { return };
        let store = RecordStore::open(&dir).unwrap();
        let mut a = Avatar::new(&store, &dir, 7, &own_update(), ActorAssets::new(&store).unwrap()).unwrap();
        assert_eq!(a.rate, 1.0, "zero speed falls back to the authored playback rate, not a stall");
        let duration = a.clip.as_ref().expect("retail idle clip resolves").duration;
        assert!(duration > 1000.0, "retail idle-stand is a multi-second authored clip, not a single pose");
        let mut prev = a.frame().skin.as_ref().unwrap().iter().map(|v| v.pos).collect::<Vec<_>>();
        let mut moved_every_sample = true;
        let steps = (duration / 100.0).ceil() as usize + 10;
        for _ in 0..steps {
            a.update(0.1);
            let pos = a.frame().skin.as_ref().unwrap().iter().map(|v| v.pos).collect::<Vec<_>>();
            moved_every_sample &= pos.iter().zip(&prev).any(|(x, y)| x != y);
            prev = pos;
        }
        assert!(moved_every_sample, "the idle pose must keep changing every 0.1 s, not freeze at one frame");
        assert!(a.ms > duration, "playback must wrap past the clip's own duration");
    }

    /// Real model: roles pick different clips, a pose moves the body, the frame is transformed by position + heading.
    /// With `AVATAR_SHOT=<png>` it also renders the character in the Arrival Hall (4604) offscreen, 3 m in front of it.
    #[test]
    fn avatar_poses_and_renders_in_the_arrival_hall() {
        let Some(dir) = client() else { return };
        let store = RecordStore::open(&dir).unwrap();
        let u = own_update();
        let mut a = Avatar::new(&store, &dir, 7, &u, ActorAssets::new(&store).unwrap()).unwrap();
        let idle = a.frame();
        a.set_pose(&store, AvatarPose { role: Role::Run, speed: 5.0, ref_speed: 5.0 }).unwrap();
        a.update(0.2);
        let run = a.frame();
        let (i, r) = (idle.skin.as_ref().unwrap(), run.skin.as_ref().unwrap());
        assert_eq!(i.len(), r.len());
        assert!(i.iter().zip(r.iter()).any(|(x, y)| x.pos != y.pos), "run moved nothing");
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
    fn cloth_pages_choose_highest_nonzero_and_reveal_lower_page() {
        let mut pages = vec![(1, 10), (6, 20), (11, 30), (2, 40)];
        assert_eq!(visible_cloth(&pages).collect::<Vec<_>>(), [(1, 30), (2, 40)]);
        pages.retain(|p| p.0 != 11);
        assert_eq!(visible_cloth(&pages).collect::<Vec<_>>(), [(1, 20), (2, 40)]);
        pages.retain(|p| p.0 != 6);
        assert_eq!(visible_cloth(&pages).collect::<Vec<_>>(), [(1, 10), (2, 40)]);
    }

    #[test]
    fn nano_cast_loop_restarts_and_release_finishes() {
        let Some(dir) = client() else { return };
        let store = RecordStore::open(&dir).unwrap();
        let mut avatar = Avatar::new(&store, &dir, 7, &own_update(), ActorAssets::new(&store).unwrap()).unwrap();
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
        let mut a = Avatar::new(&store, &dir, 7, &own_update(), ActorAssets::new(&store).unwrap()).unwrap();
        let head = a.look.head.unwrap();
        let appearance = |list: &[(u8, u32)]| ao_net::n3::world::AppearanceUpdate { cloth: Vec::new(), attractors: list.iter().map(|&(a, b)| ao_net::n3::world::Attractor { a, b: b as i32, c: 0, d: 0 }).collect(), visual_flags: 31, extra: 0 };
        let plain = (a.model().meshes.len(), a.frame().parts.len());
        assert!(!a.set_appearance(&store, &appearance(&[(0, head)])).unwrap(), "the login list is unchanged");
        assert!(a.set_appearance(&store, &appearance(&[(1, 0x3ddf), (0, head)])).unwrap());
        assert_eq!((a.model().meshes.len(), a.frame().parts.len()), (plain.0 + 1, plain.1 + 1), "the rifle is one more mounted mesh");
        let held_frame = a.frame();
        assert_eq!(held_frame.part_attractors, a.rig.part_attractors(), "the frame preserves the rig's authored mount places");
        assert_eq!(held_frame.part_attractors.len(), a.model().meshes.len());
        assert_eq!(held_frame.part_attractors[0], None, "the root body is not an attractor child");
        assert!(!a.set_appearance(&store, &appearance(&[(0, head), (1, 0x3ddf)])).unwrap(), "same set, other order");
        let mut flags_only = appearance(&[(0, head), (1, 0x3ddf)]);
        flags_only.visual_flags = 0;
        assert!(!a.set_appearance(&store, &flags_only).unwrap(), "native meshes follow the returned list, not a guessed flag mask");
        assert_eq!((a.model().meshes.len(), a.frame().parts.len()), (plain.0 + 1, plain.1 + 1));
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
        assert_eq!(a.frame().part_attractors, a.rig.part_attractors(), "unwear removes the old held-part metadata");
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
