# Zone N3: client → server messages, entry handshake, message registry

Code: `crates/ao-net/src/n3/outgoing.rs`. All addresses are VAs of the 32-bit DLLs in `~/Games/ProjectRubiKa/client`
(`[GC]` Gamecode.dll, `[N3]` N3.dll, `[MP]` MessageProtocol.dll, `[IF]` Interfaces.dll, `[GUI]` GUI.dll, `[CONN]` Connection.dll).
Capture used for checks: `docs/captures/zone_ithaca.rec` (80 s, 1098 lines; only 4 client→server lines exist: ZoneLogin + 3 ping replies).

## 1. Summary (what the server needs / what the real client does)

* **The server needs nothing beyond `ZoneLogin` (system 0x1B) to start streaming.** The capture shows the full burst
  (PlayfieldAnarchyF, own character, ~1000 dynel/stat messages over 80 s) while the client sent *only* `ZoneLogin` and
  ping replies. [observed]
* **The real client additionally sends `CharInPlayIIR_t` (`0x570C2039`, 13 bytes) exactly once** after the playfield and
  its own character are in (§3) and then `CharDCMoveIIR_t` (`0x54111123`, 54 bytes) for every movement change (§5). That
  `CharInPlay` is what the other clients see relayed (the capture contains two relayed ones from other characters,
  §4). Whether the server *withholds* anything until it receives it cannot be known from this capture (the capture
  client never sent one). [INFERENCE: PRK/AO servers use it to mark the character as "in play"/visible; not proven.]
* Every client N3 message is an `n3InfoItemRemote_t` ("IIR"); there is no other client N3 envelope.

## 2. Wire conventions of client frames

All integers big-endian (`BinaryStream` `operator<<`, same as receive).

### 2.1 Frame header (`Connection_t::Send` [CONN 0x100016f3], see `docs/protocol.md`)

`u16 seq, u16 ptype, u16 version(=1), u16 size, u32 sender, u32 receiver`, payload zero-padded to a multiple of 4,
the capture's `>` lines are plain 4-byte-padded frames.

| message | ptype | sender | receiver | constructor evidence |
|---|---|---|---|---|
| N3 (every IIR) | `0x0A` | `Client_t::s_nCharID` | **2** | `Client_t::SendACEDataBlock` [IF 0x10001692]: `N3Message_t(s_nCharID, 2, size, data)` → `Message_t(10, sender, receiver, 0)` [MP 0x10001fa7/0x10001c85] |
| text (say/shout/whisper) | `0x05` | `s_nCharID` | 2 | `Client_t::SendVicinityMessage` [IF 0x10001464] `TextMessage_t(3, s_nCharID, 2, len, data)` [MP 0x10003095] |
| ZoneLogin (system `0x1B`) | `0x01` | `s_nCharID` | 2 | `Client_t::SendClientCookie` [IF 0x100013b0] `SystemMessage_t(0x1b, s_nCharID, 2, ..)`; capture line 4704 ms: `0001 0001 0001 0020 00006584 00000002 0000001b ...` |
| ping reply | `0x0B` | `s_nCharID` | `s_nCharID` | capture 9701 ms: `0002 000b 0001 0028 00006584 00006584 00000002 ...` (not N3) |

(`s_nCharID` = 0x6584 = 25988 in the capture.) `seq` is the per-direction counter owned by the connection (first frame = 1).
`Message_t::HeaderSize`: N3 = 0x10, text/system/operator = 0x14, ping = 0x28 [MP 0x10001cd1].

### 2.2 N3 payload = `n3InfoItemRemote_t::Write` [N3 0x100098f4]

| off | type | field | meaning |
|---|---|---|---|
| 0 | u32 | `key` | `MapToKey(class name)` [N3 0x10009826]: `key = 0; for i,c in name: key ^= (i8)c << ((i&3)*8)`. This is the *message id* (e.g. `"CharDCMoveIIR_t"` → `0x54111123`). |
| 4 | i32 | target kind | `Identity_t` at `this+4` (`FUN_10037d6a`). For client messages the dynel the message is about: the control dynel `dynel+0x14` = `{0xC350, char_id}` |
| 8 | i32 | target instance | |
| 12 | u8 | to-be-passed-on | `(this+0xc == 1)`: set (1) by both client messages below. Names: `ClearToBePassedOn`, `EnablePassOn`, `ToBePassedOn` [N3 0x10052048/0x10058f99]; [INFERENCE] the server relays flag-1 IIRs to the observers: two relayed `CharInPlay` in the capture carry flag 1, relayed `CharDCMove` carry flag 0 (server rewrites). |
| 13 | … | class body | virtual slot 8 `WriteSubClass(BinaryStream&)`; receive side is slot 7 `ReadSubClass` + `Construct` [N3 0x10009b08] |

`SendIIRToServer` [N3 0x10007762] builds a 0x100-byte `BinaryStream`, `Write`s, wraps it into an `ACE_Data_Block` and
calls virtual `SendNetWorkMessage(Identity{0x2713,0}, block)` [N3 vtable +0x28]; the `Identity{0x2713,0}` argument is ignored by
`Client_t::SendACEDataBlock`, which is the only place that decides ptype/sender/receiver. `SendACEDataBlock` drops the block
silently when `Client_t` is not connected (`this+0xa8 == 0`). `n3Dynel_t::SendIIRToObservers` [N3 0x10004e43] forwards to
`SendIIRToServer` if the dynel is in the tree (else prints "not in the tree" and drops).
Server-only IIRs are marked by `IsAllowedFromClient() == false` (`n3PlayfieldFullUpdateIIR_t`, `n3LocalityUpdateIIR_t`,
`n3TeleportIIR_t`) [N3 0x1000b72e]; `n3Command_t`/`n3ToServerUnBlockIIR_t` return true [N3 0x1000ab82].

