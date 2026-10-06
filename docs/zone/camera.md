# Zone: controls, mouse-look and the third-person camera

Code: `crates/aomac/src/play/controls.rs` (`Controls`, key table, mouse state machine) and `play/camera.rs` (`Camera3p`).
Addresses are VAs of the 32-bit DLLs (`[GUI]` GUI.dll, `[GC]` Gamecode.dll, `[N3]` N3.dll). Data: `cd_image/gui/Default/{CharPrefs,LoginPrefs}.xml`.
Tags: **[RE]** read from code/data, **[INFERENCE]** conclusion from structure, **[UNRESOLVED]** not found (what was tried is listed).

## 1. Default key bindings (data, not invented)

`CharPrefs.xml` holds the default `KeyBindings` archive (`Version` 7): `Bind[] = { Input: i32, Provider: i64 }`. Loader:
`FUN_10018bbf` [GUI 0x10018bbf] (reads `"KeyBindings"`, `"Bind"`, `"Input"`, `"Provider"`).

* `Provider` = hash of the provider name registered by `FlowControlModule_t::FlowControlModule_t` [GUI 0x1002abd6]
  (e.g. `"MOVEMENT_FORWARD_V3"`): boost `hash_combine` over the signed chars, `h ^= (h<<6) + (h>>2) + c + 0x9e3779b9`,
  start 0 (`FUN_1001900a` [GUI 0x1001900a]). All 18 movement/camera providers reproduce the stored numbers exactly
  (`provider_hash`, tested against the file).
* `Input` = the client key id (table of 127 `{name*, id}` pairs at [GUI 0x10262e20]: 1..13 mouse buttons/wheel, 14 ESC, 15 TAB,
  17..22 shift/ctrl/alt, 23 SPACE, 24 ENTER, 25 BACKSPACE, 26..31 INS DEL HOME END PGUP PGDN, 32..35 CursorLeft/Right/Up/Down,
  39..62 F1..F24, 65..69 NumLock ÷ × − +, 70..79 NUMPAD_0..9, 82..107 A..Z, 108..116 1..9, 117 0, 118 SLASH) plus modifier bits
  `SHIFT 0x20000`, `CTRL 0x40000`, `ALT 0x80000` (e.g. `131118` = Shift+F8 = `CAMERA_PREVVIEW`, `262190` = Ctrl+F8 = `CAMERA_NEXTVIEW`).
  `ao_render::KeyCode` → id: `controls::key_id`.

| input (key) | provider | slot | `MovementChanged` action (press / release) |
|---|---|---|---|
| 104 (W), 9 (middle mouse button) | `MOVEMENT_FORWARD_V3` | `SlotMovementForward` [GUI 0x10027ec1] | 1 / 2 |
| 100 (S) | `MOVEMENT_BACK_V2` | `SlotMovementBack` [0x10027f12] | 3 / 4 |
| 82 (A) | `MOVEMENT_LEFT_V2` | `SlotMovementLeft` [0x10027f65] (turn) | 0xC / 0xE |
| 85 (D) | `MOVEMENT_RIGHT_V2` | `SlotMovementRight` [0x10027f91] (turn) | 9 / 0xB |
| 107 (Z) | `MOVEMENT_STRAFELEFT` | `SlotMovementStrafeLeft` [0x10027fd1] | 7 / 8 |
| 84 (C) | `MOVEMENT_STRAFERIGHT` | `SlotMovementStrafeRight` [0x10028024] | 5 / 6 |
| 23 (Space) | `MOVEMENT_JUMP` | `SlotMovementJump` [0x10028077] | 0xF / – |
| 70 (Numpad 0) | `MOVEMENT_AUTORUN` | `SlotMovementAutoRun` [0x10027fbd] = `SlotMovementForward(false)` | 1 / – (no stop: runs until a forward key) |
| 25 (Backspace) | `MOVEMENT_TOGGLEWALK` | `SlotMovementWalkToggle` [0x1002809d] | `N3Msg_PerformSpecialAction(0x12 - (GetLastSpeedMode() != 2))` |
| 34/35/32/33 (↑ ↓ ← →) | `MOVEMENT_{FORWARD,BACK,LEFT,RIGHT}_GLOBAL_V2` | `SlotMovementGlobal*` [0x100289aa, 0x100289e7, 0x10028a24, 0x10028a61] → the slots above | 1/2, 3/4, 0xC/0xE, 9/0xB |
| 46 (F8) | `CAMERA_TOGGLE3RD` | `SlotToggleCameraView` [0x10027c9f]: toggles pref `3rdPersonCamera` | – |
| Shift+F8 / Ctrl+F8 | `CAMERA_PREVVIEW` / `CAMERA_NEXTVIEW` | `SlotPrev/NextCameraView` [0x10027d0b, 0x10027cf0] → `N3Msg_Prev/NextCameraView` [GC 0x10015fd7 / 0x10015fc9] → `n3Camera_t::GetPreviousVisibleAttractor` [N3 0x10020faa] / `GetNextVisibleAttractor` [0x10021987] (§7) | – |
| 50 (F12) | `CAMERA_SCREENSHOT` | screenshot | – |
| 99 (R) | `ACTION_PICKUPITEM` | `SlotPickupItem` [0x100280d1] → `N3Msg_GetItem(object under mouse)` | – |

