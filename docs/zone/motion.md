# Movement of other dynels and overhead names

How the original client moves and animates *other* characters (players, NPCs) and draws their names, reverse engineered from
`Gamecode.dll` (GC), `Vehicle.dll` (VH), `N3.dll` (N3), `DisplaySystem.dll` (DS), `GUI.dll` (GUI), `Interfaces.dll` (IF) and checked against
`docs/captures/zone_ithaca.rec`. Implemented headless in `crates/ao-net/src/n3/motion.rs` (`Mover`, FSM, speeds, animation roles) and
`crates/ao-net/src/n3/nametag.rs` (tag text/colour/geometry rules); both have tests on the captures. Tool notes: Ghidra projects `/tmp/aomac-ghidra/proto`
(Gamecode), `playfield/P_N3`, `dsky/gui/P_GUI`, `playfield/P_DisplaySystem`, plus a scratch project of `Vehicle.dll`; addresses below are image addresses.
Conventions: Y up; `yaw = 2·atan2(qy,qw)`, facing `(sin yaw, 0, cos yaw)`, yaw grows towards +X (verified: player 33491 with `rot=(0,0.99995,0,-0.0104)` runs towards −Z,
`rot=(0,-0.728,0,0.6855)` towards −X; the `SetWantedDirection` vector `(0.368, 0, 0.930)` of NPC 1002060 is `(sin, cos)` of the heading it then runs along).

## 1. The movement FSM and the `CharDCMoveIIR_t` move types

`SimpleChar_t+0x178` is a `CharMovementStatus_t` (vftable GC 0x101606b4; slot 6 = `FUN_1006c60f` builds the transition for an id; `FUN_1006efd2` = `(dynel+0x178)->vtbl[6](id)`).
`FUN_1006c60f` is a plain `switch(id)` (decompiled again with the full body, not through the jump table at 0x1006d0ee: case values are the ids); every case stores the
`<Name>TransitionAction_t::vftable` it allocates, so the table below is exact. The `CharDCMoveIIR_t` move type **is** this id (docs/zone/dynel.md §2).
Status object layout: `+4` mode, `+8/+0xc` forward state (1 stopped, 2 moving) / dir (1 fwd, 2 back), `+0x10/+0x14` strafe state / dir (3 left, 4 right), `+0x18/+0x1c`
elevate state / dir (5 up), `+0x20/+0x24` turn state (1 stopped, 4 turning) / dir (3 left, 4 right), `+0x28` jump state (1 ground, 3 jumping), `+0x30` previous mode.
Defaults `FUN_1007038f`: mode 3 (run), every state 1, dirs 0, jump 1; `FUN_1006c16d`: previous mode 2. The `SimpleCharFullUpdate` blob starts with 12 velocity bytes, then this status as 10 bytes
(`FUN_10070438`) and a BE `i32` previous mode (`FUN_1006c4dc`). All 81 captured blobs have the default axes/current mode, but their remembered mode is Walk **or Run** (test `captured_blobs_are_the_default_status`).
NPC blobs are 28 bytes; player blobs are 42 bytes, with four input floats immediately after the remembered mode (`PlayerVehicle` read hook `FUN_1007135d`, see [movement.md](movement.md)).

| id | transition | guard (current status) → new status | live DC count |
|---|---|---|---|
| 1 | ForwardStart | fwd stopped, or reversing → fwd moving/dir 1 | 31 (players) |
| 2 | ForwardStop | fwd moving dir 1 → stopped | 1 |
| 3 | ReverseStart | fwd stopped or forward → moving dir 2 | – |
| 4 | ReverseStop | moving dir 2 → stopped | – |
| 5 | StrafeRightStart | strafe stopped or left → right (dir 4) | – |
| 6 | StrafeStop (right) | strafe right → stopped (this id and 8 share the `StrafeStopTransitionAction_t` class, different dir guard) | – |
| 7 | StrafeLeftStart | strafe stopped or right → left (dir 3) | 7 |
| 8 | StrafeStop (left) | strafe left → stopped | 1 |
| 9 | TurnRightStart | unconditional: turn state 4, dir 4 | 12 |
| 10 | MouseTurnRightStart | same as 9 (own class: mouse turning does not play the turn clip, see 1006d4a9) | 6 |
| 11 | TurnStop (right) | turn right → stopped | 7 |
| 12 | TurnLeftStart | unconditional: turn 4, dir 3 | 4 |
| 13 | MouseTurnLeftStart | as 12 | 5 |
| 14 | TurnStop (left) | turn left → stopped | 3 |
| 15 | JumpStart | not (jumping and mode ≠ fly) → jumping | – |
| 16 | JumpStop | jumping → landed (stays jumping in fly mode) | – |
| 17 | ElevateUpStart | mode = fly → elevating | – |
| 18 | ElevateUpStop | elevating → not | – |
| 19, 20 | *none* (`default:`) | – | – |
| 21 | FullStop | always: fwd/strafe/turn/jump/elevate all stopped; vehicle `Halt` | (FollowTarget mode) |
| 22 | *none*: not in the switch | `FUN_1006b84b` calls `vtbl+0x74(relpos, rot, extra0, extra1)` of the dynel first; for `SimpleChar_t` that slot is an N3 import thunk (GC 0x10131ec6 → N3 `n3Dynel_t` slot 29 `OnCellChanged`-class no-op). So 22 = **placement only** | 7 (players, paired with 1/7) |
| 23 | SwitchToFrozenMode | always → mode 1 | – |
| 24 | SwitchToWalkMode | → mode 2, previous mode := 2 | (FollowTarget mode, 28×) |
| 25 | SwitchToRunMode | → mode 3, previous mode := 3 | (FollowTarget mode, 79×) |
| 26 | SwitchToSwimMode | → mode 4 | – |
| 27 | SwitchToCrawlMode | only if not jumping and fwd stopped → mode 5 | – |
| 28 | SwitchToSneakMode | → mode 6 | – |
| 29 | SwitchToFlyMode | → mode 7 (also skipped for non-PCs whose stat 0xD7 bit 0 is clear, `FUN_1006b84b`) | – |
| 30 | SwitchToSitGroundMode | only if not (fwd/strafe moving, jumping, fly) **and `Features & 4`** → mode 8 | 60 (NPC spawn placement) |
| 31, 32 | *none* | – | – |
| 33 | SwitchToSleepMode | only if not jumping and fwd stopped → mode 0xb | – |
| 34 | SwitchToLoungeMode | only if not moving → mode 0xc | – |
| 35 | LeaveSwimMode | mode := previous mode | – |
| 36 | LeaveSneakMode | mode := previous mode | – |
| 37 | LeaveSitMode | `Features&4` clear → frozen; else if mode 8 → previous mode | – |
| 38 | LeaveFrozenMode | mode := previous mode | – |
| 39 | LeaveFlyMode | mode := previous mode | – |
| 40 | LeaveCrawlMode | not moving; no `Features&4` → frozen; mode 5 → previous mode | – |
| 41 | LeaveSleepMode | not moving; mode 0xb → **mode 8 (sit up)** | – |
| 42 | LeaveLoungeMode | not moving; mode 0xc → mode 8 | – |
| > 42 | *none* | – | – |

