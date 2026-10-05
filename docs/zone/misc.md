# Zone N3 messages: movement path, combat, buff, generic command, despawn, in-play (+ ptype 5 / 0x43 / ping review)

Decoders: `crates/ao-net/src/n3/misc.rs` (`misc::decode`, `Misc::encode`, `misc::key`). Evidence: the capture
`docs/captures/zone_ithaca.rec` (80 s, Ithaca zone, character 25988 "Testy") and the client DLLs, Ghidra projects under
`/tmp/aomac-ghidra` (`playfield/P_Gamecode`, `P_N3`, `proto/proj`). Address tags: `[GC]` Gamecode.dll, `[N3]` N3.dll,
`[IF]` Interfaces.dll, `[MP]` MessageProtocol.dll (image-base VAs).

## 1. How the message id is derived (resolves the id → name question for every N3 message)

The id is not a constant anywhere in the DLLs (byte search of all `*.dll`/`*.exe` for every captured id, little- and
big-endian: no hit). It is a hash of the **class name string**:

```
n3InfoItemRemote_t::MapToKey(const std::string&)   [N3 0x10009826]
  key = 0;  for i, c in name:  key ^= (int32)(int8)c << ((i & 3) * 8)
```

Every `*IIR_t` / `*IIR_c` / `GenericCmd_t` class registers itself with `n3InfoItemRemote_t::Register(name, factory)`
[N3 0x100099e4] (example: `HealthDamageIIR_t` registered at [GC 0x1000f494], factory [GC 0x100a023b]). The reader
`n3InfoItemRemote_t::Construct(uint, BinaryStream&)` [N3 0x10009b08] does: `i32 key` → registry lookup → factory →
`Identity` (`FUN_10037d8a`) → `u8` (must be 0 or 1, else the message is dropped; stored in object +0x0c) → virtual
`ReadSubClass` (vtable +0x1c, "slot 7"); a stream error raises `DataStreamException_t("Short read")`. The writer
`n3InfoItemRemote_t::Write` [N3 0x100098f4] emits the same four parts (slot 8 = subclass writer). Names found by
hashing every `[A-Za-z0-9_]+IIR_[tc]` / `GenericCmd_t` string in Gamecode.dll/N3.dll (`misc::key` + test
`keys_are_class_name_hashes` reproduce the ids, also `PlayfieldAnarchyFIIR_t` = 5F4B1A39 and `CharDCMoveIIR_t` = 54111123).

The **flag byte** after the identity is `n3InfoItemRemote_t` +0x0c, accessors `ToBePassedOn` / `ClearToBePassedOn`
/ `EnablePassOn` [N3 0x100097fa..10009814] (a relay flag, default true in the ctor [N3 0x10009860]). Every apply routine below
calls `ClearToBePassedOn` first. Live: 1 only for `CharInPlay`, 0 for everything else.

Per-class vtable layout (from `vftable` at the listed address; slots are 4-byte indexes):
slot 1 = validity check (`1` ok / `3` reject), slot 2 = apply (what the client does with the message), slot 4/5 =
`IsAllowedFromClient` (true for the messages the client itself may send), slot 7 = `ReadSubClass`, slot 8 = `WriteSubClass`.

Frame `sender` seen for these messages is not part of the N3 message; observed: 3502 (Missed), 3503 (Attack, SecSpecAttack,
SpecialAttackInfo), 3504 (Quit, GenericCmd), 33402 (Buff, CharInPlay) and the header dynel instance for
AttackInfo/StopFight/FollowTarget (1 for FollowTarget). Meaning of the 350x values unresolved.

All coordinates are floats exactly as on the wire (Y = height).

Summary (captured counts):

