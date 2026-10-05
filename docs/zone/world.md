# Zone N3 world messages (`crates/ao-net/src/n3/world.rs`)

Nine messages that put the player into the world: playfield, own character, game time, org,
quests, appearance, character action, vending machines and corpses. All are
`n3InfoItemRemote_t` subclasses, so the common frame is (docs/zone.md §2):

```
payload: u32 msg_type | i32 id.kind | i32 id.instance | u8 flag | ReadSubClass body   (all BE)
```

Evidence conventions: `[GC 0x..]` = Gamecode.dll VA, `[N3 0x..]` = N3.dll VA, `[DBC 0x..]` =
DatabaseController.dll VA, `[GD 0x..]` = GameData.dll VA (all 32-bit images as loaded by Ghidra,
image base 0x10000000). Bytes come from `docs/captures/zone_ithaca.rec` (frames `<`, ptype 0xA).

## 0. How the message id is derived (new, verified)

`n3InfoItemRemote_t::Register(name, factory)` [N3 0x100099e4] stores the factory under
`MapToKey(name)` [N3 0x10009826]: `key = 0; for i, c in name: key ^= (i8)c << ((i & 3) * 8)`.
`n3InfoItemRemote_t::Construct(key, stream)` [N3 0x10009b08] reads `i32 key`, looks the factory up,
builds the object, reads `Identity` (`FUN_10037d8a`: `i32 kind, i32 instance`), a `u8` that must be
0 or 1 (stored at object `+0x0C`, 1 = "to be passed on"), then calls vtable slot 7
(`ReadSubClass`, slot 8 = `Write`). `n3InfoItemRemote_t::Write` [N3 0x100098f4] is the inverse.
The ids are therefore not constants in the DLL (a byte search for e.g. `0x5F4B1A39` finds
nothing); the registration `FUN_1000fb2e` [GC] pushes the string `"PlayfieldAnarchyFIIR_t"` and
`FUN_1000c976` as factory. `key("PlayfieldAnarchyFIIR_t") == 0x5F4B1A39` is asserted in a test
for all nine classes below.

| id | class (RTTI) | vftable [GC] | decoded as |
|---|---|---|---|
| 5F4B1A39 | `PlayfieldAnarchyFIIR_t` (base `n3PlayfieldFullUpdateIIR_t`) | 0x10170078 | `PlayfieldAnarchyF` |
| 29304349 | `FullCharacterIIR_t` | 0x10160f68 | `FullCharacter` |
| 7F544905 | `VendingMachineFullUpdateIIR_t` | 0x10166e0c | `VendingMachine` |
| 5F52412E | `GameTimeIIR_t` | 0x1015d3cc | `GameTime` |
| 2E2A4A6B | `OrgInfoPacketIIR_t` | 0x101611a0 | `OrgInfo` |
| 465A4061 | `QuestFullUpdateIIR_t` | 0x101673a8 | `QuestFullUpdate` |
| 41624F0D | `AppearanceUpdateIIR_c` | 0x10160cc4 | `AppearanceUpdate` |
| 5E477770 | `CharacterActionIIR_t` | 0x10160e00 | `CharacterAction` |
| 4F474E05 | `CorpseFullUpdateIIR_t` | 0x10166a5c | `Corpse` (partial) |

Rust API: `world::decode(&N3Header, &mut Reader) -> Result<Option<World>>`; `Ok(None)` for other
ids. None of these is sent by the client in the capture, so there is no `encode`.

Shared container encoding (everywhere in these bodies): a list is `u32 W = (n + 1) * 0x3F1`
followed by `n` elements; readers reject `W % 0x3F1 != 0` (`x / 0x3f1 - 1` in `FUN_100742d5`,
`FUN_1002b8b6`, `FUN_1002a41a`, ...). An empty list is therefore `000003f1`.

---

## 1. `PlayfieldAnarchyFIIR_t` 0x5F4B1A39 — load a playfield

Header identity = `{0x9C50 (playfield), playfield id}`, frame sender = 1. One message per zone
entry (4853 ms, the first N3 message after the login handshake).

Reader [N3 0x10029c24] `n3PlayfieldFullUpdateIIR_t::ReadSubClass`, wrapped by Gamecode slot 7
[GC 0x10125390] which appends two more `i32` after it:

| payload off | type | field | meaning |
|---|---|---|---|
| 13 | i32 | `version` | stored at `this+0x18`; **4** live. `>1` => proxy follows, `>3` => DbObject follows |
| 17 | f32 x3 | `position` | `this+0x1C/0x20/0x24` (`Vector3_t`); **(927.0276, 23.4604, 742.7181)**; Y = height |
| 29 | u8 | `'a'` (0x61) | `PlayfieldProxy_t` version; anything else throws "Invalid playfieldproxy version" [GC `FUN_10038402`] |
| 30 | Identity | `proxy.playfield` | `{0xC79C, 4582}` (`this+0x28`); kind picks the factory, instance = playfield id |
| 38 | i32 | `proxy.attribute` | 0 live (`this+0x30`) |
| 42 | i32 | `proxy.exit_door` | 0 live (`this+0x34`) |
| 46 | Identity | `proxy.exit_door_id` | `{0x9C50, 4582}` (`this+0x38`); `.instance` is the **RDB key** of the playfield record |
| 54 | Identity | DbObject identity | `{0xC77D, 0}`; `{0,0}` => no object. Peeked, then `DbObject_t::CreateObject` + `Read` (`+0x40`) |
| 62 | i32 | `revision` | `AoDbObject_t::ReadBlob` [DBC 0x10004ad0] reads three `i32` (identity + this); 1 live. [GUESS] db revision (name only: `GetOwnedBuildingDbRevision`) |
| 66 | i32 | marker | must be 1 (`FUN_10124871` [GC 0x10124871] returns false otherwise) |
| 70 | i32 | `n` buildings | `FUN_1013e228`: `n` x { `i32 a; i32 m; m x (i32, i32, i32)` } (`FUN_1013e128`); 0 live. Table `BUILDING_DATA` (`FUN_10120777`), class `TemplatePlayfieldGeneratorData_t` vftable [GC 0x1016ff54] |
| 74 | i32 | `world_x` | wrapper slot 7: `this+0x44` -> `PlayfieldAnarchy_t+0xAC` (`SetPFWorldXPos`); **100000** |
| 78 | i32 | `world_z` | `this+0x48` -> `PlayfieldAnarchy_t+0xB0` (`SetPFWorldZPos`); **100000** |

Total 82 bytes. Example (hex, payload; `|` added):
`5f4b1a39 | 00009c50 000011e6 | 00 | 00000004 | 4467c1c5 41bbaedb 4439adf5 | 61 | 0000c79c 000011e6 | 00000000 | 00000000 | 00009c50 000011e6 | 0000c77d 00000000 | 00000001 | 00000001 | 00000000 | 000186a0 000186a0`
=> version 4, position (927.03, 23.46, 742.72), proxy {0xC79C:4582, 0, 0, 0x9C50:4582},
generator {0xC77D:0, rev 1, no buildings}, world pos (100000, 100000).

**There is no heading in this message.** The rotation of the player is not carried here.

### What the client does with it

`PlayfieldAnarchyFIIR_t` slot 2 (`Activate`) [GC 0x101253f7]:

1. `n3PlayfieldFullUpdateIIR_t::Activate` [N3 0x10029b0e]:
   * `n3Playfield_t::GetPlayfield(Identity)` [N3 0x1000cfda] looks the playfield up by the
     **header** identity's instance (4582) in the global playfield directory; if it already
     exists nothing happens.
   * Otherwise `engine->GetPlayfieldFactory(proxy)` (`n3EngineClientAnarchy_t` slot 3)
     [GC 0x10018804] chooses by `proxy.playfield.kind`: 0xC79C -> `AnarchyPlayfieldFactoryClient_t`
     (vftable [GC 0x10157468]); 0xC79D / 0xC79E / 0xC79F,0xC7A1 -> building / virtual / other
     factories (`FUN_1011ecd8`, `FUN_1011f0b1`, `FUN_100c94ec`). 0xC77D generator data are only
     consumed on the 0xC79D path (`FUN_1011f0d6` builds a `TemplatePlayfieldGeneratorData_t` with
     `{0xC77D, 1}` and writes the proxy into it).
   * If the message carried a DbObject it is serialised into a scratch `BinaryStream` (vtable `+0x1C`)
     and handed to the factory (vtable `+0x10` = slot 4, an empty function for the 0xC79C factory).
   * `factory->vt[3](proxy)` = `FUN_1011ee6d` [GC]: builds the key
     `Identity{0xF4241, proxy.exit_door_id.instance}` and calls `PlayfieldAnarchy_t::ReadPlayfield`
     [GC 0x10122c4c], which asks the resource database (`n3DatabaseHandler_t` -> `rdb`, vtable `+8`)
     for that record as `RDBPlayfieldAnarchy_t` (**RDB type 0xF4241 = 1 000 001, id = playfield id,
     i.e. `{1000001, 4582}` live**). Failure prints "unable to start playfield %s: couldn't load
     playfield resource %u:%u". On success it also loads `{0xF425A, id}` (`RDBDynelLoader_t`,
     static dynels) and `{0xF4267, id}` (city data, `LoadPlayfieldCityData`).
   * `factory->vt[5](playfield)` = `FUN_1011eeb4` loads `{0xF425D, proxy.playfield.instance}`
     (`PlayfieldAreaInfo_t`), `{0xF424E, ...}` (`PlayfieldDistrictInfo_t`),
     `{0xF4248, ...}` (`LandControlMap_t`).
   * The new playfield gets the DbObject (playfield vtable `+0x10`), is finished by `factory->vt[5]`,
     `n3Root_t::AddPlayfieldRoot`, `engine->NewPlayfield(playfield, &position)`
     [N3 0x10007670: stores `m_nPlayfieldInstance = playfield+0x1C` and calls
     `n3EngineClientAnarchy_t::PlayfieldInit(instance)` [GC 0x10016e2c] which creates tilemap
     children, effect handler, sky/ground objects, `SandyInterfaceModule_t::ActivateGameZone`] and
     `CalculateWaterHeightMax`. **`NewPlayfield` ignores its position argument.**
2. Back in Gamecode: `PlayfieldAnarchy_t+0xAC/+0xB0 = world_x/world_z`; an `n3StatelController_t`
   is added; then `PlayfieldAnarchy_t::AddCellMonitor(position)` [GC 0x10121754] (outdoor) or
   `AddRoomMonitor(position, controller)` (`IsDungeon` tilemap). `n3CellMonitor_t::SetGlobalPos`
   makes the **cell monitor centre = the message position** [INFERENCE for the purpose: a cell
   monitor is the object that decides which cells around a point are active/loaded; the monitor's
   own code was not traced].

So the message position is a **streaming/camera origin**, not the player-dynel placement. The
player dynel itself is created and positioned by `SimpleCharFullUpdateIIR_t` (0x271B3A6B, owned by
`n3/dynel.rs`): in the capture that message for character 25988 (t = 5003 ms) carries the exact
same three floats `4467c1c5 41bbaedb 4439adf5` followed by its rotation quaternion, so the
playfield position equals the player's spawn position (evidence: capture, not a client code
proof). `FullCharacter` (below) does not contain a position.

Unresolved: what `n3CellMonitor_t` does with `position` beyond `SetGlobalPos` (not traced);
what `attribute`/`exit_door` do for non-0xC79C playfields (0 live); the DbObject `revision`.

---

## 2. `FullCharacterIIR_t` 0x29304349 — own character state (1971 bytes)

Header identity `{0xC350, 25988}` (the player), flag 0, sender 1. Reader [GC 0x10073881];
Activate [GC 0x10073a2f] returns immediately unless `n3Dynel_t::GetDynel(identity)` is already a
`SimpleChar_t` (so it must follow the `SimpleCharFullUpdateIIR_t` of the same id; live it does,
5003 ms).

Body (offset from payload start; sizes in the live message):