There is **no default key for sit** (and none for crawl): `N3Msg_SitToggle` is reached by the nano/special-action buttons and the
`/sit`-style chat commands, not by a key binding [RE: no provider named for it among the 18; the slot list of the ctor has no sit slot].
Run/walk toggle is Backspace (not a "run" key); the default state is run.

**Polarity (resolves outgoing.md §5.1).** Every slot is `Slot*(bool released)`: the key *press* calls it with `false`
(`SlotMovementJump`/`AutoRun`/`WalkToggle`/`PickupItem`/`*CameraView` act only on `!released`), the key release with `true`;
`Forward(b)` sends `b + 1`, i.e. **press = 1, release = 2**; likewise back 3/4, strafe-right 5/6, strafe-left 7/8, turn-right
`9 + 2b` (9/0xB), turn-left `0xC + 2b` (0xC/0xE). The unused 0xA/0xD are the mouse-turn pair (§4).

The forward/back/strafe slots first call `N3Msg_GetMovementMode()`; mode 8 (sitting) sends the AFCM message `0xE3`
(`MovementWhileSitting`, the GUI stands the character up) before the action. `Controls` cannot know the mode: the lead
must forward that check to the movement module.

**`*_GLOBAL_*` (arrow keys).** `FUN_10027e46` [GUI 0x10027e46]: if the input mode `TextInputMode` (3) is off → active; if on, a
cursor key moves only when the focused text view is not a single-line `TextRenderer_c` flagged `0x8000` or Ctrl/Alt is held
(`GetQualifiers & 0x30`); every other key only when Ctrl/Alt is held. Also requires `*param_6 == 1` (initial press, so no key
repeat). Implemented as: while `set_text_input(true)` only the arrows with Ctrl or Alt act; the others (WASD…) never do.
[INFERENCE] The global bindings are matched with Ctrl/Alt held (they test the modifiers themselves); other bindings need an exact
`id | modifiers` match.

**Fixed (non-rebindable) camera keys** (the compiled key table inside GUI.dll, text block at [GUI 0x101aab18..0x101adcd8]: lines `COMMAND_X [ & mode ] = KEY`; commands in `Setupf/commands.txt`):
Numpad 4/6/2/8 = rotate camera left/right/up/down (press and release both bound, `FastRepeatMode`), Numpad +/− = zoom in/out
(held), Numpad 7 = `CAMERA_SET_PREFERRED_POS`, Numpad 5 = `RESET_CAMERA`. Laptop keyboards (no numpad) cannot reach these nor
auto-run; `Controls::from_char_prefs` takes a user's saved bindings.

Other fixed keys seen in the same table (handled elsewhere): Enter/Shift+Enter/Alt+Enter = chat input, `/` and Numpad ÷ = chat
command input, PgUp/PgDn = chat history, Shift+R = reply, Tab / Shift+Tab / Ctrl+Tab / Ctrl+Shift+Tab = next/previous
hostile/friendly target, Shift+| = control center, Shift+P perks, Shift+V vehicles, Shift+C chat config, Esc, Ctrl+F10 camp.