| id | class | vftable [GC] | captured | meaning |
|---|---|---|---|---|
| 260F3671 | `FollowTargetIIR_c` | 0x10160f18 | 173 | server-driven position + waypoint path / follow target for a character |
| 28494070 | `AttackIIR_t` | 0x1016196c | 128 | header dynel starts attacking `target` |
| 46002F16 | `AttackInfoIIR_t` | 0x10166950 | 134 | a hit landed: damage etc. |
| 4A41203E | `StopFightIIR_t` | 0x10161620 | 128 | header dynel stops fighting |
| 36510078 | `n3ToClientQuitIIR_t` | [N3] 0x1003e6b4 | 27 | dynel leaves the client's world (despawn) |
| 5C654B28 | `MissedAttackInfoIIR_t` | 0x10166c0c | 11 | an attack missed |
| 52526858 | `GenericCmd_t` | 0x10161994 | 3 | item action command (use object) |
| 39343C68 | `BuffIIR_c` | 0x10160d64 | 3 | nano effect on the dynel |
| 51492120 | `CharSecSpecAttackIIR_t` | 0x10160e50 | 3 | character performs special attack N on target |
| 754F1115 | `SpecialAttackInfoIIR_t` | 0x10166cfc | 3 | result of a special attack (damage) |
| 570C2039 | `CharInPlayIIR_t` | 0x10160e28 | 2 | character is now in play (visible) |

(`AppearanceUpdateIIR_c` = 41624F0D and `BuffIIR_c`-style neighbours not in this list belong to other modules.)

Common prefix of all rows below: `u32 id`, `Identity target` (header dynel), `u8 flag`; "offset" below counts from the end of
that 13-byte prefix.

## 2. FollowTargetIIR_c — 260F3671 (173×, 40..124 bytes)

Read [GC 0x10073130], write [GC 0x10073030], slot 1 (validity) [GC 0x10073276]: header identity must be kind 50000 (0xC350,
a character/NPC) and exist in the tree; the follow target (if any) must exist too. Apply [GC 0x100732e3].

Two wire forms, selected by the first byte (client treats any value except 1 as the long form; the writer emits 1 or 2; the
writer picks the short form when `header == target && speed == 0 && path ok`):

| off | type | field | meaning |
|---|---|---|---|
| 0 | u8 | form | 1 = short, 2 = long |
| 1 | u8 | mode | stored in object +0x24; apply calls the vehicle's movement controller `vtbl[+0x18](mode)` (controller = vehicle +0x178). Live values 21, 24, 25 (not decoded further, see unresolved) |

Short form (form = 1): `u8 count`, then `count × Vec3` (3 × f32 BE). Target = the header dynel itself, speed = 0,
position = `path[0]`.
Long form (form = 2): `Identity target` (0:0 = none), `f32 speed` (object +0x20), `Vec3 pos`, `u8 count`, `count × Vec3`.
`count` ≤ 30 (the client has 30 slots of 12 bytes at vehicle +0x190; the decoder errors above 30). Unused slots are zeroed.

What the client does (apply, [GC 0x100732e3] → `FUN_1006fe02` = Vehicle follow/path setter): resolves the vehicle of the
header dynel; unless the dynel is the locally owned one it sets the dynel's relative position to `pos`
(`UpdateReconcilePos`, `Vehicle_t::SetRelPosRot`, `SetRelPosIgnoreCollision(pos)`), calls the controller with `mode`, stores the
follow target (event-listener registration on the target dynel) and copies the waypoint array. The message is dropped
(`ClearToBePassedOn`, returns null) when the controller's current mode is 1, 8, 9, 0xB or 0xC.

Live (first FollowTarget, frame 5619 ms, sender 1; header `C350:FA8DD`, flag 1):

```
260f3671 0000c350 000fa8dd 01 | 01 19 02 | 44573333 4211daf6 444f4666 | 445a4666 4214f8e1 444dcccd
         id       target       flag form=1 mode=25 count=2 | (860.8, 36.46, 829.1) | (873.1, 37.24, 823.2)
```

Long form (11166 ms; target `C350:FA8D7`, flag 0): `02 15 | 00000000 00000000 | 00000000 | 4460199a 421500e6 444f4000 | 01 | 4460199a 421500e6 444f4000`
= form 2, mode 21, no target, speed 0.0, pos (896.4, 37.25, 829.0), 1 waypoint (equal to pos).

Capture statistics (test `follow_target`): 107 short forms (mode 25 ×79, mode 24 ×28, 2..9 points, max 9), 66 long forms (always mode 21,
target 0:0, speed 0.0, exactly one point equal to `pos`). All header kinds 0xC350.