| off | type | field | notes |
|---|---|---|---|
| 13 | u32 | `version` | **26** (`DAT_101c01ec`); other => reader fails |
| 17 | list | inventory (`this+0x70`) | `FUN_1002a41a`: `u32 W`, per element `u32 slot; i16; i16; Identity; ACGItem_t` (`FUN_1002a04a`, `GameData::operator>>`). **Empty (`3f1`)**. Non-empty => decoder stops, `rest` = bytes from this size word |
| 21 | list | `list_18` | `FUN_100742d5`: `i32` elements; empty |
| 25 | list | `triples_24` | `FUN_1002d04e`/`FUN_1002cfe5`: 3 x `u8`; empty |
| 29 | 3 groups | `groups` (`+0x28/+0x2C/+0x30`) | `FUN_1003bbad`: `u32 header` (1 live), `u32 count` (plain, 0 live), count x { `u32 ignored; Identity; i32; i32` } (`FUN_1003bb2d`); applied through `FUN_1007434f` to dynel members `+0x1BC/+0x1C0`. Meaning [UNRESOLVED] |
| 53 | list | `stats_a` (`+0x34`) | **81** x `{u32 stat, i32 value}` (0x14332 = 82 x 0x3F1). Value `0x499602D2` is skipped by the client |
| 705 | list | `stats_b` (`+0x38`) | **147** x `{u32 stat, i32 value}` |
| | list | `stats_u8` (`+0x3C`) | 8 x `{u8 stat, u8 value}` (`FUN_1002eab4`) |
| | list | `stats_i16` (`+0x40`) | 14 x `{u8 stat, i16 value}` (`FUN_1002e744`; value sign-extended) |
| | i32 + pairs | `stat_map` (`+0x44`) | plain `i32 n` then `{i32 id, i32 value}` (`FUN_10009dd4`); n = 0 |
| 1955 | u32 | `equipment_flags` | bit0 => `Identity` (+0x60) and team/equipment blocks (`FUN_10125c59`/`FUN_10125dfc`) follow; bit1 => 6 blocks + `i32` (+0x68) instead of 1. Live 0. Non-zero => decoder stops, `rest` starts with this word |
| 1959 | list | `list_6c` | `FUN_1002b8b6`: `Identity` elements; empty |
| 1963 | list | spells (`+0x78`) | `FUN_100a6c58`: `GameData::SpellData_t` records (type-tagged, `SpellFormats_c::ReadBinary` [GD 0x1000f4a6]); empty. Non-empty => `rest` |
| 1967 | list | perk map (`+0x90`) | `FUN_100537a5` ("inconsistent PerkMap!"); empty. Non-empty => `rest` |

The 1971 bytes are consumed exactly. Application order in Activate: `stats_a`, `stats_b`, `stats_u8`,
`stats_i16` are each applied with the dynel's stat object (dynel `+0xE8`, vtable `+0x48` has-stat,
`+0x44` create, `+0x40` set) unless the value is `0x499602D2`; then inventory, `list_18`, triples,
the three groups, equipment, spells; effects 0x2cf2/0x2ced/0x2d50 are created on the dynel.

Stat names below come from the client's own id -> name table built by `FUN_1002f009` [GC]
(523 ids recovered, max id 1002). Live values for character "Testy":

| stat | name | value | | stat | name | value |
|---|---|---|---|---|---|---|
| 54 | Level | 1 | | 12 | Mesh | 17530 |
| 60 | Profession | 1 | | 27 | Health | 34 |
| 4 | Breed | 1 | | 11 | PreviousHealth | 50 |
| 59 | Sex | 3 | | 61 | Cash | 1000 |
| 16..21 | Strength, Agility, Stamina, Intelligence, Sense, Psychic | 6 each | | 53 | IP | 1500 |
| 52 | XP | 0 | | 350 | NextXP | 1450 |
| 0 | Flags | 528961 | | 673 | VisualFlags | 31 |
| 181 | MaxNCU (i16 group) | 8 | | 173 | CurrentMovementMode (u8 group) | 3 |
| 224 | Features | 6 | | 360 | MonsterScale | 100 |
| 214 | CurrentNano | 32 | | 221 | MaxNanoEnergy | 1 |

`FullCharacter::stat(id)` searches all four groups. Why the wide stats are split in two lists
(`stats_a` begins `7 State, 418, 615 InvadersKilled, ... 673 VisualFlags, 674.. PVP*, 649
UnreadMailCount`; `stats_b` begins `68, 69, 672, 349, 275 XPKillRange, 194, 27 Health, 1 Life, 21..16
abilities, 61 Cash, 60 Profession, ... 12 Mesh, 4 Breed, ...` followed by the skill block all = 5) is
**unresolved**; [GUESS] the server serialises two stat classes separately. Some ids (349, 650, 594..597,
58..) have no name in the client table.

Example (first bytes): `29304349 0000c350 00006584 00 0000001a 000003f1 000003f1 000003f1
00000001 00000000 00000001 00000000 00000001 00000000 00014332 00000007 00000000 000001a2 ...`.

---

## 3. `VendingMachineFullUpdateIIR_t` 0x7F544905

Header identity `{0xC75B, 100}` (the client creates the dynel only for kind 0xC75B:
Activate [GC 0x100a2672] `GetDynel == null && kind == 0xc75b` -> `CreateDynel`). Body = the shared
**dynel base** chain (slot 7 = `FUN_100a2608` -> `FUN_1009f340` [version 3] -> `FUN_100a0730`
[version 2] -> `FUN_100a110a` [version 11]); it consumes the message exactly.

