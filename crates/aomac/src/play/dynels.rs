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
use ao_formats::character::{load_cat_mesh, AnimLayer, CatAnim, ClothPart, Equipment, NameTable, NpcRecord, TextureOverride, CHAR_MESH_TYPE};
use ao_scene::CrtRand;
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
use std::collections::{hash_map::DefaultHasher, HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use super::combat::anim as canim;
use super::dynels_doors::{Cmd, GameSound, ItemRig, PropAnim};
use super::tags::{Indicator, Listing, Tag, TagLayer};
use super::zone::{scene_pos, scene_yaw};


fn remote_pose(
    mover: &mut Mover,
    collision: Option<&std::rc::Rc<std::cell::RefCell<ao_formats::playfield::collision::Collision>>>,
    dt: f32,
) -> ao_net::n3::motion::Pose {
    let Some(collision) = collision else { return mover.advance(dt); };
    let collision = collision.borrow();
    mover.advance_with_surface(dt, |old, new, body, state| {
        let mut aligned = collision.align(scene_pos(old), scene_pos(new), body, state);
        aligned.pos = scene_pos(aligned.pos);
        aligned.normal = scene_pos(aligned.normal);
        aligned
    }, |pos| {
        let start = scene_pos(pos);
        collision.line(start, [start[0], start[1] + 100.0, start[2]]).map(|hit| hit.p[1])
    })
}
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

/// 5-plane (left/right/bottom/top/near) conservative frustum for [`Dynels::update_with_collision`]'s pre-skin cull:
/// see the call site for why this is safely never stricter than `ao_render`'s own per-mesh sphere cull. `fov_horizontal`
/// is the real camera's current horizontal FOV ([`Dynels::fov_horizontal`]): retail's fixed 90°, or wider with the aomac
/// Enhanced "Widescreen FOV" option on (audit item 16 prerequisite) so this cheap square-aspect cone keeps covering at
/// least as much horizontally as the renderer's own wider-than-90° lens.
fn skin_frustum_planes(cam: [f32; 3], fwd: [f32; 3], fov_horizontal: f32) -> [glam::Vec4; 5] {
    let cam = glam::Vec3::from(cam);
    let fwd = glam::Vec3::from(fwd).normalize_or_zero();
    let fwd = if fwd == glam::Vec3::ZERO { glam::Vec3::NEG_Z } else { fwd };
    let up = if fwd.dot(glam::Vec3::Y).abs() > 0.999 { glam::Vec3::X } else { glam::Vec3::Y };
    let vp = glam::Mat4::perspective_rh(fov_horizontal, 1.0, super::camera::NEAR, 1000.0)
        * glam::Mat4::look_to_rh(cam, fwd, up);
    let planes = ao_render::frustum_planes(&vp);
    [planes[0], planes[1], planes[2], planes[3], planes[4]]
}

/// Whether an actor's CPU skin ([`ao_formats::character::actor::ActorRig::pose_composed_cached`]) can be skipped this
/// frame (perf audit item 5): conservatively frustum-culled (see [`skin_frustum_planes`]), nothing forces a fresh
/// pose (`needs_effect_pose`), and it already has a skin to keep drawing (the first frame always skins).
fn skip_cpu_skin(skin_planes: &[glam::Vec4; 5], pos: [f32; 3], bounding_radius: f32, scale: f32, needs_effect_pose: bool, has_skin: bool) -> bool {
    let center = glam::Vec3::from(pos);
    // ponytail: the real bind sphere is centred inside the body, not at the feet; doubling the radius safely
    // over-covers that offset without plumbing the actual mesh-0 sphere centre through to this pre-GPU stage.
    let radius = (2.0 * bounding_radius + ao_render::POSE_MARGIN) * scale;
    has_skin && !needs_effect_pose && ao_render::frustum_culls_sphere(skin_planes, center, radius)
}

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
    /// `ClothData_t`: (page * 5 + part, rdb 1010004 texture).
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
            cloth: u.cloth.iter().map(|c| (c.page * 5 + c.part(), c.texture)).collect(),
            attractors,
            skip_attractors: u.flags & ao_net::n3::dynel::flag::SET_DYNEL_800 != 0,
        }
    }
}

