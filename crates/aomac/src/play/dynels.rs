//! Other players, NPCs and monsters in the zone, drawn as renderer actors (`ao_scene::ActorFrame`).
//!
//! `Dynels::on_message` keeps one [`Char`] per `SimpleCharFullUpdate` (removed on `n3ToClientQuit`) with the client's movement
//! simulation of it ([`ao_net::n3::motion::Mover`]); a worker thread builds the textured, poseable model of every distinct
//! appearance ([`Look`]) once ([`ao_formats::character::actor::ActorRig`]); `update` advances the characters, picks their clip
//! from the movement state, skins the visible ones at a distance dependent rate and pushes them to `Host::actors`.
//! Evidence for the field meanings: docs/zone/dynel.md §1, docs/zone/npc.md, docs/zone/motion.md.

use anyhow::Context;
use ao_formats::character::actor::{npc_part_textures, ActorAssets, ActorRig, PlayerLook};
use ao_formats::dynel_visual::{blob_stats, corpse_visual, default_mesh, effective_stats, item_template, placed_dynels, static_instance, visual, PlacedDynel};
use ao_formats::character::{load_cat_mesh, CatAnim, ClothPart, Equipment, NpcRecord, TextureOverride, CHAR_MESH_TYPE};
use ao_gui::{DrawList, FontId, Gui};
use ao_net::n3::dynel::{Dynel, SimpleCharFullUpdate};
use ao_net::n3::misc::Misc;
use ao_net::n3::motion::{max_speed, AnimState, Mode, Mover, STAT_HEALTH};
use ao_net::n3::nametag::{name_tag, name_tag_color, NameTagInput, INVALID_STAT};
use ao_net::n3::world::World;
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
pub struct CharLook {
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

impl CharLook {
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
}

/// The look of a corpse (`CorpseFullUpdate`, docs/zone/static.md §5).
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct CorpseLook {
    /// rdb 1010002 model of the dead character.
    pub cat_mesh: u32,
    pub head: Option<u32>,
    pub breed: i32,
    pub sex: i32,
    pub race: i32,
    /// `(part, rdb 1010004 texture)`.
    pub cloth: Vec<(i32, u32)>,
    pub textures: Vec<(String, u32)>,
}

/// Everything that decides what a model looks like; equal looks share one model.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub enum Look {
    Char(CharLook),
    Corpse(CorpseLook),
    /// Item-family dynel (vending machine, door, terminal, ...): the `StaticInstance` template (rdb 1000020) and the stats of the
    /// message / placement blob on top of it; the mesh comes from `Mesh` / `CATMesh` (docs/zone/static.md §2).
    Item { template: Option<u32>, stats: Vec<(u32, i32)> },
}

impl Look {
    pub fn key(&self) -> u64 {
        let mut h = DefaultHasher::new();
        self.hash(&mut h);
        h.finish()
    }
}

/// A built model: the scene to upload, the rig (pose source) of CAT models, the clips of its set by client animation id
/// (`AnimState::anim_id`, plus [`DIE_KEY`] / [`ATTACK_KEY`] / the death keys), the NPC record's `Features` stat.
pub struct Built {
    pub model: ao_scene::Scene,
    pub rig: Option<Arc<ActorRig>>,
    pub clips: HashMap<u32, Arc<CatAnim>>,
    pub features: Option<i32>,
    /// Name tag anchor height, metres above the feet at scale 1.
    pub tag_height: f32,
    /// Corpses and static CAT props: the fixed pose (body vertices, mount transforms).
    pub held: Option<(Vec<ao_scene::Vertex>, Vec<[[f32; 4]; 4]>)>,
    /// `Flags` bit 0 of an item-family dynel (`DisableVisibility` otherwise).
    pub visible: bool,
}

enum Model {
    Loading,
    Failed,
    Ready { built: Built, uploaded: bool },
}

enum Req {
    Model { key: u64, look: Look },
    /// The dynels the playfield places by itself (rdb 1000026).
    Placed(u32),
}

