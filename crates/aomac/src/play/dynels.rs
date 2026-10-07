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
use ao_formats::character::{load_cat_mesh, AnimLayer, CatAnim, ClothPart, CrtRand, Equipment, NpcRecord, TextureOverride, CHAR_MESH_TYPE};
use ao_gui::Gui;
use ao_net::n3::dynel::{Dynel, SimpleCharFullUpdate};
use ao_net::n3::misc::Misc;
use ao_net::n3::motion::{max_speed, AnimState, Mode, Mover, STAT_HEALTH};
use ao_net::n3::nametag::{health_bar_fill_px, name_tag, nametag_listed, selection_indicator_exists, tag_anchor_visible, IndicatorKind, NameTagInput, INVALID_STAT};
use ao_net::n3::world::World;
use ao_net::n3::{Message, N3};
use ao_rdb::RecordStore;
use ao_render::Host;
#[cfg(test)]
use ao_render::Camera;
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

#[path = "dynels_actions.rs"]
mod actions;
#[path = "dynels_buffs.rs"]
mod buffs;

/// Identity kind of character / NPC dynels (`SimpleChar_t`).
pub(super) const CHAR_KIND: i32 = 0xC350;
/// Props (non-character dynels) farther than this (metres) are not drawn (about the fog distance of the outdoor playfields; a guess:
/// the client draws them up to the far plane). Characters use `Dynels::char_view_distance`.
pub const DRAW_DISTANCE: f32 = 250.0;
/// `ShowAllNames` name tags exist for dynels within this radius of the player (`GetDynelsInVicinity`, docs/zone/motion.md §6).
pub const NAME_TAG_RADIUS: f32 = 30.0;
/// `CharacterActionIIR_t` id of the unwield of a body slot (`identity_b.instance`): `FUN_1005d0d8` case at 0x1005d790 -> `FUN_1006a857` (docs/zone/actions.md).
const ACTION_UNWIELD: i32 = 0x61;
/// NPC record key of the generic death clip and of the first unarmed attack ([`ao_formats::character::NpcAnim`]).
const DIE_KEY: u32 = 6000;
const ATTACK_KEY: u32 = canim::UNARMED_RSWING as u32;
/// Weapon item stat `AnimSet` (0x161): selects the stance clips (docs/zone/combat-anim.md §3.1).
const STAT_ANIM_SET: u32 = 353;
/// Weapon item stat `ItemDelay` (0x126, centiseconds): the swing is sped up to land within it (`FUN_1006a239`).
const STAT_ITEM_DELAY: u32 = 294;
/// Weapon stance clips loaded with every character model: the fight idle (list 0x10) of the anim sets 0/1/3/6/7/8, the rifle's idle-2h (list 0x29) and its walk/run.
const STANCE_IDS: &[u16] = &[0x3f3, 0x3e9, 0x3fd, 0xb6, 0xcb, 0x424, 0x41e, 0x421, 0x422];

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
        // The full update calls CharacterMesh::ClearAttractors (bookkeeping only),
        // not VisualCATMesh_t::ClearAttractors (which removes render children).
        // Preserve the separately mounted HeadMesh when the wire list omits it.
        let mut attractors: Vec<_> = u.attractors.iter().map(|a| (a.place, a.mesh)).collect();
        if let Some(head) = u.head_mesh.filter(|&h| h > 0) {
            if !attractors.iter().any(|a| a.0 == 0) {
                attractors.push((0, head));
            }
        }
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
            attractors,
            skip_attractors: u.flags & ao_net::n3::dynel::flag::SET_DYNEL_800 != 0,
        }
    }
}

impl CharLook {
    /// `AppearanceUpdateIIR_c::Activate` [GC 0x10071679] on a character's look, any `SimpleChar_t` (NPCs too, no kind test beyond the identity):
    /// the cloth entries are written by `FUN_100480fc` at index `page * 5 + part` (only page 0 is drawn; an entry whose texture is unchanged is
    /// left alone, one with texture 0 clears the part, parts the message does not name stay), then `VisualCATMesh_t::ClearAttractors` +
    /// `CharacterMesh::AddAttractors(list)`: the mounted set is exactly the wire list (head = its place 0), unconditionally (the `SET_DYNEL_800`
    /// flag only skips the full update's block). `cloth` is kept sorted so equal looks share a model whatever order the updates came in.
    pub fn apply_appearance(&mut self, a: &ao_net::n3::world::AppearanceUpdate) {
        for c in a.cloth.iter().filter(|c| c.c == 0) {
            self.cloth.retain(|p| p.0 != c.id);
            if c.b > 0 {
                self.cloth.push((c.id, c.b));
            }
        }
        self.cloth.sort_unstable();
        self.attractors = a.attractors.iter().map(|t| (t.a, t.b)).collect();
        self.skip_attractors = false;
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
    /// NPC record and animation key of the corpse's play-animation spell (stats 0x30 / 7).
    pub animation: Option<(u32, u32)>,
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
    /// Runtime item class selected by `CreateFromTemplate`, independent of the placed dynel identity.
    pub item_kind: Option<u32>,
    /// Display name parsed from the item template, shared with the model.
    pub name: Option<String>,
    /// The stats of an item-family dynel: its template's overlaid by the message's (`N3Msg_DefaultActionOnDynel` reads `Can`, interact.rs).
    pub stats: Vec<(u32, i32)>,
    /// The NPC record's sound multimap (`NpcRecord::sounds`: `AbstractAnimID_e` key -> sound ids; fight keys `combat::anim::npc_sound`).
    pub sounds: Vec<(u32, Vec<u32>)>,
    /// The NPC record's stat 41 `FabricType` (1..=17: the material of the impact sounds, `FUN_1009b4ac`); 0 without a record.
    pub fabric: i32,
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
                        let ids = match clip_ids(&store, &assets, &look, id) {
                            Ok(ids) => ids,
                            Err(e) => {
                                eprintln!("dynels: clip {id}: {e:#}");
                                vec![]
                            }
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
    Built { model, rig: None, clips: HashMap::new(), features: None, tag_height: 1.0, held: None, visible, item: None, item_kind: None, name: None, stats: Vec::new(), sounds: Vec::new(), fabric: 0 }
}

fn build(store: &RecordStore, assets: &mut ActorAssets, look: &Look) -> anyhow::Result<Built> {
    match look {
        Look::Char(c) => build_char(store, assets, c),
        Look::Corpse(c) => build_corpse(store, assets, c),
        Look::Item { template, stats } => {
            let mut tpl = template.map(|t| item_template(store, t)).transpose()?.flatten();
            let item_kind = tpl.as_ref().map(|t| t.kind);
            let name = tpl.as_mut().and_then(|t| t.name.take());
            let eff = effective_stats(tpl.as_ref(), stats);
            let v = visual(&eff, default_mesh(&assets.names)?);
            match (v.cat_mesh, v.mesh) {
                (Some(cat), _) => {
                    let rig = ActorRig::new(store, assets, cat, None, &Default::default(), &Default::default(), &[])?;
                    let held = rig.pose(None);
                    Ok(Built { model: rig.model().clone(), rig: Some(Arc::new(rig)), held: Some(held), item_kind, name, stats: eff, ..plain(Default::default(), v.visible) })
                }
                (None, Some(mesh)) => {
                    let model = static_model(store, mesh, v.override_texture)?;
                    let item = ItemRig::new(store, mesh, &model, &eff, tpl.map(|t| t.sounds).unwrap_or_default());
                    Ok(Built { item, item_kind, name, stats: eff, ..plain(model, v.visible) })
                }
                (None, None) => anyhow::bail!("item dynel without a model"),
            }
        }
    }
}

/// A corpse holds the terminal pose of its play-animation spell, not the CAT bind pose.
/// The four standard spell arguments precede the type arguments (GD 0x1000fa93).
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
            ActorRig::new(store, assets, c.cat_mesh, c.head, &npc_part_textures(store, &assets.names, &cat, c.head, &list, &cloth), &npc_part_layers(&cat, &list), &[])?
        }
    };
    let animation = match c.animation {
        Some((record, key)) => match NpcRecord::lookup(store, record)? {
            Some(record) => {
                let id = ao_formats::character::anim_key_variants(&record, key).first().copied().ok_or_else(|| anyhow::anyhow!("corpse animation key {key} absent from NPC record"))?;
                Some(assets.anim(store, id)?)
            }
            None => None,
        },
        None => None,
    };
    let held = rig.pose(animation.as_ref().map(|a| (&**a, a.duration)));
    Ok(Built { model: rig.model().clone(), rig: Some(Arc::new(rig)), held: Some(held), ..plain(Default::default(), true) })
}

fn build_char(store: &RecordStore, assets: &mut ActorAssets, look: &CharLook) -> anyhow::Result<Built> {
    let wire: Vec<(u8, u32)> = look.attractors.iter().filter(|a| a.1 > 0).map(|a| (a.0, a.1 as u32)).collect();
    // from_update retains the full update's separately mounted head; appearance updates replace that set.
    let mut mounted = if look.skip_attractors { vec![] } else { attractor_list(look.head.filter(|&h| h > 0).map(|h| h as u32), &wire) };
    let head = mounted.iter().position(|a| a.0 == 0).map(|i| mounted.remove(i).1);
    let attachments = mounted;
    // GC 0x10058078: null MonsterData follows the normal breed/sex/build resolver.
    let rec = monster_record(store, look)?;
    let rig = if let Some(rec) = &rec {
        npc_rig(store, assets, look, rec, head, &attachments)?
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
        let textures: Vec<_> = look.textures.iter().map(|(m, t, env, alpha)| TextureOverride { material: m, texture: *t as u32, env_texture: *env as u32, alpha_mode: *alpha as u32 }).collect();
        let look = PlayerLook { breed, gender, skin, build: look.fatness.min(2), head, equipment };
        ActorRig::player_with_textures(store, assets, &look, &attachments, &textures)?
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
    let fabric = rec.as_ref().and_then(|r| r.stat(super::combat::notes::STAT_FABRIC_TYPE)).unwrap_or(0);
    let sounds = rec.map(|r| r.sounds).unwrap_or_default();
    Ok(Built { model: rig.model().clone(), rig: Some(Arc::new(rig)), clips, features, tag_height, sounds, fabric, ..plain(Default::default(), true) })
}

fn monster_record(store: &RecordStore, look: &CharLook) -> anyhow::Result<Option<NpcRecord>> {
    // GC 0x10051f6e: Features 0x800 selects 99902 instead of the MonsterData stat.
    let id = if look.skip_attractors { 99902 } else { look.monster_data };
    if id > 0 { NpcRecord::lookup(store, id as u32) } else { Ok(None) }
}

fn clip_ids(store: &RecordStore, assets: &ActorAssets, look: &CharLook, id: u32) -> anyhow::Result<Vec<u32>> {
    let rec = monster_record(store, look)?;
    Ok(match rec {
        Some(rec) => ao_formats::character::anim_key_variants(&rec, id).to_vec(),
        None => canim::resolve_clip(&assets.names, canim::clip_set(look.breed, look.sex), id as u16, false).map(|c| c.0).into_iter().collect(),
    })
}

fn named(store: &RecordStore, assets: &mut ActorAssets, model: u32, name: &str) -> anyhow::Result<Option<Arc<CatAnim>>> {
    let id = assets.clips(store, model)?.iter().find(|c| c.0 == name).map(|c| c.1);
    id.map(|id| assets.anim(store, id)).transpose()
}

/// An NPC: the record's model (`MonsterData` -> rdb 1040023 `Mesh`), `textures[]` replacing the part textures (`SetCATTextures`),
/// worn `cloth[]` composited over the part's texture like the player equipment, `head` (the place-0 attractor) and attachment
/// meshes. The record's own `HeadMesh` stat only selects the naked skin textures (`FUN_10058078`), it mounts nothing.
fn npc_rig(store: &RecordStore, assets: &ActorAssets, look: &CharLook, rec: &NpcRecord, head: Option<u32>, attachments: &[(u8, u32)]) -> anyhow::Result<ActorRig> {
    let model = rec.mesh().context("NPC record has no mesh")?;
    let cat = load_cat_mesh(store, CHAR_MESH_TYPE, model)?;
    let list: Vec<TextureOverride> = look.textures.iter().map(|(m, t, env, alpha)| TextureOverride { material: m, texture: *t as u32, env_texture: *env as u32, alpha_mode: *alpha as u32 }).collect();
    let cloth: Vec<(ClothPart, u32)> = look.cloth.iter().filter(|c| c.1 > 0).filter_map(|&(p, t)| Some((*ClothPart::ALL.get(p as usize)?, t as u32))).collect();
    let skin_head = rec.stat(64).filter(|&h| h > 0).map(|h| h as u32);
    let overrides = npc_part_textures(store, &assets.names, &cat, skin_head, &list, &cloth);
    let rig = ActorRig::new(store, assets, model, head, &overrides, &npc_part_layers(&cat, &list), attachments)?;
    Ok(rig)
}

/// What a character is doing besides moving.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Special {
    None,
    /// Clip `AbstractAnimID` plays once (emotes and nano-release state).
    Once(u32),
    /// CharCastNano's stat 0x178 clip loops until release.
    Cast(u32),
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
    /// Key of the look in `look` while its model is still being built: `key` (the model drawn) switches to it when it is ready, so a wield does not make the character vanish for the build time.
    next: Option<u64>,
    pub key: u64,
    pub mover: Mover,
    /// `MonsterScale / 100`.
    pub scale: f32,
    pub side: u8,
    /// Stat `Flags` (0) and `VisualFlags` (0x2A1), name tag inputs.
    flags: i32,
    visual_flags: i32,
    /// Full-update InPlay and attachment state (GC 0x10077af2 / GUI 0x10024d5b).
    in_play: bool,
    parent: Option<ao_net::msg::Identity>,
    pose: ao_net::n3::motion::Pose,
    anim: u32,
    special: Special,
    actions: Vec<ActionAnim>,
    base_layers: u32,
    clip_ms: f32,
    clip_rate: f32,
    terminal_pose: bool,
    submitted: bool,
    /// Mount transforms of the last pose.
    parts: Vec<[[f32; 4]; 4]>,
    features_set: bool,
    /// Model-space box of the last pose's skinned body vertices (`RCATMesh_t+0x1fc/+0x208`, hud_pick.rs); `None` until posed.
    bounds: Option<([f32; 3], [f32; 3])>,
    roll: Roll,
    /// Bit `i` = event `i` of the playing clip has fired its note (`piVar7[0xd]` of the holder's clip entry, `FUN_1003c036`).
    note_fired: u32,
}

/// Authored action playback is independent of locomotion and its markers.
struct ActionAnim {
    id: u32,
    layer: canim::Layer,
    priority: i32,
    layers: u32,
    fade_ms: f32,
    ms: f32,
    variant: Option<usize>,
    note_fired: u32,
    rate: f32,
    key: Option<u16>,
    slot: i32,
    delay: Option<i32>,
}

impl ActionAnim {
    fn advance(&mut self, clip: &CatAnim, dt: f32, mut emit: impl FnMut(super::combat::notes::FiredNote)) -> bool {
        use super::combat::notes::{fire, finish, id, FiredNote};
        let slot = self.slot;
        let mut emit_note = |note| {
            if slot >= 0 || !matches!(note, id::ATTACK | id::ATTACK_EFFECT_1..=id::ATTACK_EFFECT_4) {
                emit(FiredNote { id: note, slot });
            }
        };
        if self.ms >= clip.duration {
            for id in finish(&clip.events, &mut self.note_fired) { emit_note(id); }
            return false;
        }
        if self.ms == 0.0 {
            if let Some(delay) = self.delay {
                self.rate = canim::swing_speed_scale(clip.events.first().map_or(0.0, |e| e.0 as f32), delay);
            }
        }
        self.ms += (dt * 1000.0 * self.rate).abs();
        for id in fire(&clip.events, self.ms.min(clip.duration), &mut self.note_fired) { emit_note(id); }
        true
    }
}

impl Char {
    fn base_priority(&self) -> i32 {
        if matches!(self.special, Special::Die(_)) { -3 } else { 0 }
    }

    fn layers<'a>(&'a self, built: &'a Built) -> impl Clone + Iterator<Item = AnimLayer<'a>> {
        let priority = self.base_priority();
        let base = built.clips.get(&self.anim)
            .and_then(|clips| clips.get(self.roll.variant % clips.len().max(1)))
            .map(|clip| AnimLayer { clip, ms: super::avatar::clip_time(clip, self.clip_ms, !matches!(self.special, Special::None | Special::Cast(_))), layers: self.base_layers, blend: 1.0 });
        let action_layer = |action: &'a ActionAnim| {
            let clips = built.clips.get(&action.id)?;
            let clip = clips.get(action.variant? % clips.len().max(1))?;
            Some(AnimLayer { clip, ms: action.ms.min(clip.duration), layers: action.layers, blend: ao_formats::character::animation_blend(action.ms, clip.duration, action.fade_ms) })
        };
        let before = self.actions.iter().filter(move |action| action.priority > priority).filter_map(action_layer);
        let after = self.actions.iter().filter(move |action| action.priority <= priority).filter_map(action_layer);
        before.chain(base).chain(after)
    }

    fn refresh_layers(&mut self) {
        let priority = self.base_priority();
        let mut state = (i32::MAX, 0, 0);
        let split = self.actions.partition_point(|action| action.priority > priority);
        let (before, after) = self.actions.split_at_mut(split);
        for action in before {
            action.layers = ao_formats::character::animation_layer_mask(&mut state, action.priority, u32::from(action.layer == canim::Layer::Upper), action.layers);
        }
        self.base_layers = ao_formats::character::animation_layer_mask(&mut state, priority, 0, self.base_layers);
        for action in after {
            action.layers = ao_formats::character::animation_layer_mask(&mut state, action.priority, u32::from(action.layer == canim::Layer::Upper), action.layers);
        }
    }
}

/// GC 0x1006fb56 sets speed at Play; DS 0x10072fde advances absolute milliseconds.
fn advance_clip_clock(ms: &mut f32, rate: &mut f32, dt: f32, start_rate: impl FnOnce() -> f32) {
    if *ms == 0.0 {
        *rate = start_rate();
    }
    *ms += (dt * 1000.0 * *rate).abs();
}

/// A corpse, vending machine, door, ... (everything that is not a `SimpleChar`): a fixed model at a fixed place.
struct Prop {
    /// `ActorFrame::id`, disjoint from the character instance ids.
    id: u32,
    key: u64,
    parent: Option<ao_net::msg::Identity>,
    /// Per-instance wire name (`DynelBase+0x6c`), independent of shared appearance.
    name: Option<String>,
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

/// A sound multimap: key -> Sandy sound ids.
type SoundTable = Vec<(u32, Vec<u32>)>;


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
    /// Characters whose fight controller is in state 2 (`SimpleChar+0x1d4 +0x44`, fed by [`Dynels::set_fighting`]): they stand in the weapon's
    /// fight idle (list 0x10), everyone else in the idle of the equip routine (`canim::peace_idle`).
    fighting: HashSet<i32>,
    /// Holders whose weapon resolved since the last [`Dynels::take_wielded`] (a weapon wielded during a fight runs the fight idle update again).
    wielded: Vec<i32>,
    /// Weapon-slot tables of the characters: stat `DamageType` of the item behind an `AttackInfo` slot (the hit lines of the chat log).
    pub arms: super::combat::arms::Armory,
    /// (swing clip, `ItemDelay`) of the last [`Dynels::pick_swing`] of a character: the clip plays sped up by [`canim::swing_speed_scale`].
    swing_delay: HashMap<i32, (u32, i32)>,
    /// (clip, playback rate) of the last [`Dynels::react_to_hit`] of a character.
    once_rate: HashMap<i32, (u32, f32)>,
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
    scene_generation: u64,
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
    calibration: super::avatar::Calibration,
    /// `PlayGameSound` calls of doors and characters (fight sounds) since the last [`Dynels::take_sounds`].
    sounds: Vec<GameSound>,
    /// Sounds that wait for their delay (`PlayGameSound`'s delay argument, the material impact sounds): (seconds left, sound).
    later: Vec<(f32, GameSound)>,
    /// Authored holder notes retain the playback node's slot, not the most recent swing's slot.
    notes: Vec<(i32, super::combat::notes::FiredNote)>,
    /// Persistent slot +0x30 damage / +0x2c hit kind (`FUN_1006a8f3`); specials never write these.
    slot_hits: HashMap<(i32, i32), (i32, i32)>,
    effects: Option<super::combat::effects::Renderer>,
    pending_effects: Vec<ao_net::n3::effects::Effects>,
    /// Visual spell applications wait until the own animated connectors are available.
    nano_visuals: Vec<ao_net::n3::spells::ApplySpells>,
    nano_handles: Vec<(ao_net::msg::Identity, ao_net::n3::spells::Spell, u32)>,
    buff_nanos: HashMap<i32, super::own_nanos::OwnNanos>,
    buff_visuals: HashMap<(i32, i32), buffs::BuffVisual>,
    nano_casts: Vec<(i32, ao_net::n3::dynel::CastNanoSpell)>,
    casting: Vec<NanoCast>,
    nano_animations: Vec<Option<(u32, bool)>>,
    nano_sounds: Vec<(u32, [f32; 3], f32, f32, u32, i32)>,
    nano_store: Option<RecordStore>,
    nano_templates: HashMap<i32, Arc<ao_formats::dynel_visual::ItemTemplate>>,
    pub nano_effect_categories: u32,
    /// Retail effect category bits: muzzle=8, tracers/hits=2.
    pub weapon_effect_categories: u32,
    /// Active swing target and clip slot, independent of the retained slot flags.
    swings: HashMap<i32, (i32, i32)>,
    impact_locations: HashMap<i32, i32>,
    /// Camera position of the last [`Dynels::update_with_collision`] (scene space).
    cam: [f32; 3],
}

struct NanoCast {
    who: i32,
    spell: i32,
    target: i32,
    handle: u32,
    remaining: f32,
    finish: [i32; 2],
    release_anim: u32,
    released: bool,
    release_seen: bool,
    done: bool,
    instant: bool,
    finish_enabled: bool,
    start_effect: i32,
}

