//! Other players, NPCs and monsters in the zone, drawn as renderer actors (`ao_scene::ActorFrame`).
//!
//! `Dynels::on_message` keeps one [`Char`] per `SimpleCharFullUpdate` (removed on `n3ToClientQuit`) with the client's movement
//! simulation of it ([`ao_net::n3::motion::Mover`]); a worker thread builds the textured, poseable model of every distinct
//! appearance ([`Look`]) once ([`ao_formats::character::actor::ActorRig`]); `update` advances the characters, picks their clip
//! from the movement state, skins the visible ones at a distance dependent rate and pushes them to `Host::actors`.
//! Evidence for the field meanings: docs/zone/dynel.md §1, docs/zone/npc.md, docs/zone/motion.md.

use anyhow::Context;
use ao_formats::character::actor::{attractor_list, npc_part_layers, npc_part_textures, ActorAssets, ActorRig, PlayerLook};
use ao_formats::dynel_visual::{blob_stats, corpse_visual, default_mesh, effective_stats, item_template, placed_dynels, static_instance, visual, PlacedDynel};
use ao_formats::character::{load_cat_mesh, CatAnim, ClothPart, CrtRand, Equipment, NpcRecord, TextureOverride, CHAR_MESH_TYPE};
use ao_gui::Gui;
use ao_net::n3::dynel::{Dynel, SimpleCharFullUpdate};
use ao_net::n3::misc::Misc;
use ao_net::n3::motion::{max_speed, AnimState, Mode, Mover, STAT_HEALTH};
use ao_net::n3::nametag::{health_bar_fill_px, name_tag, nametag_listed, selection_indicator_exists, tag_anchor_visible, IndicatorKind, NameTagInput, INVALID_STAT};
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

use super::combat::anim as canim;
use super::dynels_doors::{Cmd, GameSound, ItemRig, PropAnim};
use super::tags::{Indicator, Listing, Tag, TagLayer};
use super::zone::{scene_pos, scene_yaw};

/// Identity kind of character / NPC dynels (`SimpleChar_t`).
const CHAR_KIND: i32 = 0xC350;
/// Props (non-character dynels) farther than this (metres) are not drawn (about the fog distance of the outdoor playfields; a guess:
/// the client draws them up to the far plane). Characters use `Dynels::char_view_distance`.
pub const DRAW_DISTANCE: f32 = 250.0;
/// `ShowAllNames` name tags exist for dynels within this radius of the player (`GetDynelsInVicinity`, docs/zone/motion.md §6).
pub const NAME_TAG_RADIUS: f32 = 30.0;
/// NPC record key of the generic death clip and of the first unarmed attack ([`ao_formats::character::NpcAnim`]).
const DIE_KEY: u32 = 6000;
const ATTACK_KEY: u32 = canim::UNARMED_RSWING as u32;
/// Weapon item stat `AnimSet` (0x161): selects the stance clips (docs/zone/combat-anim.md §3.1).
const STAT_ANIM_SET: u32 = 353;
/// Weapon item stat `ItemDelay` (0x126, centiseconds): the swing is sped up to land within it (`FUN_1006a239`).
const STAT_ITEM_DELAY: u32 = 294;
/// Weapon stance clips loaded with every character model: idle (list 0x10) of the anim sets 0/1/3/6/7/8 and walk/run of a 2H stance.
const STANCE_IDS: &[u16] = &[0x3f3, 0x3e9, 0x3fd, 0xb6, 0xcb, 0x424, 0x421, 0x422];

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
    /// `TextureData_t`: (material name, rdb 1010004 texture, env texture [layer 3], alpha mode).
    pub textures: Vec<(String, i32, i32, i32)>,
    /// `ClothData_t` page 0: (part, rdb 1010004 texture).
    pub cloth: Vec<(i32, i32)>,
    /// `AttractorMeshData_t`: (place, rdb 1010001 mesh).
    pub attractors: Vec<(u8, i32)>,
    /// Message flag bit 2 (`SET_DYNEL_800`): `FUN_10077e13` skips the whole attractor block (head included).
    pub skip_attractors: bool,
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
            textures: u.textures.iter().map(|t| (t.material.clone(), t.texture, t.field_24, t.flag)).collect(),
            cloth: u.cloth.iter().filter(|c| c.page == 0).map(|c| (c.part(), c.texture)).collect(),
            attractors: u.attractors.iter().map(|a| (a.place, a.mesh)).collect(),
            skip_attractors: u.flags & ao_net::n3::dynel::flag::SET_DYNEL_800 != 0,
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
    pub clips: HashMap<u32, Vec<Arc<CatAnim>>>,
    pub features: Option<i32>,
    /// Name tag anchor height, metres above the feet at scale 1.
    pub tag_height: f32,
    /// Corpses and static CAT props: the fixed pose (body vertices, mount transforms).
    pub held: Option<HeldPose>,
    /// `Flags` bit 0 of an item-family dynel (`DisableVisibility` otherwise).
    pub visible: bool,
    /// Items whose mesh has node keyframes (doors, vending machines): the animation data (docs/zone/doors.md).
    pub item: Option<ItemRig>,
    /// The stats of an item-family dynel: its template's overlaid by the message's (`N3Msg_DefaultActionOnDynel` reads `Can`, interact.rs).
    pub stats: Vec<(u32, i32)>,
    /// The NPC record's sound multimap (`NpcRecord::sounds`: `AbstractAnimID_e` key -> sound ids; fight keys `combat::anim::npc_sound`).
    pub sounds: Vec<(u32, Vec<u32>)>,
}

/// A skinned pose held for good: vertices and mount transforms.
type HeldPose = (Vec<ao_scene::Vertex>, Vec<[[f32; 4]; 4]>);
/// A weapon to resolve: (holder, hand slot, template, message stats).
type PendingWeapon = (i32, usize, Option<u32>, Vec<(u32, i32)>);

/// A wielded weapon: item stat `AnimSet` and `ItemDelay`.
#[derive(Clone, Copy, Debug)]
struct Wield {
    set: i32,
    delay: i32,
}

enum Model {
    Loading,
    Failed,
    Ready { built: Box<Built>, uploaded: bool },
}

enum Req {
    Model { key: u64, look: Look },
    /// The dynels the playfield places by itself (rdb 1000026).
    Placed(u32),
    /// A wielded weapon: its `AnimSet` (stat 353) from the template record under the message stats.
    Weapon { holder: i32, slot: usize, template: Option<u32>, stats: Vec<(u32, i32)> },
    /// Clip `id` (AbstractAnimID) for the model `key` of a character look.
    Clip { key: u64, look: CharLook, id: u32 },
}

enum Resp {
    Model { key: u64, result: Result<Box<Built>, String> },
    Placed(u32, Vec<PlacedDynel>),
    Weapon { holder: i32, slot: usize, wield: Option<Wield> },
    Clip { key: u64, id: u32, anims: Vec<Arc<CatAnim>> },
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
                    Req::Model { key, look } => Resp::Model { key, result: build(&store, &mut assets, &look).map(Box::new).map_err(|e| format!("{e:#}")) },
                    Req::Clip { key, look, id } => {
                        // the record's variants of the key (`FUN_10010ebe`), every one loaded; the character rolls one when the clip starts
                        let ids: Vec<u32> = if look.npc {
                            NpcRecord::load(&store, look.monster_data as u32).map(|r| ao_formats::character::anim_key_variants(&r, id).to_vec()).unwrap_or_default()
                        } else {
                            canim::resolve_clip(&assets.names, canim::clip_set(look.breed, look.sex), id as u16, false).map(|c| c.0).into_iter().collect()
                        };
                        Resp::Clip { key, id, anims: ids.into_iter().filter_map(|c| assets.anim(&store, c).ok()).collect() }
                    }
                    Req::Weapon { holder, slot, template, stats } => {
                        let tpl = template.and_then(|t| item_template(&store, t).ok().flatten());
                        let stats = effective_stats(tpl.as_ref(), &stats);
                        let stat = |id| stats.iter().find(|s| s.0 == id).map(|s| s.1);
                        Resp::Weapon { holder, slot, wield: stat(STAT_ANIM_SET).map(|set| Wield { set, delay: stat(STAT_ITEM_DELAY).unwrap_or(0) }) }
                    }
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
    Built { model, rig: None, clips: HashMap::new(), features: None, tag_height: 1.0, held: None, visible, item: None, stats: Vec::new(), sounds: Vec::new() }
}