| off | type | field | live |
|---|---|---|---|
| 13 | u32 | version = 11 (`DAT_101c1160`) | 11 |
| 17 | Identity | `parent` (`+0x24`) | `{0,0}`; position and rotation follow **only when kind == 0** |
| 25 | f32 x3 | `position` | (940.2288, 47.2100, 875.3401) |
| 37 | f32 x4 | `rotation` quaternion x,y,z,w | (0, 0.7133, 0, -0.7009) |
| 53 | i32 | `playfield` (`+0x48`) | 4582 |
| 57 | Identity | `template` (`+0x4C`) | `{1000015, 0}` (0xF424F) |
| 65 | u8 u8 | flags (`+0x70/+0x71`) | 0, 0x6F |
| 67 | list | `stats` (`FUN_1002e618` -> `+0x5C`) | 9 x `{u32 stat, i32}`: (0, 0x80023203) (23, 248371) (701,0) (702,0) (703,0) (412,1) (501,2) (500,0) (12, 93117) |
| | i32 + bytes | blob (`+0x6C`) | 0 bytes here (corpses: NUL terminated name) |
| | u32 | version2 = 2 (`DAT_101c1084`) | |
| | i32 | `x4ac` | 50; stored at `(*(this+0x60))+0x4AC` [UNRESOLVED meaning] |
| | list | `list_74` (`FUN_1002b8b6`) | `Identity` elements; empty |
| | u32 | version3 = 3 (`DAT_101c0f80`) | |

Example: `7f544905 0000c75b 00000064 00 0000000b 00000000 00000000 446b0ea4 423cd70f 445ad5c4
00000000 3f369794 00000000 bf336ecb 000011e6 000f424f 00000000 00 6f 0000276a ...` (163 bytes).
Unresolved: meaning of stats 23/701-703/412/500/501 on a vending machine, of `template`.

## 4. `GameTimeIIR_t` 0x5F52412E

Reader [GC 0x10039564], Activate [GC 0x100395ea] -> `GameTime_t::Update(float, DayPeriod_e, int, int)`
[GC 0x1000b526]. Header identity `{0xC350, 25988}`, 29 payload bytes.

| off | type | field | live |
|---|---|---|---|
| 13 | f32 | `time` | 67170.0 (`4783 3100`); split with `% +0x88` / `% +0x84` / `fmod` into `GameTime_t+0x40/0x44/0x48/0x50` |
| 17 | i32 | `day_period` | 1 (`DayPeriod_e`; `Update` ignores it and runs `UpdateDayPeriod`) |
| 21 | i32 | `arg3` -> `GameTime_t+0x4C` | 276425 (0x437C9) [UNRESOLVED] |
| 25 | i32 | `arg4` | 0x6AC4214E = server unix time; `+0xC0 = arg4`, `+0xB8 = arg4 - _time64()` |

Bytes: `5f52412e 0000c350 00006584 00 47833100 00000001 000437c9 6ac4214e`.

## 5. `OrgInfoPacketIIR_t` 0x2E2A4A6B

Reader [GC 0x10075faa], Activate [GC 0x10075f20]: dynel stat **5** := `org_id`, then the name
goes to the org-name registry (`FUN_1003f4fb`). Body: `i32 org_id; str_i16 name` (u16 length, <
0x8000, `FUN_100388e2`). Live (19 bytes): `2e2a4a6b 0000c350 00006584 00 | 00000000 | 0000` =>
org_id 0, name "" (character in no org).

## 6. `QuestFullUpdateIIR_t` 0x465A4061

Reader [GC 0x100aca12]: `QuestList` (`FUN_100abd64`: size word; per quest `Identity` + a 0xF8-byte
object read by `FUN_100ab951`; failure text "inconsistent QuestList..") then `u8` (stored `+0x28`,
passed to `FUN_10056796(quest, flag)` for every quest). Live (18 bytes):
`465a4061 0000c350 00006584 01 | 000003f1 | 00` => 0 quests, flag 0. **Quest bodies are not
decoded**: a non-empty list leaves everything after the size word in `rest`. Note the header
flag byte is `01` here (to-be-passed-on).

## 7. `AppearanceUpdateIIR_c` 0x41624F0D

