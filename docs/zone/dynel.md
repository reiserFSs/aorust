# Zone N3 messages: character dynels (`crates/ao-net/src/n3/dynel.rs`)

Messages decoded here, all received from the real PRK zone server (Ithaca, playfield 4582) in `docs/captures/zone_ithaca.rec`
and all fully consumed by the decoders (the tests assert zero undecoded bytes for every captured instance):

| id | class (RTTI) | count | meaning |
|---|---|---|---|
| `271B3A6B` | `SimpleCharFullUpdateIIR_t` | 81 | a character / NPC dynel appears (name, breed/sex, position, rotation, stats, head, cloth, textures, attached meshes) |
| `54111123` | `CharDCMoveIIR_t` | 144 | movement / placement of a dynel (move type + quaternion + position); the client also sends it |
| `2B333D6E` | `StatIIR_t` | 69 | stat changes of a dynel |
| `60201D0E` | `SetWantedDirectionIIR_t` | 72 | wanted movement direction (unit vector) |
| `3B1D2268` | `WeaponItemFullUpdateIIR_t` | 10 | a weapon item dynel (identity kind `0xC74A`) held by a character |
| `1D3C0F1C` | `SpecialAttackWeaponIIR_t` | 58 | special attacks of the wielded weapon + initiative stats |
| `25314D6D` | `CastNanoSpellIIR_t` | 3 | a dynel casts a nano spell; the client also sends it |

## 0. How the ids and the wire format were established

* The 32-bit id **is a hash of the C++ class name**: `n3InfoItemRemote_t::MapToKey(const std::string&)` [N3 0x10009826]:
  `key = 0; for i, c in name: key ^= (int8)c << ((i & 3) * 8)`. `CharDCMoveIIR_t` → `54111123`, `SimpleCharFullUpdateIIR_t` → `271B3A6B`,
  `PlayfieldAnarchyFIIR_t` → `5F4B1A39` (unit test `message_keys_are_class_name_hashes`). Each IIR class registers itself with
  `n3InfoItemRemote_t::Register(name, factory)` [N3 0x100099e4] (e.g. `CharDCMoveIIR_t` [GC 0x1000d9f2], `SimpleCharFullUpdateIIR_t`
  [GC 0x10010180]); `n3InfoItemRemote_t::Construct(key, BinaryStream&)` [N3 0x10009b08] reads `i32 key`, `Identity`, `u8 pass_on` (must be 0 or 1,
  stored at `IIR+0xc`), then calls vtable slot 7 (`ReadSubClass`, the body). `Write` [N3 0x100098f4] is the mirror (slot 8 = `WriteSubClass`).
  Messages the client never sends have an empty slot 8 (`SimpleCharFullUpdateIIR_t` slot 8 = `ret`).
* Class names for the other ids seen in the capture (found by hashing the RTTI strings of Gamecode.dll; all unique): `260F3671 FollowTargetIIR_c`,
  `52526858 GenericCmd_t`, `39343C68 BuffIIR_c`, `41624F0D AppearanceUpdateIIR_c`, `46002F16 AttackInfoIIR_t`, `28494070 AttackIIR_t`,
  `4A41203E StopFightIIR_t`, `36510078 n3ToClientQuitIIR_t`, `5E477770 CharacterActionIIR_t`, `5C654B28 MissedAttackInfoIIR_t`,
  `4F474E05 CorpseFullUpdateIIR_t`, `51492120 CharSecSpecAttackIIR_t`, `754F1115 SpecialAttackInfoIIR_t`, `570C2039 CharInPlayIIR_t`,
  `7F544905 VendingMachineFullUpdateIIR_t`, `5F52412E GameTimeIIR_t`, `29304349 FullCharacterIIR_t`, `2E2A4A6B OrgInfoPacketIIR_t`,
  `465A4061 QuestFullUpdateIIR_t`, `5F4B1A39 PlayfieldAnarchyFIIR_t`.
* Layouts below are read from the decompiled `ReadSubClass` bodies (Ghidra, `Gamecode.dll`, PE base 0x10000000), operand widths resolved from the
  `BinaryStream::operator>>` import used at each call (`0x101530c4` = `char`/u8, `0x101530d8` = `short`, `0x101530bc` = `int`, `0x101530cc` = `float`,
  `0x101530dc` = `read(bytes)`), and checked by decoding every captured instance to the last byte.
* **Stat names** come from the client's own table: `n3EngineClientAnarchy_t::N3Msg_GetStatNameMap` [GC 0x10027483] (a `map<int, const char*>`, filled
  by `FUN_1002f009`, `FUN_1003227a`, `FUN_100324d2`, `FUN_10035c99` [GC], read by `fStatToString` [GC 0x100367b2]); the table has 524 entries and
  none for e.g. stats 1 and 0x300 (the client prints `Missing stat: N`).
* Container convention shared by all lists below (`FUN_1002b8b6`, `FUN_10071b49`, `FUN_10079361`, `FUN_1002b96d`, `FUN_1002e618`, `GameData operator>>(vector<…>)`
  [GameData 0x1000a709 / 0x1000c1cc]): `i32 c`, valid only if `c % 0x3f1 == 0 && c / 0x3f1 - 1 < 0x7531`; the element count is `c / 0x3f1 - 1`
  (wire value `(n + 1) × 1009`; `0x3f1 = 1009`).