enum Resp {
    Model { key: u64, result: Result<Built, String> },
    Placed(u32, Vec<PlacedDynel>),
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
                let resp = match r {
                    Req::Model { key, look } => Resp::Model { key, result: build(&store, &mut assets, &look).map_err(|e| format!("{e:#}")) },
                    Req::Placed(pf) => Resp::Placed(
                        pf,
                        placed_dynels(&store, pf).unwrap_or_else(|e| {
                            eprintln!("dynels: placed dynels of {pf}: {e:#}");
                            vec![]
                        }),
                    ),
                };
                if resp_tx.send(resp).is_err() {
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

/// `AbstractAnimID_e` of the death clips a `CharacterAction` 99 can ask for (docs/zone/combat-anim.md §5.1).
const DEATH_KEYS: &[u32] = &[500, 501, 502, 503, 504, 505, 0xcc];

fn static_model(store: &RecordStore, mesh: u32, override_texture: Option<u32>) -> anyhow::Result<ao_scene::Scene> {
    let mut scene = ao_scene::Scene::default();
    let idx = ao_formats::mesh::decode_mesh_into(store, mesh, &mut scene)?.with_context(|| format!("no mesh {mesh}"))?;
    anyhow::ensure!(idx == 0, "static model mesh index {idx}");
    if let Some(t) = override_texture {
        // `SetOverrideTexture`: every material of the mesh shows this texture
        let key = ao_scene::TextureKey { rdb_type: 1010004, id: t };
        if let Some(tex) = ao_formats::texture::load_texture(store, key)? {
            scene.textures.insert(key, tex);
            scene.meshes[0].submeshes.iter_mut().for_each(|s| s.texture = Some(key));
        }
    }
    scene.instances.clear();
    Ok(scene)
}

fn plain(model: ao_scene::Scene, visible: bool) -> Built {
    Built { model, rig: None, clips: HashMap::new(), features: None, tag_height: 1.0, held: None, visible }
}

fn build(store: &RecordStore, assets: &mut ActorAssets, look: &Look) -> anyhow::Result<Built> {
    match look {
        Look::Char(c) => build_char(store, assets, c),
        Look::Corpse(c) => build_corpse(store, assets, c),
        Look::Item { template, stats } => {
            let tpl = template.map(|t| item_template(store, t)).transpose()?.flatten();
            let v = visual(&effective_stats(tpl.as_ref(), stats), default_mesh(&assets.names)?);
            match (v.cat_mesh, v.mesh) {
                (Some(cat), _) => {
                    let rig = ActorRig::new(store, cat, None, &Default::default(), &[])?;
                    let held = rig.pose(None);
                    Ok(Built { model: rig.model().clone(), rig: Some(Arc::new(rig)), held: Some(held), ..plain(Default::default(), v.visible) })
                }
                (None, Some(mesh)) => Ok(plain(static_model(store, mesh, v.override_texture)?, v.visible)),
                (None, None) => anyhow::bail!("item dynel without a model"),
            }
        }
    }
}

/// A corpse: the dead character's model in the pose of its death clip's last frame **[GUESS]** (docs/zone/static.md §5: the
/// original's corpse pose was not found).
fn build_corpse(store: &RecordStore, assets: &mut ActorAssets, c: &CorpseLook) -> anyhow::Result<Built> {
    let player = ao_formats::screens::wire_breed_sex(c.breed, c.sex).ok().and_then(|(b, g)| {
        let model = ao_formats::character::player_model_build(store, b, g, 1).ok()?;
        (model == c.cat_mesh).then_some((b, g))
    });
    let rig = match player {
        Some((breed, gender)) => {
            let mut equipment = Equipment::default();
            for &(part, tex) in &c.cloth {
                if let Some(p) = ClothPart::ALL.get(part as usize).filter(|_| tex > 0) {
                    equipment.wear(*p, tex);
                }
            }
            let skin = match c.race {
                2 => ao_formats::character::Skin::African,
                3 => ao_formats::character::Skin::Asian,
                _ => ao_formats::character::Skin::Caucasian,
            };
            ActorRig::player(store, assets, &PlayerLook { breed, gender, skin, build: 1, head: c.head, equipment }, &[])?
        }
        None => {
            let cat = load_cat_mesh(store, CHAR_MESH_TYPE, c.cat_mesh)?;
            let list: Vec<TextureOverride> = c.textures.iter().map(|(m, t)| TextureOverride { material: m, texture: *t, env_texture: 0, alpha_mode: 0 }).collect();
            let cloth: Vec<(ClothPart, u32)> = c.cloth.iter().filter(|c| c.1 > 0).filter_map(|&(p, t)| Some((*ClothPart::ALL.get(p as usize)?, t))).collect();
            ActorRig::new(store, c.cat_mesh, c.head, &npc_part_textures(store, &cat, &list, &cloth), &[])?
        }
    };
    let die = assets.clips(store, rig.model_id)?.iter().find(|n| n.0.contains("die")).map(|n| n.1);
    let clip = die.map(|id| assets.anim(store, id)).transpose()?;
    let held = rig.pose(clip.as_deref().map(|a| (a, a.duration - 1.0)));
    Ok(Built { model: rig.model().clone(), rig: Some(Arc::new(rig)), held: Some(held), ..plain(Default::default(), true) })
}

fn build_char(store: &RecordStore, assets: &mut ActorAssets, look: &CharLook) -> anyhow::Result<Built> {
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
            let keys = STATES.iter().filter_map(|s| s.anim_id()).chain([DIE_KEY, ATTACK_KEY]).chain(DEATH_KEYS.iter().copied());
            for key in keys {
                let id = if key == DIE_KEY || key == ATTACK_KEY || DEATH_KEYS.contains(&key) { ao_formats::character::anim_key(rec, key) } else { rec.anim_variants(key).first().copied() };
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
    let features = rec.as_ref().and_then(|r| r.stat(ao_net::n3::motion::STAT_FEATURES as u32));
    Ok(Built { model: rig.model().clone(), rig: Some(Arc::new(rig)), clips, features, tag_height, ..plain(Default::default(), true) })
}

fn named(store: &RecordStore, assets: &mut ActorAssets, model: u32, name: &str) -> anyhow::Result<Option<Arc<CatAnim>>> {
    let id = assets.clips(store, model)?.iter().find(|c| c.0 == name).map(|c| c.1);
    id.map(|id| assets.anim(store, id)).transpose()
}

/// An NPC: the record's model (`MonsterData` -> rdb 1040023 `Mesh`), `textures[]` replacing the part textures (`SetCATTextures`),
/// worn `cloth[]` composited over the part's texture like the player equipment, head and attachment meshes.
fn npc_rig(store: &RecordStore, look: &CharLook, attachments: &[(u8, u32)]) -> anyhow::Result<(ActorRig, NpcRecord)> {
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
    /// The death clip (client animation id) plays once and holds its last frame.
    Die(u32),
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

/// A corpse, vending machine, door, ... (everything that is not a `SimpleChar`): a fixed model at a fixed place.
struct Prop {
    /// `ActorFrame::id`, disjoint from the character instance ids.
    id: u32,
    key: u64,
    pos: [f32; 3],
    /// Server heading.
    yaw: f32,
    scale: f32,
    submitted: bool,
}

/// First `ActorFrame::id` of props (character instances stay far below).
const PROP_ID_BASE: u32 = 0x4000_0000;

pub struct Dynels {
    dir: Option<PathBuf>,
    worker: Option<Worker>,
    own: i32,
    pub chars: HashMap<i32, Char>,
    /// Non-character dynels by `(identity kind, instance)`.
    props: HashMap<(i32, i32), Prop>,
    next_prop: u32,
    /// Looks asked for before the worker existed.
    pending: Vec<Look>,
    /// The playfield whose placed dynels (rdb 1000026) are still to be requested.
    want_placed: Option<u32>,
    playfield: Option<u32>,
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
            props: HashMap::new(),
            next_prop: PROP_ID_BASE,
            pending: vec![],
            want_placed: None,
            playfield: None,
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

/// `MonsterScale` (stat 360) percent -> factor, 1 when absent.
fn stat_scale(stats: &[(u32, i32)]) -> f32 {
    stats.iter().find(|s| s.0 == 360).map_or(1.0, |s| s.1 as f32 / 100.0)
}

/// Server quaternion `(x, y, z, w)` -> heading about Y.
fn quat_yaw(q: &[f32; 4]) -> f32 {
    ao_net::n3::dynel::yaw(q)
}

impl Dynels {
    /// Starts the model builder for the client at `dir`; `own` = the player's own instance id (drawn by the avatar code).
    pub fn start(&mut self, dir: PathBuf, own: i32) {
        self.own = own;
        self.dir = Some(dir);
    }

    /// Forgets every dynel (a playfield change, `Zone::reset_world`).
    pub fn clear(&mut self) {
        self.chars.clear();
        self.props.clear();
    }

    /// A new playfield: every dynel of the old one is gone (models stay built); the dynels the playfield places itself
    /// (doors, terminals, ...: rdb 1000026, `CreateRDBDynels`) are created for `id`.
    pub fn on_playfield(&mut self, id: u32) {
        self.clear();
        self.playfield = Some(id);
        self.want_placed = Some(id);
    }

    /// The attack clip of `id` plays once (combat messages).
    pub fn attack(&mut self, id: i32) {
        if let Some(c) = self.chars.get_mut(&id).filter(|c| c.special == Special::None) {
            c.special = Special::Attack;
            c.clip_ms = 0.0;
        }
    }

    /// `id` dies: the death clip `anim` (client animation id, `CharacterAction` 99's `identity_b.instance`; any other value =
    /// the generic death) plays once and holds.
    pub fn die(&mut self, id: i32, anim: u32) {
        if let Some(c) = self.chars.get_mut(&id) {
            c.special = Special::Die(anim);
            c.clip_ms = 0.0;
        }
    }

    fn add_prop(&mut self, kind: i32, instance: i32, look: Look, pos: [f32; 3], rot: Option<[f32; 4]>, scale: f32) {
        let (id, replace) = self.props.get(&(kind, instance)).map_or((self.next_prop, false), |p| (p.id, true));
        if !replace {
            self.next_prop += 1;
        }
        self.props.insert((kind, instance), Prop { id, key: look.key(), pos, yaw: rot.map_or(0.0, |q| quat_yaw(&q)), scale, submitted: false });
        self.pending.push(look);
    }

    pub fn on_message(&mut self, m: &Message) {
        let who = m.header.target;
        match &m.body {
            N3::World(World::VendingMachine(v)) => {
                let stats = v.base.stats.iter().map(|&(i, x)| (i, x)).collect::<Vec<_>>();
                let template = static_instance(&stats);
                let scale = stat_scale(&stats);
                if let Some(pos) = v.base.position {
                    self.add_prop(who.kind, who.instance, Look::Item { template, stats }, pos, v.base.rotation, scale);
                }
            }
            N3::World(World::Corpse(c)) => {
                let stats: Vec<(u32, i32)> = c.base.stats.clone();
                let cloth: Vec<(i32, i32, i32)> = c.cloth.iter().map(|w| (w.part(), w.texture, w.extra.map_or(0, |e| e.0))).collect();
                let textures: Vec<(&str, i32)> = c.textures.iter().map(|t| (t.material.as_str(), t.texture)).collect();
                match corpse_visual(&stats, &cloth, &textures) {
                    Ok(v) => {
                        let look = Look::Corpse(CorpseLook {
                            cat_mesh: v.cat_mesh,
                            head: v.head_mesh,
                            breed: v.breed,
                            sex: v.sex,
                            race: v.race,
                            cloth: v.cloth.iter().map(|l| (l.part, l.texture)).collect(),
                            textures: v.textures,
                        });
                        if let Some(pos) = c.base.position {
                            self.add_prop(who.kind, who.instance, look, pos, c.base.rotation, v.scale);
                        }
                    }
                    Err(e) => eprintln!("dynels: corpse {}: {e:#}", who.instance),
                }
            }
            N3::World(World::CharacterAction(a)) if who.kind == CHAR_KIND && a.action == 99 => self.die(who.instance, a.identity_b.instance as u32),
            _ if who.kind != CHAR_KIND => {
                if matches!(m.body, N3::Misc(Misc::ToClientQuit)) {
                    self.props.remove(&(who.kind, who.instance));
                }
            }
            N3::Dynel(Dynel::SimpleCharFullUpdate(u)) => {
                let look = CharLook::from_update(u);
                let look = Look::Char(look);
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
                        special: if u.max_health > 0 && u.health <= 0 { Special::Die(DIE_KEY) } else { Special::None },
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
        let worker = self.worker.take().unwrap_or_else(|| Worker::start(dir));
        if let Some(pf) = self.want_placed.take() {
            let _ = worker.tx.send(Req::Placed(pf));
        }
        let mut placed = vec![];
        while let Ok(r) = worker.rx.try_recv() {
            match r {
                Resp::Model { key, result: Ok(built) } => {
                    self.models.insert(key, Model::Ready { built, uploaded: false });
                }
                Resp::Model { key, result: Err(e) } => {
                    eprintln!("dynels: model {key:016x}: {e}");
                    self.models.insert(key, Model::Failed);
                }
                Resp::Placed(pf, list) => placed.push((pf, list)),
            }
        }
        let current = self.playfield;
        for (pf, list) in placed.into_iter().filter(|p| Some(p.0) == current) {
            eprintln!("dynels: playfield {pf} places {} dynels", list.len());
            for d in list {
                let stats = blob_stats(&d.blob).unwrap_or_default();
                let scale = stat_scale(&stats);
                let look = Look::Item { template: Some(d.template).filter(|&t| t != 0), stats };
                if d.template != 0 {
                    self.add_prop(d.kind as i32, d.instance as i32, look, d.position, Some(d.rotation), scale);
                }
            }
        }
        let own = self.own;
        for (id, c) in &self.chars {
            if *id != own {
                self.pending.push(c.look.clone());
            }
        }
        for look in std::mem::take(&mut self.pending) {
            let key = look.key();
            if self.asked.insert(key) {
                self.models.insert(key, Model::Loading);
                let _ = worker.tx.send(Req::Model { key, look });
            }
        }
        for p in self.props.values_mut() {
            let Some(Model::Ready { built, uploaded }) = self.models.get_mut(&p.key) else { continue };
            if !built.visible {
                continue;
            }
            if !*uploaded {
                host.actor_models.push((p.key, built.model.clone()));
                *uploaded = true;
            }
            let q = scene_pos(p.pos);
            let d2: f32 = (0..3).map(|i| (q[i] - cam[i]).powi(2)).sum();
            if d2 > DRAW_DISTANCE * DRAW_DISTANCE {
                p.submitted = false;
                continue;
            }
            let (s, cs) = scene_yaw(p.yaw).sin_cos();
            let k = p.scale;
            let transform = [[cs * k, 0.0, -s * k, 0.0], [0.0, k, 0.0, 0.0], [s * k, 0.0, cs * k, 0.0], [q[0], q[1], q[2], 1.0]];
            // the renderer forgets actors that were not submitted: the held pose goes out again after an absence
            let skin = built.held.as_ref().filter(|_| !p.submitted).map(|h| h.0.clone());
            p.submitted = true;
            let parts = built.held.as_ref().map_or(vec![], |h| h.1.clone());
            host.actors.push(ActorFrame { id: p.id, model: p.key, transform, parts, skin, always: false });
        }
        for (id, c) in &mut self.chars {
            c.pose = c.mover.advance(dt);
            if *id == own {
                continue;
            }
            let Some(Model::Ready { built, uploaded }) = self.models.get_mut(&c.key) else { continue };
            let Some(rig) = built.rig.clone() else { continue };
            if !c.features_set {
                if let Some(f) = built.features {
                    c.mover.set_features(f);
                }
                c.features_set = true;
            }
            if !*uploaded {
                host.actor_models.push((c.key, built.model.clone()));
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
                Special::Die(k) => match built.clips.get(&k).or_else(|| built.clips.get(&DIE_KEY)) {
                    Some(a) => (k, Some(a), 1.0),
                    None => (0x78, clip_of(built, state).map(|x| x.1), 1.0),
                },
                Special::Attack => match built.clips.get(&ATTACK_KEY) {
                    Some(a) => (ATTACK_KEY, Some(a), 1.0),
                    None => (0x78, clip_of(built, state).map(|x| x.1), 1.0),
                },
                Special::None => {
                    let (id, a) = clip_of(built, state).map_or((0x78, None), |(i, a)| (i, Some(a)));
                    let nominal = match state {
                        AnimState::Walk | AnimState::WalkBack => max_speed(Mode::Walk, state == AnimState::WalkBack, c.mover.skill()),
                        AnimState::Run | AnimState::RunBack => max_speed(Mode::Run, state == AnimState::RunBack, c.mover.skill()),
                        _ => 0.0,
                    };
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
            let dead = matches!(c.special, Special::Die(_));
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
            // skinning is the cost: skip it behind the camera, slow it down with distance, never repeat a held frame
            let facing = dx * fwd[0] + dz * fwd[2] > -0.3 * dist;
            let held = dead && c.submitted && clip.is_some_and(|a| c.clip_ms >= a.duration - 1.0);
            let skin = if (!c.submitted || (c.pose_in <= 0.0 && !held)) && (facing || dist < 6.0) {
                c.pose_in = if dist < 30.0 { 1.0 / 25.0 } else if dist < 80.0 { 0.1 } else { 0.25 };
                Some(rig.pose(clip.map(|a| (&**a, c.clip_ms))))
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
        self.worker = Some(worker);
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
        // 7 corpses arrive, the server removes some of them again within the capture
        assert!(z.world.props.keys().filter(|k| k.0 == 0xC76A).count() >= 2, "corpses");
        assert_eq!(z.world.props.keys().filter(|k| k.0 == 0xC75B).count(), 1, "vending machine");
        let looks: HashSet<u64> = z.world.chars.values().map(|c| c.key).collect();
        let keys = |z: &Zone| z.world.chars.values().map(|c| c.key).chain(z.world.props.values().map(|p| p.key)).collect::<Vec<_>>();
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
            let pending = keys(&z).iter().any(|k| matches!(z.world.models.get(k), None | Some(Model::Loading))) || z.world.want_placed.is_some() || z.world.props.len() < 12;
            if !pending {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let failed = z.world.models.values().filter(|m| matches!(m, Model::Failed)).count();
        eprintln!("{} models ready, {failed} failed, {} actors, {} props", models.len(), actors.len(), z.world.props.len());
        assert!(!actors.is_empty());
        if let Ok(out) = std::env::var("AOMAC_DYNEL_SHOT") {
            let scene = ao_formats::playfield::load_playfield_at(&RecordStore::open(&dir).unwrap(), &dir, 4582, ao_formats::playfield::DEFAULT_DAY_TIME).unwrap();
            // AOMAC_DYNEL_LOOK=<kind hex like c76a | npc | player>: camera 4 m from the first such dynel instead of the player's view
            let (mut cam, mut at) = ([eye[0], eye[1] + 1.7, eye[2]], [eye[0] + fwd[0], eye[1] + 1.5 + fwd[1], eye[2] + fwd[2]]);
            if let Ok(what) = std::env::var("AOMAC_DYNEL_LOOK") {
                let target = match what.as_str() {
                    "npc" => z.world.chars.values().find(|c| c.npc).map(|c| c.pose.pos),
                    "player" => z.world.chars.values().find(|c| !c.npc && c.name != "Testy").map(|c| c.pose.pos),
                    k => z.world.props.iter().find(|(key, _)| format!("{:x}", key.0) == k).map(|(_, p)| p.pos),
                };
                let t = scene_pos(target.expect("no such dynel"));
                cam = [t[0] + 2.5, t[1] + 1.8, t[2] + 2.5];
                at = [t[0], t[1] + 0.8, t[2]];
            }
            ao_render::render_to_png_actors(&scene, &models, actors, cam, at, 1200, 700, std::path::Path::new(&out), 0.0).unwrap();
        }
    }
}
