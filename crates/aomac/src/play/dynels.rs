//! Other players, NPCs and monsters in the zone, drawn as renderer actors (`ao_scene::ActorFrame`).
//!
//! `Dynels::on_message` keeps one [`Char`] per `SimpleCharFullUpdate` (removed on `n3ToClientQuit`) with the client's movement
//! simulation of it ([`ao_net::n3::motion::Mover`]); a worker thread builds the textured, poseable model of every distinct
//! appearance ([`Look`]) once ([`ao_formats::character::actor::ActorRig`]); `update` advances the characters, picks their clip
//! from the movement state, skins the visible ones at a distance dependent rate and pushes them to `Host::actors`.
//! Evidence for the field meanings: docs/zone/dynel.md §1, docs/zone/npc.md, docs/zone/motion.md.

use anyhow::Context;
use ao_formats::character::actor::{npc_part_textures, ActorAssets, ActorRig, PlayerLook};
use ao_formats::character::{load_cat_mesh, CatAnim, ClothPart, Equipment, NpcRecord, TextureOverride, CHAR_MESH_TYPE};
use ao_gui::{DrawList, FontId, Gui};
use ao_net::n3::dynel::{Dynel, SimpleCharFullUpdate};
use ao_net::n3::misc::Misc;
use ao_net::n3::motion::{max_speed, AnimState, Mode, Mover, STAT_HEALTH};
use ao_net::n3::nametag::{name_tag, name_tag_color, NameTagInput, INVALID_STAT};
use ao_net::n3::{Message, N3};
use ao_rdb::RecordStore;
use ao_render::{Camera, Host};
use ao_scene::{ActorFrame, Lens};
use std::collections::{hash_map::DefaultHasher, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use super::zone::{scene_pos, scene_yaw};

/// Identity kind of character / NPC dynels (`SimpleChar_t`).
const CHAR_KIND: i32 = 0xC350;
/// Characters farther than this (metres) are not drawn (about the fog distance of the outdoor playfields).
pub const DRAW_DISTANCE: f32 = 250.0;
/// `ShowAllNames` name tags exist for dynels within this radius of the player (`GetDynelsInVicinity`, docs/zone/motion.md §6).
pub const NAME_TAG_RADIUS: f32 = 30.0;
/// NPC record key of the generic death clip and of the first unarmed attack ([`ao_formats::character::NpcAnim`]).
const DIE_KEY: u32 = 6000;
const ATTACK_KEY: u32 = 1033;

/// Everything of a `SimpleCharFullUpdate` that decides what the model looks like; equal looks share one model.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct Look {
    pub npc: bool,
    pub breed: u8,
    pub sex: u8,
    pub race: u8,
    pub fatness: u8,
    pub head: Option<i32>,
    /// Stat `MonsterData` (0x167): rdb 1040023 NPC record id.
    pub monster_data: i32,
    /// `TextureData_t`: (material name, rdb 1010004 texture).
    pub textures: Vec<(String, i32)>,
    /// `ClothData_t` page 0: (part, rdb 1010004 texture).
    pub cloth: Vec<(i32, i32)>,
    /// `AttractorMeshData_t`: (place, rdb 1010001 mesh).
    pub attractors: Vec<(u8, i32)>,
}

impl Look {
    pub fn from_update(u: &SimpleCharFullUpdate) -> Self {
        Self {
            npc: u.is_npc(),
            breed: u.breed,
            sex: u.sex,
            race: u.race,
            fatness: u.fatness,
            head: u.head_mesh,
            monster_data: u.monster_data,
            textures: u.textures.iter().map(|t| (t.material.clone(), t.texture)).collect(),
            cloth: u.cloth.iter().filter(|c| c.page == 0).map(|c| (c.part(), c.texture)).collect(),
            attractors: u.attractors.iter().map(|a| (a.place, a.mesh)).collect(),
        }
    }

