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
(`MovementWhileSitting`). Its registered handler [GUI 0x10027d5b, registration 0x1002ada6] only calls `TipSystem_t::Event`;
it does not stand or toggle sitting. The ordinary movement action still goes through the seated FSM permission table.

**`*_GLOBAL_*` modifier matching [RE, resolved].** `InputConfig_t::ProcessInput` [GUI 0x1001a31a] first calls the user-binding dispatcher
`FUN_100180fe`. That dispatcher emits exact `id | modifiers` providers, then independently looks up the bare id and emits providers with `+0x71`
set (the global movement providers registered by `FlowControlModule_t` 0x1002abd6). This is not a fallback: an exact modified binding does not
suppress a global provider on the same physical key. Shift, Ctrl and Alt all leave the bare global provider eligible; ordinary providers still
require the exact input. `Controls::on_key` implements this for every global movement slot, including rebound non-arrow keys.

**Text-input filter [RE, corrected from the old inverted decompile reading].** `FUN_10027e46` [GUI 0x10027e46] returns true outside `TextInputMode` (3).
Inside it, arrows 32..35 require a focused `TextRenderer_c` with `+0x194 & 0x8000`, and **no Alt**; missing focus, a different view type, a clear
flag, or Alt returns false. Other global keys require Alt. Raw assembly 0x10027eae tests `[ESI] & 0x30`, jumps directly to the false return when
nonzero, and otherwise sets `BL = 1` at 0x10027eba; 0x10027eb5 reverses that branch for non-arrows. ESI is the qualifier argument
(`SlotMovementGlobalForward` 0x100289ae). `FUN_100180fe` maps Shift to 3, **Ctrl to 0xc**, **Alt to 0x30**, so Ctrl is not the Alt gate.
The slots require repeat mode 1 (`*param_6 == 1`), not autorepeat. The existing `set_text_input` provider represents the focused chat text entry.
Regressions: `native_100180fe_globals_do_not_lose_to_modified_bindings`, `native_10027e46_text_input_global_filter`.

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

### Native relative mouse reports

**[RE]** `Anarchy.exe 0x00404a66` handles `WM_INPUT`: relative `RAWMOUSE.lLastX/lLastY`
are copied into input state `+0x78`. `GUI 0x1001ae14` aggregates hardware reports per
frame and divides counts by 1000; `GUI 0x1002c17b` applies `MouseTurnSensitivity`
once. There is no pointer acceleration or Retina/pixel scaling in this path.

The macOS winit 0.30 AppKit path reads `NSEvent.deltaX/deltaY`, which are
system-scaled/accelerated and can be display-coalesced. Play look instead uses
Apple's `GCMouseInput.mouseMovedHandler` (GameController, macOS 11+), accumulating
unchanged hardware deltas until the next frontend frame. All connected mice are
enumerated each frame, with handlers attached or removed on hotplug.
Positive-up GameController Y is negated to the frontend's
positive-down convention. Focus loss and ending capture clear both native and
winit accumulators and backend liveness; the native handler never supplies buttons.
A frame containing native reports dispatches only their sum and discards duplicate
AppKit motion. Before the first native report in a capture, winit works immediately
even with a connected but silent GCMouse. After native delivery begins, a 100 ms
monotonic liveness grace retains the raw backend across short coalescing gaps and
discards AppKit duplicates on subsequent frames. If native reports stay absent
past that grace, the current winit frame is used; discarded frames are never replayed.
This timeout is a platform backend-handoff policy, **not a retail sensitivity,
acceleration or scale constant**, and does not claim correlated report timestamps.
`AOMAC_PERF` logs the selected backend when it changes on moving frames.
Other platforms retain winit counts; free-fly retains its existing winit controls.

