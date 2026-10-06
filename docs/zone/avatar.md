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
| head | the attractor entry of place 0 wins over `head_mesh` (as `CCCharacter_t::ChangeHead` / `CachedCharacter::head_mesh`; GC 0x10077e13 calls `AddAttractorMesh(0, HeadMesh, 4, 0)` then `ClearAttractors/AddAttractors(list)`) |
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

* `mesh` = stat 0xC of the dynel (`FUN_10057c75`), `clip` = the clip id; `calibration` comes from `AnimCalibrationControl_t`
  (constructed by `n3EngineClientAnarchy_t` ctor, GC 0x10020f52, from `Setupf/animcalibration.txt`; lookup `FUN_100017f7` returns 1.0 for a missing
  pair). The file (`mesh anim factor`, e.g. `5900 9382 1.10` = athrox male sneakcool) is loaded by `avatar::Calibration`.
  Duplicate pairs (the file has some): last wins - **[GUESS]**, insert policy not traced.
* `100 / MonsterScale` is 1.0 when the stat is 0 (`_DAT_10158670` = 100.0 double).
* `speed` = `Vehicle_t+0x3c` (current ground speed, m/s), `ref_speed` = `Vehicle_t+0x170`, set by `FUN_1006f4a2` from the movement mode
  (`FUN_100704e6`) and sub-mode (`FUN_100704ee`): mode 3/sub 2 and mode 4 -> 3.0 (`_DAT_1015d69c`), mode 3 otherwise -> 5.0
  (`_DAT_101574fc`), mode 7 -> 7.0 (`_DAT_10160a44`), anything else -> 1.5 (`_DAT_1015d76c`), mode 5 keeps the old value (`avatar::ref_speed`).
  The same function sets the vehicle's max velocity = `ref_speed + RunSpeed * k` (k = 1/275 for mode 3/sub 1 and the default, 0.002545
  for 3/sub 2, 1/440 for 4, 1/275 for 7; caps 13 / 9.1 / 8 / 15 m/s, minimum 1.5, 1.05 for 3/sub 2); that belongs to the movement state machine
  (see the Movement notes). Which `Movement_n::Mode_e` value is walk/run/sneak is theirs to resolve.
* The clips themselves are in place (the root bone has no translation in idle/walk/run/back/strafe/jump/sit), so the
  avatar's ground speed comes entirely from the movement code, never from the clips.

API: `AvatarPose { role, speed, ref_speed }` (`AvatarPose::still(role)` for idle/sit/emote), `Avatar::set_pose/update(dt)/frame()`.

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