    pub fn key(&self) -> u64 {
        let mut h = DefaultHasher::new();
        self.hash(&mut h);
        h.finish()
    }
}

/// A built model: the rig (pose source), the clips of its set by client animation id (`AnimState::anim_id`, plus
/// [`DIE_KEY`] / [`ATTACK_KEY`]) and the NPC record's `Features` stat.
pub struct Built {
    pub rig: Arc<ActorRig>,
    pub clips: HashMap<u32, Arc<CatAnim>>,
    pub features: Option<i32>,
    /// Name tag anchor height, metres above the feet at scale 1.
    pub tag_height: f32,
}

enum Model {
    Loading,
    Failed,
    Ready { built: Built, uploaded: bool },
}

struct Req {
    key: u64,
    look: Look,
}

struct Resp {
    key: u64,
    result: Result<Built, String>,
}

/// Background builder: reads the rdb and composes textures off the UI thread.
struct Worker {
    tx: Sender<Req>,
    rx: Receiver<Resp>,
}

impl Worker {
    fn start(dir: PathBuf) -> Worker {
        let (tx, req_rx) = channel::<Req>();
        let (resp_tx, rx) = channel();
        std::thread::spawn(move || {
            let (store, mut assets) = match RecordStore::open(&dir).and_then(|s| Ok((ActorAssets::new(&s)?, s))) {
                Ok((a, s)) => (s, a),
                Err(e) => {
                    eprintln!("dynels: rdb: {e:#}");
                    return;
                }
            };
            for r in req_rx {
                let result = build(&store, &mut assets, &r.look).map_err(|e| format!("{e:#}"));
                if resp_tx.send(Resp { key: r.key, result }).is_err() {
                    return;
                }
            }
        });
        Worker { tx, rx }
    }
}

/// The states with a clip of their own (`Attack`/`Die` are loaded under [`ATTACK_KEY`]/[`DIE_KEY`]).
const STATES: &[AnimState] = &[
    AnimState::Idle,
    AnimState::Walk,
    AnimState::Run,
    AnimState::WalkBack,
    AnimState::RunBack,
    AnimState::WalkLeft,
    AnimState::WalkRight,
    AnimState::TurnLeft,
    AnimState::TurnRight,
    AnimState::JumpStand,
    AnimState::JumpForward,
    AnimState::Swim,
    AnimState::IdleSwim,
    AnimState::Sneak,
    AnimState::Crawl,
    AnimState::IdleCrawl,
    AnimState::Hover,
    AnimState::Fly,
    AnimState::SitGround,
    AnimState::Sleep,
    AnimState::Lounge,
];