Modes: 1 frozen, 2 walk, 3 run, 4 swim, 5 crawl, 6 sneak, 7 fly, 8 sit-ground, 0xb sleep, 0xc lounge (9 appears only in the FollowTarget drop list). `Status::transition` implements the table;
test `fsm_guards`. "moving" in the guards is `FUN_1006c469` (vtable slot 7): fwd or strafe state 2, jump state 3, or mode 7.

**Who may drive the FSM** (`FUN_1006b84b` [GC], apply of the DC move; `FUN_1006bcc6` places first): the dynel stat `Features` (0xE0 = 224, kind 2): neither bit 2 nor bit 4 → placement only; bit 4 → every move type
(types ≠ 15/22 first cancel a follow: `FUN_1006ef82` (follow target `+0x180` ≠ 0) → `FUN_1006fe02(0, 2.0, 0)`); only bit 2 → just 9..=14. NPC records carry Features in their stat list
(rdb 1040023 header, `(stat, value)` pairs after `(0x1a5, …)`, `(2, 100000)`, `(0, Flags)`: `(0xE0, 0x8003)` for monster 254118 = ICC Shuttle Guard, `(0xE0, 0x8007)` for 17655, 22794, 30365, 45873, 251782, …); players: **[INFERENCE]** bit 4, their t1/t9-t14 messages are
applied live.

**Type 30 resolution.** All 60 captured t30 messages are the NPC spawn placement (same position as the `SimpleCharFullUpdate`, identity or spawn heading). `n3Dynel_t::SetRelPosRot` runs first (so the
message places the dynel), the FSM part then depends on Features: monster 254118 (0x8003) has no bit 4 → t30 is purely a placement for it. For the 0x8007 monsters the client code *does* run SwitchToSitGround (guard `!moving &&
Features&4`; the `Features` read is `FUN_10044b6e(helper at dynel+0x1ec, 4)` whose helper holds the dynel at +0x40, per Combat.Actions), i.e. mode 8: the NPC would play `idle-ground` (id 0xd7; `FUN_1006c065` falls back to `idle-stand` 0x78 when the NPC record has none; 17655, 26080, 30365, 204985,
247041 have 0xd7, 22794/251782/45873/30252 do not) and later FollowTargets would be dropped (mode 8). **UNRESOLVED / contradiction with what a player sees** (NPCs stand): `Mover::set_sit_allowed` defaults to false (t30 = placement), `Mover::on_move_type` uses the real guard (for
`CharacterActionIIR_t` sit actions). Needs a retail screenshot of an NPC with an 0xd7 clip just after spawning to decide.

**Pairs.** The server sends one DC move per FSM axis at the same instant (player 33491: `t1` + `t7` + `t22` at the same ms with identical pos/rot; 33402: `t9`+`t9`, `t12`+`t12`), each carrying the full position/rotation; moving players
re-send every 100-300 ms (t1 repeated = ForwardStart on an already moving char: guard false, placement only).

## 2. Modes, speeds, turning (all constants read from the DLL)

`FUN_1006f4a2` [GC] (run by the Start/Switch actions: sets mass, `Vehicle_t::SetMaxVel/SetMaxForce/SetBrakeDistance`). `skill` = `FUN_1006edb3`: stat RunSpeed (0x9C = 156), reduced when
`health/(0.15·MaxHealth) < 1` to `f·(rs+1000) − 1000`. Max speed (m/s):

| mode | formula | cap / floor |
|---|---|---|
| walk (2), sneak (6), frozen/sit/sleep/lounge | 1.5 (`FUN_1006f2b5`, `_DAT_1015d76c`) | – |
| run forward | `5 + skill/275` (0x10160a80 = 1/275) | cap 13 (`_DAT_10160a68`), floor 1.5 |
| run backward (`GetDir() < 0`) | `3 + skill·0.0025454545` | cap 9.1, floor 1.05 |
| swim (4) | `3 + skill/440` | cap 8, floor 1.5 |
| fly (7) | `7 + skill/275` | cap 15, floor 1.5; falling disabled |
| crawl (5) | 1.0 | – |