fn build(store: &RecordStore, assets: &mut ActorAssets, look: &Look) -> anyhow::Result<Built> {
    match look {
        Look::Char(c) => build_char(store, assets, c),
        Look::Corpse(c) => build_corpse(store, assets, c),
        Look::Item { template, stats } => {
            let tpl = template.map(|t| item_template(store, t)).transpose()?.flatten();
            let eff = effective_stats(tpl.as_ref(), stats);
            let v = visual(&eff, default_mesh(&assets.names)?);
            match (v.cat_mesh, v.mesh) {
                (Some(cat), _) => {
                    let rig = ActorRig::new(store, cat, None, &Default::default(), &Default::default(), &[])?;
                    let held = rig.pose(None);
                    Ok(Built { model: rig.model().clone(), rig: Some(Arc::new(rig)), held: Some(held), stats: eff, ..plain(Default::default(), v.visible) })
                }
                (None, Some(mesh)) => {
                    let model = static_model(store, mesh, v.override_texture)?;
                    let item = ItemRig::new(store, mesh, &model, &eff, tpl.map(|t| t.sounds).unwrap_or_default());
                    Ok(Built { item, stats: eff, ..plain(model, v.visible) })
                }
                (None, None) => anyhow::bail!("item dynel without a model"),
            }
        }
    }
}

/// A corpse: the dead character's model in the CAT mesh's own pose. `Corpse_t` never starts an animation (its overrides
/// `FUN_1007e7e2`, `FUN_1007e8e1`, `FUN_1007e622/642` set textures / skin / head / scale only, `VisualCATMesh_t::SetMesh` [DS 0x100728b7] and
/// its async load `FUN_100704b8` create the render mesh without one, the only `SetAnimation` caller is the "play animation"
/// spell 0xCF27 `FUN_100a4dcc` -> `FUN_10010e36`, which plays social ids < 100 only and gets key 0 on every captured corpse),
/// so the model is drawn unanimated = the bind pose (docs/zone/static.md §5).
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
            ActorRig::new(store, c.cat_mesh, c.head, &npc_part_textures(store, &cat, &list, &cloth), &npc_part_layers(&cat, &list), &[])?
        }
    };
    let held = rig.pose(None);
    Ok(Built { model: rig.model().clone(), rig: Some(Arc::new(rig)), held: Some(held), ..plain(Default::default(), true) })
}

fn build_char(store: &RecordStore, assets: &mut ActorAssets, look: &CharLook) -> anyhow::Result<Built> {
    let wire: Vec<(u8, u32)> = look.attractors.iter().filter(|a| a.1 > 0).map(|a| (a.0, a.1 as u32)).collect();
    // `FUN_10077e13`: AddAttractorMesh(0, HeadMesh), ClearAttractors, AddAttractors(wire): the head is the wire list's place 0
    let mut mounted = if look.skip_attractors { vec![] } else { attractor_list(look.head.filter(|&h| h > 0).map(|h| h as u32), &wire) };
    let head = mounted.iter().position(|a| a.0 == 0).map(|i| mounted.remove(i).1);
    let attachments = mounted;
    let (rig, rec) = if look.npc {
        let (rig, rec) = npc_rig(store, look, head, &attachments)?;
        (rig, Some(rec))
    } else {
        let (breed, gender) = ao_formats::screens::wire_breed_sex(look.breed as i32, look.sex as i32)?;
        // `BreedRace_e`: 1 caucasian, 2 african, 3 asian (docs/zone/dynel.md §1.3)
        let skin = match look.race {
            2 => ao_formats::character::Skin::African,
            3 => ao_formats::character::Skin::Asian,
            _ => ao_formats::character::Skin::Caucasian,
        };
        // the race selects the naked skin; the head mesh is the place-0 attractor (`attractor_list`)
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
    let mut clips: HashMap<u32, Vec<Arc<CatAnim>>> = HashMap::new();
    match &rec {
        // NPC: the record's table, keyed by the client animation id (`AbstractAnimID_e`)
        Some(rec) => {
            let keys = STATES.iter().filter_map(|s| s.anim_id()).chain([DIE_KEY, ATTACK_KEY]).chain(DEATH_KEYS.iter().copied());
            for key in keys {
                let ids = if key == DIE_KEY || key == ATTACK_KEY || DEATH_KEYS.contains(&key) { ao_formats::character::anim_key_variants(rec, key) } else { ao_formats::character::holder_variants(rec, key) };
                let anims = ids.iter().map(|&id| assets.anim(store, id)).collect::<anyhow::Result<Vec<_>>>()?;
                let anims: Vec<_> = anims.into_iter().filter(|a| a.signature == sig).collect();
                if !anims.is_empty() {
                    clips.insert(key, anims);
                }
            }
        }
        // player: the model set's clips by name
        None => {
            for s in STATES {
                if let (Some(name), Some(id)) = (s.clip_name(), s.anim_id()) {
                    if let Some(a) = named(store, assets, rig.model_id, name)? {
                        clips.insert(id, vec![a]);
                    }
                }
            }
            for (key, name) in [(DIE_KEY, "die-knees"), (ATTACK_KEY, "unarmed-rswing")] {
                if let Some(a) = named(store, assets, rig.model_id, name)? {
                    clips.insert(key, vec![a]);
                }
            }
        }
    }
    // weapon stance clips: players resolve the id by file name in the model's set, NPCs through their record (parent chain only)
    for &id in STANCE_IDS {
        let ids: Vec<u32> = match &rec {
            Some(rec) => ao_formats::character::holder_variants(rec, id as u32).to_vec(),
            None => canim::resolve_clip(&assets.names, canim::clip_set(look.breed, look.sex), id, false).map(|c| c.0).into_iter().collect(),
        };
        let anims = ids.iter().map(|&c| assets.anim(store, c)).collect::<anyhow::Result<Vec<_>>>()?;
        let anims: Vec<_> = anims.into_iter().filter(|a| a.signature == sig).collect();
        if !anims.is_empty() {
            clips.insert(id as u32, anims);
        }
    }
    let tag_height = rig.indicator_height();
    let features = rec.as_ref().and_then(|r| r.stat(ao_net::n3::motion::STAT_FEATURES as u32));
    let sounds = rec.map(|r| r.sounds).unwrap_or_default();
    Ok(Built { model: rig.model().clone(), rig: Some(Arc::new(rig)), clips, features, tag_height, sounds, ..plain(Default::default(), true) })
}

fn named(store: &RecordStore, assets: &mut ActorAssets, model: u32, name: &str) -> anyhow::Result<Option<Arc<CatAnim>>> {
    let id = assets.clips(store, model)?.iter().find(|c| c.0 == name).map(|c| c.1);
    id.map(|id| assets.anim(store, id)).transpose()
}

/// An NPC: the record's model (`MonsterData` -> rdb 1040023 `Mesh`), `textures[]` replacing the part textures (`SetCATTextures`),
/// worn `cloth[]` composited over the part's texture like the player equipment, `head` (the place-0 attractor) and attachment
/// meshes. The record's own `HeadMesh` stat only selects the naked skin textures (`FUN_10058078`), it mounts nothing.
fn npc_rig(store: &RecordStore, look: &CharLook, head: Option<u32>, attachments: &[(u8, u32)]) -> anyhow::Result<(ActorRig, NpcRecord)> {
    let rec = NpcRecord::load(store, look.monster_data as u32)?;
    let model = rec.mesh().context("NPC record has no mesh")?;
    let cat = load_cat_mesh(store, CHAR_MESH_TYPE, model)?;
    let list: Vec<TextureOverride> = look.textures.iter().map(|(m, t, env, alpha)| TextureOverride { material: m, texture: *t as u32, env_texture: *env as u32, alpha_mode: *alpha as u32 }).collect();
    let cloth: Vec<(ClothPart, u32)> = look.cloth.iter().filter(|c| c.1 > 0).filter_map(|&(p, t)| Some((*ClothPart::ALL.get(p as usize)?, t as u32))).collect();
    let overrides = npc_part_textures(store, &cat, &list, &cloth);
    let rig = ActorRig::new(store, model, head, &overrides, &npc_part_layers(&cat, &list), attachments)?;
    Ok((rig, rec))
}

/// What a character is doing besides moving.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Special {
    None,
    /// The attack clip plays once.
    Attack,
    /// Clip `AbstractAnimID` plays once (emotes, swings).
    Once(u32),
    /// The death clip (client animation id) plays once and holds its last frame.
    Die(u32),
}

/// The clip variant of a character, rolled when its clip starts (`FUN_1004570c`: `rand() % count`, no RNG call for a single
/// value) and kept while it loops: every start of a clip goes through the holder's resolver `FUN_1003c8b7` again (state change
/// `FUN_1006c065` -> `FUN_1006be27`, attack / emote start, revive `FUN_1003ea0d`), loops are infinite (loop count -1).
#[derive(Default)]
struct Roll {
    key: Option<(u32, Option<AnimState>)>,
    variant: usize,
}

