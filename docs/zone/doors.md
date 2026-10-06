# Doors: state messages, animation, sounds

Code: `crates/aomac/src/play/dynels_doors.rs` (pure rules + the prop glue), `Dynels` in `play/dynels.rs` (messages, per-prop state, skin), decoder
`ao_net::n3::world::{Door, DoorStatus}`, node animation `ao_formats::mesh::NodeRig` (`crates/ao-formats/src/mesh/rig.rs`), sound lists
`ao_formats::dynel_visual::ItemTemplate::sounds`, `ao_audio::Audio::play_game_sound`. Addresses: `GC` = Gamecode.dll, `DS` = DisplaySystem.dll,
`R` = randy31.dll, `N3` = N3.dll. Ghidra projects: `/tmp/aomac-ghidra/playfield` (P_Gamecode, P_N3, P_DisplaySystem, P_GameData), `/tmp/aomac-ghidra/mesh` (randy31).

## 1. Summary

A door is a `Door_t` (identity kinds 0xC748, 0xDAC6, 0xC73A, `DoorRibosome_t`; docs/zone/static.md §3) that the playfield places itself (rdb 1000026,
docs/zone/static.md §7) or the server creates with a `DoorFullUpdateIIR_t`. Its mesh is an ordinary rdb 1010001 `.abiff`, but **the door pieces carry
keyframe animations** (`FAFAnim_t`: rotation and translation keys on the frame nodes; docs/formats.md listed them as "keyframes, ignored"). Opening or
closing a door is the server's `DoorStatusUpdateIIR_t`; the client reacts by playing the mesh animation forward (open) or backward (close) once, plus a
sound from the door template. There is no client-side rule that opens a door by itself.

## 2. Messages

| id | class | decoder | evidence |
|---|---|---|---|
| `365A5071` | `DoorFullUpdateIIR_t` | `world::Door { base: DynelBase (version3 = 2), value_78 }` | vtable GC 0x10166aac; slot 7 `FUN_1009fa37` = `FUN_100a0730` (dynel base chain, versions 2 / 11) + `u32 2` (`DAT_101c0ff8`) + `i32` at `this+0x78`; slot 8 `FUN_1009fa81` writes it; slot 2 creates the dynel: `FUN_1009faaf` |
| `4C7D403B` | `DoorStatusUpdateIIR_t` | `world::DoorStatus { locked, open, value_c3, flag_1a, stats }` | vtable GC 0x10166ae0; slot 7 `FUN_1009fde6`: `u32 2` (`DAT_101c1020`), `u8` (`== 1`) at +0x18, `u8` at +0x19, `i32` at +0x1c, `u8` at +0x1a, stat list `FUN_1002d8bd` at +0x20; slot 8 `FUN_1009fcb8` writes it; slot 2 is the apply `FUN_1009fc10` |

Both are S→C (`n3InfoItemRemote_t::ToBePassedOn`); the header identity of the status update is the door. **Neither occurs in `docs/captures/zone_ithaca.rec`**
(searched for both ids in every capture): the capture's character never was near a door state change, so the layouts are code-derived (the decoder is
checked against a hand-built body written the way `FUN_1009fcb8` writes it, and the full update against the captured vending machine's chain edited to
`version3 = 2` + one `i32`). `DoorFullUpdate.value_78` is stored only; nothing in the apply path reads it **[UNRESOLVED]**.

The client sends nothing door specific; using a door goes through the generic item use (docs/zone/outgoing.md).

## 3. Apply path (what `Door::status` does)

`FUN_1009fc10` (the door is found with `n3Dynel_t::GetDynel`, `__RTDynamicCast` to `Door_t`; a missing door ignores the message):

1. `locked == false`: `FUN_1007ef1e` clears `Flags` bit 0x40 (via `FUN_100850c9`). `locked == true`: `FUN_1007ff80`: an **open** door (`Flags` bit 0x80) is closed
   first (`FUN_1007fe6a`), then `FUN_100850c1` sets bit 0x40.
2. `flag_1a`: `this+0x1d5 = 1`.
3. `open == false`: `FUN_1007fe6a` (close) then `FUN_1007ef90`; `open == true`: `FUN_1007fdcf` (open) then `FUN_1007ef56`.
4. `SetStat(0xC3, value_c3)`.