Interpretation note `[GUESS]`: short form with ≥ 2 points = the character is walking along a path starting at its current
position; long form with one point and no target = "stand at this position". Neither is established beyond the code facts above.

## 3. AttackIIR_t — 28494070 (128×, 22 bytes)

Read [GC 0x1007c590], write [GC 0x1007c5c0], validity [GC 0x1007c5e7] (both dynels exist, are in the tree, not dying; else
reject), apply [GC 0x1007c545]; `IsAllowedFromClient` true (the client sends it for N3Msg_Attack).

| off | type | field |
|---|---|---|
| 0 | Identity | target (object +0x18) |
| 8 | i8 | flag (+0x20); not used by apply (always 0 live) |

Apply: casts the header dynel to `Beholder_t` and calls `FUN_10069c68(&target, 0)`, the "start fight" routine of the fight
controller (sets fight state 2, stores the target at controller +0x4c/+0x70, emits the GlobalSignals+0x6c signal, builds
a log text). Live: always pairs of mutual attackers (e.g. `C350:F4A4B` → `C350:FA8C9` at 4879 ms and the
reverse at 4896 ms); each message appears twice back to back.

```
28494070 0000c350 000f4a4b 00 | 0000c350 000fa8c9 00     header f4a4b attacks fa8c9, flag byte 0
```

## 4. AttackInfoIIR_t — 46002F16 (134×, 45 bytes)

Read [GC 0x1009ec5d], write [GC 0x1009ecbf], apply [GC 0x1009ed0d]. Fields in **wire order** with their object offsets:

| off | type | object | field | evidence |
|---|---|---|---|---|
| 0 | i32 | +0x1c | `damage` | `FUN_100693a3(+0x1c)` formats a text with it and does `Health(stat 27) -= value` on the local char; live 8..99 |
| 4 | i32 | +0x20 | `value_20` | `FUN_1006a8f3` stores it into stat 26 ("Energy" in the client's `fStatToString` table) when the current stat is ≥ 0; live always -1 |
| 8 | i32 | +0x18 | `slot` | `FUN_10068072` accepts 0..15, 0x3d, 0x3f only; live 0..6 |
| 12 | Identity | +0x24 | `other` | the other combatant (live: always kind 0xC350; header = the one hit, `other` = attacker: header `FA8D7`/other `F4A4C` mirrors AttackIIR `F4A4C → FA8D7`) |
| 20 | i32 | +0x2c | `unk_2c` | `FUN_1005ae91(+0x2c)` when non-zero; live 0 or 4 |
| 24 | i32 | +0x30 | `unk_30` | stored at fight object +0x2c/+0x30; live 3 or 4 |
| 28 | i32 | +0x34 | `unk_34` | if ≠ 0 the target is looked up through `FUN_100686d0`; live 0 |

Apply: if the header identity is not a live character dynel → if `other` is the client's character it prints the damage and
subtracts from Health; otherwise `FUN_1006a8f3(slot, damage, value_20, unk_2c, unk_30, unk_34)`: look up the fight/weapon
object, store damage/flags, update the GUI (`FUN_1009b170`, `FUN_1006a239`, `FUN_1009b373`) and set `*(fightctrl+0x1d4)+0x7c`.
Exact on-screen semantics of `slot`, `unk_*` unresolved.

```
46002f16 0000c350 000fa8d7 00 | 00000011 ffffffff 00000002 | 0000c350 000f4a4c | 00000000 00000003 00000000
                                damage=17 -1       slot=2     other=C350:F4A4C   2c=0     30=3     34=0
```

Counts: `(unk_2c, unk_30)` = (0,3) ×117, (4,3) ×14, (0,4) ×2, (4,4) ×1.

## 5. StopFightIIR_t — 4A41203E (128×, 17 bytes)

Read [GC 0x10079daa], write [GC 0x10079d95], apply [GC 0x10079d75]. Body: `i32` (object +0x18 := value ≠ 0; writer emits the byte
as i32). Live always `00000001`. Apply: if the header dynel is a live character → `FUN_10068b7f(1, 0)` = stop-fight routine
(clears fight state to 1, resets the fight target to the null identity, emits GlobalSignals+0x1fc and +0x6c signals, GUI text).

```
4a41203e 0000c350 000fa8d7 00 | 00000001
```

## 6. n3ToClientQuitIIR_t — 36510078 (27×, 13 bytes = prefix only)

Class in N3.dll (vftable [N3 0x1003e6b4]); read/write are no-ops ([N3 0x1002a091]/[N3 0x1002a096]); apply [N3 0x1002a043]:
`GetDynel(header)`; if it exists, is not already dying (byte +0x80 = 0) and is not owned by this engine
(`dynel+0x14 != n3Engine::m_pcInstance->vtbl[0x1c]()` identity compare), calls `n3Dynel_t::Die`. So: the server tells
the client a dynel left its area → it is removed. Not allowed from the client (slot 4/5 false).
Live: 18 with kind 0xC350 (characters/NPCs, e.g. `C350:F81F0`, `C350:FA8D7`), 9 with kind 0xC76A (e.g. `C76A:162D`, `C76A:161F`; 0xC76A is the kind
of the object in the GenericCmd below; no name for the kind was derived, unresolved). Frame sender always 3504.

```
36510078 0000c76a 0000162d 00          (frame at 7236 ms)
```

## 7. MissedAttackInfoIIR_t — 5C654B28 (11×, 41 bytes)

Read [GC 0x100a0a87], write [GC 0x100a0adc], apply [GC 0x100a0b20].

| off | type | object | field | notes |
|---|---|---|---|---|
| 0 | i32 | +0x1c | `value_1c` | live always -1; passed as `FUN_1006a8f3`'s stat-26 value (see §4) |
| 4 | i32 | +0x18 | `slot` | live 0..6 |
| 8 | Identity | +0x20 | `source` | `[GUESS]` the missing attacker; equals the header dynel in all 11 live messages |
| 16 | Identity | +0x28 | `target` | `[GUESS]` the one missed |
| 24 | i32 | +0x30 | `stat` | `Stat_e`; 0 = none else its name via `fStatToString` [GC 0x100367b2] goes into the text |

Apply `FUN_1006ae50`: both identities must be `SimpleChar_t`; emits a combat-log message of category 0x3b with (`target`, 1,
`source`, stat name) through `FUN_10012a1e/10012bd5` (text id formatting; which text id is chosen was not traced), then
`FUN_10068320` and `FUN_1006a8f3(slot, 0, value_1c, 0, 1, 0)` (hit routine with damage 0).

```
5c654b28 0000c350 000fa885 00 | ffffffff 00000002 | 0000c350 000fa885 | 0000c350 000fa143 | 00000000
```

## 8. GenericCmd_t — 52526858 (3×, 45 bytes)

`n3Command_t` subclass (vftable [N3 0x1003ca18] base, [GC 0x10161994] derived). Read [GC 0x1007c850] = `n3Command_t::ReadSubClass`
[N3 0x1000378f] (two i32) + `i32 cmd` + args when `cmd ∈ {3, 5, 0x20}` (factory `FUN_1003ab80`: cmd 3 → `ItemActionData_t`
(0x1c bytes); 5/0x20 → `UseItemOnItemActionData_t` (0x24 bytes); any other cmd: reader returns status 1 = rejected).

| off | type | field |
|---|---|---|
| 0 | i32 | `state` (n3Command_t +0x18; 0 = request from client, 1.. = server verdicts; live 1) |
| 4 | i32 | `seq` (+0x1c; key into `n3Command_t::m_cRefList` used by `IsForeign`; live 238, 239, 240) |
| 8 | i32 | `cmd` (+0x24) |
| 12 | i32 | ItemActionData flag (bool = ≠ 0) `FUN_1003aa61` |
| 16 | Identity | actor (ActionData +8) |
| 24 | Identity | item (ActionData +0x14) |
| 32 | Identity | cmd 5/0x20 only: `target` (+0x1c, `FUN_1003ad14`) |

Apply: `n3Command_t::Activate` [N3 0x1000382e] runs the command through the derived hooks: state 0 → `FUN_1007c8b7` (the local
request path), otherwise → `FUN_1007c76a` (state 1: start the item action `FUN_1003bac9` on the dynel, after
`FUN_1005868b`; state 2: `FUN_1003b21d` = abort). Semantics of the cmd numbers beyond "3 = item action" unresolved.

```
52526858 0000c350 0000827a 00 | 00000001 000000ee 00000003 | 00000001 | 0000c350 0000827a | 0000c76a 0000161f
                                state=1  seq=238  cmd=3      flag=1     actor=C350:827A     item=C76A:161F
```
The other two: seq 239 / 240, item `C76A:163B` (twice).

## 9. BuffIIR_c — 39343C68 (3×, 23 bytes)

Read [GC 0x1007213b], write [GC 0x1007216f], apply [GC 0x1007219c].

| off | type | field |
|---|---|---|
| 0 | i16 | `kind` (object +0x18) |
| 2 | Identity | only when `kind == 0`: nano identity (+0x1c) |

Apply: when the header dynel resolves (`FUN_10058e36`, kind 50000) and `kind == 0` → `FUN_10050afd(&identity, 0, 1, 0, 0)`:
looks up (and caches) the nano program object for the identity — `FUN_100a45ba` creates `FUN_100861a4(0xCF1B, instance)` and
loads RDB record type **0xFDE85** with the same instance — so identity kind **0xCF1B = nano program**. It then (conditions on the local char / nano state not decoded) formats a chat line, clears bit `1 << nanoSlot` of stat 0x212, adjusts stat 0xB4
("CurrentNCU" in the stat table; exact arithmetic not decoded) and spawns the nano's effect (`FUN_1005064c`, `_EffectHandler_t`). Non-zero `kind`: the apply
code does nothing; semantics unresolved (decoder keeps the remaining bytes in `rest`).

```
39343c68 0000c350 0000827a 00 | 0000 | 0000cf1b 00027e79      nano id 163449 (0x27E79) on character 33402; seen at 23920, 29898, 74495 ms
```

## 10. CharSecSpecAttackIIR_t — 51492120 (3×, 25 bytes)

Read [GC 0x10072773], write [GC 0x100727a3], apply [GC 0x100727c8] (header dynel must be `SimpleChar_t`):
`Identity target`, `i32 special`. Apply → `FUN_10068790(&target, special)`: queues/starts the special attack animation and
state for `special` (`FUN_10063be4`, `FUN_10058816`, `FUN_1003f065`, `FUN_1006b582`). Live: `special` = 0x8E = 142 =
**"Brawl"** in the client's stat-name table (`FUN_100366ba` → `fStatToString`; ids 143 "Riposte", 144 "Dimach", 145 "Deflect",
146 "SneakAttack", 147 "FastAttack" follow). Sender 3503; header `C350:827A` all three times, targets `C350:FA8EA` (×2) and
`C350:FA513`.

