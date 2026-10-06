# Other players, NPCs and objects in the world (`play/dynels.rs`)

Everything the zone server announces besides the own character is drawn through the renderer's actor layer. Evidence for the
individual rules lives in the RE docs this page ties together: `docs/zone/dynel.md` (wire layouts), `docs/zone/npc.md` (rdb 1040023
NPC records, texture/cloth/attractor rules), `docs/zone/motion.md` (movement FSM, animation ids, name tags),
`docs/zone/static.md` (corpses, vending machines, items, playfield-placed dynels), `docs/zone/combat-anim.md` (death sequence).

## 1. Pipeline

```
N3 frame ──Zone::on_frame──▶ Dynels::on_message ──▶ Char (Mover) / Prop      ──▶ Dynels::update ──▶ Host::actors (ActorFrame)
                                      │ Look (appearance)                                 ▲ Host::actor_models (model scene, once per Look)
                                      ▼
                         worker thread: build(Look) → Built { model: Scene, rig: ActorRig, clips, features, held pose }
```

* `ao_scene::ActorFrame` / `Renderer::add_actor_model` / `Host::{actors, actor_models}` (`crates/ao-render/src/actors.rs`): the model's
  indices, materials and textures are uploaded once per key; the skinned body (`meshes[0]`) has one vertex buffer per actor that
  is rewritten when a new pose is supplied; head and attached weapons are rigid meshes that only get a transform. Actors are
  frustum-culled per mesh (bounding sphere + 1 m pose margin on the body), opaque parts first, blended parts far to near,
  after the world. Actors that are not pushed in a frame are forgotten (`Renderer::set_actors`).
* `ao_formats::character::actor::ActorRig` builds a model from a CAT model + head + part textures + attachment meshes and
  skins it on the CPU for any clip/time (`pose`) — 600 body vertices, ≈ 5 µs per pose.
* `Look` (hash = model key) is what the app derives from a message: `Char` (SimpleCharFullUpdate: breed/sex/race/fatness, head,
  `MonsterData`, `textures[]`, page-0 `cloth[]`, attractor meshes), `Corpse`, `Item` (StaticInstance template + stats).
  Equal looks share one model: the 81 characters of the capture need 13 models.

## 2. What is drawn and how

| dynel | look | source |
|---|---|---|
| player (`F & 1 == 0`) | `ActorRig::player`: body of breed/sex/`Fatness` (thin/normal/fat), naked skin of the `Race` (1 caucasian, 2 african, 3 asian) with the worn cloth composited over it (green key), head mesh `HeadMesh`, attractor meshes (weapons: places 1/2 = right/left hand) | docs/zone/dynel.md §1.3, formats.md § Skin |
| NPC (`F & 1`) | rdb 1040023 record `MonsterData` → `Mesh` (rdb 1010002), `HeadMesh` of the record if the message has none; `textures[]` replace the part textures by exact material name, `cloth[]` are composited over the part texture; `MonsterScale/100` uniform scale | docs/zone/npc.md |
| corpse (kind 0xC76A) | `CATMesh` stat model + cloth/head/skin/`textures[]`, `MonsterScale`; **pose = last frame of the model's `die*` clip [GUESS]** (the original's corpse pose was not found, docs/zone/static.md §5) | docs/zone/static.md |
| vending machine (0xC75B) and other item-family dynels | `StaticInstance` template (rdb 1000020) stats overlaid by the message stats → `Mesh` (rdb 1010001; default `pickupbox_misc` 9013) / `CATMesh`, override texture, `Flags` bit 0 = visible | docs/zone/static.md §2 |
| doors, billboards, terminals the playfield places itself | rdb 1000026 `PlacedDynel` (`CreateRDBDynels`) → same item path (template + blob stats); position/rotation as stored | docs/zone/static.md §6 |
| held weapon items (0xC74A) | invisible (they have a parent); the weapon is the holder's attractor mesh | docs/zone/static.md §4 |

Coordinates: scene = `(x, y, -z)` of the server position; a model faces -Z at rest, so the actor rotation is `scene_yaw(server_yaw)`
(`play/zone.rs`, verified against NPC travel directions). Server `y` is trusted (the client never lets a dynel sink below terrain but does not
snap it up either, docs/zone/motion.md §4d).

## 3. Movement and animation

`ao_net::n3::motion::Mover` (the client's movement FSM + vehicle) is fed per dynel: `SimpleCharFullUpdate` (position, heading,
FSM blob, `RunSpeed`, health, `path`), `CharDCMove` (move type → FSM transition; the drawn position blends `0.8·old + 0.2·new` per frame after a
correction, docs/zone/motion.md §4), `FollowTarget` (waypoint walking = NPC wandering), `SetWantedDirection` (turn in place), `Stat` (RunSpeed,
Health, Features...). `Mover::pose()` names the animation state; `AnimState::anim_id()` is the client's animation id, which is
the key of the NPC record's animation table (NPCs) or the clip name of the model's set (players). Missing clips follow the client's fallback chain
(`AnimState::fallback`), then idle. Walk/run clips are scaled by `speed / max speed of the mode` (0.3…2×). Deaths: `CharacterAction` 99 (`identity_b.instance` =
death animation id, 500…505 / 0xcc → fall back to 6000 `die-pain`) plays the clip once and holds the last frame; the server removes the dynel
~3 s later (`n3ToClientQuit`, docs/zone/combat-anim.md §5.1). `Dynels::attack(id)` plays the unarmed attack once (hook for combat).

Skinning cost control: a character is re-skinned every 40 ms within 30 m, every 100 ms within 80 m, every 250 ms beyond, and not at all
while it is behind the camera; held poses (corpses, dead characters at the end of the clip) are uploaded once. Beyond 250 m
(≈ the fog distance) nothing is drawn.

## 4. Name tags

Only with the pref `ShowAllNames` (off by default in the original, `OptionPanel/Root.xml`; here `Dynels::show_all_names`, env
`AOMAC_SHOW_ALL_NAMES` for testing): text, colour and anchor per docs/zone/motion.md §6 (`ao_net::n3::nametag`), font `FontGameShell12`,
dynels within 30 m of the player. **Not faithful yet:** the original draws the text into a world-space billboard (`width/128 × 0.3` m, so it
shrinks with distance); the GUI draw list only has 1:1 glyphs, so the tag is drawn at native size at the projected anchor. The selection /
attack indicator over a targeted dynel belongs to the target window (`hud_target.rs`).

## 5. Open items

* Corpse pose (see §2); the corpse look for non-player models without a `die*` clip is the bind pose.
* `Features` of players is an inference (bit 4); NPC `Features` come from their record.
* Environment-map layer (`TextureData.env_texture`), `AlphaMode` 5, `ClearAttractors` vs the head mesh (docs/zone/npc.md §8).
* Weapon attractor orientation: the mesh is mounted with the attractor's frame as is; no weapon-specific rotation was found.
* Name tag billboard scaling (§4); health bars / indicators over heads.
* Doors do not animate (open/close state is server driven and no door message is decoded).