Primary API contract: Apple's macOS SDK `GameController.framework/Headers/GCMouseInput.h`
lines 16–30 describes physical mouse raw movement, availability on macOS 11+, and
both delta axes as “Not affected by mouse sensitivity settings”; line 33 declares
the copied nullable `mouseMovedHandler`. `GCMouse.h` lines 72–76 declares
`mouseInput` nullable, so a connected device without a profile is skipped rather
than treated as usable raw input. See Apple's
[GCMouseInput documentation](https://developer.apple.com/documentation/gamecontroller/gcmouseinput).

API/platform evidence: [winit issue 4581](https://github.com/rust-windowing/winit/issues/4581)
reports raw unaccelerated hardware-rate GCMouse motion without Input Monitoring
or special Info.plist keys (including a trackpad check); [SDL implementation](https://github.com/libsdl-org/SDL/blob/main/src/video/cocoa/SDL_cocoamouse.m)
uses `deltaX, -deltaY` and clears `mouseMovedHandler` on removal.
Synthetic CGEvent drags need not reach GCMouse and are not proof of raw-input
delivery. Runnable accumulator regression:
`cargo test --release -p ao-render reports_sum_without_acceleration_and_clear_at_capture_boundaries`.

### Press-to-first-orbit callback delivery

The retail press path is immediate: GUI `0x1002c2ee` arms pending camera mode
when `LMBMouseLook` is enabled; `0x1002c09b` activates it on the first report;
`0x1002c17b` forwards that same report through Gamecode `0x10015f91` directly
to `n3Camera_t::MouseCameraControl`. GUI `0x1002c469` consults the `0.02`
accumulator threshold only on release to classify a click. Neither GUI hold
timers nor click/double-click decisions defer world orbit.

The port already follows that path: `Play::input` checks `Gui::wants_mouse`
on press, `Player::mouse` sets `Host::look` in the input event, and viewer
`set_look` activates `RawMouse` and locks the cursor before returning.
`Controls::on_mouse_motion` applies its first count; default camera mode 3
does not angularly damp it. The remaining platform mismatch was leaving
`GCMouse.handlerQueue` at its default main queue. Apple SDK `GCDevice.h`
lines 18–29 explicitly specifies that default. Winit 0.30.13's
`app_state.rs::cleared` invokes `RedrawRequested` synchronously on its main
run loop; our viewer runs `State::frame` (including frontend work, rendering
and presentation) inside that callback. Native reports submitted to that
same main dispatch queue cannot run during it. A separate hardware callback
queue removes this dependency; it does not speed up an actually blocked
render frame or promise a numerical OS scheduling deadline.

`raw_mouse::native::refresh` now sets every usable mouse's `handlerQueue`
to `dispatch_get_global_queue(DISPATCH_QUEUE_PRIORITY_HIGH, 0)` **before**
installing its movement handler. SDK `dispatch/queue.h` defines HIGH as 2
(user-initiated QoS); the handler only adds counts under the existing mutex.
The [reported GCMouse buffering problem and accepted workaround](https://stackoverflow.com/questions/79056173/gcmouse-input-buffering-and-latency)
independently describes multi-second main-queue bursts and the same queue
selection. This is supporting platform evidence, not an observed timing
measurement of this app. No retail threshold, sensitivity, GUI ownership,
cursor-grab semantics, backend grace or release flushing was changed.

Deterministic regression
`lmb_press_to_first_orbit_before_and_after_native_queue_backlog` drives
real GUI press routing, Controls and Camera with one count at 1 ms, 16 ms
redraws, and an explicitly modeled old callback backlog until 1000 ms.
Its expected first-orbit times are **1008 ms before / 16 ms after**
(992 ms difference), with synchronous capture and exactly 0.01 rad orbit
without avatar rotation. These are clock-model expectations, **not measured
runtime latency**. The independent headless macOS API smoke
`native_mouse_handlers_do_not_use_the_main_render_queue` checks the selected
queue is not main and reads back the installed queue for every attached
mouse; it injects no HID and opens no window. With no attached mouse only
the queue identity assertion is exercised.

Executed headlessly on 2026-10-15: the queue-readback smoke and the data-backed
Play regression passed, as did the complete workspace test and clippy gates.
The Play regression's 1008/16 ms values remain a controlled model, not measured
physical input latency. Real hardware press-to-display timing was not measured:
this assignment permits no window, HID injection or desktop capture.

For an actual headless dispatch measurement, run alone:
`cargo test --release -p ao-render native_handler_queue_delivery_while_main_is_not_pumped -- --ignored --nocapture --test-threads=1`.
This submits real callbacks to both queues while libtest's main thread does
not pump AppKit. It prints elapsed independent delivery and an observed
lower bound for the still-pending default-main callback after a bounded
50 ms wait. It does not inject mouse movement, synthesize a one-second
measurement, or claim a completed before-case/hardware-to-display latency.

The actual dispatch probe passed on 2026-10-15: independent delivery **44.041 µs**;
default-main callback still pending after **59.526583 ms**. This measures the
queue-delivery dependency directly, not a retail or hardware-camera latency.

### LMB versus RMB differential audit (2026-10-15)

The user's later observation is **RMB turning is instant; only LMB orbit feels
delayed**. Shared callback queues, presentation and cursor locking cannot by
themselves explain that difference. The paths were compared rather than adding
a second mouse backend or changing sensitivity:

| Stage | LMB | RMB | Headless delay / loss |
|---|---|---|---|
| `Play::input` → `Player::mouse` | GUI ownership checked on press | same ownership check | No timer or redraw dependency |
| `Controls::on_mouse_button` | pending `Look::Camera` | pending `Look::Turn` | Capture requested synchronously |
| `Controls::on_mouse_motion` | `CamCmd::Orbit` | `MouseTurn` and `CamCmd::Pitch` | First report activates **and applies itself**; no pixel/time threshold |
| Release classification | accumulated counts / 1000 compared with 0.02 | same | Release-only click decision; no motion held for classification |
| Raw collection / cursor locking | same `State::set_look` / `RawMouse` | same | No LMB-only frame-late activation or grab |
| Camera application | camera heading/vehicle orbit | avatar heading immediately; camera pitch uses the same rotation helper | Mode 2 manual orbit incorrectly entered passive camera steering |
| Render | `flush_mouse` precedes `Frontend::frame` | same | Next eligible frame; not an extra LMB frame |

Fresh retail decompiles: GUI `0x1002c2ee`, `0x1002c09b`, `0x1002c17b`,
`0x1002c469`; Gamecode `0x10015f91`; N3 `0x1002118c`, `0x1001f537`.
N3 `0x1002118c` computes a manual orbit from the **actual vehicle position**
and calls `Vehicle_t::SetRelPos` for every third-person camera vehicle, including
the damped fixed camera (mode 2). Its passive `SteeringCamArrive` is not an input
smoothing filter. The port previously moved the actual vehicle only in chase
mode 1; mode 2 changed its desired heading and waited for passive steering.
Mode 3 already used the complete new heading on the first rendered frame, with
radial-only return after occlusion. First-person also directly adds the angle.
There is no evidence supporting removal of retail passive camera following.

The fixed mode-2 path directly orbits the vehicle, and while resting stores
the resulting actual offset so passive following cannot steer back to its
pre-orbit target. Passive velocity, zoom and collision handling remain intact.
`manual_orbit_has_no_deadzone_first_delta_loss_or_angular_smoothing_in_any_mode`
replays the old target-only operation alongside the corrected command: one
count (0.01 rad) on a resting 5 m boom produces old yaw **0** versus corrected
yaw **0.01 rad** at the first 16 ms update. The old displacement is below
`SteeringArrive`'s 0.1 m stopping radius, so a small first movement could be lost
indefinitely rather than merely delayed one frame. Larger inputs were filtered
through passive acceleration. This is a runnable deterministic comparison, not
a measurement of the user's selected camera mode.

`mouse_first_frame_timeline_across_camera_modes` routes real frontend presses
and counts through all four camera modes, comparing LMB, Ctrl+RMB (the same
camera-orbit command) and normal RMB. It checks synchronous capture, a complete
first count, accumulated early counts and all-mode release-before-redraw.
Normal RMB's first movement starts its movement state; its next report turns
the heading. That existing retail movement FSM is not an LMB classifier.

The complete release trace found two additional LMB defects:

* First-person `EndLook` incorrectly routed its accumulated heading through
  `Movement::mouse_turn`, whose retail RMB-start branch deliberately discards
  its first horizontal delta and subsequently clamps turning speed. The view
  therefore snapped back when the button was released. Retail Gamecode
  `0x10019597` instead sends `MovementChanged(0x16, fmod(heading, 2π))`;
  `0x10018b5c`, `0x1006b84b` and `0x1006ba23` establish the SYNC forward-update
  transfer and wire order. `sync_camera_look` now emits that single SYNC with
  pre-transfer rotation, applies the complete signed heading locally, and
  rotates velocity without entering or clamping the RMB-start state.
  `first_person_release_transfers_full_heading_once_without_mouse_turn_clamp`
  covers stationary/running transfer, negative/wrapped angles, wire fields and
  visual continuity.
* The HUD separately classified world LMB releases using a 20-pixel cursor
  path, despite Controls already classifying them from raw counts as retail
  GUI `0x1002c469` does. A locked pointer need not move, so an orbit could
  select the object beneath it on release. World selection now consumes only
  the authoritative Controls click result and reuses interaction's merged hit
  list; GUI target-dock/target-of-target clicks retain their GUI ownership.
  This also removes duplicate world triangle picking on genuine click release.
  Neither release defect is evidence of a delay before mode-3's first orbit.

Native delivery still has an independent, shared limitation: reports that run
before AppKit has delivered the press and `set_active(true)` are not retained.
It affects both buttons, not just LMB, and synthetic frontend timelines cannot
measure its physical occurrence or justify replaying pre-press pointer motion.
Likewise native/AppKit deduplication's 100 ms grace intentionally discards fallback
duplicates; it is not an LMB threshold.

winit 0.30.13's macOS `window_delegate.rs:1123–1135` locks either button
with `CGAssociateMouseAndMouseCursorPosition(false)` and unlocks with `true`;
it does not warp or set an event-suppression interval. Its separate
`set_cursor_position` implementation warps then reassociates, but the viewer
never calls it. AppKit `mouseDown:`/`rightMouseDown:` and
`mouseDragged:`/`rightMouseDragged:` route through the same winit helpers;
there is no LMB-specific drag recognizer. The reported 250 ms warp-suppression
default therefore cannot be assigned to this no-warp path as a measured delay.
No deprecated `CGSetLocalEventsSuppressionInterval` call was added:
the installed SDK marks it “No longer supported” since macOS 10.6. The supported
`CGEventSourceSetLocalEventsSuppressionInterval` configures synthetic event
sources, not an extra event source in this physical GCMouse path. See
[Apple's suppression API](https://developer.apple.com/documentation/coregraphics/cgsetlocaleventssuppressioninterval(_:)).
Real-window verification is needed to establish any association-side physical
event suppression; the identical RMB grab is retained as the responsive control.

The configured surface uses `AutoVsync` (unless the user disables it) and
`desired_maximum_frame_latency = 1` (two Metal drawables, the platform minimum).
A 16 ms synthetic redraw clock with press at 0 ms and motion at 1 ms therefore
has its first eligible rendered camera update at 16 ms: 0 ms classifier delay
and 15 ms redraw quantization. These are deterministic model times, not measured
hardware-to-photon latency. Physical button-to-AppKit delivery, GCMouse versus
AppKit ordering, cursor-lock suppression on the installed macOS version, GPU
queue/compositor timing and the user's selected camera mode require a permitted
real-window/hardware check. No window or HID injection was used for this audit.

**Measured busy ICC CPU audit:** `mode3_icc_press_cpu_benchmark` replays
`zone_ithaca.rec` as Testy in pf 4582 at 5120×1440, camera mode 3, outside GUI
ownership. The real loaded scene had **65 remote characters, 31 posed pick
bodies, 16,246 posed remote-body triangles and 6,026 submitted actor entries**.
`BoundingBoxTargeting=0` is set through the actual DValue consumer and checked
after frames; setting only the derived zone flag would be overwritten by prefs.
The first LMB press after scene loading, before input warmups, took
**22.125 µs thread CPU / 22.708 µs wall time**; its first delta frame took
**1.277 ms CPU / 1.277 ms wall** and changed camera yaw by **0.009986639 rad**
(the complete nominal 0.01 rad within f32/look-at error).

Actual scopes in `Play::input`, **µs CPU median / p95**, 20 discarded warmups,
200 measured samples per button:

| Scope | LMB | RMB |
|---|---:|---:|
| Prelude / AFK activity | 0.417 / 0.667 | 0.416 / 0.834 |
| GUI ownership (`wants_mouse`) | 2.583 / 3.208 | 2.584 / 3.125 |
| `Player::mouse` | 0.125 / 0.250 | 0.125 / 0.250 |
| `interact_mouse` | 0.125 / 0.125 | 0.125 / 0.125 |
| Chat / screen dispatch | 0.167 / 0.334 | 0.167 / 0.375 |
| HUD / click dispatch | 4.875 / 5.625 | 3.959 / 4.584 |
| GUI input / event dispatch | 6.917 / 8.000 | 7.958 / 8.750 |
| Full press, recorder disabled | 14.666 / 18.417 | 14.708 / 17.541 |
| Full press wall time, recorder disabled | 14.875 / 18.917 | 14.959 / 17.958 |
| Baseline `Play::frame` | 805.750 / 1345.708 | 793.334 / 957.792 |
| First press+delta `Play::frame` | 798.958 / 1327.125 | 786.584 / 959.792 |

Uninstrumented full-press wall maxima were **36.083 µs LMB / 22.375 µs RMB**;
first press-frame wall maxima were **1.542 ms / 1.043 ms**. No 30–100 ms LMB
press stall was reproduced. The frame/press CPU scopes exclude GPU rendering,
native AppKit delivery, cursor association and asset-worker CPU; wall scopes
include blocking on the measured thread, not window/compositor latency.
Normal RMB receives its retail START event plus a subsequent count before the
same eligible frame; the report does not pretend START itself turns the view.

`populated_icc_left_mouse_down_does_not_pick_native_triangles` first aims a
positive-control query through a real posed body to prove the native triangle
counter is exercised, then asserts LMB down issues **zero** triangle queries.
The source agrees: interaction picks on classified release/item drop, not
press; target-dock press work is GUI rectangles; cursor hover picking exits
before any ray test while `host.look` is true. There is no press-side picker to
move to release. Isolated measured picker scopes cost **5.209 µs** for one
versus **10.250 µs** for two calls (medians): this models the removed duplicate
release work, **not** an old full-input timing measurement.

A second headless run was profiled with `sample <test-pid> 1 1`: sampling
succeeded, the benchmark passed, and sampled frame stacks included remote
pose/surface alignment and collision work rather than a sustained press-handler
stall. Under sampling the uninstrumented LMB/RMB press CPU medians were
16.916/15.708 µs; these are a separate instrumented run, not the table above.
No native input, real window, desktop capture or live session was used.

Final headless verification: `cargo test --release --workspace -j 2` passed
**1,912 tests** (62 ignored); strict
`cargo clippy --release --workspace --all-targets -j 2 -- -D warnings` passed.
The populated-scene zero-press-pick regression, first-person transfer regression,
all-mode synthetic timeline and raw-click classification regressions are included.
The ignored CPU benchmark and the standalone synthetic before/after command were
also exercised successfully; no real-device timing is implied by these gates.

**User confirmation (real window, 2026-10-15):** the user reported the slight
LMB grab delay gone after `2053134` (manual-orbit immediacy, first-person
release heading, locked-cursor release selection). Which of those changes was
responsible for the reported mode-3 delay was not isolated; the headless CPU
audit above had already ruled out a press-time picking/GUI stall.


### Drag release ordering (bug 6)

**[RE]** GUI `0x1002c17b` activates pending look through `0x1002c09b` and
applies that same movement; there is no initial movement deadzone. The `0.02`
threshold belongs only to release click classification (`0x1002c469`).
N3 `0x1001f537` has radial return but no angular damping in default mode 3.

**Port root cause:** `06d473d` already updates capture in `Player::mouse` on
button-down; `Controls::on_mouse_motion` already activates and applies its first
delta. The viewer, however, drained raw counts only at redraw. A button release
first ended the frontend look, then `set_look(false)` cleared unread counts.
A whole between-redraw drag could therefore become a click, and longer drags
lost their final partial-frame motion. `State::flush_mouse` now drains the same
native/AppKit-selected accumulator before mouse-button transitions reach
`Frontend::input`, as well as before `Frontend::frame`. Release flush uses elapsed
time since the last frame, not zero (right-drag turn-speed limits consume `dt`).
Focus loss still cancels rather than replaying motion. No sensitivity, backend
handoff timeout, camera-mode damping or display-size scaling was changed.

Regression coverage added (not run by the fix worker):
`release_flush_preserves_between_frame_motion_without_replaying_it` covers both
raw/native and winit count accumulation and capture reset;
`left_drag_orbits_on_first_delta_and_keeps_release_tail_at_ultrawide_size`
uses real `Play` frontend input/game-input/frame handlers at 5120×1440,
checks one count gives 0.01 rad immediately, release tails and an entire
between-redraw 31-count flick give exactly 0.31 rad, no drag click, and no
character turn. The frontend test replays the viewer's flush-before-release
ordering; it does not instantiate an OS window or prove native report delivery.



## 3. Third-person camera (mode 3, `CameraVehicleFixedThird_t`)

* Created by `n3EngineClient_t::CreateCamera` [N3 0x10007842] → `n3Camera_t` ctor `FUN_10021a76` [N3 0x10021a76] → vehicle by
  `PreferredCameraMode`; `3rdPersonCamera == 0` starts in first person.
* **Look target** (`FUN_10020af1` [N3 0x10020af1], `FUN_10020bdb`): world position of the CAT attractor `Attractor31_camera` of the own
  model, falling back to `AttractorMesh::GetName(0)` = `Attractor01_head`, times the body scale. `Attractor31_camera` exists only on
  vehicle/mech models (28 occurrences in rdb 1010002, all on `Bone Gun Base` / `Bone cocpit` models; none on the player models, e.g. 5907, 5927, 5900) so players use the head attractor.
  **Height [RE, resolved].** `FUN_10020af1` takes the translation of `VisualCATMesh_t::GetAttractorMatrix("Attractor31_camera")`, else of
  `Attractor01_head`, of the *animated* skeleton and multiplies it by `GetBodyScale`; `FUN_10020bdb` (every frame, also
  `UpdateTargetEye` [N3 0x10021187]) then blends the stored height towards it: third person keeps 0.8 of the old value and takes 0.2
  (`_DAT_1003d9c4`, `_DAT_1003ce50`), first person keeps 0.000999987 (`_DAT_1003e2a8`) and takes 0.999
  (`_DAT_1003e2a4`; the camera is on the head and bobs with it). These are per-call weights, without delta-time normalization. Horizontal x/z come from the
  first successful scaled xyz sample (`+0x234/+0x238/+0x23c`) and are not followed thereafter [RE, `FUN_10020af1`].
  The height is clamped to ≥ 0.3 m (`_DAT_1003e29c`); a model without either attractor leaves the target at the feet + 0.3.
  `CameraVehicleFixedThird_t::SetEyeTargetLocalPos` [N3 0x1001f085] stores all three local coordinates.
  `GetLookTargetPos` [N3 0x1001d890] returns target position (`+0x160`) plus the local eye target (`+0x180`)
  rotated by target rotation (`+0x16c`); thus cached x/z rotate with the avatar, not the animated head.
  Implemented: `ActorRig::head_attractor_composed` → `Avatar::head_local()` (× body scale) →
  `Camera3p::new(prefs, None)` in Player initialization, then the first successful `set_head` caches xyz and seeds height.
  This sample is taken in `Player::frame` after stance/animation update, not from `Avatar::new`'s default pose;
  missing attractors retain the 0.3 m fallback without preventing a later successful sample. Later samples follow y only.
  The pivot rotates cached x/z by `scene_yaw(server_heading)`. Player shares the one composed sample per frame with
  the animated world head used for opacity; opacity still follows all animated coordinates.
  Solitus male (rdb 5900 family): **1.793 m idle** (body 1.871 m), bobbing ±1.5 cm while running.
  **`UseNoBobCamera`** (Login pref, `n3Camera_t+0x1e0` set by the changed callback `FUN_100219d5` [N3 0x100219d5]; ported, `ControlPrefs::no_bob_camera`, `Camera3p::follow_head`): `FUN_10020bdb` calls `FUN_100013a9`, the **squared** vector norm, on the local target gap. `_DAT_1003e2b4` = f32 0.010000001 and `_DAT_1003e2b0` = 0.25 are squared-distance thresholds: following starts only above **0.5 m**, latches (`DAT_1005c87c`), and stops only below **0.1 m**. At either equality the latch is retained. Each call takes `0.99 · old + 0.01 · new` (`_DAT_1003e2ac`, `_DAT_1003d618`), with no delta-time normalization; the 0.3 m floor still applies. Cached local x/z are unchanged, so only animated y contributes to the gap. Evidence: N3 0x10020bdb and 0x100013a9. Regressions cover 0.05/0.2/0.6 m gaps, strict threshold equality, and all three blends' per-call delta-time independence (`no_bob_uses_squared_local_gap_and_strict_thresholds`, `head_blends_are_per_call_not_delta_time_normalized`); exercised in the camera test run below.
* **Position**: `target + direction · distance` with `direction` in the avatar frame (`RecalcOptimalPos` [N3 0x1001f371]); defaults
  `(0, 0.316, −0.948)` × 5.0 m: 5 m behind and 1.58 m above the target, elevation 18.4°. The camera looks at the target
  and follows the avatar heading because the offset is avatar-relative.
  **[RE, resolved]** `FUN_10020290` [N3 0x10020290] builds the vehicle from `PreferredCameraMode`: 0 first person, 1 plain `CameraVehicle_t` (`FUN_1001f9c9`), 2 `CameraVehicleFixedThird_t(false)` (damped: `CalcSteering` = `SteeringCamArrive(optimal pos, 0.01)`), 3 `CameraVehicleFixedThird_t(true)` (default). Modes 1/2 are §7.
  **Radial return [RE, resolved, fresh decompile 2026-10-07]:** `DecideSnap` [N3 0x1001f537] measures the current eye's radius
  from the look target (`+0x58`), selects the occluded candidate (`+0x208`) or, when zero, the unobstructed optimal (`+0x1ec`).
  With `+0x214` true (mode 3), it normalizes **candidate − look target** using `FUN_1001f846` [N3 0x1001f846], retaining its radius.
  An equal or smaller candidate radius places the eye immediately at the candidate; a larger radius places it at
  `look target + normalized(candidate − look target) × (0.9 × current radius + 0.1 × candidate radius)`.
  The constants are `_DAT_1003d3a8` = 0.8999999761581421 and `_DAT_1003c890` = 0.10000000149011612 (f32 0.9 and 0.1).
  `SetRelPosRot` preserves the vehicle's body rotation. There is **no angular interpolation and no time normalization**:
  the blend runs once per `DecideSnap` call and uses no delta time. With `+0x214` false (mode 2), this function does not place the eye.
  **Initialization [remaining mismatch]:** the trace of `FUN_10022345` [N3 0x10022345] shows that all third-person vehicles with
  zero x/z receive their first placement from the optimal position (`+0x1ec`). The port's explicit placed flag instead retains
  immediate first placement at the collision candidate; it avoids blending from world zero, but is not full retail initialization equivalence.
  **Idle update phase [remaining mismatch]:** `FUN_10022345` calls `FUN_1002118c` before the head blend;
  `UpdateHeadingToPos(false)` reconstructs the stored optimal offset without unit normalization. After the head setter,
  `CalcSteering` [N3 0x1001f752] recalculates the next optimal position. In clear stationary idle this telescopes to
  the same head-follow trajectory with a one-call delay; the port currently applies the new head sample before its
  placement query. No accumulating-feedback cause was established by this trace; geometry-edge phase equivalence remains unverified.
* **Occlusion** (`RecalcOptimalPos`): `LineOfSight(target, wanted + dir·radius)`; if blocked, bisect the fraction in `[0.01, 0.95]`
  (`_DAT_1003d618`, `_DAT_1003e228`) up to 20 times until the bracket is ≤ 0.001 (`_DAT_1003e220`); the camera sits at the last **tested midpoint**
  (`+0x208` written before each test, fresh decompile 2026-10-06), not the last-clear lower bound. The camera dynel has a collision sphere of radius 0.35 (`_DAT_1003ce4c` in `CreateCamera`) → `COLLISION_RADIUS`, the margin
  tested beyond the camera. `Camera3p::update_with(.., clear)` takes the line-of-sight test (scene raycast).
  **Rock clipping root fix (2026-10-06):** the integration previously sampled eight `sphere_hit(..., 0.2)` points and only checked
  ground at the endpoint. `sphere_hit` excludes every face with `|normal.y| >= 0.5`, so overhanging/sloping rocks and ceilings were
  invisible to the boom; thin walls could also fall between samples. `player::segment_clear` now reuses `Collision::line` in
  **both directions**, as `n3Playfield_t::LineOfSight` [N3 0x1000d34c] does: its surface vtable `+0x0c` call at 0x1000d34c's body
  first tests `from -> to`, then `to -> from` only if the first ray misses. Surface triangle tests are front-face-only
  (`FUN_10031c6a`); both directions are required, not a new collision tolerance. See `collision/vehicle.rs` for the shared terrain/KD line implementation.
  `CanSeeFlexedPos` [N3 0x1001d8c6] calls `FUN_1002046f`, which calls this same playfield sight function with `ignoreDoors=false`.
  Its `RoomCanSeeFromTo` gate [0x1000d265] requires both endpoints inside rooms in dungeons and an open connecting door if the rooms
  differ; the shared integration query now applies that gate too. Outdoors there is no room gate.
  `turning_camera_pulls_in_at_a_sloping_rock_face` uses an overhanging triangle, turns through three headings,
  checks the radius-extended boom is clear within the original `0.001 * wanted distance` bisection tolerance, and checks return
  towards full distance after turning away using the per-call 0.9/0.1 radial blend, not an immediate jump to 5 m.
  `occlusion_keeps_the_original_final_midpoint` distinguishes the original midpoint from a last-clear clamp.
  The camera test run completed with **50 passed, 1 ignored**.

  **Exact ICC idle reproduction (2026-10-07, offscreen):** pf 4582, server position
  `[930.0051, 24.21451, 759.66864]`, server yaw `0.98020107` radians; own position/yaw stayed unchanged.
  The initially rounded position `[930.01, 24.21, 759.67]` and yaw `0.98` mask this collision-edge case.
  Before the fixes, 120 `live_walk` idle frames logged these scene-coordinate eyes:

  | frame | eye x, y, z |
  |---|---|
  | 48 | 926.064880, 27.586578, −757.027039 |
  | 49 | 927.242126, 27.114624, −757.816284 |
  | 50 | 927.878723, 26.859455, −758.243042 |
  | 101 | 927.415710, 27.045013, −757.932678 |
  | 102 | 926.064880, 27.586651, −757.027039 |

  The stationary avatar therefore had a recurring camera pull-in/return, changing the foreground rock/tree composition.
  After caching the first scaled head x/z and rotating them by server heading, restoring outward radius with 0.9/0.1,
  and correcting per-call head blends and squared NoBob thresholds, the offline real-animated route
  (1800 warmup calls plus 120 measured calls at 60 Hz) had **all full and final booms clear** and radius
  **4.999860 … 4.999913 m**. Frame 50 eye was `[926.0184, 27.587215, −756.977]`; frame 102 was
  `[926.0183, 27.585472, −756.977]`. Inspected static-geometry frames 50/102 showed the same foreground rock/tree,
  with no composition jump; the harness animates the head but does not draw the own-avatar model.

  Reproduce without credentials or a window:

  ```sh
  AOMAC_CAMERA_ROCK_POS=930.0051,24.21451,759.66864 \
  AOMAC_CAMERA_ROCK_IDLE_YAW=0.98020107 \
  AOMAC_CAMERA_ROCK_ANIMATED=1 \
  AOMAC_CAMERA_ROCK_SHOTS=/tmp/icc-rock \
  cargo test --release -p aomac icc_rock_camera_turn_frames -- --ignored --nocapture
  ```

  `AOMAC_CAMERA_ROCK_IDLE_YAW` selects 120 fixed-heading frames instead of the default 36-heading turn route;
  `AOMAC_CAMERA_ROCK_ANIMATED=1` selects the captured player's native idle animation instead of the static 1.5 m head.
  Idle screenshots are written for frames 0, 50, 102 and 119. The regular regression
  `icc_beach_animated_idle_camera_is_stable` asserts radius `5 ± 1e-3 m`, maximum eye delta `< 1e-3 m`,
  and full-boom clearance using real pf 4582 collision and the animated player head across all 1920 calls,
  including the 1800-call warmup; the final 120 calls are logged.

  **Native idle verification (2026-10-07):** captured 120 muted, app-window-only stills from the owned client
  (PID 71677), with no input during the sequence. All five contact sheets covering frames 0–119 and
  full-size frames 50/102 were inspected: the foreground trunk, crates and Brandon remained in the same
  composition, with no camera jump. The position matched the live logs, but a preceding test had changed heading;
  an inverse A hold of 1.5 s restored approximately the original heading, without an exact yaw log.
  This is a real-window wall-clock screenshot sequence, **not** the exact-yaw fixed-60-Hz regression above.
  Proper camp reached login, the client exited with status 0, and the live lock was released.
  The live harness additionally accepts `heading=<server_yaw>` to restore heading through production right-drag input
  before a no-input capture, and logs full-precision own pose and camera eye on each captured frame.
  The new heading step was unit-tested, but no new live authentication was attempted after the wrap instruction.
  Final isolated-worktree gates on rebased `origin/main`: `cargo test --release --workspace` passed
  **1418 tests** (19 ignored); `cargo clippy --release --workspace --all-targets -- -D warnings` passed.
* **Zoom**: wheel (`n3Camera_t::` handler [N3 0x100200ef]): `pending += ZoomSpeed/10 · notches` (a notch is `raw/120` [GUI 0x1001ae14]); a
  notch against the pending direction first clears it. Per frame (`FUN_10022345` [N3 0x10022345]): `|pending| ≥ 1` → step = `dt · pending · 3`,
  else `dt · 3` signed; distance −= step, pending −= step; pending is dropped when its sign flips or it is below 0.3 m. Distance clamps to
  **0.78 … 25 m** (`_DAT_1003e48c`, `_DAT_1003e488`). Zooming in at < 0.8 m (`_DAT_1003e038`/`_DAT_1003e2f8`) with `ZoomTo1stPerson` sets
  `3rdPersonCamera = 0` (first person); a wheel notch outwards in first person sets it to 1 (third person again, at the previous offset).
  **Zoom placement order [RE]:** mode-3 pending zoom uses the unobstructed optimal offset (`vehicle +0x1ec`) to compute
  its new radius, calls `Vehicle_t::SetRelPos` between labels N3 0x100226bd and 0x100227e9, then `Update` and `ForcedUpdate(0)` before the later
  `DecideSnap`. Active zoom therefore places the requested radius immediately in a clear scene; the 0.9/0.1 outward
  blend is passive collision return, not extra wheel/key zoom damping. `zoom_out_places_requested_radius_before_radial_return`
  distinguishes the requested wheel/key radius from a tenth-sized outward step. The port retains its existing boom
  occlusion query after zoom placement; retail `SetRelPos`'s separate vehicle collision response is not modeled here.
  Numpad +/− (`StartZoomIn` 0x100 / `StartZoomOut` 0x80 flags): `±dt · ZoomSpeed` m per frame-time through `FUN_1002118c` [N3 0x1002118c].
  Fresh decompile of `FUN_1002118c` confirms key zoom also calls `SetRelPos` before mode-3 `UpdateHeadingToPos`
  and `ForcedUpdate(0)` (the latter conditional), so unobstructed key zoom is not damped by radial return either.
  Its additional collision-response rejection and choice of current offset for outward zoom when already pulled in
  remain outside the port's shared boom-query model; the regression above covers the clear-scene placement order.
* **Orbit** (`FUN_1002118c`): the camera offset is rotated about the vertical by `dx` and about the camera's right axis by `dy` (radians);
  a pitch that would make `|dot(dir, up)| > 0.9999` (`_DAT_1003e300`) is refused. `Camera3p`: `yaw_off += dx` (view turns right),
  `elev += dy` (camera rises), clamped at asin(0.9999). The sign convention is fixed by the numpad: Numpad 4 (`ROTATE_LEFT`, flag 2) gives
  `dx = +0.02 · MouseTurnSensitivity` per frame (camera swings left), Numpad 8 (`ROTATE_DOWN`, flag 0x40) `dy = +0.02 · s` (camera up);
  these per-frame amounts are frame-rate dependent in the client, `Camera3p` applies them as `rate · dt · 60`.
* **Reset / preferred**: Numpad 5 (`N3Msg_ResetCamera` sets flag 0x2000 → `FUN_10020e0c`) returns to the stored preferred direction and
  distance; Numpad 7 (`N3Msg_CameraSetDefaultPos` → `ForceUpdatePrefDir`) stores the current ones into `PreferredCamPos*`/`PreferredCamDist`
  (the prefs persist across sessions: `CameraVehicleFixedThird_t::SetPrefs` [N3 0x1001f004]; not persisted by `Camera3p`).
* **Field of view**: `SetViewPlaneWindow(π/2, aspect)` (`FUN_1002107a`, `_DAT_1003ccf8` = 1.5708): **90° horizontal** (58.7° vertical at 16:9);
  `Lens::vertical_fov(aspect)`.
  **Near / far [RE, resolved].** `n3EngineClient_t::CreateCamera` [N3 0x10007842] calls the `n3Camera_t` ctor `FUN_10021a76` with
  (`fov` π/2 `_DAT_1003ccf8`, `aspect` = pref `AspectRation`, **near 0.2** `_DAT_1003ce50`, **far 200** `_DAT_1003ce54`, first-person flag); the ctor
  passes them to `VisualCamera_t::VisualCamera_t(fov, aspect, near, far)` (stored at `+0x150/+0x154/+0x168/+0x16c`). The `ViewDistance`
  callback (`FUN_1001fc91`, registered with the fire-now flag) replaces far at once by `max(ViewDistance·1000, near + 50)` (pref default 0.8 →
  800 m) and calls `VisualFog_t::AddClipPlanes(near, far)`; the camera is rebuilt with the old pose. `camera::NEAR`, `far_plane`,
  `camera::lens(base)`; `Player::frame` sends the lens with `far = far_plane(ViewDistance)` (`/viewdist`, the option panel pref: `Player::set_view_distance`
  re-sends it; `Renderer::set_lens` moves the fog end and the statel LOD view length with it, docs/chat/dvalue.md §5). The fog
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
  turning with the keys when the first movement arrives (turn direction 3/4 at FSM +0x24 and vehicle turn input +0x368 nonzero), the turn is stopped (0xE / 0xB) and replaced by a strafe (7 / 5).
* While a look is running (`DAT_102e2588`, set by both look messages, cleared by `End*MouseLook`) `N3Msg_MovementChanged` [GC 0x10018b5c]
  remaps turn-left 0xC→7, turn-right 9→5 and their stops 0xE→8, 0xB→6 (A/D strafe during a right drag). `Controls` emits the final actions
  and, **deliberately unlike the client**, remembers which action each key started and sends the matching stop (the client would send the strafe
  stop for a turn that began before an LMB drag).
* Release (`FUN_1002c469`, `FUN_1002c14d`/`FUN_1002c0e5`): if no look was active or the accumulated movement is ≤ **0.02** (`_DAT_101aeaf4`) it
  is a click (left: select the object under the cursor — `N3Msg_GetNextTarget`/`SwitchTarget`; right: `N3Msg_DefaultActionOnDynel`; double click
  with `DoubleclickAction`); then the look ends (`N3Msg_EndCameraMouseLook` / `N3Msg_EndMouseLook`).
  `N3Msg_EndMouseMovement` [GC 0x10019a38] separately clears the look flag, stops the active turn (direction 4 → 0xB, otherwise 0xE), and stops strafing with 8/6.
  `Controls::end_look` now delivers that right-look release to `Movement::end_mouse_look`; a camera-only release uses `end_camera_look` and does not stop keyboard turning.
  `N3Msg_MouseMovement` [GC 0x1001964b] tests **movement mode 7 = Fly**, not jumping: it sends zero camera pitch in flight, irrespective of the RMB first/third-person pitch prefs.
  Outside flight the existing per-camera RMB preference gates remain unchanged. Mode 8 is SitGround and continues through the existing movement permission table.

* Middle button = input 9 = forward while held (binding table).
* **Both buttons = forward: not in the client [RE, negative].** Every layer maps buttons 1:1: `AnarchyOnline.exe` `FUN_00404a66` (window proc;
  `0x201/0x203/0x202` → 1/3/4, `0x204/0x206/0x205` → 5/7/8, `0x207/0x209/0x208` → 9/11/12, `InputConfig_t::AddUserInput`), `InputConfig_t::
  ProcessInput` [GUI 0x1001a31a] fires a hotkey when all keys of *its* list are down but the `KeyBindings` loader (`FUN_10018bbf`) stores one input per
  bind and the default archive has no chord, the fixed key table has none, `ActionViewMouseHandler_c` (§5) keeps one pending look, and
  `N3Msg_MouseMovement`/`MovementChanged` [GC] read no button state (`GetAsyncKeyState` is not imported by any game DLL). `Controls` therefore does not
  move on both buttons (test `both_buttons_do_not_move_forward`); holding both starts the two looks as the client does. The wheel/middle button is the mouse forward.

## 6. Retained input/zoom exactness (resolved)

* **Modes 1/2 wheel zoom [RE].** `FUN_100200ef` [N3 0x100200ef] accumulates `ZoomSpeed/10 * notches` in camera `+0x204` and calls vehicle `Forward`
  [0x1001d73e], which stores it at vehicle `+0x1b8`, **not** chase radius `+0x198`. Every frame, before vehicle integration, `FUN_10022345`
  [0x10022808..0x1002298d] computes the actual radius, updates pending by `radius - previous_radius` (camera `+0x208`), and stores the new previous radius.
  A direction crossing or magnitude below 0.3 clears Forward, halts the vehicle, and calls `ForcedUpdate(1)` [0x1001e5ac], storing the exact measured radius.
  At radius below 0.8 with inward pending, it clears pending and applies the existing `ZoomTo1stPerson` preference.
* **`ZoomSteer` [N3 0x1001db64].** At each integrator substep, positive Forward seeks only for squared radius above **0.4899999797344208**
  (`f64` at 0x1003e028); negative Forward seeks only below **625** (`f32` at 0x1003e030). Otherwise it halts.
  The literal seek target is `look + (look - pos) * (2 * Forward)`; it is not a normalized radial displacement.
  `Vehicle_t::SteeringSeek` [Vehicle 0x1000a87c] requests top-speed velocity toward that target, multiplies `(desired - velocity)` by max force,
  then the existing integrator clamps force/speed and integrates. Mode 1's attractor remains higher priority [0x1001e797]; mode 2 uses
  the same zoom branch before fixed-camera arrival [0x1001f752]. The port now follows this cadence instead of directly decrementing chase distance.
* **Modes 1/2 key zoom [RE, correction].** Numpad zoom is **not** ZoomSteer: `FUN_1002118c` [N3] directly `SetRelPos`s along the actual offset,
  `ZoomSpeed * dt` per frame, maximum 25 m. Below 0.8 inward it clears the zoom-in flag and applies `ZoomTo1stPerson`; at rest it updates/stores
  the actual offset/radius. `StopRotate*`/`StopZoom*` also store the measured radius. The user-approved FOV/defaults are unchanged.
* **Movement-state stale unknown [PASS, source/RE].** `FUN_1006ed98` [GC] returns `vehicle +0x178`; `FUN_10070506` returns FSM `+0x24`,
  **turn direction** (3 left, 4 right), not movement mode. `FUN_100704e6` reads mode `+4`: 7 Fly, 8 SitGround
  (docs/zone/movement.md §1). Existing `Movement::mouse_turn` already consumes the actual turn direction and turn input, including
  the initial mouse-turn start event that drops its delta and the existing sitting permission gate.
  Regressions: `native_1001964b_turn_direction_and_release_use_live_fsm`, `native_1001964b_flight_blocks_rmb_camera_pitch_not_orbit`,
  `native_1001db64_wheel_seeks_before_storing_measured_distance`, `native_1001db64_squared_limits_and_seek_target_are_literal`,
  `native_1002118c_key_zoom_places_vehicle_directly`.
  **Headless final gate (2026-10-16):** isolated `origin/main` rebase plus this source, `cargo test --release --workspace -j2`
  passed (2146 tests, 64 ignored, exit 0); `cargo clippy --release --workspace --all-targets -j2 -- -D warnings` passed (exit 0).
  Both used the private worktree target; no window, desktop input, screenshots, login, FOV/default retuning, or game-data copies.
  Publication rebase onto the additive Legacy update: `cargo test --release -p aomac camera -j2` passed (70 passed, 1 ignored),
  `cargo test --release -p aomac controls -j2` passed (27 passed), `cargo test --release -p aomac native_100 -j2` passed (12 passed),
  followed by the same strict workspace/all-target Clippy command (exit 0). The full workspace gate was not repeated for that additive rebase.
* **Intentionally omitted unreachable controls.** `+0x1a4` is only ever 0; no reachable caller sets `Turn`/`Strafe`/`MoveUp`
  (`CalcLateralSteering` [N3 0x1001e0ae]). No generalized controller, guessed smoothing, or new numeric calibration is introduced.

## 7. Camera vehicles and scripted views (Ctrl+F8 / Shift+F8)

**Ctrl+F8 is not "next attractor".** `n3Camera_t::GetNextVisibleAttractor` [N3 0x10021987] = `CameraVehicle_t::SetCameraAttractor(0)` (drop the
attractor), `+0x204 = 0` (drop the pending wheel zoom) and, unless first person (`PreferredCameraMode` 0): `PreferredCameraMode` **1 → 3, 3 → 2,
2 → 1** (`FUN_10020032` stores the pref, `FUN_10021859` swaps the vehicle). `CamCmd::NextView` does exactly that (`Camera3p::mode`).

**Shift+F8** (`GetPreviousVisibleAttractor` [N3 0x10020faa]) steps `+0x21c` back through the list at `+0x20c` (12-byte `{zone, index, score}`;
index 0 wraps to the last) and calls `CameraVehicle_t::SetCameraAttractor(list[i])`; an empty list clears it. **The attractor only steers
mode 1**: `CameraVehicle_t::CalcSteering` [N3 0x1001e797] branches on `+0x1a0` (`attractor->vtbl[1]` = `FUN_10023ab1` = `SteeringArrive(attractor.pos, 0.1)`), while
`CameraVehicleFixedThird_t::CalcSteering` [0x1001f752] (modes 2, 3) never reads it. The camera keeps looking at the look target (`SetEyeTargetLocalPos`).
**From no selection** (index `-1`, set whenever the list changes until the automatic pick of `FUN_10021921` runs) the client decrements to `-2` and reads `list[-2]`, i.e. 24 bytes before the
vector's storage: undefined. `Views::prev` answers like the automatic pick (entry 0, the best one) so the key works from nothing selected, then steps back with the wrap as above;
without a list entry it clears the attractor like the client. Ctrl+F8 drops the attractor but keeps the index, so Shift+F8 afterwards steps from the old index (as in the client).

**The list** (`FUN_100220bb`, rebuilt every 10th frame by `FUN_10022345` unless `+0x228`): the attractors of the character's zone and its
neighbours (**`CellSpaceBase_t::GenerateNeighborList(zone, out, 1)` [Vehicle.dll 0x10002d9f] is a thunk to the cell space's vtable `+0x8c`, resolved [RE]**:
outdoors `GridSpace_t` (built by `PlayfieldAnarchy_t::InitialiseOutdoorSpace` [GC 0x10121b4f], width = `GetNumZonesInX`) `MakeNeighborList` [VEH 0x10003f69]: `zone < nx · ny`, the rectangle
of zones within `radius` = 1 of `(col, row)` clamped to the grid, row-major, at most 49 entries; in a dungeon `n3RoomSpace_t` : `RoomSpace_t::MakeNeighborList` [VEH 0x10007968] (radius ignored): the room's
own list (its door-connected rooms `GetDoorConnectZone` plus itself, sorted: `n3Playfield_t::UpdateRoomSpace` [N3 0x1000d9d8] → `RoomSpace_t::AddRoom` [VEH 0x10007acf]) and, for every
entry, that entry's list, sorted and made unique, at most 49; a room outside the table gives nothing. `CameraViews::neighbours`, tests `grid_neighbours_stay_inside_the_map`,
`room_neighbours_are_the_door_rooms_and_theirs`, real-data `real_neighbour_lists_follow_the_cell_space`) whose score (`PointCameraAttractor_t` vtbl+0xc `FUN_10023c8a`, from the character's feet + 1 m) is below 100000, sorted ascending.
Score = distance; 10000 when < 0.5 m horizontally away; + 1000 per metre the authored target is beyond `range`; + 100 when the attractor lies in the direction of the
camera's current spot (dot > 0.9); + 50 when that spot is in the clear and the character lies beyond it; + 10000 below y = 0.1. Not a candidate: disabled
(`range ≥ 2.0` sets byte `+0x30`, `FUN_10023bbb`), or `FUN_10023bfc` [N3 0x10023bfc] says no: **closed door** (`PosToRoom(attractor.pos, -1)` = room `a`, `PosToRoom(point, a)` = room `b`; `a != b` and
`n3Playfield_t::IsDoorOpenBetweenRooms(a, b)` [0x1000d1e9] false; either room missing, e.g. outdoors, skips the test) or a line of sight attractor → character that is blocked.
`IsDoorOpenBetweenRooms` reads the flag byte (`+4`) of the entry in the playfield's room-link map (`+0x2c`, key `(min, max)`; entries are created closed by `UpdateRoomSpace`); it is set by
`n3Playfield_t::DoorOpened` / `DoorClosed` [vtable `+0x3c` / `+0x40`, 0x1000d2bf / 0x1000d2e8] → `ChangeRoomStatus` [0x1000d17e], called through `n3RoomMonitor_t::DoorOpened/DoorClosed` [0x10013561 / 0x10013910] by
`Door_t`'s open / close (`FUN_1007ef56` / `FUN_1007ef90`, [GC], the door's room link `+0x1d0`). This is **not** `Door_t::CanPass` (movement, `Collision::set_door_passable`): `Collision::set_door_open` /
`door_open_between` / `pos_to_room` carry it (`camera_views::door_closed`, `Sight::door_closed`). **A dungeon link without a registered door stays closed for the camera forever** (nothing else writes the flag).
`dynels_doors::Effect::RoomOpened/RoomClosed` call `set_door_open` through `Player::door_rooms` (next to `set_door_passable`, docs/zone/collision.md §5).
With no index (-1) and none selected for `+0x224` = 1.2 s since the last automatic pick (`+0x220` starts at 20 s) `FUN_10021921` selects entry 0 on its own (an empty list drops the attractor). `Views::tick`, `Views::prev`.

**Data.** Playfield record (rdb 1000001), per zone / room: `u32 n` (< 1000), n × { vec3 pos, quat rot, vec3 target (version ≥ 6, else = pos), f32 range } =
`PointCameraAttractor_t` (`+4`, `+0x10`, `+0x20`, `+0x2c`; `RDBPlayfield_t::ReadBlob` [N3 0x1001c115], `n3Room_t` reader 0x10012803), world coordinates.
`ao_formats::playfield::{camera_views, CameraViews, CameraAttractor}`. Whole game: **208 attractors in 53 playfields, 6 usable (`range < 2`)**: playfield 322 (3) and 1892 (3),
pinned by a real-data test; so in retail the scripted views exist in two zones, and only in mode 1.

**Vehicle model** (modes 1, 2; `Vehicle_t::SteeringArrive` Vehicle.dll 0x1000ab28, integrator `FUN_1000e3d3`, `UpdateMotionConstraints` [N3 0x1001e602]): mass 20, top speed
16 (= `min(16, 6 · avatar speed)`, ≥ 2), force limit `mass · v / 0.3`, brake distance `0.3 · v`; desired velocity = towards the target at `min(v, distance / brake · v)`,
steering force `(desired − velocity) · mass · 4`, stop inside the radius. **Substep [RE, resolved]**: the `CameraVehicle_t` constructor stores `_DAT_1003df54` = 0.05 in `Vehicle +0x104`; `FUN_1000e3d3`
integrates in substeps `min(left, 0.05)` and evaluates `CalcSteering` once per substep with `s_vDeltaTimeNow` = the substep (frames over 4 s, `_DAT_10012804`, are skipped). Mode 3 snaps inward to the candidate
spot and blends outward radius per call (`DecideSnap`, §3); mode 2 arrives through `SteeringCamArrive` (radius 0.01).

**`SteeringCamArrive`** [N3 0x1001dc46, modes 1 and 2; ported `Vehicle::cam_arrive_step`, tests `a_hitch_in_the_frame_time_halts_the_camera`,
`a_wanted_spot_behind_the_look_target_is_swapped_for_one_beside_it`] [RE, resolved]: a function-static running average `a` of the substep time (first call: the substep): a substep
`h > 10 · a` (`_DAT_1003d140`) halts the camera (`SteeringHalt`); otherwise `a = a/2 + h/2`. With `l` = horizontal vector camera → look target and `t` = horizontal vector camera → wanted spot (both non-zero), `c = |l|`, `d = |t|`:
if `c < d`, `d > 1`, `|sin| = |(l̂ × t̂).y| < 0.4` (`_DAT_1003d9d0`) and `l̂ · t̂ > 0` (the wanted spot lies behind the character as seen from the camera) the wanted spot is replaced by
`look + 0.5 · (l.z, 0, −l.x) · (−1 if (l̂ × t̂).y < 0 else 1)` (`_DAT_1003cb20`), a point beside the look target at the character's height, half the camera's horizontal distance away; then `SteeringArrive(spot, 0.01)`.
(The cross products are those of the client's world, z negated against the scene: the port converts with `ao()`.)

**`CameraVehicle_t::CalcSteering` for mode 1** [N3 0x1001e797, ported `Vehicle::calc_steering`]: **the camera is a chase camera that keeps `+0x198` from the look target**, not "stays put". `+0x198` is 5.0 in the constructor
(`_DAT_1003d2e8`, every mode swap makes a new vehicle: `FUN_10021859`), set to `|target − pos|` (at least 0.9, `_DAT_1003d3a8`/`_DAT_1003e04c`) by `Update` [0x1001e54f] and to the exact distance by `ForcedUpdate` [0x1001e5ac];
those run only on events (a never-placed vehicle at x = z = 0 is placed at the camera's own spot in `FUN_10022345`; an orbit `FUN_1002118c` when the vehicle is at rest, speed < 0.02; the end of a zoom; `StopRotate*` / `StopZoom*`;
`n3Camera_t` first-person `SetRotAngles`), **not every frame**. Order inside `CalcSteering`: attractor (`+0x1a0`) → `SteeringArrive(attractor, 0.1)`; zoom (`+0x1b8`) → `ZoomSteer`; `DoDirectControl` (`+0x1a4`, never non-zero);
then the wanted spot `look − unit(look − pos) · dist` (the second branch, taken when `+0x1cd` (secondary target set by `SetSecondaryTarget`) **and** `+0x1cc` (two-shot mode), is **dead code**: `FUN_10022345`
calls `CameraVehicle_t::SetTwoShotMode(false)` every frame [N3 0x100223b2], and nothing else in N3, Gamecode or GUI imports or calls `SetTwoShotMode`; the stay-behind branch at the end (`+0x1e8`, `SetStayBehind` [0x10013cdd], no caller) is cleared
every frame likewise [0x100223c3]); `UpdateSensors` [0x1001e71f] has run before (mode 1 only, `FUN_10022345` @0x1002245b): **line of sight camera → look target blocked** (`+0x1bc` = 0): the sensor's steer direction `+0x1c0..0x1c8`
is added to the spot; **clear** (`+0x1bc` = 1): `GetSurface()` (the playfield's `Surface_i`, `VetoPosition` + `CalculateClosestPoint`, docs/zone/collision.md §3) gives the ground below the camera and a camera less than 0.4 m above it
(`_DAT_1003d9d0`) lifts the spot by 0.4 (`ground` of `Sight`); in every case a camera below the look target lifts the spot by another 0.4. Then `SteeringCamArrive(spot, 0.01)`.

**Sensor steering** [N3 0x1001d955 `CalculateSensorSteerDir`, 0x1001d8c6 `CanSeeFlexedPos`; ported `Vehicle::steer_to_view`, tests `the_sensors_steer_round_an_obstacle_to_the_nearest_view`,
`the_left_probe_wins_when_only_it_sees_the_target`] [RE, resolved]: when blocked, for the distances `f` = 0.5, 2, 8, 32 m (`(1 << i) · 0.5`, i = 0, 2, 4, 6) and in this order the directions right, left, up, down, forward, back
(of the camera: forward = `VetoForward` = unit(look − pos), up = `VetoUpAlignment` = world up with the forward component removed, right = up × forward in the client's world; the camera's rotation is the one it is steered to) the first
direction `d` for which `CanSeeFlexedPos` holds becomes the steer vector (unit length, **not** scaled by `f`): the probe point `P = pos + d · f` and the segment `P − 0.5 · d → look target` is clear **and** `pos → P` is clear.
No direction at any distance: `+0x1e9` (`IsBlind`) = 1 and the steer is 0.

**Blind camera** [`FUN_10022345` @0x100224ca…, `ReposCutOnAxis` N3 0x1001e27e; ported `Camera3p::update_with` + `Vehicle::cut_on_axis`, tests `a_camera_blind_for_1_5_s_is_cut_to_the_far_side_of_the_target`,
`the_cut_picks_the_side_the_character_faces_away_from`]: `n3Camera_t +0x178` counts the seconds with `+0x1e9` set (frames < 0.3 s only); above 1.5 s (`_DAT_1003caf8`) `ReposCutOnAxis(0)` runs and the counter resets. With no attractor and a
zero axis it halts the camera and places it at `look + R · unit(w) · dist`, `w = (2 tx, |2 ty|, 2 tz)` with `t = look − pos` (the line through the character, always above it), `R` the rotation about the vertical by ±0.541 rad (`_DAT_1003e048`,
+ when `(facing × t).y < 0`, `facing` = the look target's rotation applied to `Vehicle_t::s_cReferenceForward` = (0, 0, 1) [Vehicle.dll 0x10019398; `s_cReferenceUp` 0x100193a4 = (0, 1, 0)], i.e. the avatar's heading: the port passes its scene forward) and tries up to 10 times: the spot is accepted when the line `look → spot + unit(v)` is clear; else `dist` halves, and the loop also ends when `dist ≤ 0.8` (`_DAT_1003e038`).
(`w = 0` uses `unit(0, 1, −1.3)`, `_DAT_1003e040`.) A mode swap calls `ReposCutOnAxis(n3Camera + 0x1f4)`, i.e. the new vehicle appears at look target + the stored offset (= where the old camera was).