* `Identity` = `i32 kind, i32 instance`; read by `FUN_1013cda9`. Character/NPC dynels have kind `50000` (0xC350); `SimpleCharFullUpdate` validation
  (`FUN_10077a21` [GC]) returns "reject" for any other kind.
* Floats are IEEE-754 single, big endian, written exactly as on the wire (no axis flipping). **Y is up** (heights 20 to 42 m on the island, X/Z are the
  ground plane). Rotations are **quaternions `(x, y, z, w)`** (identity = `(0, 0, 0, 1)`: the weapon reader substitutes exactly that for an all-zero
  rotation [GC 0x100a110a]); live values are rotations about Y, `(0, sin(θ/2), 0, cos(θ/2))`, so heading `θ = 2·atan2(y, w)` (`dynel::yaw`).
  [GUESS] the sign convention of θ (clockwise/counter-clockwise seen from above) was not traced into the renderer.

## 1. `SimpleCharFullUpdateIIR_t` (`271B3A6B`)

Class `SimpleCharFullUpdateIIR_t` is a `n3InfoItemRemote_t` that also implements `n3DynelRibosome_i` (a factory/initialiser for the dynel it describes);
two vtables: ribosome `0x10161464`, IIR `0x10161480`.

| slot | address | role |
|---|---|---|
| IIR 7 `ReadSubClass` | [GC 0x10078c24] | body decoder (the layout below) |
| IIR 8 `WriteSubClass` | [GC 0x10077ac7] | empty (server → client only) |
| IIR 1 validate | [GC 0x10077a21] | `identity.kind == 50000`, playfield `+0x20` exists and is not a "dead" one, dynel not already alive (`n3Dynel_t+0x80`), parent (`+0x24`) exists if non-null |
| IIR 2 apply | [GC 0x10077e13] | create + initialise the dynel |
| ribosome 3 | [GC 0x10077a84] | `operator new(0x2d4)` + `FUN_1005cb6a` = **`SimpleChar_t` constructor** (`SimpleChar_t::vftable`, derives `n3VisualDynel_t` [N3 0x10019365] → `n3Dynel_t` [N3 0x1000566e]) |
| ribosome 5 | [GC 0x10077af2] | initial stats/appearance of the new `SimpleChar_t` (see "Fields → stats") |

### 1.1 Wire layout (body after the 13-byte N3 header; offsets are payload offsets in the player's own message, see 1.4)

Conditions use the first flags word `F` (`i32` after the version byte) and the second word `F2`; both are listed in §1.2.

