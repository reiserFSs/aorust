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
| 4F474E05 | `CorpseFullUpdateIIR_t` | 0x10166a5c | `Corpse` |

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
| 29 | 3 groups | `groups` (`+0x28/+0x2C/+0x30`) | `FUN_1003bbad`: `u32 header`, plain `u32 count`, count x { `u32 ignored; Identity; i32 total; i32 remaining` } (`FUN_1003bb2d`); skill, perk-action and nano locks respectively (consumer trace below). |
| 53 | list | `stats_a` (`+0x34`) | **81** x `{u32 stat, i32 value}` (0x14332 = 82 x 0x3F1). Value `0x499602D2` is skipped by the client |
| 705 | list | `stats_b` (`+0x38`) | **147** x `{u32 stat, i32 value}` |
| | list | `stats_u8` (`+0x3C`) | 8 x `{u8 stat, u8 value}` (`FUN_1002eab4`) |
| | list | `stats_i16` (`+0x40`) | 14 x `{u8 stat, i16 value}` (`FUN_1002e744`; value sign-extended) |
| | i32 + pairs | `stat_map` (`+0x44`) | plain `i32 n` then `{i32 id, i32 value}` (`FUN_10009dd4`); n = 0 |
| 1955 | u32 | `team_flags` | bit0 => leader `Identity` (+0x60) and one team roster (`10125c59`/`10125dfc`); bit1 => own subgroup i32 (+0x68) and six raid rosters. Decoded before list6c/spells/perks, including non-empty teams. |
| 1959 | list | `list_6c` | `FUN_1002b8b6`: `Identity` elements; empty |
| 1963 | list | spells (`+0x78`) | `FUN_100a6c58`: type-tagged `SpellData_t` records; supported spell kinds decode, unsupported kinds retain `rest` and leave perks unavailable. |
| 1967 | list | perk map (`+0x90`) | `FUN_100537a5` ("inconsistent PerkMap!"); decoded at its actual position into `FullCharacter.perks`, including non-empty maps. |

The 1971 bytes are consumed exactly. Application order in Activate: `stats_a`, `stats_b`, `stats_u8`,
`stats_i16` are each applied with the dynel's stat object (dynel `+0xE8`, vtable `+0x48` has-stat,
`+0x44` create, `+0x40` set) unless the value is `0x499602D2`; then inventory, `list_18`, triples,
the three groups, team snapshots, spells; effects 0x2cf2/0x2ced/0x2d50 are created on the dynel.

### Retained metadata consumer closure (native evidence, source-ready)

* `FUN_10073a2f` appends all three lock vectors through `1007434f` without filtering zero/negative durations or deduplicating identities. Group0 targets the skill controller `SimpleChar+0x1bc` (constructor `10064194`), group1 the existing perk manager (`10058ab1`, locks via manager+8), group2 the nano controller `SimpleChar+0x1c0`, vector begin/end `+0x40/+0x44` (constructor `1005002c`, allocated by `1005cb6a`). Group0/1 already feed `Zone.skill_locks/perk_locks`, shared second pulses, action recharge UI and invocation feedback. Group2 feeds the existing `Dynels.nano_locks`: `N3Msg_IsFormulaReady` `100166c0` calls `1004ebb5`, which rejects exact nano identities and kind1 locks matching any of six nano families. `1004f063` reads those same locks for progress; `1005122c` advances their integer second pulses. There is no separate metadata registry.
* Perk-map reader `100537a5` requires positive size divisible by1009 and count<1000; each record key is followed by `10052859`. That reader supports an untagged ID plus three compatibility integers, or `ffffffVV`, ID, timer, and (only VV=1) those integers (`1002ba8c`). The key must equal the catalogue-resolved record ID. Catalogue lookup `1002bbcb` supplies the first seven runtime words; flags without `0xc0` force the timer to zero. `FullCharacter.perks=None` means an earlier unsupported spell block prevented reaching this field, not a known-empty map. The old heuristic decoding of `rest` as a perk map is removed.
  Native writer proof: map writer `10052994` emits `(count+1)*1009`, each key, then `10052813`; its complete entry-writer caller census is `100529cc` in `10052994`. `10052813` emits tag `ffffff02`, catalogue ID and timer only. `team::perk_map` now emits this proven current form instead of an assumed server version1; the reader regression retains version1 and untagged compatibility coverage. Map-reader caller census is exactly `100739dd` in `10073881`.