`FUN_1007fdcf` (open): `PlayGameSound` at the door (sound list key 0x83, else 0x64), `FUN_1007ef56`, then `FUN_100850f3`: `Flags |= 0x80`,
`SetStat(StateAction 0x62, 100)`, and if the dynel `HasMesh`: `FUN_100872a2(1)` (AnimPos 1: animation time := 0), `AnimPlay := 3`, `AnimSpeed := 3`.
`FUN_1007fe6a` (close; does nothing at all when stat 259 (0x103, unnamed) has bit 0x40): sound key 0x84, else 0x66, `FUN_1007ef90`, then `FUN_1008513a`: if the
mesh exists **and the door is open**: `FUN_100872a2(2)` (animation time := total time), `AnimPlay := 5`, `AnimSpeed := 3`; `Flags &= ~0x80`, `StateAction := 102`.
`FUN_1009faaf` (DoorFullUpdate creates a new dynel; nothing if it exists) calls the open path when `Flags` bit 0x80 is set. **Placed doors** (`CreateRDBDynels`
GC 0x10121dcb: `CreateFromTemplate`, blob stats, `vtable+0x7c`) run no such call: they start at animation time 0 whatever their `Flags` (13 of 7145 placed doors have
bit 0x80 [DATA]; 688 have 0x40).

Port: `Door::{open, close, status}` return the `Effect`s (sounds; the two room monitor calls) and move the `ItemAnim`; tests in `dynels_doors::tests`.

## 4. Animation