## 2. Options (`LoginPrefs.xml` / `IndependentPrefs`)

| pref | default | meaning | source |
|---|---|---|---|
| `MouseTurnSensitivity` | 10 (1..20) | look speed multiplier | LoginPrefs; [GUI 0x1002c17b] |
| `ZoomSpeed` | 20 (1..40) | wheel notch = `ZoomSpeed/10` m, zoom keys = `ZoomSpeed` m/s | LoginPrefs; [N3 0x100200ef], `FUN_1002118c` |
| `LMBMouseLook` | true | left drag orbits the camera | [GUI 0x1002c2ee] |
| `RMBMouseLook1st` / `3rd` | true / true | right drag also pitches the camera (first / third person) | [GC 0x1001964b] |
| `ZoomTo1stPerson` | true | zooming past the limit switches first/third person | [N3 0x10020866, 0x100200ef] |
| `MouseWheel` | 0 (0 zoom, 1 scroll sidebar, 2 none) | | Options panel |
| `MouseLookInverted` | 0 | `n3Camera_t +0x230 = ±1` via callback `FUN_100200c3` (inverted → −1); `s_nInverted` in `N3Msg_MouseMovement` | `SetDefaultLoginPrefs` [GUI 0x10124b33] |
| `3rdPersonCamera` | 1 | 0 = first person; callback `FUN_100219d5` [N3] swaps the camera vehicle | |
| `PreferredCameraMode` | 3 (0..3) | 3 = `CameraVehicleFixedThird_t` (default), 1 = steering chase camera `CameraVehicle_t`, 2 = fixed variant; 0 = first person; zone 0x380 forces 1 [N3 0x10007aa4] | |
| `PreferredCamPosX/Y/Z` | (0, 0.316, −0.948) | unit direction from the look target to the camera in avatar space | LoginPrefs defaults (floats at GUI 0x101c54a0/0x101c5488) |
| `PreferredCamDist` | 5.0 (max 347) | distance | GUI 0x101a8b98 |
| `ShowMyCharacter` | false | draw the own avatar in first person | LoginPrefs |
| `UseNoBobCamera` | 0 | | |

`ControlPrefs::default()` equals this table; a test parses the installed `LoginPrefs.xml` and compares.

## 3. Third-person camera (mode 3, `CameraVehicleFixedThird_t`)

* Created by `n3EngineClient_t::CreateCamera` [N3 0x10007842] → `n3Camera_t` ctor `FUN_10021a76` [N3 0x10021a76] → vehicle by
  `PreferredCameraMode`; `3rdPersonCamera == 0` starts in first person.
* **Look target** (`FUN_10020af1` [N3 0x10020af1], `FUN_10020bdb`): world position of the CAT attractor `Attractor31_camera` of the own
  model, falling back to `AttractorMesh::GetName(0)` = `Attractor01_head`, times the body scale. `Attractor31_camera` exists only on
  vehicle/mech models (28 occurrences in rdb 1010002, all on `Bone Gun Base` / `Bone cocpit` models; none on the player models, e.g. 5907, 5927, 5900) so players use the head attractor.
  **Height [RE, resolved].** `FUN_10020af1` takes the translation of `VisualCATMesh_t::GetAttractorMatrix("Attractor31_camera")`, else of
  `Attractor01_head`, of the *animated* skeleton and multiplies it by `GetBodyScale`; `FUN_10020bdb` (every frame, also
  `UpdateTargetEye` [N3 0x10021187]) then blends the stored height towards it: third person keeps 0.8 of the old value and takes 0.2
  (`_DAT_1003d9c4`, `_DAT_1003ce50`), first person takes 0.999 (`_DAT_1003e2a4`; the camera is on the head and bobs with it). Horizontal x/z come from the
  first sample (`+0x234/+0x23c`) and are not followed [INFERENCE: ≈0, not measured: `Camera3p` uses the height only]. The result is
  clamped to ≥ 0.3 m (`_DAT_1003e29c`); a model without either attractor leaves the target at the feet + 0.3.
  Implemented: `ActorRig::head_attractor(clip)` (cat-frame translation of `Attractor01_head` in the playing clip) →
  `Avatar::head_height()` (× body scale) → `Camera3p::set_head`, blended by `follow_head`. Solitus male (rdb 5900 family): **1.793 m idle**
  (body 1.871 m), bobbing ±1.5 cm while running; the old stand-in `DEFAULT_PIVOT_HEIGHT` 1.5 m is gone.
  With `UseNoBobCamera` (`+0x1e0`, default 0, not implemented) the target follows only beyond 0.01 m of movement (`_DAT_1003e2b4`, 0.25 m hysteresis `_DAT_1003e2b0`).