* Apply `10073a2f` transfers +0x90 through `10058ab1`/`100539fe`, replacing the manager map; `100539fe` invokes `10052e66`, replaying item event14 for timer-zero perks. `HudPerks` reuses its existing RDB1000036 catalogue, known map, wear-effect executor and completed-perk projection; unknown catalogue IDs do not fabricate perks. Ordinary/alien point counts, action availability, item criteria and personal/global research completion retain the same owners. Empty authoritative maps clear personal completion; unrelated unsupported tails leave it unchanged.
* Focused regressions: `full_character_nonempty_perks_have_a_bounded_stream_position`, `full_character_perks_are_not_guessed_from_an_unsupported_tail`, and existing `completed_projection_tracks_authoritative_timer_frames`, `native_skill_locks_membership_and_second_pulse`, `authoritative_perk_relays_update_points_effects_and_reset_time`.
* Independent cutover on public `0fb9d4b` includes only the necessary RetainedWire `05b5d36` team-snapshot dependency: `TeamSnapshot`/`TeamEntry` codec, `TeamState` roster/leader/subgroup restoration and SCFU PCfield108 team identity. No CastSource helper, Scene, Movement executor, Arms or Root contract is imported. Team entry validation is GC`10125d93` (nonempty name, character identity, side<=7, level<=999, profession<=15); apply is `10073a2f`→`10065951`, leader `10065718`. Regressions `native_team_snapshot_golden_and_malformed` and `full_character_native_rosters_restore_leader_and_raid_groups` cover these actual codec/runtime paths. The perk frame regression also reaches its typed map after all six raid rosters.
* Runtime regressions: `full_character_restores_existing_nano_lock_consumers` exercises actual message dispatch, exact/family locking, duplicate append and zero/positive expiry; `dynel_base_bytes_reach_runtime_stat_getters` exercises message dispatch through the real prop stat getter, signed values and explicit overrides.
* Authorized headless verification on public `0fb9d4b`: all nine named codec/perk/team/nano-lock/signed-seed/Masters regressions and the affected FullCharacter, completed-perk, progression-relay and team filters passed. `cargo test --release --workspace -j 2` exited0: **2159 passed, 64 ignored, 40 suites**. `cargo clippy --release --workspace --all-targets -j 2 -- -D warnings` exited0. Source remained frozen throughout both gates; only this evidence paragraph changed afterward. No login, window, input, screenshot or live session was used. Native CastSource execution and additional SCFU-stat changes are separate work and are not claimed covered by this cutover.


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
| 65 | u8 u8 | signed stat seeds (`+0x70/+0x71`) | `FUN_100a15f8` sign-extends these bytes into stats55/220 before explicit stats; explicit values override. |
| 67 | list | `stats` (`FUN_1002e618` -> `+0x5C`) | 9 x `{u32 stat, i32}`: (0, 0x80023203) (23, 248371) (701,0) (702,0) (703,0) (412,1) (501,2) (500,0) (12, 93117) |
| | i32 + bytes | blob (`+0x6C`) | 0 bytes here (corpses: NUL terminated name) |
| | u32 | version2 = 2 (`DAT_101c1084`) | |
| | i32 | `x4ac` | 50; stored at `(*(this+0x60))+0x4AC`; native-unused storage boundary below. |
| | list | `list_74` (`FUN_1002b8b6`) | `Identity` elements; empty |
| | u32 | version3 = 3 (`DAT_101c0f80`) | |

Example: `7f544905 0000c75b 00000064 00 0000000b 00000000 00000000 446b0ea4 423cd70f 445ad5c4
00000000 3f369794 00000000 bf336ecb 000011e6 000f424f 00000000 00 6f 0000276a ...` (163 bytes).
Unresolved: meaning of stats 23/701-703/412/500/501 on a vending machine, of `template`.

`DynelBase::applied_stats` projects the two signed stat seeds into every currently decoded public DynelBase producer's existing stat path (vending machines, doors, corpses); it does not invent flags or a second property store. Regressions `dynel_base_signed_byte_stats_precede_explicit_overrides` and `dynel_base_bytes_reach_runtime_stat_getters` cover negative bytes, explicit overrides and actual prop-stat retrieval. Native `100a15f8` subsequently applies the explicit stat list via stat-object virtual+0x40, so neither byte may override a transmitted stat55/220. Material's separate generic-item consumer must keep the same helper call when that independent feature is merged; it is not copied into this series.