Force `F = 2·mass·vmax` (mass = `+0x34`, default 10; `F ≤ 100000` then ≤ 10000) ⇒ acceleration `2·vmax` (full speed in 0.5 s); brake distance `vmax/4`. `Vehicle_t::Run` [VH 0x1000e849] → `FUN_1000e3d3`: sub-steps of at most `+0x104` s, velocity += steering·dt/mass truncated to
max force, speed truncated to max velocity, position += v·dt, then `EnsureSurfaceAlignment`. `Halt` zeroes the velocity at once (the Stop actions call it, 1006eb63), so starting is a ramp, stopping is instant.
Strafe: `±FUN_1006f894(mode)` (`FUN_100712c6`, sign by `FUN_100713af`): walk/sneak 1.5, run `2.5 + skill/550` (0.75..6.5), fly `3.5 + skill/550` (..7.5) — this is the client's *own* strafe
(the formula carries a ×0.5 factor, i.e. half the run speed; only used for the player's vehicle, applied to remote players as the best available guess).
Turning (`FUN_1006d3fe`/`1006d4e9`, `FUN_100712b6`): rate in rad/s = ∓3.5 stationary, ∓1.5 moving when stat TurnSpeed (0x10B = 267) is 0, else `∓TurnSpeed/11000` (×0.5 while moving); left is negative. Mouse turns (ids 10/13) set rate 0 and only
play the turn clip when stationary.

**Capture check** (`npc_run_speed_matches_the_server`): short-form FollowTarget mode 25 paths that the server interrupts with a mode-21 stop 0.3–0.7 s later (NPCs with RunSpeed 144): measured 4.7–6.5 m/s, median |Δ| < 0.8 against `5 + 144/275 = 5.52`. Players (test
`dead_reckoning_tracks_players`): the dead-reckoned position at the next t1 message is a median 0.39 m (p90 0.70 m) from the server's.

## 3. Animation roles

The animation of a status is chosen by `FUN_1006be27` (stationary/moving), `FUN_1006c065` (idle of a mode) and the start actions (1006ea44 …), by **animation id**, looked up in the dynel's own table
(`FUN_1003c8b7(id)`; for NPCs the rdb 1040023 triples `(id, 0x7e2, rdb 1010003 clip)`; players use the `<set>_<clip>_01_01.ani` names via the id → clip table of `FUN_100c01c9` [GC] reproduced below).
`AnimHolder` fields (GC vftable 0x1015d580): `+4` run clip, `+8` hover, `+0xc` walk, `+0x10` idle, `+0x14` sneak, `+0x18` crawl idle. Mapping implemented as `anim_of(&Status)`/`AnimState`:

| status | AnimState (id, clip) |
|---|---|
| stopped, walk/run, no strafe/turn | Idle (0x78 `idle-stand`; record field `+0x10`) |
| stopped, walk/run, strafing left/right | WalkLeft 0x86 / WalkRight 0x87 (`walk-left/right`, also for run: `FUN_1006d5f4` plays 0x86 whatever the mode ≠ fly) |
| stopped, walk/run, turning (keyboard turn) | TurnLeft 0xc4 / TurnRight 0xc5 (`turn-left`/`turn-right`; these two clips are **not** in `character::player::Role` – `"turn-left".parse::<Role>()` would give an emote `social-turn-left`) |
| moving forward, walk / run | Walk 0x64 / Run 0x65 (`walk`, `run`) |
| moving backward, walk / run | WalkBack 0x88 / RunBack 0xde (`walk-back`, `run-back`) |
| sneak (moving or not) | Sneak 0x66 (`sneakcool`) |
| swim moving / stopped | Swim 0x85 / IdleSwim 0xc3 (fallback 0x85, then 0x78) |
| crawl moving / stopped | Crawl 0x67 / IdleCrawl 0x9a |
| fly moving / stopped | Fly 0xb3 `hover-norm` (**[GUESS]**, `FUN_1006be27` leaves the clip unchanged for mode 7) / Hover 0xb2 `idle-hover` (`AnimHolder+8`) |
| jump | JumpStand 0x9c if fwd stopped else JumpForward 0x9d (`FUN_1006d792`); landing clips 0xb9/0xba/0xbb (`jump-land-walk/run/idle`) are one-shots |
| sit (8) | SitGround 0xd7 `idle-ground` (after `ground-start` 0xd5 / before `ground-stop` 0xd6), fallback 0x78 |
| sleep (0xb) / lounge (0xc) | 0xed `sleep-ground` → loops 0xee `idle-sleep-ground`; 0xef `lounging` → 0xf0 `idle-lounging` |
| frozen | Idle |
| attack / die | set by the application from combat messages (ids 0x3e8.. weapon sets, die 0x1f4..0x1f9, 0x1770 `die-pain`) |

`SetWantedDirection` (§4) turns a standing NPC with the turn clips. Animation speed: `FUN_1006fb56` scales a newly started movement-state clip (idle included) by `anim_calibration · (vehicle maximum speed)/(+0x170 reference speed)` and by stat 0x168 MonsterScale (`100/scale`); `Dynels` implements this clip-start clock through `avatar::anim_rate` and `Calibration` (see `avatar.md`). Direct stance idles retain authored playback.

Complete client id → clip name table (`FUN_100c01c9`; 197 entries; ids 0x79-0x7b, 0x95 and a few others are not set there):

```
0x64=walk  0x65=run  0x66=sneakcool  0x67=crawl  0x68=crawl_start  0x69=crawl_stop  0x6a=climb  0x6b=op1h-button
0x6c=op2h-wheel  0x6d=wield  0x6e=wield  0x6f=wield  0x70=wield  0x71=wield  0x72=wield  0x73=wield  0x74=wield
0x75=wield  0x76=push  0x77=pull  0x78=idle-stand  0x7c=fall-back  0x7d=onback-stop  0x7e=imp-back  0x7f=imp-chest
0x80=imp-head  0x81=imp-larm  0x82=imp-rarm  0x83=imp-legs  0x84=imp-stomach  0x85=swim  0x86=walk-left
0x87=walk-right  0x88=walk-back  0x89=sneakcool  0x8a=sneakgoof  0x8b=hidecool  0x8c=hidecool-start
0x8d=hidecool-stop  0x8e=hidegoof  0x8f=hidegoof-start  0x90=hidegoof-stop  0x91=op1h-button  0x92=op1h-keypad
0x93=op1h-lever  0x94=op2h-lever  0x95=op2h-wheel  0x96=op2h-firstaid  0x97=opself-firstaid  0x98=throw-gren-1h
0x99=throw-gren  0x9a=idle-crawl  0x9b=wield-crawl  0x9c=jump-stand  0x9d=jump-forward  0x9e=jump-forward
0x9f=action-stand  0xa0=unarmed-cranekick  0xa1=unarmed-cranepunch  0xa2=unarmed-frontkick-low
0xa3=unarmed-palmpunch-double  0xa4=unarmed-roundkick-double  0xa5=unarmed-roundkick-jump  0xa6=unarmed-saltokick
0xa7=unarmed-sidepunch  0xa8=unarmed-sidestrike  0xa9=unarmed-spinkick  0xaa=unarmed-spinkick-double
0xab=unarmed-swipekick-double  0xac=unarmed-capoeira  0xad=unarmed-snakestrike  0xae=unarmed-punch-oneinch
0xaf=unarmed-flyingkick  0xb0=unarmed-flyingkick-double  0xb1=unarmed-powersidekick  0xb2=idle-hover  0xb3=hover-norm
0xb4=hover-fast  0xb5=bow-start  0xb6=idle-bow  0xb7=bow-shot  0xb8=bow-stop  0xb9=jump-land-walk  0xba=jump-land-run
0xbb=jump-land-idle  0xbc=jump-land-walk-2h  0xbd=jump-land-run-2h  0xbe=jump-land-idle-2h  0xc3=idle-swim
0xc4=turn-left  0xc5=turn-right  0xc8=spell-gen  0xc9=spell-dir  0xca=spell-self  0xcb=spell-sys  0xcc=die-crawl
0xcd=crawl_enter-rifle  0xce=crawl_idle-rifle  0xcf=crawl_exit-rifle  0xd0=crawl_impact-rifle  0xd1=crawl_shoot-rifle
0xd2=fallback-start  0xd3=fallback-stop  0xd4=idle-fallback  0xd5=ground-start  0xd6=ground-stop  0xd7=idle-ground
0xd8=chair-start  0xd9=chair-stop  0xda=idle-chair  0xdc=run-left  0xdd=run-right  0xde=run-back  0xdf=spell-ant
0xe0=unarmed-backstab  0xe1=unarmed-charge  0xe2=unarmed-groinkick  0xe3=unarmed-headbutt  0xe4=unarmed-spell1
0xe5=unarmed-spell2  0xe6=unarmed-spell3  0xe7=crawl_enter-smallarms  0xe8=crawl_exit-smallarms
0xe9=crawl_idle-smallarms  0xea=crawl_shoot-smallarmsLh  0xeb=crawl_shoot-smallarmsRh  0xec=crawl_impact-smallarms
0xed=sleep-ground  0xee=idle-sleep-ground  0xef=lounging  0xf0=idle-lounging  0xf1=blade2h_EP3_area_attack
0xf2=blade2h_EP3_1-2-chop  0xf3=blade2h_EP3_dual_attack  0xf4=blade2h_EP3_side_sweep  0xf5=blade2h_EP3_front_attack
0xf6=blade1h_EP3_heavystrike  0xf7=blade1h_EP3_sidesweep  0xf8=unarmed_EP3_faceslap  0xf9=unarmed_EP3_leg_kick
0xfa=unarmed_EP3_slam_heavy  0xfb=unarmed-EP3_bodysmash  0xfc=rifle-burst_EP3_front  0xfd=rifle-burst_EP3_spread
0xfe=smallarms-burst_EP3_2Xpistol_spread  0xff=smallarms-burst_EP3_1Xpistol_front
0x100=smallarms-burst_EP3_2Xpistol_front  0x1f4=die-knees  0x1f5=die-pain  0x1f6=die-poison  0x1f7=die-shot
0x1f8=die-ground  0x1f9=die-float  0x3e8=blade-start  0x3e9=idle-blade  0x3ea=blade-stop  0x3eb=blade1h-slash
0x3ec=blade1h-stab  0x3ed=blade1hlr-stab  0x3ee=blade1hl-slash  0x3ef=blade1hl-stab  0x3f2=smallarms-start
0x3f3=idle-smallarms  0x3f4=smallarms-stop  0x3f5=smallarms-shot  0x3f6=smallarms-burst  0x3f7=smallarms-auto
0x3f8=smallarms-shotl  0x3f9=smallarms-burstl  0x3fa=smallarms-autol  0x3fc=rifle-start  0x3fd=idle-rifle
0x3fe=rifle-stop  0x3ff=rifle-shot  0x400=rifle-burst  0x401=rifle-auto  0x406=unarmed-start  0x407=idle-unarmed
0x408=unarmed-stop  0x409=unarmed-kick  0x40a=unarmed-rswing  0x40b=unarmed-uppercut  0x40c=unarmed-twopunch
0x40d=unarmed-lswing  0x41a=blade2h-chop  0x41b=blade2h-downcut  0x41c=blade2h-slash  0x41d=blade2h-stab
0x41e=idle-2h  0x41f=2h-start  0x420=2h-stop  0x421=walk-2h  0x422=run-2h  0x424=idle-bazooka  0x426=bazooka-shot
0x1770=die-pain  0x1a0a=noanim
```

## 4. Remote position update (c), SetWantedDirection, FollowTarget, Quit

* **Placement** (`CharDCMoveIIR_t`, `FUN_1006bcc6`): dropped if the dynel is unknown or is the client-controlled dynel while its vtable-3 check is false; else `n3VisualDynel_t::UpdateReconcilePos` (stores the *simulated* vehicle position at `dynel+0xa0`, N3 0x19414) then
  `n3Dynel_t::SetRelPosRot(pos, rot)` → `Vehicle_t::SetRelPosRot` [VH 0x1000e2af] (stores pos/rot, `+0xd0` = last server pos, `EnsureSurfaceAlignment(old, true)`).
* **Reconcile smoothing** (`n3VisualDynel_t::Run` N3 0x196bd, constants 0x1003d9c4/c8/cc, 0x1003ce50): every rendered frame `d² = |recon − pos|²`; `0.04 ≤ d² < 1225` (0.2 m .. 35 m) → `recon = 0.8·recon + 0.2·pos` and the mesh is drawn at `recon`;
  `d² < 0.04` → reset (draw at `pos`); `d² ≥ 1225` → draw at `pos` (snap). Per frame, **not** per second; `Mover` normalises it to 60 Hz (`0.8^(dt·60)`) **[the normalisation is a choice]**. The drawn rotation is the vehicle's.
* **Between messages**: the vehicle is simulated with the FSM state above: forward/reverse along the body forward at the mode speed, lateral strafe, yaw turned at the turn rate; no extrapolation limit exists in the client (the server's stop message ends it).
* **FollowTargetIIR_c apply** (`FUN_100732e3`): header dynel must have a vehicle (`dynel+0x50`); if the pos vector is non-zero: `UpdateReconcilePos` + `SetRelPosRot(pos, current rot)`;
  then the message is **dropped** if the FSM mode is 1, 8, 9, 0xB or 0xC (frozen, sit, sleep, lounge); else `controller->vtbl[6](mode)` — `mode` is an FSM id: **21 FullStop, 24 SwitchToWalkMode, 25 SwitchToRunMode** (all live values) — and for NPC vehicles (`+0x21c` set) `FUN_100708a8` clears
  the path; then `FUN_1006fe02(target, speed, path)`: stores the follow target (`+0x180`, short form = the dynel itself, long form 0:0 = none), `SetRelPosIgnoreCollision(pos)`, and copies the (≤ 30) waypoints to `+0x190` (zero-terminated). Another gate (`this[0x21c] == 0` block) drops the message when the Features bit 0 / 0x4000000
  tests on the *client character* or on the target pass (`FUN_10058816` is the own char; semantics unresolved, not applied). `speed` is not used by the follow code.
* **Steering** (`FUN_1007098d` = NPCVehicle `CalcSteering`, vtable +0x64): frozen → `SteeringHalt`; follow target set → `SteeringDirArrive` towards `FUN_1007022d(0)` = **waypoint 0** (`+0x190`; `FUN_10070019` pops it when the horizontal distance < the arrival radius — the radius comes from an unresolved virtual (`+0xb0`), `Mover` uses `max(vmax/4, 0.5)` **[GUESS]**; empty array → stand still); no target → the `Path_t` at `+0x360`
  (server paths of other kinds): empty → halt, 1 point → seek, more → follow the `PathGuide_t` point. `SteeringArrive` [VH 0x1000ab28]: desired speed `min(vmax, dist/(vmax/4)·vmax)`, halt inside radius 0.2 (`SteeringDirArrive`, VH 0x10012298) or `d² < 0.01`. When the vehicle starts to move `FUN_1006ef34` runs transition 1 (ForwardStart) or 3, and
  `FUN_10070c00`/`FUN_1006ef54` run 2/4 (ForwardStop/ReverseStop) at the end of the path, so a path walk plays walk/run by mode and returns to idle at the destination.
  Live: short form with ≥ 2 points + mode 24/25 = walk/run along `[current pos, destination …]`; long form mode 21 with one point = `FullStop` + placement at the server's current position (the captured mode-25 runs of the ICC guards are interrupted by it 0.3–0.7 s after they start; walks of other NPCs run 5–20 s).
* **SetWantedDirection** (`FUN_1003a78a` stores `dir` at `SimpleChar_t+0x1f8..0x200`): read by `FUN_1007c137` (returns `dynel+0x1f8` as the "target direction" of the char's AI state when its target identity is null), `FUN_1007c24a` turns it into `(|angle|, sign)` between the dynel's global forward and the vector (`atan2`), stored in the
  state object (`+0x10`, `+0x18`) that the turn animation uses. Live it always precedes the FollowTarget that starts a walk by a few ms and equals the direction `(dest − pos)` of that path (NPC 1002060: `(0.368,0,0.930)` vs path `(2.5,6.3)/6.8`), once without a path (WD `(-0.97,0,0.24)` at 60679 ms alone). `Mover` turns in place towards it at 3.5 rad/s with the turn clips **[GUESS on rate]**.
* **n3ToClientQuit** (N3 0x1002a043): `Die` on the dynel unless it is owned by this engine → remove the `Mover`.
* **Ground (d)**: the client does not trust the server y for a hugging vehicle: `Vehicle_t::EnsureSurfaceAlignment` [VH 0x1000d1aa] is called by every `SetRelPos*`/`SetRelPosRot` (with the old position) and every `Run` sub-step against the playfield `Surface_i`
  (`vtbl+0x20` probes along the move, 3-point sampling `+0x10`), keeps the position at least on the surface (`y = max(y, surface y + offset)`), and while the "falling" flag (`+0x52`) is set applies `s_vGravityAccel`; the hug flag `+0x50` is cleared in fly mode (`DisableFalling`) and set by `EnableFalling`.
  `SetRelPosIgnoreCollision` skips the immediate query, not subsequent vehicle substeps. Remote `Mover` now retains the existing `SurfaceState` and supplies `Body` to `Collision::align` through `Dynels::set_collision`: initial full update, DC placement, in-playfield teleport and nonzero FollowTarget placement align before their pose is exposed; every 1/60 s integration substep aligns again. Wire Z stays positive; the app mirrors Z only at the collision boundary. Grounded paths steer in X/Z rather than interpolating server Y; unsupported bodies fall with gravity −20 [VH 0x1001938c], vertical speed limited to ±50 [VH 0x10012748], and land through JumpStop. Fly/no-fall bodies retain legitimate airborne height and still collide with surfaces.
  Jump height/ceiling and Flags switches use the evidence in movement.md §§7,10 (GC 0x1005844d, 0x1006f9e9, 0x10059e6a); the NPC minimum affects stored jump state, not launch impulse (§4 native slot evidence below). Horizontal reconcile thresholds/decay and path popping are unchanged; the draw reconcile's Y follows the resolved body Y rather than reintroducing the old unsupported height. This vertical-only reconcile projection is an explicit port correction, not a separately traced retail rule. Geometry-free capture consumers retain planar dead reckoning; only the loaded-world callback executes surface/vertical physics.
  Mode transitions execute Fly Apply lift (+0.5 m), exit lift (+0.1 m for Walk/Run/LeaveFly/Swim/Frozen), and elevate up/stop speeds (3/−0.8 m/s). Initial serialized FSM bytes are restored directly, without executing Apply or replaying lifts. Falling is retained independently of the FSM mode: full-update Flags hooks run before velocity/FSM/input restore (GC `10077af2` → vehicle `+0xac`, `1007135d` → `1006f03e`), so an initial Fly FSM does not itself disable falling or erase serialized vertical velocity.
  Features bit 8 also disables falling (grant/revoke callbacks GC 0x10044842 / 0x10044a07, movement.md §10.1). Full-update PC Strength/Agility come from `PcData.stats[1/2]` (muted SetStat GC 0x10058ca8, dynel.md §3), not default-zero jump attributes.
  Regressions: `remote_ground_downhill_fall_jump_and_fly`, `captured_remote_movement_runs_surface_on_placements_and_substeps` replay the existing `ao_net::n3::capture` traffic against a deterministic surface, and `captured_remote_dynels_align_with_loaded_playfield` exercises the wire/scene adapter against pf 4582 collision records.
  Native slot evidence: SimpleChar primary vtable `1015fa94+54` = player factory `100572df` (no own-character gate); player primary `10160bbc+8c` = `1012ff2c` (true), NPC primary `10160aec+8c` = `100a078c` (false). Thus remote players pass the player-input Apply gate too. Elevate Apply `1006d948/1006d99c` writes through `100712f7`; Fly-exit reset is gated at `1006e115`, `1006dba4`, `1006dd0c`, `1006e4d8`. Both vehicle tables' `+2c` jump slot is `1006f9e9`, reached by `1006d792`. Its NPC minimum 1.5 modifies stored `+164` only, not the local height used for the launch impulse.
  Missing jump stats retain `world::STAT_UNSET` (`0x499602d2`), not zero: character stat interface `1015f9ec+3c` = `10058e52`, kind 2 forwards to inner StatHolder `10158c34+4` = `1002e46a`; absent indices return the sentinel, and expansion `1002e3c4` fills it. `1005844d` explicitly compares GmLevel to zero, so missing GM is nonzero and does not activate its 800 cap. PC Strength/Agility are overwritten by the packet; later stat updates overwrite each retained value.

## 5. API (`ao_net::n3::motion`)

```rust
MoveType::{from_id(u8)->Option<MoveType>, name()->&str, ALL}      // §1 table
Mode::{from_u8}  Status{mode,prev_mode,forward,strafe,turn,jumping,elevating}  Status::{default(), from_blob(&[u8]), transition(MoveType, bool)->bool, is_moving()}
AnimState::{anim_id()->Option<u32>, clip_name()->Option<&str>, fallback()->Option<AnimState>}   anim_of(&Status)->AnimState
max_speed(Mode, back:bool, skill:f32)->f32   strafe_speed(Mode, skill)->f32   facing(yaw)->[f32;3]
Mover::new(pos:[f32;3], yaw:f32)   Mover::with_blob(pos, yaw, &blob)         // from SimpleCharFullUpdate (+ on_path(has_target, &waypoints) for its `path`)
Mover::set_features(i32)  set_sit_allowed(bool)  set_body_scale(f32)  set_body_radius(f32)  restore_blob(&[u8])  on_stat(stat:i32, value:i32)  // Flags, Strength, Agility, GmLevel, MonsterScale
Mover::on_char_dc_move(&CharDCMove)  on_wanted_direction([f32;3])  on_follow_target(&FollowTarget)->bool  on_move_type(u8)->bool
Mover::advance(dt:f32)->Pose{pos,yaw,anim,speed}   pose()   sim_pos()   status()   path()   skill()
Mover::advance_with_surface(dt, align(old,new,&Body,&mut SurfaceState)->Aligned, ceiling(pos)->Option<f32>)->Pose
```
Typical loaded-world wiring: on `SimpleCharFullUpdate` create `Mover::new(c.pos, c.yaw().unwrap_or(0.0))`, feed speed/health/Flags and body properties, the record's Features, then `restore_blob(&c.blob)` and the full-update path. Feed every later DC/Teleport/FollowTarget/WantedDirection/Stat to it. Resolve placements with `advance_with_surface(0, ...)` before exposing their pose and advance each frame with the same loaded collision adapter; draw the resolved `Pose.pos`, `Pose.yaw` and `anim.anim_id()` without a second terrain clamp. Geometry-free tools use `advance(dt)` for planar dead reckoning. Remove on `ToClientQuit`.

## 6. Name tags (`ao_net::n3::nametag`)

* **Where**: GUI.dll `TargetingModule_t` (vftable GUI 0x101ae12c). `FrameProcess` (0x10025fa4) calls `HandleNametags` (0x10025d96) every frame; `Indicator_t` (vftable 0x101ae0d4, ctor `FUN_100255be(identity, attacking, healthbar)`) is both the nametag and the selection/attack indicator.
  Nametags (`Indicator_t(id, 0, 0)`) exist only while pref **`ShowAllNames`** (IndependentPrefs; `cd_image/gui/Default/OptionPanel/Root.xml:331` checkbox, label `#ShowAllNames`, category Login; callback `ShowAllNamesCallback` 0x100261c3: 0 → delete all, 1 → create the set and force a rebuild) is 1. Default is **1**, set by `SetDefaultLoginPrefs` GUI 0x10124b33 (`InitDefaultInt("ShowAllNames",1,0,1,Login)`), not inferred from absence in XML.
  org line pref **`IsOrgNameShownOverHead`** (`Root.xml:332`, category Char, callback 0x10026237 sets the static flag and rebuilds). Without ShowAllNames only the targeted/attacking dynel carries an indicator (`GFX_GUI_INDICATOR_SELECTED` 0xe6 / `_ATTACKING` 0xe5 plate, health bar).
* **Which dynels, how often**: every 2.0 s (`_DAT_101ae17c`; timer accumulates `Timer_t` frame time, reset to 100 on option change = immediate) `N3Msg_GetDynelsInVicinity(kind 50000)` [GC 0x1001df09], radius **30 m** (`_DAT_10157500`) around the client character, excluding it, requiring InPlay (stat 0xC2) ≠ 0 and Features bit 0x80 clear; tags of dynels no longer listed are deleted; the selected/attacked one keeps its own indicator.
  Per frame `FUN_10024d5b` shows the tag while `N3Msg_IsVisible` and the dynel has a position and no parent. **No distance fade or scaling code exists**: the tag is a world-space `VisualSprite_t` (billboard `width/128` × 0.3 world units, so it shrinks with perspective), priority 6.
* **Font/plate**: `TextOutput_t(font 2, 0x800, sprite)`: font id 2 = **`FontGameShell12.fnt`** (`textures/fonts/`, GUI table 0x10272e18; 'verdana' TTFs are other ids); sprite 32 px high, 128 px wide, 256 if the text is wider than 128 px, surface format 2 (green-keyed), cleared transparent; plate bitmaps only for indicators with the flag (nametags: text only). Main text printed with flags `(0,1,…)`, org line with `0x13`.
* **Text** (`FUN_10024e14`, GUI 0x10024e14; `name_tag`): `[title ]` (`N3Msg_GetTitleName`) + `[first name ]` (if stat `Flags` bit 22) + name — non-NPC: `** name **` if `Features & 0x4000001` (valid), else `= name =` if `VisualFlags` bit 9 (0x200), else `name`; NPC: plain — `[ last name]` (bit 22) + `[ the <breed>]` (non-NPC, stat ShadowBreed 532 valid and ≠ 0).
* **Colours** (`ColorCode_e`, `FontSystem_t::SetColorsIntoMap` 0x1012e316, table in `color_code_rgb`): `Flags>>22` bits 22/23 → code 31 (`CCShowFullNameColor` 0x00ee00) for bit 22 only, code 10 (`CCLinkColor` 0x2299ff) when bit 23 is set (table 0x1026349c); else non-NPC with stat 345 valid ≠ 0 → code 12 (red 0xff0000); else the base colour **white 0xffffff**
  (red 0xff0000 for a non-NPC in a battle-station zone whose stat 0x29C differs from the client character's). **There is no faction (side) tint of the name, no NPC hostility/level colour and no team/org/pet colour** (searched: all of `FUN_10024e14`, `FUN_10024af1`, `FUN_10024c03`, `FUN_10024d5b`, `FUN_100255be`; `TextOutput_t::Print` 0x10023581 only maps the colour code 0x10+code through `FontSystem_t::GetColor` and draws the glyphs with the default colour `TextOutput+0x30` = `Indicator+0x34` = 0xffffff; the sprite material is `VisualMaterial_t` 0x1006b42b: white diffuse/specular/ambient, black emissive, opacity 1: no tint; the `Consider` colour `Indicator+0x28` is read only by `FUN_10024c03`, the bar). The **org line** is `0x8888aa` (clan, side 1), `0xaa88aa` (omni, side 2), `0x88aaaa` (neutral), printed at y = 19 (the name at y = 1), both centred in the sprite (`TextOutput` flag 0x800, `Alignment` 0x231d8); the org line only with pref `IsOrgNameShownOverHead` (`s___AVTextLine_t___10263404[0x12]`) and a clan string (`N3Msg_GetClanString`).
* **Position**: anchor = attractor 0 `Attractor01_head` (`AttractorMesh::GetName(0)`, DS 0x10071ca2; `VisualCATMesh_t::GetIndicatorPosition`, DS 0x100731e2) local position **+0.5 m up**, transformed by the mesh world matrix (so it scales with `MonsterScale`; if the attractor is missing/zero the local point is `(0, 1.5, 0)` before the +0.5, DS 0x10089e38); non-CAT meshes: `GetMeshHeight + 0.3` (DS 0x1006c3b0). `FUN_10024aab` also stores
  `N3Msg_GetMeshHeight (or 1.9) + 0.4` at `Indicator+0x24` (used for the selection ring height, not the nametag). `name_tag_offset`, `name_tag_world_pos`.
* **Health bars over heads**: only on the target/attack indicators (flag byte `+0x31`): `FUN_10024c03` redraws a **64×4 px** bar at y 14..18 centred in the 128/256 px sprite when Health or MaxHealth (stats 27 / 1) changes: filled part `health·64/max` in the *consider* colour, rest `0x333333` (`health_bar_fill_px`).
  Consider colour (`FUN_10024af1` with `N3Msg_Consider` [GC 0x17496], a `SimpleChar_t` returns 3 with ratio `(range − own_level + target_level)/(2·range)`, `range` = stat 0x113 of the client char, −1 if `own − range > target`): ratio < 0 → grey 0xaaaaaa; < 0.5 → `(255·2r, 255, 0)`; ≥ 0.5 → `(255, 240·2(1−r), 0)`
  (`consider_ratio`, `con_color`). Ignored characters (`IgnoreSystem_t`) get the `GFX_GUI_IGNOREDICON` (0xe4) stamped on the plate.
* **When each indicator exists** (`ao_net::n3::nametag::IndicatorKind`): three creators of `Indicator_t` only. (1) `HandleNametags`: `(id, 0, 0)` nametags = no plate/bar, only with `ShowAllNames`, excluding selected/attacked dynels at rebuild. (2) `SetTarget` 0x100257b0: `(id, 0, 1)` selection plate 0xe6 + bar unless stat `Flags` bit **0x400** is set. A non-forced target is rejected when `N3Msg_isIDOnGround` [GC 0x100167a8] **fails**, not when it succeeds; selecting `(0,0)` removes the indicator. (3) `FrameProcess` 0x10025fa4: `(id, 1, 1)` attack plate 0xe5 + bar for `N3Msg_GetAttackingID` [GC 0x10026964] while the fight controller state `+0x44 != 1`, recreated on target change and removed on fight stop. `FightingTargetMessage` 0x10025947 only calls `N3Msg_Consider`. All share sprite builder 0x10024e14 and per-frame 0x10024d5b; visibility, position and no-parent gates apply, and the anchor must have server x > 0.
* **Hover**: there is no tag on mouse hover. The "object under the mouse" (`InputConverterModule_t::VoidObjectUnderMouseMessage` 0x1001bc71 → `InputConfig_t+0xc8`, `InputConfig_t::CheckObjectUnderMouse` 0x10019f00) only chooses the mouse pointer (`MousePointerModule_t::SetMousePointer`: attack / friendly / neutral cursors by NPC-ness, `Side`, `Features & 0x20000000`, fight mode) and the `AFCM` 0x1e/0x126 notification for objects with `skill 0x1e & 0x100000`; `Indicator_t` is never created there (callers of `FUN_100255be`: `SetTarget`, `HandleNametags`, `FrameProcess` only).
* **Sprite and billboard** (renderer): the 32 px sprite is composed as `FUN_10024e14` does: plate halves copied to the ends with green key 0x00ff00, then text and health bar. `VisualSprite_t(width·0.0078125, 0.3, mat)` = `RSprite(mat,0,w,h,root,SpriteMode 1)` (randy31 0x100135f1): half extents `size·0.5`, pivot zero centred on anchor, rotation 3.14 cancels flipped UVs (`FUN_100132b3`), mode 1 copies camera rotation and restores sprite translation (`FUN_10013ab2`). Dimensions are `(width/128) m` wide and `0.3 m` high.
* **Filtered-edge root cause (2026-10-06)**: GUI 0x10024e14 names the material **`[3] TargetIndicatorMat`**. randy31 material preset resolver 0x10040645 parses `[3]` and sets **ALPHABLENDENABLE (0x1b) = 1**. `RSprite` 0x10013575 adds ZWRITE=1, ALPHATEST=1, ALPHAFUNC=GREATER, ALPHAREF=30 without disabling blending. The former aomac opaque `Blend::AlphaTest` with alpha multiplied by 4.25 admitted low-coverage bilinear edges but output them fully opaque, thickening strokes. Tags now use `Blend::AlphaBlend` plus `Submesh::sprite_alpha_test`: real unscaled filtered alpha, strict `>30/255` cutoff and depth writes in the priority-6 blended phase. This corrects the proved retail state mismatch without name-based deduplication.
* **Bitmap tint correction**: GUI `TextOutput::Print` 0x10023581 routes the font-2 bitmap through DS `SpriteInfo::Copy` 0x1007ae11. For destination surface format 2, Copy tests the source `0x8000` opacity bit and writes the converted text tint with `0x8000`; it does not multiply by the source grey intensity. `tags::raster` now treats every nonzero glyph-mask sample as full tint; the binary-mask regression includes grey input. Texture filtering supplies fractional edge alpha later, handled by the blended sprite pipeline.
* **Sizing recheck**: fresh traces GUI 0x10025d96/0x10024e14, DS 0x1006ec9b/0x1006ed42 and randy31 0x100135f1/0x10013ab2 confirm no screen-space clamp. Actual constants: GUI 0x101ae0ec = f32 `0.3`; double 0x101ae108 = bits `0x3f80000000000000`, `0.0078125`. Bitmap glyphs copy directly into the 32 px surface (`TextOutput::Print` 0x10023581), centred by `Alignment` 0x100231d8. World perspective growth is retained rather than introducing unsupported pixel limits. The rendering/tint root is corrected above; only the screen-size interpretation remains unproven without a matched-distance retail comparison.
* **Post-fix visual evidence (2026-10-06, Fix8World)**: inspected isolated `tags.png` near/far labels were crisp. The live near Beach Leet label was partly occluded by the own avatar, consistent with the traced depth-tested/written sprite; this was not treated as a font-size clamp or duplicate-label defect.
* **Remaining differences**: plain tag suppressed immediately for an indicator instead of at the next 2 s rebuild; at most 96 tags (nearest first); ignored-icon stamp unavailable; indicators over non-character dynels not yet drawn. Listing now gates InPlay and display gates visibility/position/no parent.


## 7. Unresolved / not found

* Whether the retail client really sits `Features&4` NPCs down on the spawn t30 (see §1); `Features` of players (inferred).
* The FollowTarget gate on the client character's Features (bit 1 / 0x4000000) and the waypoint arrival radius (virtual `+0xb0`); the strafe speed for remote players; the fly-move clip; turn rate of NPCs (the turn-in-place rate 3.5 rad/s is the player's).
* `SimpleChar_t+0x1f8` consumer beyond the AI state angle (`FUN_1007c24a` caller chain: state ids 0/4/5/7/10/11 of `FUN_1007be9f`).
* `FUN_1007022d`'s helper functions' exact semantics, DC `time`/`extra` (always 0, type 22 hook is a no-op for SimpleChar).
* Frame-rate normalisation of the reconcile factor (client: once per rendered frame).
