# The own avatar: heading, appearance, animation

Code: `crates/aomac/src/play/avatar.rs` (appearance, clips, `ActorFrame`), `play/zone.rs::{scene_forward, scene_yaw}` (heading).
Renderer side: `ao_render` actor layer (`ActorFrame`, `Host::actor_models/actors`), `ao_formats::character::actor::ActorRig`.

## 1. Heading handedness (resolved)

**Server/engine space is the client's left-handed D3D space; a model's rest forward axis is +Z; a dynel's rotation is a unit
quaternion about +Y with angle `yaw = 2*atan2(y, w)`; the character then faces `(sin yaw, 0, cos yaw)` (x, y, z).**

Evidence:

* Live data (`docs/captures/zone_ithaca.rec`, `zone_newchar_ithaca.rec`): 26 `CharDCMoveIIR_t` samples of walking NPCs/players
  (move type 1/2 = forward start/stop, displacement > 0.3 m between consecutive messages). The travel direction
  `atan2(dx, dz)` equals the quaternion's `yaw` (23 of 26 within 0.6 rad; the rest are turning NPCs whose position lags the heading, all within 0.8 rad; e.g. quaternion `(0, -0.9705, 0, 0.2410)` ->
  yaw -2.655; travel `atan2(dx, dz)` -2.655; the other candidate `atan2(dz, dx)` -2.058). The opposite handedness
  (`(-sin, cos)`) fails (`zone::tests::heading_matches_the_direction_npcs_walk`, which also asserts the mirrored hypothesis
  matches < 50 %).
* The D3D reading agrees: `D3DXMatrixRotationQuaternion` for a Y quaternion of angle `t` makes the row-vector basis of +Z
  `(sin t, 0, cos t)`; CAT models are authored facing +Z (`ao_formats::character` mirrors Z because the model space is
  left-handed D3D, `docs/formats.md` § characters).
* Scene space is the mirror `(x, y, -z)` (`zone::scene_pos`). A model in scene space faces **-Z** at rest (`aomac view
  player ... ` from `--eye 0,1,-3` shows the face, from `+3` the back). Mirroring negates rotation sense, so:
  * `zone::scene_forward(yaw) = (sin yaw, 0, -cos yaw)` (unchanged formula, now VERIFIED, [GUESS] label removed),
  * `zone::scene_yaw(server_yaw) = -server_yaw`: rotation about +Y (`Mat4::from_rotation_y`) taking the -Z-facing scene model to
    `scene_forward`,
  * `ao_render::Camera` (yaw 0 faces -Z, forward `(sin yaw, ., -cos yaw)`) therefore takes the server heading unchanged as its yaw.
* Offscreen check (`avatar::tests::avatar_poses_and_renders_in_the_arrival_hall`, `AVATAR_SHOT=out.png`): the camera placed
  3 m along `scene_forward(yaw)` of the own dynel sees the character's face and chest, standing on the floor of the Arrival
  Hall (4604); the same camera at -3 m would see its back.

Not traced to the DLL: the matrix build in `n3VisualDynel_t::SetRelPosRot` itself (the capture data and the model data fix the
convention, which is what the placement needs).

## 2. Appearance from `SimpleCharFullUpdate` (`AvatarLook::from_update`)