## 3. Entry handshake (order and triggers)

1. `ZoneLogin` (system 0x1B) is sent by `NetworkModule_t::N3ActivatedMessage` (AFCM message `0xE4`) → `Client_t::SendClientCookie`
   [IF 0x100077a7 → 0x100013b0]. Already implemented (`msg.rs`).
2. Server burst: `PlayfieldAnarchyFIIR_t` (`5F4B1A39`), own `FullCharacterIIR_t`/`SimpleCharFullUpdateIIR_t` etc. No client send was found on the `PlayfieldAnarchyF` path (the capture client sent nothing either; `PlayfieldAnarchyFIIR_t::Activate` itself was not decompiled, see §9). Playfield loading is local (the IIR's `Activate` creates the
   playfield; `SimpleCharFullUpdateIIR_t::Activate` [GC 0x10077e13] requires `n3Playfield_t::GetPlayfield(pf)` to exist, else it
   returns without creating the dynel).
3. When `SimpleCharFullUpdateIIR_t` (`271B3A6B`) for **our own** character id has been activated, its tail does
   `if (dynel.identity == engine->GetClientCharID()) n3EngineClientAnarchy_t::Get()->SetMainDynel(dynel)` (virtual call at
   vtable `+0x40`, end of `FUN_10077e13`).
4. `n3EngineClientAnarchy_t::SetMainDynel` [GC 0x10019f77]: base `n3EngineClient_t::SetMainDynel` [N3 0x10007aa4] (camera
   attach), then for a `SimpleChar_t` posts event `6` (`FUN_10012a1e(6,..)`, `FUN_100112fb(6,..)` → `FUN_1000482e` maps 6 → AFCM
   message `0x144`).
5. AFCM `0x144` = `FlowControlModule_t::TeleportEndedMessage` [GUI 0x100292ce] (registered at [GUI 0x1002ad2c]). At its end
   (asm 0x10029628..0x10029641) it arms the in-play countdown: `DAT_102760cd = 1` (waiting), `DAT_102760d8 = 120.0f`
   (`[GUI 0x101ae4d8] = 0x42f00000`), `DAT_102760dc = 10` (frames). (`FUN_1000482e` is also reached from `StartTryingTeleport`, `TeleportFailed`, `TradeStart`, ... with other event codes; whether
   a later zone change re-sends `CharInPlay` through the same path was not traced [INFERENCE: yes, `SetMainDynel` runs again].)
6. `FlowControlModule_t::FrameProcess` [GUI 0x10028a9e] → `WaitingToStartGame` [GUI 0x10027d73] every frame while waiting:
   `t -= frameTime; frames -= 1; if (frames < 0 || t < 0.0)` **and** `!DatabaseInterfaceModule_t::HasWorkLoad()` (no pending
   rdb loads) → `N3InterfaceModule_t::N3Msg_SendInPlayMessage` [IF 0x10009576] → `n3EngineClientAnarchy_t::N3Msg_SendInPlayMessage`
   [GC 0x10018090]. So: **≥ 11 rendered frames after our own character appeared and the asset queue is idle** (120 s hard
   timeout as the alternative gate). If there is no control dynel yet it returns false and is retried next frame.
   On success: `InputConfig_t::ClrStaticInputMode(0x3a)` (input unlocked) and the waiting flag is cleared. `AliveMessage`
   [GUI 0x10028543] also clears the flag [GUI 0x10028680].
7. `N3Msg_SendInPlayMessage`: `GetClientControlDynel()`; if null → returns false, nothing sent. Else builds `CharInPlayIIR_t`
   (`FUN_100726ce` [GC]: `n3ToServerUnBlockIIR_t(dynel.identity, "CharInPlayIIR_t")` then `this[0xc] = 1`) and
   `SendIIRToServer`. **One frame** (13-byte payload, size field 29, 32 bytes on the wire).

Example (own character 0x6584), `char_in_play(0x6584)` = `570c2039 0000c350 00006584 01`; framed `0001 000a 0001 001d 00006584 00000002 | payload | 000000`.

After that the client sends only user-driven IIRs. Nothing else is sent at entry: `N3Msg_RequestCharacterInventory`/
`RequestClothInventory`/... [GC 0x10026676..] do not appear among `SendIIRToServer`/`SendIIRToObservers` callers (they read client-side
inventory state), `RequestMailMessage` [GC 0x10023975] sends `MailIIR_c` only on user action.

## 4. `CharInPlayIIR_t` — id `0x570C2039`

* Class `CharInPlayIIR_t : n3ToServerUnBlockIIR_t : n3InfoItemRemote_t`, vftable [GC 0x10160e28], ctor [GC 0x100726ce], registered at [GC 0x1000da36].
* `ReadSubClass`/`WriteSubClass` = `n3ToServerUnBlockIIR_t`'s, both empty [N3 0x1002a173 / 0x1002a178]: **no body**.

| off | type | field | value |
|---|---|---|---|
| 0 | u32 | key | `570C2039` |
| 4 | i32,i32 | target | `{0xC350, char_id}` |
| 12 | u8 | passed-on | `1` |

Captured relays (server → us, other characters; identical layout and bytes to what we build):
`570c2039 0000c350 0000827a 01` (4853 ms, sender 33402) and `570c2039 0000c350 000082d3 01` (70758 ms, sender 33491). Test `in_play_bytes`.

## 5. `CharDCMoveIIR_t` — id `0x54111123` (movement / position update)