impl Roll {
    /// Index of the variant for the clip `key` (anim id, and the movement state for the base clip) among `len`.
    fn pick(&mut self, key: (u32, Option<AnimState>), len: usize, rng: &mut CrtRand) -> usize {
        if self.key != Some(key) {
            self.key = Some(key);
            self.variant = if len > 1 { rng.rand() as usize % len } else { 0 };
        }
        self.variant % len.max(1)
    }
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
    /// Model-space box of the last pose's skinned body vertices (`RCATMesh_t+0x1fc/+0x208`, hud_pick.rs); `None` until posed.
    bounds: Option<([f32; 3], [f32; 3])>,
    roll: Roll,
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
    /// Door / animated item state and the pose the renderer holds (docs/zone/doors.md).
    anim: PropAnim,
    /// Stats set by messages (full update, `StatIIR_t`); they win over the template's ([`Built::stats`]).
    stats: Vec<(u32, i32)>,
}

/// Identity kinds of `Door_t` (`DoorRibosome_t`, docs/zone/static.md §3).
const DOOR_KINDS: [i32; 3] = [0xC748, 0xDAC6, 0xC73A];

/// Stat `Can` (0x1e): bit 0 = can be picked up, bit 3 = can be used (`N3Msg_DefaultActionOnDynel` [GC 0x100291da]).
pub const CAN_STAT: u32 = 0x1e;
/// `Corpse_t`'s constructor [GC 0x1007e652] pre-sets `Can` to 8 (`FUN_10088d80(0x1e, 8)`).
const CORPSE_CAN: i32 = 8;

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
    /// `AnimSet` of the weapon in the right (slot 6) / left (slot 8) hand of a holder.
    wield: HashMap<i32, [Option<Wield>; 2]>,
    /// (swing clip, `ItemDelay`) of the last [`Dynels::pick_swing`] of a character: the clip plays sped up by [`canim::swing_speed_scale`].
    swing_delay: HashMap<i32, (u32, i32)>,
    /// Weapon dynel instance -> (holder, hand index).
    weapons: HashMap<i32, (i32, usize)>,
    pending_clips: Vec<(u64, CharLook, u32, i32)>,
    /// (character, clip) waiting for the worker's answer; started once it is in.
    replay: Vec<(i32, u32)>,
    pending_weapons: Vec<PendingWeapon>,
    /// The playfield whose placed dynels (rdb 1000026) are still to be requested.
    want_placed: Option<u32>,
    playfield: Option<u32>,
    models: HashMap<u64, Model>,
    asked: HashSet<u64>,
    /// Pref `ShowAllNames` (default off, docs/zone/motion.md §6): name tags over every dynel within [`NAME_TAG_RADIUS`].
    pub show_all_names: bool,
    /// `DisplayCharViewDistance` in metres (default 80, `FUN_1001f964` N3 0x1001f964; docs/chat/dvalue.md): characters farther from the
    /// viewer are not drawn (`n3VisualDynel_t::Run`, N3 0x100196bd: squared distance to the controlled dynel on ground playfields).
    pub char_view_distance: f32,
    /// Lens of the playfield scene (projection of the name tags).
    pub lens: Lens,
    /// The tag sprites held by the renderer and the 2 s nametag listing (`play/tags.rs`).
    tags: TagLayer,
    listing: Listing,
    /// The CRT's `rand()` the variant picks consume (`srand(time)` when the zone starts, [GUESS] for the exact call site).
    rng: CrtRand,
    /// `PlayGameSound` calls of doors and characters (fight sounds) since the last [`Dynels::take_sounds`].
    sounds: Vec<GameSound>,
    /// Camera position of the last [`Dynels::update`] (scene space).
    cam: [f32; 3],
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
            wield: HashMap::new(),
            swing_delay: HashMap::new(),
            weapons: HashMap::new(),
            pending_weapons: vec![],
            pending_clips: vec![],
            replay: vec![],
            want_placed: None,
            playfield: None,
            models: HashMap::new(),
            asked: HashSet::new(),
            show_all_names: std::env::var_os("AOMAC_SHOW_ALL_NAMES").is_some(),
            char_view_distance: 80.0,
            lens: Lens::default(),
            tags: TagLayer::default(),
            listing: Listing::default(),
            rng: CrtRand::new(1),
            sounds: vec![],
            cam: [0.0; 3],
        }
    }
}