```
51492120 0000c350 0000827a 00 | 0000c350 000fa8ea | 0000008e
```

## 11. SpecialAttackInfoIIR_t — 754F1115 (3×, 41 bytes)

Read [GC 0x100a191e], write [GC 0x100a1972], apply [GC 0x100a19b9] → `FUN_1006a9c5`.

| off | type | object | field | notes |
|---|---|---|---|---|
| 0 | i32 | +0x20 | `slot` | live 0; passed to `FUN_10068320` |
| 4 | i32 | +0x24 | `damage` | `Health (stat 27) of target -= damage`; live 5, 20, 11 |
| 8 | i32 | +0x28 | `value_28` | live -1; passed to `FUN_10068320` |
| 12 | Identity | +0x18 | `target` | |
| 20 | i32 | +0x2c | `special` | live 0x8E (142 "Brawl"), same value as CharSecSpecAttack |
| 24 | i32 | +0x30 | `unk_30` | `FUN_1005ae91` when non-zero; live 0 |

The routine picks one of several log lines (LDB text id 0x7d3, categories 0x30 / 0x31 / 0x32 / 0x33 depending on whether the
attacker, the target or the local char is a player and on a flag at dynel +0x21c / stat bit 0x8000000) and emits it, then reduces
stat 27 of `target` by `damage` and runs the follow-up GUI refresh. Unresolved: text ids, `slot`/`value_28`/`unk_30` meaning.