| Input | Use |
|---|---|
| `breed`, `sex` | `screens::wire_breed_sex` -> `Breed`/`Gender` (Atrox is always male) |
| `fatness` (stat 0x2F, 2 bits) | body model: 0 `_thin`, 1 normal, 2 `_fat` (`player_model_build`, same names as the creation screen's `CCCharacter_t::ChangeMesh`); **[GUESS]** 3 -> normal. In the client the body is the `Mesh` stat (0xC, `FUN_10057c75` reads stat 0xC; the select-screen cache stores it as `MeshID`); the wire message carries no mesh id, and the code that derives stat 0xC from breed/sex/fatness was **not found** (Gamecode grep for `.cir`/`_thin`/`_fat`/`Fatness` strings and stat 0x2F/0x3B users in 0x10077e13 gave nothing; the stat is set through the dynel's virtual `SetStat`) |
| head | place 0 overrides `head_mesh`; when place 0 is absent a full update retains the separately mounted `HeadMesh` (GC 0x10077e13 uses base `CharacterMesh::ClearAttractors`, not the visual clear that removes render children; captured Xantarr, dynel.md §1.3). An empty AppearanceUpdate list does clear the head. |
| skin race | derived from the head mesh via the creation head table (`head_table`, like `load_cached_character`); `race` stat is 1 in every capture |
| `cloth[]` | page-0 entries -> `CachedCharacter::equipment()` (`texture` over `<part>_<race>_naked.png`, green key `0x07e0` removed, unequipped parts wear the model's `*_default.png`; `ActorRig::player` -> `part_textures`). Entries with `page != 0` are ignored (the dynel stores `cloth[page*5+part]`, which page the renderer shows was not traced) |
| `attractors[]` (place != 0) | weapons/lights as rigid meshes on `Attractor<place+1>_*` bones (`ActorRig::new` attachments) |
| `monster_scale` | `SetBodyScale(stat/100)`; uniform scale in the actor transform |

## 3. Animation

Clip per `Role` (`role` -> `<set>_<clip>_01_01.ani`, `ao_formats::character::model_clips`). A role the model's set lacks falls back
(`RunBack`->`WalkBack`->`Walk`->`Idle`, sit/crawl/jump->`Idle`); no clip = bind pose.

**Loop markers** (found on every locomotion clip of the solitus male: walk `loopstart 733 / loopend 1733` of 2433 ms, run
`1166/1933` of 4000, walk-back `666/1933`, ...): a clip is *intro, loop, outro*. While the role holds, playback wraps from `loopend`
to `loopstart` (`avatar::clip_time`); clips without markers wrap over their whole length. Footstep markers `left`/`right` also exist
(for sound). Changing between locomotion clips keeps the loop phase. **[GUESS]** the outro (after `loopend`) is not played when the
role changes (the new role starts at once); jump clips (`jump-stand`/`jump-forward`, 3000 ms, no markers) play once and hold, `finished()`
reports the end; sit/crawl clips have no markers and loop whole. The per-clip loop/one-shot flags of the original were not traced.

**Playback rate (RE).** `FUN_1006fb56` (Gamecode 0x1006fb56, called with `(abstract clip id, play handle, speed, forced)` by
`FUN_1006be27` (role -> clip), `FUN_1006d3ad` and `FUN_1006fc8e`) sets the speed scale of a freshly started clip to

```
rate = calibration(mesh, clip) * (100 / MonsterScale) * speed / ref_speed
if (!forced && rate > 1.3 [_DAT_10160a8c] && speed > 4.0 [_DAT_10160a88]) rate = 1.3
if (rate > 0) SetSpeedScale(handle, rate)   // else the authored rate 1.0
```

* `mesh` = stat 0xC of the dynel (`FUN_10057c75`), `clip` = the **AbstractAnimID** passed by the Play caller, not the selected CAT/RDB id. Calibration comes from `AnimCalibrationControl_t`
  (GC constructor 0x10020f52 → 0x100019f3 → loader 0x1000182c, from `Setupf/animcalibration.txt`; lookup 0x100017f7 returns 1.0 for a missing pair). Loader 0x1000182c parses both keys with `atoi` and inserts the pair through 0x10001d03; neither loader nor lookup converts CAT ids to AbstractAnimIDs. Thus file entries `5907 10191 0.90` / `5907 10194 1.15` do **not** match the ordinary walk/run caller's 0x64 / 0x65. Both own-avatar and NPC calibration now use abstract keys, removing the previous unsupported `source_id` lookup.
  Duplicate pairs (the file has some): last wins - **[GUESS]**, insert policy not traced.
* `100 / MonsterScale` is 1.0 when the stat is 0 (`_DAT_10158670` = 100.0 double).
* `speed` = `Vehicle_t+0x3c` (maximum velocity of the movement mode, m/s; current speed is `+0xcc`, Vehicle `FUN_1000e3d3` / `SetVel` 0x1000a4e1), `ref_speed` = `Vehicle_t+0x170`, set by `FUN_1006f4a2` from the movement mode
  (`FUN_100704e6`) and sub-mode (`FUN_100704ee`): mode 3/sub 2 and mode 4 -> 3.0 (`_DAT_1015d69c`), mode 3 otherwise -> 5.0
  (`_DAT_101574fc`), mode 7 -> 7.0 (`_DAT_10160a44`), anything else -> 1.5 (`_DAT_1015d76c`), mode 5 keeps the old value (`avatar::ref_speed`).
  The same function sets the vehicle's max velocity = `ref_speed + RunSpeed * k` (k = 1/275 for mode 3/sub 1 and the default, 0.002545
  for 3/sub 2, 1/440 for 4, 1/275 for 7; caps 13 / 9.1 / 8 / 15 m/s, minimum 1.5, 1.05 for 3/sub 2); that belongs to the movement state machine
  (see the Movement notes). Which `Movement_n::Mode_e` value is walk/run/sneak is theirs to resolve.