**Native-unused boundary, x4ac:** a complete Gamecode instruction-operand scalar search (`FindScalar.java 4ac 10000`) returns exactly one address, `100a0768` in reader `100a0730`, which passes the destination address to BinaryStream. There is no instruction reading that member in this module. The complete caller census of `100a0730` is `1009f34b` in `1009f340`, `1009fa42` in `1009fa37`, and data/vtable reference `10166ba8` (no containing function). Reader `100a110a`, subclass reader `100a0730`, activation `100a2672` and generic stat application `100a15f8` were decompiled; none applies it. Complete scalar70/71 searches returned390/14 hits respectively (cap10000); the actual byte consumers are `100a162c/100a163d`, not any relationship to x4ac. Retain x4ac losslessly as stored-but-unread native state, not as an unimplemented runtime consumer or a guessed level/range/stat.


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

## 9. `CorpseFullUpdateIIR_t` 0x4F474E05 (decoded; look: `docs/zone/static.md` §5)

Header `{0xC76A, n}` (7 live, 393-427 bytes). ReadSubClass [GC `FUN_1009f502`]: `u32 version = 8`,
dynel base (§3; `blob` = corpse name e.g. `"Remains of Cross-Wired Junkbot\0"`, position/rotation of
the corpse, 18 stat pairs, `x4ac` 50), then:

* `FUN_100a6c58` spell list: `u32 W` + `GameData::SpellData_t` records (`Corpse::spells`). Live **1 spell** (W =
  0x7e2) of type 0xCF27: `0000cf27 <id> 00000004 | 00000000 | 00000001 00000000 00000000 00000000 00000000 00000000 000001f7 | 00000001 00000004 0000b331 00000000`
  = header (type, id, version 4), criteria count 0, the 7 format arguments (`SpellFormats_c::SpellFormats_c` [GD 0x1000fb0a]),
  then 4 integers whose reader was not found (`CorpseSpell::tail`, static.md §5). Other spell types are an error.
* `Identity` owner at `+0x84` (`Corpse::owner`, live `{0xC350, 1026310}` = the dead character = stats 415/416), `vector<ClothData_t>`
  (`Corpse::cloth`, 5 empty entries live), `i32 n` (`!= 0` => `vector<TextureData_t>`: 32-byte name, 2 x i32, i32; `Corpse::textures`, none live).

Activate [GC 0x1009f5a4]: creates the corpse dynel (if absent), copies the owner identity, moves
the corpse to the dead character's relative position when closer than a threshold, sets effects.

---

## Unresolved / not done (explicit)

* `PlayfieldAnarchyF`: DbObject `revision` meaning; `attribute`/`exit_door` semantics;
  non-0xC77D DbObject kinds (decoder keeps them in `rest`, then `world_x/z` stay 0).
* `FullCharacter`: the `stats_a`/`stats_b` split remains separate research; teams, the three lock groups and perk map have the consumer trace above. Inventory is decoded: `u32 slot; u16 flags; u16 count; Identity; ACGItem` (GC `1002a04a`). `InventoryEntry.b` is the unsigned count (`1002a7d2`, `1002ad45`), not an unknown field; its Rust i16 retains the raw wire bits. Flag bit0 selects template entries (`1002aa0f`), bit0x40 is `N3Msg_IsItemDisabled` (`10027489` → `1002a6ac`), and bit0x20 is the explicit alternative to Can30 bit0x40 in `1002ace0`, consumed by reclaim iteration `1012584a`. Do not infer server flag names from that predicate.
* `Corpse`: the 4 integers after the 7 spell arguments (`CorpseSpell::tail`) and spell types other than 0xCF27 (static.md §5).
* `GameTime.arg3`, `AppearanceUpdate.extra` and cloth `b..e`, `CharacterAction` action ids,
  `DynelBase.template`. `DynelBase.x4ac` is retained native-unused storage, not an unavailable architecture consumer; byte70/71 are stat55/220 seeds (above).

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
* **Mapping to the sky clock [CODE]** (`GameDayTime` 0..6480): GC `100b005c`/`100b061b` calls `GetCurrentRealTime` (`1000b44c`, float export of **double** `+0x50`), divides by `TimeSpeed`, and passes it to DS `VisualEnvFX_t::FrameProcess10062a66`. That function writes the shared float at `+0x10`; DS `1005cdcf` binds `"GameDayTime"` (string `1008e158`, xref `1005ce31`, visual pointer `+0x2e8`) and copies that float directly to the FXS value at `variable+0x28 -> +8` every process. `Tweak_GAME.txt:17,57–59` declares a float and forwards it unchanged to `CurrentDayTime`, then divides by6480 for `DayTimeFactor`. There is no integer-second or half-second quantizer here. Live67170 -> GameDayTime4478; new-character78435 ->5229.
* **Retail evaluation cadence [CODE]**: FXS `FrameProcess100010ef` increments frame counter `10018564`; formula ctor `10008951` sets interval1, and `10008bd3` expires the cached value by that frame counter. GC `100b879b` reads the current fractional clock directly for binary-sun ephemeris `100b9044`. DS `1005c334` reads the object's quaternion and height-dependent scale on each draw, not a half-second timer. Server `Update1000b526` overwrites synchronization directly; no invented interpolation/filter is applied to server corrections.
* **App**: `Zone::on_frame` stores in-day game seconds in a double accumulator, like native `+0x50`; `Zone::tick(dt)` advances by15*dt and wraps at97200. `day_time()` exports float game seconds /15, matching the native display boundary; integer `game_seconds()` remains a separate consumer. `ZoneEvent::Time` on every `GameTimeIIR_t` also resynchronizes the live clock.
  Before the first message the frozen default (`DEFAULT_DAY_TIME`) is used. The world is loaded with
  `load_playfield_on_day(.., zone.day_time(), zone.game_day)` (weather schedule seeded by the server game day instead of
  `OFFLINE_DAY`, see docs/formats.md *Weather*), and when it appears the viewer gets `Host::live_sky = LiveSky { start: zone clock,
  scale: 1.0, source: SkyClock::on_day(game_day), pose: SkyClock::pose_source() }`. The resource/colour worker retains its twice-per-second cadence, but **positions no longer wait for it**: the render thread evaluates the actual scripted quaternion, offset and authored height-scale every frame against the current fractional clock. Relative transforms account for each resource snapshot's baked time, including late worker responses; the initial uploaded scene already carries the same identity metadata, so startup need not wait for the first worker response. Wave-driven rotations use the current sun frame. Sun fans stay resident below the horizon and receive the native quartic, byte-truncated horizon alpha every frame. `GameTime` sets `Host::sky_clock`; audio uses the zone clock too.
  **Remaining cadence differences:** atmosphere/fog/colour tracks and baked directional vertex lighting still update with the resource worker; this correction does not claim per-frame colour evaluation. The weather game day of a running `SkyClock` remains the world-load day (cross-day resync does not reseed it). Authored frozen `GAME` interface overrides remain independent of the native advancing clock.