* **Position**: `target + direction · distance` with `direction` in the avatar frame (`RecalcOptimalPos` [N3 0x1001f371]); defaults
  `(0, 0.316, −0.948)` × 5.0 m: 5 m behind and 1.58 m above the target, elevation 18.4°. The camera looks at the target, is rigid
  (`SetRelPosRot` in `DecideSnap` [N3 0x1001f537], flag `+0x214`) and follows the avatar heading because the offset is avatar-relative.
  **[INFERENCE]** the ctor flag `+0x214` is true for the player (rigid snap). **[RE, resolved]** `FUN_10020290` [N3 0x10020290] builds the vehicle from `PreferredCameraMode`: 0 first person, 1 plain `CameraVehicle_t` (`FUN_1001f9c9`), 2 `CameraVehicleFixedThird_t(false)` (damped: `CalcSteering` = `SteeringCamArrive(optimal pos, 0.01)`), 3 `CameraVehicleFixedThird_t(true)` (rigid `DecideSnap`, default). Modes 1/2 are §7.
* **Occlusion** (`RecalcOptimalPos`): `LineOfSight(target, wanted + dir·radius)`; if blocked, bisect the fraction in `[0.01, 0.95]`
  (`_DAT_1003d618`, `_DAT_1003e228`) up to 20 times until the bracket is ≤ 0.001 (`_DAT_1003e220`); the camera sits at the last clear
  fraction. The camera dynel has a collision sphere of radius 0.35 (`_DAT_1003ce4c` in `CreateCamera`) → `COLLISION_RADIUS`, the margin
  tested beyond the camera. `Camera3p::update_with(.., clear)` takes the line-of-sight test (scene raycast).
* **Zoom**: wheel (`n3Camera_t::` handler [N3 0x100200ef]): `pending += ZoomSpeed/10 · notches` (a notch is `raw/120` [GUI 0x1001ae14]); a
  notch against the pending direction first clears it. Per frame (`FUN_10022345` [N3 0x10022345]): `|pending| ≥ 1` → step = `dt · pending · 3`,
  else `dt · 3` signed; distance −= step, pending −= step; pending is dropped when its sign flips or it is below 0.3 m. Distance clamps to
  **0.78 … 25 m** (`_DAT_1003e48c`, `_DAT_1003e488`). Zooming in at < 0.8 m (`_DAT_1003e038`/`_DAT_1003e2f8`) with `ZoomTo1stPerson` sets
  `3rdPersonCamera = 0` (first person); a wheel notch outwards in first person sets it to 1 (third person again, at the previous offset).
  Numpad +/− (`StartZoomIn` 0x100 / `StartZoomOut` 0x80 flags): `±dt · ZoomSpeed` m per frame-time through `FUN_1002118c` [N3 0x1002118c].
* **Orbit** (`FUN_1002118c`): the camera offset is rotated about the vertical by `dx` and about the camera's right axis by `dy` (radians);
  a pitch that would make `|dot(dir, up)| > 0.9999` (`_DAT_1003e300`) is refused. `Camera3p`: `yaw_off += dx` (view turns right),
  `elev += dy` (camera rises), clamped at asin(0.9999). The sign convention is fixed by the numpad: Numpad 4 (`ROTATE_LEFT`, flag 2) gives
  `dx = +0.02 · MouseTurnSensitivity` per frame (camera swings left), Numpad 8 (`ROTATE_DOWN`, flag 0x40) `dy = +0.02 · s` (camera up);
  these per-frame amounts are frame-rate dependent in the client, `Camera3p` applies them as `rate · dt · 60`.