Reader [GC 0x100718dc]; Activate [GC 0x10071679] (uses the dynel found by header identity): clothing
via `FUN_100480fc`, stat **0x2A1 = 673 VisualFlags** := `visual_flags`, `FUN_100572b9(extra, 0)`,
attractors -> `VisualCATMesh_t::ClearAttractors` / `CharacterMesh::AddAttractors`.

| type | field |
|---|---|
| list of `ClothData_t` | `u32 W`; per element `i32 packed; i32 b; i32 c` plus `i32 d; i32 e` only when `packed > 0` and its high 16 bits `> 0` ([GD 0x1000a661]); `id` = low 16 bits sign-extended when `packed > 0` |
| list of attractors | `FUN_10071b49`/`FUN_10001da1`: `u8 a; i32 b; i32 c; u8 d` |
| i16 | `visual_flags` |
| u8 | `extra` [UNRESOLVED] |

Live: own char (94 bytes): 5 cloth `(slot 0..4, 0, 0)`, 1 attractor `{a 0, b 0x9EB5, c 0, d 4}`, flags 0x1F
(= stat 673 = 31 in `FullCharacter`), extra 0. NPC 1026282 (84 bytes): same 5 cloth, no attractors, 0x1F.
Hex (own): `41624f0d 0000c350 00006584 00 000017a6 00000000 00000000 00000000 00000001 ... 00000004
00000000 00000000 000007e2 00 00009eb5 00000000 04 001f 00`. The cloth slot ids 0..4 are all that
is on the wire (no item/texture ids): the visible items come from other state. [UNRESOLVED] what `b`,
`c`, `d`, `e`, attractor `a/b/c/d` mean.

## 8. `CharacterActionIIR_t` 0x5E477770

Reader [GC 0x100724e8], Activate [GC 0x10072410] -> `FUN_1005d0d8(action, &identity_a, param,
&identity_b, &text)` only if the header identity is a `SimpleChar_t`. Body (26 bytes):
`i32 action; i32 param; Identity identity_a; Identity identity_b; str_i16 text`.
Live (22 messages, sender 1): `action` 0xA7 (167, own char at spawn), 0x63 (99; NPC dynels,
`identity_b = {0, 503}`), 0x62 (98; `identity_a = {0xCF1B, 0x27E79}`, `identity_b = {1026282, 0}`),
0xAD (173). `text` always empty. The action id space (`CharacterAction_e`?) was not traced: [UNRESOLVED].

## 9. `CorpseFullUpdateIIR_t` 0x4F474E05 (partial)

Header `{0xC76A, n}` (7 live, 393-427 bytes). ReadSubClass [GC `FUN_1009f502`]: `u32 version = 8`,
dynel base (§3; `blob` = corpse name e.g. `"Remains of Cross-Wired Junkbot\0"`, position/rotation of
the corpse, 18 stat pairs, `x4ac` 50), then:

* `FUN_100a6c58` spell list: `u32 W` + `GameData::SpellData_t` records. Live **1 spell** (W =
  0x7e2) of type 0xCF27; the bytes between the size word and the owner identity are
  `0000cf27 00001238 00000004 00000000 00000001 00000000 00000000 00000000 00000000 00000000 000001f7 00000001 00000004 0000b331 00000000`.
  Spell records are type-driven (`SpellFormats_c::ReadBinary` [GD 0x1000f4a6] -> `GetFormat(typeid)` ->
  `SpellFormat_c::ReadBinary` -> per-argument `BinaryToValue`); the per-type format table for 0xCF27
  built in `SpellFormats_c::SpellFormats_c` [GD 0x1000fb0a] was **not** decoded, so the list is
  left in `Corpse::rest` (starting at the size word).
* then (from the reader, not exercised by the decoder): `Identity` owner at `+0x84` (live
  `{0xC350, 1026310}`: the dead character, visible in the bytes after the spell), `vector<ClothData_t>`,
  `i32 n` (`!= 0` => `vector<TextureData_t>`: 32-byte name, 2 x i32, i32).

Activate [GC 0x1009f5a4]: creates the corpse dynel (if absent), copies the owner identity, moves
the corpse to the dead character's relative position when closer than a threshold, sets effects.

---

## Unresolved / not done (explicit)

* `PlayfieldAnarchyF`: DbObject `revision` meaning; `attribute`/`exit_door` semantics;
  non-0xC77D DbObject kinds (decoder keeps them in `rest`, then `world_x/z` stay 0).