```
754f1115 0000c350 0000827a 00 | 00000000 00000005 ffffffff | 0000c350 000fa8ea | 0000008e 00000000   (damage 5)
```

## 12. CharInPlayIIR_t — 570C2039 (2×, 13 bytes = prefix only)

Body reader is `n3ToServerUnBlockIIR_t::ReadSubClass` [N3 0x1002a173] (returns 0, reads nothing), apply [GC 0x1007264d]:
`n3VisualDynel_t::EnableVisibility`, `stat 0xC2 ("InPlay") := 1` (`vtbl +0x40`), spawns the effect **0x2CED** at the dynel (`_EffectHandler_t::CreateEffect2`)
when the dynel is not the local char (byte +0x140 = 0), and refreshes GUI state. The client also sends this message
(`N3Msg_SendInPlayMessage` [IF 0x10009576]; slot 5 true). Live: flag byte **1**, headers `C350:827A` (sender 33402, at 4853 ms) and `C350:82D3`
(sender 33491, at 70758 ms) — other players entering play.

```
570c2039 0000c350 0000827a 01
```

## 13. ptype 0xB ping — seen live, cross-checked with `PingMessage_t`

Code: ctor (receive) [MP 0x100028b7], `CreateDataBlock` [MP 0x10002a9e], `Message_t::HeaderSize(0xB) = 0x28` [MP 0x10001cd1];
processing `Client_t::ProcessMessage` [IF 0x10002a9e] and `Client_t::Receive` [IF 0x10001d36].

Wire (after the 16-byte frame header), six BE u32 ↔ `PingMessage_t` members (offset on wire → member):
`+0x10 → +0x1c type`, `+0x14 → +0x34`, `+0x18 → +0x28 t_orig`, `+0x1c → +0x2c t_recv`, `+0x20 → +0x30 t_send`, `+0x24 → +0x38`; remaining bytes
(frame size − 0x28) are extra data (none live).

Live (3 pings, server → client, at 9701, 39712, 69686 ms = 30.0 s apart, frame `size` 0x28, `version` 1, sender = receiver = 0x6584 = our char):