| step | type | field | meaning |
|---|---|---|---|
| 1 | `u8` | `version` | `0x39` or `0x3a` (anything else rejected: `FUN_10079321` throws). Live: `0x3a`. |
| 2 | `u32` | `F` | flag word, bits in §1.2. Players live `0x4ac2`, NPCs `0x20a2ec3 … 0x20a6ec3`. |
| 3 | `i32` | `playfield` | iff `F & 0x40`; `GetPlayfield(+0x20)`; live 4582 (0x11E6). |
| 4 | `Identity` | `parent` | iff `F & 0x20`; if non-null the dynel is attached to it instead of the playfield. Never set live. |
| 5 | 3 × `f32` | `pos` | `x y z` (`FUN_1000404e`), Y up. |
| 6 | 4 × `f32` | `rot` | iff `F & 0x200`: quaternion `x y z w` (`FUN_1000a78b`). |
| 7 | `i32` | packed appearance | bits 0-2 `Side` (stat 0x21), 3-4 `Fatness` (0x2F), 5-7 `Breed` (4), 8-9 `Sex` (0x3B), 10-11 `Race` (0x59), 12-16 stat 0x1A7. |
| 8 | `u8 n` + `n` bytes | `name` | NUL-terminated string, `n` includes the NUL (the client forces `buf[n-1] = 0`); stored in the dynel via `FUN_1005b64b`. |
| 9 | `u32` | `F2` | stat `Flags` (0) and `SimpleChar_t+0x134`. Players `0x81241`, NPCs `0x200 … 0x220`, bit 22 (0x400000) = two name strings follow (PC only). |
| 10 | `u32` | packed | low 16 bits stat `Expansion` (0x185) (27 for players), high 16 bits stat `AccountFlags` (0x294) (0). |
| 11a | PC (`F & 1 == 0`): `i32` | `current_nano` | stat `CurrentNano` (0xD6). |
| | `i32` | `field_108` | passed to `FUN_100586da` with tag `0xDEA9`; 0 live. **Unresolved.** |
| | 7 × `i16` | `stats` | stats `0x8a (Swim), 0x10 Strength, 0x11 Agility, 0x12 Stamina, 0x13 Intelligence, 0x14 Sense, 0x15 Psychic` (`FUN_10058ca8` = `SetStat` with change-notification muted). |
| | 2 × `str_i16` | name parts | iff `F2 & 0x400000`: `i16 len` + bytes each (`FUN_100388e2`), dynel string slots 1, 2. Never seen. |
| | `i32`, `str_i16` | extra | iff `F & 0x4000000` (bit 26): `i32` only if `version > 0x39` (stat `Clan` 0x5), then a string (slot ?). Never seen. |
| 11b | NPC (`F & 1`): `u8`/`i16` | `npc_family` | stat `NPCFamily` (0x1C7); `u8` if `F & 0x20000`, else `i16`. |
| | `u8`/`i16` | `stat_1d2` | stat 0x1D2 (unnamed); `u8` if `F & 0x80000`. |
| | `u8`/`i16` | `pet_type` | stat `PetType` (0x200); `u8` if `F & 0x2000000`. |
| | `i16` | `tower_type` | stat `TowerType` (0x184); if `> 0` a `u8` follows, selecting a spawn effect (1→0xEEA4, 2→0xEEA5, 3/5/6→0xEEA3, 4→0xEEA2) [GC 0x1007833c…]. |
| 12 | `u8` (`F & 0x1000`: `i16`) | `level` | stat `Level` (0x36). |
| 13 | `i32` (`F & 0x800`: `u16`) | `max_health` | stat 1 (no client name). |
| 14 | `u8` if `F & 0x4000` / `u16` if `F & 0x800` / else `i32` | `health` | stat `Health` (0x1B); the `0x4000` form is the **delta** `max_health - value` (players: always 0). |
| 15 | `i32` | `monster_data` | stat `MonsterData` (0x167); NPC record id (22794 for the Surf Lizard, 254118 ICC Shuttle Guard); 0 for players. |
| 16 | `i16` | `monster_scale` | stat `MonsterScale` (0x168), percent (`SetBodyScale(stat / 100.0)`, constant [GC 0x10158670] = 100.0): players 100, NPCs 36-181. |
| 17 | `i16` | `visual_flags` | stat `VisualFlags` (0x2A1): 31 (127 for one player). |
| 18 | `u8` | `mode` | `SimpleChar_t+0x208` via `FUN_100572b9`; 0 (79×) / 1 (2×). **Unresolved.** |
| 19 | `i32 len` + bytes | `blob` | raw copy (`FUN_1006761c`) handed to the vehicle at `SimpleChar_t+0x50` (`vtable+0xac`). 42 bytes for players, 28 for NPCs: velocity, FSM axes/current mode, remembered walk/run mode; players also carry four movement inputs ([movement.md](movement.md), FullUpdate vehicle initialization). |
| 20 | `i32` | `head_mesh` | iff `F & 0x80`: stat `HeadMesh` (0x40) = **rdb 1010001 mesh id of the head** (Testy 40629 = `head_solitusfemale00.abiff`). |
| 21 | `i16` (`F & 0x2000`) / `u8` | `run_speed` | stat `RunSpeed` (0x9C) (`SetStat`, signed). Players 6, 34, 70; NPCs 6-144. |
| 22 | `Identity` | `target` | iff `F & 0x400`: passed to `FUN_10069c68` (a `SimpleChar_t` combat routine). **[GUESS]** the character's current fight target: the guards that have it set point at the `Scout - Jaax'Sinuh` NPCs they fight, and they have lowered health. |
| 23 | list | `textures` | iff `F & 0x10`: count (§0) × `TextureData_t` [GameData 0x1000c0b4]: `char material[32]` (NUL padded), `i32 texture` (+0x20, rdb 1010004 id), `i32` (+0x24), `i32` (stored as 5 if non-zero). Surf Lizard: material `lizard_green`, texture 22768 (`lizard_brown.png`). Applied with `SetTextureDataList` (`n3VisualDynel_t`). |
| 24 | `u8` | `byte_120` / `byte_121` | iff `F & 0x800000` / `F & 0x1000000`: apply passes the signed byte with stat id 0x168 (`MonsterScale`) to `FUN_10064963` / `FUN_10064916` on the object at `SimpleChar_t+0x1bc` [GC 0x100784d0, 0x100784f7] (a stat-modifier container; the second only for non-own dynels). Never seen. |
| 25 | list | `effects` | always: count × (`Identity`, 3 × `i32`) [GC 0x10051b40] (the client computes a time stamp from two of the ints and `GameTime_t`). Empty in every capture. |
| 26 | `Identity`, `i32 k`, `k` × 3 `f32` | `path` | iff `F & 0x10000`: `k <= 30` waypoints (the client has no bound on `k`; the decoder rejects `k > 30`). If the identity is non-null the apply routine calls `GetDynel(identity)` + `FUN_100574e2/FUN_1006fe02` (follow/goto). Never seen. |
| 27 | list | `cloth` | always: count × `ClothData_t` [GameData 0x1000a661], see §1.3. |
| 28 | list | `attractors` | always: count × `AttractorMeshData_t` [GC 0x10001da1]: `u8 place`, `i32 mesh`, `i32`, `u8`, §1.3. |
| 29 | list | `list_190` | iff `F & 0x100`: count × 4 `i32` (read order `[0] [1] [3] [2]`, `FUN_10065687`) at `+0x190`; apply calls `FUN_1006aef6`/`FUN_1006ac03` if non-empty. Never seen. |
| 30 | `u8` | `shadow_breed` | iff `F & 0x20000000`: stat `ShadowBreed` (0x214). Never seen. |
| 31 | list | identity list | iff `F & 0x40000000`: count × `Identity` (stored at `+0x308`; the apply loop calls `FUN_1004aff7` for each). Never seen. |
| 32 | `u32` | `F3` | bit 0: `i32 k`, `k` × (`i32 index, i32 value`) (`FUN_10009d45`), `i32` (+0x31c), `Identity` (+0x324); bit 1: `u8` stat `BattlestationSide` (0x29C); bit 2: `i32` stat `PetMaster` (0xC4). 0 in every capture. |
| 33 | `u8` | `flag_32f` | `!= 0` copied to `n3Dynel_t+0x21d`; 0 live. |

Anything after step 33 is returned in `rest` (empty in every capture).

### 1.2 Flag bits `F` (`flag::*` in the code)

`0` NPC/monster layout (live set on all 78 NPCs, clear on the 3 players) · `1` stat `InPlay` (0xC2) · `2` apply sets dynel flag `0x800`
(`FUN_10044cb5`) and **skips** the attractor block (clear: `AddAttractorMesh(0, HeadMesh, 4, 0)`, `ClearAttractors`, `AddAttractors(list)`, [GC 0x100780c0]) · `3` stat `CanChangeClothes` (0xDF) · `4` textures ·
`5` parent · `6` playfield · `7` head mesh · `8` list_190 · `9` rotation · `10` target · `11` health width · `12` level width · `13` run speed width ·
`14` health delta · `16` path · `17/19/25` NPC field widths · `18` dynel flag `0x800000` (`FUN_10044cb5`) · `21`/`22` call `vtable+0x18` of the stat object (`dynel+0xe8`) with `0x800` / `0x400` [GC 0x1007846e] · `23/24` bytes · `26` PC extra · `27` **NPC dynel only** (`SimpleChar+0x21c != 0`): stat `0x300` = 1 (else 0) [GC instructions 0x1007850f–0x10078539; the earlier “own dynel” reading was incorrect] · `28` stat `HasAlwaysLootable` (0x159) = 1 · `29` shadow breed · `30` identity list. Live values:
players `0x4ac2` = bits 1, 6, 7, 9, 11, 14; NPCs `0x20a4a43/0x20a4a53` (plain monsters), `0x20a6a43/0x20a6e43/0x20a6ac3/0x20a6ec3/0x20a2ec3`
(humanoid guards/scouts: bits 7 and 13/14 add head mesh and wide run speed), bit 25 (`0x2000000`) is set on all NPCs (`PetType` as byte).

### 1.3 `ClothData_t`, `AttractorMeshData_t` and the character look

`ClothData_t` (20 bytes in memory): wire `i32 a`, `i32 +4`, `i32 +0x10`, and, only if `a > 0 && (short)(a >> 16) > 0`, `i32 +8`, `i32 +0xc` (otherwise 0).
`(short)a` is the `ClothData_t::ClothPart_e` (GetName table [GameData 0x1000a5f0]: `hands body feet arms legs` = 0..4, the same order as
`ao_formats::character::ClothPart`), **`+4` is the rdb 1010004 texture id drawn over the skin** (`FUN_1004b5ab` [GC] calls
`VisualCATMesh_t::SetCATTexture(ClothData_t::GetName(part), texture, layer 2, …)` for each entry, `texture == 0` clears layers 2 and 3), and `+0x10`
is a page index: `FUN_100480fc` [GC] stores entry `e` at `dynel->cloth[(e.page * 5 + e.part)]`. Live captures:

| character | cloth (part, texture) | texture names (name table) |
|---|---|---|
| Bergdoktor 33402 | (2,9616) (4,22626) (3,27422) (0,9402) (1,248372) | `feet_combatboots.png` `legs_flakarmour.png` `arms_flakarmour.png` `hands_lowtecharmour.png` `body_salamander_skin.png` |
| ICC Shuttle Guard | (3,286226) (1,286227) (2,286228) (0,286229) (4,286225) | `icc_arms.png` `icc_body.png` `icc_feet.png` `icc_hands.png` `icc_legs.png` |
| Testy 25988 | none (naked: skin + model default only) | |

`AttractorMeshData_t` (`CharacterMesh::AddAttractors`, DisplaySystem import): `u8 place`, `i32 mesh` (rdb 1010001), `i32`, `u8`. The head is delivered both ways:
`HeadMesh` stat (flag bit 7) and as attractor entry `(place 0, mesh = head, 0, 4)`; weapons are `(1, 7796 weapon_shotgunsmall01, 0, 2)` ×2 on
`Stanko` (places 1 and 2) and `(1, 262556 EP3_assault_rifle_03, 0, 2)` on the guards; Bergdoktor has `(5, 26163 summon_light, 0, 0)`. Place indices match
the DisplaySystem strings `Attractor01_head`, `Attractor02_righthand`, `Attractor03_lefthand`, … (place = number − 1) in all captured cases; the last
`u8` (4 head, 2 weapon, 0 light) is unresolved.

Full-update head omission is real: `zone_enter_ithaca.rec` contains **Xantarr**, breed 1 / sex 2,
flags `0x4ac2`, `HeadMesh = 223820`, and an empty attractor list. Do not treat this as a headless
character. The two retail clear methods differ: `CharacterMesh::ClearAttractors` [DS `0x10071dd0`]
deletes bookkeeping nodes, whereas `VisualCATMesh_t::ClearAttractors` [DS `0x10073d8a`] additionally
calls `FUN_10072873` on mounted nodes; that function calls `RCATMesh_t::RemoveAttractorChild`.
The full-update path [GC `0x10077e13`, block `0x100780c0`] uses the former after adding `HeadMesh`;
appearance updates use the latter. `CharLook::from_update` therefore retains the separately
mounted head when place 0 is absent; `apply_appearance` still replaces the mounted set, including
clearing a head for an empty list. Flag bit 2 continues to skip the full-update block.
Focused read-only survey with the existing release format decoder decoded **360/360** unique
creation-head meshes (all breeds/sexes, expansion masks 0 and 2) to nonempty geometry; captured
Bergdoktor head 40103, Stanko head 223940 and Testy head 40629 each decoded to 101 vertices.
Regression: `full_update_head_survives_missing_attractor_but_appearance_clears_it`.
Rendered replay additionally exposed a distinct Atrox mounting defect: a successfully decoded head
was buried inside the torso. `ActorRig` had used the nearest weighted ancestor directly for an
unweighted attractor bone, dropping that bone's local rotation and translation. Construction now
reuses `character::build`'s `best_rest_clip` / `derived_bind_frame` resolution once, outside the pose
hot path. `atrox_unweighted_head_mount_matches_character_loader` compares the complete head
transform for captured heads 40103 and 223940 with the existing character loader.
`captured_remote_appearance_screenshots` renders front/back PNGs of the unmodified captured
Xantarr, Stanko (dual shotguns), and Bergdoktor (cloth/back attachment) with `AOMAC_SHOT_DIR`.

**What feeds the existing app model** (`crates/ao-formats/src/character/player.rs`, `Player::new(breed, gender, skin, head)`):

| `Player` input | message field |
|---|---|
| `Breed` | `breed` 1 Solitus 2 Opifex 3 Nanomage 4 Atrox (also stat 4; matches `create.rs` `CC_BREEDS`) |
| `Gender` | `sex` 2 male, 3 female (1 = neuter; `create.rs` says "sex 2 male 3 female"); Testy: breed 1, sex 3, head `head_solitusfemale00` ✓ |
| `Skin` | `race` (stat `Race` 0x59): 1 in every capture; **[GUESS]** 1 = caucasian; no asian/african player seen. `head_mesh` already encodes the ethnicity (`head_solitusfemale_asian05` = 40627 on an NPC), so no lookup through `player_heads` is needed |
| `head` | `head_mesh` = rdb 1010001 id directly |
| `Equipment` | `cloth[i].texture` per `part` (page 0) → `Equipment::wear(ClothPart, texture)` |
| weapons | `attractors` (`place`, `mesh`) |

NPCs use `monster_data` (stat `MonsterData`, 22794…) to pick their model (**unresolved**: the record behind `MonsterData` and how
`FUN_10058b6a`/`SimpleChar_t` map it to an rdb 1010002 model was not traced) and `textures` to retexture materials; `monster_scale` percent scales the model.

### 1.4 Players vs NPCs in this message

* Identity kind is `50000` for both. Player instance ids are small (25988 = our own `Testy`, 33402 `Bergdoktor`, 33491 `Stanko`), NPC ids start at 1,000,000
  (1002039 … 1026299). The frame `sender` is 1 (system) for every `SimpleCharFullUpdate`.
* Flag bit 0 selects the layout (§1.1 step 11a/11b): players carry `current_nano` + 7 ability/skill shorts, NPCs carry `npc_family`/`pet_type`/`tower_type`.
* Players: `F = 0x4ac2` (head mesh, rotation, health delta, 16-bit health), 42-byte blob, `monster_data = 0`, `monster_scale = 100`, `F2 = 0x81241`,
  `expansion = 27`. NPCs: `F2 = 0x200..0x220` or `0x201/0x202`, 28-byte blob, `monster_data != 0`, scale ≠ 100, `side` 3 on the animals (0 on the guards), `breed` field 6 on the animals.
* Both: `playfield 4582`, `version 0x3a`.

Annotated bytes, our own dynel (`Testy`, frame receiver 25988, 257 payload bytes; offsets = payload offsets):

```
271b3a6b 0000c350 00006584 00      msg_type, Identity(50000, 25988), flag 0
13:3a            version
14:00004ac2      F = 0x4ac2
18:000011e6      playfield 4582
22:4467c1c5 41bbaedb 4439adf5   pos (927.0277, 23.4604, 742.7181)
34:00000000 beb193bd 80000000 3f701c0b   rot (0, -0.3468, -0.0, 0.9379) -> yaw -0.7087 rad
50:00000728      packed: side 0 fatness 1 breed 1 sex 3 race 1
54:06 5465737479 00   name "Testy"
61:00081241      F2
65:0000001b      Expansion 27, AccountFlags 0
69:00000020 00000000 0005 0006 0006 0006 0006 0006 0006   nano 32, field_108 0, Swim 5, STR..PSY 6
91:01            level 1
92:0022          max_health 34 (u16)
94:00            health delta 0 -> 34
95:00000000 0064 001f 00   monster_data 0, scale 100, visual_flags 31, mode 0
104:0000002a <42 bytes>      blob
150:00009eb5      head mesh 40629
154:06            run speed 6
155:000003f1 000003f1        effects 0, cloth 0
163:000007e2 00 00009eb5 00000000 04   attractors: 1 entry (place 0, mesh 40629, 0, 4)
177:00000000      F3
181:00            flag_32f
```

NPC (`Surf Lizard` 1016304, payload 191 bytes): `F = 0x20a4a53` (bit 0 NPC, 4 textures, 6 playfield, 9 rotation, 11/14 health forms),
playfield 4582, pos (841.4485, 21.7523, 746.3075), rot identity, packed `0x5cb` (side 3, fatness 1, breed 6, sex 1, race 1), name `Surf Lizard`, `F2 = 0x200`, `npc_family 37` (u8),
level 1, health 25/25, `monster_data 22794`, scale 90, texture `lizard_green` → 22768. Hex:
`271b3a6b0000c350000f81f0003a020a4a53000011e644525cb541ae04cd443a93af0000000000000000000000003f800000000005cb0c53757266204c697a6172640000000200000000032500000000010019000000590a005a001f000000001c0000000000000000000000000301000100010001000100000002000006000007e26c697a6172645f677265656e0000000000000000000000000000000000000000000058f00000000000000000000003f1000003f1000003f10000000000`

### 1.5 What the client does with it

`FUN_10077e13` [GC]: if the identity's dynel does not exist yet and the playfield does, it creates the dynel through the ribosome
(`n3Dynel_t::CreateDynel` → `SimpleChar_t`), adds it to the playfield at `pos`/`rot` (`n3Playfield_t::AddChildDynel`, or attaches it to `parent`),
sets `n3Dynel_t+0x21d`, if `tower_type == 0` creates effect `100000 + id` (`id` = the playfield id, or the tilemap's id for playfield ids ≥ 10000), makes the dynel the cell (or, in dungeons, room) monitor
source if its id equals the client's own character id (`SetCellMonitorSource`/`SetRoomMonitorSource`), then fills the stat object (`dynel+0xe8`) from the fields
(`Fields → stats` in §1.1), `SetTextureDataList`, cloth (`FUN_100480fc`), head/attractor meshes (`CharacterMesh::AddAttractorMesh/ClearAttractors/AddAttractors`),
`SetBodyScale(MonsterScale/100)` and finally `DisableVisibility` for a non-own dynel when a stat read (arguments not recovered, [GC 0x100783a1]: stat 0xC2 `InPlay`, kind 2) is 0. The own dynel (`IsClientChar`) uses the PC branch
with fewer stats written (the `n3Dynel_t+0x21c` flag).

**Live changes** (`AppearanceUpdateIIR_c`, [GC 0x10071679], docs/zone/world.md §7): applied to any `SimpleChar_t` of identity kind 50000, NPCs included (no NPC special case): cloth entries by `(page*5 + part)` (`FUN_100480fc`, only changed textures written, 0 clears), `VisualFlags` stat 0x2A1, and the attractor list replaced wholesale (`ClearAttractors` + `AddAttractors`). `Dynels::on_message` edits the `CharLook` (`CharLook::apply_appearance`) and rebuilds the model through `Req::Model`; details and evidence in docs/zone/avatar.md §5.

**Unresolved:** `field_108`, `mode`, the exact meaning of `AttractorMeshData` byte/int, `target`
(`+0xbc`), `path`, `list_190`, `effects`, bits 2/18/21/22 consumers, the MonsterData → model mapping, the sign of the heading, and the PC `extra`/`name_parts` strings
(code paths read, never seen on the wire).

## 2. `CharDCMoveIIR_t` (`54111123`)

Class: base reader `FUN_1006bb79` [GC] + `CharDCMoveIIR_t` fields. vftable [GC 0x10160644]; ReadSubClass [GC 0x1006b96f]; WriteSubClass [GC 0x1006b9d6] (base writer
`FUN_1006bc55`, the **client sends this message**; the written `i32` is `GameTime_t` as int); apply [GC 0x1006b84b]; resolve/place [GC 0x1006bcc6].

| offset | type | field | meaning |
|---|---|---|---|
| 13 | `u8` | `move_type` | `value & 0x7f` (bit 7 discarded) → `IIR+0x18`. |
| 14 | 4 × `f32` | `rot` | quaternion `x y z w` → `IIR+0x28`. |
| 30 | 3 × `f32` | `pos` | `x y z` → `IIR+0x1c`. |
| 42 | `i32` | `time` | `IIR+0x38`; negative → 0; the server sends 0. |
| 46 | 2 × `f32` | `extra` | `IIR+0x40/0x44`; used by move type 0x16 only; 0 live. |

Total 54 payload bytes. Reader rejects non-finite floats (all seven of base + the two extras).

Client behaviour (`FUN_1006bcc6`, `FUN_1006b84b`): `n3Dynel_t::GetDynel(identity)`; the message is dropped (`ClearToBePassedOn`) if the dynel does not exist, or is the client-controlled dynel while `FUN_1006bb6c` (a vtable slot 3 call returning 1) is false; otherwise it calls `n3VisualDynel_t::UpdateReconcilePos`, then
`n3Dynel_t::SetRelPosRot(pos, rot)` (so the message **teleports/places the dynel to `pos`, `rot`**), and for our own dynel records
`UpdateLastMotionMessageData`. Then, if the dynel's stat `Features` (0xE0, kind 2) has bit 4 or bit 2 set, the move type drives the movement state machine: types
`9 ≤ t ≤ 14` and `t == 0xf/0x16/0x1d` take special paths (`0x16`: `vtable+0x74(relpos, rot, extra[0], extra[1])`), everything goes through
`FUN_100574e2` / `FUN_1006efd2` = `(dynel+0x178)->vtable[6](move_type)` = the SimpleChar movement FSM transition by id. Transition classes
are created in `FUN_1006c60f` [GC] by a 42-way switch on the id (jump table [GC 0x1006d0ee]); by the first `*::vftable` store after each case entry the ids map to:
1 ForwardStart, 2 ForwardStop, 3 ReverseStart, 4 ReverseStop, 5 StrafeRightStart, 7 StrafeLeftStart, 8 StrafeStop, 12 TurnLeftStart, 14 TurnStop, 15 JumpStart,
16 JumpStop, 17 ElevateUpStart, 18 ElevateUpStop, 21 FullStop, 24 SwitchToWalkMode, 25 SwitchToRunMode, 26 SwitchToSwimMode, 27 SwitchToCrawlMode,
28 SwitchToSneakMode, 29 SwitchToFlyMode, 30 SwitchToSitGroundMode, 33 SwitchToSleepMode, 34 SwitchToLoungeMode, … (**partial and not verified**: neighbouring
cases share code blocks, so a case can be attributed to its successor; ids 6, 9-11, 13, 19-20, 22-23, 31-32, 35-42 are ambiguous or unmapped here).
**Live move types** (144 messages): `30 ×60` (the initial placement of NPCs right after they appear: identity quaternion or their spawn heading, position equal to the
`SimpleCharFullUpdate` position; by the table above that is "SwitchToSitGroundMode" which contradicts "standing NPCs" → the table is unreliable at 30),
`1 ×31`, `9 ×12`, `11 ×7`, `7 ×7`, `22 ×7`, `10 ×6`, `13 ×5`, `12 ×4`, `14 ×3`, `2 ×1`, `8 ×1`. Sender is always 1; `time` and `extra` are always 0.

Example (first captured, NPC 1025299 "Cross-Wired Junkbot"): `54111123 0000c350 000fa513 00 1e 00000000 00000000 00000000 3f800000 4456291e 4220230f
442cce6a 00000000 00000000 00000000` → type 30, rot identity, pos (856.6425, 40.0342, 691.2252), time 0, extra (0, 0). Encode round-trips this frame byte for byte (test).

**Unresolved:** the names of the move type ids (see above), the meaning of `extra` outside type 0x16, `time` semantics for server-sent values (always 0).

## 3. `StatIIR_t` (`2B333D6E`)

vftable [GC 0x10166d54]; ReadSubClass [GC 0x100a1a7c] → `FUN_10009dd4`; WriteSubClass [GC 0x100a1a9b]; apply [GC 0x100a1aaf].

| offset | type | field |
|---|---|---|
| 13 | `i32 n` | number of pairs |
| 17 | `n × (i32 stat, i32 value)` | stat id (client table in §0) and new value |

The client keeps the pairs in a `std::map<int,int>` (later duplicates overwrite); apply sets each stat on the dynel and shows combat text: stat 0x1b (Health)
deltas → floating damage/heal numbers, 0x28 (AlienXP), 0xA9 (AlienLevel, sets DValue `got_perk`), 0x34 (XP), 0x23D, 0x2AA..0x2AC (LDB strings). Live (69
messages, one pair each, sender = the dynel): `(0x300 = 768, 0|1)` ×60 (stat 0x300 is not in the client's name table), `(0x40 HeadMesh, 0 | 40629)`, `(0x209 SocialStatus, 0)`,
`(0xAD CurrentMovementMode, 3)`, `(0xD6 CurrentNano, 162…166)` ×3, `(0x1B Health, 215 | 190)`. Example: `2b333d6e 0000c350 000fa513 00 00000001 00000300 00000000`.

## 4. `SetWantedDirectionIIR_t` (`60201D0E`)

vftable [GC 0x1015d4ac]; ReadSubClass [GC 0x1003a71b]; apply [GC 0x1003a78a]. Body: 3 × `f32` (`dir x y z`) stored to `SimpleChar_t+0x1f8/0x1fc/0x200` (the dynel must
be a `SimpleChar_t`). Live: 72 messages, `y == 0`, `x² + z² = 1` (a unit heading vector in the XZ plane; first = (0.3675, 0, 0.9300)). Example:
`60201d0e 0000c350 000f9324 00 3ebc29ea 00000000 3f6e15d5`. **Unresolved:** consumer of `SimpleChar_t+0x1f8` (assumed movement/turn target).

## 5. `CastNanoSpellIIR_t` (`25314D6D`)

vftable [GC 0x10160d8c]; ReadSubClass [GC 0x10072274]; WriteSubClass [GC 0x100722c6] (**the client sends it**); apply [GC 0x10072306].

| offset | type | field |
|---|---|---|
| 13 | `i32` | `spell` (nano record id; 163449 live) |
| 17 | `Identity` | `target` |
| 25 | `i32` | `flag` (kept as `bool`, 1 live) |
| 29 | `Identity` | `source` (second identity; equals the caster live) |

Apply: if the dynel is a `SimpleChar_t`: `FUN_10051754(spell, &target, 1, flag, 1, 1, &source, 0, 0)` (start the cast on the caster = header identity). Live: 3 messages, NPCs 1026282
(×2) and 1025299 cast spell 163449 at player `Bergdoktor` (50000, 33402). Example: `25314d6d 0000c350 000fa8ea 00 00027e79 0000c350 0000827a 00000001 0000c350 000fa8ea`.
Encode round-trips (test). **Unresolved:** the semantics of `source` ≠ caster and of `FUN_10051754`'s extra arguments.

## 6. `WeaponItemFullUpdateIIR_t` (`3B1D2268`, identity kind `0xC74A`)

vftable [GC 0x10166e5c]; ReadSubClass [GC 0x100a2754] → base reader [GC 0x100a110a]; apply [GC 0x100a27a5] (kind must be `0xC74A`, playfield `+0x48` must exist; `CreateDynel`, `FUN_100cb7bc/100cc270`).

| offset | type | field | meaning |
|---|---|---|---|
| 13 | `i32` | `version` | must be 11 (`DAT_101c1160`). |
| 17 | `Identity` | `parent` | holder; live `(50000, id)` of an NPC or player. |
| | 7 × `f32` | placement | iff `parent` is null: `x y z`, quaternion (an all-zero quaternion → identity). |
| 25 | `i32` | `playfield` | 4582. |
| 29 | `Identity` | `template` | `+0x4c`; live `(1000015, 0)`. **Unresolved.** |
| 37 | `u8`, `u8` | `byte_70`, `byte_71` | live (1, 6), (1, 8). **Unresolved.** |
| 39 | list | `stats` | count × (`i32 stat, i32 value`) (`FUN_1002e618`/`FUN_1002e528`). Live 9 pairs: `Flags 0x403`, `StaticInstance (0x17) 265090`, `ACGItemLevel (0x2bd) 25`, `ACGItemTemplateID (0x2be) 265090`, `ACGItemTemplateID2 (0x2bf) 265091`, `MultipleCount (0x19c) 1`, `Energy (0x1a) 0`, `MaxEnergy (0xd4) 25`, `AmmoType (0x1a4) 2`. |
| | `i32 len`, `len` bytes | `blob` | bytes only if `len > 0` (0 live). |

10 messages: 8 for the ICC Shuttle Guards' rifles (items 178411…, `EP3_assault_rifle_03`-class stats), 2 for the shotguns of the player `Stanko` (parent 33491,
`StaticInstance 0x3ca19`). The weapon mesh itself is attached through the holder's `AttractorMeshData` (§1.3), not through this message.

## 7. `SpecialAttackWeaponIIR_t` (`1D3C0F1C`)

vftable [GC 0x101615d0]; ReadSubClass [GC 0x10079a83]; WriteSubClass [GC 0x100799fc]; apply [GC 0x1007989a]. Body: list (count × 4 `i32`, read `[0] [1] [3] [2]`, `FUN_10065687`),
then 5 × `i32` → stats `CloseCombatInitiative` (0x76), `DistanceWeaponInitiative` (0x77), `PhysicalProwessInitiative` (0x78), `NanoProwessInitiative` (0x95), `AggDef` (0x33)
(each `SetStat` + `FUN_10064916(stat, 0)`); apply also calls `FUN_1006aef6/FUN_1006ac03/FUN_1006a5c7` first. Entry fields (port `SpecialAttackEntry`): `f0` low-QL / `f1` high-QL item template (rdb 1000020, kind `0xc74a`), `f3` = list key (`piVar5[5]`; 100 = the player's martial-arts item, 144/142/1 = specials, a 4-char code for NPCs), `f2` = its text code. The list becomes the `DummyWeapon_t` items behind the `AttackInfo` weapon slots: docs/zone/combat-log.md §2.1.1. The N3 `flag` byte is 1 for every instance.

Live: 58 messages with 1, 3, 4 or 5 entries (counts `1×3, 3×26, 4×6, 5×23`). The player's own (first message, sender 25988): entries `(43712, 144745, f3=100, f2='MAAT')`,
`(42033, 42032, 144, 'DIIT')`, `(70292, 70293, 142, 'BRAW')`, `(207779, 207779, 1, 'NBCK')`, stats `(6, 6, 6, 6, 100)`. Example (player, 101 payload bytes):
`1d3c0f1c 0000c350 00006584 01 000013b5 0000aac0 00023569 00000064 4d414154 0000a431 0000a430 00000090 44494954 00011294 00011295 0000008e 42524157 00032ba3 00032ba3 00000001 4e42434b 00000006 00000006 00000006 00000006 00000064`.
For NPCs the third and fourth ints are the same four ASCII letters (`AZXG`, `WXJL`, …). **Unresolved:** meaning of `f0`/`f1` (look like rdb ids; for the player the pair `43712/144745` etc.),
the weapon special-attack shorthand `f2` (`MAAT`, `DIIT`, `BRAW`, `NBCK` look like special-attack abbreviations; not verified against the client's string tables), `f3` (100/144/142/1 on the player).

## 8. API summary

`dynel::decode(&N3Header, &mut Reader) -> Result<Option<Dynel>>` returns `Ok(None)` for other ids, `Err` for short/malformed data (container counts, versions, non-finite floats,
oversized lengths). Structs: `SimpleCharFullUpdate` (+ `CharClass::{Pc, Npc}`, `ClothData`, `AttractorMesh`, `TextureData`, `Path`, `EffectEntry`), `CharDCMove` (`encode`,
`yaw`), `StatUpdate`, `SetWantedDirection`, `CastNanoSpell` (`encode`), `WeaponItemFullUpdate`, `SpecialAttackWeapon`. Unknown trailing bytes are kept in `rest`.
Tests: `cargo test --release -p ao-net dynel` (13 tests: every captured instance of all seven messages, round trips of the two client-sent messages, every truncation of a
sample of each message must error).
