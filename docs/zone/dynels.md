# Other players, NPCs and objects in the world (`play/dynels.rs`)

Everything the zone server announces besides the own character is drawn through the renderer's actor layer. Evidence for the
individual rules lives in the RE docs this page ties together: `docs/zone/dynel.md` (wire layouts), `docs/zone/npc.md` (rdb 1040023
NPC records, texture/cloth/attractor rules), `docs/zone/motion.md` (movement FSM, animation ids, name tags),
`docs/zone/static.md` (corpses, vending machines, items, playfield-placed dynels), `docs/zone/combat-anim.md` (death sequence).

## 1. Pipeline

```
N3 frame ──Zone::on_frame──▶ Dynels::on_message ──▶ Char (Mover) / Prop      ──▶ Dynels::update_with_collision ──▶ Host::actors (ActorFrame)
                                      │ Look (appearance)                                 ▲ Host::actor_models (model scene, once per Look)
                                      ▼
                         worker thread: build(Look) → Built { model: Scene, rig: ActorRig, clips, features, held pose }
```

* `ao_scene::ActorFrame` / `Renderer::add_actor_model` / `Host::{actors, actor_models}` (`crates/ao-render/src/actors.rs`): the model's
  indices, materials and textures are uploaded once per key; the skinned body (`meshes[0]`) has one vertex buffer per actor that
  is rewritten when a new pose is supplied; head and attached weapons are rigid meshes that only get a transform. Actors are
  frustum-culled per mesh (bounding sphere + 1 m pose margin on the body). Actors that are not pushed in a frame are forgotten
  (`Renderer::set_actors`).
  `Host::actor_model_removals` queues individual model retirement through `Renderer::remove_actor_model`, releasing that key's
  model resources, actor skin buffers, frames and draw items without forgetting unrelated actors. The viewer drains removals
  after a full actor clear and before model uploads, so a same-frame upload may reuse a retired key; shared instance buffers
  are rebuilt from the remaining frames before rendering.