**Clock** (`FUN_10088426` GC, the `SimpleItem_t` per-frame function, called from the item's run function `FUN_100899c0`): with the mesh present, `dt` = the frame time
(`*(m_pcInstance+0x68)`), by `AnimPlay` (stat 501): 2 `time += dt` forever; 3 `time += dt` until `total <= time + dt`, then `FUN_100872a2(2)` (time := total), done flag, `AnimPlay := 1`;
4 `time -= dt` forever; 5 until `time - dt <= 0` (`_DAT_10155e58` = 0.0), then time := 0, done, `AnimPlay := 1`; anything else idle. `FUN_100872a2(pos)` (`AnimPos`, stat 500) calls `VisualMesh_t::SetLoop(false)`,
stores the stat and sets the time to 0 (pos 1) or `GetAnimationTreeTotalTime` (pos 2). `total` is the longest `tot_time` of the mesh's nodes (`RRefFrame_t::GetAnimationTreeTotalTime` R 0x10045728).
`ItemAnim` is exactly this. Because it is the generic item clock, **every item with an animated mesh runs it**: vending machines carry `AnimPlay = 2` (loop) in their template
and now animate (docs/zone/static.md §6: "what AnimPlay / AnimPos animate" resolved). The door's state machine (stat 450 `StateMachine` = 1, rdb 1000015 record 1, `StateMachine_t::GetStateMachine` GC 0x10084566)
leaves its idle state on `StateAction == 100` and runs a spell of type 0xCF21 whose three arguments are the stats 0x29 / 0x2a / 0x2b = `FUN_100a51e5`'s `FUN_100872a2` / `AnimPlay` / `AnimSpeed`
setters (GD `SpellFormats_c` 0x1000fb0a, type 0xCF21): the same three values `FUN_100850f3` / `FUN_1008513a` set directly, so the port applies the direct calls and does not run the machine
(the machine would restart the same animation one frame later).

**Keyframes** (`RKeyFrameAnimation_t`, archive `FAFAnim_t`: `tot_time`, `loop`, `rot_keys` 20 byte `{x, y, z, w, time}`, `trans_keys` 16 byte `{x, y, z, time}`, `vis_keys` 8 byte, optional `uv_keys`; writer R 0x10028c87):
`VisualMesh_t::SetAnimationTime(t)` (DS 0x1006b899) → `RRefFrame_t::SetAnimationTime(t, true)` (R 0x1004506b) for every node: `t' = fmod(t, node total)` (an exact multiple stays at the total), then the animation's `FUN_10028fde` (R 0x10028fde):
non-looping clamps `t'` to the total, the translation is the lerp (`FUN_10028c3d`) of the two bracketing keys, the rotation the slerp (`FUN_1006eefa`, short arc, plain `(1-t, t)` weights below `1e-6`)
turned into rotation rows by `FUN_1006e393` (the same function as `local_rot`), written into the node's animation matrix. `UpdateWorldMatrix` (R 0x10044fa2) then makes the node's local matrix
`M_anim * (R(local_rot) * scale, translation local_pos)` (`FUN_1006e302`: `out = M_anim * local`), `world = local * parent_world`. The archive's `anim_matrix` member is the animation matrix as it was when
the file was written, i.e. the pose at time 0 (`RRefFrame_t::Archive` R 0x10044da8 writes `*(this+0x40)+4`); the static decoder bakes it in, so **a door at time 0 is exactly the static mesh**
(test `door_meshes_pose_to_the_static_decode_at_time_zero` over the meshes of every placed door of the data). Exception: mesh 224248 repeats `local_rot` in the rotation key of its node 1, so the engine's formula applies it twice once the clock runs
(the port follows the engine formula; the test names that mesh).
Door meshes (e.g. rdb 1010001:245910 `door_omni_med_blue2` as used in 4582): 7 nodes (`grating_bottom`, `grating_top`, `dor_top`, `dor_left`, `dor_right`, `dor`, a collision sphere), `tot_time` 1.6667 s, the panels slide up to 1.67 m
(`placed_door_animates_on_status_updates`); 41798: 0.6 s. Visibility / uv keys are not applied. `VisualMesh_t::RemoveDoorDir` (DS 0x1006bf24, called from the door's `MeshReady`, the `n3VisualDynel_t` sub-object vtable GC 0x10162444 slot +0x98 = `FUN_1007ef03`) only hides
`RRefFrame_t`s named `door_dir` (no geometry).

Rendering: `Built::item` (`ItemRig`: the `NodeRig` + the item's stats) is built next to the static model; per prop `PropAnim` creates the state when the model is in (messages that came earlier replay silently), steps the clock every frame
(also out of view), and hands `ActorFrame::skin` the posed vertices (positions and normals; uv unchanged) when the pose changed, the rest vertices when the door is shut again, and re-sends them when the renderer forgot the actor.

## 5. Sounds

The sound lists are the template's element of type `0x14` (`FUN_1002b297` GC 0x1002b297 → `FUN_1007d6d5`, the NPC record's multimap format: `n × {key, m × Sandy sound id}`; docs/zone/npc.md §4), e.g. template 41565: key 0x83 → 0xcfde8382,
key 0x84 → 0xc16f0487. `FUN_1004570c` (GC 0x1004570c) takes the list of a key at the dynel's `+0x88` and picks `rand() % count` when `count > 1` (the port uses the CRT `rand` emulation of `Dynels::rng`; open: key 0x83 then 0x64, close: 0x84 then 0x66).
Door templates: 95 placed, 45 carry sounds (keys 100 / 102 / 131 / 132) [DATA, `door_templates_have_open_and_close_sounds`]. The sound is `SandyInterfaceModule_t::PlayGameSound(id, door position, 0, 1.0, 0, 0, 100, 1)` (`FUN_1001117d`): one-shot,
level by distance only (docs/formats.md audio): `Audio::play_game_sound(id, pos, listener)`; `flow.rs` plays `Dynels::take_sounds()` after the dynel update.

## 6. Not ported / open

* **Room doors of dungeons**: `n3RoomMonitor_t::DoorOpened/DoorClosed` (N3 0x10013561 / 0x10013910, called by `FUN_1007ef56` / `FUN_1007ef90` with the door's room link `+0x1d0`) → `n3Playfield_t::ChangeRoomStatus` and
  `IsDoorOpenBetweenRooms` (N3 0x1000d1e9) decide whether a character may pass between two rooms (docs/zone/collision.md §3: "every transition is allowed" stays **[GUESS]**). The link comes from `Door_t::LinkDoorToRooms`
  (`n3Room_t::GetDoorLinkFromPos` N3 0x100105f9 on the door position; unresolved doors are "killed") which is not traced. `Effect::RoomOpened/RoomClosed` are produced and ignored.
  Consumer on the camera side (done, docs/zone/camera.md §7): `IsDoorOpenBetweenRooms` hides a scripted-view attractor in another room while the link's door is not open (`Collision::door_open_between`, fed by `Collision::set_door_open`,
  which nothing calls yet: **every link counts as closed for the camera** until `RoomOpened` / `RoomClosed` are wired to it).
* **Door collision**: doors "want collision information" (`TellCollision`) and their mesh has animated `FAFCollisionSphere` nodes (the sphere follows the panel), but the client's dynel-vs-dynel collision is not ported for any prop.
* The door state machine (rdb 1000015) is not interpreted beyond the animation spell above; `value_c3`, `flag_1a` (`+0x1d5`) and `DoorFullUpdate.value_78` are stored/ignored.
* `StateAction`/`Flags` are kept in `Door` but nothing reads them besides the open / locked tests; a locked door does not block anything client side.