impl CharLook {
    /// `AppearanceUpdateIIR_c::Activate` [GC 0x10071679] on a character's look, any `SimpleChar_t` (NPCs too, no kind test beyond the identity):
    /// the cloth entries are written by `FUN_100480fc` at index `page * 5 + part` (visible page priority 2 > 1 > 0, GC 0x1004b521); an entry whose texture is unchanged is
    /// left alone, one with texture 0 clears the part, parts the message does not name stay), then `VisualCATMesh_t::ClearAttractors` +
    /// `CharacterMesh::AddAttractors(list)`: the mounted set is exactly the wire list (head = its place 0), unconditionally (the `SET_DYNEL_800`
    /// flag only skips the full update's block). `cloth` is kept sorted so equal looks share a model whatever order the updates came in.
    pub fn apply_appearance(&mut self, a: &ao_net::n3::world::AppearanceUpdate) {
        for c in &a.cloth {
            let index = c.c * 5 + c.id;
            self.cloth.retain(|p| p.0 != index);
            if c.b > 0 {
                self.cloth.push((index, c.b));
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
    pub fn key(&self, texture_quality: i32) -> u64 {
        let mut h = DefaultHasher::new();
        self.hash(&mut h);
        // Reserve two bits for the native resource family so queued loads retain their identity.
        (h.finish() & !3) | match texture_quality { 1 => 1, 2 => 2, _ => 3 }
    }
}

#[cfg(test)]
#[test]
fn future_texture_family_changes_model_identity_without_mutating_existing_looks() {
    let look = Look::Item { template: Some(7), stats: vec![] };
    let full = look.key(3);
    assert_eq!(full, look.key(0));
    assert_ne!(full, look.key(1));
    assert_ne!(look.key(1), look.key(2));
    assert_eq!(full, look.key(3));
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

/// Approximate CPU-resident bytes of a built model's `Scene` (mesh vertices/indices + texture `rgba`, the
/// dominant cost); rig and clip animation data are smaller and not counted, so this intentionally under-estimates.
/// Used both to bound [`Dynels::models`] (`Dynels::MODEL_BYTE_BUDGET`) and by
/// `model_cache_cap_stays_under_a_byte_budget` to measure what a real built model actually costs.
fn built_bytes(built: &Built) -> usize {
    let meshes: usize = built.model.meshes.iter().map(|m| {
        m.vertices.len() * std::mem::size_of::<ao_scene::Vertex>() + m.submeshes.iter().map(|s| s.indices.len() * 4).sum::<usize>()
    }).sum();
    let textures: usize = built.model.textures.values().map(|t| t.rgba.len()).sum();
    meshes + textures
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
    Model { key: u64, look: Look, texture_quality: i32 },
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
    fn start(dir: PathBuf, names: Option<Arc<NameTable>>) -> Worker {
        let (tx, req_rx) = channel::<Req>();
        let (resp_tx, rx) = channel();
        std::thread::spawn(move || {
            let store = match RecordStore::open(&dir) {
                Ok(store) => store,
                Err(e) => { eprintln!("dynels: rdb: {e:#}"); return; }
            };
            // Reuses `Dynels::start`'s already-loaded name table when available instead of parsing rdb
            // 1000010 a second time; only a fresh session with a failed earlier load re-parses it here.
            let names = match names {
                Some(names) => names,
                None => match NameTable::load(&store) {
                    Ok(names) => Arc::new(names),
                    Err(e) => { eprintln!("dynels: rdb: {e:#}"); return; }
                },
            };
            let mut assets = ActorAssets::with_names(names);
            for r in req_rx {
                let resp = match r {
                    Req::Model { key, look, texture_quality } => {
                        store.set_other_texture_quality(texture_quality);
                        Resp::Model { key, result: build(&store, &mut assets, &look).map(Box::new).map_err(|e| format!("{e:#}")) }
                    }
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
        for (part, tex) in super::avatar::visible_cloth(&look.cloth) {
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
    // Resolve posture transitions with the model, not after the first visible sit/stand.
    for &id in STANCE_IDS.iter().chain([0xd5, 0xd6, 0xed, 0xef, 0x68, 0x69].iter()) {
        let ids: Vec<u32> = match &rec {
            Some(rec) if matches!(id, 0xd5 | 0xd6 | 0xed | 0xef | 0x68 | 0x69) => ao_formats::character::anim_key_variants(rec, id as u32).to_vec(),
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
    let stats = rec.as_ref().map(|r| r.stats.clone()).unwrap_or_default();
    let sounds = rec.map(|r| r.sounds).unwrap_or_default();
    Ok(Built { model: rig.model().clone(), rig: Some(Arc::new(rig)), clips, features, tag_height, sounds, fabric, stats, ..plain(Default::default(), true) })
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
pub(super) fn npc_rig(store: &RecordStore, assets: &ActorAssets, look: &CharLook, rec: &NpcRecord, head: Option<u32>, attachments: &[(u8, u32)]) -> anyhow::Result<ActorRig> {
    let model = rec.mesh().context("NPC record has no mesh")?;
    let cat = load_cat_mesh(store, CHAR_MESH_TYPE, model)?;
    // DS 10073110 registers mounts; Randy 100577d3 processes a child only
    // when GetAttractor succeeds. Keep the retained humanoid head for undo,
    // but do not require its mount point on a different MonsterData skeleton.
    let head = head.filter(|_| cat.attractors.iter().any(|a| a.name.ends_with("_head")));
    let list: Vec<TextureOverride> = look.textures.iter().map(|(m, t, env, alpha)| TextureOverride { material: m, texture: *t as u32, env_texture: *env as u32, alpha_mode: *alpha as u32 }).collect();
    let cloth: Vec<(ClothPart, u32)> = super::avatar::visible_cloth(&look.cloth).filter_map(|(p, t)| Some((*ClothPart::ALL.get(p as usize)?, t as u32))).collect();
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
    /// Raw MechData (662); native MonsterData writes rebuild only outside a mech.
    mech_data: i32,
    pub side: u8,
    /// Stat `Flags` (0) and `VisualFlags` (0x2A1), name tag inputs.
    flags: i32,
    visual_flags: i32,
    /// Full-update InPlay and attachment state (GC 0x10077af2 / GUI 0x10024d5b).
    in_play: bool,
    /// n3VisualDynel+0xc9: separate from InPlay (death completion can hide an in-play NPC).
    visible: bool,
    parent: Option<ao_net::msg::Identity>,
    pose: ao_net::n3::motion::Pose,
    anim: u32,
    special: Special,
    posture_idle: Option<u32>,
    restart_body: bool,
    body_delay_ms: f32,
    actions: Vec<ActionAnim>,
    base_layers: u32,
    clip_ms: f32,
    clip_rate: f32,
    body_once: bool,
    body_loop: bool,
    previous_body: Option<PreviousBody>,
    terminal_pose: bool,
    submitted: bool,
    /// Mount transforms of the last pose.
    parts: Vec<[[f32; 4]; 4]>,
    features_set: bool,
    /// Model-space box of the last pose's skinned body vertices (`RCATMesh_t+0x1fc/+0x208`, hud_pick.rs); `None` until posed.
    bounds: Option<([f32; 3], [f32; 3])>,
    current_skin: Option<Arc<Vec<ao_scene::Vertex>>>,
    cadence: ao_formats::character::AnimationCadence,
    pose_cache: ao_formats::character::actor::PoseCache,
    roll: Roll,
    /// Bit `i` = event `i` of the playing clip has fired its note (`piVar7[0xd]` of the holder's clip entry, `FUN_1003c036`).
    note_fired: u32,
    /// Local clip time ([`super::avatar::clip_time`]) as of the last tick, to detect a looping clip's wrap and re-arm
    /// `note_fired` for its next cycle (the base walk/run/jump clip's own step/land notes, not just the overlay actions).
    body_phase: f32,
}

/// DS 0x10073f23 keeps the outgoing body alive on its own playback clock.
struct PreviousBody {
    clip: Arc<CatAnim>,
    ms: f32,
    rate: f32,
    once: bool,
    elapsed: f32,
    fade_ms: f32,
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
    posture: bool,
    before_body: bool,
}

impl ActionAnim {
    fn duration(&self, clip: &CatAnim) -> f32 {
        if self.posture { super::avatar::posture_interval_end(clip) } else { clip.duration }
    }
    fn advance(&mut self, clip: &CatAnim, dt: f32, mut emit: impl FnMut(super::combat::notes::FiredNote)) -> bool {
        use super::combat::notes::{fire, finish, id, FiredNote};
        let duration = self.duration(clip);
        let slot = self.slot;
        let mut emit_note = |note| {
            if slot >= 0 || !matches!(note, id::ATTACK | id::ATTACK_EFFECT_1..=id::ATTACK_EFFECT_4) {
                emit(FiredNote { id: note, slot });
            }
        };
        if self.ms >= duration {
            if !self.posture { for id in finish(&clip.events, &mut self.note_fired) { emit_note(id); } }
            return false;
        }
        if self.ms == 0.0 {
            if let Some(delay) = self.delay {
                self.rate = canim::swing_speed_scale(clip.events.first().map_or(0.0, |e| e.0 as f32), delay);
            }
        }
        self.ms += (dt * 1000.0 * self.rate).abs();
        for id in fire(&clip.events, self.ms.min(duration), &mut self.note_fired) { emit_note(id); }
        true
    }
}

impl Char {
    fn base_priority(&self) -> i32 {
        if matches!(self.special, Special::Die(_)) { -3 } else { 0 }
    }

    fn layers<'a>(&'a self, built: &'a Built) -> impl Clone + Iterator<Item = AnimLayer<'a>> {
        let priority = self.base_priority();
        let previous = self.previous_body.as_ref().map(|body| AnimLayer {
            clip: &body.clip, ms: super::avatar::clip_time(&body.clip, body.ms, body.once),
            layers: self.base_layers, blend: 1.0,
        });
        let blend = self.previous_body.as_ref().map_or(1.0, |body| (body.elapsed / body.fade_ms).clamp(0.0, 1.0));
        let base = built.clips.get(&self.anim)
            .and_then(|clips| clips.get(self.roll.variant % clips.len().max(1)))
            .map(|clip| AnimLayer { clip, ms: super::avatar::clip_time(clip, self.clip_ms, self.body_once), layers: self.base_layers, blend });
        let action_layer = |action: &'a ActionAnim| {
            let clips = built.clips.get(&action.id)?;
            let clip = clips.get(action.variant? % clips.len().max(1))?;
            Some(AnimLayer { clip, ms: action.ms.min(action.duration(clip)), layers: action.layers, blend: ao_formats::character::animation_blend(action.ms, action.duration(clip), action.fade_ms) })
        };
        let before = self.actions.iter().filter(move |action| action.priority > priority).filter_map(action_layer);
        let older = self.actions.iter().filter(move |action| action.priority == priority && action.before_body).filter_map(action_layer);
        let after = self.actions.iter().filter(move |action| action.priority < priority || (action.priority == priority && !action.before_body)).filter_map(action_layer);
        before.chain(previous).chain(older).chain(base).chain(after)
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
fn advance_clip_clock(ms: &mut f32, rate: &mut f32, dt: f32, started: bool, start_rate: impl FnOnce() -> f32) {
    if started {
        *rate = start_rate();
    }
    *ms += (dt * 1000.0 * *rate).abs();
}

fn movement_loop(state: AnimState) -> bool {
    matches!(state, AnimState::Idle | AnimState::Walk | AnimState::WalkBack | AnimState::WalkLeft | AnimState::WalkRight | AnimState::Run | AnimState::RunBack | AnimState::TurnLeft | AnimState::TurnRight | AnimState::Swim | AnimState::IdleSwim | AnimState::SitGround | AnimState::Sleep | AnimState::Lounge | AnimState::IdleCrawl)
}

/// GC 0x1006be27 -> 0x100108be interval mode 2 applies to remote characters too.
/// Return the authored first sample and notes skipped before that interval.
fn movement_interval(clip: &CatAnim, state: AnimState) -> (f32, u32) {
    let start = if movement_loop(state) {
        super::avatar::loop_span(clip).map_or(0.0, |span| span.0)
    } else { 0.0 };
    let notes = clip.events.iter().enumerate().filter(|(_, (ms, _))| (*ms as f32) < start).fold(0, |mask, (i, _)| mask | 1u32 << (i & 31));
    (start, notes)
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
    /// Shared with `effects::Renderer` and the model-building worker, loaded once per [`Dynels::start`]
    /// (both used to each parse rdb 1000010 independently, retaining two copies of the ~8MB name table).
    names: Option<Arc<NameTable>>,
    worker: Option<Worker>,
    texture_quality: i32,
    own: i32,
    own_pick: Option<super::hud_pick::PickBody>,
    pub chars: HashMap<i32, Char>,
    /// Non-character dynels by `(identity kind, instance)`.
    props: HashMap<(i32, i32), Prop>,
    next_prop: u32,
    /// Looks asked for before the worker existed.
    pending: Vec<(u64, Look)>,
    /// `AnimSet` of the weapon in the right (slot 6) / left (slot 8) hand of a holder.
    wield: HashMap<i32, [Option<Wield>; 2]>,
    /// Characters whose fight controller is in state 2 (`SimpleChar+0x1d4 +0x44`, fed by [`Dynels::set_fighting`]): they stand in the weapon's
    /// fight idle (list 0x10), everyone else in the idle of the equip routine (`canim::peace_idle`).
    fighting: HashSet<i32>,
    /// Holders whose weapon resolved since the last [`Dynels::take_wielded`] (a weapon wielded during a fight runs the fight idle update again).
    wielded: Vec<i32>,
    /// Weapon-slot tables of the characters: stat `DamageType` of the item behind an `AttackInfo` slot (the hit lines of the chat log).
    pub arms: super::combat::arms::Armory,
    /// `(slot, elapsed since the last swing or fight start)` of the own character's wielded hands while fighting: backs the
    /// HUD `TimerSystemModule_t::StartTimerBar` kind-1 "Attack" bars (`Dynels::update_attack_bars`, `hud_stats::update_attack_timers`).
    attack_bars: HashMap<i32, f32>,
    /// `(elapsed, total)` of the server-acknowledged Reload bar (kind 5).
    reload_bar: Option<(f32, f32)>,
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
    /// Content-hash-keyed CPU model cache, intentionally kept across zone changes (the same NPC/item look recurs
    /// in many playfields); bounded by [`Dynels::MODEL_BYTE_BUDGET`] (tracked in `model_bytes`), evicted in
    /// [`Dynels::clear`] via `model_order`.
    models: HashMap<u64, Model>,
    /// Running sum of [`built_bytes`] over `models`' `Model::Ready` entries; what [`Dynels::evict_models`] compares
    /// against [`Dynels::MODEL_BYTE_BUDGET`].
    model_bytes: usize,
    /// First-seen insertion order of `models`' keys, for the FIFO eviction in [`Dynels::clear`].
    model_order: VecDeque<u64>,
    asked: HashSet<u64>,
    /// The own avatar's [`ActorAssets`], parked here (`Dynels::stash_avatar_assets`) between zone changes so its
    /// `rest`/`clips`/`anims` caches survive the per-zone `Player` rebuild instead of resolving the model's rest
    /// pose from scratch every time (`player::Player::new`, `zone-preload-proposal.md` §2.1).
    avatar_assets: Option<ActorAssets>,
    scene_generation: u64,
    /// Login pref `ShowAllNames` (default on, GUI 0x10124b33).
    pub show_all_names: bool,
    /// Char pref `IsOrgNameShownOverHead` (default on, GUI 0x1012447a).
    show_org_names: bool,
    /// `DisplayCharViewDistance` in metres (default 80, `FUN_1001f964` N3 0x1001f964; docs/chat/dvalue.md): characters farther from the
    /// viewer are not drawn (`n3VisualDynel_t::Run`, N3 0x100196bd: squared distance to the controlled dynel on ground playfields).
    pub char_view_distance: f32,
    /// Real horizontal field of view the active camera renders this frame ([`super::camera::horizontal_fov`]), set by
    /// `flow.rs` each frame: widens [`skip_cpu_skin`]'s conservative pre-skin cull cone along with the aomac Enhanced
    /// "Widescreen FOV" option (audit item 16 prerequisite) so it never culls something the renderer would still draw.
    /// Retail (and the default) is the fixed 90° [`super::camera::FOV_HORIZONTAL`].
    pub fov_horizontal: f32,
    /// Lens of the playfield scene (projection of the name tags).
    pub lens: Lens,
    /// The tag sprites held by the renderer and the 2 s nametag listing (`play/tags.rs`).
    tags: TagLayer,
    listing: Listing,
    /// Native thread CRT shared with wildlife and effect generation; fixtures retain an isolated seed.
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
    /// Result action0x6b arriving before the accepted relay's frame drain.
    nano_cast_results: HashMap<(i32, i32), i32>,
    /// Own controller+8/+c: local admission effect, distinct from relay stat428.
    local_nano_initiation: Option<(i32, i32, u32)>,
    /// Controller+0x40 entries: caster, native lock identity, whole seconds remaining.
    nano_locks: Vec<(i32, ao_net::msg::Identity, i32)>,
    nano_lock_second: f32,
    nano_animations: Vec<Option<(u32, bool)>>,
    nano_sounds: Vec<(u32, [f32; 3], f32, f32, u32, i32)>,
    nano_store: Option<RecordStore>,
    nano_templates: HashMap<i32, Arc<ao_formats::dynel_visual::ItemTemplate>>,
    /// Inventory templates are record type 1000020, not the nano cache's 1040005.
    inventory_templates: HashMap<i32, Option<ao_formats::dynel_visual::ItemTemplate>>,
    inventory_acg_templates: HashMap<(i32, i32, i32), Option<ao_formats::dynel_visual::ItemTemplate>>,
    pub nano_effect_categories: u32,
    /// Retail effect category bits: muzzle=8, tracers/hits=2.
    pub weapon_effect_categories: u32,
    /// Active swing target and clip slot, independent of the retained slot flags.
    swings: HashMap<i32, (i32, i32)>,
    impact_locations: HashMap<i32, i32>,
    /// Camera position of the last [`Dynels::update_with_collision`] (scene space).
    cam: [f32; 3],
    collision: Option<std::rc::Rc<std::cell::RefCell<ao_formats::playfield::collision::Collision>>>,
}

struct NanoCast {
    who: i32,
    spell: i32,
    target: i32,
    handle: u32,
    remaining: f32,
    /// `remaining`'s starting value (`nano_cast_delay` at cast start): backs the HUD "Nano program" timer bar
    /// (`nano_bar_progress`, kind 3 "Timer_Nano").
    total: f32,
    finish: [i32; 2],
    release_anim: u32,
    released: bool,
    release_seen: bool,
    done: bool,
    instant: bool,
    execution: Option<NanoExecution>,
    finish_enabled: bool,
    start_effect: i32,
}

/// GC10050ed9/100500ca/1004e68c: internal kind3 survives attack and recharge.
/// Visual release/finish callbacks do not remove it.
struct NanoExecution {
    left: f32,
    recharge: Option<f32>,
    /// GC10051078: only an empty execution queue allocates a TimerEntry.
    timer: bool,
    /// Native +0x1c awaits a result; +0x20 defaults to success 1.
    result: Option<i32>,
}

impl NanoExecution {
    fn advance(&mut self, dt: f32) -> bool {
        self.left -= dt;
        if self.left > 0.0 || (self.left == 0.0 && self.recharge.is_none()) { return true; }
        if self.recharge.is_some() && self.result.is_none() { return true; }
        if let Some(recharge) = self.recharge.take() {
            // Native starts recharge at this frame's time, not the old attack deadline.
            self.left = recharge;
            true
        } else {
            false
        }
    }
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
            names: None,
            worker: None,
            texture_quality: 3,
            own: 0,
            own_pick: None,
            chars: HashMap::new(),
            props: HashMap::new(),
            next_prop: PROP_ID_BASE,
            pending: vec![],
            wield: HashMap::new(),
            fighting: HashSet::new(),
            wielded: vec![],
            arms: Default::default(),
            attack_bars: HashMap::new(),
            reload_bar: None,
            swing_delay: HashMap::new(),
            once_rate: HashMap::new(),
            weapons: HashMap::new(),
            pending_weapons: vec![],
            pending_clips: vec![],
            replay: vec![],
            want_placed: None,
            playfield: None,
            models: HashMap::new(),
            model_bytes: 0,
            model_order: VecDeque::new(),
            asked: HashSet::new(),
            avatar_assets: None,
            scene_generation: 0,
            show_all_names: true,
            show_org_names: true,
            char_view_distance: 80.0,
            fov_horizontal: super::camera::FOV_HORIZONTAL,
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
            nano_cast_results: HashMap::new(),
            local_nano_initiation: None,
            nano_locks: vec![],
            nano_lock_second: 0.0,
            nano_animations: vec![],
            nano_sounds: vec![],
            nano_store: None,
            nano_templates: HashMap::new(),
            inventory_templates: HashMap::new(),
            inventory_acg_templates: HashMap::new(),
            nano_effect_categories: 36,
            weapon_effect_categories: 10,
            swings: HashMap::new(),
            impact_locations: HashMap::new(),
            cam: [0.0; 3],
            collision: None,
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
    /// Model readiness and per-instance triangles of actually skinned remote CAT bodies in the headless input fixture.
    #[cfg(test)]
    pub(super) fn cpu_benchmark_scene(&self) -> (bool, usize) {
        let mut keys = self.chars.iter().filter(|(id, _)| **id != self.own)
            .map(|(_, c)| c.next.unwrap_or(c.key))
            .chain(self.props.values().map(|p| p.key));
        let ready = self.want_placed.is_none()
            && keys.all(|key| matches!(self.models.get(&key), Some(Model::Ready { .. })));
        let triangles = self.chars.iter().filter(|(id, _)| **id != self.own).filter_map(|(_, c)| {
            c.current_skin.as_ref()?;
            c.bounds?;
            let Model::Ready { built, .. } = self.models.get(&c.key)? else { return None };
            Some(built.rig.as_ref()?.body_triangles().count())
        }).sum();
        (ready, triangles)
    }

    pub fn character_template_stat(&self, id: i32, stat: u32) -> Option<i32> {
        let c = self.chars.get(&id)?;
        let Model::Ready { built, .. } = self.models.get(&c.key)? else { return None };
        ao_formats::dynel_visual::get(&built.stats, stat)
    }

    fn cache_inventory_template(&mut self, low: i32) -> Option<()> {
        if !self.inventory_templates.contains_key(&low) {
            let store = self.nano_store.as_ref()?;
            let template = match u32::try_from(low).ok().map(|id| ao_formats::dynel_visual::item_template(store, id)).transpose() {
                Ok(template) => template.flatten(),
                Err(error) => { eprintln!("inventory template {low}: {error:#}"); None }
            };
            self.inventory_templates.insert(low, template);
        }
        self.inventory_templates.get(&low)?.as_ref().map(|_| ())
    }

    /// GC 10082a3d/100cc270: Can and Flags come from the nearest ACG endpoint, tie high.
    pub(super) fn item_template_stat(&mut self, item: ao_net::n3::world::AcgItem, stat: u32) -> Option<i32> {
        let key = (item.low_id, item.high_id, item.level);
        if !self.inventory_acg_templates.contains_key(&key) {
            self.cache_inventory_template(item.low_id)?;
            self.cache_inventory_template(item.high_id)?;
            let low = self.inventory_templates.get(&item.low_id)?.as_ref()?;
            let high = self.inventory_templates.get(&item.high_id)?.as_ref()?;
            let template = match ao_formats::dynel_visual::interpolate_item_templates(low, high, item.level) {
                Ok(template) => Some(template),
                Err(error) => { eprintln!("inventory ACG {item:?}: {error:#}"); None }
            };
            self.inventory_acg_templates.insert(key, template);
        }
        self.inventory_acg_templates.get(&key)?.as_ref()?.stat(stat)
    }

    #[cfg(test)]
    pub(super) fn test_inventory_can(&mut self, low: i32, can: i32) {
        self.test_inventory_endpoint(low, 1, can, 0);
    }

    #[cfg(test)]
    pub(super) fn test_inventory_endpoint(&mut self, id: i32, ql: i32, can: i32, flags: i32) {
        self.inventory_acg_templates.clear();
        self.inventory_templates.insert(id, Some(ao_formats::dynel_visual::ItemTemplate { kind: 0, name: None, sounds: vec![], stats: vec![(54, ql), (30, can), (0, flags)] }));
    }

    pub fn object_position(&self, id: ao_net::msg::Identity) -> Option<[f32; 3]> {
        self.props.get(&(id.kind, id.instance)).map(|p| p.pos)
    }

    /// Current remote vehicle pose in server coordinates, before the render-space Z flip.
    pub fn character_position(&self, id: i32) -> Option<[f32; 3]> {
        self.chars.get(&id).map(|c| c.pose.pos)
    }

    /// N3 `10019629` caches MeshMeta, not the posed CAT bounds; `10019b51/10019b69`
    /// scale its radius and vertical centre. Missing resource rows have native zero bounds.
    pub(super) fn character_trade_bounds(&self, id: i32) -> anyhow::Result<Option<(f32, f32)>> {
        let Some(c) = self.chars.get(&id) else { return Ok(None) };
        let Some(store) = self.nano_store.as_ref() else { return Ok(None) };
        let Look::Char(look) = &c.look else { return Ok(None) };
        let model = match monster_record(store, look)? {
            Some(record) => record.mesh().context("NPC record has no mesh")?,
            None => {
                let Some(names) = self.names.as_ref() else { return Ok(None) };
                let (breed, gender) = ao_formats::screens::wire_breed_sex(look.breed as i32, look.sex as i32)?;
                ao_formats::character::player_model_build_with_names(names, breed, gender, look.fatness.min(2))?
            }
        };
        let Some(data) = store.get(1_000_047, model)? else { return Ok(Some((0.0, 0.0))) };
        anyhow::ensure!(data.len() == 28, "MeshMeta {model}: expected 28 bytes, got {}", data.len());
        let centre_y = f32::from_le_bytes(data[4..8].try_into().unwrap());
        let radius = f32::from_le_bytes(data[12..16].try_into().unwrap());
        Ok(Some((centre_y * c.scale, radius * c.scale)))
    }

    /// GC1004faec reads the scaled native torso collision sphere, not a
    /// targeting box or an estimated humanoid radius.
    pub fn nano_target_geometry(&self, id: i32) -> Option<([f32; 3], f32)> {
        let c = self.chars.get(&id)?;
        let Model::Ready { built, .. } = self.models.get(&c.key)? else { return None };
        let radius = built.rig.as_ref()?.cat().torso_sphere.radius;
        let radius = if radius < 0.0 { ao_formats::playfield::collision::DEFAULT_BODY_RADIUS } else { radius };
        Some((c.pose.pos, radius * c.scale))
    }

    /// GC10058908: ordinary player casts use the playfield LOS query at
    /// feet + 1.6m. An unavailable surface is not an unobstructed ray.
    pub fn nano_line_of_sight(&self, from: [f32; 3], to: [f32; 3]) -> Option<bool> {
        let collision = self.collision.as_ref()?.borrow();
        let mut from = scene_pos(from);
        let mut to = scene_pos(to);
        from[1] += 1.6;
        to[1] += 1.6;
        Some(collision.line(from, to).is_none())
    }

    pub fn texture_quality(&self) -> i32 { self.texture_quality }

    /// Native CAT observer changes the resource family for future loads, not cached models.
    pub fn set_texture_quality(&mut self, quality: i32) {
        self.texture_quality = quality;
        if let Some(store) = &self.nano_store { store.set_other_texture_quality(quality); }
        if let Some(effects) = &self.effects { effects.set_texture_quality(quality); }
    }

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
        self.rng = CrtRand::thread();
        self.arms = super::combat::arms::Armory::open(&dir);
        self.calibration = super::avatar::Calibration::load(&dir);
        self.nano_store = match RecordStore::open(&dir) {
            Ok(store) => Some(store),
            Err(error) => { eprintln!("nano templates: {error:#}"); None }
        };
        self.nano_templates.clear();
        // Loaded once and shared with `effects::Renderer` and the model-building worker's `ActorAssets`,
        // instead of each independently parsing rdb 1000010 (a ~8MB name table) into its own copy.
        self.names = self.nano_store.as_ref().and_then(|store| match NameTable::load(store) {
            Ok(names) => Some(Arc::new(names)),
            Err(error) => { eprintln!("name table: {error:#}"); None }
        });
        self.effects = match &self.names {
            Some(names) => super::combat::effects::Renderer::open_with_names(&dir, names.clone()),
            None => super::combat::effects::Renderer::open(&dir),
        }.map_err(|error| eprintln!("weapon effects: {error:#}")).ok();
        self.dir = Some(dir);
    }

    /// Forgets every dynel (a playfield change, `Zone::reset_world`). `models` survives (the GPU-ready builds are
    /// reused across playfields for repeated looks) but is capped: see [`Dynels::evict_models`].
    pub fn clear(&mut self) {
        let keep: HashSet<u64> = self.chars.values().map(|c| c.key).chain(self.props.values().map(|p| p.key)).collect();
        self.chars.clear();
        self.collision = None;
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
        self.nano_cast_results.clear();
        self.local_nano_initiation = None;
        self.nano_locks.clear();
        self.nano_lock_second = 0.0;
        self.nano_animations.clear();
        self.nano_sounds.clear();
        if let Some(effects) = &mut self.effects { effects.clear(); }
        self.props.clear();
        self.weapons.clear();
        self.wield.clear();
        self.fighting.clear();
        self.wielded.clear();
        self.pending_weapons.clear();
        self.evict_models(&keep);
    }

    /// `models` is a content-hash cache, deliberately kept across zone changes so a repeated look (the same NPC
    /// or item template seen in another playfield) does not rebuild. Left unbounded it grows for the whole
    /// session: measured live, ~440-540 MB added by one large outdoor playfield's distinct looks, never freed on
    /// return to a small zone (`zone-preload-proposal.md` §3). Capped here, at the one point a look stops being
    /// "the current zone's own" (a zone change): FIFO by first-seen order, skipping `keep` (this zone's own keys,
    /// just used, most likely to be reused again right away). Byte-based, not count-based: a real capture's
    /// average built model (mesh + texture `rgba`) measured ~720 KB (`model_cache_cap_stays_under_a_byte_budget`),
    /// so a flat count cap sized for a quiet zone (hundreds of entries) would blow well past a sane memory budget
    /// in a zone full of large/varied looks; `model_bytes` tracks the running total exactly for that reason.
    /// ponytail: first-seen-order eviction, not true LRU (no per-access touch, cheap for the ~hundreds of keys a
    /// session sees).
    const MODEL_BYTE_BUDGET: usize = 500_000_000;
    fn evict_models(&mut self, keep: &HashSet<u64>) {
        let mut scanned = 0;
        while self.model_bytes > Self::MODEL_BYTE_BUDGET && scanned < self.model_order.len() {
            let Some(key) = self.model_order.pop_front() else { break };
            if keep.contains(&key) {
                self.model_order.push_back(key);
                scanned += 1;
                continue;
            }
            if let Some(Model::Ready { built, .. }) = self.models.remove(&key) {
                self.model_bytes = self.model_bytes.saturating_sub(built_bytes(&built));
            }
            self.asked.remove(&key);
        }
    }

    /// `AnimSet` of the weapon a character wields, slot 6 (right hand) before 8 (left): the weapon whose lists the idle / walk / run stance uses
    /// (AnimHolder idle update `FUN_1003cad0`, docs/zone/combat-anim.md §4). `None`: nothing wielded or the item is not resolved yet.
    pub fn wielded_set(&self, id: i32) -> Option<i32> {
        self.wield.get(&id)?.iter().flatten().next().map(|w| w.set)
    }

    /// Takes the warmed own-avatar `ActorAssets` for [`player::Player::new`] to reuse, if one survived the last
    /// zone change (`None` on the very first build of a session).
    pub fn take_avatar_assets(&mut self) -> Option<ActorAssets> {
        self.avatar_assets.take()
    }

    /// Parks the outgoing `Player`'s `ActorAssets` for the next zone's avatar build ([`Dynels::take_avatar_assets`]).
    pub fn stash_avatar_assets(&mut self, assets: ActorAssets) {
        self.avatar_assets = Some(assets);
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
                return;
            }
        }
        self.pending_clips.push((c.key, look.clone(), anim_id, id));
    }

    /// GC 0x1006d330 queues the delayed infinite idle first, then the finite enter at priority zero.
    pub fn play_posture(&mut self, id: i32, anim_id: u32) {
        let Some(c) = self.chars.get(&id) else { return };
        // Every supported posture was loaded with the model; never replay a stale worker response.
        if !matches!(self.models.get(&c.key), Some(Model::Ready { built, .. }) if built.clips.contains_key(&anim_id)) { return; }
        let Some(c) = self.chars.get_mut(&id).filter(|c| !matches!(c.special, Special::Die(_))) else { return };
        c.posture_idle = Some(anim_id);
        c.special = Special::None;
        for action in &mut c.actions { if action.priority == 0 { action.before_body = true; } }
        self.start_node(id, ActionAnim { id: anim_id, layer: canim::Layer::Body, priority: 0, layers: 3, fade_ms: 200.0, ms: 0.0, variant: None, note_fired: 0, rate: 1.0, key: None, slot: -1, delay: None, posture: true, before_body: false });
    }

    /// Original producer's layer wins over the generic name-table mapping.
    pub fn play_action(&mut self, id: i32, anim_id: u32, layer: canim::Layer, priority: i32) {
        self.start_node(id, ActionAnim { id: anim_id, layer, priority, layers: 3, fade_ms: 200.0, ms: 0.0, variant: None, note_fired: 0, rate: 1.0, key: None, slot: -1, delay: None, posture: false, before_body: false });
    }

    fn start_node(&mut self, id: i32, mut node: ActionAnim) {
        let Some(c) = self.chars.get_mut(&id).filter(|c| !matches!(c.special, Special::Die(_))) else { return };
        let Look::Char(look) = &c.look else { return };
        if node.layer == canim::Layer::Body && matches!(c.special, Special::Once(_)) {
            let outgoing = ActionAnim {
                id: c.anim, layer: canim::Layer::Body, priority: -1, layers: c.base_layers, fade_ms: 200.0,
                ms: c.clip_ms, variant: Some(c.roll.variant), note_fired: c.note_fired,
                rate: c.clip_rate, key: None, slot: -1, delay: None, posture: false, before_body: false,
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
            c.body_phase = 0.0;
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
        self.start_node(id, ActionAnim { id: anim, layer: canim::Layer::Body, priority: -1, layers: 3, fade_ms: 200.0, ms: 0.0, variant: None, note_fired: 0, rate: 1.0, key: Some(key), slot, delay, posture: false, before_body: false });
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
            if !matches!(c.special, Special::Die(_)) && c.anim == anim {
                c.restart_body = true;
            }
            c.special = Special::Die(anim);
        }
    }

    /// Dynel stat-flag bit 0x10, represented by the character's death state.
    pub fn is_dead(&self, id: i32) -> bool {
        self.chars.get(&id).is_some_and(|c| matches!(c.special, Special::Die(_)))
    }

    /// GC 0x1001c411 virtual +0x8c → N3 0x10019552.
    pub(super) fn character_visible(&self, id: i32) -> bool {
        self.chars.get(&id).is_some_and(|c| c.visible)
    }

    /// Wildlife vicinity queries use Type50000 positions; fly landing accepts
    /// their dead flag (0x10), never the separate Type51050 corpse props.
    pub fn life_sensors(&self, own_pos: glam::Vec3, characters: &mut Vec<super::native_life::CharacterSensor>, dead: &mut Vec<glam::Vec3>) {
        characters.clear();
        dead.clear();
        if !self.chars.contains_key(&self.own) {
            characters.push(super::native_life::CharacterSensor { pos: own_pos, radius: 0.0 });
        }
        for (&id, character) in &self.chars {
            let pos = if id == self.own { own_pos } else { glam::Vec3::from_array(character.pose.pos) };
            // Radius belongs to the separate danger sphere list; character
            // proximity is tested against the species' authored query radius.
            characters.push(super::native_life::CharacterSensor { pos, radius: 0.0 });
            if matches!(character.special, Special::Die(_)) { dead.push(pos); }
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

    /// Starts/clears the own Attack bars and advances every open one (once per tick, `combat/glue.rs`). `fighting` =
    /// `combat::module::Module::attacking()`; a slot whose hand is no longer wielded drops its bar, a newly wielded one
    /// starts fresh at zero. Returns true once when the Reload timer completes (GC10068301).
    pub fn update_attack_bars(&mut self, own: i32, fighting: bool, dt: f32) -> bool {
        if fighting {
            let slots = self.arms.attack_slots(own);
            self.attack_bars.retain(|slot, _| slots.contains(slot));
            for slot in slots {
                self.attack_bars.entry(slot).or_insert(0.0);
            }
            for v in self.attack_bars.values_mut() {
                *v += dt;
            }
        } else if !self.attack_bars.is_empty() {
            self.attack_bars.clear();
        }
        if let Some((elapsed, total)) = self.reload_bar.as_mut() {
            *elapsed += dt;
            if *elapsed >= *total {
                self.reload_bar = None;
                return true;
            }
        }
        false
    }

    /// Restarts the Attack bar of `slot` at a swing of the own character (`CombatEvent::Hit`/`Miss` whose attacker is
    /// `own`, `combat/glue.rs`). A no-op if that hand's bar is not open (the swing belongs to a special-attack key, not a
    /// tracked hand slot).
    pub fn restart_attack_bar(&mut self, slot: i32) {
        if let Some(v) = self.attack_bars.get_mut(&slot) {
            *v = 0.0;
        }
    }

    /// `(slot, progress 0..1, total seconds)` of every open Attack bar, for the HUD's TimerBarBase windows.
    pub fn attack_bar_progress(&self, own: i32, stats: &HashMap<u32, i32>) -> Vec<(i32, f32, f32)> {
        self.attack_bars.iter().filter_map(|(&slot, &elapsed)| {
            let item = self.arms.slot_item(own, slot)?;
            let total = super::combat::arms::item_round_seconds(item, stats);
            Some((slot, (elapsed / total).min(1.0), total))
        }).collect()
    }

    /// GC10069274 -> 10068a89: the acknowledged weapon's raw RechargeDelay / 100.
    pub fn start_reload_bar(&mut self, recharge: i32) {
        self.reload_bar = Some((0.0, recharge as f32 / 100.0));
    }

    /// `(progress 0..1, total seconds)` of the open Reload bar, if any.
    pub fn reload_bar_progress(&self) -> Option<(f32, f32)> {
        self.reload_bar.map(|(elapsed, total)| ((elapsed / total).min(1.0), total))
    }

    /// `(progress 0..1, total seconds)` of the own character's active nano cast, for the HUD "Nano program" timer
    /// bar (`TimerSystemModule_t::StartTimerBar` kind 3). Trigger and duration are both the already-ported
    /// `CastNanoSpellIIR_t` apply / [`nano_cast_delay`] (GC `FUN_10051754`, confirmed caller of the apply at GC
    /// 0x10072306 matching `dynel.rs`'s own documented `CastNanoSpellIIR_t` apply address; its tail constructs the
    /// `TimerEntry_t` through the same `FUN_1003d7ce` chain as Reload). `None` while not casting, once released
    /// (the effect already fired) or for an instant cast (duration 0, no bar in retail). docs/zone/combat-timers.md §4.
    pub fn nano_bar_progress(&self, own: i32) -> Option<(f32, f32)> {
        self.casting.iter().find(|c| c.who == own && !c.instant && !c.released && c.total > 0.0)
            .map(|c| ((1.0 - c.remaining / c.total).clamp(0.0, 1.0), c.total))
    }

    /// Internal TimerEntry kind3, distinct from the GUI timer bar's type enum.
    pub fn nano_busy(&self, own: i32) -> bool {
        let executing = self.casting.iter().any(|cast| cast.who == own && cast.execution.is_some());
        self.casting.iter().any(|cast| cast.who == own && cast.execution.as_ref().is_some_and(|execution| execution.timer))
            || (!executing && self.nano_casts.iter().any(|(who, cast)| *who == own && self.nano_templates.get(&cast.spell).is_some_and(|template| template.stat(0).unwrap_or(0) & 0x80000 == 0)))
    }

    /// GC1004ee95 scans executing entries (including recharge) by school.
    pub fn nano_school_busy(&self, own: i32, school: i32) -> bool {
        let same_school = |spell| self.nano_templates.get(&spell).is_some_and(|template| template.stat(0).unwrap_or(0) & 0x80000 == 0 && template.stat(0x195).unwrap_or(0) == school);
        self.casting.iter().any(|cast| cast.who == own && cast.execution.is_some() && same_school(cast.spell))
            || self.nano_casts.iter().any(|(who, cast)| *who == own && same_school(cast.spell))
    }

    /// GC1004f368: resolved, recharge-stage self programs are replacement candidates.
    pub fn nano_pending_self(&self, own: i32) -> impl Iterator<Item = (i32, &ao_formats::dynel_visual::ItemTemplate)> {
        self.casting.iter().filter(move |cast| cast.who == own && cast.target == own
            && cast.execution.as_ref().is_some_and(|execution| execution.recharge.is_none() && execution.result.is_some()))
            .filter_map(|cast| self.nano_templates.get(&cast.spell).filter(|template| template.stat(0).unwrap_or(0) & 0x10000 != 0).map(|template| (cast.spell, template.as_ref())))
    }

    /// GC1004effe: result zero preserves the queue entry's default success code 1.
    fn nano_result(&mut self, who: i32, spell: i32, result: i32) {
        let result = if result == 0 { 1 } else { result };
        for cast in self.casting.iter_mut().filter(|cast| cast.who == who && cast.spell == spell) {
            if let Some(execution) = cast.execution.as_mut().filter(|execution| execution.result.is_none()) {
                execution.result = Some(result);
            }
        }
        if self.nano_casts.iter().any(|(caster, cast)| *caster == who && cast.spell == spell) {
            self.nano_cast_results.entry((who, spell)).or_insert(result);
        }
    }

    /// GC1004ebb5: compare controller kind1 locks to all six incoming families.
    pub fn nano_family_locked(&self, own: i32, families: [i32; 6]) -> bool {
        self.nano_locks.iter().any(|&(who, lock, _)| who == own && lock.kind == 1 && families.contains(&lock.instance))
    }

    /// GC1004ebb5 also compares exact nano identities, not just family locks.
    pub fn nano_program_locked(&self, own: i32, spell: i32) -> bool {
        self.nano_locks.iter().any(|&(who, lock, _)| who == own && lock.kind == 0xcf1b && lock.instance == spell)
    }

    fn start_nano_locks(&mut self, who: i32, spell: i32) {
        let Some(template) = self.nano_templates.get(&spell) else { return };
        let seconds = template.stat(254).unwrap_or(0) / 100;
        if seconds != 0 {
            self.nano_locks.push((who, ao_net::msg::Identity { kind: 0xcf1b, instance: spell }, seconds));
        }
        for (index, family_stat) in [75, 546, 547, 548, 549, 550].into_iter().enumerate() {
            let family = template.stat(family_stat).unwrap_or(0);
            let seconds = template.stat(643 + index as u32).unwrap_or(0) / 100;
            if seconds != 0 && (index == 0 || family != 0) {
                self.nano_locks.push((who, ao_net::msg::Identity { kind: 1, instance: family }, seconds));
            }
        }
    }

    fn advance_nano_locks(&mut self, dt: f32) {
        self.nano_lock_second += dt;
        if self.nano_lock_second < 1.0 { return; }
        // GC1005122c decrements once on GameTime's second pulse, after execution.
        self.nano_lock_second %= 1.0;
        self.nano_locks.retain_mut(|(_, _, seconds)| { *seconds -= 1; *seconds > 0 });
    }

    fn load_nano_template(&mut self, spell: i32) -> bool {
        if self.nano_templates.contains_key(&spell) { return true; }
        let Some(store) = &self.nano_store else { return false };
        let result = (|| -> anyhow::Result<_> {
            let record = store.get(super::hud_nanodb::NANO_RDB_TYPE, u32::try_from(spell)?)?.context("missing nano template")?;
            ao_formats::dynel_visual::parse_item_template(&record)
        })();
        match result {
            Ok(template) => { self.nano_templates.insert(spell, Arc::new(template)); true }
            Err(error) => { eprintln!("nano cast: {error:#}"); false }
        }
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
        let world = glam::Mat4::from_scale_rotation_translation(glam::Vec3::splat(c.scale), glam::Quat::from_rotation_y(scene_yaw(c.pose.yaw)), glam::Vec3::from(scene_pos(c.pose.pos)));
        if anchor==0 {return Some(world);}
        let Model::Ready { built, .. } = self.models.get(&c.key)? else { return None };
        let rig=built.rig.as_ref()?;
        let matrix = if c.current_skin.is_some() {
            if anchor == 3000 { rig.cached_weapon_effect_anchor(if slot == 8 { 2 } else { 1 }, &c.pose_cache) } else { rig.cached_effect_anchor(anchor, &c.pose_cache) }
        } else {
            let layers = c.layers(built);
            if anchor == 3000 { rig.weapon_effect_anchor_composed(if slot == 8 { 2 } else { 1 }, layers) } else { rig.effect_anchor_composed(anchor, layers) }
        };
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
        if who == self.own { self.clear_local_nano_initiation(); }
        self.cancel_buff_visuals(who);
        self.nano_sounds.retain(|sound| sound.5 != who);
        self.nano_casts.retain(|(caster, cast)| *caster != who && !(cast.target.kind == CHAR_KIND && cast.target.instance == who));
        self.nano_cast_results.retain(|&(caster, spell), _| self.nano_casts.iter().any(|(who, cast)| *who == caster && cast.spell == spell));
        self.nano_visuals.retain(|application| application.target.kind != CHAR_KIND || application.target.instance != who);
        if who == self.own { self.nano_animations.push(None); }
        if let Some(c) = self.chars.get_mut(&who).filter(|c| matches!(c.special, Special::Cast(_))) {
            c.special = Special::None;
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
        let named = spell != 0;
        let spell = if spell != 0 { Some(spell) } else {
            self.casting.iter().find(|cast| cast.who == who && cast.execution.is_some()).map(|cast| cast.spell)
                .or_else(|| self.nano_casts.iter().find(|(caster, _)| *caster == who).map(|(_, cast)| cast.spell))
        };
        let Some(spell) = spell else { return };
        if who == self.own && self.local_nano_initiation.is_some_and(|local| local.0 == spell) {
            self.clear_local_nano_initiation();
        }
        self.nano_casts.retain(|(caster, cast)| *caster != who || cast.spell != spell);
        self.nano_cast_results.remove(&(who, spell));
        self.casting.retain(|cast| {
            if cast.who != who || cast.spell != spell || (named && cast.execution.as_ref().is_some_and(|execution| execution.recharge.is_none())) { return true; }
            self.nano_sounds.retain(|sound| sound.5 != who);
            let charge = self.nano_templates.get(&cast.spell).and_then(|template| template.stat(0x178)).unwrap_or(203) as u32;
            self.pending_clips.retain(|clip| clip.3 != who || (clip.2 != charge && clip.2 != cast.release_anim));
            self.replay.retain(|clip| clip.0 != who || (clip.1 != charge && clip.1 != cast.release_anim));
            if who == self.own { self.nano_animations.push(None); }
            else if let Some(c) = self.chars.get_mut(&who).filter(|c| matches!(c.special, Special::Cast(_)) || c.special == Special::Once(cast.release_anim)) {
                c.special = Special::None;
            }
            if let Some(renderer) = &mut self.effects { renderer.delete(cast.handle); }
            false
        });
    }

    /// Movement part of travel spells; Zone owns canonical stat writes and CF76 snapshots.
    pub fn apply_travel_spells(&mut self, application: &ao_net::n3::spells::ApplySpells) {
        if application.target.kind != CHAR_KIND || application.target.instance == self.own { return; }
        let Some(c) = self.chars.get_mut(&application.target.instance) else { return };
        for spell in &application.spells {
            if spell.function == ao_net::n3::spells::function::FLIGHT {
                c.mover.apply_flight_spell(application.apply);
                if application.apply {
                    c.anim = 0xb2;
                    c.restart_body = true;
                }
            }
        }
        c.pose = remote_pose(&mut c.mover, self.collision.as_ref(), 0.0);
    }

    /// StatIIR and spell writes share GC 10059e6a's visual/movement stat hooks.
    pub fn apply_travel_stats(&mut self, target: ao_net::msg::Identity, stats: &[(i32, i32)]) {
        if target.kind != CHAR_KIND { return; }
        let Some(c) = self.chars.get_mut(&target.instance) else { return };
        for &(stat, value) in stats {
            let value = if stat == 360 { value.max(20) } else { value };
            c.mover.on_stat(stat, value);
            match stat {
                0 => c.flags = value,
                359 if c.mech_data == 0 => {
                    if let Look::Char(look) = &mut c.look {
                        look.monster_data = value;
                        c.next = Some(c.look.key(self.texture_quality)).filter(|&key| key != c.key);
                    }
                }
                360 => c.scale = value as f32 / 100.0,
                662 => c.mech_data = value,
                ao_net::n3::motion::STAT_FEATURES => c.features_set = true,
                _ if stat == super::zone::IN_PLAY_STAT as i32 => c.in_play = value != 0,
                _ => {}
            }
        }
        c.pose = remote_pose(&mut c.mover, self.collision.as_ref(), 0.0);
    }

    /// GC1004e177: request the template's initiation visual on the source.
    pub fn start_local_nano(&mut self, spell: i32, effect: i32) {
        if effect == 0 { return; }
        self.clear_local_nano_initiation();
        self.local_nano_initiation = Some((spell, if effect == 49999 { 0 } else { effect }, 0));
    }

    /// GC1004eef0, also called by CharCastNano when attack leaves stage1.
    fn clear_local_nano_initiation(&mut self) {
        if let Some((_, _, handle)) = self.local_nano_initiation.take() {
            if let Some(renderer) = &mut self.effects { renderer.delete(handle); }
        }
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
        character.restart_body = true;
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
        if let Some((spell, mut effect, mut handle)) = self.local_nano_initiation.take() {
            if handle == 0 && effect != 0 && self.nano_effect_categories & 4 != 0 {
                if let Some(r) = &mut renderer {
                    let attractor = r.attractor(effect, 0).unwrap_or(0);
                    if let (Some(source), Some(target)) = (anchor(self, self.own, attractor, &mut own_anchor).or_else(|| anchor(self, self.own, 0, &mut own_anchor)), anchor(self, self.own, 0, &mut own_anchor)) {
                        r.prepare_anchors((CHAR_KIND as u32, self.own as u32), |identity, id| anchor(self, identity.1 as i32, id, &mut own_anchor));
                        let look = appearance(self.own, &mut stat);
                        let config = EffectConfig { creation: Creation::Dynel, track_source: true,
                            source_identity: Some((CHAR_KIND as u32, self.own as u32)),
                            target_identity: Some((CHAR_KIND as u32, self.own as u32)),
                            source_appearance: look, target_appearance: look, ..Default::default() };
                        match r.spawn_configured(Binding { group: 0, attractor, effect, note: 0, color: 0 }, source, target.w_axis.truncate(), config) {
                            Ok(created) => handle = created,
                            Err(error) => eprintln!("nano initiation {spell}: {error:#}"),
                        }
                        effect = 0; // Native factory is attempted once, including null/error.
                    }
                }
            }
            if self.nano_effect_categories & 4 == 0 { effect = 0; }
            self.local_nano_initiation = Some((spell, effect, handle));
        }
        self.nano_handles.retain(|(_, _, handle)| renderer.as_ref().is_some_and(|r| r.is_active(*handle)));
        for (who, cast) in std::mem::take(&mut self.nano_casts) {
            let result = self.nano_cast_results.remove(&(who, cast.spell));
            if !self.load_nano_template(cast.spell) { continue; }
            let template = Arc::clone(&self.nano_templates[&cast.spell]);
            let target = if cast.target.kind == 0 && cast.target.instance == 0 && template.stat(0).unwrap_or(0) & 0x8000 == 0 { who } else { cast.target.instance };
            self.casting.retain(|old| { if old.who == who && old.spell == cast.spell { if let Some(r) = &mut renderer { r.delete(old.handle); } false } else { true } });
            let remaining = nano_cast_delay(template.stat(0x126).unwrap_or(200), template.stat(0x20b).unwrap_or(1_234_567_890), stat(who, 0x95).unwrap_or(0), stat(who, 0x33).unwrap_or(0), template.stat(0).unwrap_or(1));
            let instant = template.stat(0).unwrap_or(1) & 0x80000 != 0;
            if instant { self.start_nano_locks(who, cast.spell); }
            if instant && who == self.own {
                if let Some((_, _, handle)) = self.local_nano_initiation.take() {
                    if let Some(r) = &mut renderer { r.delete(handle); }
                }
            }
            let release_anim = template.stat(if target == who { 0x17a } else { 0x179 }).unwrap_or(if target == who { 202 } else { 201 }) as u32;
            let timer = !self.casting.iter().any(|cast| cast.who == who && cast.execution.is_some());
            self.casting.push(NanoCast { who, spell: cast.spell, target, handle: 0, remaining, total: remaining, release_anim, released: instant, release_seen: false, done: false, instant, execution: (!instant).then_some(NanoExecution { left: remaining, recharge: Some(template.stat(0xd2).unwrap_or(0).max(0) as f32 / 100.0), timer, result }), finish_enabled: !instant || cast.flag, start_effect: if instant || self.nano_effect_categories & 4 == 0 { 49999 } else { template.stat(0x1ac).unwrap_or(49999) }, finish: if !instant || cast.flag { [template.stat(0x19e).unwrap_or(49999), template.stat(0x169).unwrap_or(49999)] } else { [49999; 2] } });
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
            let attacking = cast.execution.as_ref().is_some_and(|execution| execution.recharge.is_some());
            if cast.execution.as_mut().is_some_and(|execution| !execution.advance(dt)) { cast.execution = None; }
            if attacking && cast.execution.as_ref().is_some_and(|execution| execution.recharge.is_none() && execution.result == Some(1)) {
                self.start_nano_locks(cast.who, cast.spell);
            }
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
                if cast.remaining > 0.0 || cast.execution.as_ref().is_some_and(|execution| execution.result.is_none()) {
                    self.nano_sound(cast.spell, 0x10d, (cast.who, own_pos), 0.2, 1.0);
                    continue;
                }
                if let Some(r) = &mut renderer { r.next_state(cast.handle); }
                cast.released = true;
                if cast.who == self.own {
                    if let Some((_, _, handle)) = self.local_nano_initiation.take() {
                        if let Some(r) = &mut renderer { r.delete(handle); }
                    }
                }
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
        casting.retain(|cast| cast.execution.is_some() || !cast.done || cast.finish.iter().any(|&effect| effect != 0 && effect != 49999) || renderer.as_ref().is_some_and(|r| r.is_active(cast.handle)));
        self.casting = casting;
        self.advance_nano_locks(dt);
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
            id::STEP | id::LAND | id::SWIM => self.footstep(who, note),
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

    /// Notes `step`/`stepfast`/`left`/`right` (0x26) and `land` (0x85, `FUN_10045069`'s ground-material block): a player plays
    /// `SM_Sandy_Game_Feet_Man_Walk` / `_Land`, a creature a value of its record's list `note`; both with the ground material
    /// under the character ([`ao_formats::playfield::collision::Collision::ground_material`]). A `land` note is silent while the
    /// character is still in liquid (`FUN_10045069`'s submersion guard: no splash thud for the jump-landing clip).
    fn footstep(&mut self, who: i32, note: u32) {
        use super::combat::notes::id;
        let (Some(at), Some(npc)) = (self.char_pos(who), self.chars.get(&who).map(|c| c.npc)) else { return };
        if note == id::LAND && self.collision.as_ref().is_some_and(|c| c.borrow().liquid_at(at).is_some()) {
            return;
        }
        let sound = if npc {
            self.record_of(who).and_then(|(sounds, _)| self.pick_of(&sounds, note))
        } else {
            super::combat::notes::footstep_name(note).map(ao_audio::sbf::sound_id)
        };
        let material = self.collision.as_ref().map_or(5, |c| c.borrow().ground_material(at));
        self.sounds.extend(sound.map(|id| GameSound { id, pos: at, material, size: 1 }));
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
        self.props.insert((kind, instance), Prop { id, key: look.key(self.texture_quality), parent: None, name: None, pos, yaw: rot.map_or(0.0, |q| quat_yaw(&q)), scale, submitted: false, anim: PropAnim::default(), stats });
        self.pending.push((look.key(self.texture_quality), look));
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
        if kind == ao_net::n3::dynel::KIND_WEAPON_ITEM && id == 26 {
            if let Some(item) = self.arms.weapon_item(instance) { return Some(item.clip); }
        }
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
    /// GC1001698d: an existing dynel gets authored Highlight11507, no replacement art.
    pub fn flash_head(&mut self, who: u32) -> anyhow::Result<()> {
        use super::combat::effects::{Binding, Creation, EffectConfig};
        if who != self.own as u32 && !self.chars.contains_key(&(who as i32)) { return Ok(()); }
        let Some(renderer) = &mut self.effects else { return Ok(()); };
        renderer.spawn_configured(
            Binding { group: 0, attractor: 0, effect: 0x2cf3, note: 0, color: 0 },
            glam::Mat4::IDENTITY, glam::Vec3::ZERO,
            EffectConfig { creation: Creation::Dynel, source_identity: Some((CHAR_KIND as u32, who)), ..Default::default() },
        ).map(|_| ())
    }

    pub(in crate::play) fn report_effect_failure(&mut self, message: std::fmt::Arguments<'_>) {
        if let Some(renderer) = &mut self.effects { renderer.report_failure(message); }
    }

    /// GUI world feedback creates the authored Font control on the real dynel.
    pub fn floating_text(&mut self, who:i32, text:&str, color:u32, mut own_anchor:impl FnMut(i32)->Option<glam::Mat4>) -> anyhow::Result<()> {
        use anyhow::Context;
        use super::combat::effects::{Binding,Creation,EffectConfig};
        // GC10058e36/10011108: feedback resolves the subject dynel before
        // creating Font12122; an already removed subject creates no effect.
        if who != self.own && !self.chars.contains_key(&who) { return Ok(()); }
        let Some(mut renderer)=self.effects.take() else {return Ok(())};
        let result=(|| {
            let attractor=renderer.attractor(12122,0).context("missing floating text effect 12122")?;
            let mut resolve=|_: (u32,u32),id| if who==self.own {own_anchor(id)}else{self.effect_anchor(who,id,0)};
            let identity=(CHAR_KIND as u32,who as u32);
            renderer.prepare_anchors(identity,&mut resolve);
            let source=resolve(identity,attractor).or_else(||resolve(identity,0));
            let source_nonvisual=source.is_none();
            let source=source.unwrap_or(glam::Mat4::IDENTITY);
            let [a,r,g,b]=color.to_be_bytes().map(|v|v as f32/255.0);
            let handle=renderer.spawn_configured(Binding {group:0,attractor,effect:12122,note:0,color:0},source,source.w_axis.truncate(),EffectConfig {creation:Creation::Dynel,track_source:true,source_nonvisual,source_identity:Some(identity),start_color:Some([r,g,b,a]),stop_color:Some([r,g,b,a]),..Default::default()})?;
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
            N3::World(World::CharacterAction(a)) if who.kind == CHAR_KIND && matches!(a.action, ao_net::n3::action::id::SIT_RELAY | ao_net::n3::action::id::STAND_UP) => {
                use ao_net::n3::action::{id, mv, wait};
                if let Some(c) = self.chars.get_mut(&who.instance) {
                    let transition = if a.action == id::SIT_RELAY {
                        mv::SWITCH_TO_SIT_GROUND
                    } else {
                        match c.mover.wait_state() {
                            wait::SLEEP => mv::LEAVE_SLEEP,
                            wait::LOUNGE => mv::LEAVE_LOUNGE,
                            _ => mv::LEAVE_SIT,
                        }
                    };
                    c.mover.on_move_type(transition);
                }
            }
            N3::Unknown(body) if who.kind == CHAR_KIND && m.header.msg_type == ao_net::n3::action::SOCIAL_ACTION_CMD => {
                use ao_net::n3::action::{self, mv};
                if let Ok(social) = action::parse_social_body(who, body) {
                    if social.state == 1 {
                        let transition = match social.anim {
                            action::SLEEP_EMOTE => Some(mv::SWITCH_TO_SLEEP),
                            action::LOUNGE_EMOTE => Some(mv::SWITCH_TO_LOUNGE),
                            _ => None,
                        };
                        if let (Some(c), Some(transition)) = (self.chars.get_mut(&who.instance), transition) {
                            c.mover.on_move_type(transition);
                        }
                    }
                }
            }
            N3::World(World::CharacterAction(a)) if who.kind == CHAR_KIND && a.action == ACTION_UNWIELD => self.unwield_slot(who.instance, a.identity_b.instance),
            N3::World(World::CharacterAction(a)) if who.kind == CHAR_KIND && a.action == 0x6b => {
                self.nano_result(who.instance, a.identity_b.instance, a.identity_b.kind);
            }
            N3::World(World::CharacterAction(a)) if who.kind == CHAR_KIND && matches!(a.action, 0x66 | 0x6c) => {
                self.cancel_nano_cast(who.instance, if a.action == 0x66 { 0 } else { a.identity_b.kind });
            }
            N3::World(World::CharacterAction(a)) if who.kind == CHAR_KIND && a.action == 0x75 && a.identity_a.kind == CHAR_KIND && self.chars.contains_key(&a.identity_a.instance) => {
                self.cancel_nano_cast(who.instance, a.identity_b.instance);
            }
            N3::World(World::CharacterAction(a)) if who.kind == CHAR_KIND && a.action == 0x89 => {
                if who.instance == self.own { self.clear_local_nano_initiation(); }
                // GC1004f274 clears the queue without holder+0x10: stage0 releases,
                // with no result to copy; an already released controller keeps its cached result.
                self.nano_casts.retain(|(caster, _)| *caster != who.instance);
                self.nano_cast_results.retain(|&(caster, _), _| caster != who.instance);
                // GC1004f274 -> 1004e68c removes every internal kind3 timer,
                // independently of the retained visual controller.
                for cast in self.casting.iter_mut().filter(|cast| cast.who == who.instance) { cast.execution = None; }
                for cast in self.casting.iter_mut().filter(|cast| cast.who == who.instance && !cast.released) {
                    cast.remaining = 0.0;
                    cast.finish_enabled = false;
                    cast.finish = [49999; 2];
                }
            }
            N3::Dynel(Dynel::CastNanoSpell(cast)) if who.kind == CHAR_KIND => {
                self.load_nano_template(cast.spell);
                self.nano_casts.push((who.instance, cast.clone()));
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
            // Same zone-in gap as `Armory::on_message` (docs/zone/static.md §4): seed the hand `AnimSet` resolution pipeline
            // (normally fed by a live `WeaponItemFullUpdateIIR_t`, line above) from the `FullCharacterIIR_t` inventory so the
            // own character's attack animation resolves after a fresh login too.
            N3::World(World::FullCharacter(c)) if who.kind == CHAR_KIND => {
                for e in &c.inventory {
                    if let Some(hand) = match e.slot { 6 => Some(0), 8 => Some(1), _ => None } {
                        self.weapons.insert(e.id.instance, (who.instance, hand));
                        self.pending_weapons.push((who.instance, hand, Some(e.item.low_id as u32), Vec::new()));
                    }
                }
            }
            // `AppearanceUpdateIIR_c::Activate` for any character (`FUN_10058e36` only tests the identity kind 50000): cloth, stat 0x2A1 `VisualFlags`,
            // attractors; the model is rebuilt through the worker (the look's key changes) and replaces the drawn one when it is ready
            N3::World(World::Appearance(a)) if who.kind == CHAR_KIND => {
                if let Some(c) = self.chars.get_mut(&who.instance) {
                    if let Look::Char(l) = &mut c.look {
                        l.apply_appearance(a);
                        let key = c.look.key(self.texture_quality);
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
                let key = look.key(self.texture_quality);
                let yaw = u.yaw().unwrap_or(0.0);
                let mut mover = Mover::new(u.pos, yaw);
                mover.set_npc(u.is_npc());
                mover.set_body_scale(u.monster_scale.max(20) as f32 / 100.0);
                mover.on_stat(0, u.flags2 as i32);
                if let ao_net::n3::dynel::CharClass::Pc(pc) = &u.class {
                    mover.on_stat(16, pc.stats[1] as i32);
                    mover.on_stat(17, pc.stats[2] as i32);
                }
                // players: [INFERENCE] bit 4 (their moves are applied live); NPCs get their record's `Features` when the model is built
                mover.set_features(if u.is_npc() { 2 } else { 4 });
                // PC type-30 relays are real sits; preserve the NPC spawn-placement exception.
                mover.set_sit_allowed(!u.is_npc());
                for &(stat, value) in &u.pairs {
                    mover.on_stat(stat, value);
                }
                mover.on_stat(ao_net::n3::motion::STAT_RUN_SPEED, u.run_speed as i32);
                mover.on_stat(ao_net::n3::motion::STAT_MAX_HEALTH, u.max_health);
                mover.on_stat(STAT_HEALTH, u.health);
                mover.restore_blob(&u.blob);
                if let Some(p) = &u.path {
                    mover.on_path(p.id.kind != 0 || p.id.instance != 0, &p.waypoints);
                }
                let pose = remote_pose(&mut mover, self.collision.as_ref(), 0.0);
                self.chars.insert(
                    who.instance,
                    Char {
                        name: u.name.clone(),
                        npc: u.is_npc(),
                        look,
                        key,
                        mover,
                        scale: u.monster_scale.max(20) as f32 / 100.0,
                        mech_data: u.pairs.iter().find(|p| p.0 == 662).map_or(0, |p| p.1),
                        side: u.side,
                        flags: u.flags2 as i32,
                        visual_flags: u.visual_flags as i32,
                        in_play: u.flags & ao_net::n3::dynel::flag::IN_PLAY != 0,
                        visible: u.is_npc() || who.instance == self.own || u.flags & ao_net::n3::dynel::flag::IN_PLAY != 0,
                        restart_body: false,
                        parent: u.parent.filter(|p| *p != ao_net::msg::Identity::default()),
                        posture_idle: None,
                        body_delay_ms: 0.0,
                        pose,
                        anim: 0x78,
                        special: if u.max_health > 0 && u.health <= 0 { Special::Die(DIE_KEY) } else { Special::None },
                        actions: vec![],
                        base_layers: 3,
                        clip_ms: 0.0,
                        clip_rate: 1.0,
                        body_once: false,
                        body_loop: false,
                        previous_body: None,
                        terminal_pose: false,
                        submitted: false,
                        parts: vec![],
                        features_set: !u.is_npc() || u.pairs.iter().any(|p| p.0 == ao_net::n3::motion::STAT_FEATURES),
                        bounds: None,
                        current_skin: None,
                        cadence: Default::default(),
                        pose_cache: Default::default(),
                        roll: Roll::default(),
                        next: None,
                        note_fired: 0,
                        body_phase: 0.0,
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
                self.apply_travel_stats(who, &s.stats);
                if s.stats.iter().any(|&(stat, value)| stat == 0x1b && value <= 0) && self.chars.get(&who.instance).is_some_and(|c| !matches!(c.special, Special::Die(_))) {
                    self.die(who.instance, DIE_KEY);
                }
            }
            N3::Misc(Misc::CharInPlay) => {
                if let Some(c) = self.chars.get_mut(&who.instance) {
                    c.in_play = true;
                    c.visible = true;
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
        if let Some(c) = self.chars.get_mut(&who.instance) {
            c.pose = remote_pose(&mut c.mover, self.collision.as_ref(), 0.0);
        }
    }

    /// Integrates the movement of every character by `dt` seconds (the drawn pose and animation state).
    pub fn advance(&mut self, dt: f32) {
        for c in self.chars.values_mut() {
            if !matches!(c.special, Special::Die(_)) {
                if let Some(Model::Ready { built, .. }) = self.models.get(&c.key) {
                    if let Some(rig) = &built.rig {
                        let radius = rig.cat().torso_sphere.radius;
                        c.mover.set_body_radius((if radius < 0.0 {
                            ao_formats::playfield::collision::DEFAULT_BODY_RADIUS
                        } else { radius }) * c.scale);
                    }
                }
                c.pose = remote_pose(&mut c.mover, self.collision.as_ref(), dt);
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

    pub fn set_collision(&mut self, collision: Option<std::rc::Rc<std::cell::RefCell<ao_formats::playfield::collision::Collision>>>) {
        self.collision = collision.clone();
        if let Some(renderer) = &mut self.effects { renderer.set_collision(collision); }
        self.advance(0.0);
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
        &mut self, dt: f32, cam: [f32; 3], fwd: [f32; 3], host: &mut Host,
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
        let worker = self.worker.take().unwrap_or_else(|| Worker::start(dir, self.names.clone()));
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
                    self.model_bytes += built_bytes(&built);
                    if let Some(Model::Ready { built: old, .. }) = self.models.insert(key, Model::Ready { built, uploaded: false }) {
                        self.model_bytes = self.model_bytes.saturating_sub(built_bytes(&old));
                    }
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
                self.pending.push((c.next.unwrap_or(c.key), c.look.clone()));
            }
        }
        for (key, look) in std::mem::take(&mut self.pending) {
            if self.asked.insert(key) {
                self.models.insert(key, Model::Loading);
                self.model_order.push_back(key);
                let _ = worker.tx.send(Req::Model { key, look, texture_quality: (key & 3) as i32 });
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
            let actor = ActorFrame { id: p.id, model: p.key, transform, parts, part_attractors: built.rig.as_ref().map_or_else(Vec::new, |rig| rig.part_attractors()), skin: skin.map(Arc::new), always: false, alpha: 1.0, ..Default::default() };
            if let Some(effects) = &mut self.effects {
                let identity = (kind as u32, instance as u32);
                if effects.needs_source_mesh(identity) { effects.prepare_source_mesh(identity, &built.model, &actor); }
            }
            host.actors.push(actor);
        }
        self.advance(dt);
        // Perf audit item 5: avoid CPU-skinning (`pose_composed_cached`) actors the renderer would cull anyway.
        // 5 planes (no far plane: the 80 m `char_view_distance` check below already caps distance), built from a
        // `self.fov_horizontal` cone around `fwd` with aspect 1 — identical to the renderer's own horizontal
        // half-angle at retail's fixed 90° (aspect-independent), or widened along with the aomac Enhanced
        // "Widescreen FOV" option (audit item 16), and strictly wider vertically than any real landscape window
        // (aspect >= 1 shrinks the renderer's vertical fov below this), so this test can only be more lenient
        // than `ao_render`'s final per-mesh sphere cull, never stricter.
        let skin_planes = skin_frustum_planes(cam, fwd, self.fov_horizontal);
        for (id, c) in &mut self.chars {
            if *id == own {
                continue;
            }
            let id_ref = id;
            if let Some(k) = c.next {
                match self.models.get(&k) {
                    Some(Model::Ready { .. }) => {
                        (c.key, c.next) = (k, None);
                        c.current_skin = None;
                        c.cadence = Default::default();
                        c.pose_cache = Default::default();
                        c.bounds = None;
                        c.previous_body = None;
                        c.parts.clear();
                    }
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
            let state = if c.mover.status().mode == Mode::Fly && !c.mover.status().jumping { AnimState::Hover } else { c.pose.anim };
            // Finish before selecting the next body, never rewind the outgoing clip before skinning.
            if matches!(c.special, Special::Once(_)) && c.body_once && !c.restart_body
                && built.clips.get(&c.anim).and_then(|clips| clips.get(c.roll.variant)).is_some_and(|clip| c.clip_ms >= clip.duration)
            {
                c.special = Special::None;
            }
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
                // GC 1006be27 leaves moving mode 7 unchanged; 1006c065/1006dfaf
                // request holder+8 on idle/entry, using the existing native key-parent resolver.
                Special::None if c.mover.status().mode == Mode::Fly && !c.mover.status().jumping => {
                    let entering = !matches!(c.roll.key, Some((_, Some(AnimState::Hover))));
                    let hover = (entering || c.mover.status().forward == 0) && built.clips.contains_key(&0xb2);
                    let id = if hover { 0xb2 } else { c.anim };
                    movement_rate = false;
                    (id, built.clips.get(&id), 1.0)
                }
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
            let started = key != c.anim || c.roll.key != Some(roll_key) || c.restart_body;
            let incoming_delayed = c.body_delay_ms > 0.0 && c.previous_body.as_ref().is_some_and(|body| body.elapsed == 0.0);
            let once = !matches!(c.special, Special::None | Special::Cast(_));
            if started && c.posture_idle.is_none() {
                c.body_delay_ms = 0.0;
                for action in &mut c.actions { if action.priority == 0 { action.before_body = true; } }
            }
            if started && c.roll.key.is_some() && !incoming_delayed {
                c.previous_body = built.clips.get(&c.anim).and_then(|clips| clips.get(c.roll.variant)).map(|old| PreviousBody {
                    clip: Arc::clone(old), ms: c.clip_ms, rate: c.clip_rate, once: c.body_once, elapsed: 0.0,
                    fade_ms: super::avatar::body_fade_ms(old, c.clip_ms, c.body_once, c.body_loop, once),
                });
                if let Some((duration, ms)) = c.actions.iter().rev().filter(|action| action.priority == 0 && action.before_body).find_map(|action| {
                    let clip = built.clips.get(&action.id)?.get(action.variant?)?;
                    Some((action.duration(clip), action.ms))
                }) {
                    if let Some(previous) = &mut c.previous_body {
                        previous.fade_ms = ao_formats::character::animation_fade_ms(duration, ms);
                    }
                }
            }
            let clip = list.filter(|l| !l.is_empty()).map(|l| &l[c.roll.pick(roll_key, l.len(), &mut self.rng)]);
            let (clip_start, skipped_notes) = clip.filter(|_| c.special == Special::None).map_or((0.0, 0), |a| movement_interval(a, state));
            if started {
                c.restart_body = false;
                c.anim = key;
                c.body_once = once;
                c.body_loop = c.special == Special::None && movement_loop(state);
                c.base_layers = 3;
                c.refresh_layers();
                c.clip_ms = clip_start;
                c.note_fired = skipped_notes;
                c.body_phase = clip_start;
                c.terminal_pose = false;
            }
            if let Some(animation) = c.posture_idle.take() {
                if let (Some(idle), Some(action)) = (clip, c.actions.iter_mut().rev().find(|action| action.posture && action.id == animation)) {
                    if let Some(enter) = built.clips.get(&animation).and_then(|clips| clips.first()) {
                        c.body_delay_ms = (action.duration(enter) - super::avatar::POSTURE_IDLE_LEAD_MS).max(0.0);
                        let (start, end) = super::avatar::loop_span(idle).unwrap_or((0.0, idle.duration));
                        action.fade_ms = ao_formats::character::animation_fade_ms(end - start, 0.0);
                    }
                }
                movement_rate = false; // queued idle Play explicitly uses speed 1.0.
            }
            let body_dt = if c.body_delay_ms > 0.0 {
                let elapsed = dt.abs() * 1000.0;
                let remaining = (elapsed - c.body_delay_ms).max(0.0);
                c.body_delay_ms = (c.body_delay_ms - elapsed).max(0.0);
                remaining / 1000.0
            } else { dt };
            // GC 0x1006be27 / 0x1006fb56 set the rate once after Play, including idle.
            advance_clip_clock(&mut c.clip_ms, &mut c.clip_rate, body_dt, started, || {
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
            if let Some(previous) = &mut c.previous_body {
                previous.ms += (dt * 1000.0 * previous.rate).abs();
                previous.elapsed += (body_dt * 1000.0 * c.clip_rate).abs();
                if previous.elapsed >= previous.fade_ms { c.previous_body = None; }
            }
            // the base clip's own notes (`FUN_1003c036`'s per-frame holder walk, not just the overlay `c.actions`): a wrap
            // of the looping clip's local cycle position re-arms `note_fired` for the step/land notes of its next cycle.
            if let Some(a) = clip {
                let once = !matches!(c.special, Special::None | Special::Cast(_));
                let local = super::avatar::clip_time(a, c.clip_ms, once);
                if local < c.body_phase {
                    c.note_fired = skipped_notes;
                }
                let note_id = *id;
                let airborne = matches!(state, AnimState::JumpStand | AnimState::JumpForward);
                use super::combat::notes::id as note;
                self.notes.extend(super::combat::notes::fire(&a.events, local, &mut c.note_fired)
                    .filter_map(|n| (n != note::ATTACK).then_some(super::combat::notes::FiredNote { id: n, slot: -1 }))
                    .filter(|n| !(airborne && matches!(n.id, note::STEP | note::LAND | note::SWIM)))
                    .map(|note| (note_id, note)));
                c.body_phase = local;
            }
            c.actions.retain_mut(|action| {
                let Some(clips) = built.clips.get(&action.id).filter(|clips| !clips.is_empty()) else { return true };
                let variant = *action.variant.get_or_insert_with(|| if clips.len() > 1 { self.rng.rand() as usize % clips.len() } else { 0 });
                let clip = &clips[variant % clips.len()];
                action.advance(clip, dt, |note| self.notes.push((*id, note)))
            });
            let dead = matches!(c.special, Special::Die(_));
            if let Some(a) = clip.filter(|_| c.body_once) {
                c.clip_ms = c.clip_ms.min(a.duration);
            }
            if let Some(a) = clip.filter(|a| matches!(c.special, Special::None | Special::Cast(_)) && a.duration > 0.0 && c.clip_ms > 4.0 * a.duration) {
                c.clip_ms = super::avatar::clip_time(a, c.clip_ms, false);
            }
            let sample_pose = c.cadence.sample(host.smooth_animations, dist * dist, rig.bounding_radius, host.previous_visible_actors.contains(&(*id as u32)));
            if dist > self.char_view_distance || !c.visible {
                c.submitted = false;
                continue;
            }
            let terminal = dead && clip.is_some_and(|a| c.clip_ms >= a.duration);
            let needs_effect_pose = self.effects.as_ref().is_some_and(|effects| effects.needs_source_pose((CHAR_KIND as u32, *id as u32)));
            let skip_culled = skip_cpu_skin(&skin_planes, p, rig.bounding_radius, c.scale, needs_effect_pose, c.current_skin.is_some());
            let skin = if !skip_culled && (c.current_skin.is_none() || (sample_pose && (!terminal || !c.terminal_pose || !c.submitted || needs_effect_pose))) {
                c.terminal_pose = terminal;
                let mut cache = std::mem::take(&mut c.pose_cache);
                let pose = rig.pose_composed_cached(c.layers(built), &mut cache);
                c.pose_cache = cache;
                Some(pose)
            } else {
                None
            };
            c.submitted = true;
            let skin = skin.map(|(v, parts)| {
                c.parts = parts;
                c.bounds = super::hud_pick::bounds_of(v.iter().map(|x| &x.pos));
                let vertices = Arc::new(v);
                c.current_skin = Some(Arc::clone(&vertices));
                vertices
            });
            let (s, cs) = scene_yaw(c.pose.yaw).sin_cos();
            let k = c.scale;
            let transform = [[cs * k, 0.0, -s * k, 0.0], [0.0, k, 0.0, 0.0], [s * k, 0.0, cs * k, 0.0], [p[0], p[1], p[2], 1.0]];
            let actor = ActorFrame { id: *id as u32, model: c.key, transform, parts: c.parts.clone(), part_attractors: rig.part_attractors(), skin, always: false, alpha: 1.0, ..Default::default() };
            // Worn/attached item lights (the Light Bar's embedded `RLight_t` and similar, `mesh.rs::decode_mesh_lights`):
            // actor-relative from `ActorRig::mount_lights`, placed in the world by this dynel's own transform, the same
            // way `ActorFrame::transform` places its mesh vertices (`vertex_lights` in the shader then lights terrain,
            // statics and other actors from them, not just this dynel's own body).
            for l in rig.mount_lights(&c.parts) {
                let point = |p: [f32; 3]| -> [f32; 3] { std::array::from_fn(|j| p[0] * transform[0][j] + p[1] * transform[1][j] + p[2] * transform[2][j] + transform[3][j]) };
                let dir = |d: [f32; 3]| -> [f32; 3] { std::array::from_fn(|j| d[0] * transform[0][j] + d[1] * transform[1][j] + d[2] * transform[2][j]) };
                host.dynamic_lights.push(ao_scene::Light { pos: point(l.pos), spot: l.spot.map(|s| ao_scene::Spot { dir: dir(s.dir), ..s }), ..l });
            }
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

    pub fn set_shadows(&mut self, enabled: bool) {
        if let Some(effects) = &mut self.effects {
            effects.set_shadows(enabled);
        }
    }

    pub fn set_name_prefs(&mut self, values: &super::dvalue::DValues) {
        use super::dvalue::Kind;
        self.show_all_names = values.prefs.get_int("ShowAllNames", Kind::Login).unwrap_or(1) != 0;
        self.show_org_names = values.prefs.get_int("IsOrgNameShownOverHead", Kind::Char).unwrap_or(1) != 0;
    }

    /// The tag of `id` for `kind` (`FUN_10024e14` text and colours, `FUN_10024d5b` position): the text of [`name_tag`], centred on the head
    /// anchor (`GetIndicatorPosition`: attractor 0 + 0.5 m, scaled with the dynel); `None` while the dynel has no model, or its anchor
    /// fails the `x > 0` test of `FUN_10024d5b`. `bar` = health bar inputs `((health, max), colour)`.
    fn tag_of(&self, id: i32, kind: IndicatorKind, bar: Option<((i32, i32), u32)>, battlestation: Option<(i32, i32)>) -> Option<Tag> {
        let c = self.chars.get(&id)?;
        let Some(Model::Ready { built, .. }) = self.models.get(&c.key) else { return None };
        if !c.in_play || !c.visible || !built.visible || c.parent.is_some() || !tag_anchor_visible(c.pose.pos[0]) {
            return None;
        }
        let t = name_tag(&NameTagInput { name: &c.name, is_npc: c.npc, flags: c.flags, features: INVALID_STAT, visual_flags: c.visual_flags, side: c.side as i32, battlestation, ..Default::default() });
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
    fn collect_tags(&mut self, dt: f32, own_pos: [f32; 3], indicators: &[Indicator], battlestation: impl Fn(i32) -> Option<(i32, i32)>) -> Vec<Tag> {
        let mut tags: Vec<Tag> = indicators
            .iter()
            .filter(|i| i.kind != IndicatorKind::Selection || self.chars.get(&i.id).is_some_and(|c| selection_indicator_exists(c.flags)))
            .filter_map(|i| self.tag_of(i.id, i.kind, i.bar, battlestation(i.id)))
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
        tags.extend(ids.into_iter().filter_map(|(_, id)| self.tag_of(id, IndicatorKind::Nametag, None, battlestation(id))));
        tags
    }

    /// Adds the world-space tags of this frame to `host` (docs/zone/motion.md §6, `play/tags.rs`): `own_pos` = the player's scene
    /// position, `indicators` from [`super::tags::indicators`].
    #[allow(clippy::too_many_arguments)]
    pub fn name_tags(&mut self, dt: f32, gui: &mut Gui, host: &mut Host, own_pos: [f32; 3], indicators: &[Indicator], battlestation: impl Fn(i32) -> Option<(i32, i32)>, org_name: impl Fn(i32) -> Option<String>) {
        let mut tags = self.collect_tags(dt, own_pos, indicators, battlestation);
        if self.show_org_names {
            for tag in &mut tags {
                if let Some(name) = org_name(tag.id) {
                    let side = self.chars.get(&tag.id).map_or(0, |c| i32::from(c.side));
                    tag.org = Some((name, ao_net::n3::nametag::org_color(side)));
                }
            }
        }
        self.tags.frame(gui, host, &tags);
    }

    /// The camera's selection line hits visible posed dynels; the own body is supplied by Player only in third person
    /// (`FUN_10020a3c`, N3: client character permitted when `camera+0x1ec` is set).
    pub fn pick_bodies(&self, own: i32) -> Vec<super::hud_pick::PickBody> {
        self.chars
            .iter()
            .filter(|(id, c)| **id != own && c.visible)
            .filter_map(|(id, c)| Some(super::hud_pick::PickBody { id: *id, bounds: c.bounds?, pos: scene_pos(c.pose.pos), yaw: scene_yaw(c.pose.yaw), scale: c.scale }))
            .chain(self.own_pick.filter(|body| body.id == own))
            .collect()
    }

    /// False BoundingBoxTargeting uses the same composed body pose as the actor renderer.
    pub fn pick_character_hit(&self, body: &super::hud_pick::PickBody, ray: &super::hud_target::Ray, bounding_box: bool) -> Option<f32> {
        let distance = body.hit(ray.origin, ray.dir, ray.len)?;
        if bounding_box { return Some(distance); }
        let character = self.chars.get(&body.id)?;
        let Some(Model::Ready { built, .. }) = self.models.get(&character.key) else { return None };
        let rig = built.rig.as_ref()?;
        let vertices = character.current_skin.as_ref()?;
        body.hit_triangles(ray.origin, ray.dir, ray.len, vertices, rig.body_triangles())
    }

    pub fn set_own_pick(&mut self, body: Option<super::hud_pick::PickBody>) {
        self.own_pick = body;
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
fn model_state(world: &Dynels, key: u64) -> &'static str {
    match world.models.get(&key) {
        Some(Model::Ready { .. }) => "ready",
        Some(Model::Loading) => "loading",
        Some(Model::Failed) => "failed",
        None => "not queued",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effect_roots_exist_before_async_meshes_and_disappear_with_dynels() {
        let spawn = frames(include_str!("../../../../docs/captures/zone_ithaca.rec")).iter().find_map(|frame| {
            let m = ao_net::n3::decode(frame).ok()?;
            matches!(&m.body, N3::Dynel(Dynel::SimpleCharFullUpdate(_))).then_some(m)
        }).expect("captured visual character");
        let mut world = Dynels::default();
        world.on_message(&spawn);
        let who = spawn.header.target.instance;
        let c = &world.chars[&who];
        let key = c.key;
        let expected = glam::Mat4::from_scale_rotation_translation(glam::Vec3::splat(c.scale), glam::Quat::from_rotation_y(scene_yaw(c.pose.yaw)), glam::Vec3::from(scene_pos(c.pose.pos)));
        for model in [None, Some(Model::Loading), Some(Model::Failed)] {
            world.models.remove(&key);
            if let Some(model) = model { world.models.insert(key, model); }
            assert_eq!(world.effect_anchor(who, 0, 0), Some(expected));
            assert_eq!(world.effect_anchor(who, 2000, 0), None, "no fabricated named connector");
        }
        world.chars.remove(&who);
        assert_eq!(world.effect_anchor(who, 0, 0), None, "deleted dynels have no root");
        let item = ao_net::msg::Identity { kind: 0xc76a, instance: 9 };
        world.test_prop(item, vec![]);
        assert_eq!(world.item_effect_anchor(item, 0), None, "unresolved items are not necessarily visual dynels");
        assert_eq!(world.item_effect_anchor(item, 2000), None);
        world.props.remove(&(item.kind, item.instance));
        assert_eq!(world.item_effect_anchor(item, 0), None);
    }

    #[test]
    fn floating_text_builds_its_visual_on_the_loading_subject_dynel() -> anyhow::Result<()> {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/rdb.db").exists() { return Ok(()); }
        let spawn = frames(include_str!("../../../../docs/captures/zone_ithaca.rec")).iter().find_map(|frame| {
            let m = ao_net::n3::decode(frame).ok()?;
            matches!(&m.body, N3::Dynel(Dynel::SimpleCharFullUpdate(_))).then_some(m)
        }).expect("captured visual character");
        let mut world = Dynels { effects: Some(super::super::combat::effects::Renderer::open(&dir)?), ..Default::default() };
        world.on_message(&spawn);
        let who = spawn.header.target.instance;
        let key = world.chars[&who].key;
        world.models.insert(key, Model::Loading);
        let root = world.effect_anchor(who, 0, 0).unwrap();
        world.floating_text(who, "17", 0xffffff00, |_| panic!("remote feedback must not attach to own player"))?;
        assert_eq!(world.effects.as_ref().unwrap().resource_counts().3, 1, "SetText creates the private glyph visual");
        let mut host = Host::headless();
        world.effects.as_mut().unwrap().frame(0.1, &mut host, None);
        let text = host.actors.first().expect("floating text render actor");
        let skin = text.skin.as_ref().expect("world-space glyph vertices");
        assert!(!skin.is_empty());
        assert!(skin.iter().all(|v| (glam::Vec3::from_array(v.pos) - root.w_axis.truncate()).length() < 3.0));
        world.chars.remove(&who);
        let count = world.effects.as_ref().unwrap().resource_counts().0;
        world.floating_text(who, "18", 0xffffff00, |_| panic!("removed subject is rejected first"))?;
        assert_eq!(world.effects.as_ref().unwrap().resource_counts().0, count);
        Ok(())
    }

    #[test]
    fn name_preferences_follow_independent_stores() {
        use super::super::dvalue::{DValues, Kind};
        let mut world = Dynels::default();
        let mut values = DValues::new(std::path::Path::new(""));
        world.set_name_prefs(&values);
        assert!(world.show_all_names && world.show_org_names);
        values.prefs.set_int("ShowAllNames", 0, Kind::Login);
        values.prefs.set_int("IsOrgNameShownOverHead", 0, Kind::Char);
        world.set_name_prefs(&values);
        assert!(!world.show_all_names && !world.show_org_names);
        let mut restored = DValues::new(std::path::Path::new(""));
        restored.prefs.load(&values.prefs.save(Kind::Login), Kind::Login);
        restored.prefs.load(&values.prefs.save(Kind::Char), Kind::Char);
        let mut startup = Dynels::default();
        startup.set_name_prefs(&restored);
        assert!(!startup.show_all_names && !startup.show_org_names, "both native archives restore before the first name-tag frame");
        values.prefs.set_int("ShowAllNames", 1, Kind::Login);
        values.prefs.set_int("IsOrgNameShownOverHead", 1, Kind::Char);
        world.set_name_prefs(&values);
        assert!(world.show_all_names && world.show_org_names);
    }

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
        world.casting.push(NanoCast { who: 42, spell: 163449, target: 43, handle: 0, remaining: 2.0, total: 2.0, finish: [49999; 2], release_anim: 201, released: false, release_seen: false, done: false, instant: false, execution: Some(NanoExecution { left: 2.0, recharge: Some(1.0), timer: true, result: None }), finish_enabled: true, start_effect: 49999 });
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
        advance_clip_clock(&mut ms, &mut rate, 0.2, true, || super::super::avatar::anim_rate(0.9, 50.0, 1.5, 1.5, false));
        assert!((ms - 360.0).abs() < 0.001);
        advance_clip_clock(&mut ms, &mut rate, 0.2, false, || panic!("not reset until Play"));
        assert!((ms - 720.0).abs() < 0.001);
        ms = 0.0;
        advance_clip_clock(&mut ms, &mut rate, -0.1, true, || 0.5);
        assert_eq!(ms, 50.0);
        assert_eq!(rate, 0.5);
        ms = 1166.0;
        advance_clip_clock(&mut ms, &mut rate, 0.2, true, || 1.25);
        assert_eq!((ms, rate), (1416.0, 1.25), "nonzero authored loopstart must still initialize playback rate");
        advance_clip_clock(&mut ms, &mut rate, 0.0, false, || panic!("zero delta is not another Play"));
        assert_eq!((ms, rate), (1416.0, 1.25));
    }

    #[test]
    fn remote_movement_starts_at_named_loop_and_skips_intro_notes() {
        let clip = CatAnim { source_id: 10194, root: String::new(), events: vec![(733, "right".into()), (1166, "loopstart".into()), (1300, "left".into()), (1933, "loopend".into())], version: 0x106, duration: 4000.0, signature: 0, param: 0.0, tracks: vec![] };
        for state in [AnimState::Idle, AnimState::Walk, AnimState::Run, AnimState::RunBack, AnimState::TurnLeft] {
            let (start, mut fired) = movement_interval(&clip, state);
            assert_eq!(start, 1166.0);
            assert_eq!(fired, 1, "intro right-foot marker is already consumed");
            assert!(super::super::combat::notes::fire(&clip.events, start, &mut fired).next().is_none());
            assert_eq!(super::super::combat::notes::fire(&clip.events, 1300.0, &mut fired).count(), 1);
            let (_, next_cycle) = movement_interval(&clip, state);
            fired = next_cycle;
            assert!(super::super::combat::notes::fire(&clip.events, start, &mut fired).next().is_none(), "wrapping does not replay the intro footstep");
        }
        assert_eq!(movement_interval(&clip, AnimState::JumpForward), (0.0, 0), "one-shot jump playback is not a movement loop interval");
        let unmarked = CatAnim { events: vec![], ..clip };
        assert_eq!(movement_interval(&unmarked, AnimState::Run), (0.0, 0));
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

    /// A synthetic built model whose [`built_bytes`] is exactly `bytes` (one texture, no mesh), for deterministic
    /// byte-budget eviction tests without needing the real worker thread.
    fn sized_built(bytes: usize) -> Built {
        let texture = ao_scene::Texture { width: 1, height: 1, rgba: vec![0u8; bytes], source: Default::default() };
        let scene = ao_scene::Scene { textures: [(ao_scene::TextureKey { rdb_type: 0, id: 0 }, texture)].into(), ..Default::default() };
        plain(scene, true)
    }

    /// Reproduces the unbounded `models` growth measured live (`zone-preload-proposal.md` §3: RSS never dropped
    /// between crossings, climbing to >1 GB after a few zones) and checks the [`Dynels::MODEL_BYTE_BUDGET`] fix:
    /// the tracked `model_bytes` stays within budget across repeated A -> B -> A -> B zone changes, the current
    /// zone's own key is never the one evicted, an evicted key is forgotten from `asked` too so a later
    /// re-encounter rebuilds it instead of silently staying without a model forever, and `model_bytes` always
    /// equals the real sum over `models` (no drift between the running counter and the map it describes).
    #[test]
    fn model_cache_stays_within_its_byte_budget_across_repeated_zone_changes() {
        let mut world = Dynels::default();
        let entry_bytes = 1_000_000; // 1 MB each, synthetic
        let count = Dynels::MODEL_BYTE_BUDGET as u64 / entry_bytes as u64 + 50; // comfortably over budget
        for key in 1..=count {
            let built = sized_built(entry_bytes);
            world.model_bytes += built_bytes(&built);
            world.models.insert(key, Model::Ready { built: Box::new(built), uploaded: false });
            world.model_order.push_back(key);
            world.asked.insert(key);
        }
        assert!(world.model_bytes > Dynels::MODEL_BYTE_BUDGET, "test setup must start over budget");
        let kept = count;
        world.props.insert((1, 1), Prop { id: 0x4000_0001, key: kept, parent: None, name: None, pos: [0.0; 3], yaw: 0.0, scale: 1.0, submitted: false, anim: PropAnim::default(), stats: vec![] });
        world.on_playfield(100); // A -> B: clear() runs, which now evicts down to the budget
        assert!(world.model_bytes <= Dynels::MODEL_BYTE_BUDGET, "within budget after one zone change: {} bytes", world.model_bytes);
        assert!(world.models.contains_key(&kept), "the zone's own just-used key survives eviction");
        assert!(!world.asked.contains(&1), "an evicted key is forgotten from `asked`, so it can be refetched");
        let real_total: usize = world.models.values().filter_map(|m| match m { Model::Ready { built, .. } => Some(built_bytes(built)), _ => None }).sum();
        assert_eq!(world.model_bytes, real_total, "model_bytes tracks the real sum exactly");
        for pf in [200, 100, 200] { // B -> A -> B: round trips must never grow past the budget
            for key in (count + 1)..=(count + 20) {
                if world.asked.insert(key) {
                    let built = sized_built(entry_bytes);
                    world.model_bytes += built_bytes(&built);
                    world.models.insert(key, Model::Ready { built: Box::new(built), uploaded: false });
                    world.model_order.push_back(key);
                }
            }
            world.on_playfield(pf);
            assert!(world.model_bytes <= Dynels::MODEL_BYTE_BUDGET, "pf {pf}: {} bytes", world.model_bytes);
        }
    }

    /// Confirms the production `model_bytes` accounting is correct against real built models, not synthetic ones:
    /// settles a real capture's characters and placed items through the actual worker path (`Resp::Model`, the
    /// only production insertion site) and checks `model_bytes` matches the sum of [`built_bytes`] over every
    /// `Ready` entry, logging the measured average model size that [`Dynels::MODEL_BYTE_BUDGET`] is sized against.
    #[test]
    fn model_bytes_tracks_real_built_models() {
        let Some(dir) = client() else { return };
        let mut z = Zone::new(25988);
        for f in frames(include_str!("../../../../docs/captures/zone_ithaca.rec")) {
            z.on_frame(&f);
        }
        let keys = |z: &Zone| z.world.chars.iter().filter(|(id, _)| **id != z.world.own)
            .map(|(_, c)| c.next.unwrap_or(c.key)).chain(z.world.props.values().map(|p| p.key)).collect::<Vec<_>>();
        let playfield = z.world.playfield.expect("capture announces its playfield");
        let placed: Vec<_> = placed_dynels(&RecordStore::open(&dir).unwrap(), playfield).unwrap().into_iter()
            .filter(|d| d.template != 0).map(|d| (d.kind as i32, d.instance as i32)).collect();
        z.world.start(dir.clone(), 25988);
        let own = z.own().unwrap().clone();
        let eye = scene_pos(own.pos);
        let fwd = scene_forward(own.yaw.unwrap_or(0.0));
        let mut host = Host::headless();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
        loop {
            z.world.update_with_collision(0.05, eye, fwd, &mut host, None, |_| None);
            host.actor_models.clear();
            host.actors.clear();
            let pending = keys(&z).iter().any(|k| !matches!(z.world.models.get(k), Some(Model::Ready { .. })))
                || z.world.want_placed.is_some() || placed.iter().any(|id| !z.world.props.contains_key(id));
            if !pending {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "captured zone not ready after 300s");
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let real_total: usize = z.world.models.values().filter_map(|m| match m { Model::Ready { built, .. } => Some(built_bytes(built)), _ => None }).sum();
        assert_eq!(z.world.model_bytes, real_total, "model_bytes must track every Resp::Model insertion exactly");
        let count = z.world.models.values().filter(|m| matches!(m, Model::Ready { .. })).count();
        eprintln!("model cache: {count} models built via the real worker path, {real_total} bytes total, avg {:.0} bytes/model",
            real_total as f64 / count.max(1) as f64);
    }

    #[test]
    fn skip_cpu_skin_culls_behind_camera_but_not_in_front() {
        // Camera at the origin looking down -Z (`Camera::forward`'s convention elsewhere in this file's tests).
        let planes = skin_frustum_planes([0.0, 0.0, 0.0], [0.0, 0.0, -1.0], super::super::camera::FOV_HORIZONTAL);
        let behind = [0.0, 0.0, 10.0];
        let in_front = [0.0, 0.0, -10.0];
        assert!(skip_cpu_skin(&planes, behind, 1.0, 1.0, false, true), "an actor behind the camera is CPU-skin-skipped");
        assert!(!skip_cpu_skin(&planes, in_front, 1.0, 1.0, false, true), "an actor in front of the camera still skins");
    }

    #[test]
    fn skip_cpu_skin_never_skips_without_a_prior_skin_or_with_needs_effect_pose() {
        let planes = skin_frustum_planes([0.0, 0.0, 0.0], [0.0, 0.0, -1.0], super::super::camera::FOV_HORIZONTAL);
        let behind = [0.0, 0.0, 10.0];
        assert!(!skip_cpu_skin(&planes, behind, 1.0, 1.0, false, false), "no skin pose yet: the first frame always skins");
        assert!(!skip_cpu_skin(&planes, behind, 1.0, 1.0, true, true), "needs_effect_pose forces a fresh skin even off-screen");
    }

    #[test]
    fn widescreen_fov_widens_the_skin_cull_cone_so_edge_characters_still_animate() {
        // Audit item 16 prerequisite: a character 55 degrees off-centre is outside retail's fixed-90-degree cull cone
        // (half-angle 45 degrees) but inside the real frustum once the Enhanced widescreen option widens the lens
        // past 90 degrees horizontal (e.g. 120.6 degrees at a 21:9 aspect) -- it must not stop animating.
        // Far enough out that `skip_cpu_skin`'s fixed `POSE_MARGIN` cull-radius inflation (`ao_render::POSE_MARGIN`,
        // additive, independent of distance) is negligible next to how far past/short of the frustum edge this
        // point sits.
        let angle = 55f32.to_radians();
        let pos = [angle.sin() * 1000.0, 0.0, -angle.cos() * 1000.0];
        let retail = skin_frustum_planes([0.0, 0.0, 0.0], [0.0, 0.0, -1.0], super::super::camera::FOV_HORIZONTAL);
        assert!(skip_cpu_skin(&retail, pos, 0.1, 1.0, false, true), "55 degrees off-centre must be outside retail's fixed 90 degree cull cone");
        let wide = skin_frustum_planes([0.0, 0.0, 0.0], [0.0, 0.0, -1.0], super::super::camera::horizontal_fov(true, 21.0 / 9.0));
        assert!(!skip_cpu_skin(&wide, pos, 0.1, 1.0, false, true), "widescreen FOV must widen the cull cone so edge characters keep animating");
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

    #[test]
    fn trade_sphere_uses_meshmeta_and_body_scale_not_pose_bounds() {
        let Ok(store) = RecordStore::open(&ao_gui::client_dir()) else { return };
        let names = Arc::new(NameTable::load(&store).unwrap());
        let mut spawn = captured_posture_player();
        let who = spawn.header.target.instance;
        let N3::Dynel(Dynel::SimpleCharFullUpdate(update)) = &mut spawn.body else { unreachable!() };
        update.monster_scale = 100;
        let (breed, gender) = ao_formats::screens::wire_breed_sex(update.breed as i32, update.sex as i32).unwrap();
        let model = ao_formats::character::player_model_build_with_names(&names, breed, gender, update.fatness.min(2)).unwrap();
        let data = store.get(1_000_047, model).unwrap().unwrap();
        let expected = (f32::from_le_bytes(data[4..8].try_into().unwrap()), f32::from_le_bytes(data[12..16].try_into().unwrap()));
        let mut world = Dynels { nano_store: Some(store), names: Some(names), ..Default::default() };
        world.on_message(&spawn);
        assert_eq!(world.character_trade_bounds(who).unwrap(), Some(expected));
        assert_ne!(expected.1, 0.5, "the fixture distinguishes visual metadata from the old collision guess");
        let character = world.chars.get_mut(&who).unwrap();
        character.scale = 2.0;
        character.bounds = Some(([-100.0; 3], [100.0; 3]));
        assert_eq!(world.character_trade_bounds(who).unwrap(), Some((expected.0 * 2.0, expected.1 * 2.0)));
        assert_eq!(world.character_trade_bounds(-1).unwrap(), None);
    }

    #[test]
    fn captured_remote_sitting_player_stands_before_forward_locomotion() {
        use ao_net::n3::{action::id, outgoing::{char_dc_move, n3_frame, CharMove}};
        let mut spawn = frames(include_str!("../../../../docs/captures/zone_ithaca.rec")).iter().find_map(|frame| {
            let m = ao_net::n3::decode(frame).ok()?;
            (m.header.target.instance == 33491 && matches!(&m.body, N3::Dynel(Dynel::SimpleCharFullUpdate(_)))).then_some(m)
        }).expect("captured remote Stanko SCFU at 70758 ms");
        // No capture has a seated SCFU: reproduce the reported initial posture
        // by changing only the decoded movement mode in this real remote fixture.
        let N3::Dynel(Dynel::SimpleCharFullUpdate(u)) = &mut spawn.body else { unreachable!() };
        u.blob[12] = Mode::SitGround as u8;
        let who = spawn.header.target.instance;
        let mut stand = frames(include_str!("../../../../docs/captures/zone_kill_ithaca.rec")).iter().find_map(|frame| {
            let m = ao_net::n3::decode(frame).ok()?;
            matches!(&m.body, N3::World(World::CharacterAction(a)) if a.action == id::STAND_UP).then_some(m)
        }).expect("captured incoming stand relay at 106841 ms");
        // Retarget the captured own stand relay to the captured remote player:
        // retail cases 0x15/0x16 use exactly the same handler for either identity.
        stand.header.target = spawn.header.target;
        let mut world = Dynels::default();
        world.on_message(&spawn);
        assert_eq!(world.chars[&who].mover.pose().anim, AnimState::SitGround);
        world.on_message(&stand);
        let pos = world.chars[&who].mover.sim_pos();
        let forward = CharMove { action: 1, pos, rot: [0.0, 0.0, 0.0, 1.0], elapsed_ms: 0, look: [0.0; 2] };
        world.on_message(&ao_net::n3::decode(&n3_frame(1, 1, char_dc_move(who, &forward))).unwrap());
        let c = world.chars.get_mut(&who).unwrap();
        assert!(matches!(c.mover.pose().anim, AnimState::Walk | AnimState::Run), "stand relay must replace sitting with locomotion");
        let pose = c.mover.advance(0.25);
        assert_ne!(pose.pos[2], pos[2], "forward movement advances the standing character");
        assert_eq!(pose.pos[1], pos[1], "ground locomotion does not hover upward");
        let stopped = CharMove { action: 21, pos: c.mover.sim_pos(), ..forward };
        world.on_message(&ao_net::n3::decode(&n3_frame(1, 1, char_dc_move(who, &stopped))).unwrap());
        let sit = CharMove { action: 30, ..stopped };
        world.on_message(&ao_net::n3::decode(&n3_frame(1, 1, char_dc_move(who, &sit))).unwrap());
        assert_eq!(world.chars.get_mut(&who).unwrap().mover.advance(0.25).anim, AnimState::SitGround, "PC sit DC persists beyond the transition one-shot");
        world.on_message(&stand);
        assert_eq!(world.chars[&who].mover.pose().anim, AnimState::Idle, "captured stand clears the DC sit too");
    }

    #[test]
    fn remote_real_posture_clips_preserve_terminal_pose_and_crossfade() {
        use ao_net::n3::action::id;
        use std::fmt::Write as _;
        let Some(dir) = client() else { return };
        let store = RecordStore::open(&dir).unwrap();
        let output = std::env::var_os("AOMAC_POSTURE_TRACE").map(std::path::PathBuf::from);
        if let Some(output) = &output { std::fs::create_dir_all(output).unwrap(); }
        let scene = if output.is_some() && std::env::var_os("AOMAC_POSTURE_PNG").is_some() {
            Some(ao_formats::playfield::load_playfield(&store, &dir, 4604).unwrap())
        } else { None };
        let mut assets = ActorAssets::new(&store).unwrap();
        let mut spawn = captured_posture_player();
        let N3::Dynel(Dynel::SimpleCharFullUpdate(update)) = &mut spawn.body else { unreachable!() };
        update.blob[12] = Mode::Run as u8;
        let who = spawn.header.target.instance;
        let mut world = Dynels { dir: Some(dir), ..Default::default() };
        world.on_message(&spawn);
        let c = &world.chars[&who];
        let key = c.key;
        let built = build(&store, &mut assets, &c.look).unwrap();
        for id in [0xd5, 0xd6] {
            assert!(built.clips.get(&id).is_some_and(|clips| !clips.is_empty()), "first posture {id:#x} must be preloaded by the real model build");
        }
        world.asked.insert(key);
        world.model_order.push_back(key);
        world.model_bytes += built_bytes(&built);
        world.models.insert(key, Model::Ready { built: Box::new(built), uploaded: false });
        let pos = scene_pos(world.chars[&who].pose.pos);
        let eye = [pos[0], pos[1] + 1.5, pos[2] + 4.0];
        let mut host = Host::headless();
        let mut csv = String::from("frame,dt_ms,clip,clip_ms,rate,idle_delay,head_height,max_vertex_step,layers\n");
        let mut frame_no = 0usize;
        let mut previous_vertices: Option<Vec<ao_scene::Vertex>> = None;
        let mut tick = |world: &mut Dynels, host: &mut Host, dt| {
            host.actors.clear();
            host.actor_models.clear();
            world.update_with_collision(dt, eye, [0.0, 0.0, -1.0], host, None, |_| None);
            if let Some(output) = &output {
                let c = &world.chars[&who];
                let Model::Ready { built, .. } = &world.models[&key] else { unreachable!() };
                let rig = built.rig.as_ref().unwrap();
                let (vertices, parts) = rig.pose_composed(c.layers(built));
                let step = previous_vertices.as_ref().map_or(0.0, |previous| previous.iter().zip(&vertices)
                    .map(|(a, b)| (glam::Vec3::from(a.pos) - glam::Vec3::from(b.pos)).length()).fold(0.0f32, f32::max));
                let head = rig.head_attractor_composed(c.layers(built)).map_or(f32::NAN, |head| head[1]);
                let layers = c.layers(built).map(|layer| format!("{}:{:.6}:{:.6}", layer.clip.source_id, layer.ms, layer.blend)).collect::<Vec<_>>().join(";");
                writeln!(csv, "{frame_no},{:.6},{:#x},{:.6},{:.6},{:.6},{head:.7},{step:.7},{layers}", dt * 1000.0, c.anim, c.clip_ms, c.clip_rate, c.body_delay_ms).unwrap();
                if let Some(scene) = &scene {
                    let actor = ActorFrame {
                        id: who as u32, model: key, transform: glam::Mat4::from_scale(glam::Vec3::splat(c.scale)).to_cols_array_2d(),
                        parts, part_attractors: rig.part_attractors(), skin: Some(Arc::new(vertices.clone())), alpha: 1.0, always: true, ..Default::default()
                    };
                    let png = output.join(format!("remote-current-{frame_no:04}.png"));
                    ao_render::render_to_png_actors(scene, &[(key, built.model.clone())], vec![actor],
                        [3.0, 1.4, 3.0], [0.0, 0.9, 0.0], 640, 480, &png, 0.0).unwrap();
                }
                previous_vertices = Some(vertices);
                frame_no += 1;
            }
        };
        for (action, animation) in [(id::SIT_RELAY, 0xd5), (id::STAND_UP, 0xd6)] {
            for _ in 0..60 { tick(&mut world, &mut host, 1.0 / 60.0); }
            let c = &world.chars[&who];
            let Model::Ready { built, .. } = &world.models[&key] else { unreachable!() };
            let before = built.rig.as_ref().unwrap().pose_composed(c.layers(built)).0;
            let old = &built.clips[&c.anim][c.roll.variant];
            let (start, end) = super::super::avatar::loop_span(old).unwrap_or((0.0, old.duration));
            let span = end - start;
            let mut remaining = span - (c.clip_ms - start).rem_euclid(span);
            if remaining < 200.0 { remaining += (200.0 / span).ceil() * span; }
            let expected_idle_fade = remaining.clamp(200.0, 300.0);
            world.on_message(&posture_action(who, action));
            world.play_posture(who, animation);
            tick(&mut world, &mut host, 0.0);
            let c = &world.chars[&who];
            assert!(c.previous_body.is_some(), "posture entry preserves the old idle");
            assert!(world.pending_clips.is_empty() && world.replay.is_empty(), "first posture never waits for or replays an asynchronous clip");
            let Model::Ready { built, .. } = &world.models[&key] else { unreachable!() };
            assert_eq!(c.previous_body.as_ref().unwrap().fade_ms, expected_idle_fade, "idle fade derives from the outgoing CAT interval at enqueue");
            let start_pose = built.rig.as_ref().unwrap().pose_composed(c.layers(built)).0;
            let max_delta = before.iter().zip(&start_pose).map(|(a, b)| (glam::Vec3::from(a.pos) - glam::Vec3::from(b.pos)).length()).fold(0.0f32, f32::max);
            assert!(max_delta < 1e-5, "zero-time enqueue preserves the real CAT pose: {max_delta}");
            let node = c.actions.iter().find(|node| node.id == animation).unwrap();
            let duration = node.duration(&built.clips[&animation][node.variant.unwrap()]);
            let idle = &built.clips[&c.anim][c.roll.variant];
            let (start, end) = super::super::avatar::loop_span(idle).unwrap_or((0.0, idle.duration));
            assert_eq!(node.fade_ms, (end - start).clamp(200.0, 300.0), "enter fade derives from the newly queued idle interval");
            let delay = (duration - super::super::avatar::POSTURE_IDLE_LEAD_MS).max(0.0);
            assert_eq!(c.body_delay_ms, delay);
            let idle_start = c.clip_ms;
            let delay_frames = (delay / (1000.0 / 60.0)).ceil() as usize;
            for _ in 0..=delay_frames {
                let remaining = world.chars[&who].body_delay_ms;
                if remaining <= 0.0 { break; }
                let dt_ms = if remaining <= 1000.0 / 60.0 { remaining + 8.0 } else { 1000.0 / 60.0 };
                tick(&mut world, &mut host, dt_ms / 1000.0);
            }
            let c = &world.chars[&who];
            assert!((c.clip_ms - idle_start - 8.0).abs() < 0.001, "idle clock consumes the delay and advances by leftover time");
            assert!(c.actions.iter().any(|node| node.id == animation), "enter survives the queued idle waking");
            let frames = (super::super::avatar::POSTURE_IDLE_LEAD_MS / (1000.0 / 60.0)).ceil() as usize;
            for _ in 0..frames { tick(&mut world, &mut host, 1.0 / 60.0); }
            // Sample exactly at the finite node's end: its zero weight must agree with removal.
            let remaining = world.chars[&who].actions.iter().find(|node| node.id == animation).map_or(0.0, |node| (duration - node.ms).max(0.0));
            if remaining > 0.0 { tick(&mut world, &mut host, remaining / 1000.0); }
            let c = &world.chars[&who];
            let Model::Ready { built, .. } = &world.models[&key] else { unreachable!() };
            let terminal = built.rig.as_ref().unwrap().pose_composed(c.layers(built)).0;
            tick(&mut world, &mut host, 0.0);
            let c = &world.chars[&who];
            let Model::Ready { built, .. } = &world.models[&key] else { unreachable!() };
            let handoff = built.rig.as_ref().unwrap().pose_composed(c.layers(built)).0;
            let max_delta = terminal.iter().zip(&handoff).map(|(a, b)| (glam::Vec3::from(a.pos) - glam::Vec3::from(b.pos)).length()).fold(0.0f32, f32::max);
            assert!(max_delta < 1e-5, "finite enter removal preserves the real CAT pose: {max_delta}");
            for _ in 0..60 { tick(&mut world, &mut host, 1.0 / 60.0); }
            assert!(world.chars[&who].previous_body.is_none());
        }
        // The legacy emote/nano Once path must also render its last frame before changing bodies.
        world.play_once(who, 0xd5);
        tick(&mut world, &mut host, 0.0);
        let Model::Ready { built, .. } = &world.models[&key] else { unreachable!() };
        let duration = built.clips[&0xd5][world.chars[&who].roll.variant].duration;
        world.chars.get_mut(&who).unwrap().clip_ms = duration - 1.0;
        tick(&mut world, &mut host, 0.001);
        assert_eq!(world.chars[&who].special, Special::Once(0xd5));
        assert_eq!(world.chars[&who].clip_ms, duration, "Once cannot rewind before its final pose is sampled");
        tick(&mut world, &mut host, 0.0);
        assert!(world.chars[&who].previous_body.as_ref().unwrap().once, "outgoing Once playback must clamp, not wrap");
        tick(&mut world, &mut host, 1.0);
        world.on_message(&posture_action(who, id::SIT_RELAY));
        world.play_posture(who, 0xd5);
        tick(&mut world, &mut host, 0.0);
        tick(&mut world, &mut host, 0.1);
        let c = &world.chars[&who];
        assert!(c.body_delay_ms > 0.0, "reverse before the queued sit idle has any weight");
        let Model::Ready { built, .. } = &world.models[&key] else { unreachable!() };
        let rig = built.rig.as_ref().unwrap();
        let before_reverse = rig.pose_composed(c.layers(built)).0;
        let distance = |a: &[ao_scene::Vertex], b: &[ao_scene::Vertex]| a.iter().zip(b)
            .map(|(a, b)| (glam::Vec3::from(a.pos) - glam::Vec3::from(b.pos)).length()).fold(0.0f32, f32::max);
        let mut authored_step = 0.0f32;
        let mut radius = 0.0f32;
        for animation in [c.anim, 0xd7, 0xd5, 0xd6] {
            for clip in built.clips.get(&animation).into_iter().flatten() {
                let mut previous: Option<Vec<ao_scene::Vertex>> = None;
                let count = (clip.duration / (1000.0 / 60.0)).ceil() as usize;
                for frame in 0..=count {
                    let pose = rig.pose_composed([AnimLayer { clip, ms: (frame as f32 * (1000.0 / 60.0)).min(clip.duration), layers: 3, blend: 1.0 }].into_iter()).0;
                    radius = radius.max(distance(&before_reverse, &pose));
                    if let Some(previous) = &previous { authored_step = authored_step.max(distance(previous, &pose)); }
                    previous = Some(pose);
                }
            }
        }
        // At most three linear envelopes can change; 200 ms is retail's minimum fade.
        let next_frame_bound = authored_step + 3.0 * (2.0 * radius) / (200.0 / (1000.0 / 60.0));
        world.on_message(&posture_action(who, id::STAND_UP));
        world.play_posture(who, 0xd6);
        tick(&mut world, &mut host, 0.0);
        let c = &world.chars[&who];
        let Model::Ready { built, .. } = &world.models[&key] else { unreachable!() };
        let after_reverse = built.rig.as_ref().unwrap().pose_composed(c.layers(built)).0;
        assert!(distance(&before_reverse, &after_reverse) < 1e-5, "reversing a delayed idle cannot promote its invisible pose");
        tick(&mut world, &mut host, 1.0 / 60.0);
        let c = &world.chars[&who];
        let Model::Ready { built, .. } = &world.models[&key] else { unreachable!() };
        let next = built.rig.as_ref().unwrap().pose_composed(c.layers(built)).0;
        assert!(distance(&after_reverse, &next) <= next_frame_bound + 1e-5, "reversal step exceeds authored 60-Hz motion plus retail envelope bound");
        if let Some(output) = output { std::fs::write(output.join("remote-current.csv"), csv).unwrap(); }
    }

    fn captured_posture_player() -> Message {
        frames(include_str!("../../../../docs/captures/zone_enter_ithaca.rec")).iter().find_map(|frame| {
            let m = ao_net::n3::decode(frame).ok()?;
            matches!(&m.body, N3::Dynel(Dynel::SimpleCharFullUpdate(u)) if !u.is_npc()).then_some(m)
        }).unwrap()
    }

    #[test]
    fn captured_remote_yalmaha_stat_morph_restores_original_look() {
        let spawn = captured_posture_player();
        let who = spawn.header.target;
        let mut world = Dynels::default();
        world.on_message(&spawn);
        let original = world.chars[&who.instance].look.clone();
        let original_key = world.chars[&who.instance].key;
        let mut stat = spawn.clone();
        stat.body = N3::Dynel(Dynel::Stat(ao_net::n3::dynel::StatUpdate { stats: vec![(359, 99470), (360, 10)], rest: vec![] }));
        world.on_message(&stat);
        let c = &world.chars[&who.instance];
        assert!(matches!(&c.look, Look::Char(look) if look.monster_data == 99470));
        assert!(c.next.is_some());
        assert_eq!(c.scale, 0.2, "native stat360 minimum is 20 percent");
        world.apply_travel_stats(who, &[(662, 1), (359, 0)]);
        assert!(matches!(&world.chars[&who.instance].look, Look::Char(look) if look.monster_data == 99470), "mech gates rebuild, not canonical raw stat");
        world.apply_travel_stats(who, &[(662, 0), (359, 0), (360, 100)]);
        let c = &world.chars[&who.instance];
        assert_eq!(c.look, original);
        assert_eq!(c.next, None);
        assert_eq!(c.key, original_key);
        assert_eq!(c.scale, 1.0);

        let Some(dir) = client() else { return };
        let store = RecordStore::open(&dir).unwrap();
        let record = NpcRecord::lookup(&store, 99470).unwrap().unwrap();
        assert_eq!(record.mesh(), Some(42370));
        let mut assets = ActorAssets::new(&store).unwrap();
        let Look::Char(mut look) = original else { unreachable!() };
        look.monster_data = 99470;
        let built = build_char(&store, &mut assets, &look).unwrap();
        assert!(built.clips.contains_key(&0x78), "authored idle99429");
        assert!(built.clips.contains_key(&0x65), "authored movement99435");
        for key in [0xb2, 0xb3, 0xb4] {
            assert!(record.anim_variants(key).is_empty(), "Yalmaha has no authored hover clip {key:x}");
        }
        assert_eq!(ao_formats::character::holder_variants(&record, 0xb2), &[99429], "native hover parent resolves authored idle");
    }

    #[test]
    fn rdb_yalmaha_spells_replay_remote_apply_and_undo_through_zone() {
        use ao_net::{n3::{inventory as inv, outgoing::n3_frame, spells, world::{AcgItem, InventoryEntry}}, Writer};
        use super::super::hud_stats::equipment::Equipment;
        let Some(dir) = client() else { return };
        let frame = frames(include_str!("../../../../docs/captures/zone_enter_ithaca.rec")).into_iter().find(|frame| {
            matches!(ao_net::n3::decode(frame).map(|m| m.body), Ok(N3::Dynel(Dynel::SimpleCharFullUpdate(u))) if !u.is_npc())
        }).unwrap();
        let who = ao_net::n3::decode(&frame).unwrap().header.target;
        let mut zone = Zone::new((who.instance + 1) as u32);
        zone.on_frame(&frame);
        let original = zone.world.chars[&who.instance].look.clone();
        let original_scale = zone.stat_of(who.instance, 360);
        assert_eq!(zone.stat_of(who.instance, 359), Some(0));
        zone.inventory.insert(1, InventoryEntry {
            slot: 1, a: 0, b: 1, id: inv::item_identity(1),
            item: AcgItem { low_id: 96088, high_id: 117322, level: 30 },
        });
        let mut equipment = Equipment::new(&dir);
        equipment.refresh(&zone);
        assert_eq!(equipment.effects().iter().map(|spell| spell.function).collect::<Vec<_>>(),
            [0xcf35, 0xcf4c, 0xcf76, 0xcf92, 0xcf44]);
        for apply in [true, false] {
            let mut writer = Writer::default();
            writer.u32(spells::APPLY_SPELLS);
            who.write(&mut writer);
            writer.u8(0);
            writer.0.extend(spells::encode(who, equipment.effects(), apply).unwrap());
            zone.on_frame(&n3_frame(0, 43, writer.0));
            let remote = &zone.world.chars[&who.instance];
            if apply {
                assert_eq!((zone.stat_of(who.instance, 359), zone.stat_of(who.instance, 360)), (Some(99470), Some(20)));
                assert!(matches!(&remote.look, Look::Char(look) if look.monster_data == 99470));
                assert_eq!(remote.scale, 0.2);
                assert_eq!(remote.mover.status().mode, Mode::Fly);
            } else {
                assert_eq!((zone.stat_of(who.instance, 359), zone.stat_of(who.instance, 360)), (Some(0), original_scale));
                assert_eq!(remote.look, original);
                assert_ne!(remote.mover.status().mode, Mode::Fly);
            }
        }
    }

    fn posture_action(who: i32, action_id: i32) -> Message {
        use ao_net::n3::{action, outgoing::n3_frame};
        let none = ao_net::msg::Identity::default();
        ao_net::n3::decode(&n3_frame(1, 1, action::character_action(who, &action::simple(action_id, none, none)))).unwrap()
    }

    #[test]
    fn remote_stand_selects_sleep_and_lounge_from_full_update_and_stat() {
        use ao_net::n3::{action::{id, stat, wait}, dynel::StatUpdate, motion::STAT_FEATURES};
        for (mode, wait_state) in [(Mode::Sleep, wait::SLEEP), (Mode::Lounge, wait::LOUNGE)] {
            for initial in [false, true] {
                let mut spawn = captured_posture_player();
                let who = spawn.header.target.instance;
                let N3::Dynel(Dynel::SimpleCharFullUpdate(u)) = &mut spawn.body else { unreachable!() };
                u.blob[12] = mode as u8;
                u.pairs.extend([(STAT_FEATURES, 4), (stat::WAIT_STATE, if initial { wait_state } else { 0 })]);
                let mut world = Dynels::default();
                world.on_message(&spawn);
                if !initial {
                    let mut update = spawn.clone();
                    update.body = N3::Dynel(Dynel::Stat(StatUpdate { stats: vec![(stat::WAIT_STATE, wait_state)], rest: vec![] }));
                    world.on_message(&update);
                }
                world.on_message(&posture_action(who, id::STAND_UP));
                assert_eq!(world.chars[&who].mover.status().mode, Mode::SitGround, "{mode:?}, initial={initial}");
            }
        }
    }

    #[test]
    fn remote_accepted_sleep_and_lounge_update_mover_and_wait_state() {
        use ao_net::n3::{action::{self, id, wait}, outgoing::n3_frame};
        for (emote, mode, wait_state) in [(action::SLEEP_EMOTE, Mode::Sleep, wait::SLEEP), (action::LOUNGE_EMOTE, Mode::Lounge, wait::LOUNGE)] {
            let spawn = captured_posture_player();
            let who = spawn.header.target.instance;
            let mut world = Dynels::default();
            world.on_message(&spawn);
            world.on_message(&posture_action(who, id::SIT_RELAY));
            let mut payload = action::social_action(who, 1, emote);
            world.on_message(&ao_net::n3::decode(&n3_frame(1, 1, payload.clone())).unwrap());
            assert_eq!(world.chars[&who].mover.status().mode, Mode::SitGround, "unaccepted social does not change mode");
            payload[16] = 1; // SocialActionCmd state = accepted, as in combat/actions.rs.
            world.on_message(&ao_net::n3::decode(&n3_frame(1, 1, payload)).unwrap());
            assert_eq!(world.chars[&who].mover.status().mode, mode);
            assert_eq!(world.chars[&who].mover.wait_state(), wait_state);
            world.on_message(&posture_action(who, id::STAND_UP));
            assert_eq!(world.chars[&who].mover.status().mode, Mode::SitGround);
            world.on_message(&posture_action(who, id::STAND_UP));
            assert!(matches!(world.chars[&who].mover.status().mode, Mode::Walk | Mode::Run), "a second stand must leave sit, not repeat the sleep/lounge exit");
        }
    }

    #[test]
    fn remote_sit_relay_keeps_features_and_movement_guards() {
        use ao_net::n3::{action::id, motion::{MoveType, STAT_FEATURES}};
        let mut spawn = captured_posture_player();
        let who = spawn.header.target.instance;
        let N3::Dynel(Dynel::SimpleCharFullUpdate(u)) = &mut spawn.body else { unreachable!() };
        u.blob[12] = Mode::Run as u8;
        u.pairs.push((STAT_FEATURES, 2));
        let mut world = Dynels::default();
        world.on_message(&spawn);
        world.on_message(&posture_action(who, id::SIT_RELAY));
        assert_eq!(world.chars[&who].mover.status().mode, Mode::Run, "Features without mask 4 cannot sit");
        let mover = &mut world.chars.get_mut(&who).unwrap().mover;
        mover.on_stat(STAT_FEATURES, 4);
        mover.on_move_type(MoveType::ForwardStart as u8);
        world.on_message(&posture_action(who, id::SIT_RELAY));
        assert_eq!(world.chars[&who].mover.status().mode, Mode::Run, "moving characters cannot sit");
        world.chars.get_mut(&who).unwrap().mover.on_move_type(MoveType::FullStop as u8);
        world.on_message(&posture_action(who, id::SIT_RELAY));
        assert_eq!(world.chars[&who].mover.status().mode, Mode::SitGround);
        world.chars.get_mut(&who).unwrap().mover.on_move_type(MoveType::ForwardStart as u8);
        assert_eq!(world.chars[&who].mover.status().mode, Mode::SitGround, "movement must not invent a stand relay");
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
        assert!(w.collect_tags(0.016, own_pos, &[], |_| None).is_empty());
        // pref on: the characters within 30 m, nearest first, never the client character
        w.own = 25988;
        w.show_all_names = true;
        let tags = w.collect_tags(0.016, own_pos, &[], |_| None);
        assert!(tags.len() > 3, "{} tags", tags.len());
        assert!(tags.len() > 3 && tags.iter().all(|t| t.kind == IndicatorKind::Nametag && t.bar.is_none() && t.id != 25988));
        let d = |id: i32| {
            let p = scene_pos(w.chars[&id].pose.pos);
            (0..3).map(|i| (p[i] - own_pos[i]).powi(2)).sum::<f32>().sqrt()
        };
        assert!(tags.iter().all(|t| d(t.id) <= NAME_TAG_RADIUS) && tags.windows(2).all(|p| d(p[0].id) <= d(p[1].id)));
        // the set is rebuilt only every 2 s: walking away keeps it, 2 s later it is empty
        let far = [own_pos[0] + 500.0, own_pos[1], own_pos[2]];
        assert_eq!(w.collect_tags(0.016, far, &[], |_| None).len(), tags.len());
        assert!(w.collect_tags(2.1, far, &[], |_| None).is_empty());
        // indicators: the selected dynel gets a plate + bar tag first and no plain tag; flags bit 0x400 suppresses the selection one
        w.collect_tags(2.1, own_pos, &[], |_| None);
        let id = w.collect_tags(0.016, own_pos, &[], |_| None)[0].id;
        let sel = Indicator { id, kind: IndicatorKind::Selection, bar: Some(((30, 60), 0x00ff00)) };
        let t = w.collect_tags(0.016, own_pos, &[sel], |_| None);
        assert_eq!((t[0].id, t[0].kind, t[0].bar), (id, IndicatorKind::Selection, Some((32, 0x00ff00))));
        assert_eq!(t.iter().filter(|t| t.id == id).count(), 1);
        let npc = w.chars[&id].npc;
        w.chars.get_mut(&id).unwrap().npc = false;
        w.chars.get_mut(&id).unwrap().flags = 0;
        let red = w.collect_tags(0.016, own_pos, &[sel], |_| Some((1, 2)));
        assert_eq!(red.iter().find(|t| t.id == id).unwrap().rgb, 0xff0000);
        assert_eq!(w.tag_of(id, IndicatorKind::Nametag, None, Some((1, 1))).unwrap().rgb, 0xffffff);
        w.chars.get_mut(&id).unwrap().npc = true;
        assert_eq!(w.tag_of(id, IndicatorKind::Nametag, None, Some((1, 2))).unwrap().rgb, 0xffffff, "NPCs do not get BattleStation name tint");
        w.chars.get_mut(&id).unwrap().npc = false;
        w.chars.get_mut(&id).unwrap().flags = 1 << 22;
        assert_eq!(w.tag_of(id, IndicatorKind::Nametag, None, Some((1, 2))).unwrap().rgb, 0x00ee00, "native special-name colour takes precedence");
        w.chars.get_mut(&id).unwrap().npc = npc;
        w.chars.get_mut(&id).unwrap().flags = 0x400;
        let t = w.collect_tags(0.016, own_pos, &[sel], |_| None);
        assert!(t.iter().all(|t| t.kind == IndicatorKind::Nametag) && t.iter().any(|t| t.id == id));
        w.chars.get_mut(&id).unwrap().in_play = false;
        assert!(w.collect_tags(0.016, own_pos, &[sel], |_| None).iter().all(|t| t.id != id));
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
        assert!(w.collect_tags(0.016, own_pos, &[sel], |_| None).iter().all(|t| t.id != id));
        relocate(w, ao_net::msg::Identity::default());
        assert!(w.on_ground(prop));
        let key = w.chars[&id].key;
        if let Some(Model::Ready { built, .. }) = w.models.get_mut(&key) {
            built.visible = false;
        }
        assert!(w.collect_tags(0.016, own_pos, &[sel], |_| None).iter().all(|t| t.id != id));
        if let Some(Model::Ready { built, .. }) = w.models.get_mut(&key) {
            built.visible = true;
        }
        // an unplaced dynel (server x <= 0) shows nothing
        w.chars.get_mut(&id).unwrap().pose.pos[0] = 0.0;
        assert!(w.collect_tags(0.016, own_pos, &[], |_| None).iter().all(|t| t.id != id));
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
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
        loop {
            if let Some(p) = w.props.get(&(who.kind, who.instance)) {
                eye = scene_pos(p.pos);
                eye[2] += 4.0;
                if matches!(w.models.get(&p.key), Some(Model::Ready { built, .. }) if built.item.is_some()) {
                    break;
                }
            }
            frame(&mut w, eye, 0.05, &mut host);
            let state = w.props.get(&(who.kind, who.instance)).map(|p| (p.key, model_state(&w, p.key)));
            assert!(!state.is_some_and(|(_, state)| state == "failed"), "door {who:?} model failed: {state:?}");
            assert!(std::time::Instant::now() < deadline, "door {who:?} model not ready after 300s: {state:?}");
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
        // The own avatar has its own builder; Dynels only queues remote characters.
        // An appearance update's next look is the requested model, not its old displayed key.
        let keys = |z: &Zone| z.world.chars.iter().filter(|(id, _)| **id != z.world.own)
            .map(|(_, c)| c.next.unwrap_or(c.key)).chain(z.world.props.values().map(|p| p.key)).collect::<Vec<_>>();
        assert!(looks.len() < z.world.chars.len(), "NPCs of one species share a model");
        let Some(dir) = client() else { return };
        let playfield = z.world.playfield.expect("capture announces its playfield");
        let placed: Vec<_> = placed_dynels(&RecordStore::open(&dir).unwrap(), playfield).unwrap().into_iter()
            .filter(|d| d.template != 0).map(|d| (d.kind as i32, d.instance as i32)).collect();
        z.world.start(dir.clone(), 25988);
        let own = z.own().unwrap().clone();
        let eye = scene_pos(own.pos);
        let fwd = scene_forward(own.yaw.unwrap_or(0.0));
        let mut host = Host::headless();
        let mut models = vec![];
        let mut actors = vec![];
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
        loop {
            z.world.update_with_collision(0.05, eye, fwd, &mut host, None, |_| None);
            models.append(&mut host.actor_models);
            actors = std::mem::take(&mut host.actors);
            let pending = keys(&z).iter().any(|k| !matches!(z.world.models.get(k), Some(Model::Ready { .. })))
                || z.world.want_placed.is_some() || placed.iter().any(|id| !z.world.props.contains_key(id)) || z.world.wield.len() < 3;
            if !pending {
                break;
            }
            let waiting: Vec<_> = keys(&z).into_iter().filter_map(|key| {
                let state = model_state(&z.world, key);
                (state != "ready").then_some((key, state))
            }).collect();
            assert!(!waiting.iter().any(|(_, state)| *state == "failed"), "captured required model failed: {waiting:?}");
            assert!(std::time::Instant::now() < deadline,
                "captured zone not ready after 300s: models={waiting:?}, placed request={:?}, missing placed={:?}, props={}, wield={}",
                z.world.want_placed, placed.iter().filter(|id| !z.world.props.contains_key(id)).collect::<Vec<_>>(),
                z.world.props.len(), z.world.wield.len());
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
        let hit = crate::play::interact_use::pick_objects(&crate::play::hud_target::Ray { origin, dir: toward, len: 100.0 }, &z, None);
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

    #[test]
    fn captured_remote_dynels_align_with_loaded_playfield() {
        let Some(dir) = client() else { return };
        let store = RecordStore::open(&dir).unwrap();
        let collision = std::rc::Rc::new(std::cell::RefCell::new(
            ao_formats::playfield::collision::Collision::load(&store, 4582).unwrap(),
        ));
        let mut world = Dynels::default();
        world.set_collision(Some(collision.clone()));
        let mut placements = 0;
        for frame in frames(include_str!("../../../../docs/captures/zone_ithaca.rec")) {
            let Ok(message) = ao_net::n3::decode(&frame) else { continue };
            world.on_message(&message);
            if matches!(message.body, N3::Dynel(Dynel::CharDCMove(_) | Dynel::SimpleCharFullUpdate(_))) {
                placements += 1;
            }
            world.advance(1.0 / 60.0);
            for character in world.chars.values() {
                let simulated = character.mover.sim_pos();
                assert!(simulated.iter().all(|v| v.is_finite()));
                assert_eq!(character.pose.pos[1], simulated[1], "draw reconcile must retain aligned Y");
            }
        }
        assert!(placements > 10);
        let character = world.chars.values_mut().find(|c| !matches!(c.special, Special::Die(_))).unwrap();
        let mut position = character.mover.sim_pos();
        let ground = collision.borrow().ground(scene_pos(position)).expect("captured actor has terrain");
        position[1] = ground - 0.2;
        character.mover.on_teleport(position, &[0.0, 0.0, 0.0, 1.0]);
        character.pose = remote_pose(&mut character.mover, Some(&collision), 0.0);
        assert!(character.mover.sim_pos()[1] >= ground, "placement uses the loaded floor immediately");
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
        c.actions.insert(0, ActionAnim { id: 0x6d, layer: canim::Layer::Upper, priority: 1, layers: 3, fade_ms: 200.0, ms: 100.0, variant: Some(0), note_fired: 0, rate: 1.0, key: None, slot: -1, delay: None, posture: false, before_body: false });
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

    /// `update_attack_bars`/`restart_attack_bar`/`attack_bar_progress`/`start_reload_bar`: no client rdb needed, the
    /// message-stat path of `combat::arms::Armory` already carries AttackDelay/RechargeDelay/InitiativeSkill/AmmoType.
    #[test]
    fn attack_and_reload_bars_track_hands_swings_and_fight_state() {
        let mut d = Dynels::default();
        let own = 1;
        d.arms.wield(own, 500, 6, None, &[(0x126, 500), (0xd2, 500), (440, 100), (0x1a4, 2)]);
        let mut stats = HashMap::new();
        stats.insert(100, 1200);
        stats.insert(51, 0);
        // no bar while not fighting
        d.update_attack_bars(own, false, 1.0);
        assert!(d.attack_bar_progress(own, &stats).is_empty());
        // fighting opens the right-hand bar at zero progress, then it fills
        d.update_attack_bars(own, true, 0.0);
        let bars = d.attack_bar_progress(own, &stats);
        assert_eq!(bars.len(), 1);
        assert_eq!(bars[0].0, 6);
        assert_eq!(bars[0].1, 0.0);
        let total = bars[0].2;
        assert!(total > 0.0);
        d.update_attack_bars(own, true, total * 0.5);
        let half = d.attack_bar_progress(own, &stats)[0].1;
        assert!((half - 0.5).abs() < 0.01, "{half}");
        // a swing of the own character's right hand restarts the bar
        d.restart_attack_bar(6);
        assert_eq!(d.attack_bar_progress(own, &stats)[0].1, 0.0);
        // a swing of an untracked slot (no such hand open) is a no-op
        d.restart_attack_bar(8);
        assert!(d.attack_bar_progress(own, &stats).iter().all(|&(s, ..)| s != 8));
        // leaving the fight clears the bar
        d.update_attack_bars(own, false, 1.0);
        assert!(d.attack_bar_progress(own, &stats).is_empty());
        // Reload: the ranged hand (AmmoType present) starts a bar and it clears once elapsed reaches its total
        assert!(d.reload_bar_progress().is_none());
        d.start_reload_bar(500);
        let (p0, reload_total) = d.reload_bar_progress().expect("a ranged hand is wielded");
        assert_eq!(reload_total, 5.0, "raw RechargeDelay (stat 0xd2 = 500) / 100, no initiative scaling, no AttackDelay sum");
        assert_eq!(p0, 0.0);
        assert!(!d.update_attack_bars(own, false, reload_total * 0.5));
        let (half, _) = d.reload_bar_progress().unwrap();
        assert!((half - 0.5).abs() < 0.01, "{half}");
        assert!(d.update_attack_bars(own, false, reload_total));
        assert!(d.reload_bar_progress().is_none(), "the reload bar closes once its duration elapses");
        assert!(!d.update_attack_bars(own, false, reload_total), "completion fires once");
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
        let playfield = zone.playfield.unwrap_or(800);
        let mut player = Player::new(&dir, &mut zone, playfield).expect("capture has own character");
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
        // Positive readiness waits use the safety deadline; negative observations retain their frame windows.
        let mut pump = |z: &mut Zone, frames: Option<usize>, until: &dyn Fn(&Dynels) -> bool| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
            let mut remaining = frames;
            loop {
                if remaining == Some(0) { return false; }
                if let Some(n) = &mut remaining { *n -= 1; }
                z.world.update_with_collision(0.02, [0.0; 3], [0.0, 0.0, -1.0], &mut host, None, |_| None);
                if until(&z.world) {
                    return true;
                }
                assert!(std::time::Instant::now() < deadline,
                    "own rifle {own} not resolved after 300s: wield={:?}, weapons={:?}, pending weapons={}",
                    z.world.wield.get(&own), z.world.weapons, z.world.pending_weapons.len());
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        };
        let attractors = |z: &mut Zone| std::mem::take(&mut z.own_events).into_iter().filter_map(|e| if let OwnEvent::Appearance(a) = e { Some(a.attractors.iter().filter(|t| t.b > 0).map(|t| (t.a, t.b as u32)).collect::<Vec<_>>()) } else { None }).collect::<Vec<_>>();
        let wear = |z: &mut Zone, i: usize| rec[2 * i..2 * i + 2].iter().for_each(|f| {
            let _ = z.on_frame(f);
        });
        wear(&mut z, 0);
        assert!(pump(&mut z, None, &|w| w.wielded_set(own) == Some(3)), "the rifle never resolved");
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
        assert!(!pump(&mut z, Some(20), &|w| w.wielded_set(own).is_some()), "the bag slot 0x41 is no hand");
        // wear again; unwield after the request went to the worker, before its answer is read: the stale answer is dropped
        wear(&mut z, 2);
        pump(&mut z, Some(1), &|_| false);
        unwield.iter().for_each(|f| {
            let _ = z.on_frame(f);
        });
        assert!(!pump(&mut z, Some(100), &|w| w.wielded_set(own).is_some()));
        wear(&mut z, 2);
        assert!(pump(&mut z, None, &|w| w.wielded_set(own) == Some(3)));
    }

    /// Same zone-in gap as `Armory` (`combat/arms.rs` test `full_character_inventory_seeds_the_own_weapon_at_zone_in`):
    /// a `FullCharacterIIR_t` inventory entry in the right-hand weapon-page slot resolves the own avatar's hand
    /// `AnimSet` and swing animation without a live `WeaponItemFullUpdateIIR_t` (the same rifle template 121569 as
    /// `zone_wear_rifle_borealis.rec`, so the expected AnimSet 3 / shot anim 0x3ff / delay 100 are already evidenced).
    #[test]
    fn full_character_inventory_seeds_the_own_wield_at_zone_in() {
        use ao_net::msg::Identity;
        use ao_net::n3::world::{AcgItem, FullCharacter, InventoryEntry};
        use ao_net::n3::N3Header;
        let Some(dir) = client() else { return };
        let own = 0x82e8;
        let mut z = Zone::new(own as u32);
        z.world.start(dir, own);
        let m = Message {
            header: N3Header { msg_type: 0, target: Identity { kind: CHAR_KIND, instance: own }, flag: 0 },
            sender: 1,
            body: N3::World(World::FullCharacter(Box::new(FullCharacter {
                inventory: vec![InventoryEntry {
                    slot: 6, a: 0, b: 1,
                    id: Identity { kind: 0xc74a, instance: 900 },
                    item: AcgItem { low_id: 121569, high_id: 121569, level: 1 },
                }],
                ..FullCharacter::default()
            }))),
        };
        z.world.on_message(&m);
        let mut host = ao_render::Host::headless();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
        while z.world.wielded_set(own) != Some(3) {
            z.world.update_with_collision(0.02, [0.0; 3], [0.0, 0.0, -1.0], &mut host, None, |_| None);
            assert!(std::time::Instant::now() < deadline, "own rifle {own} not resolved: weapons={:?} pending={}", z.world.weapons, z.world.pending_weapons.len());
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(z.world.arms.slot_item(own, 6).map(|i| i.dtype), Some(0x5a), "Armory sees the same seed");
        let (anim, delay, key) = z.world.pick_swing(own, canim::list::ATTACK).unwrap();
        assert_eq!((anim, delay, key), (0x3ff, 100, canim::list::ATTACK), "rifle shot");
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

    /// Opt-in visual evidence (`AOMAC_SHOT_DIR`): Bergdoktor's real `summon_light` lighting a dark ground plane at his
    /// feet, with and without the dynamic light wired through `Host::dynamic_lights`.
    #[test]
    fn captured_light_bar_lights_the_ground_at_night() {
        let Some(out) = std::env::var_os("AOMAC_SHOT_DIR").map(PathBuf::from) else { return };
        let Some(dir) = client() else { return };
        struct Shot { gui: Gui, scene: Option<ao_scene::Scene>, lights: Vec<ao_scene::Light> }
        impl ao_render::Frontend for Shot {
            fn gui(&self) -> &Gui { &self.gui }
            fn input(&mut self, _: ao_gui::InputEvent, _: &mut Host) {}
            fn frame(&mut self, dt: f32, _: (u32, u32), host: &mut Host) -> ao_gui::DrawList {
                host.camera = Camera::look_at(ao_render::Vec3::new(1.8, 1.3, 2.2), ao_render::Vec3::new(0.0, -0.3, 0.3));
                if let Some(scene) = self.scene.take() { host.set_scene(scene); }
                host.dynamic_lights = self.lights.clone();
                self.gui.frame(dt)
            }
        }
        let store = RecordStore::open(&dir).unwrap();
        let mut assets = ActorAssets::new(&store).unwrap();
        std::fs::create_dir_all(&out).unwrap();
        let u = frames(include_str!("../../../../docs/captures/zone_ithaca.rec")).iter().filter_map(|f| ao_net::n3::decode(f).ok())
            .find_map(|m| match m.body {
                N3::Dynel(Dynel::SimpleCharFullUpdate(u)) if u.name == "Bergdoktor" => Some(u),
                _ => None,
            }).expect("captured remote player");
        let built = build_char(&store, &mut assets, &CharLook::from_update(&u)).unwrap();
        let rig = built.rig.unwrap();
        let (vertices, transforms) = rig.pose(None);
        let lights = rig.mount_lights(&transforms); // actor-relative = world, the actor sits at the scene origin here
        assert!(!lights.is_empty(), "Bergdoktor's summon_light must decode");
        let mut scene = built.model;
        scene.meshes[0].vertices = vertices;
        scene.instances = transforms.into_iter().enumerate().map(|(mesh, transform)| ao_scene::Instance { mesh, transform }).collect();
        scene.environment = Some(ao_scene::Environment { sky_color: [0.0; 3], fog_color: [0.0; 3], fog_start: 400.0, fog_end: 800.0, ambient: [0.01; 3], sun_color: [0.0; 3], sun_dir: [0.0, 1.0, 0.0], sun_specular: 0.0 });
        // A plain, untextured 10x10 m ground quad under Bergdoktor's feet: without a dynamic light, `base_color` white
        // and near-zero ambient leave it almost black; the Light Bar's glow should visibly lift the patch around him.
        let gi = scene.meshes.len();
        let quad = [(-5.0, -5.0), (5.0, -5.0), (5.0, 5.0), (-5.0, 5.0)];
        let vb: Vec<ao_scene::Vertex> = quad.iter().map(|&(x, z)| ao_scene::Vertex { pos: [x, 0.0, z], normal: [0.0, 1.0, 0.0], uv: [0.0, 0.0], color: ao_scene::WHITE }).collect();
        let sub = ao_scene::Submesh::new(vec![0, 1, 2, 0, 2, 3], None);
        scene.meshes.push(ao_scene::Mesh { vertices: vb, submeshes: vec![sub] });
        scene.instances.push(ao_scene::Instance { mesh: gi, transform: ao_scene::IDENTITY });
        let mut shot = Shot { gui: Gui::new(&dir, None).unwrap(), scene: Some(scene), lights: vec![] };
        let mut renderer = ao_render::Offscreen::new(&shot, (640, 480)).unwrap();
        for (name, on) in [("off", false), ("on", true)] {
            shot.lights = if on { lights.clone() } else { vec![] };
            let list = renderer.frame(&mut shot, 0.016);
            renderer.png(&shot, &list, &out.join(format!("light-bar-{name}.png"))).unwrap();
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

    /// `AppearanceUpdateIIR_c::Activate` [GC 0x10071679] on a look: cloth by page and part (texture 0 clears, unnamed parts stay), the
    /// attractor list replaces the old one wholesale (an empty list unmounts the head too), `SET_DYNEL_800` no longer skips it.
    #[test]
    fn appearance_update_edits_the_look() {
        use ao_net::n3::world::{AppearanceUpdate, Attractor, ClothData};
        let mut l = CharLook { npc: false, breed: 1, sex: 2, race: 1, fatness: 1, head: Some(9), monster_data: 0, textures: vec![], cloth: vec![(4, 50), (1, 100)], attractors: vec![(0, 9)], skip_attractors: true };
        let cl = |id, b, c| ClothData { id, b, c, ..Default::default() };
        let mut a = AppearanceUpdate { cloth: vec![cl(1, 0, 0), cl(2, 77, 0), cl(3, 9, 1)], attractors: vec![Attractor { a: 1, b: 0x3ddf, c: 0, d: 2 }, Attractor { a: 0, b: 7, c: 0, d: 4 }], visual_flags: 0x1f, extra: 0 };
        l.apply_appearance(&a);
        assert_eq!((l.cloth.as_slice(), l.attractors.as_slice(), l.skip_attractors), (&[(2, 77), (4, 50), (8, 9)][..], &[(1, 0x3ddf), (0, 7)][..], false));
        a.cloth.clear();
        a.attractors.clear();
        l.apply_appearance(&a);
        assert_eq!((l.cloth.len(), l.attractors.len()), (3, 0), "no cloth named = unchanged; no attractors = no head, no weapon");
    }

    /// The captured wear / unwear `AppearanceUpdate`s (zone_wear_rifle_borealis.rec, own character) retargeted to another player of
    /// zone_ithaca.rec: his attractor list becomes the rifle (right hand) + head, then the head only, each time as a rebuilt model; the
    /// old model stays drawn until the new one is ready. Asset readiness has only a 300s safety deadline, not a fixed frame budget.
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
            let expected = w.chars[&id].next.unwrap_or(w.chars[&id].key);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
            loop {
                w.update_with_collision(0.0, [0.0; 3], [0.0, 0.0, -1.0], &mut host, None, |_| None);
                host.actors.clear();
                let c = &w.chars[&id];
                let state = match w.models.get(&expected) {
                    Some(Model::Ready { .. }) => "ready",
                    Some(Model::Loading) => "loading",
                    Some(Model::Failed) => "failed",
                    None => "not queued",
                };
                assert_ne!(state, "failed", "player {id} ({}) model {expected:016x} failed; current={:016x}, next={:?}", c.name, c.key, c.next);
                if c.key == expected && c.next.is_none() && state == "ready" {
                    return;
                }
                assert!(std::time::Instant::now() < deadline,
                    "player {id} ({}) model {expected:016x} not ready after 300s: {state}; current={:016x}, next={:?}", c.name, c.key, c.next);
                std::thread::yield_now();
            }
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
                    assert_eq!(zone.target, Some(npc), "death itself does not invalidate selection");
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
        assert!(z.world.character_visible(leet), "death construction does not hide the NPC");
        let mut host = Host::headless();
        let (eye, fwd) = (crate::play::zone::scene_pos(z.own().unwrap().pos), [0.0, 0.0, -1.0]);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
        loop {
            z.world.update_with_collision(0.0, eye, fwd, &mut host, None, |_| None);
            host.actors.clear();
            let c = &z.world.chars[&leet];
            if matches!(z.world.models.get(&c.key), Some(Model::Ready { .. })) { break; }
            let state = model_state(&z.world, c.key);
            assert_ne!(state, "failed", "death model {:016x} for {leet} failed", c.key);
            assert!(std::time::Instant::now() < deadline, "death model {:016x} for {leet} not ready after 300s: {state}; next={:?}", c.key, c.next);
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
        assert!(c.visible, "GC 0x1007b58c requires the corpse-received run flag 0x200 before hiding an NPC");
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

    #[test]
    fn wildlife_sensors_use_native_characters_and_dead_flags() {
        let mut zone = Zone::new(25988);
        for frame in frames(include_str!("../../../../docs/captures/zone_ithaca.rec")) {
            zone.on_frame(&frame);
        }
        let id = *zone.world.chars.iter().find(|(_, c)| c.name == "Beach Leet").unwrap().0;
        let native_pos = glam::Vec3::from_array(zone.world.chars[&id].pose.pos);
        zone.world.die(id, 0);
        let control = glam::Vec3::new(100.0, 20.0, 300.0);
        let (mut characters, mut dead) = (Vec::new(), Vec::new());
        zone.world.life_sensors(control, &mut characters, &mut dead);
        assert!(characters.iter().any(|sensor| sensor.pos == control));
        assert!(characters.iter().any(|sensor| sensor.pos == native_pos));
        assert!(dead.contains(&native_pos));
        assert!(characters.iter().all(|sensor| sensor.radius == 0.0), "danger radii do not come from character bodies");
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
        // The worker builds one FIFO queue; capture props and HashMap-ordered characters
        // must not put unrelated models ahead of the two this fixture waits for.
        z.world.pending.splice(0..0, [(z.world.chars[&player].key, z.world.chars[&player].look.clone()), (z.world.chars[&leet].key, z.world.chars[&leet].look.clone())]);
        let mut host = Host::headless();
        let (eye, fwd) = (scene_pos(z.own()?.pos), [0.0, 0.0, -1.0]);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
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
            assert!(std::time::Instant::now() < deadline, "fight fixture target models not ready after 300s: {pending:?}");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        Some((z, player, leet))
    }

    #[test]
    fn native_nano_timer_blocks_through_recharge_not_visual_finish() {
        let mut world = Dynels { own: 42, nano_effect_categories: 0, ..Default::default() };
        world.nano_templates.insert(-1, Arc::new(ao_formats::dynel_visual::ItemTemplate {
            kind: 0, name: None, sounds: vec![],
            stats: vec![(0, 0), (0x126, 100), (0xd2, 200), (0x195, 7)],
        }));
        let source = ao_net::msg::Identity { kind: CHAR_KIND, instance: 42 };
        let message = Message {
            header: ao_net::n3::N3Header { msg_type: ao_net::n3::dynel::CAST_NANO_SPELL, target: source, flag: 0 },
            sender: 42,
            body: N3::Dynel(Dynel::CastNanoSpell(ao_net::n3::dynel::CastNanoSpell {
                spell: -1, target: source, source, flag: true, rest: vec![],
            })),
        };
        let tick = |world: &mut Dynels, dt| world.nano_visual_frame(dt, true, |_| None, |_, _| None);
        world.start_local_nano(-1, 49999);
        assert_eq!(world.local_nano_initiation, Some((-1, 0, 0)), "native no-effect selector retains the program id but creates no handle");
        // Supplementary synthetic selector: tests ownership without claiming a rendered resource.
        world.start_local_nano(-1, 123);
        world.on_message(&message);
        world.nano_result(42, -1, 0);
        assert!(world.nano_busy(42), "accepted relay blocks before the frame drain");
        assert!(world.nano_school_busy(42, 7));
        assert!(!world.nano_busy(43));
        tick(&mut world, 0.5);
        assert!(world.nano_busy(42));
        assert!(world.local_nano_initiation.is_some(), "local initiation survives the attack phase");
        tick(&mut world, 0.5);
        assert!(world.local_nano_initiation.is_none(), "CharCastNano clears controller+8 when attack releases");
        world.take_nano_animations();
        tick(&mut world, 0.0);
        assert!(world.casting[0].done, "visual callback may finish before recharge");
        assert!(world.nano_busy(42));
        tick(&mut world, 2.0);
        assert!(world.nano_busy(42), "native recharge expiry is strictly past deadline");
        tick(&mut world, 0.01);
        assert!(!world.nano_busy(42));
        assert!(!world.nano_school_busy(42, 7));

        world.on_message(&message);
        world.start_local_nano(-1, 123);
        tick(&mut world, 0.0);
        world.cancel_nano_cast(42, -1);
        assert!(!world.nano_busy(42), "named attack cancel removes its timer");
        assert!(world.local_nano_initiation.is_none(), "named attack cancellation deletes the local initiation");
        world.on_message(&message);
        world.nano_result(42, -1, 1);
        tick(&mut world, 1.0);
        world.cancel_nano_cast(42, -1);
        assert!(world.nano_busy(42), "named cancel does not select recharge-stage entries");
        world.cancel_nano_cast(42, 0);
        assert!(!world.nano_busy(42), "unnamed cancel removes the first entry during recharge");

        world.on_message(&message);
        tick(&mut world, 0.0);
        world.on_message(&Message {
            header: ao_net::n3::N3Header { msg_type: ao_net::n3::world::CHARACTER_ACTION, target: source, flag: 0 },
            sender: 42,
            body: N3::World(World::CharacterAction(ao_net::n3::world::CharacterAction {
                action: 0x89, param: 0, identity_a: source, identity_b: source, text: String::new(),
            })),
        });
        assert!(!world.nano_busy(42), "queue clear releases kind3 even while visuals finish");

        world.nano_templates.insert(-2, Arc::new(ao_formats::dynel_visual::ItemTemplate {
            kind: 0, name: None, sounds: vec![],
            stats: vec![(0, 0x80000), (0x126, 100), (0xd2, 200), (0x195, 7)],
        }));
        world.nano_casts.push((42, ao_net::n3::dynel::CastNanoSpell {
            spell: -2, target: source, source, flag: true, rest: vec![],
        }));
        assert!(!world.nano_busy(42), "instant relays never allocate an internal timer");
        assert!(!world.nano_school_busy(42, 7));
        tick(&mut world, 0.0);
        assert!(!world.nano_busy(42));
        world.start_local_nano(-1, 123);
        world.cancel_nano_visuals(42);
        assert!(world.local_nano_initiation.is_none(), "source death/disappearance clears the initiation even before a relay");
        world.start_local_nano(-1, 123);
        world.clear();
        assert!(world.local_nano_initiation.is_none());
    }

    #[test]
    fn native_nano_result_releases_wait_and_exposes_pending_self_program() {
        let mut world = Dynels { own: 42, nano_effect_categories: 0, ..Default::default() };
        world.nano_templates.insert(-1, Arc::new(ao_formats::dynel_visual::ItemTemplate {
            kind: 0, name: None, sounds: vec![],
            stats: vec![(0, 0x10000), (75, 9), (643, 1000)],
        }));
        world.casting.push(NanoCast {
            who: 42, spell: -1, target: 42, handle: 0, remaining: 0.0, total: 1.0,
            finish: [49999; 2], release_anim: 201, released: false, release_seen: false,
            done: false, instant: false, execution: Some(NanoExecution {
                left: 0.0, recharge: Some(2.0), timer: true, result: None,
            }), finish_enabled: true, start_effect: 49999,
        });
        let tick = |world: &mut Dynels, dt| world.nano_visual_frame(dt, true, |_| None, |_, _| None);
        tick(&mut world, 5.0);
        assert!(world.nano_busy(42), "elapsed attack cannot invent an authoritative result");
        assert!(!world.casting[0].released, "release callbacks also await the authoritative result");
        assert_eq!(world.nano_pending_self(42).count(), 0);
        assert!(!world.nano_family_locked(42, [9; 6]));
        let source = ao_net::msg::Identity { kind: CHAR_KIND, instance: 42 };
        world.on_message(&Message {
            header: ao_net::n3::N3Header { msg_type: ao_net::n3::world::CHARACTER_ACTION, target: source, flag: 0 },
            sender: 42,
            body: N3::World(World::CharacterAction(ao_net::n3::world::CharacterAction {
                action: 0x6b, param: 0, identity_a: source,
                identity_b: ao_net::msg::Identity { kind: 0, instance: -1 }, text: String::new(),
            })),
        });
        tick(&mut world, 0.0);
        assert_eq!(world.nano_pending_self(42).map(|(spell, _)| spell).collect::<Vec<_>>(), [-1]);
        assert_eq!(world.nano_pending_self(43).count(), 0);
        assert!(world.nano_family_locked(42, [9; 6]), "result0 preserves success1");
        tick(&mut world, 2.1);
        assert!(!world.nano_busy(42));
        assert_eq!(world.nano_pending_self(42).count(), 0);
    }

    #[test]
    fn native_nano_family_lock_uses_authored_whole_second_ttl() {
        let mut world = Dynels::default();
        world.nano_templates.insert(-1, Arc::new(ao_formats::dynel_visual::ItemTemplate {
            kind: 0, name: None, sounds: vec![],
            stats: vec![(75, 9), (546, 10), (254, 199), (643, 299), (644, 100), (645, 99)],
        }));
        world.start_nano_locks(42, -1);
        assert!(world.nano_family_locked(42, [9, 0, 0, 0, 0, 0]));
        assert!(world.nano_family_locked(42, [0, 0, 10, 0, 0, 0]));
        assert!(!world.nano_family_locked(43, [9; 6]));
        assert!(!world.nano_family_locked(42, [0; 6]), "absent extra families create no lock");
        assert!(world.nano_program_locked(42, -1));
        assert!(!world.nano_program_locked(43, -1));
        assert!(!world.nano_program_locked(42, 9), "a family number is not a program identity");
        world.advance_nano_locks(0.5);
        assert!(world.nano_family_locked(42, [10; 6]));
        assert!(world.nano_program_locked(42, -1));
        world.advance_nano_locks(0.5);
        assert!(!world.nano_family_locked(42, [10; 6]));
        assert!(!world.nano_program_locked(42, -1), "stat254 truncates to whole seconds on the shared pulse");
        assert!(world.nano_family_locked(42, [9; 6]));
        world.advance_nano_locks(1.0);
        assert!(!world.nano_family_locked(42, [9; 6]));
        world.start_nano_locks(42, -1);
        world.clear();
        assert!(!world.nano_family_locked(42, [9; 6]));
        assert!(!world.nano_program_locked(42, -1));
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
            world.nano_result(player, -1, 1); // Explicit successful result for this synthetic stage replay.
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
        world.nano_result(player, 29091, 1); // Explicit result; the template alone cannot confirm execution.
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
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
            while zone.world.effect_anchor(target, 0, 0).is_none() {
                zone.world.update_with_collision(0.0, eye.to_array(), [0.0, 0.0, -1.0], &mut host, None, |_| None);
                let c = &zone.world.chars[&target];
                let state = model_state(&zone.world, c.next.unwrap_or(c.key));
                assert_ne!(state, "failed", "captured target {target} model failed");
                assert!(std::time::Instant::now() < deadline,
                    "captured target {target} connector unavailable after 300s: model={:016x}, next={:?}, state={state}", c.key, c.next);
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

    /// `step`/`land` notes (the walk/run footfall and jump-landing clip): a player plays the shared `Feet_Man_Walk`/`Feet_Man_Land`
    /// samples (not something per breed/sex), a creature its own record list instead, with no collision loaded the default
    /// material 5 (`Collision::ground_material`'s own no-terrain fallback, covered separately in ao-formats).
    #[test]
    fn footstep_and_land_notes_play_the_shared_feet_sounds_or_the_creatures_own_list() {
        use crate::play::combat::notes::id;
        let Some((mut z, player, leet)) = fight_zone() else { return };
        let (pp, lp) = (scene_pos(z.world.chars[&player].pose.pos), scene_pos(z.world.chars[&leet].pose.pos));
        let sid = ao_audio::sbf::sound_id;
        z.world.note_sounds(player, id::STEP, -1);
        z.world.note_sounds(player, id::LAND, -1);
        assert_eq!(
            z.world.take_sounds(),
            [
                GameSound { id: sid("SM_Sandy_Game_Feet_Man_Walk"), pos: pp, material: 5, size: 1 },
                GameSound { id: sid("SM_Sandy_Game_Feet_Man_Land"), pos: pp, material: 5, size: 1 },
            ],
            "no collision loaded: the client's default ground material 5"
        );
        let rec = {
            let c = &z.world.chars[&leet];
            let Some(Model::Ready { built, .. }) = z.world.models.get(&c.key) else { panic!("leet model not ready") };
            built.sounds.clone()
        };
        for note in [id::STEP, id::LAND] {
            z.world.note_sounds(leet, note, -1);
            let got = z.world.take_sounds();
            let want = rec.iter().find(|s| s.0 == note).map_or(&[][..], |s| &s.1[..]);
            assert_eq!(got.len(), usize::from(!want.is_empty()), "note {note:#x}: {want:?}");
            assert!(got.iter().all(|g| want.contains(&g.id) && g.pos == lp && g.material == 5), "a creature plays its own list, not the player's sample, at the same default material");
        }
    }

    /// `swim` (0x38): a player plays `SM_Sandy_Game_Feet_Man_Swim`, a creature its own record list, same as step/land.
    #[test]
    fn swim_notes_play_the_shared_swim_sample_or_the_creatures_own_list() {
        use crate::play::combat::notes::id;
        let Some((mut z, player, _)) = fight_zone() else { return };
        let pp = scene_pos(z.world.chars[&player].pose.pos);
        let sid = ao_audio::sbf::sound_id;
        z.world.note_sounds(player, id::SWIM, -1);
        assert_eq!(z.world.take_sounds(), [GameSound { id: sid("SM_Sandy_Game_Feet_Man_Swim"), pos: pp, material: 5, size: 1 }]);
    }

    /// The Beach Leet's authored attack notes come from exactly one holder node per swing, not a duplicate/stacked
    /// one: some attack clip variants are authored multi-hit combos whose own note id legitimately fires more than
    /// once in a single (non-looping) play (confirmed live: clip 1034's note 38 fires at 4 distinct authored times,
    /// `note_fired` bits 0 and 1 both set), so repeated note values are not themselves evidence of a bug.
    #[test]
    fn a_holder_swing_clip_reports_its_notes() {
        let Some((mut z, _, leet)) = fight_zone() else { return };
        let mut host = Host::headless();
        let (eye, fwd) = (scene_pos(z.own().unwrap().pos), [0.0, 0.0, -1.0]);
        // `fight_zone` replays real captured combat traffic, which can leave the leet already mid-swing by the time
        // the async model finishes loading and this function returns: clear any leftover action so `play_swing`
        // below always begins a genuinely fresh holder node (not reusing one with in-flight `ms`/`note_fired`).
        z.world.chars.get_mut(&leet).unwrap().actions.clear();
        z.world.take_notes();
        z.world.play_swing(leet, None, canim::list::ATTACK);
        let mut notes = vec![];
        // Observe this holder node's lifetime (`Char::actions`, emptied by `ActionAnim::advance` returning false on
        // completion), not a fixed frame count: whichever attack clip variant `Dynels::rng` happens to pick
        // (wall-clock seeded in production, like the client's `srand(time)`) plays to its own natural end.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            z.world.update_with_collision(0.05, eye, fwd, &mut host, None, |_| None);
            host.actors.clear();
            notes.extend(z.world.take_notes());
            let count = z.world.chars[&leet].actions.iter().filter(|a| a.key == Some(canim::list::ATTACK)).count();
            assert!(count <= 1, "at most one attack holder node for this key at a time: {count}");
            if count == 0 {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "leet attack holder node never finished");
        }
        // idle/walk clips legitimately repeat their locomotion notes (`0xf` idle) every loop now that the base clip fires
        // notes too (`footstep_and_land_notes_...`); this test is about the attacking leet's own attack notes only.
        notes.retain(|n| n.0 == leet && n.1.id != 0xf);
        eprintln!("leet attack clip notes: {notes:?}");
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
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
        loop {
            z.world.update_with_collision(0.05, eye, fwd, &mut host, None, |_| None);
            host.actors.clear();
            if matches!(z.world.chars[&leet].special, Special::Once(k) if k == u32::from(anim)) { break; }
            assert!(std::time::Instant::now() < deadline, "imp clip {anim:#x} for {leet} did not start after 300s; model={:016x} ({}), pending replay {:?}", z.world.chars[&leet].key, model_state(&z.world, z.world.chars[&leet].key), z.world.replay);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(z.world.react_to_hit(leet, 4).1, 1.0);
    }

    /// Leak regression (user report: "massive memory leak... had to hard reset"): replays a real capture (combat,
    /// notes, NPC dynels) many times through a single long-lived `Dynels`, ticking the production `update_with_collision`
    /// path every frame like the live client. Per-source/per-dynel caches (`chars`, `combat::effects::Renderer`'s
    /// `active`/`anchors`/`source_runtime`/`buff_models`) must plateau once every dynel in the capture has appeared at
    /// least once, not keep growing with the number of replays (frames, not wall-clock).
    #[test]
    fn replayed_capture_keeps_dynel_and_effect_caches_bounded() {
        let Some(dir) = client() else { return };
        let own = 25988;
        let mut z = Zone::new(own as u32);
        z.world.start(dir, own);
        let rec = frames(include_str!("../../../../docs/captures/zone_ithaca.rec"));
        let mut host = Host::headless();
        let mut samples: Vec<(usize, usize, usize, usize, usize)> = Vec::new();
        for rep in 0..300 {
            for f in &rec {
                z.on_frame(f);
                let (eye, fwd) = z.own().map_or(([0.0; 3], [0.0, 0.0, -1.0]), |o| (scene_pos(o.pos), [0.0, 0.0, -1.0]));
                z.world.update_with_collision(0.016, eye, fwd, &mut host, None, |_| None);
                host.actors.clear();
            }
            let (active, anchors, source_runtime, buff_models) = z.world.effects.as_ref().map_or((0, 0, 0, 0), |e| e.resource_counts());
            samples.push((z.world.chars.len(), active, anchors, source_runtime, buff_models));
            // warm-up window: let every dynel in the capture appear at least once before measuring the plateau.
            if rep == 19 {
                let warm = samples.last().copied().unwrap();
                samples.clear();
                samples.push(warm);
            }
        }
        let (first, last) = (samples[0], *samples.last().unwrap());
        assert_eq!(first.0, last.0, "chars must not grow across replays: {first:?} -> {last:?}");
        assert!(last.1 <= first.1 + 2, "active effects must plateau, not grow per replay: {first:?} -> {last:?}");
        assert!(last.2 <= first.2 + 4, "effect anchors must plateau, not grow per replay: {first:?} -> {last:?}");
        assert!(last.3 <= first.3 + 2, "source_runtime must plateau, not grow per replay: {first:?} -> {last:?}");
        assert!(last.4 <= first.4 + 2, "buff_models must plateau, not grow per replay: {first:?} -> {last:?}");
    }

    /// Bergdoktor's real `summon_light` attachment (mesh 26163, `Attractor06_back`, `docs/captures/zone_ithaca.rec`,
    /// `docs/zone/dynel.md` §1.3 -- the user-reported "Light Bar" UTILS item) must reach the renderer as a moving
    /// point light once his model is built, tracking his position every frame, and disappear once he leaves view.
    #[test]
    fn worn_light_bar_reaches_the_renderer_as_a_tracked_point_light() {
        let Some(dir) = client() else { return };
        let own = 25988;
        let mut z = Zone::new(own as u32);
        z.world.start(dir, own);
        z.world.char_view_distance = 100_000.0; // the authored light's own range (6.13 m) still bounds who it lights, see `vertex_lights`
        let rec = frames(include_str!("../../../../docs/captures/zone_ithaca.rec"));
        let mut host = Host::headless();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        let berg_key = loop {
            for f in &rec {
                z.on_frame(f);
                let (eye, fwd) = z.own().map_or(([0.0; 3], [0.0, 0.0, -1.0]), |o| (scene_pos(o.pos), [0.0, 0.0, -1.0]));
                z.world.update_with_collision(0.016, eye, fwd, &mut host, None, |_| None);
                host.actors.clear();
                host.dynamic_lights.clear();
            }
            if let Some(c) = z.world.chars.values().find(|c| c.name == "Bergdoktor") {
                if model_state(&z.world, c.key) == "ready" {
                    break c.key;
                }
            }
            assert!(std::time::Instant::now() < deadline, "Bergdoktor's model not ready after 60s");
        };
        // One more tick with Bergdoktor's model ready: his embedded light must be present, near his own position,
        // matching the authored `RLight_t` (range/colour) within the mount's own accumulated transform error.
        let (eye, fwd) = z.own().map_or(([0.0; 3], [0.0, 0.0, -1.0]), |o| (scene_pos(o.pos), [0.0, 0.0, -1.0]));
        z.world.update_with_collision(0.016, eye, fwd, &mut host, None, |_| None);
        let berg_pos = scene_pos(z.world.chars.values().find(|c| c.name == "Bergdoktor").unwrap().pose.pos);
        let lights = host.dynamic_lights.clone();
        let near = lights.iter().find(|l| (0..3).map(|i| (l.pos[i] - berg_pos[i]).powi(2)).sum::<f32>().sqrt() < 2.5);
        let l = near.unwrap_or_else(|| panic!("no dynamic light near Bergdoktor {berg_pos:?}: {lights:?}"));
        assert!((l.range - 6.13).abs() < 0.1, "authored summon_light range: {}", l.range);
        assert!((l.color[0] - 1.0).abs() < 0.05 && (l.color[1] - 0.836).abs() < 0.05 && (l.color[2] - 0.394).abs() < 0.05, "authored summon_light colour: {:?}", l.color);
        // Dropping his dynel (n3ToClientQuit) must drop his light with it, not leave it lit forever.
        z.world.chars.remove(&z.world.chars.iter().find(|(_, c)| c.key == berg_key).map(|(id, _)| *id).unwrap());
        host.dynamic_lights.clear();
        let (eye, fwd) = z.own().map_or(([0.0; 3], [0.0, 0.0, -1.0]), |o| (scene_pos(o.pos), [0.0, 0.0, -1.0]));
        z.world.update_with_collision(0.016, eye, fwd, &mut host, None, |_| None);
        assert!(host.dynamic_lights.iter().all(|l| (0..3).map(|i| (l.pos[i] - berg_pos[i]).powi(2)).sum::<f32>().sqrt() >= 2.5), "light must not linger after the dynel is gone");
    }
}