* **Reset / preferred**: Numpad 5 (`N3Msg_ResetCamera` sets flag 0x2000 → `FUN_10020e0c`) returns to the stored preferred direction and
  distance; Numpad 7 (`N3Msg_CameraSetDefaultPos` → `ForceUpdatePrefDir`) stores the current ones into `PreferredCamPos*`/`PreferredCamDist`
  (the prefs persist across sessions: `CameraVehicleFixedThird_t::SetPrefs` [N3 0x1001f004]; not persisted by `Camera3p`).
* **Field of view**: `SetViewPlaneWindow(π/2, aspect)` (`FUN_1002107a`, `_DAT_1003ccf8` = 1.5708): **90° horizontal** (58.7° vertical at 16:9);
  `camera::vertical_fov(aspect)`.
  **Near / far [RE, resolved].** `n3EngineClient_t::CreateCamera` [N3 0x10007842] calls the `n3Camera_t` ctor `FUN_10021a76` with
  (`fov` π/2 `_DAT_1003ccf8`, `aspect` = pref `AspectRation`, **near 0.2** `_DAT_1003ce50`, **far 200** `_DAT_1003ce54`, first-person flag); the ctor
  passes them to `VisualCamera_t::VisualCamera_t(fov, aspect, near, far)` (stored at `+0x150/+0x154/+0x168/+0x16c`). The `ViewDistance`
  callback (`FUN_1001fc91`, registered with the fire-now flag) replaces far at once by `max(ViewDistance·1000, near + 50)` (pref default 0.8 →
  800 m) and calls `VisualFog_t::AddClipPlanes(near, far)`; the camera is rebuilt with the old pose. `camera::NEAR`, `far_plane`,
  `camera::lens(base)`; `Player::frame` sets the lens once (near 0.2; far stays the playfield lens' 800 m, which is that formula). The fog
  start `environment::NEAR` (0.5, docs/formats.md) is the fog agent's and is untouched.
* **Free camera**: the original has none for players. There are GM-only debug cameras (`COMMAND_DEBUG_TOGGLE_CAMERAMODE` = Ctrl+Alt+C, `DEBUG_CAM_*`,
  `[GMLevel1Mode]`) and `COMMAND_TOGGLE_FLYING_MODE_DEBUG` = F7 (GM level). Free-fly in play mode is therefore a debug feature only.
* **Scripted views and Ctrl/Shift+F8**: §7.

## 4. First person (mode 0, `CameraVehicleFirstPerson_t`)

* Eye = look target. Mouse: `MouseCameraControl` [N3 0x10021689]: `heading += 1.0 · dx`, `pitch += (±1) · dy`, pitch clamped to
  ±1.5533 rad (±89°, `_DAT_1003e320`); positive pitch looks down. `SetRotAngles(pitch, heading)` [N3 0x1001ef0d].
* `N3Msg_EndCameraMouseLook` [GC 0x10019597]: in first person `EndMouseCameraControl` [N3 0x1002094f] resets the view heading to 0 (pitch kept)
  and the character gets `MovementChanged(0x16, fmod(heading, 2π))`, i.e. the avatar turns to where the camera looked
  (`Camera3p::apply(EndLook)` returns that angle). It then sends the strafe stops 8 and 6 if the movement state is a strafe.
* The own model is drawn in first person only with `ShowMyCharacter` (default off).

## 5. Mouse (`ActionViewMouseHandler_c`, ctor `FUN_1002c66b` [GUI])

* Raw movement is divided by **1000** per axis (`InputConfig_t::FrameProcess` [GUI 0x1001ae14], input id 0x78) and the wheel by **120**
  (id 0xD); the cursor is re-centred every move (`WindowController_c::SetMousePosition`).
* Press (`FUN_1002c2ee`): left (`LMBMouseLook`) sets pending mode 1 (camera look), right sets pending mode 2 (character look). The first
  movement activates it (`FUN_1002c09b`, accumulator = 0).
* Movement (`FUN_1002c17b`): the length of the delta accumulates; delta × `MouseTurnSensitivity` is sent to
  mode 1: `N3Msg_CameraMouseLookMovement(dx, dy)` [GC 0x10015f91] (orbit), mode 2: `N3Msg_MouseMovement(dx, dy)` [GC 0x1001964b]. So 100 counts at
  sensitivity 10 = 1 rad. With the right button and **Ctrl held** the look is mode 1 (camera only); releasing Ctrl returns to mode 2.
* `N3Msg_MouseMovement`: character turn by `dx` (action 0x2B → sent as a sync `0x16` with a rotation, never as 0x2B); pitch `dy`
  (inverted by `s_nInverted`); the camera pitches by `dy` only if `RMBMouseLook1st/3rd` (`MouseCameraControl(0, dy)`). If the character is
  turning with the keys when the first movement arrives (movement state 3/4), the turn is stopped (0xE / 0xB) and replaced by a strafe (7 / 5).
* While a look is running (`DAT_102e2588`, set by both look messages, cleared by `End*MouseLook`) `N3Msg_MovementChanged` [GC 0x10018b5c]
  remaps turn-left 0xC→7, turn-right 9→5 and their stops 0xE→8, 0xB→6 (A/D strafe during a right drag). `Controls` emits the final actions
  and, **deliberately unlike the client**, remembers which action each key started and sends the matching stop (the client would send the strafe
  stop for a turn that began before an LMB drag).
* Release (`FUN_1002c469`, `FUN_1002c14d`/`FUN_1002c0e5`): if no look was active or the accumulated movement is ≤ **0.02** (`_DAT_101aeaf4`) it
  is a click (left: select the object under the cursor — `N3Msg_GetNextTarget`/`SwitchTarget`; right: `N3Msg_DefaultActionOnDynel`; double click
  with `DoubleclickAction`); then the look ends (`N3Msg_EndCameraMouseLook` / `N3Msg_EndMouseLook`).
* Middle button = input 9 = forward while held (binding table).
* **Both buttons = forward: not in the client [RE, negative].** Every layer maps buttons 1:1: `AnarchyOnline.exe` `FUN_00404a66` (window proc;
  `0x201/0x203/0x202` → 1/3/4, `0x204/0x206/0x205` → 5/7/8, `0x207/0x209/0x208` → 9/11/12, `InputConfig_t::AddUserInput`), `InputConfig_t::
  ProcessInput` [GUI 0x1001a31a] fires a hotkey when all keys of *its* list are down but the `KeyBindings` loader (`FUN_10018bbf`) stores one input per
  bind and the default archive has no chord, the fixed key table has none, `ActionViewMouseHandler_c` (§5) keeps one pending look, and
  `N3Msg_MouseMovement`/`MovementChanged` [GC] read no button state (`GetAsyncKeyState` is not imported by any game DLL). `Controls` therefore does not
  move on both buttons (test `both_buttons_do_not_move_forward`); holding both starts the two looks as the client does. The wheel/middle button is the mouse forward.

## 6. Not resolved

* `UseNoBobCamera` smoothing, the horizontal part of the head attractor (x/z), `CameraVehicle_t` sensor steering (`UpdateSensors`,
  `CalculateSensorSteerDir`, `FUN_1002046f`), the lateral avoidance branch of `SteeringCamArrive` and the second chase branch of
  `CameraVehicle_t::CalcSteering` (flags `+0x1cc/+0x1cd`, never found set), `Vehicle_t`'s integration substep (`+0x104`).
* The movement-state values 3/4/7/8 used by `N3Msg_MouseMovement` (3 = turning left, 4 = turning right by the keys, 7 = jump, 8 = sitting; from the branch
  structure), `N3Msg_EndMouseLook`, `FUN_1006ed98`/`FUN_10070506` (movement controller state getters, Gamecode).

## 7. Camera vehicles and scripted views (Ctrl+F8 / Shift+F8)

**Ctrl+F8 is not "next attractor".** `n3Camera_t::GetNextVisibleAttractor` [N3 0x10021987] = `CameraVehicle_t::SetCameraAttractor(0)` (drop the
attractor), `+0x204 = 0` (drop the pending wheel zoom) and, unless first person (`PreferredCameraMode` 0): `PreferredCameraMode` **1 → 3, 3 → 2,
2 → 1** (`FUN_10020032` stores the pref, `FUN_10021859` swaps the vehicle). `CamCmd::NextView` does exactly that (`Camera3p::mode`).

**Shift+F8** (`GetPreviousVisibleAttractor` [N3 0x10020faa]) steps `+0x21c` back through the list at `+0x20c` (12-byte `{zone, index, score}`;
index 0 wraps to the last) and calls `CameraVehicle_t::SetCameraAttractor(list[i])`; an empty list clears it. **The attractor only steers
mode 1**: `CameraVehicle_t::CalcSteering` [N3 0x1001e797] branches on `+0x1a0` (`attractor->vtbl[1]` = `FUN_10023ab1` = `SteeringArrive(attractor.pos, 0.1)`), while
`CameraVehicleFixedThird_t::CalcSteering` [0x1001f752] (modes 2, 3) never reads it. The camera keeps looking at the look target (`SetEyeTargetLocalPos`).
When the index is -1 (after a list change) the code would index at -2: `Views::prev` steps from "none" like from index 0 [INFERENCE].

**The list** (`FUN_100220bb`, rebuilt every 10th frame by `FUN_10022345` unless `+0x228`): the attractors of the character's zone and its
neighbours (`CellSpaceBase_t::GenerateNeighborList(zone, out, 1)`, a virtual of the playfield's cell space; implemented as the 8-neighbourhood / door-connected
rooms [INFERENCE], `CameraViews::neighbours`) whose score (`PointCameraAttractor_t` vtbl+0xc `FUN_10023c8a`, from the character's feet + 1 m) is below 100000, sorted ascending.
Score = distance; 10000 when < 0.5 m horizontally away; + 1000 per metre the authored target is beyond `range`; + 100 when the attractor lies in the direction of the
camera's current spot (dot > 0.9); + 50 when that spot is in the clear and the character lies beyond it; + 10000 below y = 0.1. Not a candidate: disabled
(`range ≥ 2.0` sets byte `+0x30`, `FUN_10023bbb`), line of sight attractor → character blocked (`FUN_10023bfc`; its closed-door room test is not applied: doors count as open).
With no index (-1) and none selected for `+0x224` = 1.2 s since the last automatic pick (`+0x220` starts at 20 s) `FUN_10021921` selects entry 0 on its own. `Views::tick`, `Views::prev`.

**Data.** Playfield record (rdb 1000001), per zone / room: `u32 n` (< 1000), n × { vec3 pos, quat rot, vec3 target (version ≥ 6, else = pos), f32 range } =
`PointCameraAttractor_t` (`+4`, `+0x10`, `+0x20`, `+0x2c`; `RDBPlayfield_t::ReadBlob` [N3 0x1001c115], `n3Room_t` reader 0x10012803), world coordinates.
`ao_formats::playfield::{camera_views, CameraViews, CameraAttractor}`. Whole game: **208 attractors in 53 playfields, 6 usable (`range < 2`)**: playfield 322 (3) and 1892 (3),
pinned by a real-data test; so in retail the scripted views exist in two zones, and only in mode 1.

**Vehicle model** (modes 1, 2; `Vehicle_t::SteeringArrive` Vehicle.dll 0x1000ab28, integrator `FUN_1000e3d3`, `UpdateMotionConstraints` [N3 0x1001e602]): mass 20, top speed
16 (= `min(16, 6 · avatar speed)`, ≥ 2), force limit `mass · v / 0.3`, brake distance `0.3 · v`; desired velocity = towards the target at `min(v, distance / brake · v)`,
steering force `(desired − velocity) · mass · 4`, stop inside the radius. Mode 3 snaps the vehicle to the optimal spot every frame (`DecideSnap`), mode 2 arrives at it
(radius 0.01), mode 1 (`CameraVehicle_t`): without an attractor `CalcSteering`'s first branch asks for `target − unit(target − pos) · max(|target − pos|, 0.9)` (`+0x198`, `_DAT_1003e04c`) plus 0.4 m
while the camera is below the target, i.e. **the camera stays where it is, watches the character, is pushed out to 0.9 m and lifted to the target's height** [RE of the
branch as decompiled; the sensor steering and the second branch are unresolved, so mode 1 may follow in the retail client in cases not modelled here].