/// Clip(s) for `state`: its own, else the client's fallback chain, else idle.
fn clip_of(built: &Built, mut state: AnimState) -> Option<(u32, &Vec<Arc<CatAnim>>)> {
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
        self.rng = CrtRand::new(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_secs() as u32));
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

    /// Plays clip `anim_id` (client AbstractAnimID, social ids 1..=0x46 are emotes) once on `id` if it is not busy. The clip is resolved
    /// on the worker the first time (NPC record table, or the player set's file name via `combat::anim::resolve_clip`).
    pub fn play_once(&mut self, id: i32, anim_id: u32) {
        let Some(c) = self.chars.get_mut(&id).filter(|c| c.special == Special::None) else { return };
        let Look::Char(look) = &c.look else { return };
        if let Some(Model::Ready { built, .. }) = self.models.get(&c.key) {
            if built.clips.contains_key(&anim_id) {
                c.special = Special::Once(anim_id);
                c.clip_ms = 0.0;
                return;
            }
        }
        self.pending_clips.push((c.key, look.clone(), anim_id, id));
    }

    /// The weapon swing of `id` (`FUN_10069acb` [GC 0x10069acb] + `FUN_1003c594`): a random value of list `key` of the `AnimSet` lists of the
    /// weapon in its right hand (else left) - list 0xb when the weapon has no such key - and the weapon's `ItemDelay` (centiseconds), which
    /// a following [`Dynels::play_once`] of that clip uses for the swing speed scale. `None`: nothing wielded, or an `AnimSet` whose lists live
    /// in the item record (4, 5, martial arts; record layout not decoded, docs/zone/combat-anim.md §3.1): the caller plays the unarmed swing.
    /// [GUESS] the wielder is never crawling (stat 0x1ae == 0xe is not tracked), so the crawl lists are not used.
    pub fn pick_swing(&mut self, id: i32, key: u16) -> Option<(u16, i32)> {
        let hands = self.wield.get(&id)?;
        let hand = hands.iter().position(Option::is_some)?;
        let w = hands[hand]?;
        let mut list = canim::weapon_list(w.set, hand == 1, false, key);
        if list.is_empty() {
            list = canim::weapon_list(w.set, hand == 1, false, canim::list::ATTACK);
        }
        let anim = *list.get(self.rng.rand() as usize % list.len().max(1))?;
        self.swing_delay.insert(id, (anim as u32, w.delay));
        Some((anim, w.delay))
    }

    /// `id` dies: the death clip `anim` (client animation id, `CharacterAction` 99's `identity_b.instance`; any other value =
    /// the generic death) plays once and holds.
    pub fn die(&mut self, id: i32, anim: u32) {
        if let Some(c) = self.chars.get_mut(&id) {
            c.special = Special::Die(anim);
            c.clip_ms = 0.0;
        }
    }

    /// `CharDie_t` ctor [GC 0x1007b2ba] / `FUN_1005d0d8` case 0x5b [GC 0x1005eada]: the death (`key` = [`canim::npc_sound::DEATH`]) or hit
    /// (`HIT`) sound of character `id`, `PlayGameSound` at its position (docs/zone/combat-anim.md §6). A character with an NPC record plays a random
    /// value of the record's sound list `key` (`FUN_1004570c`; nothing while the model is not built or the record has no such key), every other
    /// one the male / female sound (Sex stat 3 = female). [INFERENCE] `FUN_10051f6e() != 0` = "the look has an NPC record".
    /// The own character is not drawn here: its sound plays at the camera (the camera sits at the avatar).
    pub fn char_sound(&mut self, id: i32, key: u32) {
        let Some(c) = self.chars.get(&id) else { return };
        let Look::Char(look) = &c.look else { return };
        let sound = if look.npc {
            let Some(Model::Ready { built, .. }) = self.models.get(&c.key) else { return };
            let ids = built.sounds.iter().find(|s| s.0 == key).map_or(&[][..], |s| &s.1[..]);
            match ids.len() {
                0 => return,
                1 => ids[0],
                n => ids[self.rng.rand() as usize % n],
            }
        } else {
            let name = if key == canim::npc_sound::DEATH { canim::die_sound(look.sex) } else { canim::hit_sound(look.sex) };
            ao_audio::sbf::sound_id(name)
        };
        let pos = if id == self.own { self.cam } else { scene_pos(c.pose.pos) };
        self.sounds.push(GameSound { id: sound, pos });
    }

    /// A named fight sound (`SM_Sandy_Game_Brawl` / `_Dimach`, `FUN_1003c594`) at character `id`.
    pub fn sound_at(&mut self, id: i32, name: &str) {
        let Some(c) = self.chars.get(&id) else { return };
        let pos = if id == self.own { self.cam } else { scene_pos(c.pose.pos) };
        self.sounds.push(GameSound { id: ao_audio::sbf::sound_id(name), pos });
    }

    fn add_prop(&mut self, kind: i32, instance: i32, look: Look, pos: [f32; 3], rot: Option<[f32; 4]>, scale: f32) {
        let (id, replace) = self.props.get(&(kind, instance)).map_or((self.next_prop, false), |p| (p.id, true));
        if !replace {
            self.next_prop += 1;
        }
        let stats = match &look {
            Look::Item { stats, .. } => stats.clone(),
            Look::Corpse(_) => vec![(CAN_STAT, CORPSE_CAN)],
            Look::Char(_) => vec![],
        };
        self.props.insert((kind, instance), Prop { id, key: look.key(), pos, yaw: rot.map_or(0.0, |q| quat_yaw(&q)), scale, submitted: false, anim: PropAnim::default(), stats });
        self.pending.push(look);
    }
    /// Message stats of the known prop `who` (a corpse's, a `StatIIR_t`'s): they replace earlier values.
    fn set_stats(&mut self, who: ao_net::msg::Identity, new: impl Iterator<Item = (u32, i32)>) {
        let Some(p) = self.props.get_mut(&(who.kind, who.instance)) else { return };
        for (id, v) in new {
            match p.stats.iter_mut().find(|s| s.0 == id) {
                Some(s) => s.1 = v,
                None => p.stats.push((id, v)),
            }
        }
    }

    /// Test hook: a prop with message stats and no model.
    #[cfg(test)]
    pub(super) fn test_prop(&mut self, who: ao_net::msg::Identity, stats: Vec<(u32, i32)>) {
        self.add_prop(who.kind, who.instance, Look::Item { template: None, stats }, [0.0; 3], None, 1.0);
    }

    /// Stat `id` of the non-character dynel `(kind, instance)`: a message's value, else its template's (`None` while the model is not built
    /// yet, the dynel is unknown or has no such stat).
    pub fn stat_of(&self, kind: i32, instance: i32, id: u32) -> Option<i32> {
        let p = self.props.get(&(kind, instance))?;
        ao_formats::dynel_visual::get(&p.stats, id).or_else(|| match self.models.get(&p.key) {
            Some(Model::Ready { built, .. }) => ao_formats::dynel_visual::get(&built.stats, id),
            _ => None,
        })
    }

    /// The non-character dynels the camera's selection line can hit (`FUN_10020a3c`, docs/zone/interact.md §8): the visible, built props
    /// with the box of their model. Each [`super::hud_pick::PickBody::id`] is the prop's `ActorFrame::id`; the identity is the second value.
    /// [INFERENCE] The original tests `VisualMesh_t` bodies against a bounding sphere and their triangles (`FUN_1006bb2a`); the box of the
    /// model's vertices stands in for it. The box is computed per call (a click, not per frame).
    pub fn pick_props(&self) -> Vec<(super::hud_pick::PickBody, ao_net::msg::Identity)> {
        self.props
            .iter()
            .filter_map(|(&(kind, instance), p)| {
                let Some(Model::Ready { built, .. }) = self.models.get(&p.key) else { return None };
                let verts = built.held.as_ref().map(|h| &h.0).into_iter().chain(built.model.meshes.iter().map(|m| &m.vertices)).flatten();
                let bounds = super::hud_pick::bounds_of(verts.map(|v| &v.pos))?;
                let body = super::hud_pick::PickBody { id: p.id as i32, bounds, pos: scene_pos(p.pos), yaw: scene_yaw(p.yaw), scale: p.scale };
                built.visible.then_some((body, ao_net::msg::Identity { kind, instance }))
            })
            .collect()
    }

    /// Live harness: `(kind, instance, server position, can stat)` of every prop.
    #[cfg(test)]
    pub fn prop_list(&self) -> Vec<(i32, i32, [f32; 3], Option<i32>)> {
        self.props.iter().map(|(&(k, i), p)| (k, i, p.pos, self.stat_of(k, i, CAN_STAT))).collect()
    }

    /// A message for the door `who` (queued while its model is still being built; unknown doors ignore it, like the client's `GetDynel`).
    fn door_command(&mut self, who: ao_net::msg::Identity, c: Cmd) {
        let Some(p) = self.props.get_mut(&(who.kind, who.instance)) else { return };
        let item = match self.models.get(&p.key) {
            Some(Model::Ready { built, .. }) => built.item.as_ref(),
            _ => None,
        };
        p.anim.command(c, item, scene_pos(p.pos), &mut self.rng, &mut self.sounds);
    }

    /// Door sounds started since the last call (the app plays them: `SandyInterfaceModule_t::PlayGameSound`).
    pub fn take_sounds(&mut self) -> Vec<GameSound> {
        std::mem::take(&mut self.sounds)
    }

    /// Doors whose room link state changed since the last call: `(scene position, open, passable)` (`n3RoomMonitor_t::DoorOpened/Closed`,
    /// `Door_t::CanPass`, see [`PropAnim::take_room_state`]); the caller maps the position to the link (`Collision::door_link_from_pos`).
    pub fn take_door_rooms(&mut self) -> Vec<([f32; 3], bool, bool)> {
        self.props.values_mut().filter_map(|p| p.anim.take_room_state().map(|(open, pass)| (scene_pos(p.pos), open, pass))).collect()
    }

    /// Every door hands its room link state out again (the caller built a new collision world).
    pub fn resync_doors(&mut self) {
        self.props.values_mut().for_each(|p| p.anim.resync());
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
            N3::World(World::DoorStatus(d)) if DOOR_KINDS.contains(&who.kind) => self.door_command(who, Cmd::Status(d.locked, d.open, d.value_c3, d.flag_1a)),
            N3::World(World::Door(d)) if DOOR_KINDS.contains(&who.kind) && !self.props.contains_key(&(who.kind, who.instance)) => {
                // `FUN_1009faaf`: nothing happens when the dynel exists; a new one opens at once when `Flags` bit 0x80 is set
                let stats = d.base.stats.clone();
                if let Some(pos) = d.base.position {
                    let open = ao_formats::dynel_visual::get(&stats, 0).is_some_and(|f| f as u32 & super::dynels_doors::FLAG_OPEN != 0);
                    let (template, scale) = (static_instance(&stats), stat_scale(&stats));
                    self.add_prop(who.kind, who.instance, Look::Item { template, stats }, pos, d.base.rotation, scale);
                    if open {
                        self.door_command(who, Cmd::Open);
                    }
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
                            self.set_stats(who, stats.iter().copied());
                        }
                    }
                    Err(e) => eprintln!("dynels: corpse {}: {e:#}", who.instance),
                }
            }
            N3::World(World::CharacterAction(a)) if who.kind == CHAR_KIND && canim::death_anim_from_action(a.action, a.identity_b.instance as u32).is_some() => {
                self.die(who.instance, a.identity_b.instance as u32)
            }
            N3::Dynel(Dynel::WeaponItemFullUpdate(w)) if w.parent.kind == CHAR_KIND => {
                // body location 6 = right hand, 8 = left hand (docs/zone/static.md §4)
                if let Some(hand) = match w.byte_71 {
                    6 => Some(0),
                    8 => Some(1),
                    _ => None,
                } {
                    let stats: Vec<(u32, i32)> = w.stats.iter().map(|&(i, v)| (i as u32, v)).collect();
                    self.weapons.insert(who.instance, (w.parent.instance, hand));
                    self.pending_weapons.push((w.parent.instance, hand, static_instance(&stats), stats));
                }
            }
            _ if who.kind != CHAR_KIND => {
                if let N3::Dynel(Dynel::Stat(s)) = &m.body {
                    self.set_stats(who, s.stats.iter().map(|x| (x.0 as u32, x.1)));
                }
                if matches!(m.body, N3::Misc(Misc::ToClientQuit)) {
                    self.props.remove(&(who.kind, who.instance));
                    if let Some((holder, hand)) = self.weapons.remove(&who.instance) {
                        if let Some(w) = self.wield.get_mut(&holder) {
                            w[hand] = None;
                        }
                    }
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
                        bounds: None,
                        roll: Roll::default(),
                    },
                );
            }
            N3::Dynel(Dynel::CharDCMove(mv)) => {
                if let Some(c) = self.chars.get_mut(&who.instance) {
                    c.mover.on_char_dc_move(mv);
                }
            }
            // `n3TeleportIIR_t::Activate` in-playfield branch for another character (zone changes only concern the own dynel)
            N3::Teleport(t) if !t.is_zone_change() => {
                if let Some(c) = self.chars.get_mut(&who.instance) {
                    c.mover.on_teleport(t.pos, &t.rot);
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
                self.swing_delay.remove(&who.instance);
            }
            _ => {}
        }
    }

    /// Integrates the movement of every character by `dt` seconds (the drawn pose and animation state).
    pub fn advance(&mut self, dt: f32) {
        for c in self.chars.values_mut() {
            c.pose = c.mover.advance(dt);
        }
    }

    /// Advances the dynels and hands the visible ones to the renderer. `cam` = camera position in scene space, `fwd` = its view direction.
    pub fn update(&mut self, dt: f32, cam: [f32; 3], fwd: [f32; 3], host: &mut Host) {
        self.cam = cam;
        let Some(dir) = self.dir.clone() else { return };
        let worker = self.worker.take().unwrap_or_else(|| Worker::start(dir));
        if let Some(pf) = self.want_placed.take() {
            let _ = worker.tx.send(Req::Placed(pf));
        }
        for (key, look, id, who) in std::mem::take(&mut self.pending_clips) {
            let _ = worker.tx.send(Req::Clip { key, look, id });
            self.replay.push((who, id));
        }
        for (holder, slot, template, stats) in std::mem::take(&mut self.pending_weapons) {
            let _ = worker.tx.send(Req::Weapon { holder, slot, template, stats });
        }
        let mut placed = vec![];
        while let Ok(r) = worker.rx.try_recv() {
            match r {
                Resp::Clip { key, id, anims } => {
                    if let (false, Some(Model::Ready { built, .. })) = (anims.is_empty(), self.models.get_mut(&key)) {
                        built.clips.insert(id, anims);
                    }
                    for (who, _) in self.replay.iter().filter(|r| r.1 == id) {
                        if let Some(c) = self.chars.get_mut(who).filter(|c| c.key == key && c.special == Special::None) {
                            if matches!(self.models.get(&key), Some(Model::Ready { built, .. }) if built.clips.contains_key(&id)) {
                                c.special = Special::Once(id);
                                c.clip_ms = 0.0;
                            }
                        }
                    }
                    self.replay.retain(|r| r.1 != id);
                }
                Resp::Weapon { holder, slot, wield } => self.wield.entry(holder).or_default()[slot] = wield,
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
            if *id != own && !self.asked.contains(&c.key) {
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
            // the clock runs for every prop, drawn or not (`SimpleItem_t` update)
            if let Some(item) = &built.item {
                p.anim.step(dt, item);
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
            let moved = built.item.as_ref().and_then(|item| p.anim.pose(item, &built.model.meshes[0].vertices, !p.submitted));
            let skin = built.held.as_ref().filter(|_| !p.submitted).map(|h| h.0.clone()).or(moved);
            p.submitted = true;
            let parts = built.held.as_ref().map_or(vec![], |h| h.1.clone());
            host.actors.push(ActorFrame { id: p.id, model: p.key, transform, parts, skin, always: false });
        }
        self.advance(dt);
        for (id, c) in &mut self.chars {
            if *id == own {
                continue;
            }
            let id_ref = id;
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
            if dist > self.char_view_distance {
                c.submitted = false;
                continue;
            }
            // clip: death / attack override the movement state; walking and running scale the clip with the real speed
            let state = c.pose.anim;
            let (key, list, rate) = match c.special {
                Special::Die(k) => match built.clips.get(&k).or_else(|| built.clips.get(&DIE_KEY)) {
                    Some(a) => (k, Some(a), 1.0),
                    None => (0x78, clip_of(built, state).map(|x| x.1), 1.0),
                },
                Special::Once(k) => match built.clips.get(&k) {
                    // `FUN_1006a239`: a weapon swing is sped up so its first note lands within the weapon's ItemDelay
                    Some(a) => (k, Some(a), self.swing_delay.get(id_ref).filter(|s| s.0 == k).map_or(1.0, |s| canim::swing_speed_scale(a.first().and_then(|a| a.events.first()).map_or(0.0, |e| e.0 as f32), s.1))),
                    None => (0x78, clip_of(built, state).map(|x| x.1), 1.0),
                },
                Special::Attack => match built.clips.get(&ATTACK_KEY) {
                    Some(a) => (ATTACK_KEY, Some(a), 1.0),
                    None => (0x78, clip_of(built, state).map(|x| x.1), 1.0),
                },
                Special::None => {
                    let (id, a) = clip_of(built, state).map_or((0x78, None), |(i, a)| (i, Some(a)));
                    // a wielder: weapon idle (list 0x10 of the first weapon in slot 6, 8) and 2H walk/run (lists 0x2a / 0x2b)
                    let set = self.wield.get(id_ref).and_then(|w| w.iter().flatten().next().map(|w| w.set));
                    let stance = set.and_then(|set| {
                        let key = match state {
                            AnimState::Idle => canim::list::IDLE,
                            AnimState::Walk => canim::list::WALK_2H,
                            AnimState::Run => canim::list::RUN_2H,
                            _ => return None,
                        };
                        canim::weapon_list(set, false, false, key).first().and_then(|&sid| built.clips.get(&(sid as u32)).map(|a| (sid as u32, a)))
                    });
                    let (id, a) = stance.map_or((id, a), |(i, a)| (i, Some(a)));
                    let nominal = match state {
                        AnimState::Walk | AnimState::WalkBack => max_speed(Mode::Walk, state == AnimState::WalkBack, c.mover.skill()),
                        AnimState::Run | AnimState::RunBack => max_speed(Mode::Run, state == AnimState::RunBack, c.mover.skill()),
                        _ => 0.0,
                    };
                    let rate = if nominal > 0.0 { (c.pose.speed / nominal).clamp(0.3, 2.0) } else { 1.0 };
                    (id, a, rate)
                }
            };
            // the variant is rolled when the clip starts, not while it loops
            let clip = list.filter(|l| !l.is_empty()).map(|l| &l[c.roll.pick((key, matches!(c.special, Special::None).then_some(state)), l.len(), &mut self.rng)]);
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
                    if matches!(c.special, Special::Attack | Special::Once(_)) {
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
                c.bounds = super::hud_pick::bounds_of(v.iter().map(|x| &x.pos));
                v
            });
            let (s, cs) = scene_yaw(c.pose.yaw).sin_cos();
            let k = c.scale;
            let transform = [[cs * k, 0.0, -s * k, 0.0], [0.0, k, 0.0, 0.0], [s * k, 0.0, cs * k, 0.0], [p[0], p[1], p[2], 1.0]];
            host.actors.push(ActorFrame { id: *id as u32, model: c.key, transform, parts: c.parts.clone(), skin, always: false });
        }
        self.worker = Some(worker);
    }

    /// The tag of `id` for `kind` (`FUN_10024e14` text and colours, `FUN_10024d5b` position): the text of [`name_tag`], centred on the head
    /// anchor (`GetIndicatorPosition`: attractor 0 + 0.5 m, scaled with the dynel); `None` while the dynel has no model, or its anchor
    /// fails the `x > 0` test of `FUN_10024d5b`. `bar` = health bar inputs `((health, max), colour)`.
    fn tag_of(&self, id: i32, kind: IndicatorKind, bar: Option<((i32, i32), u32)>) -> Option<Tag> {
        let c = self.chars.get(&id)?;
        let Some(Model::Ready { built, .. }) = self.models.get(&c.key) else { return None };
        if !tag_anchor_visible(c.pose.pos[0]) {
            return None;
        }
        let t = name_tag(&NameTagInput { name: &c.name, is_npc: c.npc, flags: c.flags, features: INVALID_STAT, visual_flags: c.visual_flags, side: c.side as i32, ..Default::default() });
        let p = scene_pos(c.pose.pos);
        Some(Tag {
            id,
            kind,
            rgb: t.rgb(),
            text: t.text,
            org: t.org,
            bar: bar.map(|((health, max), rgb)| (health_bar_fill_px(health, max), rgb)),
            centre: [p[0], p[1] + built.tag_height * c.scale, p[2]],
        })
    }

    /// The world-space tags of this frame (docs/zone/motion.md §6): the selection / attack `indicators` (the selection one only when the
    /// target's `Flags` bit 0x400 is clear), then — with `ShowAllNames` — the nametag set: every character except the client character
    /// within [`NAME_TAG_RADIUS`] of `own_pos` at the last 2 s rebuild (`HandleNametags`), except the dynels that carry an indicator,
    /// nearest first.
    fn collect_tags(&mut self, dt: f32, own_pos: [f32; 3], indicators: &[Indicator]) -> Vec<Tag> {
        let mut tags: Vec<Tag> = indicators
            .iter()
            .filter(|i| i.kind != IndicatorKind::Selection || self.chars.get(&i.id).is_some_and(|c| selection_indicator_exists(c.flags)))
            .filter_map(|i| self.tag_of(i.id, i.kind, i.bar))
            .collect();
        if !self.show_all_names {
            self.listing.clear();
            return tags;
        }
        let (chars, models, own) = (&self.chars, &self.models, self.own);
        let dist2 = |c: &Char| {
            let p = scene_pos(c.pose.pos);
            (0..3).map(|i| (p[i] - own_pos[i]).powi(2)).sum::<f32>()
        };
        let with_indicator: HashSet<i32> = tags.iter().map(|t| t.id).collect();
        let marked = |id: i32| with_indicator.contains(&id);
        let listed = self.listing.update(
            dt,
            || {
                let near = |c: &Char| dist2(c) <= NAME_TAG_RADIUS * NAME_TAG_RADIUS;
                let ok = |c: &Char| match models.get(&c.key) {
                    Some(Model::Ready { built, .. }) => nametag_listed(built.features),
                    _ => true,
                };
                chars.iter().filter(|(id, c)| **id != own && !marked(**id) && near(c) && ok(c)).map(|(id, _)| *id).collect()
            },
            |id| chars.contains_key(&id),
        );
        let mut ids: Vec<(f32, i32)> = listed.iter().filter(|id| !marked(**id)).filter_map(|id| Some((dist2(chars.get(id)?), *id))).collect();
        ids.sort_by(|a, b| a.0.total_cmp(&b.0));
        tags.extend(ids.into_iter().filter_map(|(_, id)| self.tag_of(id, IndicatorKind::Nametag, None)));
        tags
    }

    /// Adds the world-space tags of this frame to `host` (docs/zone/motion.md §6, `play/tags.rs`): `own_pos` = the player's scene
    /// position, `indicators` from [`super::tags::indicators`].
    pub fn name_tags(&mut self, dt: f32, gui: &mut Gui, host: &mut Host, own_pos: [f32; 3], indicators: &[Indicator]) {
        let tags = self.collect_tags(dt, own_pos, indicators);
        self.tags.frame(gui, host, &tags);
    }

    /// The characters the camera's selection line can hit (`FUN_10020a3c`): every dynel but the client character, with the
    /// box of its last pose ([`super::hud_pick`]); dynels not posed yet have no box and are skipped.
    pub fn pick_bodies(&self, own: i32) -> Vec<super::hud_pick::PickBody> {
        self.chars
            .iter()
            .filter(|(id, _)| **id != own)
            .filter_map(|(id, c)| Some(super::hud_pick::PickBody { id: *id, bounds: c.bounds?, pos: scene_pos(c.pose.pos), yaw: scene_yaw(c.pose.yaw), scale: c.scale }))
            .collect()
    }

    /// `(Flags (stat 0), Features (stat 0xe0) of the NPC record)` of `id`: what `InputConfig_t::CheckObjectUnderMouse` reads for the pointer.
    pub fn pointer_stats(&self, id: i32) -> Option<(i32, Option<i32>)> {
        let c = self.chars.get(&id)?;
        let features = match self.models.get(&c.key) {
            Some(Model::Ready { built, .. }) => built.features,
            _ => None,
        };
        Some((c.flags, features))
    }

    /// Screen position (GUI pixels) of the point `rise` metres above the head anchor of `id` (floating combat numbers rise 0.4 m/s,
    /// docs/zone/combat-log.md §6); `None` while the dynel has no model yet or is behind the camera.
    pub fn head_point(&self, id: i32, cam: &Camera, size: (u32, u32), rise: f32) -> Option<(f32, f32, ao_render::Vec3)> {
        let c = self.chars.get(&id)?;
        let Some(Model::Ready { built, .. }) = self.models.get(&c.key) else { return None };
        let p = scene_pos(c.pose.pos);
        let (w, h) = (size.0 as f32, size.1.max(1) as f32);
        let tan = (self.lens.vertical_fov(w / h) * 0.5).tan();
        let d = ao_render::Vec3::new(p[0], p[1] + built.tag_height * c.scale + rise, p[2]) - cam.pos;
        let z = d.dot(cam.forward());
        if z < 0.3 {
            return None;
        }
        Some(((0.5 + 0.5 * d.dot(cam.right()) / (z * tan * w / h)) * w, (0.5 - 0.5 * d.dot(cam.up()) / (z * tan)) * h, cam.pos + d))
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
    fn tags_follow_the_original_rules() {
        let mut z = Zone::new(25988);
        for f in frames(include_str!("../../../../docs/captures/zone_ithaca.rec")) {
            z.on_frame(&f);
        }
        let own_pos = scene_pos(z.own().unwrap().pos);
        let w = &mut z.world;
        for k in w.chars.values().map(|c| c.key).collect::<Vec<_>>() {
            w.models.insert(k, Model::Ready { built: Box::new(plain(Default::default(), true)), uploaded: true });
        }
        // pref off: nothing without indicators
        w.show_all_names = false;
        assert!(w.collect_tags(0.016, own_pos, &[]).is_empty());
        // pref on: the characters within 30 m, nearest first, never the client character
        w.own = 25988;
        w.show_all_names = true;
        let tags = w.collect_tags(0.016, own_pos, &[]);
        assert!(tags.len() > 3, "{} tags", tags.len());
        assert!(tags.len() > 3 && tags.iter().all(|t| t.kind == IndicatorKind::Nametag && t.bar.is_none() && t.id != 25988));
        let d = |id: i32| {
            let p = scene_pos(w.chars[&id].pose.pos);
            (0..3).map(|i| (p[i] - own_pos[i]).powi(2)).sum::<f32>().sqrt()
        };
        assert!(tags.iter().all(|t| d(t.id) <= NAME_TAG_RADIUS) && tags.windows(2).all(|p| d(p[0].id) <= d(p[1].id)));
        // the set is rebuilt only every 2 s: walking away keeps it, 2 s later it is empty
        let far = [own_pos[0] + 500.0, own_pos[1], own_pos[2]];
        assert_eq!(w.collect_tags(0.016, far, &[]).len(), tags.len());
        assert!(w.collect_tags(2.1, far, &[]).is_empty());
        // indicators: the selected dynel gets a plate + bar tag first and no plain tag; flags bit 0x400 suppresses the selection one
        w.collect_tags(2.1, own_pos, &[]);
        let id = w.collect_tags(0.016, own_pos, &[])[0].id;
        let sel = Indicator { id, kind: IndicatorKind::Selection, bar: Some(((30, 60), 0x00ff00)) };
        let t = w.collect_tags(0.016, own_pos, &[sel]);
        assert_eq!((t[0].id, t[0].kind, t[0].bar), (id, IndicatorKind::Selection, Some((32, 0x00ff00))));
        assert_eq!(t.iter().filter(|t| t.id == id).count(), 1);
        w.chars.get_mut(&id).unwrap().flags = 0x400;
        let t = w.collect_tags(0.016, own_pos, &[sel]);
        assert!(t.iter().all(|t| t.kind == IndicatorKind::Nametag) && t.iter().any(|t| t.id == id));
        // an unplaced dynel (server x <= 0) shows nothing
        w.chars.get_mut(&id).unwrap().pose.pos[0] = 0.0;
        assert!(w.collect_tags(0.016, own_pos, &[]).iter().all(|t| t.id != id));
    }

    /// A door of playfield 4582 (`{0xC748, 0xC00011E6}`, mesh 245910 with 7 animated nodes) follows `DoorStatusUpdate`: it slides open over
    /// the animation's 1.67 s and parks, then slides shut and returns to the rest vertices; each plays its sound from the template.
    #[test]
    fn placed_door_animates_on_status_updates() {
        use ao_net::msg::Identity;
        use ao_net::n3::world::{DoorStatus, DOOR_STATUS_UPDATE};
        use ao_net::n3::N3Header;
        let Some(dir) = client() else { return };
        let mut w = Dynels::default();
        w.start(dir.clone(), 1);
        w.on_playfield(4582);
        let who = Identity { kind: 0xC748, instance: 0xC00011E6u32 as i32 };
        let mut host = Host::headless();
        let mut eye = [0.0; 3];
        let frame = |w: &mut Dynels, eye: [f32; 3], dt: f32, host: &mut Host| {
            w.update(dt, eye, [0.0, 0.0, -1.0], host);
            let id = w.props.get(&(who.kind, who.instance)).map(|p| p.id);
            let skin = host.actors.iter().find(|a| Some(a.id) == id).map(|a| a.skin.clone());
            host.actors.clear();
            host.actor_models.clear();
            skin
        };
        for _ in 0..600 {
            if let Some(p) = w.props.get(&(who.kind, who.instance)) {
                eye = scene_pos(p.pos);
                eye[2] += 4.0;
                if matches!(w.models.get(&p.key), Some(Model::Ready { built, .. }) if built.item.is_some()) {
                    break;
                }
            }
            frame(&mut w, eye, 0.05, &mut host);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let key = w.props[&(who.kind, who.instance)].key;
        let Some(Model::Ready { built, .. }) = w.models.get(&key) else { panic!("door model not built") };
        let rest = built.model.meshes[0].vertices.clone();
        let item = built.item.as_ref().expect("the door mesh has animated nodes");
        assert!((item.rig.total_time() - 1.6667).abs() < 1e-3);
        // first sight: the actor goes out with the rest vertices (no skin)
        assert_eq!(frame(&mut w, eye, 0.016, &mut host), Some(None));
        let send = |w: &mut Dynels, open: bool| {
            let header = N3Header { msg_type: DOOR_STATUS_UPDATE, target: who, flag: 0 };
            let body = N3::World(World::DoorStatus(DoorStatus { locked: false, open, value_c3: 0, flag_1a: false, stats: vec![] }));
            w.on_message(&Message { header, sender: 1, body });
        };
        let moved = |a: &[ao_scene::Vertex]| a.iter().zip(&rest).map(|(a, b)| (0..3).map(|k| (a.pos[k] - b.pos[k]).abs()).fold(0.0, f32::max)).fold(0.0, f32::max);
        send(&mut w, true);
        let mut last = None;
        let mut mid = 0.0;
        for i in 0..150 {
            if let Some(Some(s)) = frame(&mut w, eye, 0.016, &mut host) {
                if i == 50 {
                    mid = moved(&s);
                }
                last = Some(s);
            }
        }
        let open = moved(&last.expect("pose changed while opening"));
        assert!(mid > 0.1 && open > mid && open > 1.0, "mid {mid:.2} open {open:.2}");
        // parked: nothing new is sent while the door stays open
        assert_eq!(frame(&mut w, eye, 0.016, &mut host), Some(None));
        send(&mut w, false);
        let mut closed = None;
        for _ in 0..150 {
            if let Some(Some(s)) = frame(&mut w, eye, 0.016, &mut host) {
                closed = Some(s);
            }
        }
        assert!(moved(&closed.expect("pose changed while closing")) < 1e-4, "the closed door shows the rest vertices again");
        let sounds = w.take_sounds();
        assert_eq!(sounds.iter().map(|s| s.id).collect::<Vec<_>>(), [0xcfde8382, 0xc16f0487]);
        assert!(w.take_sounds().is_empty());
        // the sound ids are Sandy sound definitions of the client's banks, with a sample of their own or children
        let lib = ao_audio::Library::load(&dir.join("cd_image/sound")).unwrap();
        for s in &sounds {
            assert!(lib.sounds.get(s.id).is_some_and(|d| d.file.is_some() || !d.children.is_empty()), "sound {:#x}", s.id);
        }
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
            let pending = keys(&z).iter().any(|k| matches!(z.world.models.get(k), None | Some(Model::Loading))) || z.world.want_placed.is_some() || z.world.props.len() < 12 || z.world.wield.len() < 3;
            if !pending {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        // frame cost of the dynel update with every captured dynel in view (skinning at most every 40 ms each)
        let t = std::time::Instant::now();
        let n = 200;
        let mut pushed = 0;
        for _ in 0..n {
            z.world.update(0.016, eye, fwd, &mut host);
            pushed += host.actors.len();
            host.actors.clear();
            host.actor_models.clear();
        }
        eprintln!("update: {:.3} ms/frame, {} actors/frame", t.elapsed().as_secs_f32() * 1000.0 / n as f32, pushed / n);
        let failed = z.world.models.values().filter(|m| matches!(m, Model::Failed)).count();
        eprintln!("{} models ready, {failed} failed, {} actors, {} props", models.len(), actors.len(), z.world.props.len());
        assert!(!actors.is_empty());
        // object use (docs/zone/interact.md §8): `Corpse_t`'s constructor sets Can 8, the vending machine's template has bit 3 (use);
        // built props have a pick box and a ray at one of them hits it, nearest first
        let corpse = z.world.props.keys().find(|k| k.0 == 0xC76A).copied().unwrap();
        assert_eq!(z.world.stat_of(corpse.0, corpse.1, CAN_STAT), Some(8));
        let vending = z.world.props.keys().find(|k| k.0 == 0xC75B).copied().unwrap();
        assert!(z.world.stat_of(vending.0, vending.1, CAN_STAT).is_some_and(|c| c & 8 != 0), "vending machine Can");
        let picks = z.world.pick_props();
        assert!(picks.len() >= 2 && picks.iter().all(|(b, _)| (0..3).all(|i| b.bounds.0[i] <= b.bounds.1[i])), "{} pick boxes", picks.len());
        let (body, who) = picks.iter().find(|(_, id)| id.kind == 0xC76A).unwrap();
        let mid = [body.pos[0], body.pos[1] + (body.bounds.0[1] + body.bounds.1[1]) * 0.5 * body.scale, body.pos[2]];
        let origin = ao_render::Vec3::new(mid[0] + 4.0, mid[1] + 0.5, mid[2] + 4.0);
        let toward = (ao_render::Vec3::new(mid[0], mid[1], mid[2]) - origin).normalize();
        let hit = crate::play::interact_use::pick_objects(&crate::play::hud_target::Ray { origin, dir: toward, len: 100.0 }, &z);
        assert_eq!(hit.first(), Some(who), "{hit:?}");
        if let Ok(out) = std::env::var("AOMAC_DYNEL_SHOT") {
            let scene = ao_formats::playfield::load_playfield_at(&RecordStore::open(&dir).unwrap(), &dir, 4582, ao_formats::playfield::DEFAULT_DAY_TIME).unwrap();
            // AOMAC_DYNEL_LOOK=<kind hex like c76a | npc | player>: camera 4 m from the first such dynel instead of the player's view
            let (mut cam, mut at) = ([eye[0], eye[1] + 1.7, eye[2]], [eye[0] + fwd[0], eye[1] + 1.5 + fwd[1], eye[2] + fwd[2]]);
            if let Ok(what) = std::env::var("AOMAC_DYNEL_LOOK") {
                let target = match what.as_str() {
                    "npc" => z.world.chars.values().find(|c| c.npc).map(|c| c.pose.pos),
                    m if m.starts_with("monster:") => z.world.chars.values().find(|c| matches!(&c.look, Look::Char(l) if l.monster_data.to_string() == m[8..])).map(|c| c.pose.pos),
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

    /// The capture replayed with its timestamps: moving NPCs walk (their animation state follows the movement FSM) and their drawn
    /// position stays within a few metres of what the server last told.
    #[test]
    fn replayed_npcs_move_and_play_walk_clips() {
        let mut z = Zone::new(25988);
        z.world.start(PathBuf::new(), 25988);
        let rec = include_str!("../../../../docs/captures/zone_ithaca.rec");
        let mut last_ms = 0u32;
        let (mut walked, mut ran) = (HashSet::new(), HashSet::new());
        let mut known: HashMap<i32, Vec<[f32; 3]>> = HashMap::new();
        for l in rec.lines() {
            let mut p = l.split(' ');
            let (ms, dir, hex) = (p.next().unwrap().parse::<u32>().unwrap(), p.next().unwrap(), p.next().unwrap());
            let b: Vec<u8> = (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap()).collect();
            if dir != "<" {
                continue;
            }
            // advance the clock to the frame's time in 1/30 s steps
            while last_ms + 33 <= ms {
                z.world.advance(1.0 / 30.0);
                last_ms += 33;
                for (id, c) in &z.world.chars {
                    match c.pose.anim {
                        AnimState::Walk => walked.insert(*id),
                        AnimState::Run => ran.insert(*id),
                        _ => false,
                    };
                }
            }
            let Some((f, _)) = Frame::decode_with(&b, false).ok().flatten() else { continue };
            if let Ok(m) = ao_net::n3::decode(&f) {
                match &m.body {
                    N3::Dynel(Dynel::CharDCMove(mv)) => known.entry(m.header.target.instance).or_default().push(mv.pos),
                    N3::Dynel(Dynel::SimpleCharFullUpdate(u)) => known.entry(m.header.target.instance).or_default().push(u.pos),
                    N3::Misc(Misc::FollowTarget(t)) => known.entry(m.header.target.instance).or_default().extend(t.path.iter().map(|v| [v.x, v.y, v.z])),
                    _ => {}
                }
                z.on_frame(&f);
            }
        }
        assert!(walked.len() + ran.len() >= 5, "walking {} running {}", walked.len(), ran.len());
        // every dynel is drawn on the polyline of the positions and waypoints the server sent (within 3 m)
        let seg = |p: [f32; 3], a: [f32; 3], b: [f32; 3]| {
            let ab: [f32; 3] = std::array::from_fn(|i| b[i] - a[i]);
            let l2: f32 = ab.iter().map(|x| x * x).sum();
            let t = if l2 > 0.0 { ((0..3).map(|i| (p[i] - a[i]) * ab[i]).sum::<f32>() / l2).clamp(0.0, 1.0) } else { 0.0 };
            (0..3).map(|i| (p[i] - a[i] - t * ab[i]).powi(2)).sum::<f32>().sqrt()
        };
        let off = z
            .world
            .chars
            .iter()
            .filter(|(id, c)| {
                let pts = &known[*id];
                pts.len() == 1 && seg(c.pose.pos, pts[0], pts[0]) > 3.0 || pts.windows(2).map(|w| seg(c.pose.pos, w[0], w[1])).fold(f32::MAX, f32::min) > 3.0 && pts.len() > 1
            })
            .count();
        assert!(off * 10 <= z.world.chars.len(), "{off} of {} dynels are more than 3 m off the server polyline", z.world.chars.len());
    }
}

#[cfg(test)]
mod variant_tests {
    use super::*;
    use crate::play::zone::Zone;
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

    /// `FUN_10069acb`: the swing is a value of the wielded weapon's list (its `AnimSet`, the hand), a special key the weapon lacks falls back
    /// to list 0xb; nothing wielded = no weapon swing.
    #[test]
    fn swing_comes_from_the_wielded_weapon() {
        let mut d = Dynels::default();
        assert_eq!(d.pick_swing(7, canim::list::ATTACK), None);
        d.wield.entry(7).or_default()[0] = Some(Wield { set: 1, delay: 120 });
        for _ in 0..20 {
            let (a, delay) = d.pick_swing(7, canim::list::ATTACK).unwrap();
            assert!([0x3eb, 0x3ec].contains(&a) && delay == 120, "{a:#x}");
            assert!([0x3eb, 0x3ec].contains(&d.pick_swing(7, canim::list::BURST).unwrap().0), "a 1H blade has no burst: list 0xb");
        }
        assert_eq!(d.pick_swing(7, canim::list::SNEAK_ATTACK).unwrap().0, 0x3ec);
        assert_eq!(d.swing_delay[&7], (0x3ec, 120));
        // only the left hand holds a weapon
        d.wield.get_mut(&7).unwrap().swap(0, 1);
        assert!([0x3ee, 0x3ef].contains(&d.pick_swing(7, canim::list::ATTACK).unwrap().0));
        // an AnimSet without lists (4, 5, martial arts): the caller plays the unarmed swing
        d.wield.entry(8).or_default()[0] = Some(Wield { set: 4, delay: 100 });
        assert_eq!(d.pick_swing(8, canim::list::ATTACK), None);
    }

    /// The variant is rolled when a clip starts (key or state change), not on every frame of its loop, and a single clip never
    /// consumes the RNG (`FUN_1004570c`).
    #[test]
    fn variant_rolled_per_clip_start() {
        let mut rng = CrtRand::new(1); // rand(): 41, 18467, 6334, 26500
        let mut r = Roll::default();
        let idle = (0x78, Some(AnimState::Idle));
        assert_eq!(r.pick(idle, 3, &mut rng), 41 % 3);
        for _ in 0..50 {
            assert_eq!(r.pick(idle, 3, &mut rng), 41 % 3, "looping idle keeps its variant");
        }
        // a single-variant clip does not touch the RNG ...
        assert_eq!(r.pick((0x65, Some(AnimState::Walk)), 1, &mut rng), 0);
        // ... and the next start of the idle clip rolls the next value
        assert_eq!(r.pick(idle, 3, &mut rng), 18467 % 3);
        // the same clip started again after a special (attack) is a new start
        assert_eq!(r.pick((1033, None), 2, &mut rng), 6334 % 2);
        assert_eq!(r.pick(idle, 3, &mut rng), 26500 % 3);
    }

    /// Real data: the client's NPC records do file several idle variants under one key, and `holder_variants` finds them all.
    #[test]
    fn real_npc_records_have_idle_variants() {
        let Some(dir) = std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Games/ProjectRubiKa/client")).filter(|d| d.join("cd_image/rdb.db").exists()) else { return };
        let store = RecordStore::open(&dir).unwrap();
        let multi = store.ids(ao_formats::character::NPC_TYPE).unwrap().into_iter().filter_map(|id| NpcRecord::load(&store, id).ok()).filter(|r| ao_formats::character::holder_variants(r, 0x78).len() > 1).count();
        eprintln!("records with >1 idle-stand variants: {multi}");
        assert!(multi > 0);
    }

    /// A corpse is drawn unanimated: its body vertices are the model's own (bind) vertices, whatever death clips the model has.
    #[test]
    fn corpse_is_unanimated() {
        let Some(dir) = std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Games/ProjectRubiKa/client")).filter(|d| d.join("cd_image/rdb.db").exists()) else { return };
        let store = RecordStore::open(&dir).unwrap();
        let mut assets = ActorAssets::new(&store).unwrap();
        let c = CorpseLook { cat_mesh: 22773, head: None, breed: 6, sex: 1, race: 1, cloth: vec![], textures: vec![] };
        let built = build_corpse(&store, &mut assets, &c).unwrap();
        let held = built.held.expect("held pose");
        assert!(built.clips.is_empty());
        assert_eq!(held.0, built.model.meshes[0].vertices);
    }

    /// The kill of `zone_fight_ithaca.rec` (`CharacterAction` 99, death animation 503, on a Beach Leet) applied to a Beach Leet of
    /// `zone_ithaca.rec` (the fight capture starts after the NPC was announced): it plays its death clip and holds the last frame.
    #[test]
    fn replayed_kill_plays_the_death_clip() {
        let Some(dir) = client() else { return };
        let mut z = Zone::new(25988);
        z.world.start(dir, 25988);
        for f in frames(include_str!("../../../../docs/captures/zone_ithaca.rec")) {
            z.on_frame(&f);
        }
        let mut kill = None;
        for f in frames(include_str!("../../../../docs/captures/zone_fight_ithaca.rec")) {
            if let Ok(m) = ao_net::n3::decode(&f) {
                if matches!(&m.body, N3::World(World::CharacterAction(a)) if a.action == 99) {
                    kill = Some(m);
                }
            }
        }
        let mut kill = kill.expect("the capture holds the kill");
        let leet = *z.world.chars.iter().find(|(_, c)| c.name == "Beach Leet").expect("a Beach Leet in the zone capture").0;
        kill.header.target.instance = leet;
        z.world.on_message(&kill);
        assert!(matches!(z.world.chars[&leet].special, Special::Die(503)));
        let mut host = Host::headless();
        let (eye, fwd) = (crate::play::zone::scene_pos(z.own().unwrap().pos), [0.0, 0.0, -1.0]);
        for _ in 0..600 {
            z.world.update(0.05, eye, fwd, &mut host);
            host.actors.clear();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let c = &z.world.chars[&leet];
        let Some(Model::Ready { built, .. }) = z.world.models.get(&c.key) else { panic!("model not ready") };
        eprintln!("kill: special {:?} anim {} clips {:?}", c.special, c.anim, built.clips.keys().collect::<Vec<_>>());
        assert!(matches!(c.special, Special::Die(_)));
        assert!(c.anim == 503 || c.anim == DIE_KEY, "playing a death clip, not idle: {}", c.anim);
        // the creature's death sound is a value of its record's key 0x1e list (`FUN_1004570c`), at the corpse-to-be's position
        let want = built.sounds.iter().find(|s| s.0 == canim::npc_sound::DEATH).map(|s| s.1.clone()).unwrap_or_default();
        let pos = crate::play::zone::scene_pos(c.pose.pos);
        z.world.char_sound(leet, canim::npc_sound::DEATH);
        let got = z.world.take_sounds();
        assert_eq!(got.len(), usize::from(!want.is_empty()), "{want:?}");
        assert!(got.iter().all(|s| want.contains(&s.id) && s.pos == pos));
        // the server removes the dynel ~3 s later (`n3ToClientQuit`): the NPC is gone
        let mut quit = kill.clone();
        quit.body = N3::Misc(Misc::ToClientQuit);
        z.world.on_message(&quit);
        assert!(!z.world.chars.contains_key(&leet));
    }

    /// `CharDie_t` / action 0xd1 sounds of a player (no NPC record): the male / female sound by the Sex stat, at the character.
    #[test]
    fn player_fight_sounds_follow_the_sex() {
        let mut z = Zone::new(25988);
        for f in frames(include_str!("../../../../docs/captures/zone_ithaca.rec")) {
            z.on_frame(&f);
        }
        let (id, sex) = z.world.chars.iter().find_map(|(id, c)| match &c.look { Look::Char(l) if !l.npc => Some((*id, l.sex)), _ => None }).expect("a player in the capture");
        z.world.char_sound(id, canim::npc_sound::DEATH);
        z.world.char_sound(id, canim::npc_sound::HIT);
        z.world.sound_at(id, canim::sound::BRAWL);
        let s = z.world.take_sounds();
        let ids: Vec<u32> = s.iter().map(|s| s.id).collect();
        let sid = ao_audio::sbf::sound_id;
        assert_eq!(ids, [sid(canim::die_sound(sex)), sid(canim::hit_sound(sex)), sid(canim::sound::BRAWL)]);
        assert!(s.iter().all(|s| s.pos == scene_pos(z.world.chars[&id].pose.pos)));
        z.world.char_sound(id + 1_000_000, canim::npc_sound::DEATH);
        assert!(z.world.take_sounds().is_empty(), "unknown dynels are silent");
    }
}