fn build(store: &RecordStore, assets: &mut ActorAssets, look: &Look) -> anyhow::Result<Built> {
    let attachments: Vec<(u8, u32)> = look.attractors.iter().filter(|a| a.0 != 0).map(|a| (a.0, a.1 as u32)).collect();
    let (rig, rec) = if look.npc {
        let (rig, rec) = npc_rig(store, look, &attachments)?;
        (rig, Some(rec))
    } else {
        let (breed, gender) = ao_formats::screens::wire_breed_sex(look.breed as i32, look.sex as i32)?;
        // `BreedRace_e`: 1 caucasian, 2 african, 3 asian (docs/zone/dynel.md §1.3)
        let skin = match look.race {
            2 => ao_formats::character::Skin::African,
            3 => ao_formats::character::Skin::Asian,
            _ => ao_formats::character::Skin::Caucasian,
        };
        let head = look.head.or_else(|| look.attractors.iter().find(|a| a.0 == 0).map(|a| a.1)).map(|h| h as u32);
        let mut equipment = Equipment::default();
        for &(part, tex) in &look.cloth {
            if let Some(p) = ClothPart::ALL.get(part as usize).filter(|_| tex > 0) {
                equipment.wear(*p, tex as u32);
            }
        }
        let look = PlayerLook { breed, gender, skin, build: look.fatness.min(2), head, equipment };
        (ActorRig::player(store, assets, &look, &attachments)?, None)
    };
    let sig = rig.cat().signature;
    let mut clips = HashMap::new();
    match &rec {
        // NPC: the record's table, keyed by the client animation id (`AbstractAnimID_e`)
        Some(rec) => {
            let keys = STATES.iter().filter_map(|s| s.anim_id()).chain([DIE_KEY, ATTACK_KEY]);
            for key in keys {
                let id = if key == DIE_KEY || key == ATTACK_KEY { ao_formats::character::anim_key(rec, key) } else { rec.anim_variants(key).first().copied() };
                if let Some(id) = id {
                    let a = assets.anim(store, id)?;
                    if a.signature == sig {
                        clips.insert(key, a);
                    }
                }
            }
        }
        // player: the model set's clips by name
        None => {
            for s in STATES {
                if let (Some(name), Some(id)) = (s.clip_name(), s.anim_id()) {
                    if let Some(a) = named(store, assets, rig.model_id, name)? {
                        clips.insert(id, a);
                    }
                }
            }
            for (key, name) in [(DIE_KEY, "die-knees"), (ATTACK_KEY, "unarmed-rswing")] {
                if let Some(a) = named(store, assets, rig.model_id, name)? {
                    clips.insert(key, a);
                }
            }
        }
    }
    let tag_height = rig.indicator_height();
    Ok(Built { rig: Arc::new(rig), clips, features: rec.as_ref().and_then(|r| r.stat(ao_net::n3::motion::STAT_FEATURES as u32)), tag_height })
}

fn named(store: &RecordStore, assets: &mut ActorAssets, model: u32, name: &str) -> anyhow::Result<Option<Arc<CatAnim>>> {
    let id = assets.clips(store, model)?.iter().find(|c| c.0 == name).map(|c| c.1);
    id.map(|id| assets.anim(store, id)).transpose()
}

/// An NPC: the record's model (`MonsterData` -> rdb 1040023 `Mesh`), `textures[]` replacing the part textures (`SetCATTextures`),
/// worn `cloth[]` composited over the part's texture like the player equipment, head and attachment meshes.
fn npc_rig(store: &RecordStore, look: &Look, attachments: &[(u8, u32)]) -> anyhow::Result<(ActorRig, NpcRecord)> {
    let rec = NpcRecord::load(store, look.monster_data as u32)?;
    let model = rec.mesh().context("NPC record has no mesh")?;
    let cat = load_cat_mesh(store, CHAR_MESH_TYPE, model)?;
    let list: Vec<TextureOverride> = look.textures.iter().map(|(m, t)| TextureOverride { material: m, texture: *t as u32, env_texture: 0, alpha_mode: 0 }).collect();
    let cloth: Vec<(ClothPart, u32)> = look.cloth.iter().filter(|c| c.1 > 0).filter_map(|&(p, t)| Some((*ClothPart::ALL.get(p as usize)?, t as u32))).collect();
    let overrides = npc_part_textures(store, &cat, &list, &cloth);
    let head = look.head.map(|h| h as u32).or_else(|| rec.head_mesh());
    let rig = ActorRig::new(store, model, head, &overrides, attachments)?;
    Ok((rig, rec))
}

/// What a character is doing besides moving.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Special {
    None,
    /// The attack clip plays once.
    Attack,
    /// The death clip plays once and holds its last frame.
    Die,
}

/// One character/NPC dynel.
pub struct Char {
    pub name: String,
    pub npc: bool,
    pub look: Look,
    pub key: u64,
    pub mover: Mover,
    /// `MonsterScale / 100`.
    pub scale: f32,
    pub side: u8,
    pub level: i16,
    /// Stat `Flags` (0) and `VisualFlags` (0x2A1), name tag inputs.
    flags: i32,
    visual_flags: i32,
    pose: ao_net::n3::motion::Pose,
    anim: u32,
    special: Special,
    clip_ms: f32,
    pose_in: f32,
    submitted: bool,
    /// Mount transforms of the last pose.
    parts: Vec<[[f32; 4]; 4]>,
    features_set: bool,
}