* The clips themselves are in place (the root bone has no translation in idle/walk/run/back/strafe/jump/sit), so the
  avatar's ground speed comes entirely from the movement code, never from the clips.

API: `AvatarPose { role, speed, ref_speed }` (`AvatarPose::still(role)` for idle/sit/emote), `Avatar::set_pose/update(dt)/frame()`.

The shared CAT clock is milliseconds: DisplaySystem 0x10074bb4 → 0x10074228 → 0x10072fde advances `abs(frameSeconds * 1000 * speedScale)` (double 1000.0 at 0x1008aa78); Randy `SetTime` 0x10051cfc stores it unchanged. `ActorRig` samples the supplied time without wrapping; own-avatar and other-character playback choose loop-marker wrapping or one-shot clamping before sampling. Death samples exactly the last frame before holding its terminal pose. Other-character movement-state clips, **including idle**, call the calibration/body-scale/reference-speed formula once at clip start: `FUN_1006be27` calls holder Play at authored rate 1.0, then `FUN_1006fb56` with speed argument zero, which substitutes the vehicle maximum at `+0x3c`. A stopped vehicle does not imply a zero playback speed. The port retains this start rate until the next Play/state transition, starts at zero rather than an instance-id-derived phase, and samples every visible frame rather than invented 25/10/4-Hz distance tiers. View-distance culling no longer stops its clock. `npc_clock_keeps_clip_start_rate_and_uses_absolute_milliseconds` covers idle's maximum-speed input, retained start rate, restart and absolute millisecond advancement (not run during this change).

`retail_authored_gait_durations_use_milliseconds_and_abstract_calibration_keys` reads model 5907's CAT 10191/10194 and `Setupf/animcalibration.txt`: walk duration 2433 ms, loop 733–1733; run duration 4000 ms, loop 1166–1933. It checks the raw RDB-numbered file factors 0.90/1.15 separately from the runtime abstract-id lookup, whose factor is 1.0 for 0x64/0x65. Formula-derived reference-speed clock periods are therefore 1.000/0.767 seconds (ground distance 1.500/3.835 m per cycle), not the previously claimed source-id-calibrated values. This updated regression has not been run during this change; the periods are calculations, not a live or screenshot claim.

### Engine frame delta (window hitches)

`N3InterfaceModule_t::FrameProcessor` (Interfaces **0x10007f32**) passes
`**(Timer_t + 0x14)` directly to the engine's virtual slot `+8`.
`n3EngineClientAnarchy_t::RunEngine` (Gamecode **0x100180e6**) forwards that
argument to `n3EngineClient_t::RunEngine` (N3 **0x10007a01**), which forwards it
to `n3Engine_t::RunEngine` (N3 **0x100066ad**). The latter stores the argument
unchanged at engine **+0x68**, runs the root, and adds it to engine `+0x70`.
There is no 0.1-second clamp on this route.

The source is `Timer_t::FrameProcess` (AFCM **0x100065d3**); `GetDeltaTime`
(**0x1000657e**) simply reads the same `+0x14` pointer. Its performance-counter
path bounds negative elapsed time to zero and has a **100.0-second**, not
0.1-second, upper bound (comparison **0x100066a7–0x100066ba**, double
**0x10010b50 = 100.0**). It quantizes elapsed seconds through milliseconds
(double **0x10010b60 = 1000.0**, stores **0x10006710/15/1b**).
The integer conversion (**0x10007f86**, positive rounding correction
**0x10007fcd–0x10007fe0**) truncates toward zero, but this does **not** discard
each frame's sub-millisecond fraction: residual seconds at **0x10017088**
are added to the next elapsed interval and rewritten at **0x10006702** as
elapsed minus quantized seconds.
The fallback accumulates `DeltaTimer_t::GetDeltaTime` milliseconds and divides
the elapsed ticks by 1000 without an upper clamp; its maximum-framerate loop
waits for a minimum frame duration rather than discarding a hitch.
DeltaTimer **0x1000107f** reads system time and subtracts the previous value;
`WrapperTime_t::ReadSystemTime` (**0x1000114b**) uses `timeGetTime`,
subtraction (**0x100010bb**) is plain float subtraction, and `Normalize`
(**0x100011a2**) is a bare return.