* Class `CharDCMoveIIR_t : CharDCMoveIIRBase_t`, vftable [GC 0x10160644]; ctor chain [GC 0x1006ba23] → [GC 0x1006bb28]; registered [GC 0x1000d9da].
* Client producer: `n3EngineClientAnarchy_t::N3Msg_MovementChanged(action, f1, f2, bool)` [GC 0x18b5c]: builds
  `CharDCMoveIIR_t(dynel.identity, action, relPos, relRot, f1, f2)` [GC 0x1006ba23], calls `FUN_1006b84b` (applies the move to the
  local dynel and `UpdateLastMotionMessageData`), then `SendIIRToServer`. A second sender, `FUN_1005a5d6` [GC], also builds `CharDCMoveIIR_t`
  (not analysed). Server relays the same class to other clients (144 captured).
* Body write = `FUN_1006bc55` (base) [GC 0x1006bc55] + `FUN_1006b9d6` [GC 0x1006b9d6]; read = `FUN_1006bb79` + `FUN_1006b96f`:

| off (payload) | type | field | meaning |
|---|---|---|---|
| 13 | u8 | `action` | `Movement_n::MovementAction_e` (receiver: `& 0x7f`, non-finite floats reject the message) |
| 14 | f32×4 | `rot` x,y,z,w | `GetRelRot()` quaternion, `w` last (identity = `0 0 0 1` = `3f800000` last) (`FUN_1000a74d` [GC]) |
| 30 | f32×3 | `pos` x,y,z | `GetRelPos()` (Y up) (`FUN_1000401b` [GC]) |
| 42 | i32 | `elapsed_ms` | `(int)round((GameTime.now - last_written) * 1000.0)`; `last_written` is the global `DAT_102e32d8` updated on every write; 1000.0 = `[GC 0x10157870]` (`0x408f4000_00000000`). Relayed copies carry 0. |
| 46 | f32 | `look[0]` | `N3Msg_MovementChanged` param 2 (CharDCMove `+0x40`) |
| 50 | f32 | `look[1]` | param 3 (`+0x44`) |

54 bytes total (header 13 + 41). All 144 captured relays parse and re-encode byte-exact (test `captured_moves_roundtrip`), e.g.
`54111123 0000c350 0000827a 00 0c 00000000 bf67e036 00000000 3ed8f9c0 445aaef1 4220051e 442ed415 00000000 00000000 00000000`
→ id 0x827a, action 0x0c, quat (0,-0.906,0,0.424), pos (874.73, 40.005, 699.31), elapsed 0, look (0,0).

### 5.1 Action codes seen in client code (`MovementAction_e`)

Evidence = the call sites in `FlowControlModule_t::SlotMovement*` [GUI 0x10027ec1..0x10028077] (bool arg `b`) and `N3Msg_*` [GC].
Which polarity of `b` is key-press is **not proven**; `SlotMovementJump` and `SlotMovementAutoRun` act only when `b == false`, so `false` is most
probably "pressed" [INFERENCE]. Names are the slot names, not enum names (the enum has no symbols in the DLLs).

| action | source | meaning |
|---|---|---|
| `1` / `2` | `SlotMovementForward(b)` = `b+1` (AutoRun calls `Forward(false)`) | forward (b=false → 1) |
| `3` / `4` | `SlotMovementBack(b)` = `b+3` | back |
| `0x0C` / `0x0E` | `SlotMovementLeft(b)` = `0xC + 2b` | turn left |
| `0x09` / `0x0B` | `SlotMovementRight(b)` = `9 + 2b` | turn right |
| `7` / `8` | `SlotMovementStrafeLeft(b)` = `7+b` | strafe left |
| `5` / `6` | right-strafe (derived: mouse-look remaps `0xC→7, 9→5, 0xE→8, 0xB→6`, `N3Msg_MovementChanged` [GC 0x18b5c]) | strafe right |
| `0x0F` | `SlotMovementJump` (`b==false` only) | jump |
| `0x16` | `CheckMotionUpdate`, `StartTeleportTry`, `EndCameraMouseLook`; also `0x2B` is rewritten to `0x16` with a "send as-is" flag | position/rotation sync (no key) |
| `0x2B` | `N3Msg_MouseMovement` [GC 0x1964b] with `look = (dx, dy)`; **never written to the wire** | mouse-look turn/pitch (local) |
| `0x0A` / `0x0D` | `N3Msg_MouseMovement` when `dx >= 0` / `< 0` and no `dy` | mouse-turn right/left start [GUESS from branch structure] |
| `0x1B` | `N3Msg_CrawlToggle` | crawl |
| `0x1E` | `N3Msg_SitToggle`, `N3Msg_StartCamping` | sit / camp |
| `0x24` | [GC 0x1003e2f8], [GC 0x10042720] | unresolved |
| `0x1C`, `0x18` | tested in `N3Msg_MovementChanged` (special cases with fight mode 8 / a stat) | unresolved |

Captured relayed actions: `01 02 07 08 09 0a 0b 0c 0d 0e 16 1e`.
`N3Msg_MovementChanged` additionally suppresses the send unless the movement mode allows it (`vtable+0x90`, `+0xc` checks) — conditions not decoded.

## 6. Chat (text messages, ptype 5)

Not N3. `NetworkModule_t` [IF 0x100077bd] registers AFCM handlers `0x16d` Vicinity [IF 0x100076b4], `0x129` Shout [IF 0x10007705],
`0x170` Whisper [IF 0x10007756]; each reads `(Identity_t, uint len, char* text)` from the AFCM queue and calls
`Client_t::SendVicinityMessage/SendShoutMessage/SendWhisperMessage` [IF 0x10001464 / 0x1000151e / 0x100015d8]:

```
BinaryStream s(0x800); s << identity.kind << identity.instance;  // FUN_10012306
s << (int)len; s.write(text, len);                               // no terminator added
TextMessage_t(type = 3 vicinity | 4 shout | 2 whisper, sender = s_nCharID, receiver = 2, size, data) -> Connection_t::Send
```

`TextMessage_t::CreateDataBlock` [MP 0x10002f7a]: frame offset 16 = `u32 type` (htonl), offset 20.. = the stream.

| payload off | type | field |
|---|---|---|
| 0 | u32 | type (2 whisper, 3 vicinity, 4 shout) |
| 4 | i32,i32 | identity passed in by the GUI |
| 12 | i32 | `len` |
| 16 | bytes | `len` text bytes |

`outgoing::text_payload` / `text_frame`. **Unresolved**: (a) who the identity names (the GUI sender of AFCM `0x16d/0x129/0x170` was not found: an immediate
search over GUI/Gamecode/Interfaces finds only registrations; probable: our own or the target's id — `{0,0}` is used in the unit test only
as a byte example), (b) whether the GUI's `len` counts a trailing NUL, (c) ptype-5 *received* text is out of scope here (none in the capture).
Slash commands (`N3Msg_TextCommand` [GC 0x176db] → `FUN_1003fba6`) are not text frames: they are dispatched client side and become
`CharacterActionIIR_t`, `FollowTargetIIR_c`, `OrgClientIIR_c`, `PetCommandIIR_c`, `RaidCmdIIR_c` via `SendIIRToObservers` (layouts not decoded here).
[INFERENCE] Private/channel chat (org, tell, general) uses the separate chat server, not this connection; no code for it was read.

## 7. Other client senders (class only; layouts NOT decoded here)

From the `SendIIRToServer` / `SendIIRToObservers` call graph of Gamecode.dll (function → IIR class constructed in it or its direct callees):
see the last column of the registry. Notable: `CharacterActionIIR_t` (`5E477770`) is the generic action carrier for ~60 `N3Msg_*` functions
(UseItem, UseSkill, CastNanoSpell, Team*, Duel_*, SitToggle, RequestChecklist, ...); `GenericCmd_t` (`52526858`) via `N3Msg_UseItem`; `AttackIIR_t` (`28494070`)
[GC 0x10067c34]; `StopFightIIR_t` (`4A41203E`) `N3Msg_StopAttack`; `CharSecSpecAttackIIR_t` (`51492120`); `MailIIR_c`; `TradeIIR_t`; `Knubot*IIR_c` (NPC dialogue);
`ClientRequestBuy/Build/Demolish/CloseGUI/RqToggleCloaking IIR_c` [GC 0x1012e086..]; `GridSelectedIIR_t`. These are the user-action messages; none are needed for entry.

## 8. Registry of all N3 message ids

Source: every `n3InfoItemRemote_t::Register(name, ctor)` call in Gamecode.dll (135) and N3.dll (3), found with a Ghidra script
(functions that call the imported `Register` and reference the class-name string; scripts in the work dir of this agent, not committed), id = `MapToKey(name)` [N3 0x10009826].
No two classes collide. All 27 ids seen in the capture are present. `class_name(id)` / `REGISTRY` in `outgoing.rs` carry this table.

Direction column: **C→S** = a client function constructs it and sends it (`SendIIRToServer`/`SendIIRToObservers` call graph, column 7, evidence
[GC] addresses); **S→C** = seen in the capture from the server or `IsAllowedFromClient()==false`; **both** = both; **?** = registered but no client sender
found and not in the capture — presumably server→client [INFERENCE]; the client *can* also receive classes marked C→S (the server relays them). The `n3LocalityUpdateIIR_t`,
`n3ToServerUnBlockIIR_t` classes exist in N3.dll but are not in a `Register` call found by the scan (their ids are not listed).

Note on names: the id of the 4 ids first not matched by an `IIR_t` string scan were `FollowTargetIIR_c` (`260F3671`), `GenericCmd_t` (`52526858`),
`BuffIIR_c` (`39343C68`), `AppearanceUpdateIIR_c` (`41624F0D`): the `_c` and `Cmd_t` suffixes are real class names.