* **Draw order** (RE: `DisplaySystem_t::Render` @0x100793b8 = `RViewPort_t::Render(list a, list b, type, first bucket, last bucket)` @0x1004bfff
  calls; a visual is queued by `RVisual_t::AddToRenderList` @0x1004c9c2 into `list × 0x708 distance buckets`): sky/ground lists 0–2, then **lists 3 + 4** (3 = opaque
  meshes and the CAT visual `FUN_10056ed6` without transparency (`SetRenderPriority(3)`); **4 = `VisualLiquid_t`** = the water ctor @0x10067385 `SetRenderPriority(4)`), then the blended
  **lists 5 + 6** far → near (5 = blended meshes, 6 = CAT visuals with opacity < 1 / colour modifiers, particle effects), then `RenderRefraction(list 4)` (water's second look), the whole
  thing once for the far distance buckets (≥ 500) and once for the near ones; list 7 (fader) and 9/10 last. So an **opaque actor is drawn before the water surface** and
  the water blends over it (actors standing in / under water are tinted), while blended actor parts come after the water. The renderer does exactly that (`Submesh::liquid`,
  set by `playfield/water.rs`): sky → world opaque → actors opaque → liquids (lava's opaque layer first) → blended world far→near → actors blended far→near.
  Not modelled: the far/near split at bucket 500 (approximately 100 m under the native five-buckets-per-metre mapping), `RenderRefraction`'s second water pass, the CAT visual's transparent-material second pass per
  actor (`+0xbd` flag: here each submesh is classified by its blend), water drawn near→far (the client's bucket order for lists 3/4) instead of far→near.
* **Authored visual priority:** `ActorFrame.priority` and mesh-indexed `part_priorities` override the render list without changing the material pipeline. The native constructor `RVisual_t` @0x1004d544 sets priority **3**; `SetRenderPriority` @0x1004cb7e writes `+0xe4`. `Process` @0x1004cc35 rejects **-1 before** applying its opacity-below-one override to list **6**, so -1 suppresses a visual rather than drawing it first. `AddToRenderList` @0x1004c9c2 prepends to each bucket; `RViewPort_t::Render` @0x1004bfff traverses lists before distance buckets. DS @0x100796d9 renders 3/4 near-to-far, 5/6 far-to-near, then screen list 7. Evidence was obtained with Ghidra `-readOnly` against the original randy31 project.
  Native2011 flag 0x400 suppresses only the CAT root (`GC` @0x100e292b, called by @0x100e2b46), saves its previous priority at controller `+0x88`, and restores it on deletion @0x100e2bde; mounted visuals remain independently drawable. Native3034 flag 0x8000 selects priority **3** (`GC` @0x10111147 / @0x10110dc3). Native1020 screen sprite constructor (`DS` @0x10023885) selects **7** and its sampled screen pass remains after list 6 and before GUI.
  **Distance buckets:** `ActorFrame.render_bucket` preserves an explicit bucket (Native3015 nonzero mode is **100**, GC @0x100e413e); `None` uses each visual's transformed frame origin, not its culling sphere centre. Randy instructions @0x1004ca33–0x1004ca81 compute `roundTiesEven(f32(distance * 5) - 0.49999)` below **200 m**, otherwise `roundTiesEven(distance - 0.49999) + 800`, clamped to **0..1800**. Memory reads show doubles **5.0** @0x10095970, **200.0** @0x10095978 and **0.49999** @0x10094360; the assembly uses `FISTP`, not truncation. DS `GfxVisual::Process` @0x100078c8 has the same 5/200 constants @0x1008a000/0x1008a008, reads explicit bucket `+0x1a0` for sentinel screen coordinates and adds world bucket bias `+0x1a4`. Actor and viewport-depth-clear commands now share integer bucket keys; equal buckets draw in reverse insertion order, matching native prepend. World opaque batching remains unchanged.
* Actor materials: the AlphaMode of the part's texture and the **environment (sphere-map) layer** are applied per submesh (`Submesh::glow_mask` / `blend`, `Submesh::env_texture`),
  rules and addresses in docs/zone/npc.md §5; the wire `TextureData` env texture and alpha mode flow through `CharLook::textures` → `TextureOverride` → `actor::npc_part_layers` → `ActorRig::new`.
* `ao_formats::character::actor::ActorRig` builds a model from a CAT model + head + part textures + attachment meshes and
  skins it on the CPU for any clip/time (`pose`) — 600 body vertices, ≈ 5 µs per pose.
* `Look` (hash = model key) is what the app derives from a message: `Char` (SimpleCharFullUpdate: breed/sex/race/fatness, head,
  `MonsterData`, `textures[]`, page-0 `cloth[]`, attractor meshes), `Corpse`, `Item` (StaticInstance template + stats).
  Equal looks share one model: the 81 characters of the capture need 13 models.
* Cache lifetime (implementation: `ao-render/src/viewer.rs::Host::set_scene`, `play/dynels.rs::sync_scene`, `play/tags.rs::TagLayer::frame`):
  CPU models survive playfield changes, but each scene replacement advances the host generation and invalidates every model's uploaded flag
  and every tag sprite hash. This also covers a scene arriving asynchronously after a playfield notification and an intervening upload;
  reused characters, corpses, props and unchanged selection/attack/name tags are uploaded again before drawing in the new scene.
* Unweighted attachment bind frames use the same best-rest-clip selection as the character loader (`character.rs::best_rest_clip`);
  `ActorAssets` memoises successful selections by `(model id, skeleton signature)` across appearance rebuilds. Model id matters because
  the score uses that model's fitted bind frames; different models sharing a skeleton must not share the selected clip.

## 2. What is drawn and how

| dynel | look | source |
|---|---|---|
| player (`F & 1 == 0`) | `ActorRig::player`: body of breed/sex/`Fatness` (thin/normal/fat), naked skin of the `Race` (1 caucasian, 2 african, 3 asian) with the worn cloth composited over it (green key), head mesh `HeadMesh`, attractor meshes (weapons: places 1/2 = right/left hand) | docs/zone/dynel.md §1.3, formats.md § Skin |
| NPC (`F & 1`) | rdb 1040023 record `MonsterData` → `Mesh` (rdb 1010002); head/attachments = the message's attractor list only (the record's `HeadMesh` selects the skin, it mounts nothing, npc.md §6); `textures[]` replace the part textures by exact material name, `cloth[]` are composited over the part texture; `MonsterScale/100` uniform scale; clip variants rolled per clip start (npc.md §3) | docs/zone/npc.md |
| corpse (kind 0xC76A) | `CATMesh` stat model + cloth/head/skin/`textures[]`, `MonsterScale`; **pose = the unanimated CAT mesh (bind pose)**: `Corpse_t` / `VisualCATMesh_t::SetMesh` never start a clip, the 0xCF27 spell plays social keys < 100 only and the captured key is 0 (docs/zone/static.md §5) | docs/zone/static.md |
| vending machine (0xC75B) and other item-family dynels | `StaticInstance` template (rdb 1000020) stats overlaid by the message stats → `Mesh` (rdb 1010001; default `pickupbox_misc` 9013) / `CATMesh`, override texture, `Flags` bit 0 = visible | docs/zone/static.md §2 |
| doors, billboards, terminals the playfield places itself | rdb 1000026 `PlacedDynel` (`CreateRDBDynels`) → same item path (template + blob stats); position/rotation as stored | docs/zone/static.md §6 |
| held weapon items (0xC74A) | invisible (they have a parent); the weapon is the holder's attractor mesh | docs/zone/static.md §4 |

Missing MonsterData is not a missing model error: Gamecode's null-record path uses the normal humanoid resolver for breed 1..4; breed > 4 leaves the previous visual unchanged. The NPC wire bit alone does not select a morphed rig. No other-record/direct-mesh/default-creature fallback exists (npc.md §1).

Coordinates: scene = `(x, y, -z)` of the server position; a model faces -Z at rest, so the actor rotation is `scene_yaw(server_yaw)`
(play/zone.rs, verified against NPC travel directions). Remote placements and every vehicle substep run
`Vehicle_t::EnsureSurfaceAlignment` (Vehicle.dll @0x1000d1aa) through the loaded playfield collision;
unsupported walking bodies fall with retail gravity, while flying/no-fall modes retain their vertical rules
(docs/zone/motion.md §4d). Server `y` is a placement input, not a permanently trusted render height.

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

Name tags and the target indicators are the original's world-space billboards (`play/tags.rs`, details and addresses in docs/zone/motion.md §6):
`Dynels::name_tags(dt, gui, host, own_pos, indicators)` composes the 32 px sprite (name line `FontGameShell12` at y = 1, organisation line at
y = 19, plate 0xe6 / 0xe5 and the 64×4 health bar for indicators), uploads it as a four-vertex emissive alpha-tested/blended sprite and pushes one camera-facing
`ActorFrame` per tag, `(width/128) m` wide and `0.3 m` high, centred on the head anchor, so it shrinks with perspective. Filtered edges retain their alpha (strict cutoff `>30/255`, depth writes), matching `[3] TargetIndicatorMat` plus `RSprite` states; see motion.md §6 for the formerly opaque-edge root cause. Which tags exist: the selected
target's indicator (unless its `Flags` has bit 0x400), the attacked dynel's indicator (`Zone::fight_target` of the own character), and with the
pref `ShowAllNames` (retail IndependentPrefs default 1, read through the HUD DValue from `Login.cfg`; no environment override) the nametags of the characters within
30 m of the player, rebuilt every 2 s. Colours: white, green/blue by `Flags`, red for stat 345 / hostile battle-station players; the original has
no faction / con / team tint of the name (motion.md §6), the con colour only fills the health bar. There is no hover tag in the original (the
object under the mouse only picks the pointer). InPlay is retained from full-update flag bit 1, stat 0xC2 changes and CharInPlay relays;
listing requires it, and drawing requires visibility, a position and no parent (GUI 0x10024d5b).
Not modelled: ignored-character icon, org line (needs `IsOrgNameShownOverHead` and the clan string, not decoded), more than 96 tags at once.

Live regression (2026-10-06, `live_walk`, audio muted): Aomacfixr selected the Helpful Colonist in Arrival Hall (4604), walked `goto=193:157`, then entered ICC Shuttleport (4582) in the same session. After `zc=8,drag=left:314:0,wait=1`, the inspected offscreen frame showed NPC models and nameplates; `clickdyn=Surf Lizard` picked instance 1042513 at pixel (452,216), and the next frame showed its selection plate/bar. The harness passed in 69.16 s. This exercises the renderer scene replacement between the two sets of dynel/tag uploads; the cache regressions separately require every cached model and unchanged sprite to upload again on a new scene generation.

Captured screenshot helper: `captured_dynels_become_actors` now selects the screenshot camera before its final actor submission,
normalizes its culling direction and forces fresh pose payloads for the new offscreen renderer. The benchmark's Testy-centred
actor list must not be reused for a distant guard camera; test assertions require a nonempty camera-local submission.
Fix8World's isolated check (2026-10-06), `AOMAC_DYNEL_LOOK=monster:254118 AOMAC_DYNEL_SHOT=<png>` with
`captured_dynels_become_actors`, passed in 33.77 s: 18 models, 0 failed, 36 actors, 6 props.
The inspected screenshot now showed two humanoid guards in the ramp foreground (back view), not empty terrain.
This proves the camera-local submission fix, not front-view hand-colour acceptance.

## 5. Open items

* A server-sent `CorpseAnimKey` 1..99 (never captured) would start a social clip on the corpse (static.md §5).
* `Features` of players is an inference (bit 4); NPC `Features` come from their record.
* Corpse `TextureData` env texture / alpha mode are passed as 0 (the corpse path takes `(material, texture)` pairs; both are 0 in every capture). Full updates preserve the separately mounted `HeadMesh` when the attractor list omits place 0: base `CharacterMesh::ClearAttractors` deletes bookkeeping but does not remove render children. Appearance updates use `VisualCATMesh_t::ClearAttractors` and replace the rendered set, including the head (dynel.md §1.3); a flag-bit-2 full update skips the mounting block.
* Weapon attractor orientation: the mesh is mounted with the attractor's frame as is; no weapon-specific rotation was found.
* Indicators over non-character dynels (corpses, props); the ignored-character icon on the tag plate (§4).
* Doors: open / close / lock is the server's `DoorStatusUpdateIIR_t`; the mesh animation, sounds and the open items that animate (vending machines) are ported (docs/zone/doors.md). Open: dungeon room doors and door collision (docs/zone/doors.md §6).