/// GC 10050ed9 (asm 10050f85–10051064), in seconds.
fn nano_cast_delay(delay: i32, minimum: i32, initiative: i32, agg_def: i32, flags: i32) -> f32 {
    if flags & 0x80000 != 0 { return 0.0; }
    let reduction = if initiative <= 1200 { initiative as f32 * 0.5 } else { (initiative - 1200) as f32 / 6.0 + 600.0 };
    let duration = ((delay as f32 - reduction - agg_def as f32) / 100.0).max(0.0);
    if minimum == 1_234_567_890 { duration } else { duration.max(minimum as f32 / 100.0) }
}

/// Argument conversion of GC 100a5083 / 100a78c6 / 100a8c03.
fn nano_visual(spell: &ao_net::n3::spells::Spell) -> Option<(i32, i32, super::combat::effects::EffectConfig)> {
    use super::combat::effects::{Creation, EffectConfig};
    let (effect, attractor, duration) = match spell.function {
        0xcf26 => (spell.stat(0x27), 0, spell.stat(0x31)),
        0xcf57 => (spell.stat(0x57), 0, spell.stat(0x19)),
        0xcfd4 => (spell.stat(0x27), spell.stat(0x56), spell.stat(0x31)),
        _ => return None,
    };
    let mut config = EffectConfig { creation: Creation::Dynel, duration: (duration != 0).then_some(duration as f32 / 100.0), ..Default::default() };
    if spell.function == 0xcfd4 {
        config.repetitions = Some(spell.stat(0xa1) as u32);
        config.start_color = Some([0x99, 0x9a, 0x9b, 0x9c].map(|id| spell.stat(id) as f32 / 255.0));
        config.stop_color = Some([0x9d, 0x9e, 0x9f, 0xa0].map(|id| spell.stat(id) as f32 / 255.0));
        config.scale = Some(1.0 + spell.stat(0x32) as f32 / 100.0);
    }
    Some((effect, attractor, config))
}

/// GC100a5083/100a8c03 retry another overload only for a native null.
fn spawn_spell_visual(mut spawn: impl FnMut(super::combat::effects::Creation) -> anyhow::Result<u32>) -> anyhow::Result<u32> {
    use super::combat::effects::Creation;
    for creation in [Creation::Unlocated,Creation::Dynel,Creation::HitLocation] {
        let handle=spawn(creation)?;
        if handle!=0 {return Ok(handle);}
    }
    Ok(0)
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
            fighting: HashSet::new(),
            wielded: vec![],
            arms: Default::default(),
            swing_delay: HashMap::new(),
            once_rate: HashMap::new(),
            weapons: HashMap::new(),
            pending_weapons: vec![],
            pending_clips: vec![],
            replay: vec![],
            want_placed: None,
            playfield: None,
            models: HashMap::new(),
            asked: HashSet::new(),
            scene_generation: 0,
            show_all_names: false,
            char_view_distance: 80.0,
            lens: Lens::default(),
            tags: TagLayer::default(),
            listing: Listing::default(),
            rng: CrtRand::new(1),
            calibration: Default::default(),
            sounds: vec![],
            later: vec![],
            notes: vec![],
            slot_hits: HashMap::new(),
            effects: None,
            pending_effects: vec![],
            nano_visuals: vec![],
            nano_handles: vec![],
            buff_nanos: HashMap::new(),
            buff_visuals: HashMap::new(),
            nano_casts: vec![],
            casting: vec![],
            nano_animations: vec![],
            nano_sounds: vec![],
            nano_store: None,
            nano_templates: HashMap::new(),
            nano_effect_categories: 36,
            weapon_effect_categories: 10,
            swings: HashMap::new(),
            impact_locations: HashMap::new(),
            cam: [0.0; 3],
        }
    }
}