pub struct Dynels {
    dir: Option<PathBuf>,
    worker: Option<Worker>,
    own: i32,
    pub chars: HashMap<i32, Char>,
    models: HashMap<u64, Model>,
    asked: HashSet<u64>,
    /// Pref `ShowAllNames` (default off, docs/zone/motion.md §6): name tags over every dynel within [`NAME_TAG_RADIUS`].
    pub show_all_names: bool,
    /// Lens of the playfield scene (projection of the name tags).
    pub lens: Lens,
}

impl Default for Dynels {
    fn default() -> Self {
        Self {
            dir: None,
            worker: None,
            own: 0,
            chars: HashMap::new(),
            models: HashMap::new(),
            asked: HashSet::new(),
            show_all_names: std::env::var_os("AOMAC_SHOW_ALL_NAMES").is_some(),
            lens: Lens::default(),
        }
    }
}

/// Clip for `state`: its own, else the client's fallback chain, else idle.
fn clip_of(built: &Built, mut state: AnimState) -> Option<(u32, &Arc<CatAnim>)> {
    loop {
        if let Some(id) = state.anim_id() {
            if let Some(a) = built.clips.get(&id) {
                return Some((id, a));
            }
        }
        match state.fallback() {
            Some(next) => state = next,
            None => break,
        }
    }
    built.clips.get(&0x78).map(|a| (0x78, a))
}

impl Dynels {
    /// Starts the model builder for the client at `dir`; `own` = the player's own instance id (drawn by the avatar code).
    pub fn start(&mut self, dir: PathBuf, own: i32) {
        self.own = own;
        self.dir = Some(dir);
    }

    /// A new playfield: every dynel of the old one is gone (models stay built).
    pub fn clear(&mut self) {
        self.chars.clear();
    }

    /// The attack clip of `id` plays once (combat messages).
    pub fn attack(&mut self, id: i32) {
        if let Some(c) = self.chars.get_mut(&id).filter(|c| c.special == Special::None) {
            c.special = Special::Attack;
            c.clip_ms = 0.0;
        }
    }

    /// `id` dies: the death clip plays once and holds.
    pub fn die(&mut self, id: i32) {
        if let Some(c) = self.chars.get_mut(&id) {
            c.special = Special::Die;
            c.clip_ms = 0.0;
        }
    }