The native monotonic window clock therefore uses `min(100.0)`, matching
the high-resolution retail route instead of the former `min(0.1)`, so normal
render hitches do not discard animation time. The fallback retail clock is
unbounded as described above; millisecond quantization is not reproduced by
this narrow window-clock fix. Movement/vehicle substeps and offscreen clocks
are unchanged. Evidence: read-only Ghidra decompilation of the named functions,
plus instruction and double-constant reads for the QPC bound.

## 4. Unresolved / labelled

* Mesh (stat 0xC) selection for players from breed/sex/fatness (see §2); `fatness == 3`.
* Which cloth page is drawn; the `AttractorMeshData` byte/`field`.
* Outro playback and the per-role loop/one-shot flags (§3).
* Matrix build in `SetRelPosRot` (conventions fixed empirically, §1).

## 5. Own-character fade (`FadeCharacter*`)

Consumer `VisualCATMesh_t::RefreshAlpha` (DisplaySystem 0x10073b7f), run every frame by `VisualCATMesh_t::RunFunction` (0x10074bb4); ported as `avatar::Fade` / `Fader`, applied through `ActorFrame::alpha` (`Player::frame`, prefs read each frame in `flow.rs` via `Fade::from_prefs`).
* Prefs (Char category, `Char.cfg`, `SetDefaultCharPrefs` GUI 0x1012447a): `FadeCharacter` (1), `FadeCharacterStartDist` (1.5, 0.1..10), `FadeCharacterEndDist` (0.7), `FadeCharacterEndAlpha` (0.15, 0..1; option label "Max transparency level"). The `VisualCATMesh_t` constructor (0x100745e8) registers fire-now changed callbacks `FUN_10072c3c/4d/5b/69` that store `DAT_1015082c` (on), `DAT_100af880` (start), `DAT_100af884` (end), `DAT_100af888 = 1 - endAlpha`.
* **Who**: only the own character. `n3VisualDynel_t::SetCatMesh` (N3 0x10019fb2) sets `VisualCATMesh_t+0x98` when `n3VisualDynel_t+0xc8` is set, which the constructor (0x10019365) fills from `n3Dynel_t::IsClientChar`; every other CAT mesh keeps 0 (the ctor's value) and gets factor 1.
* **Formula**: `d` = distance between the camera position (`VisualEnvFX+0x7e8` camera `+0x14`) and the head attractor (`Attractor01_head` translation x body scale through the CAT frame's world matrix). `S` = start, `E = min(end, start)`, `K = 1 - endAlpha`. `d² ≥ S²` → 1; `E² < d² < S²` → `(1 - K)·(d - E)/(S - E) + K`; `d² ≤ E²` → `K`. So with the defaults the character is at most 15 % transparent, fully reached at 0.7 m. (Hysteresis: the cached factor `+0x94` is replaced only when it moved by more than 0.01, `_DAT_10090a50`.)
* **Effect**: `SetAlpha` multiplies the factor into `RRefFrame_t::SetTransparency` of the CAT frame and its children (randy31 0x100453ed). `RVisual_t::Rasterize` (0x1004d84a) skips a visual at transparency ≤ 1e-5 (`_DAT_10095a08`); `RVisual_t::RenderWithTransparency` (0x1004d2d8) puts the value into the D3D material's diffuse alpha and switches ALPHABLENDENABLE (state 0x1b) on; no change to depth write or render list. Renderer: `ao-render` fade pipelines (opaque / alpha-tested submeshes blend with the frame's alpha, depth write kept; blended / additive ones multiply it into their alpha). **[INFERENCE]** the texture alpha of opaque submeshes is not used as opacity (it is a glow mask for some skins); the env-map pass (`ONE, ONE`) is not faded.
* Screenshots (`AVATAR_FADE_SHOT=<dir> cargo test --release -p aomac -- fade_screenshots`): camera 2.0 / 1.2 / 0.9 / 0.5 m from the head, fade off / default / `endAlpha` 1.0.
* Not ported: the character LOD (`RunFunction` sets `RCATMesh+0x21c` = 0 / 1 / 3 / 6 by the squared camera distance over scale², thresholds `_DAT_10090b18/14/10`, skipped when `EnvPrefs[0xe]`), no LOD meshes are selected by our renderer.

## 6. Worn weapon (live wield / unwield of the own character)

Capture `docs/captures/zone_wear_rifle_borealis.rec` (Aomacvolk, Borealis; double click on bag slot 0x41 = item 121569 "Solar-Powered Assault Rifle", `ItemClass` 1, `AnimSet` 3, `DamageType` 0x5a, `ItemDelay` 100; `WeaponMesh` 15839 = 0x3ddf). The server's answer to `MoveItemToInventory({0x68,0x41}, 6)`:

| order | wear | unwear (`MoveItemToInventory({0x68,6}, 0x41)`) |
|---|---|---|
| 1 | | `CharacterAction` 0x61, `identity_b = {0, 6}` (empty slot 6) |
| 2 | `ContainerAddItemIIR_t` (0x47537A24) | same, into the bag slot 0x41 |
| 3 | `WeaponItemFullUpdateIIR_t` (header = the item `{0xC74A, 0xd4d810}`, parent = the own char, `byte_71` 6) | same with `byte_71` = 0x41 |
| 4 | `CharacterAction` 0xa7 (empty), 0x83 (`identity_a` = item, `identity_b = {0, 6}`) | same with `{0, 0x41}` |
| 5 | **`AppearanceUpdateIIR_c`** with 2 attractors `{a 1, b 0x3ddf, c 0, d 2}` (right hand = place 1, the item's `WeaponMesh`) and `{a 0, b 0x9ee9, d 4}` (head) | the same message with the head attractor only |

The saved 14-line `.rec` contains only the generic-command acknowledgements, three weapon full updates (19373 / 25452 / 28594 ms), and three appearance updates (19475 / 25452 / 28595 ms). The 0x61 / 0x83 rows above describe the separately observed live broadcast chronology, **not bytes present in this saved capture**. Regression `combat::module::tests::rifle_capture_replication_and_confirmed_equipment_are_distinct` replays those bytes for own and identity-remapped remote actors, then checks explicit unwear and an empty-slot repeat separately.

Equipment sound flags traced: Gamecode `FUN_1006a857` calls unwield vtable `+0xa8(char, 0)`; `FUN_1006ad94` calls wield `+0xa4(char, slot, 0)`; `FUN_10047873` also calls `+0xa4(char, slot, 0)` for ItemClass stat 0x37 in `{1,14}` and slots `{6,8,0x3d,0x3f}`. `FUN_100a1216` reaches that apply only after resolving the parent dynel and casting it to `SimpleChar_t`; it disables holder animation application around the call (`FUN_1003c48b(1)` / `(0)`). `FUN_1009e301` plays item sound key 8 whenever the wielder is non-null; an already-wielded item first calls `+0xa8` with the same flag. `FUN_1009ce50` plays key 9 only while the item is wielded, with a non-null wielder. The flags gate item spell work, **not these sounds**; no login-only sound suppression exists at these traced entry points. Full updates therefore queue the retail attach sound without synthesizing a gesture or counting an explicit action; appearance updates never replay it.

`Module::take_equipment_sounds` returns `(actor, candidate sound ids)` for the existing positional sound path and shared CRT random selector; the key-9 list is retained before Armory removes the slot. Explicit 0x61 / 0x83 confirmations are separately drained by `take_equipment_actions`, even for soundless items. Gestures reuse `take_anims`; an occupied unwear out of combat queues 0x6d, and a bag-slot 0x83 retains that gesture without duplicating the same queued clip. Repeated sound ids rely on the existing retail `PlaySample` still-playing gate, not suppression of server messages.


* The mesh in the hand is **the `AppearanceUpdate` attractor list**, not the weapon dynel (`FUN_100a1216` disables its visibility while it has a parent): `AppearanceUpdateIIR_c::Activate` [GC 0x10071679] = cloth writes through `FUN_100480fc`, then `VisualCATMesh_t::ClearAttractors` + `CharacterMesh::AddAttractors(list)`. `Zone` emits `OwnEvent::Appearance` and updates `own_update` cloth by page/part, attractors, visual flags and mode for a rebuilt `Player`. `Player::apply_own_events` -> `Avatar::set_appearance` applies page-0 cloth deltas (unnamed parts remain, texture 0 clears), replaces the head/mount set, and rebuilds `ActorRig::player` only when equipment, head or mounts differ. This reuses the full update's skin/cloth compositor and reuploads `MODEL_KEY` without restarting the current clip. Regression `captured_appearance_cloth_reaches_live_avatar_and_rebuild_snapshot` uses the wear capture's body-cloth clear and checks the live rig and cached full-update look agree.
* Wield state of the own character is the same as for every character: `Dynels::wield` (`WeaponItemFullUpdate` of the own dynel, slot 6 / 8; swing lists, `AnimSet` / `ItemDelay`) and `Armory` (`DamageType` 0x5a projectile for the rifle; bare hands = the martial-arts item, melee, slot 0, again after the unwear). The **unwear is `CharacterAction` 0x61**, not a `ToClientQuit` (the weapon dynel stays in the bag): `Dynels::unwield_slot` / `Armory::unwield_slot`. The weapon update with the bag slot 0x41 is not a hand and is ignored.
* Stance (docs/zone/combat-anim.md §4): while `Dynels::wielded_set(own)` is a weapon, `Avatar::set_stance` makes the idle out of a fight the equip routine's idle (rifle `idle-2h` 0x41e, bazooka 0x424, every other weapon the plain `idle-stand`), the fight idle (`IdleCombat`, `Player::fighting`) the weapon's list 0x10 (`idle-rifle`), and walk / run the rifle's constants 0x421 / 0x422 (`combat::anim::{peace_idle, fight_idle, wield_walk_run}`), each when the model's set has the clip (else the plain role); `Dynels::update_with_collision` draws other wielders the same way. Swings: `combat/glue.rs::swing` -> `Dynels::pick_swing(own)` -> rifle `0x3ff` sped up for `ItemDelay` 100.
* **Draw / holster / wield gesture** (traced: combat-anim.md §4): the wear itself plays nothing visible out of a fight (`FUN_1003c930` is replaced within the frame by the idle it starts; [INFERENCE]); the **unwear** plays the wield gesture 0x6d (`wield`) once (`Module::on_frame` 0x61 -> `Module::take_anims` -> `Player::play(Role::Clip("wield"))`); a **fight start** plays the weapon's list-0x1a clip (rifle `rifle-start` 0x3fc) once and then the fight idle, a **fight stop** the list-0x1b clip (`rifle-stop` 0x3fe) once and then the plain idle (`combat/glue.rs::stance`, `FightStarted` / `FightStopped`). Same for every other character (`Dynels::play_once`).
* **Not done / unresolved**: the bare-hand draw / holster (martial-arts item lists, record layout not decoded); selection of cloth pages other than page 0.
* **Other characters** (`Dynels::on_message`, `CharLook::apply_appearance`): `AppearanceUpdateIIR_c::Activate` [GC 0x10071679] is not specific to the own dynel: `FUN_10058e36` only tests the identity kind 50000 (`SimpleChar_t`, players and NPCs alike, no NPC branch), then `FUN_100480fc` (cloth: entry `(page*5 + part)`, `{b = texture, c = page}` of the wire `ClothData`; written only when the texture changed, a texture 0 clears the part, parts not named stay), stat 0x2A1 `VisualFlags`, `FUN_100572b9(extra)`, and `VisualCATMesh_t::ClearAttractors` [DS 0x10073d8a] (walks the same list at `this+4` as `CharacterMesh::ClearAttractors` [DS 0x10071dd0], deletes every node, head included) + `CharacterMesh::AddAttractors(wire list)`. So the mounted set is exactly the update's list, also when the full update carried the `SET_DYNEL_800` skip flag. `Dynels` edits the character's `CharLook` (cloth page 0 by part, sorted; attractors replaced; `visual_flags`) and sends the changed look through the worker's `Req::Model`; the character keeps drawing its old model (`Char::next`) until the new one is ready. Evidence: in `zone_ithaca.rec` the player 33513 arrives with no attractors and receives his head (place 0, mesh 223820) only through this update; the NPC 1026282's full update and its update both have an empty list; the wear capture's lists retargeted to another player (`other_player_wields_and_unwields_live`: rifle `{1, 0x3ddf}` + head, then head only, the player's earlier back item `{5, ..}` is dropped by the wholesale replace) rebuild his model each time.