/// The weapon stance clip (AbstractAnimID) of a wielder in `state`: the equip routine's idle out of a fight (`None` = the plain `idle-stand`), the
/// weapon's list-0x10 idle in a fight, the rifle's constant walk / run (`canim::{peace_idle, fight_idle, wield_walk_run}`).
fn stance_id(set: Option<i32>, state: AnimState, fighting: bool) -> Option<u16> {
    match state {
        AnimState::Idle if fighting => canim::fight_idle(set),
        AnimState::Idle => Some(canim::peace_idle(set)).filter(|&i| i != canim::IDLE_STAND),
        AnimState::Walk => canim::wield_walk_run(set).map(|w| w.0),
        AnimState::Run => canim::wield_walk_run(set).map(|w| w.1),
        _ => None,
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
    #[cfg(test)]
    pub fn npc_movement_probe(&self, id: i32) -> Option<AnimState> {
        self.chars.get(&id).filter(|c| c.npc).map(|c| c.pose.anim)
    }

    /// The selected variant and clock actually sampled by the last NPC frame.
    #[cfg(test)]
    pub fn npc_animation_probe(&self, id: i32) -> Option<String> {
        let c = self.chars.get(&id).filter(|c| c.npc)?;
        let Model::Ready { built, .. } = self.models.get(&c.key)? else { return None };
        let rig = built.rig.as_ref()?;
        let a = built.clips.get(&c.anim)?.get(c.roll.variant)?;
        let once = !matches!(c.special, Special::None | Special::Cast(_));
        Some(format!(
            "npc={id} name={:?} clip={:#x} source_id={} clip_root={:?} duration_ms={:.3} loopspan_ms={:?} clip_ms={:.3} pose_ms={:.3} rate={:.6} model={} scale={:.3} movement={:?} status={:?}",
            c.name, c.anim, a.source_id, a.root, a.duration, super::avatar::loop_span(a),
            c.clip_ms, super::avatar::clip_time(a, c.clip_ms, once), c.clip_rate,
            rig.model_id, c.scale, c.pose.anim, c.mover.status(),
        ))
    }

    /// Starts the model builder for the client at `dir`; `own` = the player's own instance id (drawn by the avatar code).
    pub fn start(&mut self, dir: PathBuf, own: i32) {
        self.own = own;
        self.rng = CrtRand::new(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_secs() as u32));
        self.arms = super::combat::arms::Armory::open(&dir);
        self.calibration = super::avatar::Calibration::load(&dir);
        self.nano_store = match RecordStore::open(&dir) {
            Ok(store) => Some(store),
            Err(error) => { eprintln!("nano templates: {error:#}"); None }
        };
        self.nano_templates.clear();
        self.effects = match super::combat::effects::Renderer::open(&dir) {
            Ok(renderer) => Some(renderer),
            Err(error) => { eprintln!("weapon effects: {error:#}"); None }
        };
        self.dir = Some(dir);
    }

    /// Forgets every dynel (a playfield change, `Zone::reset_world`).
    pub fn clear(&mut self) {
        self.chars.clear();
        self.slot_hits.clear();
        self.later.clear();
        self.arms.clear();
        self.swings.clear();
        self.impact_locations.clear();
        self.nano_visuals.clear();
        self.pending_effects.clear();
        self.nano_handles.clear();
        self.buff_nanos.clear();
        self.buff_visuals.clear();
        self.nano_casts.clear();
        self.casting.clear();
        self.nano_animations.clear();
        self.nano_sounds.clear();
        if let Some(effects) = &mut self.effects { effects.clear(); }
        self.props.clear();
        self.weapons.clear();
        self.wield.clear();
        self.fighting.clear();
        self.wielded.clear();
        self.pending_weapons.clear();
    }

    /// `AnimSet` of the weapon a character wields, slot 6 (right hand) before 8 (left): the weapon whose lists the idle / walk / run stance uses
    /// (AnimHolder idle update `FUN_1003cad0`, docs/zone/combat-anim.md §4). `None`: nothing wielded or the item is not resolved yet.
    pub fn wielded_set(&self, id: i32) -> Option<i32> {
        self.wield.get(&id)?.iter().flatten().next().map(|w| w.set)
    }

    /// `id`'s fight controller changed state (`FUN_10069c68` start / `FUN_10068b7f` stop, `CombatEvent::FightStarted` / `FightStopped`): a
    /// fighting wielder stands in the weapon's fight idle, a character out of a fight in the equip routine's idle.
    pub fn set_fighting(&mut self, id: i32, fighting: bool) {
        if fighting {
            self.fighting.insert(id);
        } else {
            self.fighting.remove(&id);
        }
    }

    /// Holders whose weapon finished resolving since the last call (the worker's `Resp::Weapon`): `FUN_1006a700` runs the AnimHolder idle update
    /// (`FUN_1003cad0`, the draw) for a weapon wielded while the holder fights.
    pub fn take_wielded(&mut self) -> Vec<i32> {
        std::mem::take(&mut self.wielded)
    }

    /// `CharacterActionIIR_t` 0x61 (`FUN_1006a857` -> `FUN_1006a772`): body slot `slot` (6 right hand, 8 left) of `holder` is emptied. The weapon dynel
    /// stays alive in the inventory (its `WeaponItemFullUpdate` then names the bag slot), so no `n3ToClientQuit` follows.
    fn unwield_slot(&mut self, holder: i32, slot: i32) {
        let Some(hand) = (match slot {
            6 => Some(0),
            8 => Some(1),
            _ => None,
        }) else {
            return;
        };
        self.weapons.retain(|_, w| *w != (holder, hand));
        self.pending_weapons.retain(|w| (w.0, w.1) != (holder, hand));
        if let Some(w) = self.wield.get_mut(&holder) {
            w[hand] = None;
        }
    }

    /// A new playfield: every dynel of the old one is gone (models stay built); the dynels the playfield places itself
    /// (doors, terminals, ...: rdb 1000026, `CreateRDBDynels`) are created for `id`.
    pub fn on_playfield(&mut self, id: u32) {
        self.clear();
        self.playfield = Some(id);
        self.want_placed = Some(id);
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

    /// Original producer's layer wins over the generic name-table mapping.
    pub fn play_action(&mut self, id: i32, anim_id: u32, layer: canim::Layer, priority: i32) {
        self.start_node(id, ActionAnim { id: anim_id, layer, priority, layers: 3, fade_ms: 200.0, ms: 0.0, variant: None, note_fired: 0, rate: 1.0, key: None, slot: -1, delay: None });
    }

    fn start_node(&mut self, id: i32, mut node: ActionAnim) {
        let Some(c) = self.chars.get_mut(&id).filter(|c| !matches!(c.special, Special::Die(_))) else { return };
        let Look::Char(look) = &c.look else { return };
        if node.layer == canim::Layer::Body && matches!(c.special, Special::Once(_)) {
            let outgoing = ActionAnim {
                id: c.anim, layer: canim::Layer::Body, priority: -1, layers: c.base_layers, fade_ms: 200.0,
                ms: c.clip_ms, variant: Some(c.roll.variant), note_fired: c.note_fired,
                rate: c.clip_rate, key: None, slot: -1, delay: None,
            };
            let at = c.actions.partition_point(|action| action.priority >= -1);
            c.actions.insert(at, outgoing);
        }
        let previous = c.actions.iter().rev().find(|action| action.priority == node.priority).and_then(|action| {
            let Model::Ready { built, .. } = self.models.get(&c.key)? else { return None };
            let clips = built.clips.get(&action.id)?;
            let clip = clips.get(action.variant? % clips.len().max(1))?;
            Some((clip.duration, action.ms))
        }).unwrap_or((0.0, 0.0));
        node.fade_ms = ao_formats::character::animation_fade_ms(previous.0, previous.1);
        let anim_id = node.id;
        let at = c.actions.partition_point(|action| action.priority >= node.priority);
        if node.layer == canim::Layer::Body && c.special != Special::None {
            c.special = Special::None;
            c.note_fired = 0;
        }
        c.actions.insert(at, node);
        if anim_id != ATTACK_KEY && !matches!(self.models.get(&c.key), Some(Model::Ready { built, .. }) if built.clips.contains_key(&anim_id))
            && !self.pending_clips.iter().any(|pending| pending.0 == c.key && pending.2 == anim_id && pending.3 == id)
            && !self.replay.contains(&(id, anim_id)) {
            self.pending_clips.push((c.key, look.clone(), anim_id, id));
        }
        c.refresh_layers();
    }

    /// Holder list keys remain busy while any matching positive-count node is alive.
    /// `None` uses the preloaded NPC fallback attack without requesting an abstract id for its cache key.
    pub fn play_swing(&mut self, id: i32, anim: Option<u32>, key: u16) {
        let Some(c) = self.chars.get(&id) else { return };
        if matches!(c.special, Special::Die(_)) || c.actions.iter().any(|action| action.key == Some(key)) { return }
        let anim = anim.unwrap_or(ATTACK_KEY);
        let slot = self.note_slot(id).unwrap_or(-1);
        let delay = self.swing_delay.get(&id).filter(|entry| entry.0 == anim).map(|entry| entry.1);
        self.start_node(id, ActionAnim { id: anim, layer: canim::Layer::Body, priority: -1, layers: 3, fade_ms: 200.0, ms: 0.0, variant: None, note_fired: 0, rate: 1.0, key: Some(key), slot, delay });
    }

    pub fn pick_item_swing(&mut self, id: i32, special: i32, key: u16) -> Option<(u16, i32, u16)> {
        let (anim, resolved_key) = self.arms.item_animation(id, special, key, self.rng.rand())?;
        let delay = self.arms.swing_delay(id).unwrap_or(0);
        self.swing_delay.insert(id, (u32::from(anim), delay));
        Some((anim, delay, resolved_key))
    }

    /// The weapon swing of `id` (`FUN_10069acb` [GC 0x10069acb] + `FUN_1003c594`): a random value of list `key` of the `AnimSet` lists of the
    /// weapon in its right hand (else left) - list 0xb when the weapon has no such key. Returns (clip, ItemDelay in centiseconds, resolved list key);
    /// [`Dynels::play_swing`] uses the delay for speed scaling and the resolved key for duplicate suppression.
    /// `None`: nothing wielded, or an `AnimSet` whose lists live in the item record; the caller tries [`Dynels::pick_item_swing`].
    /// [GUESS] the wielder is never crawling (stat 0x1ae == 0xe is not tracked), so the crawl lists are not used.
    pub fn pick_swing(&mut self, id: i32, mut key: u16) -> Option<(u16, i32, u16)> {
        let hands = self.wield.get(&id)?;
        let hand = hands.iter().position(Option::is_some)?;
        let w = hands[hand]?;
        let mut list = canim::weapon_list(w.set, hand == 1, false, key);
        if list.is_empty() {
            key = canim::list::ATTACK;
            list = canim::weapon_list(w.set, hand == 1, false, canim::list::ATTACK);
        }
        let anim = *list.get(self.rng.rand() as usize % list.len().max(1))?;
        self.swing_delay.insert(id, (anim as u32, w.delay));
        Some((anim, w.delay, key))
    }

    /// `SimpleChar::GetImpactAnim` (vtable `+0x90` = `FUN_10058cfa` [GC 0x10058cfa]): a crawling character (stat 0x1ae == 0xe, not tracked here) plays
    /// 0xd0, everyone else a random one of `0x81, 0x82, 0x80, 0x84, 0x7f` (`rand() % 5`, the `imp-*` clips).
    pub fn impact_anim(&mut self) -> u16 {
        [0x81, 0x82, 0x80, 0x84, 0x7f][self.rng.rand() as usize % 5]
    }

    /// The struck character's reaction (`FUN_1009b4ac` [GC 0x1009b4ac] before its sounds): `Play(GetImpactAnim(), rate)` on `id` when its own `imp` clip
    /// is not already playing, at rate 1 for hit kind 4 (a crit), else `_DAT_1015d0a4` = 0.5. Returns the clip id and the rate.
    pub fn react_to_hit(&mut self, id: i32, flags: i32) -> (u16, f32) {
        let anim = self.impact_anim();
        self.set_impact_location(id, anim);
        let rate = if flags == 4 { 1.0 } else { 0.5 };
        self.once_rate.insert(id, (u32::from(anim), rate));
        self.play_once(id, u32::from(anim));
        (anim, rate)
    }

    /// `id` dies: the death clip `anim` (client animation id, `CharacterAction` 99's `identity_b.instance`; any other value =
    /// the generic death) plays once and holds.
    pub fn die(&mut self, id: i32, anim: u32) {
        self.cancel_nano_visuals(id);
        if let Some(c) = self.chars.get_mut(&id) {
            if !matches!(c.special, Special::Die(_)) {
                c.clip_ms = 0.0;
            }
            c.special = Special::Die(anim);
        }
    }

    /// Dynel stat-flag bit 0x10, represented by the character's death state.
    pub fn is_dead(&self, id: i32) -> bool {
        self.chars.get(&id).is_some_and(|c| matches!(c.special, Special::Die(_)))
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
        self.sounds.push(GameSound::at(sound, pos));
    }

    /// A named fight sound (`SM_Sandy_Game_Brawl` / `_Dimach`, `FUN_1003c594`) at character `id`.
    pub fn sound_at(&mut self, id: i32, name: &str) {
        let Some(c) = self.chars.get(&id) else { return };
        let pos = if id == self.own { self.cam } else { scene_pos(c.pose.pos) };
        self.sounds.push(GameSound::at(ao_audio::sbf::sound_id(name), pos));
    }

    /// An authored Sandy sound ID at the character, shared by confirmed item actions.
    pub fn sound_id_at(&mut self, id: i32, sound: u32) {
        if let Some(pos) = self.char_pos(id) {
            self.sounds.push(GameSound::at(sound, pos));
        }
    }

    /// An item callback can explicitly use the world origin (depleted-item key 0x32).
    pub fn sound_id_at_position(&mut self, sound: u32, pos: [f32; 3]) {
        self.sounds.push(GameSound::at(sound, pos));
    }

    /// Select an authored sound variant with the same CRT stream as other character sounds.
    pub fn sound_variants_at(&mut self, id: i32, sounds: &[u32]) {
        if let Some(pos) = self.char_pos(id) {
            self.sound_variants_at_position(sounds, pos);
        }
    }

    /// Item actions use the actor's explicit scene position, including the own actor without a dynel model.
    pub fn sound_variants_at_position(&mut self, sounds: &[u32], pos: [f32; 3]) {
        if let Some(sound) = self.pick_variant(sounds) {
            self.sound_id_at_position(sound, pos);
        }
    }

    /// Authored item animation and sound lists share the character CRT stream.
    pub fn pick_variant(&mut self, values: &[u32]) -> Option<u32> {
        match values.len() {
            0 => None,
            1 => Some(values[0]),
            n => Some(values[self.rng.rand() as usize % n]),
        }
    }

    /// Where a character's sounds play: the camera for the own character (the avatar is not a dynel model here), else its position.
    fn char_pos(&self, id: i32) -> Option<[f32; 3]> {
        let c = self.chars.get(&id)?;
        Some(if id == self.own { self.cam } else { scene_pos(c.pose.pos) })
    }

    /// `FUN_1004570c(key)` (@0x1004570c) on a sound multimap: a random value of the key's list (a CRT `rand` only when there is a choice).
    fn pick_of(&mut self, list: &[(u32, Vec<u32>)], key: u32) -> Option<u32> {
        let l = list.iter().find(|s| s.0 == key).map(|s| &s.1).filter(|l| !l.is_empty())?;
        Some(if l.len() > 1 { l[self.rng.rand() as usize % l.len()] } else { l[0] })
    }

    /// The NPC record of `id` as the fight sounds read it: `Some((sound table, FabricType))`; `None` for a player look, or while the model is not built.
    fn record_of(&self, id: i32) -> Option<(SoundTable, i32)> {
        let c = self.chars.get(&id)?;
        let Look::Char(look) = &c.look else { return None };
        let Some(Model::Ready { built, .. }) = self.models.get(&c.key).filter(|_| look.npc) else { return None };
        Some((built.sounds.clone(), built.fabric))
    }

    /// Counts the delayed sounds down; the due ones join [`Dynels::take_sounds`].
    fn tick_sounds(&mut self, dt: f32) {
        let mut due = Vec::new();
        self.later.retain_mut(|(t, s)| {
            *t -= dt;
            let keep = *t > 0.0;
            if !keep {
                due.push(*s);
            }
            keep
        });
        self.sounds.extend(due);
    }


    /// `FUN_1006a8f3` [GC 0x1006a8f3] stores the damage and the hit kind of an `AttackInfo` in the attacker's slot object (and the victim is its target).
    pub fn hit_seen(&mut self, attacker: i32, ctx: super::combat::notes::HitCtx) {
        self.slot_hits.insert((attacker, ctx.slot), (ctx.damage, ctx.flags));
        self.swings.insert(attacker, (ctx.victim, ctx.slot));
    }

    /// `FUN_1006a9c5` selects a special clip but retains the slot's previous flags and damage.
    pub fn special_hit_seen(&mut self, who: i32, victim: i32, slot: i32) {
        self.swings.insert(who, (victim, slot));
    }

    pub fn note_slot(&self, who: i32) -> Option<i32> {
        self.swings.get(&who).map(|swing| swing.1)
    }

    pub fn note_ctx(&self, who: i32, slot: i32) -> Option<super::combat::notes::HitCtx> {
        if slot < 0 { return None }
        let &(victim, _) = self.swings.get(&who)?;
        let (damage, flags) = self.slot_hits.get(&(who, slot)).copied().unwrap_or((0, 0));
        Some(super::combat::notes::HitCtx { victim, slot, damage, flags })
    }

    pub fn note_target(&mut self, who: i32, victim: i32) {
        if let Some(swing) = self.swings.get_mut(&who) {
            swing.0 = victim;
        }
    }

    pub fn set_impact_location(&mut self, victim: i32, anim: u16) {
        let anchor = match anim { 0x7f => 1001, 0x81 => 1007, 0x82 => 1008, 0x84 => 1000, _ => 1006 };
        self.impact_locations.insert(victim, anchor);
    }

    fn effect_anchor(&self, who: i32, anchor: i32, slot: i32) -> Option<glam::Mat4> {
        let c = self.chars.get(&who)?;
        let Model::Ready { built, .. } = self.models.get(&c.key)? else { return None };
        let world = glam::Mat4::from_scale_rotation_translation(glam::Vec3::splat(c.scale), glam::Quat::from_rotation_y(scene_yaw(c.pose.yaw)), glam::Vec3::from(scene_pos(c.pose.pos)));
        if anchor==0 {return Some(world);}
        let rig=built.rig.as_ref()?;
        let layers = c.layers(built);
        let matrix = if anchor == 3000 { rig.weapon_effect_anchor_composed(if slot == 8 { 2 } else { 1 }, layers) } else { rig.effect_anchor_composed(anchor, layers) };
        matrix.map(|matrix| world * glam::Mat4::from_cols_array_2d(&matrix))
    }

    /// Raw geometry lookup; locator callers separately apply GC1010603b's actual-root fallback.
    fn item_effect_anchor(&self, identity: ao_net::msg::Identity, attractor: i32) -> Option<glam::Mat4> {
        if attractor!=0 {return None;}
        let prop = self.props.get(&(identity.kind, identity.instance))?;
        let Model::Ready { .. } = self.models.get(&prop.key)? else { return None };
        Some(glam::Mat4::from_scale_rotation_translation(glam::Vec3::splat(prop.scale), glam::Quat::from_rotation_y(scene_yaw(prop.yaw)), glam::Vec3::from(scene_pos(prop.pos))))
    }

    fn item_effect_locator(&self, identity: ao_net::msg::Identity, attractor: i32) -> Option<glam::Mat4> {
        self.item_effect_anchor(identity,attractor).or_else(||self.item_effect_anchor(identity,0))
    }

    pub(in crate::play) fn cancel_nano_visuals(&mut self, who: i32) {
        self.cancel_buff_visuals(who);
        self.nano_sounds.retain(|sound| sound.5 != who);
        self.nano_casts.retain(|(caster, cast)| *caster != who && !(cast.target.kind == CHAR_KIND && cast.target.instance == who));
        self.nano_visuals.retain(|application| application.target.kind != CHAR_KIND || application.target.instance != who);
        if who == self.own { self.nano_animations.push(None); }
        if let Some(c) = self.chars.get_mut(&who).filter(|c| matches!(c.special, Special::Cast(_))) {
            c.special = Special::None;
            c.clip_ms = 0.0;
        }
        let mut renderer = self.effects.take();
        self.casting.retain(|cast| {
            if cast.who != who && cast.target != who { return true; }
            self.nano_sounds.retain(|sound| sound.5 != cast.who);
            let charge = self.nano_templates.get(&cast.spell).and_then(|template| template.stat(0x178)).unwrap_or(203) as u32;
            self.pending_clips.retain(|clip| clip.3 != cast.who || (clip.2 != charge && clip.2 != cast.release_anim));
            self.replay.retain(|clip| clip.0 != cast.who || (clip.1 != charge && clip.1 != cast.release_anim));
            if cast.who == self.own { self.nano_animations.push(None); }
            else if let Some(c) = self.chars.get_mut(&cast.who).filter(|c| matches!(c.special, Special::Cast(_)) || c.special == Special::Once(cast.release_anim)) {
                c.special = Special::None;
                c.clip_ms = 0.0;
            }
            if let Some(renderer) = &mut renderer { renderer.delete(cast.handle); }
            false
        });
        self.nano_handles.retain(|(target, _, handle)| {
            if target.kind != CHAR_KIND || target.instance != who { return true; }
            if let Some(renderer) = &mut renderer { renderer.delete(*handle); }
            false
        });
        self.effects = renderer;
    }

    /// GC 1004f504: spell0 selects the first pending cast; this never removes applied buffs.
    fn cancel_nano_cast(&mut self, who: i32, spell: i32) {
        let spell = if spell != 0 { Some(spell) } else {
            self.casting.iter().find(|cast| cast.who == who && !cast.done).map(|cast| cast.spell)
                .or_else(|| self.nano_casts.iter().find(|(caster, _)| *caster == who).map(|(_, cast)| cast.spell))
        };
        let Some(spell) = spell else { return };
        self.nano_casts.retain(|(caster, cast)| *caster != who || cast.spell != spell);
        self.casting.retain(|cast| {
            if cast.who != who || cast.spell != spell || cast.done { return true; }
            self.nano_sounds.retain(|sound| sound.5 != who);
            let charge = self.nano_templates.get(&cast.spell).and_then(|template| template.stat(0x178)).unwrap_or(203) as u32;
            self.pending_clips.retain(|clip| clip.3 != who || (clip.2 != charge && clip.2 != cast.release_anim));
            self.replay.retain(|clip| clip.0 != who || (clip.1 != charge && clip.1 != cast.release_anim));
            if who == self.own { self.nano_animations.push(None); }
            else if let Some(c) = self.chars.get_mut(&who).filter(|c| matches!(c.special, Special::Cast(_)) || c.special == Special::Once(cast.release_anim)) {
                c.special = Special::None;
                c.clip_ms = 0.0;
            }
            if let Some(renderer) = &mut self.effects { renderer.delete(cast.handle); }
            false
        });
    }


    /// GC 100a5083 / 100a78c6 / 100a8c03: spell visual handlers use category 0x20.
    pub fn apply_nano_visuals(&mut self, application: ao_net::n3::spells::ApplySpells) {
        self.nano_visuals.push(application);
    }

    /// Native CharCastNano calls: id, scene position, duration override, volume, selector, emitter.
    pub fn take_nano_sounds(&mut self) -> Vec<(u32, [f32; 3], f32, f32, u32, i32)> {
        std::mem::take(&mut self.nano_sounds)
    }

    fn nano_sound(&mut self, spell: i32, selector: u32, source: (i32, Option<[f32; 3]>), duration: f32, volume: f32) {
        let (who, own_pos) = source;
        let Some(sound) = self.nano_templates.get(&spell).and_then(|t| t.stat(selector)).filter(|&id| id != 0) else { return };
        let Some(character) = self.chars.get(&who) else { return };
        // The own character is simulated by Player, not the remote-character mover.
        let pos = if who == self.own { own_pos.unwrap_or_else(|| scene_pos(character.pose.pos)) } else { scene_pos(character.pose.pos) };
        self.nano_sounds.push((sound as u32, pos, duration, volume, selector, who));
    }

    /// Nano states use the single body clip, not the combat holder's positive-count history.
    fn nano_clip(&mut self, who: i32, animation: u32, looping: bool) {
        let Some(character) = self.chars.get_mut(&who).filter(|c| !matches!(c.special, Special::Die(_))) else { return };
        character.special = Special::None;
        character.clip_ms = 0.0;
        character.note_fired = 0;
        character.roll.key = None;
        self.pending_clips.retain(|clip| clip.3 != who);
        self.replay.retain(|clip| clip.0 != who);
        self.once_rate.remove(&who);
        self.play_once(who, animation);
        if looping {
            if let Some(character) = self.chars.get_mut(&who) { character.special = Special::Cast(animation); }
        }
    }

    pub fn take_nano_animations(&mut self) -> Vec<Option<(u32, bool)>> {
        std::mem::take(&mut self.nano_animations)
    }
    pub fn refresh_effect_anchors(&mut self, mut own_anchor: impl FnMut(i32) -> Option<glam::Mat4>) {
        let Some(mut renderer) = self.effects.take() else { return };
        renderer.refresh_anchors(|identity, id| {
            if identity.0 != CHAR_KIND as u32 { return self.item_effect_anchor(ao_net::msg::Identity { kind: identity.0 as i32, instance: identity.1 as i32 }, id); }
            if identity.1 as i32 == self.own { own_anchor(id) }
            else { self.effect_anchor(identity.1 as i32, id, 0) }
        });
        self.effects = Some(renderer);
    }


    pub fn nano_visual_frame(&mut self, dt: f32, own_finished: bool, mut own_anchor: impl FnMut(i32) -> Option<[[f32; 4]; 4]>, mut stat: impl FnMut(i32, u32) -> Option<i32>) {
        use super::combat::effects::{Binding, Creation, EffectConfig, HitLocationRequest};
        let own_pos = own_anchor(0).map(|m| glam::Mat4::from_cols_array_2d(&m).w_axis.truncate().to_array());
        let mut renderer = self.effects.take();
        let anchor = |world: &Self, who, id, own_anchor: &mut dyn FnMut(i32) -> Option<[[f32; 4]; 4]>| {
            if who == world.own { own_anchor(id).map(|m| glam::Mat4::from_cols_array_2d(&m)) }
            else { world.effect_anchor(who, id, 0) }
        };
        let appearance = |who, stat: &mut dyn FnMut(i32, u32) -> Option<i32>| -> Option<[i32; 4]> {
            Some([stat(who, 4)?, stat(who, 59)?, stat(who, 47)?, stat(who, 360)?])
        };
        self.nano_handles.retain(|(_, _, handle)| renderer.as_ref().is_some_and(|r| r.is_active(*handle)));
        for (who, cast) in std::mem::take(&mut self.nano_casts) {
            if !self.nano_templates.contains_key(&cast.spell) {
                let Some(store) = &self.nano_store else { continue };
                let result = (|| -> anyhow::Result<_> {
                    let record = store.get(super::hud_nanodb::NANO_RDB_TYPE, u32::try_from(cast.spell)?)?.context("missing nano template")?;
                    ao_formats::dynel_visual::parse_item_template(&record)
                })();
                match result {
                    Ok(template) => { self.nano_templates.insert(cast.spell, Arc::new(template)); }
                    Err(error) => { eprintln!("nano cast: {error:#}"); continue; }
                }
            }
            let template = Arc::clone(&self.nano_templates[&cast.spell]);
            let target = if cast.target.kind == 0 && cast.target.instance == 0 && template.stat(0).unwrap_or(0) & 0x8000 == 0 { who } else { cast.target.instance };
            self.casting.retain(|old| { if old.who == who && old.spell == cast.spell { if let Some(r) = &mut renderer { r.delete(old.handle); } false } else { true } });
            let remaining = nano_cast_delay(template.stat(0x126).unwrap_or(200), template.stat(0x20b).unwrap_or(1_234_567_890), stat(who, 0x95).unwrap_or(0), stat(who, 0x33).unwrap_or(0), template.stat(0).unwrap_or(1));
            let instant = template.stat(0).unwrap_or(1) & 0x80000 != 0;
            let release_anim = template.stat(if target == who { 0x17a } else { 0x179 }).unwrap_or(if target == who { 202 } else { 201 }) as u32;
            self.casting.push(NanoCast { who, spell: cast.spell, target, handle: 0, remaining, release_anim, released: instant, release_seen: false, done: false, instant, finish_enabled: !instant || cast.flag, start_effect: if instant || self.nano_effect_categories & 4 == 0 { 49999 } else { template.stat(0x1ac).unwrap_or(49999) }, finish: if !instant || cast.flag { [template.stat(0x19e).unwrap_or(49999), template.stat(0x169).unwrap_or(49999)] } else { [49999; 2] } });
            if !instant {
                self.nano_sound(cast.spell, 0x10d, (who, own_pos), 0.2, 0.6);
                let animation = template.stat(0x178).unwrap_or(203) as u32;
                if who == self.own { self.nano_animations.push(Some((animation, true))); }
                else { self.nano_clip(who, animation, true); }
            }
            else if who == self.own { self.nano_animations.push(Some((release_anim, false))); }
            else { self.nano_clip(who, release_anim, false); }
        }
        let mut casting = std::mem::take(&mut self.casting);
        for cast in &mut casting {
            if !cast.released && cast.start_effect != 49999 && cast.start_effect != 0 {
                if let Some(r) = &mut renderer {
                    let effect = cast.start_effect;
                    let attractor = r.attractor(effect, 0).unwrap_or(0);
                    if let (Some(source), Some(destination)) = (anchor(self, cast.who, attractor, &mut own_anchor).or_else(||anchor(self,cast.who,0,&mut own_anchor)), anchor(self, cast.target, 0, &mut own_anchor)) {
                        r.prepare_anchors((CHAR_KIND as u32, cast.who as u32), |identity, id| anchor(self, identity.1 as i32, id, &mut own_anchor));
                        r.prepare_anchors((CHAR_KIND as u32, cast.target as u32), |identity, id| anchor(self, identity.1 as i32, id, &mut own_anchor));
                        match r.spawn_configured(Binding { group: 0, attractor, effect, note: 0, color: 0 }, source, destination.w_axis.truncate(), EffectConfig { creation: super::combat::effects::Creation::Dynel, duration: Some(6000.0), track_source: true, source_identity: Some((CHAR_KIND as u32, cast.who as u32)), target_identity: Some((CHAR_KIND as u32, cast.target as u32)), source_appearance: appearance(cast.who, &mut stat), target_appearance: appearance(cast.target, &mut stat), ..Default::default() }) {
                            Ok(handle) => { cast.handle = handle; cast.start_effect = 49999; }
                            Err(error) => { eprintln!("nano cast: {error:#}"); cast.start_effect = 49999; }
                        }
                    }
                }
            }
            if !cast.released && !cast.instant {
                cast.remaining -= dt;
                if cast.remaining > 0.0 {
                    self.nano_sound(cast.spell, 0x10d, (cast.who, own_pos), 0.2, 1.0);
                    continue;
                }
                if let Some(r) = &mut renderer { r.next_state(cast.handle); }
                cast.released = true;
                if cast.who == self.own { self.nano_animations.push(Some((cast.release_anim, false))); }
                else {
                    self.nano_clip(cast.who, cast.release_anim, false);
                    cast.release_seen = self.chars.get(&cast.who).is_some_and(|c| c.special == Special::Once(cast.release_anim));
                }
                continue;
            }
            let finished = if cast.who == self.own {
                !self.nano_animations.contains(&Some((cast.release_anim, false))) && own_finished
            } else {
                self.chars.get(&cast.who).is_some_and(|c| {
                    if c.special == Special::Once(cast.release_anim) { cast.release_seen = true; }
                    cast.release_seen && c.special == Special::None
                })
            };
            if !finished && !cast.done {
                self.nano_sound(cast.spell, 0x10f, (cast.who, own_pos), 0.2, 1.0);
            }
            if finished && !cast.done {
                cast.done = true;
                if cast.finish_enabled {
                    self.nano_sound(cast.spell, 0x110, (cast.target, own_pos), 0.0, 1.0);
                }
            }
            if cast.done {
                if self.nano_effect_categories & 4 == 0 { cast.finish = [49999; 2]; }
                if let Some(r) = &mut renderer {
                    for effect in &mut cast.finish {
                        if *effect == 0 || *effect == 49999 { continue; }
                        if let Some(attractor) = r.attractor(*effect, 0) {
                            if let Some(source) = anchor(self, cast.target, attractor, &mut own_anchor).or_else(||anchor(self,cast.target,0,&mut own_anchor)) {
                                let binding = Binding { group: 0, attractor, effect: *effect, note: 0, color: 0 };
                                let identity = Some((CHAR_KIND as u32, cast.target as u32));
                                let profile = appearance(cast.target, &mut stat);
                                let config = EffectConfig { creation: super::combat::effects::Creation::Dynel, track_source: true, source_identity: identity, target_identity: identity, source_appearance: profile, target_appearance: profile, ..Default::default() };
                                r.prepare_anchors((CHAR_KIND as u32, cast.target as u32), |identity, id| anchor(self, identity.1 as i32, id, &mut own_anchor));
                                if let Err(error) = r.spawn_configured(binding, source, source.w_axis.truncate(), config) { eprintln!("nano release: {error:#}"); }
                                *effect = 49999;
                            }
                        }
                    }
                }
            }
        }
        casting.retain(|cast| !cast.done || cast.finish.iter().any(|&effect| effect != 0 && effect != 49999) || renderer.as_ref().is_some_and(|r| r.is_active(cast.handle)));
        self.casting = casting;
        let Some(mut renderer) = renderer else { return };
        for application in std::mem::take(&mut self.nano_visuals) {
            let who = application.target.instance;
            for spell in application.spells {
                let Some((effect, explicit, mut config)) = nano_visual(&spell) else { continue };
                // GC 100a78c6 rejects non-control characters (+0x140 == 0).
                if spell.function == 0xcf57 && (application.target.kind != CHAR_KIND || who != self.own) { continue; }
                if !application.apply {
                    self.nano_handles.retain(|(target, original, handle)| {
                        if *target == application.target && *original == spell { renderer.delete(*handle); false } else { true }
                    });
                    continue;
                }
                if self.nano_effect_categories & 32 == 0 { continue; }
                let Some(attractor) = renderer.attractor(effect, explicit) else { continue };
                let source = if application.target.kind == CHAR_KIND { anchor(self, who, attractor, &mut own_anchor).or_else(||anchor(self,who,0,&mut own_anchor)) } else { self.item_effect_locator(application.target, attractor) };
                let binding = Binding { group: 0, attractor, effect, note: 0, color: 0 };
                config.source_identity = Some((application.target.kind as u32, who as u32));
                config.track_source = true;
                config.source_nonvisual = if application.target.kind == CHAR_KIND {anchor(self,who,0,&mut own_anchor).is_none()} else {self.item_effect_anchor(application.target,0).is_none()};
                config.target_identity = config.source_identity;
                config.source_appearance = (application.target.kind == CHAR_KIND).then(|| appearance(who, &mut stat)).flatten();
                config.target_appearance = config.source_appearance;
                renderer.prepare_anchors((application.target.kind as u32, who as u32), |identity, id| if identity.0 == CHAR_KIND as u32 { anchor(self, identity.1 as i32, id, &mut own_anchor) } else { self.item_effect_anchor(application.target, id) });
                let mut spawn = |creation| {
                    let mut attempt=config;
                    attempt.creation=creation;
                    match creation {
                        Creation::Unlocated => {
                            attempt.track_source=false;
                            attempt.source_nonvisual=false;
                            attempt.source_identity=None;
                            attempt.target_identity=None;
                            // This overload has no position argument; the Rust matrix is unused.
                            renderer.spawn_configured(binding,glam::Mat4::IDENTITY,glam::Vec3::ZERO,attempt)
                        }
                        Creation::Dynel => {
                            // An actual nonvisual dynel still allocates its native control. Its
                            // missing locator must not become a factory null or visible origin.
                            let matrix=source.unwrap_or(glam::Mat4::IDENTITY);
                            renderer.spawn_configured(binding,matrix,matrix.w_axis.truncate(),attempt)
                        }
                        Creation::HitLocation => {
                            let caster=(CHAR_KIND as u32,self.own as u32);
                            renderer.prepare_anchors(caster,|identity,id|anchor(self,identity.1 as i32,id,&mut own_anchor));
                            let hit=renderer.new_hit_location(HitLocationRequest {
                                source:(application.target.kind as u32,who as u32),target:caster,
                                source_attractor:3001,target_attractor:1006,hit:true,
                            },|identity,id| if identity==caster {anchor(self,self.own,id,&mut own_anchor)}
                                else if identity.0==CHAR_KIND as u32 {anchor(self,identity.1 as i32,id,&mut own_anchor)}
                                else {self.item_effect_anchor(application.target,id)});
                            let Some(hit)=hit else {return Ok(0)};
                            attempt.track_source=false;
                            attempt.source_nonvisual=false;
                            attempt.hit_location_handle=Some(hit);
                            attempt.hit_location=renderer.sample_hit_location(hit);
                            attempt.target_identity=Some(caster);
                            attempt.target_appearance=appearance(self.own,&mut stat);
                            let Some((start,end))=attempt.hit_location else {return Ok(0)};
                            renderer.spawn_configured(binding,glam::Mat4::from_translation(start),end,attempt)
                        }
                        _ => unreachable!("spell factory overload"),
                    }
                };
                let result=if spell.function==0xcf57 {spawn(Creation::Dynel)} else {spawn_spell_visual(spawn)};
                match result {
                    Ok(0) => {},
                    Ok(handle) => {
                        self.nano_handles.push((application.target, spell, handle));
                    }
                    Err(error) => eprintln!("nano effects: {error:#}"),
                }
            }
        }
        self.effects = Some(renderer);
    }

    /// Visual effects use the actor's actual animated connector, never `char_pos`'s
    /// own-character sound/camera shortcut.
    pub fn note_effects(&mut self, who: i32, note: u32, slot: i32, mut own_anchor: impl FnMut(i32, i32) -> Option<[[f32; 4]; 4]>) {
        use super::combat::{effects::{Binding, Creation, EffectConfig, HitLocationRequest}, notes::id};
        if !matches!(note, id::ATTACK | id::ATTACK_EFFECT_1..=id::ATTACK_EFFECT_4) { return; }
        let Some(h) = self.note_ctx(who, slot) else { return };
        let (victim, slot, hit) = (h.victim, h.slot, h.flags > 1);
        let Some(item) = self.arms.slot_item(who, slot) else { return };
        let mut bindings = item.effects.clone();
        if hit && !bindings.iter().any(|b| b.group == 2) {
            bindings.push(Binding { group: 2, attractor: 0, effect: 62002, note: 0, color: 0 });
        }
        let Some(mut renderer) = self.effects.take() else { return };
        let mut anchor = |id, a, slot| {
            if id == self.own { own_anchor(a, slot).map(|m| glam::Mat4::from_cols_array_2d(&m)) }
            else { self.effect_anchor(id, a, slot) }
        };
        renderer.prepare_anchors((CHAR_KIND as u32,who as u32),|identity,id| anchor(identity.1 as i32,id,slot));
        renderer.prepare_anchors((CHAR_KIND as u32,victim as u32),|identity,id| anchor(identity.1 as i32,id,0));
        for binding in bindings.into_iter().filter(|b| b.fires(note as i32, hit)) {
            let category = if binding.group == 0 { 8 } else { 2 };
            if self.weapon_effect_categories & category == 0 { continue; }
            let source_anchor = if binding.group == 0 {
                renderer.attractor(binding.effect, binding.attractor).unwrap_or(0)
            } else if binding.attractor != 0 { binding.attractor } else { 2000+i32::from(slot == 8) };
            let Some(source) = anchor(who, source_anchor, slot).or_else(||anchor(who,0,slot)) else { continue };
            let target_anchor = self.impact_locations.get(&victim).copied().unwrap_or(1000);
            let target = anchor(victim, target_anchor, 0).or_else(||anchor(victim,0,0));
            if binding.group != 0 && target.is_none() { continue; }
            let position = target.map_or(source.w_axis.truncate(), |m| m.w_axis.truncate());
            let origin = if binding.group == 2 { target.unwrap_or(source) } else { source };
            #[cfg(test)]
            if std::env::var_os("AOMAC_COMBAT_LOG").is_some() {
                eprintln!("live weapon effect note={note:#x} who={who} victim={victim} slot={slot} effect={} group={} source_anchor={source_anchor} target_anchor={target_anchor} source={:?} origin={:?} target={position:?}", binding.effect, binding.group, source.w_axis.truncate(), origin.w_axis.truncate());
            }
            // GC1009ad7d: muzzle uses the dynel overload; tracer and impact both
            // allocate NewHitLocation and use CreateEffect2(effect, hit_handle).
            let hit_location_handle = if binding.group == 0 { None } else {
                renderer.new_hit_location(HitLocationRequest {
                    source:(CHAR_KIND as u32,who as u32),target:(CHAR_KIND as u32,victim as u32),
                    source_attractor:source_anchor,target_attractor:target_anchor,hit,
                },|identity,id| anchor(identity.1 as i32,id,if identity.1 as i32==who {slot}else{0}))
            };
            if binding.group != 0 && hit_location_handle.is_none() {continue;}
            let config = EffectConfig {
                creation: if binding.group == 0 { Creation::Dynel } else { Creation::HitLocation },
                hit_location: hit_location_handle.and_then(|handle|renderer.sample_hit_location(handle)),
                hit_location_handle,
                source_identity: Some((CHAR_KIND as u32, who as u32)),
                target_identity: Some((CHAR_KIND as u32, victim as u32)),
                source_attractor: (binding.group == 0).then_some(binding.attractor),
                track_source: binding.group == 0,
                ..Default::default()
            };
            if let Err(error) = renderer.spawn_configured(binding, origin, position, config) { eprintln!("weapon effects: {error:#}"); }
        }
        self.effects = Some(renderer);
    }

    /// One animation note of `who`'s swing clip (`FUN_10045069` [GC 0x10045069], `combat::notes`).
    pub fn note_sounds(&mut self, who: i32, note: u32, slot: i32) {
        use super::combat::notes::id;
        match note {
            id::SWISH_PUNCH..=id::SWISH_HUGE => self.swish(who, note),
            id::ATTACK_START_1..=id::ATTACK_START_9 => self.record_note(who, note),
            id::ATTACK | id::ATTACK_EFFECT_1..=id::ATTACK_EFFECT_4 => {
                if let Some(h) = self.note_ctx(who, slot) {
                    self.weapon_hit(who, note, h);
                }
            }
            _ => {}
        }
    }

    /// The notes the swing clips of the other characters fired since the last call: (character, note id).
    pub fn take_notes(&mut self) -> Vec<(i32, super::combat::notes::FiredNote)> {
        std::mem::take(&mut self.notes)
    }

    /// Notes `swish_punch` .. `swish_huge` (0x73..0x76, `FUN_10045069`): a player plays `SM_Sandy_Swish_*`, a creature a value of its record's list
    /// `note`, at the character (material 0, plain size).
    fn swish(&mut self, who: i32, note: u32) {
        let (Some(at), Some(npc)) = (self.char_pos(who), self.chars.get(&who).map(|c| c.npc)) else { return };
        let id = if npc {
            self.record_of(who).and_then(|(sounds, _)| self.pick_of(&sounds, note))
        } else {
            super::combat::notes::swish_name(note).map(ao_audio::sbf::sound_id)
        };
        self.sounds.extend(id.map(|id| GameSound::at(id, at)));
    }

    /// Notes `attack_start_N` (0x77..0x7f): creature attack sounds, a value of the NPC record's list `note` at the character; players play nothing.
    fn record_note(&mut self, who: i32, note: u32) {
        let Some(at) = self.char_pos(who) else { return };
        let id = self.record_of(who).and_then(|(sounds, _)| self.pick_of(&sounds, note));
        self.sounds.extend(id.map(|id| GameSound::at(id, at)));
    }

    /// An `attack` / `attack_effect_N` note of `who`'s swing clip with `key` = the note id: `FUN_100688f9` [GC 0x100688f9] on the item behind the
    /// attack slot ([`Armory::slot_item`](super::combat::arms::Armory::slot_item)), docs/zone/combat-anim.md section 6.
    /// 1. The weapon's own sound at the attacker, plain `PlayGameSound`: `FUN_1009cd68` (wielded `WeaponItem_t` only: list `key`),
    ///    `FUN_1009b4ac` (list `key`, else `0xb`; only when the hit kind `flags` is > 1) and `FUN_1009cc50` (wielded: list `key`, else for `0xb` the sound of
    ///    the item's `AmmoType`). Identical ids start once (a sound that is already playing is not restarted).
    /// 2. With damage and a hit kind > 1 (`FUN_1009b4ac`): at the victim, after 0.4 s, the sound the material selects. A creature (NPC record) plays
    ///    the weapon's list `0x1f` (the weapon's impact sound, with the record's `FabricType` 1..=17 as the material) and its own record list `0x1f`;
    ///    a player of breed 1, 2, 3, 4 or 7 plays `Male/FemaleGetsHit` as material 7. The impact size comes from the damage ([`notes::impact_size`]).
    fn weapon_hit(&mut self, who: i32, key: u32, h: super::combat::notes::HitCtx) {
        use super::combat::notes;
        let Some(item) = self.arms.slot_item(who, h.slot).cloned() else { return };
        let Some(at) = self.char_pos(who) else { return };
        let mut ids: Vec<u32> = Vec::new();
        if item.wielded {
            ids.extend(self.pick_of(&item.sounds, key));
        }
        if h.flags > 1 {
            let own = self.pick_of(&item.sounds, key);
            ids.extend(own.or_else(|| if key == notes::id::ATTACK { None } else { self.pick_of(&item.sounds, notes::id::ATTACK) }));
        }
        if item.wielded {
            let own = self.pick_of(&item.sounds, key);
            ids.extend(own.or_else(|| if key == notes::id::ATTACK { notes::ammo_default(item.ammo) } else { None }));
        }
        for (i, id) in ids.iter().enumerate() {
            if !ids[..i].contains(id) {
                self.sounds.push(GameSound::at(*id, at));
            }
        }
        if h.flags <= 1 || h.damage < 1 {
            return;
        }
        let (Some(vpos), Some(v)) = (self.char_pos(h.victim), self.chars.get(&h.victim)) else { return };
        let Look::Char(look) = &v.look else { return };
        let (npc, breed, sex) = (look.npc, look.breed, look.sex);
        let size = notes::impact_size(h.damage);
        let (mut material, mut sound) = (0, 0u32);
        if npc {
            let Some((sounds, fabric)) = self.record_of(h.victim) else { return };
            (material, sound) = (fabric, self.pick_of(&sounds, canim::npc_sound::HIT).unwrap_or(0));
            if (1..=17).contains(&material) {
                if let Some(w) = self.pick_of(&item.sounds, canim::npc_sound::HIT) {
                    self.later.push((notes::IMPACT_DELAY_S, GameSound { id: w, pos: vpos, material, size }));
                }
            }
        }
        if material == 0 {
            if let Some((m, name)) = notes::player_impact(breed, sex) {
                material = m;
                sound = name.map_or(sound, ao_audio::sbf::sound_id);
            }
        }
        if sound != 0 && (1..=17).contains(&material) {
            self.later.push((notes::IMPACT_DELAY_S, GameSound { id: sound, pos: vpos, material, size }));
        }
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
        self.props.insert((kind, instance), Prop { id, key: look.key(), parent: None, name: None, pos, yaw: rot.map_or(0.0, |q| quat_yaw(&q)), scale, submitted: false, anim: PropAnim::default(), stats });
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

    /// Real-template fixture through the production build path, without renderer upload.
    #[cfg(test)]
    pub(super) fn test_template_prop(&mut self, who: ao_net::msg::Identity, template: u32, store: &RecordStore, mut stats: Vec<(u32, i32)>) -> anyhow::Result<()> {
        stats.push((23, template as i32));
        let look = Look::Item { template: Some(template), stats };
        let built = build(store, &mut ActorAssets::new(store)?, &look)?;
        self.add_prop(who.kind, who.instance, look, [0.0; 3], None, 1.0);
        let key = self.props[&(who.kind, who.instance)].key;
        self.models.insert(key, Model::Ready { built: Box::new(built), uploaded: false });
        Ok(())
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
    /// `N3Msg_isIDOnGround`: an existing unparented world item.
    pub fn on_ground(&self, who: ao_net::msg::Identity) -> bool {
        self.props.get(&(who.kind, who.instance)).is_some_and(|p| p.parent.is_none())
    }

    /// Wire instance name, otherwise a parent character's or the built template's name.
    pub fn name_of(&self, kind: i32, instance: i32) -> Option<&str> {
        let p = self.props.get(&(kind, instance))?;
        if let Some(name) = p.name.as_deref() {
            return Some(name);
        }
        if let Some(parent) = p.parent.filter(|p| p.kind == CHAR_KIND) {
            if let Some(c) = self.chars.get(&parent.instance).filter(|c| !c.name.is_empty()) {
                return Some(&c.name);
            }
        }
        match self.models.get(&p.key) {
            Some(Model::Ready { built, .. }) => built.name.as_deref(),
            _ => None,
        }
    }

    /// Original use dispatch calls the item's runtime vtable, not its wire identity kind.
    pub fn item_class_of(&self, kind: i32, instance: i32) -> Option<u32> {
        let prop = self.props.get(&(kind, instance))?;
        match self.models.get(&prop.key) {
            Some(Model::Ready { built, .. }) => Some(built.item_kind.unwrap_or(kind as u32)),
            _ => None,
        }
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

    pub fn take_effect_sounds(&mut self) -> impl Iterator<Item = super::combat::effects::AuxSound> + '_ {
        self.effects.iter_mut().flat_map(|effects| effects.take_aux_sounds())
    }

    pub fn effect_camera_offset(&mut self, eye: glam::Vec3) -> anyhow::Result<glam::Vec3> {
        self.effects.as_mut().map_or(Ok(glam::Vec3::ZERO), |effects| effects.camera_offset(eye))
    }
    /// GUI world feedback creates the authored Font control on the real dynel.
    pub fn floating_text(&mut self, who:i32, text:&str, color:u32, mut own_anchor:impl FnMut(i32)->Option<glam::Mat4>) -> anyhow::Result<()> {
        use anyhow::Context;
        use super::combat::effects::{Binding,Creation,EffectConfig};
        let Some(mut renderer)=self.effects.take() else {return Ok(())};
        let result=(|| {
            let attractor=renderer.attractor(12122,0).context("missing floating text effect 12122")?;
            let mut resolve=|_: (u32,u32),id| if who==self.own {own_anchor(id)}else{self.effect_anchor(who,id,0)};
            let identity=(CHAR_KIND as u32,who as u32);
            renderer.prepare_anchors(identity,&mut resolve);
            let source=resolve(identity,attractor).or_else(||resolve(identity,0)).context("missing floating text visual dynel")?;
            let [a,r,g,b]=color.to_be_bytes().map(|v|v as f32/255.0);
            let handle=renderer.spawn_configured(Binding {group:0,attractor,effect:12122,note:0,color:0},source,source.w_axis.truncate(),EffectConfig {creation:Creation::Dynel,track_source:true,source_identity:Some(identity),start_color:Some([r,g,b,a]),stop_color:Some([r,g,b,a]),..Default::default()})?;
            renderer.set_text(handle,text.as_bytes())
        })();
        self.effects=Some(renderer);
        result
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
        if matches!(&m.body, N3::Misc(Misc::ToClientQuit)) {
            if let Some(effects) = &mut self.effects { effects.source_deleted((who.kind as u32, who.instance as u32)); }
            self.nano_visuals.retain(|application| application.target != who);
            self.nano_handles.retain(|(target, _, handle)| {
                if *target != who { return true; }
                if let Some(effects) = &mut self.effects { effects.delete(*handle); }
                false
            });
        }
        self.buff_message(m);
        self.arms.on_message(m, self.chars.get(&who.instance).is_some_and(|c| c.npc));
        match &m.body {
            N3::Effects(effect) => self.pending_effects.push(effect.clone()),
            N3::World(World::VendingMachine(v)) => {
                let stats = v.base.stats.iter().map(|&(i, x)| (i, x)).collect::<Vec<_>>();
                let template = static_instance(&stats);
                let scale = stat_scale(&stats);
                let parent = (v.base.parent.kind != 0).then_some(v.base.parent);
                let pos = v.base.position.or_else(|| parent.and_then(|p| self.chars.get(&p.instance).map(|c| c.pose.pos))).unwrap_or([0.0; 3]);
                self.add_prop(who.kind, who.instance, Look::Item { template, stats }, pos, v.base.rotation, scale);
                self.props.get_mut(&(who.kind, who.instance)).unwrap().parent = parent;
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
                            animation: c.spells.iter().find(|s| s.type_id == 0xcf27 && s.args[3] != 0 && s.args[4] == 4).and_then(|s| Some((u32::try_from(s.args[5]).ok()?, u32::try_from(s.args[2]).ok()?))),
                        });
                        if let Some(pos) = c.base.position {
                            self.add_prop(who.kind, who.instance, look, pos, c.base.rotation, v.scale);
                            self.set_stats(who, stats.iter().copied());
                            let name = c.base.name();
                            self.props.get_mut(&(who.kind, who.instance)).unwrap().name = (!name.is_empty()).then_some(name);
                            // A corpse replaces its character even when the full update beats the quit.
                            if let Some(effects) = &mut self.effects { effects.source_deleted((c.owner.kind as u32, c.owner.instance as u32)); }
                            self.cancel_nano_visuals(c.owner.instance);
                            self.chars.remove(&c.owner.instance);
                        }
                    }
                    Err(e) => eprintln!("dynels: corpse {}: {e:#}", who.instance),
                }
            }
            N3::World(World::CharacterAction(a)) if who.kind == CHAR_KIND && canim::death_anim_from_action(a.action, a.identity_b.instance as u32).is_some() => {
                self.die(who.instance, a.identity_b.instance as u32)
            }
            N3::World(World::CharacterAction(a)) if who.kind == CHAR_KIND && a.action == ACTION_UNWIELD => self.unwield_slot(who.instance, a.identity_b.instance),
            N3::World(World::CharacterAction(a)) if who.kind == CHAR_KIND && matches!(a.action, 0x66 | 0x6c) => {
                self.cancel_nano_cast(who.instance, if a.action == 0x66 { 0 } else { a.identity_b.kind });
            }
            N3::World(World::CharacterAction(a)) if who.kind == CHAR_KIND && a.action == 0x75 && a.identity_a.kind == CHAR_KIND && self.chars.contains_key(&a.identity_a.instance) => {
                self.cancel_nano_cast(who.instance, a.identity_b.instance);
            }
            N3::World(World::CharacterAction(a)) if who.kind == CHAR_KIND && a.action == 0x89 => {
                // GC1004f274 clears the queue without holder+0x10: stage0 releases,
                // with no result to copy; an already released controller keeps its cached result.
                self.nano_casts.retain(|(caster, _)| *caster != who.instance);
                for cast in self.casting.iter_mut().filter(|cast| cast.who == who.instance && !cast.released) {
                    cast.remaining = 0.0;
                    cast.finish_enabled = false;
                    cast.finish = [49999; 2];
                }
            }
            N3::Dynel(Dynel::CastNanoSpell(cast)) if who.kind == CHAR_KIND => self.nano_casts.push((who.instance, cast.clone())),
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
            // `AppearanceUpdateIIR_c::Activate` for any character (`FUN_10058e36` only tests the identity kind 50000): cloth, stat 0x2A1 `VisualFlags`,
            // attractors; the model is rebuilt through the worker (the look's key changes) and replaces the drawn one when it is ready
            N3::World(World::Appearance(a)) if who.kind == CHAR_KIND => {
                if let Some(c) = self.chars.get_mut(&who.instance) {
                    if let Look::Char(l) = &mut c.look {
                        l.apply_appearance(a);
                        let key = c.look.key();
                        c.next = Some(key).filter(|&k| k != c.key);
                    }
                    c.visual_flags = i32::from(a.visual_flags);
                }
            }
            N3::Unknown(body) if m.header.msg_type == ao_net::n3::server_move::RELOCATE && who.kind == CHAR_KIND && self.chars.contains_key(&who.instance) => {
                if let Ok(r) = ao_net::n3::server_move::parse_relocate(body) {
                    let parent = (r.parent != ao_net::msg::Identity::default()).then_some(r.parent);
                    for child in r.children {
                        if child.kind == CHAR_KIND {
                            if let Some(c) = self.chars.get_mut(&child.instance) {
                                c.parent = parent;
                            }
                        } else if let Some(p) = self.props.get_mut(&(child.kind, child.instance)) {
                            p.parent = parent;
                        }
                    }
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
                        scale: u.monster_scale.max(20) as f32 / 100.0,
                        side: u.side,
                        flags: u.flags2 as i32,
                        visual_flags: u.visual_flags as i32,
                        in_play: u.flags & ao_net::n3::dynel::flag::IN_PLAY != 0,
                        parent: u.parent.filter(|p| *p != ao_net::msg::Identity::default()),
                        pose,
                        anim: 0x78,
                        special: if u.max_health > 0 && u.health <= 0 { Special::Die(DIE_KEY) } else { Special::None },
                        actions: vec![],
                        base_layers: 3,
                        clip_ms: 0.0,
                        clip_rate: 1.0,
                        terminal_pose: false,
                        submitted: false,
                        parts: vec![],
                        features_set: !u.is_npc(),
                        bounds: None,
                        roll: Roll::default(),
                        next: None,
                        note_fired: 0,
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
                        if stat == super::zone::IN_PLAY_STAT as i32 {
                            c.in_play = value != 0;
                        }
                    }
                }
                if s.stats.iter().any(|&(stat, value)| stat == 0x1b && value <= 0) && self.chars.get(&who.instance).is_some_and(|c| !matches!(c.special, Special::Die(_))) {
                    self.die(who.instance, DIE_KEY);
                }
            }
            N3::Misc(Misc::CharInPlay) => {
                if let Some(c) = self.chars.get_mut(&who.instance) {
                    c.in_play = true;
                }
            }
            N3::Misc(Misc::FollowTarget(f)) => {
                if let Some(c) = self.chars.get_mut(&who.instance) {
                    c.mover.on_follow_target(f);
                }
            }
            N3::Misc(Misc::ToClientQuit) => {
                self.cancel_nano_visuals(who.instance);
                self.chars.remove(&who.instance);
                self.swing_delay.remove(&who.instance);
                self.once_rate.remove(&who.instance);
                self.swings.remove(&who.instance);
                self.slot_hits.retain(|&(attacker, _), _| attacker != who.instance);
            }
            _ => {}
        }
    }

    /// Integrates the movement of every character by `dt` seconds (the drawn pose and animation state).
    pub fn advance(&mut self, dt: f32) {
        for c in self.chars.values_mut() {
            if !matches!(c.special, Special::Die(_)) {
                c.pose = c.mover.advance(dt);
            }
        }
    }

    /// Scene replacement can arrive after the playfield notification and erase already queued uploads.
    fn sync_scene(&mut self, host: &Host) {
        if self.scene_generation == host.scene_generation() {
            return;
        }
        self.scene_generation = host.scene_generation();
        for model in self.models.values_mut() {
            if let Model::Ready { uploaded, .. } = model {
                *uploaded = false;
            }
        }
        for prop in self.props.values_mut() {
            prop.submitted = false;
        }
    }

    pub fn shared_native_fog(&self) -> Option<ao_render::SharedNativeFog> {
        self.effects.as_ref().map(super::combat::effects::Renderer::shared_native_fog)
    }

    pub fn set_effect_source_runtime(&mut self, identity: (u32, u32), body_scale: f32, breed: i32, vehicle_speed: Option<f32>, vehicle_direction: i32, visible: bool) {
        if let Some(renderer) = &mut self.effects {
            renderer.set_source_runtime(identity, body_scale, breed, vehicle_speed, vehicle_direction, visible);
        }
    }
    pub fn set_effect_source_head_height(&mut self, identity: (u32, u32), height: Option<f32>) {
        if let Some(renderer) = &mut self.effects { renderer.set_source_native_head_height(identity, height); }
    }
    pub fn set_effect_source_liquid(&mut self, identity: (u32, u32), liquid: Option<(f32, u32, glam::Vec3)>) {
        if let Some(renderer) = &mut self.effects { renderer.set_source_native_liquid(identity, liquid); }
    }
    pub fn set_effect_source_torso_factor(&mut self, identity: (u32, u32), factor: f32) {
        if let Some(renderer) = &mut self.effects { renderer.set_source_torso_factor(identity, factor); }
    }

    pub fn set_effect_collision(&mut self, collision: Option<std::rc::Rc<std::cell::RefCell<ao_formats::playfield::collision::Collision>>>) {
        if let Some(renderer) = &mut self.effects { renderer.set_collision(collision); }
    }


    pub fn refresh_effect_source_runtime(&mut self) {
        let Some(renderer) = &mut self.effects else { return };
        for (&id, c) in &self.chars {
            if id == self.own { continue; }
            let breed = match &c.look { Look::Char(look) => i32::from(look.breed), _ => 0 };
            renderer.set_source_runtime((CHAR_KIND as u32, id as u32), c.scale, breed, Some(c.mover.vehicle_speed()), c.mover.vehicle_direction(), c.in_play);
            if let Some(Model::Ready { built, .. }) = self.models.get(&c.key) {
                if let Some(rig) = &built.rig {
                    let factor = (rig.cat().torso_sphere.radius as f64 / 0.250_750_005_245_208_74 * c.scale as f64) as f32;
                    renderer.set_source_torso_factor((CHAR_KIND as u32, id as u32), factor);
                    renderer.set_source_native_head_height((CHAR_KIND as u32, id as u32), rig.head_attractor_composed(c.layers(built)).map(|p| p[1] * c.scale));
                }
            }
        }
    }

    /// Advances the dynels and hands the visible ones to the renderer. `cam` = camera position in scene space, `fwd` = its view direction.
    /// The loaded playfield surface capability; absent geometry is not a no-hit query.
    pub fn update_with_collision(
        &mut self, dt: f32, cam: [f32; 3], _fwd: [f32; 3], host: &mut Host,
        collision: Option<&mut dyn FnMut(ao_render::Vec3) -> Option<(ao_render::Vec3, ao_render::Vec3)>>,
        own_anchor: impl FnMut(i32) -> Option<glam::Mat4>,
    ) {
        self.sync_scene(host);
        self.cam = cam;
        self.tick_sounds(dt);
        self.refresh_effect_source_runtime();
        let Some(dir) = self.dir.clone() else {
            self.refresh_effect_anchors(own_anchor);
            if let Some(effects) = &mut self.effects { effects.frame(dt, host, collision); }
            return;
        };
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
                        if let Some(c) = self.chars.get_mut(who).filter(|c| c.key == key) {
                            let available = matches!(self.models.get(&key), Some(Model::Ready { built, .. }) if built.clips.contains_key(&id));
                            if c.actions.iter().any(|action| action.id == id) {
                                if !available { c.actions.retain(|action| action.id != id); }
                                if available { c.refresh_layers(); }
                                continue;
                            }
                            if available && c.special == Special::None {
                                c.special = Special::Once(id);
                                c.clip_ms = 0.0;
                            }
                        }
                    }
                    self.replay.retain(|r| r.1 != id);
                }
                // an answer for a hand that was emptied meanwhile (unwield before the worker resolved the item) is stale
                Resp::Weapon { holder, slot, wield } => {
                    if self.weapons.values().any(|&w| w == (holder, slot)) {
                        if wield.is_some() {
                            self.wielded.push(holder);
                        }
                        self.wield.entry(holder).or_default()[slot] = wield;
                    }
                }
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
            if *id != own && !self.asked.contains(&c.next.unwrap_or(c.key)) {
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
        for (&(kind, instance), p) in &mut self.props {
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
            let actor = ActorFrame { id: p.id, model: p.key, transform, parts, skin, always: false, alpha: 1.0, ..Default::default() };
            if let Some(effects) = &mut self.effects {
                let identity = (kind as u32, instance as u32);
                if effects.needs_source_mesh(identity) { effects.prepare_source_mesh(identity, &built.model, &actor); }
            }
            host.actors.push(actor);
        }
        self.advance(dt);
        for (id, c) in &mut self.chars {
            if *id == own {
                continue;
            }
            let id_ref = id;
            if let Some(k) = c.next {
                match self.models.get(&k) {
                    Some(Model::Ready { .. }) => (c.key, c.next) = (k, None),
                    Some(Model::Failed) => c.next = None,
                    _ => {}
                }
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
            // clip: death / attack override the movement state; walking and running scale the clip with the real speed
            let state = c.pose.anim;
            let mut movement_rate = c.special == Special::None;
            let (key, list, rate) = match c.special {
                Special::Cast(k) => (k, built.clips.get(&k), 1.0),
                Special::Die(k) => match built.clips.get(&k).or_else(|| built.clips.get(&DIE_KEY)) {
                    Some(a) => (k, Some(a), 1.0),
                    None => (0x78, clip_of(built, state).map(|x| x.1), 1.0),
                },
                Special::Once(k) => match built.clips.get(&k) {
                    // `FUN_1006a239`: a weapon swing is sped up so its first note lands within the weapon's ItemDelay
                    Some(a) => (
                        k,
                        Some(a),
                        // a hit reaction plays at its own rate, a weapon swing is sped up for the weapon delay
                        self.once_rate.get(id_ref).filter(|s| s.0 == k).map(|s| s.1).unwrap_or_else(|| {
                            self.swing_delay.get(id_ref).filter(|s| s.0 == k).map_or(1.0, |s| canim::swing_speed_scale(a.first().and_then(|a| a.events.first()).map_or(0.0, |e| e.0 as f32), s.1))
                        }),
                    ),
                    None => (0x78, clip_of(built, state).map(|x| x.1), 1.0),
                },
                Special::None => {
                    let (id, a) = clip_of(built, state).map_or((0x78, None), |(i, a)| (i, Some(a)));
                    // a wielder: the idle of the equip routine (rifle / bazooka list 0x29) out of a fight, the weapon idle (list 0x10) in a fight, the
                    // rifle's constant walk / run (`canim::{peace_idle, fight_idle, wield_walk_run}`, docs/zone/combat-anim.md §4)
                    let set = self.wield.get(id_ref).and_then(|w| w.iter().flatten().next().map(|w| w.set));
                    let sid = stance_id(set, state, self.fighting.contains(id_ref));
                    let stance = sid.and_then(|sid| built.clips.get(&(sid as u32)).map(|a| (sid as u32, a)));
                    // Direct stance idle Play retains its authored 1.0 (GC 0x1003cc15 / 0x1003cad0).
                    if stance.is_some() && !c.mover.status().is_moving() {
                        movement_rate = false;
                    }
                    let (id, a) = stance.map_or((id, a), |(i, a)| (i, Some(a)));
                    (id, a, 1.0)
                }
            };
            // the variant is rolled when the clip starts, not while it loops
            let roll_key = (key, matches!(c.special, Special::None).then_some(state));
            let started = c.roll.key != Some(roll_key);
            let clip = list.filter(|l| !l.is_empty()).map(|l| &l[c.roll.pick(roll_key, l.len(), &mut self.rng)]);
            if key != c.anim || started {
                c.anim = key;
                c.base_layers = 3;
                c.refresh_layers();
                c.clip_ms = 0.0;
                c.note_fired = 0;
                c.terminal_pose = false;
            }
            // GC 0x1006be27 / 0x1006fb56 set the rate once after Play, including idle.
            advance_clip_clock(&mut c.clip_ms, &mut c.clip_rate, dt, || {
                if movement_rate {
                    let status = c.mover.status();
                    let reference = match status.mode {
                        Mode::Run if status.forward < 0 => 3.0,
                        Mode::Run => 5.0,
                        Mode::Swim => 3.0,
                        Mode::Fly => 7.0,
                        _ => 1.5,
                    };
                    super::avatar::anim_rate(
                        self.calibration.get(rig.model_id, key),
                        c.scale * 100.0,
                        max_speed(status.mode, status.forward < 0, c.mover.skill()),
                        reference,
                        false,
                    )
                } else { rate }
            });
            c.actions.retain_mut(|action| {
                let Some(clips) = built.clips.get(&action.id).filter(|clips| !clips.is_empty()) else { return true };
                let variant = *action.variant.get_or_insert_with(|| if clips.len() > 1 { self.rng.rand() as usize % clips.len() } else { 0 });
                let clip = &clips[variant % clips.len()];
                action.advance(clip, dt, |note| self.notes.push((*id, note)))
            });
            let dead = matches!(c.special, Special::Die(_));
            if let Some(a) = clip.filter(|_| !matches!(c.special, Special::None | Special::Cast(_))) {
                // Legacy emote/nano-release returns to movement; death holds its last frame.
                if c.clip_ms >= a.duration {
                    if matches!(c.special, Special::Once(_)) {
                        c.special = Special::None;
                        c.clip_ms = 0.0;
                    } else {
                        c.clip_ms = a.duration;
                    }
                }
            }
            if let Some(a) = clip.filter(|a| matches!(c.special, Special::None | Special::Cast(_)) && a.duration > 0.0 && c.clip_ms > 4.0 * a.duration) {
                c.clip_ms = super::avatar::clip_time(a, c.clip_ms, false);
            }
            // DS 0x10074bb4 samples the CAT clock each frame, not a distance-selected Hz.
            if dist > self.char_view_distance {
                c.submitted = false;
                continue;
            }
            let terminal = dead && clip.is_some_and(|a| c.clip_ms >= a.duration);
            let needs_effect_pose = self.effects.as_ref().is_some_and(|effects| effects.needs_source_pose((CHAR_KIND as u32, *id as u32)));
            let skin = if !terminal || !c.terminal_pose || !c.submitted || needs_effect_pose {
                c.terminal_pose = terminal;
                Some(rig.pose_composed(c.layers(built)))
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
            let actor = ActorFrame { id: *id as u32, model: c.key, transform, parts: c.parts.clone(), skin, always: false, alpha: 1.0, ..Default::default() };
            if let Some(effects) = &mut self.effects {
                let identity = (CHAR_KIND as u32, *id as u32);
                if effects.needs_source_mesh(identity) { effects.prepare_source_mesh(identity, &built.model, &actor); }
            }
            host.actors.push(actor);
        }
        self.refresh_effect_anchors(own_anchor);
        if let Some(effects) = &mut self.effects { effects.frame(dt, host, collision); }
        self.worker = Some(worker);
    }

    /// The tag of `id` for `kind` (`FUN_10024e14` text and colours, `FUN_10024d5b` position): the text of [`name_tag`], centred on the head
    /// anchor (`GetIndicatorPosition`: attractor 0 + 0.5 m, scaled with the dynel); `None` while the dynel has no model, or its anchor
    /// fails the `x > 0` test of `FUN_10024d5b`. `bar` = health bar inputs `((health, max), colour)`.
    fn tag_of(&self, id: i32, kind: IndicatorKind, bar: Option<((i32, i32), u32)>) -> Option<Tag> {
        let c = self.chars.get(&id)?;
        let Some(Model::Ready { built, .. }) = self.models.get(&c.key) else { return None };
        if !c.in_play || !built.visible || c.parent.is_some() || !tag_anchor_visible(c.pose.pos[0]) {
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
                chars.iter().filter(|(id, c)| **id != own && c.in_play && !marked(**id) && near(c) && ok(c)).map(|(id, _)| *id).collect()
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

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spell_visual_fallback_only_retries_native_null() {
        use super::super::combat::effects::Creation;
        let mut calls=Vec::new();
        assert_eq!(spawn_spell_visual(|creation| {
            calls.push(creation);
            Ok(if creation==Creation::HitLocation {42}else{0})
        }).unwrap(),42);
        assert_eq!(calls,[Creation::Unlocated,Creation::Dynel,Creation::HitLocation]);
        calls.clear();
        assert_eq!(spawn_spell_visual(|creation| {
            calls.push(creation);
            // A nonzero, already-terminated control is still a constructed control.
            Ok(if creation==Creation::Dynel {43}else{0})
        }).unwrap(),43);
        assert_eq!(calls,[Creation::Unlocated,Creation::Dynel]);
        calls.clear();
        assert!(spawn_spell_visual(|creation| {
            calls.push(creation);
            anyhow::bail!("real asset failure")
        }).is_err());
        assert_eq!(calls,[Creation::Unlocated]);
        assert_eq!(spawn_spell_visual(|_|Ok(0)).unwrap(),0);
    }

    #[test]
    fn nano_visual_arguments_and_cast_timing_follow_retail() {
        use ao_net::n3::spells::Spell;
        let spell = Spell { function: 0xcfd4, stats: [(0x27, 62002), (0x57, 2000), (0x56, 1006), (0x31, 250), (0x32, 25), (0xa1, 3), (0x99, 255), (0x9c, 128)].into(), ..Default::default() };
        let (effect, attractor, config) = nano_visual(&spell).unwrap();
        assert_eq!((effect, attractor), (62002, 1006));
        assert_eq!(config.duration, Some(2.5));
        assert_eq!(config.scale, Some(1.25));
        assert_eq!(config.repetitions, Some(3));
        assert_eq!(config.start_color, Some([1.0, 0.0, 0.0, 128.0 / 255.0]));
        let absent = 1_234_567_890;
        assert_eq!(nano_cast_delay(500, absent, 600, 0, 0), 2.0);
        assert_eq!(nano_cast_delay(1000, absent, 1800, 0, 0), 3.0);
        assert_eq!(nano_cast_delay(500, 200, 1200, 0, 0), 2.0);
        assert_eq!(nano_cast_delay(500, 200, 0, 0, 0x80000), 0.0);
        let mut world = Dynels::default();
        for who in [42, 43] {
            world.apply_nano_visuals(ao_net::n3::spells::ApplySpells { target: ao_net::msg::Identity { kind: CHAR_KIND, instance: who }, spells: vec![spell.clone()], apply: true });
            let identity = ao_net::msg::Identity { kind: CHAR_KIND, instance: who };
            let cast = ao_net::n3::dynel::CastNanoSpell { spell: 163449, target: identity, flag: true, source: identity, rest: vec![] };
            world.on_message(&Message { header: ao_net::n3::N3Header { msg_type: ao_net::n3::dynel::CAST_NANO_SPELL, target: identity, flag: 0 }, sender: who as u32, body: N3::Dynel(Dynel::CastNanoSpell(cast)) });
        }
        assert_eq!(world.nano_visuals.len(), 2);
        assert_eq!(world.nano_casts.len(), 2);
        world.own = 42;
        world.casting.push(NanoCast { who: 42, spell: 163449, target: 43, handle: 0, remaining: 2.0, finish: [49999; 2], release_anim: 201, released: false, release_seen: false, done: false, instant: false, finish_enabled: true, start_effect: 49999 });
        world.cancel_nano_visuals(43);
        assert!(world.casting.is_empty(), "target disappearance ends the caster's loop even without an effect renderer");
        assert_eq!(world.take_nano_animations(), [None]);
        world.clear();
        assert!(world.nano_visuals.is_empty());
        assert!(world.nano_casts.is_empty());
    }

    #[test]
    fn zone_routes_foreign_nano_visuals_and_undo() {
        use ao_net::msg::Identity;
        use ao_net::n3::{outgoing::n3_frame, spells};
        let mut zone = super::super::zone::Zone::new(42);
        let target = Identity { kind: CHAR_KIND, instance: 43 };
        let spell = spells::spell(0xcf26, &[(0x27, 62002)]);
        for apply in [true, false] {
            let mut writer = ao_net::wire::Writer::default();
            writer.u32(spells::APPLY_SPELLS);
            target.write(&mut writer);
            writer.u8(0);
            writer.0.extend(spells::encode(target, std::slice::from_ref(&spell), apply).unwrap());
            zone.on_frame(&n3_frame(0, 43, writer.0));
        }
        assert_eq!(zone.world.nano_visuals.len(), 2);
        assert!(zone.world.nano_visuals[0].apply);
        assert!(!zone.world.nano_visuals[1].apply);
        assert!(zone.own_events.is_empty());
        zone.world.cancel_nano_visuals(43);
        assert!(zone.world.nano_visuals.is_empty());
    }

    #[test]
    fn npc_clock_keeps_clip_start_rate_and_uses_absolute_milliseconds() {
        let (mut ms, mut rate) = (0.0, 1.0);
        // Idle uses the vehicle maximum too (GC 0x1006be27), not zero velocity.
        advance_clip_clock(&mut ms, &mut rate, 0.2, || super::super::avatar::anim_rate(0.9, 50.0, 1.5, 1.5, false));
        assert!((ms - 360.0).abs() < 0.001);
        advance_clip_clock(&mut ms, &mut rate, 0.2, || panic!("not reset until Play"));
        assert!((ms - 720.0).abs() < 0.001);
        ms = 0.0;
        advance_clip_clock(&mut ms, &mut rate, -0.1, || 0.5);
        assert_eq!(ms, 50.0);
        assert_eq!(rate, 0.5);
    }

    #[test]
    fn scene_replacement_invalidates_every_cached_model_even_after_zone_upload() {
        let mut world = Dynels::default();
        for key in 1..=3 {
            world.models.insert(key, Model::Ready { built: Box::new(plain(Default::default(), true)), uploaded: true });
        }
        world.models.insert(4, Model::Loading);
        world.models.insert(5, Model::Failed);
        world.on_playfield(4582);
        let mut host = Host::headless();
        world.sync_scene(&host);
        assert!(world.models.values().filter_map(|m| match m { Model::Ready { uploaded, .. } => Some(*uploaded), _ => None }).all(|u| u));
        // The asynchronous scene arrives after the playfield notification (and possibly an upload).
        host.set_scene(Default::default());
        world.sync_scene(&host);
        assert_eq!(world.models.len(), 5, "CPU models and pending builds survive zoning");
        assert!(world.models.values().filter_map(|m| match m { Model::Ready { uploaded, .. } => Some(*uploaded), _ => None }).all(|u| !u));
        if let Model::Ready { uploaded, .. } = world.models.get_mut(&1).unwrap() {
            *uploaded = true;
        }
        world.sync_scene(&host);
        assert!(matches!(world.models[&1], Model::Ready { uploaded: true, .. }), "same scene does not trigger repeated uploads");
        host.set_scene(Default::default());
        world.sync_scene(&host);
        assert!(matches!(world.models[&1], Model::Ready { uploaded: false, .. }));
    }

    #[test]
    fn item_use_class_follows_template_without_remapping_identity() {
        let mut world = Zone::new(1).world;
        let id = ao_net::msg::Identity { kind: 0xc748, instance: -1073737242 };
        world.test_prop(id, vec![]);
        assert_eq!(world.item_class_of(id.kind, id.instance), None);
        let key = world.props[&(id.kind, id.instance)].key;
        let mut built = plain(Default::default(), true);
        built.item_kind = Some(0xdac1);
        world.models.insert(key, Model::Ready { built: Box::new(built), uploaded: false });
        assert_eq!(world.item_class_of(id.kind, id.instance), Some(0xdac1));
        assert!(world.props.contains_key(&(id.kind, id.instance)));
        assert_eq!(world.item_class_of(0xdac1, id.instance), None);
        assert!(world.on_ground(id));
        world.props.get_mut(&(id.kind, id.instance)).unwrap().parent = Some(ao_net::msg::Identity { kind: CHAR_KIND, instance: 1 });
        assert!(!world.on_ground(id));
    }

    #[test]
    fn item_visual_missing_connector_uses_actual_root_but_nonvisual_dynel_does_not() {
        let mut world = Zone::new(1).world;
        let item = ao_net::msg::Identity { kind: 0xc73d, instance: 1 };
        world.test_prop(item, vec![]);
        world.props.get_mut(&(item.kind,item.instance)).unwrap().pos=[13.0,7.0,-4.0];
        let key = world.props[&(item.kind, item.instance)].key;
        world.models.insert(key, Model::Ready { built: Box::new(plain(Default::default(), true)), uploaded: false });
        let spell = ao_net::n3::spells::spell(0xcf26, &[(0x27, 71214)]);
        world.apply_nano_visuals(ao_net::n3::spells::ApplySpells { target: item, spells: vec![spell], apply: true });
        world.cancel_nano_visuals(1);
        assert_eq!(world.nano_visuals.len(), 1, "same-instance character teardown must not remove an item application");
        let root=world.item_effect_anchor(item,0).unwrap();
        assert!(world.item_effect_anchor(item,1000).is_none(),"raw missing bone remains absent");
        assert_eq!(world.item_effect_locator(item,1000),Some(root),"missing bone locator uses the actual item visual root");
        assert_eq!(world.item_effect_locator(item,3001),Some(root),"missing mesh connector locator uses the actual item visual root");
        world.models.remove(&key);
        assert!(world.item_effect_anchor(item,0).is_none(),"a dynel identity without a visual has no root fallback");
        assert!(world.item_effect_locator(item,3001).is_none(),"nonvisual dynels cannot synthesize an effect origin");
        world.clear();
        assert!(world.nano_visuals.is_empty());
    }

    #[test]
    fn authored_beacon_and_laboratory_spawn_on_their_real_receivers() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/rdb.db").exists() { return; }
        let mut world = Zone::new(1).world;
        world.own = 1;
        world.effects = Some(crate::play::combat::effects::Renderer::open(&dir).unwrap());
        let item = ao_net::msg::Identity { kind: 0xc73d, instance: 2 };
        world.test_prop(item, vec![(23, 288073)]);
        let key = world.props[&(item.kind, item.instance)].key;
        world.models.insert(key, Model::Ready { built: Box::new(plain(Default::default(), true)), uploaded: false });
        for (receiver, effect) in [(item, 71214), (ao_net::msg::Identity { kind: CHAR_KIND, instance: 1 }, 13600)] {
            let spell = ao_net::n3::spells::spell(0xcf26, &[(0x27, effect), (0x31, 100)]);
            world.apply_nano_visuals(ao_net::n3::spells::ApplySpells { target: receiver, spells: vec![spell.clone()], apply: true });
            world.nano_visual_frame(0.0, false, |_| Some(glam::Mat4::IDENTITY.to_cols_array_2d()), |_, _| None);
            let (_, _, handle) = world.nano_handles.iter().find(|(who, original, _)| *who == receiver && *original == spell).unwrap();
            assert!(world.effects.as_ref().unwrap().is_active(*handle));
            world.apply_nano_visuals(ao_net::n3::spells::ApplySpells { target: receiver, spells: vec![spell], apply: false });
            world.nano_visual_frame(0.0, false, |_| Some(glam::Mat4::IDENTITY.to_cols_array_2d()), |_, _| None);
            assert!(!world.nano_handles.iter().any(|(who, _, _)| *who == receiver));
        }
    }
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
        w.chars.get_mut(&id).unwrap().in_play = false;
        assert!(w.collect_tags(0.016, own_pos, &[sel]).iter().all(|t| t.id != id));
        w.chars.get_mut(&id).unwrap().in_play = true;
        let child = ao_net::msg::Identity { kind: CHAR_KIND, instance: id };
        let prop = ao_net::msg::Identity { kind: 0xc76a, instance: -1 };
        w.test_prop(prop, vec![]);
        let header = ao_net::n3::N3Header { msg_type: ao_net::n3::server_move::RELOCATE, target: child, flag: 0 };
        let relocate = |w: &mut Dynels, parent| {
            let payload = ao_net::n3::server_move::relocate(child, &ao_net::n3::server_move::Relocate { parent, children: vec![child, prop] });
            w.on_message(&Message { header, sender: 1, body: N3::Unknown(payload[13..].to_vec()) });
        };
        relocate(w, ao_net::msg::Identity { kind: CHAR_KIND, instance: 25988 });
        assert!(!w.on_ground(prop));
        assert!(w.collect_tags(0.016, own_pos, &[sel]).iter().all(|t| t.id != id));
        relocate(w, ao_net::msg::Identity::default());
        assert!(w.on_ground(prop));
        let key = w.chars[&id].key;
        if let Some(Model::Ready { built, .. }) = w.models.get_mut(&key) {
            built.visible = false;
        }
        assert!(w.collect_tags(0.016, own_pos, &[sel]).iter().all(|t| t.id != id));
        if let Some(Model::Ready { built, .. }) = w.models.get_mut(&key) {
            built.visible = true;
        }
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
            w.update_with_collision(dt, eye, [0.0, 0.0, -1.0], host, None, |_| None);
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

    /// Keep wire cloth/NPC overrides and mounted meshes tied to real client resources.
    #[test]
    fn captured_appearance_resources_resolve_and_decode() {
        let Some(dir) = client() else { return };
        let store = RecordStore::open(&dir).unwrap();
        let captures = [
            include_str!("../../../../docs/captures/zone_ithaca.rec"),
            include_str!("../../../../docs/captures/zone_enter_ithaca.rec"),
            include_str!("../../../../docs/captures/zone_newchar_ithaca.rec"),
            include_str!("../../../../docs/captures/zone_kill_ithaca.rec"),
            include_str!("../../../../docs/captures/zone_death_borealis.rec"),
            include_str!("../../../../docs/captures/zone_fight_ithaca.rec"),
            include_str!("../../../../docs/captures/zone_antonio_shop_ithaca.rec"),
        ];
        let mut textures = std::collections::BTreeSet::new();
        let mut meshes = std::collections::BTreeSet::new();
        let mut bodies = std::collections::BTreeSet::new();
        let mut characters = 0;
        for rec in captures {
            for frame in frames(rec) {
                let Ok(Message { body: N3::Dynel(Dynel::SimpleCharFullUpdate(c)), .. }) = ao_net::n3::decode(&frame) else { continue };
                characters += 1;
                textures.extend(c.cloth.iter().filter(|c| c.texture > 0).map(|c| (1010004, c.texture as u32)));
                textures.extend(c.textures.iter().filter(|t| t.texture > 0).map(|t| (1010004, t.texture as u32)));
                meshes.extend(c.attractors.iter().filter(|a| a.mesh > 0).map(|a| a.mesh as u32));
                meshes.extend(c.head_mesh.filter(|&h| h > 0).map(|h| h as u32));
                let model = if let Some(record) = monster_record(&store, &CharLook::from_update(&c)).unwrap() {
                    record.mesh().expect("captured NPC has a model")
                } else {
                    let (breed, gender) = ao_formats::screens::wire_breed_sex(c.breed as i32, c.sex as i32).unwrap();
                    ao_formats::character::player_model_build(&store, breed, gender, c.fatness.min(2)).unwrap()
                };
                bodies.insert(model);
            }
        }
        assert_eq!(characters, 144);
        assert_eq!(textures.len(), 93);
        for id in meshes {
            let refs = ao_formats::mesh::texture_refs(&store, ao_formats::mesh::MESH_TYPE, id)
                .unwrap_or_else(|e| panic!("attachment {id}: {e:#}")).expect("captured attachment exists");
            textures.extend(refs.into_iter().map(|k| (k.rdb_type, k.id)));
        }
        for id in bodies {
            let cat = load_cat_mesh(&store, CHAR_MESH_TYPE, id).unwrap();
            for part in cat.parts {
                textures.extend([part.texture, part.env_texture].into_iter().filter(|&id| id != 0).map(|id| (1010004, id)));
            }
        }
        for (rdb_type, id) in textures {
            let key = ao_scene::TextureKey { rdb_type, id };
            let texture = ao_formats::texture::load_texture(&store, key)
                .unwrap_or_else(|e| panic!("captured appearance texture {key:?}: {e:#}"))
                .unwrap_or_else(|| panic!("missing captured appearance texture {key:?}"));
            assert!(texture.width > 0 && texture.height > 0);
            assert_eq!(texture.rgba.len(), (texture.width * texture.height * 4) as usize);
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
        for &(kind, id) in z.world.props.keys().filter(|k| k.0 == 0xC76A) {
            assert!(z.world.name_of(kind, id).is_some_and(|n| n.starts_with("Remains of ")), "corpse name survives its owner's removal before model loading");
        }
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
            z.world.update_with_collision(0.05, eye, fwd, &mut host, None, |_| None);
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
            z.world.update_with_collision(0.016, eye, fwd, &mut host, None, |_| None);
            pushed += host.actors.len();
            host.actors.clear();
            host.actor_models.clear();
        }
        eprintln!("update: {:.3} ms/frame, {} actors/frame", t.elapsed().as_secs_f32() * 1000.0 / n as f32, pushed / n);
        let failed = z.world.models.values().filter(|m| matches!(m, Model::Failed)).count();
        eprintln!("{} models ready, {failed} failed, {} actors, {} props", models.len(), actors.len(), z.world.props.len());
        assert!(!actors.is_empty());
        // object use (docs/zone/interact.md §8): `Corpse_t`'s constructor sets Can 8, the vending machine's template has bit 3 (use);
        // built props have a pick box; the merged hit list sorts every intersecting body by model-space distance.
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
        // A ray aimed at a corpse can enter a captured character first (23557:
        // 50000:1026268 before 51050:5628). N3 0x1000fc8d -> 0x1000f8e5
        // sorts collision distance, not the identity whose centre we aimed at.
        assert!(body.hit(origin, toward, 100.0).is_some(), "aimed corpse has a collision");
        let characters = z.world.pick_bodies(z.char_id as i32);
        let mut expected: Vec<_> = characters.iter()
            .map(|b| (b, ao_net::msg::Identity { kind: CHAR_KIND, instance: b.id }))
            .chain(picks.iter().map(|(b, id)| (b, *id)))
            .filter_map(|(b, id)| b.hit(origin, toward, 100.0).map(|distance| (distance, b.id, id)))
            .collect();
        expected.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        assert_eq!(hit, expected.into_iter().map(|(_, _, id)| id).collect::<Vec<_>>(), "all intersecting identities in native distance order");
        assert_eq!(hit.iter().filter(|id| *id == who).count(), 1, "aimed corpse retained exactly once");
        if let Ok(out) = std::env::var("AOMAC_DYNEL_SHOT") {
            let scene = ao_formats::playfield::load_playfield_at(&RecordStore::open(&dir).unwrap(), &dir, 4582, ao_formats::playfield::DEFAULT_DAY_TIME).unwrap();
            // AOMAC_DYNEL_LOOK=<kind hex like c76a | npc | player>: camera 4 m from the first such dynel instead of the player's view
            let (mut cam, mut at) = ([eye[0], eye[1] + 1.7, eye[2]], [eye[0] + fwd[0], eye[1] + 1.5 + fwd[1], eye[2] + fwd[2]]);
            let mut target_actor = None;
            if let Ok(what) = std::env::var("AOMAC_DYNEL_LOOK") {
                let target = match what.as_str() {
                    "npc" => z.world.chars.iter().find(|(_, c)| c.npc).map(|(id, c)| (*id as u32, c.pose.pos)),
                    m if m.starts_with("monster:") => z.world.chars.iter().find(|(_, c)| matches!(&c.look, Look::Char(l) if l.monster_data.to_string() == m[8..])).map(|(id, c)| (*id as u32, c.pose.pos)),
                    "player" => z.world.chars.iter().find(|(_, c)| !c.npc && c.name != "Testy").map(|(id, c)| (*id as u32, c.pose.pos)),
                    k => z.world.props.iter().find(|(key, _)| format!("{:x}", key.0) == k).map(|(_, p)| (p.id, p.pos)),
                };
                let (id, pos) = target.expect("no such dynel");
                target_actor = Some(id);
                let t = scene_pos(pos);
                cam = [t[0] + 2.5, t[1] + 1.8, t[2] + 2.5];
                at = [t[0], t[1] + 0.8, t[2]];
            }
            // Submit the current poses against the camera actually used by the screenshot:
            // the earlier actor list was culled around Testy and predates the benchmark.
            host.actors.clear();
            for c in z.world.chars.values_mut() {
                c.submitted = false;
            }
            let direction = ao_render::Vec3::new(at[0] - cam[0], at[1] - cam[1], at[2] - cam[2]).normalize();
            z.world.update_with_collision(0.0, cam, [direction.x, direction.y, direction.z], &mut host, None, |_| None);
            models.append(&mut host.actor_models);
            actors = std::mem::take(&mut host.actors);
            assert!(!actors.is_empty(), "screenshot camera must submit nearby dynels");
            if let Some(id) = target_actor {
                let actor = actors.iter().find(|a| a.id == id).expect("screenshot target was culled");
                if z.world.chars.contains_key(&(id as i32)) {
                    assert!(actor.skin.is_some(), "new screenshot renderer requires the target's current pose");
                }
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

    #[test]
    fn distinct_swing_lists_preserve_parallel_holder_nodes() {
        let mut z = Zone::new(25988);
        for l in include_str!("../../../../docs/captures/zone_ithaca.rec").lines() {
            let mut p = l.split(' ');
            let (_, dir, hex) = (p.next().unwrap(), p.next().unwrap(), p.next().unwrap());
            if dir != "<" { continue; }
            let bytes: Vec<u8> = (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[2*i..2*i+2], 16).unwrap()).collect();
            if let Some((f, _)) = Frame::decode_with(&bytes, false).unwrap() { z.on_frame(&f); }
            if !z.world.chars.is_empty() { break; }
        }
        let who = *z.world.chars.keys().next().unwrap();
        let c = z.world.chars.get_mut(&who).unwrap();
        c.special = Special::None;
        c.clip_ms = 100.0;
        z.world.play_swing(who, Some(0x3ff), canim::list::ATTACK);
        z.world.play_swing(who, Some(0x3ff), canim::list::ATTACK);
        assert_eq!(z.world.chars[&who].actions.len(), 1, "same live list must not restart");
        z.world.play_swing(who, Some(0x3ff), canim::list::FLING_SHOT);
        assert_eq!(z.world.chars[&who].actions.len(), 2, "different list adds a holder node even for the same clip");
        assert_eq!(z.world.pending_clips.iter().filter(|p| p.2 == 0x3ff && p.3 == who).count(), 1, "distinct list nodes share one native rifle-shot clip request while busy");
        z.world.play_swing(who, None, canim::list::ATTACK);
        assert_eq!(z.world.chars[&who].actions.len(), 2, "fallback cannot bypass an active key");
        z.world.play_swing(who, None, canim::list::BURST);
        assert_eq!(z.world.chars[&who].actions.last().unwrap().id, ATTACK_KEY, "fallback uses the preloaded record attack");
        assert_eq!(z.world.chars[&who].clip_ms, 100.0, "swings do not restart locomotion");
        z.world.play_action(who, 0x6d, canim::Layer::Upper, -2);
        let c = &z.world.chars[&who];
        assert_eq!(c.clip_ms, 100.0, "authored action must not restart the base clock");
        assert_eq!(c.special, Special::None, "action owns a separate playback slot");
        assert_eq!(c.actions.last().unwrap().layer, canim::Layer::Upper);
        z.world.play_action(who, 0x6d, canim::Layer::Body, -1);
        let c = &z.world.chars[&who];
        assert_eq!(c.actions.iter().find(|action| action.id == 0x6d && action.priority == -1).unwrap().layer, canim::Layer::Body, "explicit producer layer wins over the wield table");
        assert_eq!(c.special, Special::None, "replaced combat transient must not resume after the action");
        assert_eq!(z.world.pending_clips.iter().filter(|pending| pending.3 == who).count(), 2, "parallel nodes share clip requests without canceling another clip");
        let mut built = plain(Default::default(), true);
        let clip = Arc::new(CatAnim { source_id: 0, root: String::new(), events: vec![], version: 0x106, duration: 1000.0, signature: 7, param: 0.0, tracks: vec![] });
        built.clips.insert(0x6d, vec![clip.clone()]);
        let c = z.world.chars.get_mut(&who).unwrap();
        built.clips.insert(c.anim, vec![clip]);
        for action in &mut c.actions { action.variant = Some(0); action.ms = 100.0; }
        let samples: Vec<_> = c.layers(&built).map(|layer| (layer.layers, layer.blend)).collect();
        assert_eq!(samples[0], (3, 1.0), "base priority 0 must compose before negative action priorities");
        assert!(samples[1..].iter().all(|sample| sample.1 == 0.5), "action nodes must retain their authored fade weights");
        c.actions.insert(0, ActionAnim { id: 0x6d, layer: canim::Layer::Upper, priority: 1, layers: 3, fade_ms: 200.0, ms: 100.0, variant: Some(0), note_fired: 0, rate: 1.0, key: None, slot: -1, delay: None });
        c.refresh_layers();
        assert_eq!(c.base_layers, 2);
        c.actions.remove(0);
        c.refresh_layers();
        assert_eq!(c.base_layers, 2, "expiration/zero exclusion must not reset stored playback masks");

        c.actions.clear();
        let burst = CatAnim { events: vec![(200, "attack_effect_1".into()), (400, "attack_effect_2".into()), (600, "attack_effect_3".into()), (800, "attack_effect_4".into())], ..built.clips[&0x6d][0].as_ref().clone() };
        let tick = |world: &mut Dynels, clip: &CatAnim, dt| {
            world.chars.get_mut(&who).unwrap().actions.retain_mut(|node| node.advance(clip, dt, |note| world.notes.push((who, note))));
        };
        z.world.hit_seen(who, crate::play::combat::notes::HitCtx { victim: 77, slot: 6, damage: 20, flags: 4 });
        z.world.play_swing(who, Some(1034), canim::list::BURST);
        tick(&mut z.world, &burst, 0.05);
        z.world.hit_seen(who, crate::play::combat::notes::HitCtx { victim: 77, slot: 0, damage: 0, flags: 1 });
        z.world.play_swing(who, Some(1034), canim::list::ATTACK);
        z.world.play_swing(who, Some(1034), canim::list::BURST);
        assert_eq!(z.world.chars[&who].actions.len(), 2, "the older Burst key stays suppressed while its holder node lives");
        for _ in 0..5 { tick(&mut z.world, &burst, 0.2); }
        let notes = z.world.take_notes();
        assert_eq!(notes.iter().filter(|(_, note)| note.slot == 6).count(), 4, "a normal swing at 50 ms must not lose Burst's later notes");
        assert_eq!(z.world.note_ctx(who, 6).unwrap().flags, 4, "older nodes use their original slot's retained flags");

        z.world.chars.get_mut(&who).unwrap().actions.clear();
        z.world.special_hit_seen(who, 77, 6);
        z.world.play_swing(who, Some(1034), canim::list::FLING_SHOT);
        let fling = CatAnim { events: vec![(200, "attack".into())], ..burst.clone() };
        tick(&mut z.world, &fling, 0.133);
        assert!(z.world.take_notes().is_empty());
        z.world.play_action(who, 0x3fe, canim::Layer::Body, 0);
        assert!(z.world.chars[&who].actions.iter().any(|node| node.id == 0x3fe && node.priority == 0 && node.key.is_none()), "FightStop inserts its group-0 holster node");
        assert!(z.world.chars[&who].actions.iter().any(|node| node.key == Some(canim::list::FLING_SHOT) && (node.ms - 133.0).abs() < 0.001), "holster preserves the advancing Fling node");
        z.world.note_target(who, 88);
        z.world.chars.get_mut(&who).unwrap().refresh_layers();
        tick(&mut z.world, &fling, 0.068);
        let notes = z.world.take_notes();
        assert_eq!(notes, [(who, crate::play::combat::notes::FiredNote { id: crate::play::combat::notes::id::ATTACK, slot: 6 })], "FightStop at 133 ms must preserve Fling's 200 ms attack note");
        let ctx = z.world.note_ctx(who, notes[0].1.slot).unwrap();
        assert_eq!((ctx.victim, ctx.flags), (88, 4), "target is dynamic, slot flags are retained");
        assert!(z.world.note_ctx(who, -1).is_none(), "non-swing action notes cannot reuse combat context");

        z.world.chars.get_mut(&who).unwrap().actions.clear();
        z.world.wield.entry(who).or_default()[0] = Some(Wield { set: 1, delay: 120 });
        let special = crate::play::combat::state::CombatEvent::SpecialAttack {
            who, target: ao_net::msg::Identity { kind: 50000, instance: 77 }, special: 148, slot: 6, damage: 20,
        };
        crate::play::combat::glue::combat_animations(&mut z.world, None, -1, &[special], |_| false);
        assert_eq!(z.world.chars[&who].actions.len(), 1);
        assert_eq!(z.world.chars[&who].actions[0].key, Some(canim::list::ATTACK), "missing Burst list resolves to retail 0xb");
        let normal = crate::play::combat::state::CombatEvent::Hit { attacker: who, victim: 77, damage: 10, slot: 6, flags: 4 };
        crate::play::combat::glue::combat_animations(&mut z.world, None, -1, &[normal], |_| false);
        assert_eq!(z.world.chars[&who].actions.len(), 1, "normal attack is suppressed by the fallback special's resolved key");
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

    #[test]
    fn explicit_actor_sound_position_uses_the_shared_variant_stream_without_a_model() {
        let mut dynels = Dynels::default();
        let mut expected_rng = CrtRand::new(1);
        let pos = [12.0, 4.0, -7.0];
        let variants = [11, 22, 33];
        assert!(dynels.chars.is_empty());
        dynels.sound_variants_at_position(&[], pos);
        dynels.sound_variants_at_position(&[99], pos);
        dynels.sound_variants_at_position(&variants, pos);
        assert_eq!(dynels.take_sounds(), [
            GameSound::at(99, pos),
            GameSound::at(variants[expected_rng.rand() as usize % variants.len()], pos),
        ]);
        let list = [(8, variants.to_vec())];
        assert_eq!(dynels.pick_of(&list, 8), Some(variants[expected_rng.rand() as usize % variants.len()]));
    }

    /// `FUN_10069acb`: the swing is a value of the wielded weapon's list (its `AnimSet`, the hand), a special key the weapon lacks falls back
    /// to list 0xb; nothing wielded = no weapon swing.
    #[test]
    fn swing_comes_from_the_wielded_weapon() {
        let mut d = Dynels::default();
        assert_eq!(d.pick_swing(7, canim::list::ATTACK), None);
        d.wield.entry(7).or_default()[0] = Some(Wield { set: 1, delay: 120 });
        for _ in 0..20 {
            let (a, delay, key) = d.pick_swing(7, canim::list::ATTACK).unwrap();
            assert_eq!(key, canim::list::ATTACK);
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

    #[test]
    fn fight_start_does_not_replace_same_frame_rifle_special() {
        use super::super::{combat::state::CombatEvent, player::Player};
        use ao_formats::character::Role;
        let Some(dir) = client() else { return };
        let own = 25988;
        let mut zone = Zone::new(own as u32);
        zone.world.start(dir.clone(), own);
        for frame in frames(include_str!("../../../../docs/captures/zone_ithaca.rec")) {
            zone.on_frame(&frame);
        }
        zone.world.wield.entry(own).or_default()[0] = Some(Wield { set: 3, delay: 200 });
        let mut player = Player::new(&dir, &zone, zone.playfield.unwrap_or(800)).expect("capture has own character");
        for (special, expected) in [(148, 1024), (150, 1023)] {
            let target = ao_net::msg::Identity { kind: CHAR_KIND, instance: 7 };
            let events = [
                CombatEvent::FightStarted { who: own, target, switched: false },
                CombatEvent::SpecialAttack { who: own, target, special, slot: 6, damage: 3 },
            ];
            super::super::combat::glue::combat_animations(&mut zone.world, Some(&mut player), own, &events, |_| true);
            assert_eq!(player.swing_role(), Some(&Role::Clip(canim::anim_name(expected).unwrap().0.into())));
        }
    }

    /// A rifle wielder out of a fight stands in `idle-2h`, in a fight in `idle-rifle`, and walks / runs with 0x421 / 0x422; a blade keeps the plain idle
    /// out of a fight and has the blade idle in one (`FUN_1009c858`, `FUN_1003cad0`).
    #[test]
    fn wielder_stance_follows_the_fight_state() {
        assert_eq!(stance_id(Some(3), AnimState::Idle, false), Some(0x41e));
        assert_eq!(stance_id(Some(3), AnimState::Idle, true), Some(0x3fd));
        assert_eq!((stance_id(Some(3), AnimState::Walk, false), stance_id(Some(3), AnimState::Run, true)), (Some(0x421), Some(0x422)));
        assert_eq!((stance_id(Some(1), AnimState::Idle, false), stance_id(Some(1), AnimState::Idle, true)), (None, Some(0x3e9)));
        assert_eq!((stance_id(Some(1), AnimState::Walk, false), stance_id(Some(8), AnimState::Run, false)), (None, None));
        assert_eq!((stance_id(None, AnimState::Idle, true), stance_id(Some(4), AnimState::Idle, false)), (None, None));
        let mut d = Dynels::default();
        d.set_fighting(7, true);
        assert!(d.fighting.contains(&7));
        d.set_fighting(7, false);
        assert!(d.fighting.is_empty());
    }

    /// The wear capture (`zone_wear_rifle_borealis.rec`): the rifle (AnimSet 3, `ItemDelay` 100) in slot 6 is the own character's wield (swing list,
    /// stance set, damage type, the avatar's attractor list); the server's `CharacterAction` 0x61 for slot 6 (the unwear; the item lives on in the bag)
    /// empties it again, also when it comes before the worker resolved the item.
    #[test]
    fn own_wield_follows_the_captured_wear_and_unwear() {
        use crate::play::combat::arms::UNWIELD_SLOT_6;
        use crate::play::zone::OwnEvent;
        let Some(dir) = client() else { return };
        let own = 0x82e8;
        let mut z = Zone::new(own as u32);
        z.world.start(dir, own);
        // the weapon update + appearance update of the wear, the unwear (bag slot 0x41) and the wear again (the capture's other frames do not matter here)
        let rec: Vec<Frame> = frames(include_str!("../../../../docs/captures/zone_wear_rifle_borealis.rec"))
            .into_iter()
            .filter(|f| matches!(ao_net::n3::decode(f).map(|m| m.body), Ok(N3::Dynel(Dynel::WeaponItemFullUpdate(_)) | N3::World(World::Appearance(_)))))
            .collect();
        assert_eq!(rec.len(), 6);
        let unwield = frames(&format!("0 < {UNWIELD_SLOT_6}"));
        let mut host = ao_render::Host::headless();
        let mut pump = |z: &mut Zone, n: usize, until: &dyn Fn(&Dynels) -> bool| {
            for _ in 0..n {
                z.world.update_with_collision(0.02, [0.0; 3], [0.0, 0.0, -1.0], &mut host, None, |_| None);
                if until(&z.world) {
                    return true;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            false
        };
        let attractors = |z: &mut Zone| std::mem::take(&mut z.own_events).into_iter().filter_map(|e| if let OwnEvent::Appearance(a) = e { Some(a.attractors.iter().filter(|t| t.b > 0).map(|t| (t.a, t.b as u32)).collect::<Vec<_>>()) } else { None }).collect::<Vec<_>>();
        let wear = |z: &mut Zone, i: usize| rec[2 * i..2 * i + 2].iter().for_each(|f| {
            let _ = z.on_frame(f);
        });
        wear(&mut z, 0);
        assert!(pump(&mut z, 500, &|w| w.wielded_set(own) == Some(3)), "the rifle never resolved");
        assert_eq!(attractors(&mut z), [vec![(1, 0x3ddf), (0, 0x9ee9)]], "the rifle mesh in the right hand attractor, next to the head");
        assert_eq!(z.world.arms.damage_type(own, 6, 0), Some(0x5a));
        let (anim, delay, key) = z.world.pick_swing(own, canim::list::ATTACK).unwrap();
        assert_eq!((anim, delay, key), (0x3ff, 100, canim::list::ATTACK), "rifle shot");
        // the resolved weapon is announced once (a weapon wielded during a fight runs the AnimHolder idle update, `combat/glue.rs::stance`)
        assert_eq!((z.world.take_wielded(), z.world.take_wielded()), (vec![own], vec![]));
        unwield.iter().for_each(|f| {
            let _ = z.on_frame(f);
        });
        assert_eq!((z.world.wielded_set(own), z.world.pick_swing(own, canim::list::ATTACK), z.world.arms.damage_type(own, 6, 0)), (None, None, None));
        wear(&mut z, 1);
        assert_eq!(attractors(&mut z), [vec![(0, 0x9ee9)]], "the unwear's list: the head only");
        assert!(!pump(&mut z, 20, &|w| w.wielded_set(own).is_some()), "the bag slot 0x41 is no hand");
        // wear again; unwield after the request went to the worker, before its answer is read: the stale answer is dropped
        wear(&mut z, 2);
        pump(&mut z, 1, &|_| false);
        unwield.iter().for_each(|f| {
            let _ = z.on_frame(f);
        });
        assert!(!pump(&mut z, 100, &|w| w.wielded_set(own).is_some()));
        wear(&mut z, 2);
        assert!(pump(&mut z, 500, &|w| w.wielded_set(own) == Some(3)));
    }

    /// Opt-in real-renderer evidence of unmodified captured remote-player looks.
    #[test]
    fn captured_remote_appearance_screenshots() {
        let Some(out) = std::env::var_os("AOMAC_SHOT_DIR").map(PathBuf::from) else { return };
        let Some(dir) = client() else { return };
        struct Shot { gui: Gui, scene: Option<ao_scene::Scene>, eye_z: f32 }
        impl ao_render::Frontend for Shot {
            fn gui(&self) -> &Gui { &self.gui }
            fn input(&mut self, _: ao_gui::InputEvent, _: &mut Host) {}
            fn frame(&mut self, dt: f32, _: (u32, u32), host: &mut Host) -> ao_gui::DrawList {
                host.camera = Camera::look_at(ao_render::Vec3::new(0.0, 1.2, self.eye_z), ao_render::Vec3::new(0.0, 1.0, 0.0));
                if let Some(scene) = self.scene.take() { host.set_scene(scene); }
                self.gui.frame(dt)
            }
        }
        let store = RecordStore::open(&dir).unwrap();
        let mut assets = ActorAssets::new(&store).unwrap();
        std::fs::create_dir_all(&out).unwrap();
        for (capture, name) in [
            (include_str!("../../../../docs/captures/zone_enter_ithaca.rec"), "Xantarr"),
            (include_str!("../../../../docs/captures/zone_ithaca.rec"), "Stanko"),
            (include_str!("../../../../docs/captures/zone_ithaca.rec"), "Bergdoktor"),
        ] {
            let u = frames(capture).iter().filter_map(|f| ao_net::n3::decode(f).ok())
                .find_map(|m| match m.body {
                    N3::Dynel(Dynel::SimpleCharFullUpdate(u)) if u.name == name => Some(u),
                    _ => None,
                }).expect("captured remote player");
            let built = build_char(&store, &mut assets, &CharLook::from_update(&u)).unwrap();
            let rig = built.rig.unwrap();
            let (vertices, transforms) = rig.pose(None);
            let mut scene = built.model;
            scene.meshes[0].vertices = vertices;
            scene.instances = transforms.into_iter().enumerate().map(|(mesh, transform)| ao_scene::Instance { mesh, transform }).collect();
            let mut shot = Shot { gui: Gui::new(&dir, None).unwrap(), scene: Some(scene), eye_z: 3.5 };
            let mut renderer = ao_render::Offscreen::new(&shot, (640, 800)).unwrap();
            for (side, eye_z) in [("back", 3.5), ("front", -3.5)] {
                shot.eye_z = eye_z;
                let list = renderer.frame(&mut shot, 0.016);
                renderer.png(&shot, &list, &out.join(format!("remote-{name}-{side}.png"))).unwrap();
            }
        }
    }

    #[test]
    fn full_update_head_survives_missing_attractor_but_appearance_clears_it() {
        let mut u = frames(include_str!("../../../../docs/captures/zone_enter_ithaca.rec"))
            .iter().filter_map(|f| ao_net::n3::decode(f).ok())
            .find_map(|m| match m.body {
                N3::Dynel(Dynel::SimpleCharFullUpdate(u)) if u.name == "Xantarr" => Some(u),
                _ => None,
            }).expect("captured Solitus player");
        assert_eq!((u.flags, u.head_mesh, u.attractors.len()), (0x4ac2, Some(223820), 0));
        let mut look = CharLook::from_update(&u);
        assert_eq!(look.attractors, [(0, 223820)]);
        look.apply_appearance(&ao_net::n3::world::AppearanceUpdate {
            cloth: vec![], attractors: vec![], visual_flags: 31, extra: 0,
        });
        assert!(look.attractors.is_empty(), "visual clear removes the head as well");
        u.flags |= ao_net::n3::dynel::flag::SET_DYNEL_800;
        assert!(CharLook::from_update(&u).skip_attractors);
        if let Some(dir) = client() {
            let store = RecordStore::open(&dir).unwrap();
            let mut assets = ActorAssets::new(&store).unwrap();
            u.flags &= !ao_net::n3::dynel::flag::SET_DYNEL_800;
            let headed = build_char(&store, &mut assets, &CharLook::from_update(&u)).unwrap();
            let cleared = build_char(&store, &mut assets, &look).unwrap();
            assert_eq!(headed.model.meshes.len(), cleared.model.meshes.len() + 1);
        }
    }

    /// `AppearanceUpdateIIR_c::Activate` [GC 0x10071679] on a look: cloth by part (page 0 only; texture 0 clears, unnamed parts stay), the
    /// attractor list replaces the old one wholesale (an empty list unmounts the head too), `SET_DYNEL_800` no longer skips it.
    #[test]
    fn appearance_update_edits_the_look() {
        use ao_net::n3::world::{AppearanceUpdate, Attractor, ClothData};
        let mut l = CharLook { npc: false, breed: 1, sex: 2, race: 1, fatness: 1, head: Some(9), monster_data: 0, textures: vec![], cloth: vec![(4, 50), (1, 100)], attractors: vec![(0, 9)], skip_attractors: true };
        let cl = |id, b, c| ClothData { id, b, c, ..Default::default() };
        let mut a = AppearanceUpdate { cloth: vec![cl(1, 0, 0), cl(2, 77, 0), cl(3, 9, 1)], attractors: vec![Attractor { a: 1, b: 0x3ddf, c: 0, d: 2 }, Attractor { a: 0, b: 7, c: 0, d: 4 }], visual_flags: 0x1f, extra: 0 };
        l.apply_appearance(&a);
        assert_eq!((l.cloth.as_slice(), l.attractors.as_slice(), l.skip_attractors), (&[(2, 77), (4, 50)][..], &[(1, 0x3ddf), (0, 7)][..], false));
        a.cloth.clear();
        a.attractors.clear();
        l.apply_appearance(&a);
        assert_eq!((l.cloth.len(), l.attractors.len()), (2, 0), "no cloth named = unchanged; no attractors = no head, no weapon");
    }

    /// The captured wear / unwear `AppearanceUpdate`s (zone_wear_rifle_borealis.rec, own character) retargeted to another player of
    /// zone_ithaca.rec: his attractor list becomes the rifle (right hand) + head, then the head only, each time as a rebuilt model; the
    /// old model stays drawn until the new one is ready.
    #[test]
    fn other_player_wields_and_unwields_live() {
        let Some(dir) = client() else { return };
        let own = 25988;
        let mut z = Zone::new(own as u32);
        z.world.start(dir, own);
        for f in frames(include_str!("../../../../docs/captures/zone_ithaca.rec")) {
            z.on_frame(&f);
        }
        let mut ids: Vec<i32> = z.world.chars.iter().filter(|(id, c)| **id != own && matches!(&c.look, Look::Char(l) if !l.npc)).map(|(id, _)| *id).collect();
        ids.sort_unstable();
        let id = ids[0];
        let mut host = ao_render::Host::headless();
        let mut settle = |w: &mut Dynels| {
            for _ in 0..1000 {
                w.update_with_collision(0.02, [0.0; 3], [0.0, 0.0, -1.0], &mut host, None, |_| None);
                let c = &w.chars[&id];
                if c.next.is_none() && matches!(w.models.get(&c.key), Some(Model::Ready { .. })) {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            panic!("model never ready");
        };
        settle(&mut z.world);
        let meshes = |w: &Dynels| match w.models.get(&w.chars[&id].key) {
            Some(Model::Ready { built, .. }) => built.model.meshes.len(),
            _ => panic!("no model"),
        };
        let plain = meshes(&z.world);
        let mut wear: Vec<Message> = frames(include_str!("../../../../docs/captures/zone_wear_rifle_borealis.rec"))
            .iter()
            .filter_map(|f| ao_net::n3::decode(f).ok())
            .filter(|m| matches!(m.body, N3::World(World::Appearance(_))))
            .collect();
        assert!(wear.len() >= 2);
        wear.iter_mut().for_each(|m| m.header.target.instance = id);
        // the wear: rifle (right hand 0x3ddf) + head; the old model stays drawn while the new one builds
        let before = z.world.chars[&id].key;
        z.world.on_message(&wear[0]);
        let (look, next) = (z.world.chars[&id].look.clone(), z.world.chars[&id].next);
        assert!(matches!(&look, Look::Char(l) if l.attractors.contains(&(1, 0x3ddf))), "{look:?}");
        assert!(next.is_some_and(|k| k != before) && z.world.chars[&id].key == before);
        settle(&mut z.world);
        // this player carried a back item (place 5), the wear's list replaces it with the rifle: same mesh count; his captured cloth (all zero in
        // the own character's message) is cleared like `FUN_100480fc` writes it
        assert_eq!(meshes(&z.world), plain, "back item gone, rifle in");
        assert!(matches!(&z.world.chars[&id].look, Look::Char(l) if l.cloth.is_empty()));
        // the unwear: head only
        z.world.on_message(&wear[1]);
        settle(&mut z.world);
        assert_eq!(meshes(&z.world), plain - 1, "the rifle is gone, nothing mounted but the head");
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

    /// 288560 is an item template, not MonsterData: retail resolves a normal humanoid.
    #[test]
    fn missing_monster_data_builds_humanoid_with_named_clips() {
        let Some(dir) = client() else { return };
        let store = RecordStore::open(&dir).unwrap();
        assert!(NpcRecord::lookup(&store, 288560).unwrap().is_none());
        assert!(NpcRecord::load(&store, 288560).is_err(), "surveys retain strict loading");
        let mut assets = ActorAssets::new(&store).unwrap();
        let look = CharLook { npc: true, breed: 1, sex: 2, race: 1, fatness: 1, head: None, monster_data: 288560, textures: vec![], cloth: vec![], attractors: vec![], skip_attractors: false };
        let built = build_char(&store, &mut assets, &look).unwrap();
        let rig = built.rig.as_ref().unwrap();
        let (breed, gender) = ao_formats::screens::wire_breed_sex(1, 2).unwrap();
        assert_eq!(rig.model_id, ao_formats::character::player_model_build(&store, breed, gender, 1).unwrap());
        assert!(!built.model.meshes.is_empty());
        for key in [0x78, DIE_KEY, ATTACK_KEY] {
            assert!(built.clips.get(&key).is_some_and(|clips| !clips.is_empty()), "named clip {key}");
        }
        let player = CharLook { npc: false, monster_data: 0, ..look.clone() };
        let ids = clip_ids(&store, &assets, &look, 1033).unwrap();
        assert!(!ids.is_empty());
        assert_eq!(ids, clip_ids(&store, &assets, &player, 1033).unwrap());
        for id in ids {
            assert_eq!(assets.anim(&store, id).unwrap().signature, rig.cat().signature);
        }
        let corpse = CorpseLook { cat_mesh: rig.model_id, head: None, breed: 1, sex: 2, race: 1, cloth: vec![], textures: vec![], animation: Some((288560, 503)) };
        let corpse = build_corpse(&store, &mut assets, &corpse).unwrap();
        assert_eq!(corpse.held.unwrap().0, corpse.rig.unwrap().pose(None).0);
    }

    /// A captured corpse holds the terminal frame of its NPC-record death animation.
    #[test]
    fn corpse_holds_death_pose() {
        let Some(dir) = std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Games/ProjectRubiKa/client")).filter(|d| d.join("cd_image/rdb.db").exists()) else { return };
        let store = RecordStore::open(&dir).unwrap();
        let mut assets = ActorAssets::new(&store).unwrap();
        let c = CorpseLook { cat_mesh: 22773, head: None, breed: 6, sex: 1, race: 1, cloth: vec![], textures: vec![], animation: Some((22794, 503)) };
        let built = build_corpse(&store, &mut assets, &c).unwrap();
        let held = built.held.expect("held pose");
        assert!(built.clips.is_empty());
        let (record, key) = c.animation.unwrap();
        let record = NpcRecord::load(&store, record).unwrap();
        let id = ao_formats::character::anim_key_variants(&record, key)[0];
        let animation = assets.anim(&store, id).unwrap();
        let expected = built.rig.as_ref().unwrap().pose(Some((&animation, animation.duration)));
        assert_eq!(held.0, expected.0);
        assert_ne!(held.0, built.model.meshes[0].vertices);
    }

    /// Complete received kill sequence, including the distinct corpse identity and late messages to the removed NPC.
    #[test]
    fn captured_kill_replaces_character_with_persistent_corpse() {
        let (npc, corpse) = (0xf7f82, (0xc76a, 0xcf3));
        let mut zone = Zone::new(0x830e);
        let mut order = Vec::new();
        for f in frames(include_str!("../../../../docs/captures/zone_kill_ithaca.rec")) {
            let Ok(m) = ao_net::n3::decode(&f) else { continue };
            if matches!(&m.body, N3::World(World::CharacterAction(a)) if m.header.target.instance == npc && a.action == 99) {
                zone.target = Some(npc);
            }
            zone.on_frame(&f);
            let world = &mut zone.world;
            match &m.body {
                N3::World(World::CharacterAction(a)) if m.header.target.instance == npc && a.action == 99 => {
                    order.push("death");
                    assert_eq!(zone.target, None);
                    assert!(matches!(world.chars[&npc].special, Special::Die(503)));
                    let at = world.chars[&npc].pose.pos;
                    world.advance(1.0);
                    assert_eq!(world.chars[&npc].pose.pos, at);
                }
                N3::Misc(Misc::ToClientQuit) if m.header.target.instance == npc => {
                    order.push("quit");
                    assert!(!world.chars.contains_key(&npc));
                    assert!(!zone.dynels.contains_key(&npc));
                    assert!(!zone.character_stats.contains_key(&npc));
                }
                N3::World(World::Corpse(c)) if m.header.target.instance == corpse.1 => {
                    order.push("corpse");
                    assert_eq!(c.owner.instance, npc);
                    assert_eq!(c.spells[0].standard, [1, 0, 0, 0]);
                    assert_eq!(c.spells[0].args, [0, 0, 503, 1, 4, 17655, 0]);
                    assert_eq!(world.props[&corpse].pos, c.base.position.unwrap());
                    assert!(!world.chars.contains_key(&npc));
                }
                _ => {}
            }
        }
        assert_eq!(order, ["death", "quit", "corpse"]);
        assert!(zone.world.props.contains_key(&corpse), "loot updates do not despawn the corpse");
        assert!(!zone.world.chars.contains_key(&npc), "late stop-fight cannot resurrect the NPC");
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
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            z.world.update_with_collision(0.0, eye, fwd, &mut host, None, |_| None);
            host.actors.clear();
            let c = &z.world.chars[&leet];
            if matches!(z.world.models.get(&c.key), Some(Model::Ready { .. })) { break; }
            assert!(std::time::Instant::now() < deadline, "death model did not become ready");
            std::thread::yield_now();
        }
        // Advance the same thirty simulated seconds only after the async model is available.
        for _ in 0..600 {
            z.world.update_with_collision(0.05, eye, fwd, &mut host, None, |_| None);
            host.actors.clear();
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

    /// A zone with the captured characters and every model built (`start` + frames + update ticks): (zone, a player, a Beach Leet).
    fn fight_zone() -> Option<(Zone, i32, i32)> {
        let dir = client()?;
        let mut z = Zone::new(25988);
        z.world.start(dir, 25988);
        for f in frames(include_str!("../../../../docs/captures/zone_ithaca.rec")) {
            z.on_frame(&f);
        }
        let leet = *z.world.chars.iter().find(|(_, c)| c.name == "Beach Leet")?.0;
        let mut players: Vec<i32> = z.world.chars.iter().filter(|(id, c)| **id != z.world.own && matches!(&c.look, Look::Char(l) if !l.npc)).map(|(id, _)| *id).collect();
        players.sort_unstable();
        let player = *players.first()?;
        let mut host = Host::headless();
        let (eye, fwd) = (scene_pos(z.own()?.pos), [0.0, 0.0, -1.0]);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            z.world.update_with_collision(0.05, eye, fwd, &mut host, None, |_| None);
            host.actors.clear();
            let pending: Vec<_> = [player, leet].into_iter().filter_map(|id| {
                let c = &z.world.chars[&id];
                match z.world.models.get(&c.key) {
                    Some(Model::Ready { .. }) => None,
                    Some(Model::Loading) => Some((id, c.name.as_str(), c.key, "loading")),
                    Some(Model::Failed) => Some((id, c.name.as_str(), c.key, "failed")),
                    None => Some((id, c.name.as_str(), c.key, "not queued")),
                }
            }).collect();
            if pending.is_empty() { break; }
            assert!(!pending.iter().any(|p| p.3 == "failed"), "fight fixture target model failed: {pending:?}");
            assert!(std::time::Instant::now() < deadline, "fight fixture target models not ready after 30s: {pending:?}");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        Some((z, player, leet))
    }

    #[test]
    fn nano_sound_stages_own_foreign_and_cancel() {
        let Some((mut zone, player, leet)) = fight_zone() else { return };
        let world = &mut zone.world;
        world.nano_casts.clear();
        world.casting.clear();
        world.nano_effect_categories = 0;
        world.effects = None; // Audio must not depend on FX renderer or connector availability.
        world.nano_templates.insert(-1, Arc::new(ao_formats::dynel_visual::ItemTemplate {
            kind: 0, name: None, sounds: vec![],
            stats: vec![(0, 0), (0x126, 100), (269, 11), (270, 99), (271, 12), (272, 13)],
        }));
        for (own, cancellation) in [(player, 0x66), (25988, 0x6c), (player, 0x75), (25988, 0x89)] {
            world.own = own;
            let source = ao_net::msg::Identity { kind: CHAR_KIND, instance: player };
            let target = ao_net::msg::Identity { kind: CHAR_KIND, instance: leet };
            let cast = ao_net::n3::dynel::CastNanoSpell { spell: -1, target, source, flag: true, rest: vec![] };
            world.nano_casts.push((player, cast.clone()));
            let live_pos = glam::Vec3::from(scene_pos(world.chars[&player].pose.pos)) + glam::Vec3::new(31.0, 24.21 - 9.733, -17.0);
            let matrix = glam::Mat4::from_translation(live_pos).to_cols_array_2d();
            world.nano_visual_frame(0.0, false, |_| Some(matrix), |_, _| None);
            let start = world.take_nano_sounds();
            assert_eq!(start.iter().map(|s| (s.0, s.2, s.3, s.4)).collect::<Vec<_>>(), [(11, 0.2, 0.6, 269), (11, 0.2, 1.0, 269)]);
            let expected_pos = if own == player { live_pos.to_array() } else { scene_pos(world.chars[&player].pose.pos) };
            assert!(start.iter().all(|sound| sound.1 == expected_pos), "own audio follows the rendered Player, foreign audio follows its mover");
            if own != player {
                assert_eq!(world.chars[&player].special, Special::Cast(203));
                assert!(world.chars[&player].actions.iter().all(|node| node.key.is_none()), "nano charge is not a combat list-history node");
            }
            world.take_nano_animations();
            world.nano_visual_frame(1.1, false, |_| Some(matrix), |_, _| None);
            world.take_nano_animations();
            world.nano_visual_frame(0.0, false, |_| Some(matrix), |_, _| None);
            let release = world.take_nano_sounds();
            assert_eq!(release.iter().map(|s| (s.0, s.2, s.4)).collect::<Vec<_>>(), [(12, 0.2, 271)]);
            assert_eq!(release[0].1, expected_pos);
            world.casting[0].release_seen = true;
            world.chars.get_mut(&player).unwrap().special = Special::None;
            world.nano_visual_frame(0.0, true, |_| Some(matrix), |_, _| None);
            let finish = world.take_nano_sounds();
            assert_eq!(finish.len(), 1);
            assert_eq!((finish[0].0, finish[0].2, finish[0].4, finish[0].5), (13, 0.0, 272, leet));
            assert_eq!(finish[0].1, scene_pos(world.chars[&leet].pose.pos));
            world.nano_sound(-1, 272, (player, Some(live_pos.to_array())), 0.0, 1.0);
            assert_eq!(world.take_nano_sounds()[0].1, expected_pos, "self-target finish uses the same live source");
            world.nano_casts.push((player, cast));
            world.nano_visual_frame(0.0, false, |_| Some(matrix), |_, _| None);
            world.on_message(&Message {
                header: ao_net::n3::N3Header { msg_type: ao_net::n3::world::CHARACTER_ACTION, target: source, flag: 0 },
                sender: player as u32,
                body: N3::World(World::CharacterAction(ao_net::n3::world::CharacterAction {
                    action: cancellation, param: 0,
                    identity_a: source, identity_b: if cancellation == 0x75 { ao_net::msg::Identity { kind: 0, instance: -1 } } else { ao_net::msg::Identity { kind: -1, instance: 7 } }, text: String::new(),
                })),
            });
            if cancellation == 0x89 {
                assert!(world.take_nano_sounds().iter().all(|sound| sound.4 == 269));
                assert!(!world.casting.is_empty(), "queue clear does not cancel the playing controller");
                world.nano_visual_frame(0.0, false, |_| Some(matrix), |_, _| None);
                world.take_nano_animations();
                world.nano_visual_frame(0.0, false, |_| Some(matrix), |_, _| None);
                assert_eq!(world.take_nano_sounds().iter().map(|sound| sound.4).collect::<Vec<_>>(), [271]);
                world.casting[0].release_seen = true;
                world.chars.get_mut(&player).unwrap().special = Special::None;
                world.nano_visual_frame(0.0, true, |_| Some(matrix), |_, _| None);
                assert!(world.take_nano_sounds().is_empty(), "queue clear has no successful finish");
            } else {
                assert!(world.take_nano_sounds().is_empty(), "cancel removes undelivered refreshes");
                assert!(world.pending_clips.iter().all(|clip| clip.3 != player || !matches!(clip.2, 201 | 203)), "cancel removes pending nano clip loads");
                assert!(world.replay.iter().all(|clip| clip.0 != player || !matches!(clip.1, 201 | 203)), "cancel cannot replay a late nano clip");
                world.nano_visual_frame(2.0, true, |_| Some(matrix), |_, _| None);
                assert!(world.take_nano_sounds().is_empty(), "cancel never emits finish or release");
            }
            assert!(world.casting.is_empty());
        }
    }

    #[test]
    fn body_boost_uses_authored_sound_ids_without_270_substitution() {
        let Some((mut zone, player, leet)) = fight_zone() else { return };
        let world = &mut zone.world;
        world.nano_casts.clear();
        world.casting.clear();
        world.effects = None;
        world.nano_effect_categories = 0;
        world.own = player;
        let source = ao_net::msg::Identity { kind: CHAR_KIND, instance: player };
        let target = ao_net::msg::Identity { kind: CHAR_KIND, instance: leet };
        world.nano_casts.push((player, ao_net::n3::dynel::CastNanoSpell { spell: 29091, target, source, flag: true, rest: vec![] }));
        world.nano_visual_frame(0.0, false, |_| None, |_, _| None);
        let template = &world.nano_templates[&29091];
        assert_eq!(template.stat(269).map(|v| v as u32), Some(0x35a9ce7d));
        assert_eq!(template.stat(270).map(|v| v as u32), Some(0x94bb7805));
        assert_eq!(template.stat(271), None);
        assert_eq!(template.stat(272).map(|v| v as u32), Some(0x80d5111a));
        assert!(world.take_nano_sounds().iter().all(|sound| sound.0 == 0x35a9ce7d && sound.4 == 269));
        world.take_nano_animations();
        world.nano_visual_frame(100.0, false, |_| None, |_, _| None);
        world.take_nano_animations();
        world.nano_visual_frame(0.0, false, |_| None, |_, _| None);
        assert!(world.take_nano_sounds().is_empty(), "missing271 is silent, not stat270");
        world.nano_visual_frame(0.0, true, |_| None, |_, _| None);
        let sounds = world.take_nano_sounds();
        assert_eq!(sounds.len(), 1);
        assert_eq!((sounds[0].0, sounds[0].2, sounds[0].4, sounds[0].5), (0x80d5111a, 0.0, 272, leet));
    }

    #[test]
    fn captured_npc_shadow_touch_traverses_silent_template_sound_path() {
        let Some((mut zone, _, _)) = fight_zone() else { return };
        let casts: Vec<_> = frames(include_str!("../../../../docs/captures/zone_ithaca.rec"))
            .iter().filter_map(|frame| ao_net::n3::decode(frame).ok())
            .filter(|message| matches!(&message.body, N3::Dynel(Dynel::CastNanoSpell(cast)) if cast.spell == 163449)).collect();
        assert_eq!(casts.len(), 3);
        zone.world.nano_casts.clear();
        zone.world.casting.clear();
        zone.world.nano_effect_categories = 0;
        zone.world.effects = None;
        for message in casts {
            let who = message.header.target.instance;
            zone.world.own = who;
            zone.world.on_message(&message);
            zone.world.nano_visual_frame(0.0, false, |_| None, |_, _| None);
            assert_eq!(zone.world.casting.len(), 1, "actual received cast enters stage path");
            assert!(zone.world.casting[0].instant);
            let template = &zone.world.nano_templates[&163449];
            assert!((269..=272).all(|stat| template.stat(stat).is_none()));
            zone.world.take_nano_animations();
            zone.world.nano_visual_frame(0.0, false, |_| None, |_, _| None);
            zone.world.nano_visual_frame(0.0, true, |_| None, |_, _| None);
            assert!(zone.world.casting.is_empty(), "actual release completion ends the cast");
            assert!(zone.world.take_nano_sounds().is_empty(), "no fabricated sound for absent stats");
        }
    }

    /// Actual received casts, with an explicit simulated release completion; never real-window evidence.
    #[test]
    #[ignore = "installed retail assets and offscreen Metal capture replay"]
    fn captured_shadow_touch_offscreen_effect_frames() {
        let (mut zone, _, _) = fight_zone().expect("installed capture fixture");
        let out = std::path::PathBuf::from(std::env::var_os("AOMAC_EFFECT_FRAMES").expect("AOMAC_EFFECT_FRAMES output directory"));
        std::fs::create_dir_all(&out).unwrap();
        let casts: Vec<_> = frames(include_str!("../../../../docs/captures/zone_ithaca.rec"))
            .iter().filter_map(|frame| ao_net::n3::decode(frame).ok())
            .filter(|message| matches!(&message.body, N3::Dynel(Dynel::CastNanoSpell(cast)) if cast.spell == 163449)).collect();
        assert_eq!(casts.len(), 3);
        zone.world.nano_casts.clear();
        zone.world.casting.clear();
        zone.world.nano_effect_categories = 4;
        for (index, message) in casts.into_iter().enumerate() {
            let N3::Dynel(Dynel::CastNanoSpell(cast)) = &message.body else { unreachable!() };
            let target = cast.target.instance;
            let mut host = Host::headless();
            let point = glam::Vec3::from_array(scene_pos(zone.world.chars[&target].pose.pos));
            let eye = point + glam::Vec3::new(0.0, 1.0, 4.0);
            let look = point + glam::Vec3::Y;
            host.camera = ao_render::Camera::look_at(ao_render::Vec3::from_array(eye.to_array()), ao_render::Vec3::from_array(look.to_array()));
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            while zone.world.effect_anchor(target, 0, 0).is_none() {
                zone.world.update_with_collision(0.0, eye.to_array(), [0.0, 0.0, -1.0], &mut host, None, |_| None);
                assert!(std::time::Instant::now() < deadline, "captured target connector unavailable");
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            zone.world.effects = Some(crate::play::combat::effects::Renderer::open(&ao_gui::client_dir()).unwrap());
            zone.world.own = message.header.target.instance;
            zone.world.on_message(&message);
            zone.world.nano_visual_frame(0.0, false, |_| None, |_, _| None);
            let template = &zone.world.nano_templates[&163449];
            assert!((269..=272).all(|stat| template.stat(stat).is_none()));
            assert!(template.stat(413).is_none());
            assert!(zone.world.casting[0].finish.contains(&2710));
            zone.world.take_nano_animations();
            zone.world.nano_visual_frame(0.0, true, |_| None, |_, _| None);
            assert!(zone.world.take_nano_sounds().is_empty());
            assert!(zone.world.casting.is_empty(), "captured finish connector must spawn without a pending cast");
            host.actors.clear();
            host.actor_models.clear();
            let mut models = Vec::new();
            let mut visible = false;
            for frame in 0..30 {
                host.actors.clear();
                zone.world.effects.as_mut().unwrap().frame(1.0 / 60.0, &mut host, None);
                models.append(&mut host.actor_models);
                visible |= !host.actors.is_empty();
                if [0, 2, 5, 11, 23, 29].contains(&frame) {
                    let path = out.join(format!("shadow-touch-{index}-{frame:04}.png"));
                    ao_render::render_to_png_actors(&ao_scene::Scene::default(), &models, host.actors.clone(), eye.to_array(), look.to_array(), 640, 480, &path, (frame + 1) as f32 / 60.0).unwrap();
                    eprintln!("captured ShadowTouch caster={} target={target} effect=2710 frame={frame} dt=1/60 actors={} template_sounds=0 persistent_stat413=absent path={}", message.header.target.instance, host.actors.len(), path.display());
                }
            }
            assert!(visible, "actual captured finish effect must produce renderer actors");
        }
    }

    #[test]
    fn special_notes_retain_only_their_attacker_and_slot_flags() {
        use crate::play::combat::{glue::note_reaction, notes::{HitCtx, id}};
        let mut world = Dynels::default();
        world.hit_seen(1, HitCtx { victim: 2, slot: 6, damage: 20, flags: 4 });
        assert!(world.once_rate.is_empty(), "message arrival must not react");
        note_reaction(&mut world, None, 99, 1, id::SWISH_PUNCH, 6);
        assert!(world.once_rate.is_empty(), "only an attack note reacts");
        note_reaction(&mut world, None, 99, 1, id::ATTACK, 6);
        assert_eq!(world.once_rate[&2].1, 1.0);
        world.once_rate.clear();
        world.special_hit_seen(1, 3, 6);
        assert_eq!(world.note_ctx(1, 6).map(|h| (h.victim, h.damage, h.flags)), Some((3, 20, 4)));
        world.note_target(1, 4);
        note_reaction(&mut world, None, 99, 1, id::ATTACK_EFFECT_1, 6);
        assert_eq!(world.once_rate[&4].1, 1.0, "special reacts on its actual target");
        world.once_rate.clear();
        world.hit_seen(1, HitCtx { victim: 2, slot: 8, damage: 0, flags: 1 });
        world.special_hit_seen(1, 3, 8);
        note_reaction(&mut world, None, 99, 1, id::ATTACK, 8);
        assert!(world.once_rate.is_empty(), "special after miss has no impact");
        world.special_hit_seen(1, 3, 6);
        assert_eq!(world.note_ctx(1, 6).map(|h| (h.damage, h.flags)), Some((20, 4)), "other slot's miss is isolated");
        for (who, slot) in [(1, 7), (5, 6)] {
            world.special_hit_seen(who, 3, slot);
            assert_eq!(world.note_ctx(who, slot).map(|h| (h.damage, h.flags)), Some((0, 0)));
            note_reaction(&mut world, None, 99, who, id::ATTACK, slot);
            assert!(world.once_rate.is_empty(), "constructors initialize both fields to zero");
        }
    }

    #[test]
    fn special_sound_impact_uses_retained_damage_not_the_result() {
        use crate::play::combat::notes::HitCtx;
        let Some((mut z, att, _)) = fight_zone() else { return };
        z.world.arms.clear();
        z.world.arms.list(att, false, &[(43712, 100), (43713, 142)]);
        z.world.hit_seen(att, HitCtx { victim: att, slot: 0, damage: 20, flags: 4 });
        z.world.special_hit_seen(att, att, 0);
        z.world.note_sounds(att, 0xb, 0);
        assert!(!z.world.take_sounds().is_empty(), "special retains slot swing sound");
        assert!(!z.world.later.is_empty(), "special after hit retains delayed impact");
        assert!(z.world.later.iter().all(|(_, s)| s.size == crate::play::combat::notes::impact_size(20)));
        z.world.later.clear();
        z.world.hit_seen(att, HitCtx { victim: att, slot: 0, damage: 0, flags: 1 });
        z.world.special_hit_seen(att, att, 0);
        z.world.note_sounds(att, 0xb, 0);
        assert!(z.world.take_sounds().is_empty(), "dummy slot after miss has no gated sound");
        assert!(z.world.later.is_empty());
        z.world.special_hit_seen(att, att, 8);
        z.world.note_sounds(att, 0xb, 8);
        assert!(z.world.later.is_empty(), "untouched slot has no impact");
        z.world.arms.wield(att, 900, 6, Some(121567), &[]);
        z.world.special_hit_seen(att, att, 6);
        z.world.note_sounds(att, 0xb, 6);
        assert!(!z.world.take_sounds().is_empty(), "constructor-zero wielded slot still plays its ungated weapon sound");
        assert!(z.world.later.is_empty());
        z.world.hit_seen(att, HitCtx { victim: att, slot: 6, damage: 0, flags: 1 });
        z.world.special_hit_seen(att, att, 6);
        z.world.note_sounds(att, 0xb, 6);
        assert!(!z.world.take_sounds().is_empty(), "special after miss still plays wielded swing sound");
        assert!(z.world.later.is_empty());
    }

    /// `FUN_10045069` + `FUN_1009b4ac` with real data (docs/zone/combat-anim.md section 6): a bare-handed player (martial-arts item 43712: swing `0xb` ->
    /// `0xc1080179`, impact `0x1f` -> `0x7199b60d`) hits a Beach Leet and another player. The swing sound is immediate at the attacker; the impact
    /// sounds come 0.4 s later at the victim, with the creature's `FabricType` / the flesh material 7 and the damage's size.
    #[test]
    fn a_bare_handed_hit_plays_the_weapon_swing_and_the_material_impact() {
        use crate::play::combat::notes::{impact_size, HitCtx, IMPACT_DELAY_S};
        let Some((mut z, att, leet)) = fight_zone() else { return };
        let mut others: Vec<i32> = z.world.chars.iter().filter(|(id, c)| **id != att && **id != z.world.own && matches!(&c.look, Look::Char(l) if !l.npc)).map(|(id, _)| *id).collect();
        others.sort_unstable();
        let victim = others.first().copied().unwrap_or(att);
        z.world.arms.clear();
        z.world.arms.list(att, false, &[(43712, 100)]);
        let apos = scene_pos(z.world.chars[&att].pose.pos);
        let fabric = {
            let c = &z.world.chars[&leet];
            let Some(Model::Ready { built, .. }) = z.world.models.get(&c.key) else { panic!("leet model not ready") };
            built.fabric
        };
        eprintln!("Beach Leet FabricType {fabric}");
        // a hit on the leet: the swing sound now, the impact later
        z.world.hit_seen(att, HitCtx { victim: leet, slot: 0, damage: 20, flags: 3 });
        z.world.note_sounds(att, 0xb, 0);
        let now = z.world.take_sounds();
        assert_eq!(now, [GameSound::at(0xc1080179, apos)], "the weapon's list 0xb at the attacker");
        assert!(z.world.take_sounds().is_empty());
        let mut host = Host::headless();
        let eye = scene_pos(z.own().unwrap().pos);
        let lpos = scene_pos(z.world.chars[&leet].pose.pos);
        z.world.update_with_collision(IMPACT_DELAY_S - 0.1, eye, [0.0, 0.0, -1.0], &mut host, None, |_| None);
        assert!(z.world.take_sounds().is_empty(), "the impact waits {IMPACT_DELAY_S} s");
        z.world.update_with_collision(0.2, eye, [0.0, 0.0, -1.0], &mut host, None, |_| None);
        let later = z.world.take_sounds();
        eprintln!("impact sounds on the leet: {later:?}");
        if (1..=17).contains(&fabric) {
            assert!(!later.is_empty() && later.iter().all(|s| s.pos == lpos && s.material == fabric && s.size == impact_size(20)), "{later:?}");
            assert!(later.iter().any(|s| s.id == 0x7199b60d), "the martial-arts impact list 0x1f");
        } else {
            assert!(later.is_empty(), "no FabricType, no impact: {later:?}");
        }
        // a hit that does no damage / a hit kind <= 1 plays the swing only (`FUN_1009b4ac` returns before the impact)
        z.world.hit_seen(att, HitCtx { victim: leet, slot: 0, damage: 20, flags: 1 });
        z.world.note_sounds(att, 0xb, 0);
        assert!(z.world.take_sounds().is_empty() || z.world.take_sounds().is_empty(), "hit kind 1: the dummy weapon's b4ac part is skipped");
        z.world.update_with_collision(1.0, eye, [0.0, 0.0, -1.0], &mut host, None, |_| None);
        assert!(z.world.take_sounds().is_empty());
        // a player is struck: Male / FemaleGetsHit, material 7
        if victim != att {
            let Look::Char(l) = &z.world.chars[&victim].look else { unreachable!() };
            let (breed, sex) = (l.breed, l.sex);
            z.world.hit_seen(att, HitCtx { victim, slot: 0, damage: 4, flags: 4 });
            z.world.note_sounds(att, 0xb, 0);
            z.world.take_sounds();
            let vpos = scene_pos(z.world.chars[&victim].pose.pos);
            z.world.update_with_collision(0.5, eye, [0.0, 0.0, -1.0], &mut host, None, |_| None);
            let s = z.world.take_sounds();
            match crate::play::combat::notes::player_impact(breed, sex) {
                Some((7, Some(name))) => assert_eq!(s, [GameSound { id: ao_audio::sbf::sound_id(name), pos: vpos, material: 7, size: 1 }]),
                _ => assert!(s.is_empty(), "{s:?}"),
            }
        }
    }

    /// Notes `swish_*` / `attack_start_N`: a player plays `SM_Sandy_Swish_*` (the holder's own ids), a creature the list of its record; the creature's
    /// `attack_start` list is silent for players.
    #[test]
    fn swish_and_attack_start_notes() {
        let Some((mut z, player, leet)) = fight_zone() else { return };
        let (pp, lp) = (scene_pos(z.world.chars[&player].pose.pos), scene_pos(z.world.chars[&leet].pose.pos));
        z.world.note_sounds(player, 0x73, -1);
        z.world.note_sounds(player, 0x75, -1);
        z.world.note_sounds(player, 0x77, -1);
        let sid = ao_audio::sbf::sound_id;
        assert_eq!(z.world.take_sounds(), [GameSound::at(sid("SM_Sandy_Swish_punch"), pp), GameSound::at(sid("SM_Sandy_Swish_tail"), pp)]);
        let rec = {
            let c = &z.world.chars[&leet];
            let Some(Model::Ready { built, .. }) = z.world.models.get(&c.key) else { panic!("leet model not ready") };
            built.sounds.clone()
        };
        for note in [0x73u32, 0x77, 0x78] {
            z.world.note_sounds(leet, note, -1);
            let got = z.world.take_sounds();
            let want = rec.iter().find(|s| s.0 == note).map_or(&[][..], |s| &s.1[..]);
            assert_eq!(got.len(), usize::from(!want.is_empty()), "note {note:#x}: {want:?}");
            assert!(got.iter().all(|g| want.contains(&g.id) && g.pos == lp));
        }
    }

    /// The Beach Leet's authored attack notes fire once per independent holder node.
    #[test]
    fn a_holder_swing_clip_reports_its_notes() {
        let Some((mut z, _, leet)) = fight_zone() else { return };
        let mut host = Host::headless();
        let (eye, fwd) = (scene_pos(z.own().unwrap().pos), [0.0, 0.0, -1.0]);
        z.world.take_notes();
        z.world.play_swing(leet, None, canim::list::ATTACK);
        let mut notes = vec![];
        for _ in 0..80 {
            z.world.update_with_collision(0.05, eye, fwd, &mut host, None, |_| None);
            host.actors.clear();
            notes.extend(z.world.take_notes());
        }
        eprintln!("leet attack clip notes: {notes:?}");
        let mut uniq = notes.clone();
        uniq.dedup();
        assert_eq!(notes.len(), uniq.len(), "every note fires once");
        assert!(!notes.is_empty() && notes.iter().all(|n| n.0 == leet));
    }

    /// The `FabricType` (NPC record stat 41, the impact material `FUN_1004d8e6(0x29)`) is 0 for nearly every creature record: only a handful carry
    /// a material, so a creature's own list-0x1f impact sound is rare and a hit on a player (material 7) is the common impact.
    #[test]
    fn creature_records_carry_a_fabric_type() {
        let Some(dir) = client() else { return };
        let store = RecordStore::open(&dir).unwrap();
        let (mut all, mut fabric, mut hist) = (0, 0, std::collections::BTreeMap::new());
        for id in store.ids(ao_formats::character::NPC_TYPE).unwrap() {
            let Ok(r) = NpcRecord::load(&store, id) else { continue };
            all += 1;
            let f = r.stat(super::super::combat::notes::STAT_FABRIC_TYPE).unwrap_or(0);
            fabric += usize::from((1..=17).contains(&f));
            *hist.entry(f).or_insert(0) += 1;
        }
        eprintln!("{all} creature records, {fabric} with a FabricType 1..=17: {hist:?}");
        assert!(fabric > 0 && fabric * 20 < all, "{fabric} of {all}");
    }

    /// `FUN_10058cfa` (the `GetImpactAnim` override of `SimpleChar`): one of `0x81 0x82 0x80 0x84 0x7f`; the struck leet plays the `imp-*` clip once,
    /// at half speed unless the hit kind is 4.
    #[test]
    fn a_struck_creature_plays_an_impact_clip() {
        let Some((mut z, _, leet)) = fight_zone() else { return };
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..40 {
            seen.insert(z.world.impact_anim());
        }
        assert_eq!(seen.into_iter().collect::<Vec<_>>(), [0x7f, 0x80, 0x81, 0x82, 0x84], "crawling (0xd0) is not tracked");
        let (anim, rate) = z.world.react_to_hit(leet, 3);
        assert_eq!(rate, 0.5);
        let mut host = Host::headless();
        let (eye, fwd) = (scene_pos(z.own().unwrap().pos), [0.0, 0.0, -1.0]);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            z.world.update_with_collision(0.05, eye, fwd, &mut host, None, |_| None);
            host.actors.clear();
            if matches!(z.world.chars[&leet].special, Special::Once(k) if k == u32::from(anim)) { break; }
            assert!(std::time::Instant::now() < deadline, "imp clip {anim:#x} did not start; pending replay {:?}", z.world.replay);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(z.world.react_to_hit(leet, 4).1, 1.0);
    }
}