    pub fn on_message(&mut self, m: &Message) {
        let who = m.header.target;
        if who.kind != CHAR_KIND {
            return;
        }
        match &m.body {
            N3::Dynel(Dynel::SimpleCharFullUpdate(u)) => {
                let look = Look::from_update(u);
                let key = look.key();
                let yaw = u.yaw().unwrap_or(0.0);
                let mut mover = Mover::with_blob(u.pos, yaw, &u.blob);
                // players: [INFERENCE] bit 4 (their moves are applied live); NPCs get their record's `Features` when the model is built
                mover.set_features(if u.is_npc() { 2 } else { 4 });
                mover.on_stat(ao_net::n3::motion::STAT_RUN_SPEED, u.run_speed as i32);
                mover.on_stat(ao_net::n3::motion::STAT_MAX_HEALTH, u.max_health);
                mover.on_stat(STAT_HEALTH, u.health);
                if let Some(p) = &u.path {
                    mover.on_path(p.id.kind != 0 || p.id.instance != 0, &p.waypoints);
                }
                let pose = mover.pose();
                self.chars.insert(
                    who.instance,
                    Char {
                        name: u.name.clone(),
                        npc: u.is_npc(),
                        look,
                        key,
                        mover,
                        scale: if u.monster_scale > 0 { u.monster_scale as f32 / 100.0 } else { 1.0 },
                        side: u.side,
                        level: u.level,
                        flags: u.flags2 as i32,
                        visual_flags: u.visual_flags as i32,
                        pose,
                        anim: 0x78,
                        special: if u.max_health > 0 && u.health <= 0 { Special::Die } else { Special::None },
                        clip_ms: (who.instance as u32 % 1000) as f32 * 3.0,
                        pose_in: 0.0,
                        submitted: false,
                        parts: vec![],
                        features_set: !u.is_npc(),
                    },
                );
            }
            N3::Dynel(Dynel::CharDCMove(mv)) => {
                if let Some(c) = self.chars.get_mut(&who.instance) {
                    c.mover.on_char_dc_move(mv);
                }
            }
            N3::Dynel(Dynel::SetWantedDirection(d)) => {
                if let Some(c) = self.chars.get_mut(&who.instance) {
                    c.mover.on_wanted_direction(d.dir);
                }
            }
            N3::Dynel(Dynel::Stat(s)) => {
                if let Some(c) = self.chars.get_mut(&who.instance) {
                    for &(stat, value) in &s.stats {
                        c.mover.on_stat(stat, value);
                        if stat == STAT_HEALTH && value <= 0 {
                            c.special = Special::Die;
                            c.clip_ms = 0.0;
                        }
                    }
                }
            }
            N3::Misc(Misc::FollowTarget(f)) => {
                if let Some(c) = self.chars.get_mut(&who.instance) {
                    c.mover.on_follow_target(f);
                }
            }
            N3::Misc(Misc::ToClientQuit) => {
                self.chars.remove(&who.instance);
            }
            _ => {}
        }
    }