| id | class (RTTI / `Register` string) | direction | Register fn | ctor | live count (S→C) | client senders (`N3Msg_*`, `[GC]`) |
|---|---|---|---|---|---|---|
| `000A0C5A` | `KnubotNPCDescriptionIIR_c` | C→S | [GC 0x1000defa] | [GC 0x1000ba30] |  | `N3Msg_NPCChatRequestDescription` 0x10017e56 |
| `052E2F0C` | `AddTemplateIIR_t` | ? | [GC 0x1000ea52] | [GC 0x1000c06b] |  |  |
| `0639474D` | `GridDestinationSelectIIR_t` | ? | [GC 0x1000f436] | [GC 0x1000c5ba] |  |  |
| `08536F65` | `CentralControllerStateIIR_t` | ? | [GC 0x1000ed9c] | [GC 0x1000c22d] |  |  |
| `0C5A5D6D` | `WeatherControlIIR_t` | ? | [GC 0x100106e8] | [GC 0x1000cf8d] |  |  |
| `0D381F02` | `PetToMasterIIR_c` | ? | [GC 0x1000fa14] | [GC 0x1000c8e0] |  |  |
| `1078735A` | `FlushRDBCachesIIR_c` | ? | [GC 0x1000f1a6] | [GC 0x1000c459] |  |  |
| `15253307` | `CentralControllerFullUpdateIIR_t` | ? | [GC 0x1000ed3e] | [GC 0x1000c1fb] |  |  |
| `166A435E` | `AcceptBSInviteIIR_t` | C→S | [GC 0x1000d866] | [GC 0x1000b6a9] |  | `N3Msg_GoToBattle` 0x10018297 |
| `194E4F76` | `AddPetIIR_c` | ? | [GC 0x1000e9f4] | [GC 0x1000c039] |  |  |
| `195E496E` | `SetPosIIR_c` | ? | [GC 0x1000ff96] | [GC 0x1000cb9c] |  |  |
| `1C3A4F77` | `ReflectAttackIIR_t` | ? | [GC 0x1000fc48] | [GC 0x1000ca0c] |  |  |
| `1D3C0F1C` | `SpecialAttackWeaponIIR_t` | S→C | [GC 0x10010284] | [GC 0x1000cd2f] | 58 |  |
| `2001377E` | `MentorInviteIIR_c` | C→S | [GC 0x1000e18c] | [GC 0x1000bb91] |  | `N3Msg_SendMentorInvite` 0x1001860b, `FUN_10039ec0` 0x10039ec0 |
| `2049527C` | `ActionIIR_t` | ? | [GC 0x1000e996] | [GC 0x1000c007] |  |  |
| `204F4871` | `ScriptIIR_t` | ? | [GC 0x1000e47a] | [GC 0x1000bd31] |  |  |
| `206B4B73` | `FormatFeedbackIIR_t` | ? | [GC 0x1000f204] | [GC 0x1000c48b] |  |  |
| `2103247D` | `KnubotAnswerIIR_c` | C→S | [GC 0x1000dde0] | [GC 0x1000b99a] |  | `N3Msg_SendNPCChatAnswer` 0x1001902a |
| `212C487A` | `QuestIIR_t` | C→S | [GC 0x1000e360] | [GC 0x1000bc98] |  | `N3Msg_RemoveQuest` 0x10019e5a |
| `215B5678` | `MineFullUpdateIIR_t` | ? | [GC 0x1000f83e] | [GC 0x1000c7e0] |  |  |
| `2252445F` | `LookAtIIR_t` | C→S | [GC 0x1000e0d0] | [GC 0x1000bb2a] |  | `FUN_1003b0db` 0x1003b0db, `FUN_1003b73e` 0x1003b73e |
| `25192476` | `ShieldAttackIIR_t` | ? | [GC 0x100100b0] | [GC 0x1000cc32] |  |  |
| `25314D6D` | `CastNanoSpellIIR_t` | S→C | [GC 0x1000ece0] | [GC 0x1000c1c9] | 3 |  |
| `253D0240` | `ResearchUpdateIIR` | ? | [GC 0x1000fdc0] | - |  |  |
| `260F3671` | `FollowTargetIIR_c` | both | [GC 0x1000dc0a] | [GC 0x1000b89d] | 173 | `FUN_1003fba6` 0x1003fba6 |
| `264B514B` | `RelocateDynelsIIR_t` | ? | [GC 0x1000fd04] | [GC 0x1000ca70] |  |  |
| `264E5F61` | `AbsorbIIR_t` | ? | [GC 0x1000e938] | [GC 0x1000bfd5] |  |  |
| `26515E61` | `ReloadIIR_t` | ? | [GC 0x1000fca6] | [GC 0x1000ca3e] |  |  |
| `270A4C62` | `KnubotCloseChatWindowIIR_c` | C→S | [GC 0x1000de3e] | [GC 0x1000b9cc] |  | `N3Msg_NPCChatCloseWindow` 0x1001ce62 |
| `271B3A6B` | `SimpleCharFullUpdateIIR_t` | S→C | [GC 0x1001016a] | [GC 0x1000cc96] | 81 |  |
| `28251F01` | `StartLogoutIIR_t` | ? | [GC 0x1000e6ae] | [GC 0x1000be6a] |  |  |
| `28494070` | `AttackIIR_t` | both | [GC 0x1000d920] | [GC 0x1000b70d] | 128 | `FUN_10067c34` 0x10067c34 |
| `28784248` | `TeamMemberInfoIIR_t` | ? | [GC 0x100103fa] | [GC 0x1000cdf7] |  |  |
| `29304349` | `FullCharacterIIR_t` | S→C | [GC 0x1000f31e] | [GC 0x1000c521] | 1 |  |
| `2933154F` | `LaserTagListIIR_t` | ? | [GC 0x1000f782] | [GC 0x1000c77c] |  |  |
| `2A253F5F` | `TrapDisarmedIIR_t` | ? | [GC 0x100104b6] | [GC 0x1000ce5b] |  |  |
| `2A293D0F` | `FovIIR_c` | ? | [GC 0x1000f262] | [GC 0x1000c4bd] |  |  |
| `2B333D6E` | `StatIIR_t` | S→C | [GC 0x10010340] | [GC 0x1000cd93] | 69 |  |
| `2C2F061C` | `QueueUpdateIIR_t` | ? | [GC 0x1000e3be] | [GC 0x1000bcca] |  |  |
| `2D212407` | `KnubotRejectedItemsIIR_c` | ? | [GC 0x1000f724] | [GC 0x1000c74a] |  |  |
| `2E2A4A6B` | `OrgInfoPacketIIR_t` | S→C | [GC 0x1000f958] | [GC 0x1000c879] | 1 |  |
| `30161355` | `n3PlayfieldFullUpdateIIR_t` | S→C | [N3 0x1000aedf] | [N3 0x1000ac60] |  |  |
| `3129233B` | `AreaFormulaIIR_t` | ? | [GC 0x1000eb0e] | [GC 0x1000c0cf] |  |  |
| `3301337A` | `InfromPlayerIIR_t` | ? | [GC 0x1000dd82] | [GC 0x1000b968] |  |  |
| `33312042` | `WaypointPathIIR_c` | ? | [GC 0x10010860] | [GC 0x1000d055] |  |  |
| `333B2867` | `MailIIR_c` | C→S | [GC 0x1000e12e] | [GC 0x1000bb5c] |  | `N3Msg_MailTakeAll` 0x100237d7, `N3Msg_DeleteMail` 0x1002383c, `N3Msg_SendMail` 0x100238a1 (+2) |
| `342C1D1D` | `ApplySpellsIIR_t` | ? | [GC 0x1000eab0] | [GC 0x1000c09d] |  |  |
| `343C287F` | `BankIIR_t` | ? | [GC 0x1000ec24] | [GC 0x1000c165] |  |  |
| `35505644` | `TemplateActionIIR_t` | ? | [GC 0x10010458] | [GC 0x1000ce29] |  |  |
| `36284F6E` | `TradeIIR_t` | C→S | [GC 0x1000e87e] | [GC 0x1000bf71] |  | `N3Msg_TradeAccept` 0x10015bfd, `N3Msg_TradeConfirm` 0x10015c66, `N3Msg_TradeAbort` 0x10015ccf (+4) |
| `36510078` | `n3ToClientQuitIIR_t` | S→C | [N3 0x1000ae82] | - | 27 |  |
| `365A5071` | `DoorFullUpdateIIR_t` | S→C | [GC 0x1000f02e] | [GC 0x1000c391] |  | decoded `world::Door`, docs/zone/doors.md §2 |
| `365E555B` | `CityAdvantagesIIR_t` | ? | [GC 0x1000eeb6] | [GC 0x1000c2c6] |  |  |
| `3710256C` | `HealthDamageIIR_t` | ? | [GC 0x1000f494] | [GC 0x1000c5ec] |  |  |
| `371D0542` | `FightModeUpdate_t` | ? | [GC 0x1000dbac] | [GC 0x1000b86b] |  |  |
| `39343C68` | `BuffIIR_c` | S→C | [GC 0x1000ec82] | [GC 0x1000c197] | 3 |  |
| `3A1B2C0C` | `KnubotTradeIIR_c` | C→S | [GC 0x1000e014] | [GC 0x1000bac6] |  | `N3Msg_NPCChatAddTradeItem` 0x10017ea0, `N3Msg_NPCChatRemoveTradeItem` 0x10017f31 |
| `3A223B50` | `ItemReplacedIIR_c` | ? | [GC 0x1000f60a] | [GC 0x1000c6b4] |  |  |
| `3A243F41` | `DropTemplateIIR_t` | C→S | [GC 0x1000db4e] | [GC 0x1000b839] |  | `N3Msg_DropItem` 0x10027c94 |
| `3A322A4A` | `GridSelectedIIR_t` | C→S | [GC 0x1000dd24] | [GC 0x1000b936] |  | `N3Msg_GridDestinationSelected` 0x1001817b |
| `3B11256F` | `SimpleItemFullUpdateIIR_t` | ? | [GC 0x100101c8] | [GC 0x1000cccb] |  |  |
| `3B132D64` | `KnubotOpenChatWindowIIR_c` | C→S | [GC 0x1000df58] | [GC 0x1000ba62] |  | `N3Msg_DefaultActionOnDynel` 0x100291da |
| `3B1D2268` | `WeaponItemFullUpdateIIR_t` | S→C | [GC 0x1001068a] | [GC 0x1000cf5b] | 10 |  |
| `3B290771` | `SocialActionCmd_t` | ? | [GC 0x1000e5f2] | [GC 0x1000bdf9] |  |  |
| `3B3B2878` | `RaidIIR_c` | ? | [GC 0x1000e41c] | [GC 0x1000bcfc] |  |  |
| `3C1E2803` | `ShadowLevelIIR_t` | ? | [GC 0x10010052] | [GC 0x1000cc00] |  |  |
| `3C265179` | `CloneIIR_t` | ? | [GC 0x1000ef14] | [GC 0x1000c2f8] |  |  |
| `3D746C70` | `ServerPathPosDebugInfoIIR_c` | ? | [GC 0x1000feda] | [GC 0x1000cb38] |  |  |
| `3E205660` | `SkillIIR_t` | C→S | [GC 0x1000e594] | [GC 0x1000bdc7] |  | `N3Msg_ClientIPAdjust` 0x10026e7e |
| `3F3A1914` | `LeaveBattleIIR_t` | C→S | [GC 0x1000e072] | [GC 0x1000baf8] |  | `N3Msg_LeaveBattle` 0x100182ec |
| `41624F0D` | `AppearanceUpdateIIR_c` | S→C | [GC 0x10010746] | [GC 0x1000cfbf] | 2 |  |
| `43197D22` | `n3TeleportIIR_t` | S→C | [N3 0x1000af3c] | [N3 0x1000ac92] |  |  |
| `435F7023` | `PerkUpdateIIR` | ? | [GC 0x1000e248] | [GC 0x1000bbf5] |  |  |
| `44483B3A` | `SendScoreIIR_t` | ? | [GC 0x1000fe7c] | [GC 0x1000cb06] |  |  |
| `445F2A0B` | `ResurrectIIR_t` | ? | [GC 0x1000fe1e] | [GC 0x1000cad4] |  |  |
| `45072A2D` | `UpdateClientVisualIIR_t` | ? | [GC 0x10010572] | [GC 0x1000cec2] |  |  |
| `455D2938` | `PlaySoundIIR_c` | ? | [GC 0x1000fa72] | [GC 0x1000c912] |  |  |
| `46002F16` | `AttackInfoIIR_t` | S→C | [GC 0x1000eb6c] | [GC 0x1000c101] | 134 |  |
| `46312D2E` | `TeamMemberIIR_t` | ? | [GC 0x1001039e] | [GC 0x1000cdc5] |  |  |
| `464D000A` | `SpawnMechIIR_t` | C→S | [GC 0x1000e650] | [GC 0x1000be38] |  | `N3Msg_ActivateMech` 0x100270d4 |
| `465A4061` | `QuestFullUpdateIIR_t` | S→C | [GC 0x1000fbea] | [GC 0x1000c9da] | 1 |  |
| `465A5D73` | `ChestFullUpdateIIR_t` | ? | [GC 0x1000ee58] | [GC 0x1000c291] |  |  |
| `470B2E14` | `MarketSendIIR_c` | C→S | [GC 0x1000e8dc] | [GC 0x1000bfa3] |  | `N3Msg_SendMarketItem` 0x1001dff9 |
| `47483633` | `DropDynelIIR_t` | ? | [GC 0x1000f0ea] | [GC 0x1000c3f5] |  |  |
| `47537A24` | `ContainerAddItemIIR_t` | ? | [GC 0x1000ef72] | [GC 0x1000c32a] |  |  |
| `485E7202` | `InventoryUpdatedIIR_t` | ? | [GC 0x1000f5ac] | [GC 0x1000c682] |  |  |
| `49222612` | `VisibilityIIR_t` | ? | [GC 0x1001062e] | [GC 0x1000cf29] |  |  |
| `4A41203E` | `StopFightIIR_t` | both | [GC 0x1000e70c] | [GC 0x1000be9c] | 128 | `N3Msg_StopAttack` 0x10027f55 |
| `4B062919` | `BattleOverIIR_t` | ? | [GC 0x1000d97e] | [GC 0x1000b73f] |  |  |
| `4C7D403B` | `DoorStatusUpdateIIR_t` | S→C | [GC 0x1000f08c] | [GC 0x1000c3c3] |  | decoded `world::DoorStatus`, docs/zone/doors.md §2 |
| `4D2A313B` | `TeamInviteIIR_t` | ? | [GC 0x1000e822] | [GC 0x1000bf3f] |  |  |
| `4D38242E` | `InfoPacketIIR_t` | ? | [GC 0x1000f4f2] | [GC 0x1000c61e] |  |  |
| `4D450114` | `SpellListIIR_t` | ? | [GC 0x100102e2] | [GC 0x1000cd61] |  |  |
| `4E536976` | `InventoryUpdateIIR_t` | ? | [GC 0x1000f54e] | [GC 0x1000c650] |  |  |
| `4F474E05` | `CorpseFullUpdateIIR_t` | S→C | [GC 0x1000efd0] | [GC 0x1000c35c] | 7 |  |
| `50544D19` | `FeedbackIIR_t` | ? | [GC 0x1000f148] | [GC 0x1000c427] |  |  |
| `51492120` | `CharSecSpecAttackIIR_t` | both | [GC 0x1000da92] | [GC 0x1000b7d5] | 3 | `N3Msg_SecondarySpecialAttack` 0x10028071 |
| `52213420` | `BankCorpseIIR_t` | ? | [GC 0x1000ebc8] | [GC 0x1000c133] |  |  |
| `52526858` | `GenericCmd_t` | both | [GC 0x1000dc68] | [GC 0x1000b8d2] | 3 | `N3Msg_UseItem` 0x100286f8 |
| `540E3B27` | `ArriveAtBsIIR_t` | ? | [GC 0x1000d8c4] | [GC 0x1000b6db] |  |  |
| `54111123` | `CharDCMoveIIR_t` | both | [GC 0x1000d9da] | [GC 0x1000b771] | 144 | `N3Msg_MovementChanged` 0x10018b5c, `FUN_1005a5d6` 0x1005a5d6 |
| `55220726` | `PlayfieldAllTowersIIR_t` | ? | [GC 0x1000fad0] | [GC 0x1000c944] |  |  |
| `55682B24` | `KnubotFinishTradeIIR_c` | C→S | [GC 0x1000de9c] | [GC 0x1000b9fe] |  | `N3Msg_NPCChatEndTrade` 0x10017fb1 |
| `55704D31` | `KnubotAnswerListIIR_c` | ? | [GC 0x1000f668] | [GC 0x1000c6e6] |  |  |
| `56353038` | `StopLogoutIIR_t` | ? | [GC 0x1000e76a] | [GC 0x1000bece] |  |  |
| `570C2039` | `CharInPlayIIR_t` | both | [GC 0x1000da36] | [GC 0x1000b7a3] | 2 | `N3Msg_SendInPlayMessage` 0x10018090 |
| `58362220` | `ShopUpdateIIR_t` | ? | [GC 0x1001010e] | [GC 0x1000cc64] |  |  |
| `58574239` | `MechInfoIIR_t` | ? | [GC 0x1000f7e0] | [GC 0x1000c7ae] |  |  |
| `58742A0F` | `RemovePetIIR_c` | ? | [GC 0x1000fd62] | [GC 0x1000caa2] |  |  |
| `59210126` | `PlayfieldAllCitiesIIR_t` | ? | [GC 0x1000e302] | [GC 0x1000bc66] |  |  |
| `59313928` | `TrapItemFullUpdateIIR_t` | ? | [GC 0x10010514] | [GC 0x1000ce8d] |  |  |
| `5A585F65` | `InspectIIR_c` | ? | [GC 0x100107a4] | [GC 0x1000cff1] |  |  |
| `5B1E052C` | `PlayfieldTowerUpdateClientIIR_t` | ? | [GC 0x1000fb8c] | [GC 0x1000c9a8] |  |  |
| `5C240404` | `ServerPosDebugInfoIIR_c` | ? | [GC 0x1000ff38] | [GC 0x1000cb6a] |  |  |
| `5C436609` | `QuestAlternativeIIR_t` | C→S | [GC 0x100c9806] | [GC 0x100c976b] |  | `N3Msg_GenerateMissions` 0x1001620e |
| `5C4A493A` | `FullAutoIIR_t` | ? | [GC 0x1000f2c0] | [GC 0x1000c4ef] |  |  |
| `5C654B28` | `MissedAttackInfoIIR_t` | S→C | [GC 0x1000f89c] | [GC 0x1000c815] | 11 |  |
| `5D70532A` | `KnubotAppendTextIIR_c` | ? | [GC 0x1000f6c6] | [GC 0x1000c718] |  |  |
| `5E477770` | `CharacterActionIIR_t` | both | [GC 0x1000daf0] | [GC 0x1000b807] | 22 | `N3Msg_Duel_Challenge` 0x1001d2ac, `N3Msg_Duel_Accept` 0x1001d349, `N3Msg_Duel_Refuse` 0x1001d467 (+61) |
| `5F4A4C6C` | `ImpulseIIR_c` | ? | [GC 0x10010802] | [GC 0x1000d023] |  |  |
| `5F4B1A39` | `PlayfieldAnarchyFIIR_t` | S→C | [GC 0x1000fb2e] | [GC 0x1000c976] | 1 |  |
| `5F4B442A` | `ChatTextIIR_t` | ? | [GC 0x1000edfa] | [GC 0x1000c25f] |  |  |
| `5F52412E` | `GameTimeIIR_t` | S→C | [GC 0x1000f37c] | [GC 0x1000c556] | 1 |  |
| `60201D0E` | `SetWantedDirectionIIR_t` | S→C | [GC 0x1000fff4] | [GC 0x1000cbce] | 72 |  |
| `62741E15` | `AOTransportSignalIIR_c` | ? | [GC 0x1000d808] | [GC 0x1000b677] |  |  |
| `64582A07` | `OrgServerIIR_c` | ? | [GC 0x1000f9b6] | [GC 0x1000c8ab] |  |  |
| `6B333303` | `PetCommandIIR_c` | C→S | [GC 0x1000e2a6] | [GC 0x1000bc34] |  | `N3Msg_SendPetCommand` 0x1001b261, `FUN_1003fba6` 0x1003fba6, `FUN_1005555f` 0x1005555f |
| `6C6C756E` | `null` | ? | [GC 0x100c97a8] | [GC 0x100c9739] |  |  |
| `6E5F566E` | `SetStatIIR_t` | ? | [GC 0x1000e536] | [GC 0x1000bd95] |  |  |
| `734E5A7B` | `SetNameIIR_t` | ? | [GC 0x1000e4d8] | [GC 0x1000bd63] |  |  |
| `742E2314` | `StopMovingCmd_t` | ? | [GC 0x1000e7c6] | [GC 0x1000bf00] |  |  |
| `754F1115` | `SpecialAttackInfoIIR_t` | S→C | [GC 0x10010226] | [GC 0x1000ccfd] | 3 |  |
| `77230927` | `GiveQuestToMembersIIR_t` | C→S | [GC 0x1000dcc6] | [GC 0x1000b904] |  | `N3Msg_UpdateNearbyTeamMembers` 0x10017dfb |
| `7864401D` | `KnubotStartTradeIIR_c` | C→S | [GC 0x1000dfb6] | [GC 0x1000ba94] |  | `N3Msg_NPCChatStartTrade` 0x1001cee9 |
| `7A222202` | `GfxTriggerIIR_t` | ? | [GC 0x1000f3da] | [GC 0x1000c588] |  |  |
| `7F405A16` | `NewLevelIIR_t` | ? | [GC 0x1000f8fa] | [GC 0x1000c847] |  |  |
| `7F4B3108` | `OrgClientIIR_c` | C→S | [GC 0x1000e1ea] | [GC 0x1000bbc3] |  | `N3Msg_OrgDisbandConfirmed` 0x1001a611, `N3Msg_LeavePayTaxConfirmed` 0x1001a6a0, `N3Msg_TaxChangeLeaveConfirmed` 0x1001a732 (+6) |
| `7F544905` | `VendingMachineFullUpdateIIR_t` | S→C | [GC 0x100105d0] | [GC 0x1000cef4] | 1 |  |

## 9. Unresolved

* Direction `?` for 86 registry rows (see §8) — only client-side senders could be enumerated, not server behaviour.
* Meaning of the server-side relay (what the server does on `CharInPlay`): no server code available; capture client never sent one.
* `MovementAction_e` names for `0x0A, 0x0D, 0x18, 0x1C, 0x24` and the exact press/release polarity (§5.1).
* The conditions in `N3Msg_MovementChanged` that suppress a send (movement-mode checks via `vtable+0x90/+0xc`).
* Text messages: identity semantics, NUL accounting, GUI sender of AFCM `0x16d/0x129/0x170` (§6).
* Second `CharDCMoveIIR_t` producer `FUN_1005a5d6` [GC] (not read).
* What triggers AFCM `0xE4` (`N3ActivatedMessage`, → ZoneLogin).
* Layout of all other client IIRs (§7).
* `PlayfieldAnarchyFIIR_t::Activate` not decompiled; the claim "no client send on the PlayfieldAnarchyF path" rests on the complete list of Gamecode functions that call `SendIIRToServer`/`SendIIRToObservers` (§7: none of them is a playfield/IIR-activation function).