* **Stepping root cause / regression (2026-10-15, headless)**: the former `LiveRun::tick` held every celestial mesh until `since>=0.5` **and** the resource worker replied. That is an implementation throttle, absent from the retail chain above. `celestial_frame_positions_follow_dt_across_resource_snapshot_replacement` emits `celestial_csv` numerical before/after positions for120 frames of both suns and all three authored moon orbits at alternating1/60 and1/30s, asserts nonzero displacement proportional to dt, and crosses half-second resource replacements. “Before” is the old idealized half-second sample/hold (excluding additional worker latency), not a fabricated live capture. `live_sky_poses_advance_every_frame_without_waiting_for_resource_samples` deliberately blocks the worker while120 frame poses advance; `game_clock_accumulates_native_double_precision_before_float_export` covers ten-minute float-accumulator drift. These checks were added but **not executed by the implementation worker**. Parent commands: `cargo test --release -p ao-formats celestial_frame_positions_follow_dt -- --nocapture`; `cargo test --release -p ao-render live_sky_poses_advance`; `cargo test --release -p aomac game_clock_accumulates_native`.
* **Renderer integration evidence command**: `cargo test --release -p ao-render live_sky_renderer_frame_positions_survive_delayed_worker_responses -- --nocapture`. This offscreen-GPU resource test uploads an initial sky, runs the real `LiveRun::advance -> pose -> Renderer::apply_sky_pose` path for120 variable-dt frames, and releases three deliberately delayed resource responses. `renderer_celestial_csv` explicitly separates `modeled_old_hold_x` from `observed_callback_x` read from the renderer's applied instance matrix. It checks smooth deltas even when a late sample replaces the baked geometry. The fixture is a linear clock-driven triangle (the separate authored-orbit regression covers both suns/three moons), not a retail visual capture. Added, not executed by the worker.

### 10.2 Playfield changes
* **In-session zone change** `0x3C ZoneRedirection` (`ip[4]`, `u16 port`) [IF 0x10002a9e case 0x3C]: `ao_net::client` (zone phase)
  reconnects (3 tries, 1 s / 2 s backoff, like the first connect), sends `ZoneLogin` again with the cookies of the earlier
  `ZoneInfo` (protocol.md §5), keeps the wire tap and reports `LoginEvent::ZoneRedirect { zone_ip, zone_port }`; every following
  frame (`PlayfieldAnarchyF`, own character, `GameTime`, ...) is a `ZoneFrame` of the new connection. Test
  `zone_redirection_reconnects_with_the_same_cookies` (loopback servers). The original does this inside `Client_t` without the UI.