```
00fa 000b 0001 0028 00006584 00006584 | 00000001 00000000 0003d4db 00000000 00000000 0005bf9b
seq  pt   ver  size sender   receiver  | type=1   f14=0    t_orig=251099 t_recv=0 t_send=0  f24=376731
```
The same words in all three requests (only `seq` 0x00fa, 0x0254, 0x039f changes). Our reply (type 2, `seq` 2, 3, 4 from our counter,
same sender/receiver) echoed `f14`, `t_orig`, `f24` and set `t_recv = t_send` = ms since midnight: 0x04c5f9d2, 0x04c66f0d, 0x04c6e424
(= 79 952 338, 79 982 349, 80 012 324; deltas 30 011 and 29 975 ms match the capture spacing). This is exactly the code path in
`Client_t::Receive` (`Duplicate`, `sender := s_nCharID`, `receiver := request sender`, type := 2, +0x2c/+0x30 := `GetMillisecondsSinceMidnight`)
for a type-1 ping whose `receiver == s_nCharID`. A type-2 ping the client *receives* gets both timestamps stamped and goes to
`PingManager_t::PreprocessIncomingPing` + connection statistics (`AvgRTT/LastRTT` text); types 5..8 are stamped likewise; other types hit
"Protocol error. PROTOCOL_PACKET_TYPE_PINGMESSAGE gotten but default case was hit". The semantics of `f14`, `t_orig`, `f24` are server-defined
(not read by the client's reply path).

## 14. ptype 5 (text) — what the client does

`Client_t::ProcessMessage` [IF 0x10002a9e] dispatches on the frame type: 1 system, 0xA → `N3InterfaceModule_t::ServerN3Message(data, len)`,
0xB ping, 0xE operator (only sub-type 0x32 = `ReceiveN3StatisticData`); **anything else** (including 5) takes the final `else`:
`sprintf("Protocol error 2, unknown or unhandled message type %d")` and the function returns -1 (the message block is released at
`LAB_1000388e`; whether the caller turns -1 into a disconnect was not traced). So the original client **does not process incoming ptype 5 frames**;
chat from the server reaches it as N3 messages (`ChatTextIIR_t`, `FeedbackIIR_t`, `FormatFeedbackIIR_t` are registered classes, out of this module's scope).
No ptype 5 frame is in the capture.

The type exists for **outgoing** chat: `TextMessage_t` [MP 0x10002fb8 (receive ctor), 0x10003095 (send ctor)], header size 0x14 (frame header + `u32 kind`),
`CreateDataBlock` [MP 0x10002f7a]: `u32 kind` at payload offset 0 then the data. Senders [IF 0x100015d8 / 0x10001464 / 0x1000151e]:
whisper = kind 2, vicinity = kind 3, shout = kind 4; frame `sender = s_nCharID`, `receiver = 2`; data = `Identity` (i32, i32 = the channel/char
identity, `FUN_10012306`) + `i32 length` + text bytes (`BinaryStream::write`). Sent only if the connection flag at Client_t +0xa8 is set. (Layout from
the decompile; not exercised live since the capture has no client chat.)

## 15. System message 0x43 (ptype 1) — chat server list

`ProcessMessage` case 0x43 [IF 0x10002a9e]: `i32 n`; `n ×` { `i32 len; u8 str[len]; i32 a; f32 b` } into a vector (each entry 0x24 bytes:
`std::string` + two words); then emits GlobalSignals +0x1c8 (`Slot1_c<const vector<ChatServerEntry_s>&>`, namespace `ppj`). The consumer is the chat
module in GUI.dll ([GUI 0x10089dfc], strings "Got connected to chat server. (GM msg only)", "Lost connection to chat server.
Attempts to reconnect... (GM msg only)", "Attempts to reconnect to chat server: %d"): it keeps the vector (object +0x20..+0x24, 0x24 B/entry), copies the first entry (string + the two words at entry +0x1c/+0x20) into a
connection object (`FUN_1016cefb`), and with more than one entry runs a selection (`FUN_1008b3b9/1008bba5`; not traced). Reconnect pacing in the same function:
wait `1 << (attempt+12)` ms (capped at 0x8000) between attempts; the "(GM msg only)" lines are shown to GMs only (`InputConfig_t::CheckMode(0x38)`).

Live (4852 ms, sender 1, receiver 0x6584, message type 0x43, frame size 0x33):
```
00000001 | 0000000f "199.241.136.157" | 00001b5d | 00000000
n=1        len=15   host               a=7005 (port)  b=0.0
```
Host 199.241.136.157, `a` = 0x1B5D = 7005, `b` = 0.0. `[GUESS]` `a` is the TCP port (value is plausible; the connect call that consumes it was not traced) and `b` a load/weight value (always 0.0 here).

## UNRESOLVED / guesses

* FollowTarget: meaning of `mode` 21/24/25 (they go to the vehicle controller at `Vehicle+0x178`, vtbl +0x18; class not identified); unused `speed`.
* AttackInfo / MissedAttackInfo / SpecialAttackInfo: names for `slot`, `value_20`/`value_1c`/`value_28` (stat-26 "Energy" write, -1 live), `unk_2c/30/34`;
  which text ids the log lines use; direction of `source`/`target` in Missed (header equality is the only evidence).
* Identity kind 0xC76A: no class name derived (it is the kind of the object used by GenericCmd 3 and of 9 despawned dynels).
* GenericCmd: meaning of `state` values 2..5 and of cmd numbers other than 3/5/0x20; header `flag` meaning beyond `ToBePassedOn`.
* Buff with `kind != 0`.
* Frame `sender` values 3502/3503/3504/33402 (not part of the N3 message).
* The full path `ServerN3Message` → `Construct` was not traced; `Construct` was identified as the sole reader of the key/Identity/flag layout.
* Ping `f14`/`t_orig`/`f24` meaning; consumer behaviour of the -1 return for unknown frame types.