    /// Advances the dynels and hands the visible ones to the renderer. `cam` = camera position in scene space, `fwd` = its view direction.
    pub fn update(&mut self, dt: f32, cam: [f32; 3], fwd: [f32; 3], host: &mut Host) {
        let Some(dir) = self.dir.clone() else { return };
        let worker = self.worker.get_or_insert_with(|| Worker::start(dir));
        while let Ok(r) = worker.rx.try_recv() {
            match r.result {
                Ok(built) => {
                    self.models.insert(r.key, Model::Ready { built, uploaded: false });
                }
                Err(e) => {
                    eprintln!("dynels: model {:016x}: {e}", r.key);
                    self.models.insert(r.key, Model::Failed);
                }
            }
        }
        let own = self.own;
        for (id, c) in &self.chars {
            if *id != own && self.asked.insert(c.key) {
                self.models.insert(c.key, Model::Loading);
                let _ = worker.tx.send(Req { key: c.key, look: c.look.clone() });
            }
        }
        for (id, c) in &mut self.chars {
            c.pose = c.mover.advance(dt);
            if *id == own {
                continue;
            }
            let Some(Model::Ready { built, uploaded }) = self.models.get_mut(&c.key) else { continue };
            if !c.features_set {
                if let Some(f) = built.features {
                    c.mover.set_features(f);
                }
                c.features_set = true;
            }
            if !*uploaded {
                host.actor_models.push((c.key, built.rig.model().clone()));
                *uploaded = true;
            }
            let p = scene_pos(c.pose.pos);
            let (dx, dz) = (p[0] - cam[0], p[2] - cam[2]);
            let dist = (dx * dx + (p[1] - cam[1]).powi(2) + dz * dz).sqrt();
            if dist > DRAW_DISTANCE {
                c.submitted = false;
                continue;
            }
            // clip: death / attack override the movement state; walking and running scale the clip with the real speed
            let state = c.pose.anim;
            let (key, clip, rate) = match c.special {
                Special::Die => match built.clips.get(&DIE_KEY) {
                    Some(a) => (DIE_KEY, Some(a), 1.0),
                    None => (0x78, clip_of(built, state).map(|x| x.1), 1.0),
                },
                Special::Attack => match built.clips.get(&ATTACK_KEY) {
                    Some(a) => (ATTACK_KEY, Some(a), 1.0),
                    None => (0x78, clip_of(built, state).map(|x| x.1), 1.0),
                },
                Special::None => {
                    let (id, a) = clip_of(built, state).map_or((0x78, None), |(i, a)| (i, Some(a)));
                    let mode = c.mover.status().mode;
                    let nominal = match state {
                        AnimState::Walk | AnimState::WalkBack => max_speed(Mode::Walk, state == AnimState::WalkBack, c.mover.skill()),
                        AnimState::Run | AnimState::RunBack => max_speed(Mode::Run, state == AnimState::RunBack, c.mover.skill()),
                        _ => 0.0,
                    };
                    let _ = mode;
                    let rate = if nominal > 0.0 { (c.pose.speed / nominal).clamp(0.3, 2.0) } else { 1.0 };
                    (id, a, rate)
                }
            };
            if key != c.anim {
                c.anim = key;
                c.clip_ms = 0.0;
                c.pose_in = 0.0;
            }
            c.clip_ms += dt * 1000.0 * rate;
            if let Some(a) = clip.filter(|_| c.special != Special::None) {
                // one-shot clips: Attack returns to the movement state at the end, Die holds the last frame
                if c.clip_ms >= a.duration {
                    if c.special == Special::Attack {
                        c.special = Special::None;
                        c.clip_ms = 0.0;
                    } else {
                        c.clip_ms = a.duration - 1.0;
                    }
                }
            }
            c.pose_in -= dt;
            // skinning is the cost: skip it behind the camera, slow it down with distance
            let facing = dx * fwd[0] + dz * fwd[2] > -0.3 * dist;
            let animating = c.special != Special::Die || c.pose_in < -1.0e9;
            let skin = if (!c.submitted || (c.pose_in <= 0.0 && animating)) && (facing || dist < 6.0) {
                c.pose_in = if dist < 30.0 { 1.0 / 25.0 } else if dist < 80.0 { 0.1 } else { 0.25 };
                Some(built.rig.pose(clip.map(|a| (&**a, c.clip_ms))))
            } else {
                None
            };
            c.submitted = true;
            let skin = skin.map(|(v, parts)| {
                c.parts = parts;
                v
            });
            let (s, cs) = scene_yaw(c.pose.yaw).sin_cos();
            let k = c.scale;
            let transform = [[cs * k, 0.0, -s * k, 0.0], [0.0, k, 0.0, 0.0], [s * k, 0.0, cs * k, 0.0], [p[0], p[1], p[2], 1.0]];
            host.actors.push(ActorFrame { id: *id as u32, model: c.key, transform, parts: c.parts.clone(), skin, always: false });
        }
    }