* **What starts a zone change (RE, N3.dll / Gamecode.dll / GUI.dll, Ghidra decompile + asm):**
  * **`n3TeleportIIR_t` 0x43197D22** (S→C; `IsAllowedFromClient` false; `ao_net::n3::teleport`). `ReadSubClass` [N3 0x10029e7f]:
    `f32 x3 pos` (`+0x18`), `f32 x4 rot` (`+0x24`, x y z w), `PlayfieldProxy_t` (`+0x34`: `u8 'a'`, `Identity playfield`, `i32 attribute`,
    `i32 exit_door`, `Identity exit_door_id`, `FUN_10038402`), `Identity` (`+0x4c`), `Identity` (`+0x54`), `i32 len` (> 1000 fails) +
    `len` bytes (kept in the message's own stream, unused). `Activate` [N3 0x10029f87] on the header's dynel:
    `exit_door_id.instance` (`+0x48`) **== 0** → `n3Dynel_t::UpdateLastAllowedPosition(-1, pos)` + `SetRelPosRot(pos, rot)` (a placement
    inside the playfield; for the own character also `FUN_10022b2d(engine+0x7c)` and the engine's vtable `+0x48`, the controller reset);
    **!= 0** → for the own character `n3EngineClient_t::StartTeleport` [N3 0x100076ad] (= `n3Playfield_t::StopPlayfield` [N3 0x1000ccbd]:
    every child dynel `Die`s, `playfield+0x64 = 1`; then `n3Engine_t::SetTeleportStatus(true)`), other dynels ignore it. App:
    `Zone::on_frame` → `ZoneEvent::Teleport` (own, destination) / `DynelState` + `Mover::on_teleport` (others) / the own in-place
    placement goes to `Movement::teleport` through the own-event queue.
  * `PlayfieldAnarchy_t::Run` [GC 0x101225b3] posts GUI event **5** every frame while `playfield+0x64` is set; `FUN_1000482e` [GC] maps
    event 5 → AFCM `0x145` = `FlowControlModule_t::TeleportStartedMessage` and event 6 → `0x144` = `TeleportEndedMessage`.
    Event 5 is also posted by `TeleportTrier_t::StartTryingTeleport` [GC 0x10037d48] (the **client-initiated** path:
    `n3EngineClientAnarchy_t::StartTeleportTry` [GC 0x10018f9a] runs when the own dynel is in a teleportal, `FUN_100585ee` /
    `n3Zone_t::IsPosInTeleportal` [GC], or on the own-character callbacks `FUN_100a4d7d` / `FUN_100a7510` / `FUN_100a7a7f` / `FUN_1007eec3`;
    it sends `MovementChanged(0x16)`, creates a `TeleportTrier_t` that fails after a timeout (`RunFunction`, `TeleportFailed` posts event 6
    and `Feedback_AreaChangeNotInitiated`)), and by `FUN_100a94ce` (own `SimpleChar_t`). Event 6 is posted by
    `n3EngineClientAnarchy_t::SetMainDynel` [GC 0x10019f77] (the own dynel appeared in the new playfield), `TeleportFailed` and `FUN_1005c0f3`.
  * `m_isTeleporting` [GUI 0x102635d8] is **initialised true** by `FlowControlModule_t::InitialiseMessage` [GUI 0x10027739], so the very
    first login is a "teleport" ended by the first `SetMainDynel`; both handlers are guarded by it (`Started` runs only when false,
    `Ended` only when true); `TargetingModule_t::FrameProcess` [GUI 0x10025fa4] does nothing while it is set.
* **`TeleportStartedMessage` [GUI 0x1002910e]** (registered at [GUI 0x1002ad2c] as message `0x145`): `m_isTeleporting = true`; emits
  `GlobalSignals+0x17c (0, LDB text (110, "ChangingArea") = "Changing area. Please wait.", 0xc)` = a **red** (`CCRed`) line in the System
  window (docs/chat/zone.md §1); `SpriteList_t::ParseList` + `DisplaySystem` vtable `+0x14` (render + present a frame) **twice**;
  `DisplaySystem+0x44 = 0` (the 3D viewport flag set by `Activate3DViewPort` [DisplaySystem 0x10078c02], tested by the frame function
  [DisplaySystem 0x10079a2e]: `RViewPort_t::Process` of the 3D root is skipped, the sprites/GUI are still drawn); `InputConfig+0x18 = 1`
  and the `InputConfig+0x1a8` timer stopped; `AFCM::Send(0x1e, 0x112)` = `TargetingModule_t::RemoveTargetMessage` (registered with 0x112
  in the `TargetingModule_t` ctor [GUI 0x10026322]); `Preferences_t::SaveAllPrefs`.
* **`TeleportEndedMessage` [GUI 0x100292ce]** (`0x144`): `m_isTeleporting = false`; `N3Msg_GetPosAndPF`, `BugReport_t::SetPFProxyInfo`;
  signal `+0x17c (0, text, 0xc)` with (110, `EnteringPF`) = `Entering '%s'` formatted with `N3Msg_GetPFName` (`FUN_1003676c(pf id)`, else
  the playfield's own name), or (110, `EnteringNewArea`) = `Entering new area.` for `N3Msg_IsDungeon` / no name; then arms the
  `CharInPlay` countdown (`DAT_102760cd = 1`, `DAT_102760d8 = 120.0`, `DAT_102760dc = 10`; float constant
  `[GUI 0x101ae4d8]`, freshly dumped; docs/zone/outgoing.md §3). `WaitingToStartGame` [GUI 0x10027d73] subtracts
  frame time and one frame, requiring `(frames < 0 || seconds < 0)` and no database workload. The port counts actual
  queued/unanswered dynel asset-worker requests (including placed dynels, models, weapons and clips), not a timer-only guard.
  Installing the world behind the loading overlay does not begin its close fade or unlock game input. The server echoes our
  `CharInPlayIIR_t` for the own character: its `Activate` [GC 0x1007264d] posts event `0xa5` = `FlowControlModule_t::AliveMessage`
  [GUI 0x10028543]: sends AFCM `(0x1b, 0x135)` if the loading module is active (retaining its existing 7-second close fade),
  then `SetStaticInputMode(8)`, `Activate3DViewPort`, `EnableUserInput`, and clears the countdown flag.
* **Loading screen: none for a teleport.** `ServerLogin3DModule_t` (program 5, module `0x1b`) is added only by
  `CharCreateDoneMessage` [GUI 0x100278f9] (the `AddProgram` scan of GUI.dll finds no other `PUSH 5`) and is closed by message `0x135`
  from `AliveMessage`; `SetLoadingScreen(n)` only picks `welcome_to_rubika.jpg` after character creation (docs/screens.md §7);
  `cd_image/gui/Default/gfx` has no per-playfield image. Messages `0x136` / `0x13b` (`ActionView_t` frame processor [GUI 0x100249a0]:
  `Activate3DViewPort` / `DisplaySystem+0x44 = 0`) have no sender in GUI.dll or Gamecode.dll. The window layer is never torn down: the
  HUD, chat windows and bars are GUI windows that read `N3Msg_*` state, nothing in these handlers closes or recreates them. No fade,
  no `PlayStartupMusic` (only `CharacterLoggedInMessage`, the login); the playfield's ambience / district music follow
  `PlayfieldInit` [GC 0x10016e2c] → `SandyInterfaceModule_t::ActivateGameZone` (docs/formats.md §audio).
* **App** (`Play::begin_zone_change` = `TeleportStarted`, `Play::entering_text` + the world-ready block of `frame` = `TeleportEnded`):
  `ZoneEvent::Teleport`, `ZoneEvent::Playfield` or `LoginEvent::ZoneRedirect` while `Screen::InWorld` (idempotent through
  `Play::teleporting`, also when the server sends no `n3TeleportIIR_t`): red `ChangingArea` line, target cleared, `Player` dropped (input
  locked), old dynels/actors dropped (`Zone::reset_world`), `CharInPlay` owed again, and a black full-screen quad under the GUI
  (the 3D viewport is off; black is [INFERENCE]). The HUD and chat stay. The new playfield loads in the background (a result for a
  replaced playfield is dropped); when it is ready the scene/sky/camera are replaced, `Hud::provide_ground` refreshes the map, the player is
  rebuilt from the new own `SimpleCharFullUpdate`, a red `EnteringPF` / `EnteringNewArea` line is printed, `teleporting` clears and the
  `CharInPlay` countdown (> 10 frames) starts. The viewport / input stay off until the echo (`AliveMessage`, below).
  Tests: `flow::tests::{second_playfield_keeps_the_interface_and_hides_the_world, zone_redirect_starts_the_teleport,
  teleport_iir_starts_the_zone_change_only_with_a_destination}`, `ao_net::n3::teleport::tests`.
* **Client-initiated path (RE, N3.dll / Gamecode.dll / GUI.dll)** — implemented (`Player::teleport_try`, `Zone::{start_teleport_try, run_trier}`,
  `Collision::in_teleportal`, `Play::{teleport_started, teleport_ended}`):
  * **Teleportal data**: `n3SurfaceResource_t::ReadVersion4/5` [N3 0x10015e17 / 0x10015fc1] read an optional `PortalArea_t` (+0x34: polygon of
    `Vector3_t`, `FUN_1001b3e6` / `FUN_1001b473`) and the destination bits (+0x40: low 16 bits = playfield, +0x38; localizer type
    `0x186a1` at +0x3c). `GetTeleportalSurface` (+0x30, a KD tree) is never written by either reader. `n3Zone_t::IsPosInTeleportal`
    [N3 0x1001a86a] asks the zone's polygon (`FUN_1001b21a` [N3 0x1001b21a]: crossing parity of the +x ray in the x/z plane, `y` ignored; an
    edge counts when `(z0-z)*(z1-z) < 0`, `|dz| >= 1e-6` [f32 @0x1003da14] and its x at `z` exceeds `p.x`). The client never uses the
    destination bits for the try: the server decides the target. Survey (`cargo run --release -p ao-formats --example portal_survey`): 8948
    of 234349 surface records carry a polygon (4 to 6 corners; the same polygon is stored in every zone it overlaps), in playfields
    505..800 (49 of them), 4001/4005/4006, 4310-4312/4320/4321, 4504/4505/4540-4544, 4605, 4872/4873, 6101/6102 and 6550/6551.
    **None in ICC Shuttleport 4582 / Arrival Hall 4604** (`pfnrmap.dat`; Newland City is 566, the earlier label was wrong). These polygons are distinct from placed travel objects: Borealis's template-96272 Whom-Pahs were exercised by walking through them (movement.md §4.2), whereas its template-95350 Grid terminal is used as an object. A missing polygon does not prove that a playfield has no travel objects or server-controlled boundary transitions.
    `Collision::in_teleportal(scene_pos)` picks the zone (`n3Dynel_t::GetZone`: zone grid outdoors via `zone::grid_zone`, `PosToRoom` or 0 in
    a dungeon) and runs the same parity test. Tests: `collision::portal::tests`, `tests/collision_real.rs::collision_teleportal_of_real_polygons`.
  * **Per frame** (`FUN_1005b016` [GC], the own dynel's `Run`, calls `FUN_100585ee` [GC 0x100585ee]): `IsPosInTeleportal(vehicle global pos)`
    then the gate `FUN_1005859d` [GC 0x1005859d]: not an NPC (`+0x21c`), dynel flag bit 17 (`0x20000`) clear, not dying (`+0x80`), stat
    `InPlay` (0xC2) != 0 and dead flag `0x20` clear (set by `CharDie_t`). [UNRESOLVED] bit 17 has no writer in Gamecode.dll (a `PUSH 0x20000`
    scan finds two readers masking `Features` bit 2 in `FUN_10044842` / `FUN_10044a07` and one clear in `FUN_1005bea6`): taken as clear.
    Stat `InPlay` comes from the own `SimpleCharFullUpdate` (`FUN_10077af2`: **main flags** at message `+4`, `>> 1 & 1`, not VisualFlags) and the `CharInPlay` echo.
  * **`StartTeleportTry`** [GC 0x10018f9a] (when the playfield exists and no `TeleportTrier_t` lives): `N3Msg_MovementChanged(0x16, 0, 0, true)`
    (our `Movement::action(SYNC)`, a position sync without FSM transition), `new TeleportTrier_t(30.0)` (f32 @GC 0x10157500) added to the
    playfield, `StartTryingTeleport` [GC 0x10037d48]: elapsed := 0, GUI event 5 (= `TeleportStartedMessage`) and, with a control dynel,
    `FUN_10059ae5(1)` (full stop while moving). The playfield **keeps running** (no `StopPlayfield`): the app's `teleport_started` prints
    `ChangingArea`, clears the target, stops the input and hides the viewport, the old world stays loaded.
  * **`TeleportTrier_t::RunFunction`** [GC 0x10037e29] adds the frame time (`n3EngineClient_t+0x68`) and past 30 s runs `TeleportFailed`
    [GC 0x10037db7]: `n3Fobj_t::Die` (`+0x14 := 1`; `n3Fobj_t::Run` [N3 0x10007e07] then removes the child), GUI event 6 (=
    `TeleportEndedMessage`: `m_isTeleporting := false`, `Entering ...` line, `CharInPlay` countdown re-armed) and, for the control dynel,
    `FUN_10058b00("Feedback_AreaChangeNotInitiated")` (category 110, System window line, no colour). A still-standing character starts a new try
    in the next frame (the original loops the same way every 30 s). `StartTeleportConfirmed` / `StartTeleportFailed` [GC 0x10016aa4 /
    0x10016a8e] (`TeleportProceeding` / `TeleportFailed` from outside) have **no caller** in any client DLL (exports only), so a try ends by the
    timeout or by the playfield being stopped (`Zone::reset_world` drops the trier like `StopPlayfield` kills its children). The live run
    (docs/play.md) shows the server changes the playfield from the movement stream; with a try pending that is the `n3TeleportIIR_t` path above.
  * Tests: `zone::tests::teleport_trier_runs_once_and_times_out`; the player/flow glue needs the client's data (`flow::tests`).
* **`AliveMessage` / input unlock** (`Play::alive`, `Zone::on_frame` -> `ZoneEvent::Alive`): the server relays our own `CharInPlayIIR_t` back
  (live capture `docs/captures/zone_enter_ithaca.rec`: sent at 39975, relayed at 40079, sender = receiver = own id). `CharInPlayIIR_t::Activate`
  [GC 0x1007264d]: `EnableVisibility`, `SetStat(0xC2, 1)`, and for the control dynel GUI event `0xa5` = `AliveMessage` [GUI 0x10028543]:
  `InputConfig_t::m_isAlive := true`, closes module `0x1b` if active, `SetStaticInputMode(8)`, `Activate3DViewPort`,
  `InputConfig_t::EnableUserInput` [GUI 0x10019ce8] (`+0x18 := 0`, key states reset, idle timer started), countdown flag `DAT_102760cd := 0`.
  The lock side: `TeleportStartedMessage` sets `InputConfig+0x18 := 1` (`isUserInputStopped`) and `DisplaySystem+0x44 := 0`;
  `WaitingToStartGame` [GUI 0x10027d73] clears static input mode `0x3a` (set by `CharCreateModule_t::InitialiseMessage` [GUI 0x1011ba81]
  with `EnableUserInput`) after `N3Msg_SendInPlayMessage` succeeded. **No timeout** on the echo exists in the original (the 120 s of
  `WaitingToStartGame` only bound the *send*), so after a zone change the viewport stays off and the input stopped until the echo arrives; the
  port replicates that. App: `Play::awaiting_alive` (= input/viewport gate; set at the hand-off and by `teleport_started`,
  cleared only by the own character's `alive` after world installation). `Play::loading_active` retains module `0x1b` until
  the Alive-triggered close fade completes; rezones do not reactivate it. `Play::game_input_open` gates game keys / mouse
  (releases always pass, replacing native key-state reset). Static mode `0x3a` is not a separate port field:
  [UNRESOLVED] its `InputConfig_t::CheckMode` consumers have not been traced; clearing that mode after sending is not
  treated as permission to bypass the Alive input/viewport gate. The black quad follows `awaiting_alive` (colour still [INFERENCE]).
  Regressions: `loading_waits_for_alive_and_countdown_uses_native_boundaries`,
  `teleport_try_stops_the_input_until_the_char_in_play_echo`, and `zone::tests::own_char_in_play_relay_is_the_alive_event`.
* **Zone-change audio**: `PlayfieldInit` [GC 0x10016e2c] -> `SandyInterfaceModule_t::ActivateGameZone`: when the new world appears the loader
  thread's `ao_audio::PlayfieldAudio::load(store, id, Report::sounds)` is handed to `Audio::set_playfield` (`Bg::Info`, `Play::frame`'s
  world-ready block; `None` leaves the playfield), which stops the old ambience and follows the new districts / emitters through the existing
  `Audio::update(dt, camera, zone clock)`. Live `Play::frame` now feeds `Audio::update_weather` after the server-day weather update,
  sharing the offline viewer's music inputs and six rain/wind/sand/fallout/quake keepalives. Native GC `100be767` submits rain and quake
  levels each frame, `100bfa11` submits the four wind levels, and `100b6d67` consumes weather music inputs on the existing 1 Hz district pass.
  Regression: `weather_frame_routes_music_and_each_keepalive` (installed retail sound bank, headless).
* **`EnteringNewArea`**: `Report::dungeon` (`Layout::Dungeon`: the playfield record's tilemap id differs from its own id) travels through `Bg::Info`
  to `Play::dungeon`; a dungeon always says `EnteringNewArea` like `N3Msg_IsDungeon`, also when `pfnrmap.dat` names it.
* **Not implemented**: `SaveAllPrefs` and the two `ParseList`/present passes of `TeleportStartedMessage` (no effect on state/visuals we draw).