* `FullCharacter`: element layouts of inventory (`ACGItem_t`), equipment/team blocks, spells
  (`SpellData_t`), perk map and quest bodies; meaning of the three entry groups and of the
  `stats_a`/`stats_b` split; ~180 stat ids have no name in the table that was recovered.
* `Corpse`: everything after the spell list (owner identity, cloth, textures) is raw `rest`.
* `GameTime.arg3`, `AppearanceUpdate.extra` and cloth `b..e`, `CharacterAction` action ids,
  `DynelBase.x4ac`, `flags`, `template`.

## 10. Time of day and playfield changes in the app (`play/zone.rs`, `play/flow.rs`)

### 10.1 `GameTimeIIR_t` -> sky clock (RE of `GameTime_t`, Gamecode.dll, Ghidra disassembly of 0x1000b526)
* **Ctor** `GameTime_t::GameTime_t` [GC 0x1000af71]: `+0x84 = +0x88 = 60` (seconds per minute / minutes per hour), `+0x8c = 3600`,
  `+0x80 = 27` (**hours per day**), day-length double `+0x60 = 97200.0` [GC 0x10155ee8] (= 27 x 3600), `+0x68 = 24300.0`,
  `+0x70 = 72900.0` (quarter / three quarters of the day, `SubtractTime[Exact]` wrap tests), `+0x5c = 15` (`TimeSpeed`,
  `SetTimeSpeed` [GC 0x1000b1b5]), `+0x90/+0x94 = 21/2` dusk start / length, `+0x98/+0x9c = 7/2` dawn start / length (hours),
  `+0xa4/+0xa8/+0xac/+0xbc = 1.0` (speed factors).
* **`Update(float time, DayPeriod_e, int day, int unix)`** [GC 0x1000b526, from the `GameTimeIIR_t::Activate` [GC 0x100395ea]]:
  `ti = ftol(time)`; `+0x20 = time - ti` (fraction), `+0x40 = ti % 60` (second), `+0x44 = (ti/60) % 60` (minute), `+0x48 = ti / 3600`
  (hour, **not** wrapped at 27: the server sends a time inside the day), `+0x50 = fmod(hour*3600 + minute*60 + second, +0x60)` (**seconds
  into the 97200 s game day**, `GetCurrentRealTime` [GC 0x1000b44c] returns it), `+0x4C = day` (`arg3`: **the game day number**,
  so `GameTimeIIR_t.arg3` = 276425 is resolved), `+0xC0 = unix`, `+0xB8 = unix - _time64()`; then `UpdateDayPeriod` [GC 0x1000b1c2]
  (hour `< 7` night, `< 9` dawn, `< 21` day, `< 23` dusk, else night; ids 3/0/1/2).
* **Per frame** `RunFunction` [GC 0x1000b214]: `+0x20 += TimeSpeed * dt * factor` and `+0x50 += TimeSpeed * dt * factor` (`factor = 1.0`
  by day and night; `dt` = n3 engine frame time) with the carry second -> minute -> hour (`% 27`) -> day (`+0x4C`) when the
  fraction passes 1 s: the game clock runs **15 game seconds per real second**, a game day takes **6480 real seconds** (108 min).
* **Mapping to the sky clock** (`GameDayTime` 0..6480, `DayTimeFactor = GameDayTime / 6480`): `GameDayTime = GameTime+0x50 / 15`.
  Evidence: `DayTimeForGroundShadows = GameDayTime * 15` is compared with the ground-shadow table (bounds 0 .. 97700, GC 0x101c14d0,
  docs/formats.md) = the in-day game seconds, and the audio/viewer clock of 240 s per game hour is 3600 / 15. **Not found:** the code
  that writes the `GAME.GameDayTime` tweak value (no string reference in Gamecode.dll; FXS/other DLL), so this last step is [INFERENCE]
  from those two consumers; the clock rate and day length above are from the code. Live capture: `time = 67170.0` -> 18:39:30 ->
  `GameDayTime 4478` (day period 1 = day), the new-character capture 78435 -> 5229.