    /// `ShowAllNames` tags (docs/zone/motion.md §6): the name of every dynel within [`NAME_TAG_RADIUS`] of the player, projected
    /// above its head. `own_pos` = the player's scene position, `cam` the render camera, `size` the window in GUI pixels.
    pub fn name_tags(&mut self, gui: &mut Gui, cam: &Camera, size: (u32, u32), own_pos: [f32; 3], list: &mut DrawList) {
        if !self.show_all_names {
            return;
        }
        let (w, h) = (size.0 as f32, size.1.max(1) as f32);
        let (fwd, right, up) = (cam.forward(), cam.right(), cam.up());
        let tan = (self.lens.vertical_fov(w / h) * 0.5).tan();
        for (id, c) in &self.chars {
            if *id == self.own {
                continue;
            }
            let Some(Model::Ready { built, .. }) = self.models.get(&c.key) else { continue };
            let p = scene_pos(c.pose.pos);
            if (0..3).map(|i| (p[i] - own_pos[i]).powi(2)).sum::<f32>() > NAME_TAG_RADIUS * NAME_TAG_RADIUS {
                continue;
            }
            let d = ao_render::Vec3::new(p[0], p[1] + built.tag_height * c.scale, p[2]) - cam.pos;
            let z = d.dot(fwd);
            if z < 0.3 {
                continue;
            }
            let (x, y) = ((0.5 + 0.5 * d.dot(right) / (z * tan * w / h)) * w, (0.5 - 0.5 * d.dot(up) / (z * tan)) * h);
            let tag = name_tag(&NameTagInput {
                name: &c.name,
                is_npc: c.npc,
                flags: c.flags,
                features: INVALID_STAT,
                visual_flags: c.visual_flags,
                side: c.side as i32,
                ..Default::default()
            });
            let [r, g, b, _] = name_tag_color(&tag);
            let (tw, fh) = (gui.text_width(FontId::Shell, &tag.text), gui.font_height(FontId::Shell));
            gui.text_cmds(FontId::Shell, &tag.text, x as i32 - tw / 2, y as i32 - fh, u32::from_be_bytes([0, r, g, b]), 1.0, list);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::play::zone::{scene_forward, Zone};
    use ao_net::frame::Frame;

    fn frames(rec: &str) -> Vec<Frame> {
        rec.lines()
            .filter_map(|l| {
                let mut p = l.split(' ');
                let (_, dir, hex) = (p.next()?, p.next()?, p.next()?);
                let b: Vec<u8> = (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap()).collect();
                (dir == "<").then(|| Frame::decode_with(&b, false).ok().flatten().map(|(f, _)| f)).flatten()
            })
            .collect()
    }

    fn client() -> Option<PathBuf> {
        let d = PathBuf::from(std::env::var_os("HOME")?).join("Games/ProjectRubiKa/client");
        d.join("cd_image/rdb.db").exists().then_some(d)
    }

    #[test]
    fn captured_dynels_become_actors() {
        let mut z = Zone::new(25988);
        for f in frames(include_str!("../../../../docs/captures/zone_ithaca.rec")) {
            z.on_frame(&f);
        }
        assert!(z.world.chars.len() > 50, "{}", z.world.chars.len());
        let looks: HashSet<u64> = z.world.chars.values().map(|c| c.key).collect();
        assert!(looks.len() < z.world.chars.len(), "NPCs of one species share a model");
        let Some(dir) = client() else { return };
        z.world.start(dir.clone(), 25988);
        let own = z.own().unwrap().clone();
        let eye = scene_pos(own.pos);
        let fwd = scene_forward(own.yaw.unwrap_or(0.0));
        let mut host = Host::headless();
        let mut models = vec![];
        let mut actors = vec![];
        for _ in 0..600 {
            z.world.update(0.05, eye, fwd, &mut host);
            models.append(&mut host.actor_models);
            actors = std::mem::take(&mut host.actors);
            let pending = z.world.chars.values().any(|c| matches!(z.world.models.get(&c.key), None | Some(Model::Loading)));
            if !pending {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let failed = z.world.models.values().filter(|m| matches!(m, Model::Failed)).count();
        eprintln!("{} models ready, {failed} failed, {} actors", models.len(), actors.len());
        assert!(!actors.is_empty());
        if let Ok(out) = std::env::var("AOMAC_DYNEL_SHOT") {
            let scene = ao_formats::playfield::load_playfield_at(&RecordStore::open(&dir).unwrap(), &dir, 4582, ao_formats::playfield::DEFAULT_DAY_TIME).unwrap();
            let at = [eye[0] + fwd[0], eye[1] + 1.7 + fwd[1] - 0.2, eye[2] + fwd[2]];
            ao_render::render_to_png_actors(&scene, &models, actors, [eye[0], eye[1] + 1.7, eye[2]], at, 1200, 700, std::path::Path::new(&out), 0.0).unwrap();
        }
    }
}
