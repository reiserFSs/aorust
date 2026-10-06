# Own-character movement (RE of Gamecode.dll / Vehicle.dll / N3.dll)

Implementation: `crates/aomac/src/play/movement.rs` (`Movement`, `Fsm`, `World`; unit tests `cargo test --release -p aomac movement`).
Address tags: `[GC]` Gamecode.dll, `[VH]` Vehicle.dll (the steering integrator, imported into a private Ghidra project;
`Vehicle_t` is not in the other projects), `[N3]` N3.dll, `[GUI]`, `[IF]` Interfaces.dll. All images base 0x10000000.
Scope: how the control dynel moves and which `CharDCMoveIIR_t` it sends (format: `docs/zone/outgoing.md` §5). Corrects/extends
outgoing.md §5.1.

## 1. Object graph

* `n3Dynel_t + 0x50` = `Vehicle_t*` (a `PlayerVehicle_t`/`CharVehicle_t` for the own character; vftables [GC] `PlayerVehicle_t` 0x10160ba4/0x10160bb0/**0x10160bbc**
  (the 45-slot one is the primary, object offset 0), `NPCVehicle_t` 0x10160ad4.., `CharVehicle_t` 0x1016095c..). `Vehicle_t` itself
  (position, velocity, steering integrator, `EnsureSurfaceAlignment`) lives in [VH]; the player-specific inputs and the
  state machine in [GC].
* `CharVehicle_t + 0x178` = `CharMovementStatus_t*` (the FSM, vftable [GC] 0x101606b4; ctor `FUN_1006c16d`, base `MovementStatus_t`
  vftable 0x10160aa0, ctor `FUN_1007038f`). Reached through `FUN_1006ed98` (= `vehicle + 0x178`). Its state variables
  (getters `FUN_100704e6..FUN_1007050a`, setters `FUN_1007052b..FUN_100705a0`):

  | off | var | values |
  |---|---|---|
  | +4 | mode | 1 Frozen, 2 Walk, 3 Run (ctor), 4 Swim, 5 Crawl, 6 Sneak, 7 Fly, 8 SitGround, 0xB Sleep, 0xC Lounge (9 = camping?, see §9) |
  | +8 | forward axis | 1 idle, 2 moving |
  | +0xC | forward dir | 0, 1 forward, 2 reverse |
  | +0x10 / +0x14 | strafe axis / dir | 1 idle, 2 moving / 0, 3 left, 4 right |
  | +0x18 / +0x1C | elevate axis / dir | 1, 2 / 0, 5 up |
  | +0x20 / +0x24 | turn axis / dir | 1 idle, 4 turning / 0, 3 left, 4 right |
  | +0x28 | jump | 1 idle, 3 jumping |
  | +0x30 (CharMovementStatus) | last speed mode | ctor 2 (walk); `SwitchToRun/Walk` apply sets 3/2; every `Leave*` returns to it |

  FSM vftable: [3] `FUN_1006c1ab` IsAllowed(id), [6] `FUN_1006c460` → `FUN_1007069f` Transition(id), [7] `FUN_1006c469` IsMoving, [8]
  `FUN_1006c60f` BuildTransition(id). `FUN_1007069f(id)`: `IsAllowed(id)` && `BuildTransition(id)` valid → copy new state, run the action's
  `Apply` (action vftable [1]; the `Transition_t` stores the old state at +8 and the new one at +0x38, `FUN_1007034e/1007035a`).
  `FUN_1006c469` IsMoving = forward axis 2 || strafe axis 2 || jump 3 || mode 7.

### FullUpdate vehicle initialization

The own character previously kept the constructor's remembered Walk mode instead of deserializing its FullUpdate vehicle blob; leaving water therefore returned a server-initialized runner to Walk.
`PlayerVehicle_t` primary vtable `0x10160bbc`, slot `+0xac` (`0x10160c68`), points to [GC `0x1007135d`]: it calls `0x1006f03e`, then reads four floats into inputs `+0x360..+0x36c`.
`0x1006f03e` reads a velocity vec3 through `0x1000404e`, calls `0x1006c4dc` for the FSM at vehicle `+0x178`, then calls `Vehicle_t::SetVel`.
`0x1006c4dc` calls `0x10070438` (ten FSM bytes), then reads the remembered mode as an integer into FSM `+0x30`.
Thus the 42-byte player blob is velocity (three big-endian floats, bytes 0..12), ten FSM bytes (12..22), remembered mode (big-endian i32, 22..26), and forward/strafe/turn/elevate input floats (26..42).
The hook does not execute transition actions or synthesize inputs from axes. `Movement::restore_blob` reuses `Status::from_blob`, restores serialized velocity and player inputs, and recalculates the model's per-mode speed parameters. `Player::new` applies it after initial ground placement because placement clears axes and inputs. Short/unknown-mode blobs rejected by `Status::from_blob`, or non-finite serialized velocity/input floats, leave movement unchanged.


## 2. Move types 1..0x2A (`FUN_1006c60f` jump table [GC 0x1006d0ee], 42 entries; verified entry by entry in the assembly)

Names = RTTI/vftable names of the `TransitionAction_t` classes (vftable, Apply = vftable[1]). Entries `0x13 0x14 0x16 0x1f 0x20` (and every id
> 0x2A, `JA 0x1006d030`) point at the default label 0x1006d030 = "no transition". Neighbouring cases share code only where noted
(5/6/7/8 share the StrafeStop tail, 9/0xA and 0xC/0xD differ only in the action class, 0xB/0xE share TurnStop).

| id | action class | vftable / Apply | jump-table target | build rule (current state → new state) |
|---|---|---|---|---|
| 0x01 | ForwardStart | 0x101606dc / 0x1006ea44 | 0x1006c6e3 | fwd axis 1, or (axis 2 and dir 2) → axis 2, dir 1 |
| 0x02 | ForwardStop | 0x101606ec / 0x1006eb63 | 0x1006c765 | axis 2 ∧ dir 1 → axis 1, dir 0 |
| 0x03 | ReverseStart | 0x101606fc / 0x1006ebd9 | 0x1006c7aa | axis 1, or (axis 2 ∧ dir 1) → axis 2, dir 2 |
| 0x04 | ReverseStop | 0x1016070c / 0x1006ecf0 | 0x1006c7fa | axis 2 ∧ dir 2 → axis 1, dir 0 |
| 0x05 | StrafeRightStart | 0x101607a0 / 0x1006d6ea | 0x1006c8f9 | strafe 1, or (2 ∧ dir 3) → 2, dir 4 |
| 0x06 | StrafeStop (right) | 0x10160790 / 0x1006d69c | 0x1006c8e3 | strafe 2 ∧ dir 4 → 1, dir 0 |
| 0x07 | StrafeLeftStart | 0x1016077c / 0x1006d5f4 | 0x1006c844 | strafe 1, or (2 ∧ dir 4) → 2, dir 3 |
| 0x08 | StrafeStop (left) | 0x10160790 / 0x1006d69c | 0x1006c899 | strafe 2 ∧ dir 3 → 1, dir 0 |
| 0x09 | TurnRightStart | 0x1016074c / 0x1006d4e9 | 0x1006c9b9 | always: turn 4, dir 4 |
| 0x0A | MouseTurnRightStart | 0x1016075c / 0x1006d590 | 0x1006c9e7 | always: turn 4, dir 4 |
| 0x0B | TurnStop (right) | 0x1016076c / 0x1006d5d0 | 0x1006ca65 | turn 4 ∧ dir 4 → 1, dir 0 |
| 0x0C | TurnLeftStart | 0x1016071c / 0x1006d3fe | 0x1006c953 | always: turn 4, dir 3 |
| 0x0D | MouseTurnLeftStart | 0x1016073c / 0x1006d4a9 | 0x1006c986 | always: turn 4, dir 3 |
| 0x0E | TurnStop (left) | 0x1016076c / 0x1006d5d0 | 0x1006ca15 | turn 4 ∧ dir 3 → 1, dir 0 |
| 0x0F | JumpStart | 0x101607b0 / 0x1006d792 | 0x1006ca82 | refused if jump 3 and mode ≠ 7; else jump 3 |
| 0x10 | JumpStop | 0x101607c0 / 0x1006d821 | 0x1006cac2 | needs jump 3; jump → 1 unless mode 7 |
| 0x11 | ElevateUpStart | 0x101607d0 / 0x1006d948 | 0x1006cb04 | mode 7 only: axis 2, dir 5 |
| 0x12 | ElevateUpStop | 0x101607e0 / 0x1006d99c | 0x1006cb41 | axis 2 ∧ dir 5 → axis 1, dir 0 |
| 0x13 0x14 | – | – | default | no transition (no class; unused) |
| 0x15 | FullStop | 0x101607f4 / 0x1006d9e8 | 0x1006cb91 | forward/strafe/turn/elevate/jump axes → idle, dirs 0 |
| 0x16 | – (sync) | – | default | no transition: position/rotation sync only (§4) |
| 0x17 | SwitchToFrozenMode | 0x101608b8 / 0x1006e4d8 | 0x1006cdfa | mode 1 |
| 0x18 | SwitchToWalkMode | 0x10160818 / 0x1006dba4 | 0x1006cc0b | mode 2 |
| 0x19 | SwitchToRunMode | 0x10160804 / 0x1006da5d | 0x1006cbd5 | mode 3 and last-speed-mode := 3 immediately |
| 0x1A | SwitchToSwimMode | 0x10160828 / 0x1006dd0c | 0x1006cc37 | mode 4 |
| 0x1B | SwitchToCrawlMode | 0x101608d8 / 0x1006e646 | 0x1006cce5 | needs jump 1 ∧ forward axis 1; mode 5 |
| 0x1C | SwitchToSneakMode | 0x10160848 / 0x1006deb1 | 0x1006cc8e | mode 6 |
| 0x1D | SwitchToFlyMode | 0x10160868 / 0x1006dfaf | 0x1006ce2b | mode 7 |
| 0x1E | SwitchToSitGroundMode | 0x10160888 / 0x1006e2be | 0x1006cd30 | needs !IsMoving ∧ Features bit 4; mode 8 |
| 0x1F 0x20 | – | – | default | no transition |
| 0x21 | SwitchToSleepMode | 0x101608e8 / 0x1006e6ff | 0x1006cedf | needs jump 1 ∧ forward axis 1; mode 0xB |
| 0x22 | SwitchToLoungeMode | 0x10160908 / 0x1006e8a9 | 0x1006cf86 | needs !IsMoving ∧ jump 1 ∧ axis 1; mode 0xC |
| 0x23 | LeaveSwimMode | 0x10160838 / 0x1006de93 | 0x1006cc63 | mode := last speed mode |
| 0x24 | LeaveSneakMode | 0x10160858 / 0x1006df81 | 0x1006ccba | mode := last speed mode |
| 0x25 | LeaveSitMode | 0x10160898 / 0x1006e372 | 0x1006cd88 | no Features bit 4 → SwitchToFrozen (mode 1); else if mode 8 → last speed mode |
| 0x26 | LeaveFrozenMode | 0x101608c8 / 0x1006e5e3 | 0x1006ce00 | mode := last speed mode |
| 0x27 | LeaveFlyMode | 0x10160878 / 0x1006e115 | 0x1006ce57 | mode := last speed mode |
| 0x28 | LeaveCrawlMode | 0x101608a8 / 0x1006e3ff | 0x1006ce82 | refused if IsMoving; no Features bit 4 → frozen; mode 5 → last speed mode |
| 0x29 | LeaveSleepMode | 0x101608f8 / 0x1006e79f | 0x1006cf2a | refused if IsMoving; (feature rule); mode 0xB → **8** (sit) |
| 0x2A | LeaveLoungeMode | 0x10160918 / 0x1006e949 | 0x1006cfd5 | refused if IsMoving; (feature rule); mode 0xC → **8** (sit) |
| 0x2B | (pseudo) mouse look | – | not a table entry | `N3Msg_MovementChanged` rewrites it to 0x16 and never builds a message (§5) |

Quirk, kept: the new "elevate axis" local is initialised from the *strafe* axis (`FUN_100704f2` is called twice in the prologue,
asm 0x1006c688/0x1006c68f), so any transition copies the strafe axis into the elevate axis.

Names of the older notes: 0x0A/0x0D = `MouseTurnRight/LeftStart`; 0x18 = SwitchToWalk, 0x1C = SwitchToSneak (outgoing.md listed them unresolved);
0x24 = LeaveSneak. The ids the captures contain (`01 02 07 08 09 0a 0b 0c 0d 0e 16 1e`) are all in the table (0x1e = SitGround: the NPCs sitting
at the Arrival Hall bar, 60 of the 144 captured moves).

### 2.1 Permission table (`FUN_1006c1ab`, vtable [3]) – id refused in mode

| mode | refused ids (everything else allowed) |
|---|---|
| any other value | none |
| 1 Frozen | all except 9 0xB 0xC 0xE 0x10 0x26 |
| 2 Walk | 0x18, 0x21, 0x22, 0x25, and 0x1E while forward axis 2 or jumping |
| 3 Run | 0x19, 0x21, 0x22, and 0x1E while forward axis 2 or jumping |
| 4 Swim | 5 7 0xF 0x18 0x19 0x1B 0x1C 0x1D 0x1E 0x1F 0x24 0x27 0x28 |
| 5 Crawl | 5 7 0xF, 0x18..0x19, 0x1C 0x1D 0x1E 0x27 |
| 6 Sneak | 0x21 0x2A |
| 7 Fly | 0x1B 0x1C 0x1E 0x21 0x22 0x25 0x28 0x29 0x2A |
| 8 SitGround | all except 9..0xE, 0x1A, 0x25, and 0x21/0x22 only while !IsMoving |
| 0xB Sleep | all except 0x1A 0x22 0x29 |
| 0xC Lounge | all except 0x1A 0x21 0x2A |

Note 0x16 (sync) is refused in Frozen, SitGround, Sleep, Lounge (and mode 5 passes it): a sitting character sends no syncs.

## 3. `N3Msg_MovementChanged(action, f1, f2, bool)` [GC 0x18b5c] – the only key → message path

GUI slots (`FlowControlModule_t::SlotMovement*` [GUI 0x10027ec1..0x10028077]) call `N3InterfaceModule_t::N3Msg_MovementChanged(action, &f)` [IF 0x1000902e],
which forwards `(action, f, 0.0, **true**)` (asm 0x10009040: `PUSH 1`). The 4th argument is therefore always true. Slot polarity: `b == false`
= start (`Forward(b) = b+1`, `StrafeLeft(b) = 7+b`, `Right(b) = 9+2b`, `Left(b) = 0xC+2b`); Forward/Back/StrafeLeft slots fire AFCM
event (0xA, 0xE3) first when mode == 8 (sit-to-stand request).

Order of events inside (all must hold, otherwise nothing happens, **nothing is sent**):

1. control dynel exists; `0x2B` → `0x16` + "local" flag;
2. `vehicle->vtbl[0x24]` (`FUN_10070fd0`): no path-following (`+0x108 == 0`) and `dynel+0x21d == 0`;
3. `fsm.IsAllowed(action)` (§2.1);
4. not (mode 7 ∧ action 0xF); not (action 0xF ∧ stat 0x296 MechData ≠ 0); not (action 0x18 ∧ `dynel+0x2c8 ≠ 0` ∧ `FUN_1002e347() ≠ 0`); not
   (mode 8 ∧ action 0x1C);
5. if mouse-look is active (`DAT_102e2588`): `0xC→7, 9→5, 0xE→8, 0xB→6` (turn keys become strafes);
6. not local: build `CharDCMoveIIR_t(dynel.identity, action, RelPos, RelRot, f1, f2)` [GC 0x1006ba23] **from the pose before the action**, apply it
   (`FUN_1006b84b`, vtable [2] of the IIR, §3.1), then `SendIIRToServer`. Local (mouse look): `dynel->vtbl[0x74](zero, RelRot, f1, f2)` =
   `n3Dynel_t::VehicleForwardUpdate` [N3 0x10004ebd] (yaw += f1, pitch part f2 – see §9) and `client+0xFC := 1`.

### 3.1 Applying a message (`FUN_1006b84b` → `FUN_1006bcc6`)

* `FUN_1006bcc6`: dynel by identity; passes only if `vehicle->vtbl[0x24]` and not (action 0x1C ∧ `dynel+0x1d4 → +0x44 ≠ 1`) and
  (control dynel ≠ this dynel **or** `ToBePassedOn == 1`, `FUN_1006bb6c` = vtable [3] `ToBePassedOn`, set to 1 only by the constructor of a locally
  built message `FUN_1006bb28`, 0 for network-read messages `FUN_1006bb79`). Otherwise `ClearToBePassedOn`, drop.
  **So `CharDCMoveIIR_t` received for the own character is ignored.** (Server corrections of the own position are not CharDCMoves.)
  For a local message it stores `UpdateLastMotionMessageData` (time, RelPos, RelRot: `client+0xD8/0xF0/0xE0` [GC 0x10016a48]) and re-sets the
  dynel pose to itself (`Vehicle_t::SetRelPosRot` is a no-op for identical values, velocity is kept).
* then: Features (stat 0xE0) must have bit 2 or 4; with bit 4 every action is applied, with only bit 2 just the turn ids 9..0xE; action 0x16 →
  `VehicleForwardUpdate(pos, rot, f1, f2)`; action ≠ 0xF and path-follow active → cancel it; action 0x1D (fly) additionally needs
  `dynel+0x21c ≠ 0` or GmLevel (stat 0xD7) bit 0; finally `fsm.Transition(action)`.
* the IIR is written after that (`FUN_1006bc55`): `elapsed_ms = (int)((now - DAT_102e32d8) * 1000)` with `now = GameTime_t+0x28` (double
  seconds), **truncated** (`_ftol` = CVTTSD2SI; asm 0x1006bc62..0x1006bc78), then `DAT_102e32d8 := now` (initially 0, so the first message carries `now*1000`).
  (outgoing.md says "round": it is a truncation.)

## 4. Other producers and cadence

* `CheckMotionUpdate` [GC 0x18eb8], called every frame: `t = now - client+0xD8` (`GetTimeSinceLastMotionMessage`);
  1. `t > 5.0` ([GC 0x101574fc]) ∧ `vehicle->vtbl[0x27]` (`FUN_1006efe1` → `fsm.IsMoving`) → `MovementChanged(0x16, 0, 0, true)`; return;
  2. `t > 0.25` ([GC 0x101574f8]) ∧ `client+0xFC` ∧ `acos(dot(RelRot, client+0xE0)) > 0.17` ([GC 0x101574f0], radians, no `abs`, no `2×`) →
     `MovementChanged(0x16, 0, 0, true)`, `client+0xFC := 0`.
  So a moving character repeats a 0x16 sync every ~5 s of silence; mouse turning produces one sync per ≥ 0.25 s once the heading differs from the
  last sent one by 0.17 rad (quaternion half-angle: 0.34 rad of yaw). The `look` floats of every sent message are (0, 0) except the
  mouse-look start moves, which also carry (0, 0): the look deltas of 0x2B are never put on the wire (`FUN_1006b9d6` writes the IIR's `+0x40/+0x44`;
  `N3Msg_MovementChanged` only passes real values for the local branch).
* `FUN_1005a5d6` [GC] (second `CharDCMoveIIR_t` producer) is the per-frame handler of the client character (`this+0x140 ≠ 0`): counter
  `DAT_101bef94` (initial −1): `if (>0) --`; at 0 it builds a 0x16 move from RelPos/RelRot (look 0,0), applies it and sends it, counter := −1.
  The counter is armed to 2 when `dynel+0x220 != GetZoneInstanceID()` (zone border crossed) and the old id was non-zero; i.e. **two frames after
  entering a new zone a sync is sent**. It bypasses the permission checks of `N3Msg_MovementChanged`. (Also computes `VisualEnvFX_t::DisplaySyncPosition`.)
* No other sender: `StartTeleportTry`, `EndCameraMouseLook` call `MovementChanged(0x16,…)` (outgoing.md §5.1).

### 4.1 [LIVE] A same-playfield `ZoneRedirection` in the middle of a walk is the death respawn, not a movement rejection
`AOMAC_NET_TRACE` of the Borealis `goto` walk (Aomacvolk, 1661-cell route; capture excerpt `docs/captures/zone_walk_death_borealis.rec`, test
`ao-net/tests/walk_death_capture.rs`): ~218 s in, around (716, 32, 355) after 478 route cells, the session got "Locating next playfield server.", an `n3TeleportIIR_t`
with a playfield proxy, "Located.", "NEW LOC: 679.6 72.8 476.7" and `ZoneRedirection` to the same zone server, then `CharInPlay` at the playfield start.
Reading the frames before it: the hostile camp on the route (`FollowTarget` + `AttackIIR_t` / `StopFightIIR_t` from hostile dynels such as the Rollerrat 1003039) engaged the
unarmed lvl 2 walker (4/40 HP after the previous respawn), `CharacterAction` 99 (`Died`) arrived 2.46 s before the redirection (the death flow of `docs/zone/combat.md`).
Our `CharDCMove` stream (736 frames over the session) is within the client's rules:
* only move ids 1..8 (forward / back / strafe / turn start+stop) and 0x16; elapsed-ms is the truncated gap, `look` = (0, 0), positions advance <= 13 m/s (no teleport-like step);
* the 0x16 syncs (6 in the session) are the `FUN_1005a5d6` zone-border syncs two frames after `GetZoneInstanceID` changed (§4), never the 5 s idle sync (the walker sends
  a start/stop pair every 0.1-0.4 s: the harness autopilot toggles W; a real player sends one per key change);
* the only `n3TeleportIIR_t` of the whole session is the one of the respawn: the server never corrected a position.
The speed/teleport/terrain-height checks are therefore not the cause; the walk simply has to avoid hostiles (`AOMAC_LIVE_AVOID=1` makes `goto` route around living
side-3 NPCs by the given metres) and the harness now ends a `goto` when the own character dies (`Module::is_dying`).

### 4.2 [LIVE] Zone line, swimming (Aomacvolk, Borealis pf 800, 2026-10-06)
* **Borealis exit** (no whompah / Grid terminal in pf 800, as in 4582 / 4833): walking into the door prop 0xC748 at (684, 74, 534) (placed doors carry no destination: the zone
  lines are the server's) gave "Locating next playfield server." + `n3TeleportIIR_t` + `ZoneRedirection` to **pf 790 "Stret West Bank"** at (1277.05, 0.12, 2891.71) (`NEW LOC` line). Walking
  back into the "BOREALIS" gate door at (1273, 1, 2887) returned to pf 800 at (681.5, 72.8, 530.7). Both redirections kept the zone server (199.241.136.157:8502).
* **Swimming**: the nearest deep water by a walkable route is the lake at (773, 8.8, 589) (the plateau's south-east foot; the nearer pools at (592, 89, 521) / (592, 59, 565) are not
  reachable by the collision route). A 576-cell `goto` (`AOMAC_LIVE_AVOID=8`, around the Pumpkin-Head line) ended with the FSM in **mode 4 (Swim)** at y 9.99 (surface 10.0): the avatar holds the IdleSwim clip (0xc3, arms out, shot inspected; night 20:49 RKT, so the shot is dark).
  The earlier harness `water` scan mixed the server and the collision z axis (`player::to_col` negates z) and pointed `goto=400:-147` at the wrong place; fixed with a test.
* **Swim → ground restores Run** (post-fix, offscreen real live flow, audio muted): `AOMAC_LIVE_CHAR=Aomacvolk AOMAC_LIVE_AVOID=8 AOMAC_LIVE_STEPS='wait=2,pos,goto=773:589,pos,goto=760:575,pos,W=1,pos' cargo test --release -p aomac live_walk -- --ignored --nocapture`.
  The received own blob restored remembered Run at login. The second route crossed deep water at (768.43, 9.99, 583.20), FSM **4**, then emerged at (760.13, 15.01, 581.31), FSM **3**, with remembered Run unchanged. On dry land a one-second forward hold moved (760.19, 17.03, 575.24) → (756.34, 18.80, 575.65), rather than the 1.5 m/s Walk mode. No run-toggle key was used. Live test passed.
  Integrated-snapshot `movement::tests` passed all 25 cases, including `full_update_restores_speed_axes_and_player_inputs` (also checks explicitly remembered Walk, invalid blobs and no outgoing messages).

## 5. Mouse look (`N3Msg_MouseMovement(dx, dy)` [GC 0x1964b], `N3Msg_EndMouseMovement` [GC 0x19a38])

* First event (`DAT_102e2588 == 0`): a key turn in progress is converted: left turn (dir 3, turn rate ≠ 0) → `MovementChanged(0xE)` then `(7)`; right → `(0xB)` then `(5)`. Then
  `DAT_102e2588 := 1`.
* Needs Features bit 2 or 4. If TurnSpeed (stat 0x10B) ≠ 0: `|dx| ≤ TurnSpeed / 100000 [GC 0x101575d0] * (client+0x68)` (client+0x68 unresolved).
* `dx ≥ 0`: turn dir ∈ {0, 3} → `MovementChanged(0xA, 0, 0)` (the `dx` of that event is dropped); else (already turning right) → `MovementChanged(0x2B, dx, dy')`.
  `dx < 0`: turn dir ∈ {0, 4} → `(0xD)`; else `(0x2B, dx, dy')`. `dy' = 0` in third person, `s_nInverted*dy` in first person (mode ≠ 7).
* `0x2B` = local yaw rotation by `dx` radians (positive = right = same sense as TurnRightStart), marks the dynel dirty for `CheckMotionUpdate`.
* Release: `DAT_102e2588 := 0`; if turn axis 4 → `MovementChanged(dir 4 ? 0xB : 0xE)`; if strafe axis 2 → `(8)` and `(6)`.
* Keyboard turn keys while mouse-look is active are remapped to strafes (step 5 of §3).

## 6. The vehicle (inputs and integration)

`PlayerVehicle_t` inputs (floats): `+0x360` forward (±1, `FUN_100712a6`), `+0x364` strafe speed (`FUN_100712c6`), `+0x368` turn rate rad/s
(`FUN_100712b6`), `+0x36C` elevate speed (`FUN_100712f7`). Set by the `Apply` functions of the actions (only when `vehicle->vtbl[0x23]` = player-controlled,
true for `PlayerVehicle_t`):

| action | effect |
|---|---|
| ForwardStart | `SetDirection(1)`, `fwd := 1`, recompute (`FUN_1006f4a2`), re-tune an active key turn (`FUN_1006c506`) |
| ForwardStop | `fwd := 0`, **`Halt` (velocity := 0, instant stop)**, recompute, re-tune |
| ReverseStart / ReverseStop | `fwd := −1`, `SetDirection(−1)` / `fwd := 0`, `Halt`, `SetDirection(1)` (a direction change halts) |
| TurnLeft/RightStart | turn rate ∓ (below); MouseTurn*Start and TurnStop: 0 |
| StrafeLeft/RightStart | `strafe := ∓ FUN_1006f894(class)` with class 2 (walk, sneak), 7 (fly), else 3; StrafeStop: 0 |
| JumpStart | recompute, `vtbl[0x2c]` (`FUN_1006f9e9`) launch (§7) |
| ElevateUpStart / Stop | `elevate := 3.0` / `−0.8` (hover sinks) |
| FullStop, Frozen | `Halt`, `SetDirection(1)`, forward/strafe/turn := 0 |
| Run/Walk | recompute; last-speed-mode := 3/2; from fly: `EnableFalling`, `y += 0.1`, elevate := 0 |
| Fly | `DisableFalling`, `y += 0.5`, orientation mode 4 |
| Sit | recompute, `Halt`; Crawl/Sleep/Lounge/Leave*: recompute, `CalculateGroundPoint` (snap to the ground) |

**Speeds** (m/s; `FUN_1006f4a2` sets `SetMaxVel`, `SetMaxForce`; RS = `FUN_1006edb3`):
RS = RunSpeed stat 0x9C, but below 15 % health (`ratio = Health(0x1B) / (Life(1) * 0.15) < 1`) `RS := ratio * (RS + 1000) − 1000`.

| mode | max speed | note |
|---|---|---|
| Walk, Sneak, other | 1.5 | [GC 0x1015d76c]; not stat dependent |
| Run forward (dir 1 or none) | `clamp(5.0 + RS/275, 1.5, 13)` | consts [GC 0x101574fc 0x10160a48 0x10160a34] |
| Run reverse | `clamp(3.0 + RS*0.0025454545, 1.05, 9.1)` | [GC 0x10160a60 0x10160a54 0x10160a50] |
| Swim | `clamp(3.0 + RS/440, 1.5, 8)` | |
| Crawl | 1.0 | |
| Fly | `clamp(7.0 + RS/275, 1.5, 15)` | |
| Strafe (`FUN_1006f894`) | walk/sneak 1.5; run `clamp(2.5 + RS/550, 0.75, 6.5)`; fly `clamp(3.5 + RS/550, 0.75, 7.5)` | half of forward, except walking |

Steering force `F = min(2 * mass * v, 100000, 10000)`, `mass = Vehicle+0x34` (10.0 when 0 [GC 0x1015f168], real source unresolved) ⇒ acceleration
`F/mass = 2 v` m/s²: top speed after 0.5 s. The stat WalkSpeed/SwimSpeed are not read; **CurrentMovementMode (0xAD) is never read by the
movement code** (immediates 0xAD occur only in the stat-name table). The WaitState stat (0x1AE) mirrors sit/crawl/sleep/lounge (`Apply` writes
2/0xE/0xF/0x10, 0 on leave; `N3Msg_SitToggle` [GC 0x10028e0a] and `N3Msg_CrawlToggle` [GC 0x278c9] read it).

**Key turn rates** (rad/s, `FUN_1006d3fe/1006d4e9/1006c506`): TurnSpeed (stat 0x10B) = 0: standing (forward axis 1) ∓3.5, moving ∓1.5 ([GC 0x10160690
0x10160694 0x1016072c 0x10160728]); ≠ 0: `∓TurnSpeed/11000` standing, `× 0.5` moving. Negative = left. Captures: stand-still turns 3.6 rad/s,
walking turns 1.2–1.9 rad/s.

**Integrator** (`Vehicle_t::Run` [VH 0x1000e849] → `FUN_1000e3d3`; substeps ≤ `Vehicle+0x104`, frames > 4.0 s skip; gravity `s_vGravityAccel = −20`):
1. airborne: `vy += −20 dt`, `|vy| ≤ 50`;
2. `CalcSteering` (vtbl [0x13] `FUN_10070fee`; none in modes 1, 8, 9): forward input > 0 → `SteeringForward`: force = body forward × F; < 0 →
   `SteeringReverse` (× −1). `v += force/mass*dt`, then `|v| ≤ maxVel`. No input → velocity unchanged (only `Halt` stops);
3. strafe/elevate (vtbl [0x14] `FUN_1007118c`): direct velocity `right*strafe + up*elevate`; standing: truncated to maxVel; moving: the sum of velocity and strafe
   is rescaled to the forward speed (diagonals are not faster); `pos += that*dt`; `pos += v*dt`, `pos.y += vy*dt`;
4. turn (vtbl [0x15] `FUN_1007124c`, angular velocity `(0, rate, 0)`): standing → body quaternion := `q(Y, rate*dt) * q`; moving →
   **the velocity vector is rotated** (`FUN_100014c3`), the body follows;
5. `EnsureSurfaceAlignment` [VH 0x1000d1aa] (collision, ground, landing) and OrientationMode 0 (`FUN_1000c616`): body heading :=
   horizontal direction of the velocity (opposite of it while `SetDirection(−1)`).

Heading convention (see the module doc): `q = (0, sin(yaw/2), 0, cos(yaw/2))`, forward = `rot(0,0,1) = (sin yaw, 0, cos yaw)`, `yaw` increases clockwise
(right). Validated against the captured relayed moves (`docs/captures/zone_ithaca.rec`, char 33402): after ForwardStart the displacement direction
`atan2(dx, dz)` equals `2*atan2(q.y, q.w)` to < 0.01 rad, reverse moves point at `yaw + π`, left turns decrease `yaw`; unit test
`heading_convention_matches_capture`. Scene mapping: scene = (x, y, −z) ⇒ `ao_render::Camera` yaw = `yaw`.

## 7. Jump and falling

`JumpStart.Apply` [GC 0x1006d792] → `vtbl[0x2c]` = `FUN_1006f9e9(h)`: ignored while `PlayerVehicle+0x164 ≠ 0` (a jump is in progress);
height `h = FUN_1005844d` = `max(0.5, (Agility(0x11) + Strength(0x10))/200 + 1)` (sums > 800 are clamped to 800 unless GmLevel ≠ 0; consts [GC
0x1015f368 0x1015f358 0x10155eb8 0x1015d0a4]); the ceiling raycast `Surface_i::GetLineIntersection(pos, pos + (0,100,0))` (vtable +0xc, f32 100 @ GC 0x10155eb0; N3 slot 3 = slot 4 without the normal) shortens `h` to
`max(hit.y - pos.y - 2*BodyScale, 0.1)` (`BodyScale = MonsterScale/100`; f64 0.1 @ 0x1015def8, f32 0.1 @ 0x10160810; `World::ceiling`, launched at the next `Movement::update`); `+0x164 := h`;
an NPC (`dynel+0x21c != 0`) is raised to at least 1.5 (f32 @ 0x1015d76c, not applicable to the own player); launch speed `vy = sqrt(2 h |g|)` = `Vehicle_t::Impact((0, v*mass, 0))` [VH 0xa1b8:
`vy += impulse.y / mass`, only while not airborne and with zero x/z] + `EnableFalling`. Test `jump_numbers_and_ceiling_clamp`. Landing (`LandNow` → vtbl [0x6c] `FUN_1006eef9`): if jump state 3 → `Transition(0x10)` (JumpStop), `+0x164 := 0`. No
key-release action exists for jumping (`SlotMovementJump` acts on `b == false` only). Walking off an edge starts the same fall without a
JumpStart. Terminal speed ±50 m/s.

## 8. Role / animation mapping

`role()` returns `Role::{Idle, Walk, Run, WalkBack, RunBack, WalkLeft, WalkRight, Sneak, Swim, IdleSwim, Crawl, SitGround, SleepGround, Lounge, Hover, JumpStand,
JumpForward}` from the FSM (the original picks anim ids in the Apply functions: 0x86/0x87 strafe left/right in every mode, 0xC4/0xC5 turn in place
(no `Role`, mapped to Idle), 0x9C jump standing / 0x9D jump moving, 0xB9/0xBA/0xBB landings, 0x85 swim, 0x67 crawl, 0x88 walk back, 0xDE run back).
`anim_scale()`: `FUN_1006fb56`: `max_vel / ref_speed` (ref = the base speed of the mode: walk 1.5, run 5 / reverse 3, swim 3, fly 7, crawl 1) ×
`100/MonsterScale(0x168)`, capped at 1.3 when `max_vel > 4` (consts [GC 0x10160a8c 0x10160a88]); the animation-calibration control (`FUN_100017f7`) is implemented by `avatar::Calibration`. `Dynels` snapshots this rate at movement-state Play (`FUN_1006be27`), rather than changing it every frame.

## 9. Unresolved / approximated (labelled GUESS in the code)

* `EnsureSurfaceAlignment` is ported in docs/zone/collision.md §3.4 (`World::align`); older note: wall slide, support below with a step tolerance `0.48 + 1.1547·step` (corrected, docs/zone/collision.md §3.4) (consts [VH 0x100127e0,
  0x100127d8], rays from `y + 0.4` [VH 0x100127f8]), fall when no support, landing when `vy ≤ 0` and `y ≤ ground`. The real function casts three rays per
  step, aligns the body to the surface normal (OrientationMode 1/3/4), checks the slope (`a4 < 0.5` [VH 0x10012134]) and wades through
  `LiquidMediumData_t::m_vLiquidHeight`; the body sphere radius is `n3Dynel_t::GetBodyCollSphereRadi` (per dynel).
* `MAX_SUBSTEP` (`Vehicle+0x104`), the mass source (`Vehicle+0x34`), the mouse clamp factor `client+0x68`, the walk lock condition
  (`dynel+0x2c8`, `FUN_1002e347`), mode 9 (`FUN_10070fee` and `N3Msg_StartCamping` test it), the pitch half of `VehicleForwardUpdate` (first person
  only), the fly vertical limits (the jump ceiling clamp is ported, §7).
* Features bits: only bits 2 and 4 are read here (4 = may act, 2 = may turn); names of the other bits unknown. The default in `Stats` is 4.
* Sit / server-driven mode and position changes are traced in §10; what stays open is listed there.

## 10. Sit / stand and server-driven own movement (RE: Gamecode.dll, Vehicle.dll, GUI.dll)

Code: `Movement::{sit_input, set_pos, impulse, follow_place, follow_target, stop_if_moving, take_stat_writes}`, `Player::{sit, sit_ground, apply_own_events}`,
`Zone::own_events` (`OwnEvent`), `ao_net::n3::server_move` (decoders). Tests: `cargo test --release -p aomac movement` / `own_server_moves`, `-p ao-net server_move`.
No capture contains any of these messages (checked `docs/captures/*.rec`), so the tests use hand-built frames.

**Sit / stand.** Triggers: key `ACTION_SIT` (`Cmd::Sit`), hotbar special actions 0x4c / 0x4d (`FUN_1004256c`), camping 0x51 (`N3Msg_StartCamping`); the client has no `/sit`
text command (only `/camp` is in GUI.dll's command strings; emotes `sleep` / `lounge` need sitting). `N3Msg_SitToggle` = `action::sit_toggle(Movement::sit_input())`:
the gate `char+0x50` virtual `+0x9c` is **FSM `IsMoving`** (vehicle vtable slot 39 `FUN_1006efe1` -> `fsm.vtable[7]`; resolves the earlier [UNRESOLVED]), so a moving
character cannot sit. Sit sends `CharDCMove` 0x1e (applied locally at once, `FUN_1006b84b`); standing up sends `CharacterActionIIR_t` 0x57 and waits for the server's copy
(`OwnEvent::Action(0x57)`: LeaveSleep / LeaveLounge / LeaveSit by WaitState 0xF / 0x10 / else, [GC 0x1005d72f]); `0x56` relays a sit (`SwitchToSitGround`). The item branch (`0x55`
with the selected item's identity) needs the targeted `SimpleItem`, which the client does not track: `SitInput::item` stays `None`.
Keys while sitting: forward / back / strafe slots only emit the tip event `OnMovementWhileSitting` (`MovementWhileSittingMessage` [GUI 0x10027d5b], AFCM 0xE3); the permission
table refuses the move, the character stays sat.
Transition `Apply`s write stats locally (`SetStat`, `dynel+0xe8 vtbl[0x10]`): SwitchToSitGround [GC 0x1006e2be] RestModifier (0x1a9) 25 + WaitState (0x1ae) 2; LeaveSit
[0x1006e372] 100 + 0; ToCrawl [0x1006e646] WaitState 0xE; LeaveCrawl [0x1006e3ff] 100 + 0; ToSleep [0x1006e6ff] 0xF; LeaveSleep [0x1006e79f] 100 + 0 (mode becomes 8);
ToLounge [0x1006e8a9] 0x10; LeaveLounge [0x1006e949] 100 + **2**. `Movement` records them (`take_stat_writes`), `Player::frame` stores them in `Zone::stats`, so `WaitState`
is the single source for the next toggle. Not ported: sit's "stop fighting" (`FUN_10068b7f(1,0)`; the key path does it through `Module::before_sit`) and the GUI signals
0x4c / 0x4d (`FUN_10042da4`).

**Messages that move the own dynel** (apply = vtable slot 2; `CharDCMoveIIR_t` is dropped for it, §3.1):

| message (id) | vtable / apply | effect on the own character |
|---|---|---|
| `SetPosIIR_c` 195E496E | [GC 0x101612e0] / `FUN_10076e5a`, read `FUN_10076df2` | body `Vec3 pos, u8, i32, u8`. `UpdateReconcilePos`; i32 != 0 on the client char -> `Feedback_CrowdLimiting` (text not shown); u8 `+0x24` -> `UpdateLastAllowedPosition` (anti-cheat bookkeeping, not needed); u8 `+0x2c` -> `FUN_10059ae5(1)` = FullStop if `IsMoving`; then vehicle vtable `+0x60` = `FUN_1006eed0` (`+0x174 = +0xd4 = y`, `LandNow(y)`) and `SetRelPosRot(pos, current rot)`. -> `Movement::set_pos` (rotation and velocity kept, airborne ends, landing callback). |
| `n3TeleportIIR_t` 43197D22 | [N3 0x1003e68c] / `Activate` 0x10029f87 | in-playfield (`exit_door_id == 0`): `UpdateLastAllowedPosition`, `SetRelPosRot(pos, rot)`; own -> `OwnEvent::Place { full_reset }` -> `Movement::teleport` (zone change: docs/zone/world.md §10.2). |
| `ImpulseIIR_c` 5F4A4C6C | [GC 0x10160fe0] / `FUN_100745bb`, read `FUN_10074731` | body `i32 n`, n x `{Identity, Vec3, f32}`; per element whose dynel exists `Vehicle_t::Impulse(vec3, f32)` [VH 0x1000cd61]: a `BallisticPath_t` (ctor `FUN_100011fb`) from the vehicle position to `pos + (dx, 0, dz)` (the y of the vector is overwritten by the vehicle's y) in `time` seconds, gravity **-9.81** (f32 @ VH 0x10012798, not the integrator's -20), `v0 = (dest - start)/T - g T/2`, eval `FUN_10001294` (`pos(t) = start + v0 t + g t²/2`, `t` clamped to `T`, returns `t <= T`). `FUN_1000c41a` installs it at `Vehicle+0x108` (replaces/deletes the old one, `+0xac = 0`, first path: saves the falling flag to `+0x150`, `DisableFalling`). `Vehicle_t::Run` [VH 0x1000e849] then skips the integrator: each call sets `pos` from the path, runs `EnsureSurfaceAlignment`, and aborts (position restored) when it moved the body >= 0.5 (f32 @ VH 0x10012134); at the end: delete, `+0x108 = 0`, vtable `+0x68`, `EnableFalling` if saved. While `+0x108 != 0` `FUN_10070fd0` (vtable 0x24) refuses every movement action. -> `Movement::impulse` / `run_ballistic` (alignment reduced to `World::align` + never below the support). A non-positive time is ignored (the original divides by it). |
| `FollowTargetIIR_c` 260F3671 | [GC 0x10160f18] / `FUN_100732e3` | header = own dynel: (1) a non-zero `pos` is set with the current rotation **first** (`SetRelPosRot`; `FUN_1006fe02` repeats it as `SetRelPosIgnoreCollision`) - `Movement::follow_place`; (2) gate: dropped if Features bit 0 or `0x4000000` of the client char, or district fight-mode level (`FUN_1003e228` -> `FUN_1003e1d0`, default **2** without `PlayfieldDistrictInfo` data) > 1; (3) dropped in FSM modes 1, 8, 9, 0xB, 0xC; (4) else `fsm.vtable[6](mode)` (21 FullStop / 24 Walk / 25 Run) and `FUN_1006fe02` stores the follow target and <= 30 waypoints (`Vehicle+0x190`). The player vehicle's `CalcSteering` `FUN_10070fee` steers to the first waypoint (`SteeringDirArrive`) and clears the path through `FUN_1006fe02(0, ..)` when the same gate fires. -> `Player::follow_gated`, `Movement::follow_target`, `steer_follow`. A movement action cancels the path (`FUN_1006b84b`). |
| `CharacterActionIIR_t` 0x63 (death), 0xAD | `FUN_1005d0d8` | 0x63: `FUN_10059ae5(1)` = FullStop while `IsMoving` (`Movement::stop_if_moving`; vehicle vtable `+0x98` = `FUN_100713a7` -> `FUN_1006f008` -> `Transition(0x15)`); 0xAD: LeaveSneak when mode 6. Movement is **not** blocked by death in the movement code (the only locks are `Vehicle+0x108`, dynel `+0x21d`, Features); the death clip is the combat layer's. |
| `ResurrectIIR_t` 445F2A0B | [GC 0x10161290] / `FUN_100769bd` | body `i32 health, i32 nano`: `SetStat(Health 0x1b)`, `SetStat(CurrentNano 0xd6)` (-> `Zone::stats`), then `FUN_1003ea0d` (feedback texts, vehicle recalc `FUN_1006f963(0)`). |
| `RelocateDynelsIIR_t` 264B514B | [GC 0x1015d484] / `FUN_1003a364` | body `Identity parent`, `(n+1)*0x3f1` size word, n child `Identity` (read `FUN_1003a40a`); `RelocateDynel(parent, child, NullPos, NullRot)` = `Vehicle_t::SetParentVehicle(child, parent, NullPos)`: the child sits at the parent's origin. Decoded (`server_move::parse_relocate`); `Zone::parents` records the links. A parented vehicle's `+0x58 / +0x80` are relative; `UpdateListeners` [VH 0x1000d0a2] computes `global = parent global + parent rotation * relative` (`+0xdc`) and `global rot = parent rot * rot` (`+0xe8`): `Zone::child_rel` keeps each character child's relative pose, a later `CharDCMove` of the child is composed with the parent's position / heading (`Zone::compose`), and a parent's `CharDCMove` moves its children (`place_children`; test `children_move_with_their_parent`). The own char via `OwnEvent::Relocated` -> `set_pos`. **Not ported:** non-char parents (props), nested parents, the rotation sense of `FUN_10001ccb` was assumed to be the standard rotation about +Y (the quaternion's `2*atan2(y, w)` heading). |

**Stat hook** `FUN_10059e6a(stat, value)` (the character's `SetStat`), what it does to the vehicle: stat 0 `Flags`: `0x20000000` -> `DisableFalling` else `EnableFalling`, `0x80000000` ->
`DisableSurfaceCollision` else enable (`Movement::set_stats`: `Stats::flags`); `RunSpeed` 0x9c and `Health` 0x1b (in mode 3, when crossing the 15 % threshold) -> `FUN_1006f963` = speed
recalculation (`recalc`, run every frame here); `MechData` 0x296 -> 0 clears a vehicle object. `WaitState` 0x1ae, `Features` 0xe0 and `CurrentMovementMode` 0xad have **no hook** (they are
only read). `FUN_10044842 / 10044a07` (Features bit counters used by the status machine of fear / charm effects) call `Transition(0x17)` Frozen when bit 4 changes - the trigger is the
nano-effect pipeline (`FUN_100a8161`: "Feedback_FearActivated", swaps the `PlayerVehicle_t` for an `NPCVehicle_t` and sets dynel `+0x21d` = input lock via `FUN_10058d6c`), which is not
implemented here: see §10.1.

**Camping / logout** (`/camp` = AFCM 0x134 `StartQuitToLoginMessage` [GUI 0x10027c74] -> `N3Msg_StartCamping`): `CampStartedMessage` [GUI 0x10029d38] creates a `TimerBar_c` through `TimerSystemModule_t::CreateTimer(40000, 0:0, "Logout", 0xffffff)` [GUI 0x100518f0] (40000 is the `RenderWindow_t` / `PowerBar_t` id argument, **not** a duration; ctor `FUN_100512ae`: a `RenderWindow_t` with a `PowerBar_t(gfx 0x1a8 GFX_GUI_TIMERBAR_EMPTY, 0x1a9 GFX_GUI_TIMERBAR_FULL)` resized to the art and a `TextLine_t` with the name), sets total 30.0 s (f32 @ GUI 0x101ae5c8) and direction 1 (`FUN_1002ba93`: level = remaining / total, the bar drains; slot 0 `FUN_100514d5` subtracts the frame time) and centres it `((display - size)/2)`; then feeds LDB text 0xc8 `LogoutStarted` with 30 to signal `+0x17c` (the chat line). `CancelCampMessage` [0x10029c26] (AFCM 0x20; its sender was not found among the `PUSH 0x20` sites of Gamecode / GUI - the server's StopLogout path is [UNRESOLVED]) deletes the bar; `QuitGameToLoginMessage` -> `ActivateGameClosing(2)` [0x10028194]: save config,
clear the screen, 9 frames, login. `StartLogoutIIR_t` / `StopLogoutIIR_t` have no client apply (validity + read only), so the 30 s expiry is the server's [INFERENCE]; `Play::camp` /
`camp_frame` return to the login after 30 s unless special action 0x52 cancels. `/camp` and `/quit` both go through `start_camping` (`Chat` -> `GameAction::Camp` / quit -> `Play::camp` / `quit_cmd`), which opens the bar (`camp_bar_open`, a frame-less window with the `PowerBar` + centred "Logout" text, filled `1 - t/30` by `camp_frame`); cancel and expiry close it. [UNRESOLVED] the text colour / font (`TextLine_t::SetDefaultColor(0)`).

### 10.1 ApplySpellsIIR_t and crowd control (`ao_net::n3::spells`, `play/movement/fx.rs`)

Wire: `(n+1)*0x3f1` size word (n < 1000, read `FUN_100a6c58`), n `SpellData_t`, target `Identity`, `u8` apply (non-zero) / undo (0) [slot 7 `FUN_10128956`, apply `FUN_101288f2` -> `Beholder_t` vtable [0xc] `FUN_100026d0`]. `SpellData_t` = `i32 function, i32 p2, i32 version(4)`, criteria (`i32 n <= 1000` x 3 i32), values per `SpellFormat_c` (GameData.dll 0x1000f39e / table = ctor 0x1000fb0a, extracted mechanically: 130 formats, 231 ids; HudBuff added the 4 standard arguments of `SpellFormat_c::SpellFormat_c` @0x1000fa93), `PostBinarySpellRead` (0xcf17 criteria list, 0xcf20 ExpressionData = error). Dispatcher `FUN_100a59f5` `switch(function)`.
Movement-relevant functions (own char, `+0x21c == 0`): `0xcf81` Fear (`FUN_100a8161`; stat 0x42 == 1 or undo = end): input lock `dynel+0x21d` (`FUN_10058d6c`; makes `FUN_10070fd0` false so every movement action is refused), `PlayerVehicle_t` -> `NPCVehicle_t` (`FUN_1005a71b`: stands up from sit/sleep/lounge/crawl, fresh vehicle), Features -2 / +0x500; `0xcff0` crowd state 5 (lock + swap only); `0xcf82` (`FUN_100a8246`): FullStop, Features -6 +0x400 (undo reverse); `0xcf4b` / `0xcf4c` (`FUN_100a767e` / `FUN_100a7723`): grant / revoke Features mask stat 0x49. States 2 (charm `FUN_100459ca`/`FUN_100a805c`/`FUN_100a8384`) and 4 (daze `FUN_100a843e`) only act on NPCs. "Feedback_FearActivated" needs a caster identity of kind 50000 (`FUN_10058e36`); `ApplySpells` passes the null identity `DAT_102e9d18`, so no text.
Features counters (`FUN_10044842` grant / `FUN_10044a07` revoke, signed byte per bit at `+0x44+bit`, all 0 for a player): grant: `c+1`, bit set iff `c > 0`; revoke: `c-1`, bit cleared iff `c < 1`. Bit 4 changing: grant -> `Transition(0x26)` (+0x1a in liquid), revoke -> FullStop + `Transition(0x17)` (both skipped when `Flags & 0x20000`; grant then clears bit 4); bit 8: `DisableFalling` / `EnableFalling`; bit 0x2000000 granted while flying -> `Transition(0x27)`. Consequence (faithful): a revoke followed by its undo leaves the counter at 0 and the bit cleared. Tests `movement::fx::tests`, `ao-net spells`.
Root / snare speed: `FUN_1006edb3` and every movement read `GetStat(0x9c / 0xe0, 2)`; the nano `ModifyStat` family (0xcf22..0xcf29, `FUN_100a4a34`) goes through the stat store's inner `vtbl[3]/[4]` which was not traced, so RunSpeed / Features come from the server's StatIIR (the hook `FUN_10059e6a` recalculates speed for 0x9c) - **[UNRESOLVED]** whether the client's local ModifyStat changes kind-2 values.
Not ported: criteria evaluation (`FUN_100065d8`, ~2000 lines), target selection by stat 0x20 (2/3/0xe/0x17; default = the message target), `stat 3` repeat counter / `stat 4` chance / timed expiry (`FUN_1002de07` events 0x80 / 0x40 with stat 0x19 duration, unit unresolved: undo arrives as `ApplySpells` flag 0), the `dynel+0x2c8` (vehicle pilot) prologue of the counter functions, NPCVehicle's own steering/speed model.

### 10.2 FollowTarget path and district fight level (`play/fightmode.rs`)

* District level `FUN_1003e1d0`: `PlayfieldDistrictInfo` (rdb 1000014, `ao_audio::district`) -> district of `GetZoneInstanceID` (zone locator) -> `DistrictData+0x54` fight mode, then `FUN_1011f88b` over the `FightModeHandler` changes (`FightModeUpdate_t` 371D0542, `FUN_10124b02` read / `FUN_10124b70` apply: `u32 id, str_i16 district, u8 flags(1 set, 2 fixed, 4 remove), u8 value 0..4`), clamp 0..4; default 2 without data. `FUN_1003e228` returns the controller cache `+0xf4` for a dead / flag-0x10000 char (not ported). Used by `Player::follow_gated`.
* Sync rule (resolved, `FUN_1006b84b`): actions 0x16 (sync) and 0xF (jump) never cancel the follow; any other accepted action runs `FUN_1006fe02(0, 2.0, 0)` (clears the follow target). Per-frame `FUN_10070fee` also cancels it when own or target Features bit 0 / 0x4000000 or fight level > 1.
* Steering (`FUN_10070fee`, `Movement::steer_follow`): `FUN_1007022d(1)` picks the point: with a follow-target dynel (`Vehicle +0x180`, from the message's `target`, `Movement::follow_target(.., target)`; the position is `n3Dynel_t::GetRelPos` fed every frame by `Player::update`, a vanished dynel drops the chase) `FUN_10070185`: `d = target - pos` (3-D), point = `pos + d/|d| * (|d| - 4.0)` (`DAT_10160a90` = 4.0) when `|d|² > 16` (`DAT_10160a98`), else `pos` itself; without a target the first waypoint (`FUN_10070019` pops waypoints closer than 1.0 m horizontally, repeatedly; an emptied list runs ForwardStop). `Vehicle_t::SteeringDirArrive` [VH 0x1000ac8c]: halt when the point was crossed (`(p-prev)·(p-cur) < 0` in xz, prev = `+0xd0/+0xd8`) else `SteeringArrive(radius 0.2)`: halt for `d² < 0.04` or `< 0.01`, else desired speed `min(d/brake*maxVel, maxVel)` along `d`, force `(desired-v)*mass*4` truncated to maxForce, `v += F/mass*dt` (`FUN_1000e3d3`). Per frame `FUN_10070fee` clears the target (`FUN_1006fe02(0,2.0,0)`) when own / target Features bit 0 or `0x4000000` or the district fight level > 1 (own flags only here: the target's are not tracked). **[GUESS]** brake distance `maxVel/4`, vertical force dropped; the stop transition of a chase arrival (`FUN_10070c00`'s target branch `FUN_1006ef54`, its caller was not traced) is not run. Tests `movement::tests::follow_target_*`.