* **App**: `Zone::on_frame` stores `day_time_of(time)` (`(time mod 97200) / 15`) and the game day; `Zone::tick(dt)` runs it once
  per frame at 1 s/s (wraps at 6480); `ZoneEvent::Time` on every `GameTimeIIR_t` (also resyncs: the original's `Update` simply
  overwrites the clock). Before the first message the frozen default (`DEFAULT_DAY_TIME`) is used. The world is loaded with
  `load_playfield_on_day(.., zone.day_time(), zone.game_day)` (weather schedule seeded by the server game day instead of
  `OFFLINE_DAY`, see docs/formats.md *Weather*), and when it appears the viewer gets `Host::live_sky = LiveSky { start: zone clock,
  scale: 1.0, source: SkyClock::on_day(game_day) }` (the sky, sun, fog tint and light are re-evaluated twice a second); a later
  `GameTime` sets `Host::sky_clock`. Audio (`Audio::update`) uses the zone clock too. Limit: the weather game day of a running
  `SkyClock` is the one at world load (a resync across the 27 h day boundary does not change it) [ponytail].

### 10.2 Playfield changes
* **In-session zone change** `0x3C ZoneRedirection` (`ip[4]`, `u16 port`) [IF 0x10002a9e case 0x3C]: `ao_net::client` (zone phase)
  reconnects (3 tries, 1 s / 2 s backoff, like the first connect), sends `ZoneLogin` again with the cookies of the earlier
  `ZoneInfo` (protocol.md §5), keeps the wire tap and reports `LoginEvent::ZoneRedirect { zone_ip, zone_port }`; every following
  frame (`PlayfieldAnarchyF`, own character, `GameTime`, ...) is a `ZoneFrame` of the new connection. Test
  `zone_redirection_reconnects_with_the_same_cookies` (loopback servers). The original does this inside `Client_t` without the UI.
* **Teleport UI** (`FlowControlModule_t`, GUI.dll): `TeleportStartedMessage` [GUI 0x1002910e] (registered at [GUI 0x1002ad2c]) sets `m_isTeleporting`, emits the `GlobalSignals_c` text signal with LDB text `0x6e` and
  `+0x48`, clears `DisplaySystem+0x44`, sets `InputConfig+0x18 = 1` (input locked), stops `InputConfig+0x1a8` timer, sends
  `AFCM::Send(0x1e, 0x112)` and saves the preferences; `TeleportEndedMessage` [GUI 0x100292ce] (also the `CharInPlay` countdown, docs/zone/outgoing.md §3)
  clears the flag, reads `N3Msg_GetPosAndPF`, signals the new playfield (`BugReport_t::SetPFProxyInfo`, `N3Msg_GetPFName` /
  `N3Msg_IsDungeon` text). `ServerLogin3DModule_t` [GUI 0x100175f0] (program 5, module 0x1b) is the only loading-screen module;
  `AFCM::Send(.., 0x135)` (`StartClosingLoadscreen`) is called only from `ServerLogin3DModule_t`'s own setup [GUI 0x1001765c] and
  `FlowControlModule_t::AliveMessage` [GUI 0x10028543]; `cd_image/gui/Default/gfx` holds only `ai_loading_login.png`, `welcome_to_rubika.jpg`
  and the unused `loadingimage_fullscreen.jpg`: there is **no per-playfield loading image**.
* **UNRESOLVED [GUESS]**: whether the original shows `ServerLogin3DModule_t` (program 5) again for a teleport (the receiver of
  `AFCM (0x1e, 0x112)` and the sender of a second `AddProgram(5)` were not located: `Imm` scans of GUI.dll for 0x112 / 0x1e / 0x135
  find only the send sites above, no registration). The app shows the same loading screen (`ai_loading_login.png`, black + image fade
  in over `FADE_IN`, text "..Anarchy Online is loading..") without `PlayStartupMusic` (that is `CharacterLoggedInMessage`, the
  login only).
* **App** (`Play::begin_zone_change`): a `ZoneEvent::Playfield` or `LoginEvent::ZoneRedirect` while `Screen::InWorld` closes the
  in-world interface, drops the dynels / actors (`Zone::reset_world`), clears `in_play_sent`/`world_frames` and shows the
  loading screen again (`host.fly = false`); the new playfield is loaded in the background (a result for a playfield the server
  has since replaced is dropped), the player is placed from the new own `SimpleCharFullUpdate` when the screen dissolves
  and `CharInPlay` is sent after the same countdown (>= 11 frames) as the first time ([INFERENCE] from `SetMainDynel` -> event 6
  -> `TeleportEndedMessage` running again, docs/zone/outgoing.md §3). Tests: `flow::tests::second_playfield_shows_the_loading_screen_again`
  (captures `zone_newchar_ithaca.rec` then `zone_ithaca.rec`), `zone_redirect_shows_the_loading_screen`.
