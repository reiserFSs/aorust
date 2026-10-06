# NPC / monster records (rdb 1040023) and how the client skins a NPC

Code: `ao_formats::character::NpcRecord` (`crates/ao-formats/src/character/npc.rs`), survey `cargo run --release -p ao-formats --example npc_stats`
(`npc_stats <id>…` dumps records). `[CODE]` = read from the decompile, `[DATA]` = measured on the client data, `[GUESS]` = not proven.
All addresses are in the 32-bit client DLLs (Ghidra projects `/tmp/aomac-ghidra/playfield`, `terrain`).

## 1. Who reads it

* The record type is never a literal in any DLL: `Gamecode.dll` `FUN_1004dc30` builds `Identity{type 0xfde97 (= 1040023), id}` (`*(local - 0x1c) = 0xfde97`),
  asks `ResourceDatabase_t::GetBinaryStream` (`m_pcInstance + 0x60`), reads it with `FUN_1004d919` and inserts the 0x34-byte object into a global
  `std::map<id, MonsterData*>` (`FUN_1004db93`; cache flush = `MonsterData_t::RemoveMonsterData`, string @0x1015d1b0, called from `FUN_1003909f`).
  Lookup: `FUN_1004dd31(id)` (loads on demand, 0 = no record). The class name `MonsterData_t` appears only in that string [CODE].
* The object is cached per SimpleChar: `FUN_10051d2d(this, id)` sets `this+8 = FUN_1004dd31(id)` (object `+0x10` = id); `FUN_10051f6e()` is the accessor
  (`N3Msg_IsCharacterMorphed` @0x100187c0 = "accessor != 0", so a NPC is a *morphed character*). If stat `Features` (0xE0) bit `0x800` is set the id comes from
  `FUN_10057b41` (decompile lost its return value), otherwise from `this+8` [CODE]. Stat `MonsterData` (0x167) is written by `FUN_10077af2` (SimpleChar update,
  value `*(msg+0x8c)`) and `FUN_100a754c`; its stat-change handler is `FUN_10059e6a` (`iVar8 == 0x167`) [CODE]; the setter chain was not followed further
  ([INFERENCE]: stat write → `FUN_10051d2d`).

## 2. Record layout (`FUN_1004d919`, all little-endian `i32`; verified by 1360/1360 real records)

Every count is stored as `(n + 1) * 1009` (`0x3f1`): `v % 1009 == 0`, `v >= 1009`, `n <= 30000`, anything else sets the stream's error bit (`BinaryStream::clear(4)`).

| off | field | reader | meaning |
|---|---|---|---|
| 0 | `i32, i32` | discarded | section marker pair (`0x0f,0x17` in 22794; `0x0f,0x17` in 254118). Value/meaning unknown (not stored) [GUESS: type tag + size] |
| 8 | count `n` | `FUN_1002e618` | **stat overrides**: `n × (stat id, value)` (vector at object `+0`, linear lookup `FUN_1002e57f(stat, default)`, first match wins) |
| … | `i32, i32` | discarded | marker (`0x0e,0x13`) |
| … | count, `n × { key, count m, m × value }` | `FUN_1007d63a` | **animation multimap** (object `+0x14`): `AbstractAnimID_e` key → rdb 1010003 clip ids (`m` ≤ 30000; several values per key = random variants) |
| … | `i32, i32` | discarded | marker (`0x14,5`) |
| … | same shape | `FUN_1007d6d5` | **sound multimap** (`+0x24`): key → 32-bit Sandy sound ids (see § 4) |
| … | `i32, i32` | discarded | marker (`6,0x1c`) |
| … | count, `n × (i32, i32)` | `FUN_1002b8b6` (`FUN_1013cda9` per pair) | fourth list (object `+8` → vector; adder `FUN_1004d9ce`). **Empty in all 1360 records**; no consumer found |
| … | `u32 len`, `len` bytes | `BinaryStream::read` | name (no terminator on disk; the reader appends NUL, `FUN_1004d877` also rewrites `/` to `X` in a *different* string field `+0xc`) |
| … | 13 bytes | **never read** | present in all 1360 records (`trailing = 13`, zeros in 22794) [DATA] |

Insertion into the multimaps is `FUN_1004e103` (`std::multimap::insert`): sorted by key, equal keys keep file order. Stat vector insertion
(`FUN_1002e5e2`) updates an existing key but the vector starts empty, so duplicates would be appended and the first wins.

### Decoded: 22794 `green lizard` (457 bytes)

```
stats  (0xc=Mesh 22773, 0x1a5=CharRadius 4, 2=VolumeMass 100000, 0=Flags 0x4000, 0xe0=Features 0x8007, 0xc5=1, 0x29=FabricType 0)
anims  0x64 walk 22783 · 0x65 run 22783 · 0x78 idle 22777 · 0x7f impact-chest 22775 · 0xc8..0xcb 55702 attack-spray ·
       0x406 unarmed-start 22781 · 0x407 idle-unarmed 22778 · 0x408 unarmed-stop 22782 · 0x409 55702 · 0x40a..0x40d attack1 22779 · 0x1770 die-pain 22776
sounds 0xf edecdaaa · 0x1e ac4d7b93 (liz_die.wav) · 0x1f adcd79b3 · 0x26 bc8321fb · 0x36 890264fa · 0x73..0x77 …
name   "green lizard"  (no head mesh: stat 0x40 absent)
```

### Decoded: 254118 `icc shuttle guard-rooted` (2205 bytes, humanoid)

Stats `Mesh 5907` (= `solitus_male.cir`), `HeadMesh (0x40) 40627`, `CharRadius 3`, `Features 0x8003`, no `Flags`. The animation table holds the **player clip set**
of model 5907 (`male_walk_01_01.ani` 10191 for key 100, `male_run` 10194 for 101, `male_idle-stand` 10173 for 120, `male_die-pain` 15621 for 501, `male_unarmed-start/idle/stop`
for 1030..1032, `male_smallarms-*` 1010..1018, `male_rifle-*` 1020..1025, `male_blade*` 1000..1007 and 1050, bow 149/181/183/184 …, 200..203 `male_spell-gen/dir/self/sys`,
210..218 fallback/ground/chair start/stop/idle, 222 `male_run-back`, 500..505 `male_die-knees/pain/poison/shot/ground/float`). So humanoid NPCs reuse the **player** model and clips.

## 3. Animation keys

The key space is `AbstractAnimID_e` (`n3EngineClientAnarchy_t::N3Msg_DoSocialAction(AbstractAnimID_e)`, `N3Msg_GetActionByName`, `n3VisualDynel_t::GetImpactAnim`);
the client has **no names** for the enumerators (no PDB). Names below are the clips the client's *own* records file under each key [DATA] (survey of 114 730 table
entries). `NpcAnim` exposes the useful subset.

**Lookup** (`FUN_10010ebe(table, key)`, called as `FUN_1004d8f9(key)` on the record; `NpcRecord::anim` / `anim_key` return the first variant,
`anim_key_variants` the candidate list): take one value of `key` (`FUN_1004570c`, below), else follow the parent key `FUN_10010ad1` until a key hits, the parent is 0, equals itself, or loops back to the start; else the
**first entry of the map** (smallest key, first value; `0` when the map is empty). The holder's resolver `FUN_1003c8b7` -> `FUN_1003c802` [GC 0x1003c802] (every state / stance / attack clip start) walks the same chain
with `FUN_1004570c` per key but **without** the first-entry last resort (`holder_variants`).

**Variant pick** (`FUN_1004570c(key)` [GC 0x1004570c], `pick_variant`): `n = count(key)` (`FUN_100456ac`); `n == 0` -> 0; `n == 1` -> that value **without calling `rand`**; `n > 1` ->
value `rand() % n` of the multimap's equal range (insertion = file order). `rand` = the MSVCR100 CRT's (`CrtRand`: `holdrand * 214013 + 2531011 >> 16 & 0x7fff`, a thread starts at 1);
`srand` callers: `FUN_1011bb4a` [GC] `srand(_time64())` (reads the playfield; a playfield-load remapper) and `GfxVisualNano2::ProcessStuff` [DS 0x1001975e] `srand(0x2a)` / `srand(effect seed)`
per particle effect (not modelled). **When it is rolled**: on every lookup, i.e. every clip *start*: the movement-state handler `FUN_1006c065` -> `FUN_1006be27` (callers: the state
transitions `FUN_1006d196 d5d0 d69c d821 d9e8 dd0c de93 df81 eb63 ecf0`) starts the idle (key from `holder+0x10`, default 0x78) / walk / run clip with `AnimHolder::Play(clip, 1.0, loop -1 (0xffffffff), layer 2)`:
an **infinite loop**, so an idle never re-rolls at the loop end and there is no chance rule or blend between idle variants; the next roll happens at the next state change (also `FUN_1003ea0d` revive,
`FUN_1006fcfa` end of a waypoint path, `FUN_1003cc15`/`FUN_1003cad0` stance idle, `FUN_1006a239` attack, `FUN_100a4dcc` emote). Implemented per character in `dynels::Roll` (rolled when the clip key or movement state changes), all variants are loaded into `Built::clips`.

| key → parent | |
|---|---|
| 0x3fc→0x3f2, 0x3fd→0x3f3, 0x3fe→0x3f4, 0x3ff→0x3f5 | rifle start/idle/stop/shot → smallarms |
| 0x41e→0x78, 0xb2→0x78 | → idle |
| 0x421→100, 0xb3→100, 0x66→100 | → walk |
| 0x422→0x65, 0xb4→0x65 | → run |
| 0x424→0x3fd, 0x426→0x3ff | |
| 0x7e→0x7f, 0x80..0x84→0x7f | impact back/head/arms/stomach → chest |
| 0xc4→0x86, 0xc5→0x87 | only while the client char's stat 0x296 ≠ 0 (not modelled → 0) |
| 0xcc, 500..0x1f9→6000 | any death → generic die |

`FUN_1004dc30` (loader): if the animation map is non-empty and key 0x65 (run) is absent, it is filled from key 100 (walk) (`FUN_1004570c(0x65)==0 → FUN_1004570c(100)`;
the decompile lost `this`, so the target map is [INFERENCE]; every real record already has 0x65).

Key meaning [DATA] (clip suffix most frequent for the key): 100 walk · 101 run · 103 crawl (104/105 start/stop) · 107/109 op1h-button/wield · 114/115 pickup low/middle ·
118/119 push/pull · **120 idle-stand** · 126..132 impact back/chest/head/larm/rarm/back/stomach · 133 swim · 134/135 walk-left/right · 136 walk-back · 137..142 sneak/hide ·
146..148 keypad/lever · 149 bow set · 150..153 first-aid/throw · 154/155 idle/wield-crawl · 156..158 jump · 160..181 unarmed special moves · 178..180 hover · 183 bow shot ·
185..187 jump-land · 195 idle-swim · **200..203 spell-gen/dir/self/sys** · 210..218 fallback/ground/chair start/stop/idle · 222 run-back · **500..505 die-knees/pain/poison/shot/ground/float** ·
1000..1007 blade · 1010..1018 smallarms · 1020..1025 rifle · **1030..1032 unarmed start/idle/stop · 1033..1037 unarmed attacks** · 1050..1062 blade2h/…· **6000 die (generic)**.
Creatures only file a handful: lizard = 100, 101, 120, 127, 200..203, 1030..1037, 6000 (so combat = 1034..1037 `attack1`, spell cast = `attack-spray`).
Key 0x78 is also what `FUN_1012b24c` (VisualCATMesh created for a spell/effect, low-detail rdb 0xf696b = 1010027) plays on a record: `SetAnimation(clip(0x78), …)` + `SetScale`.

## 4. Sound table and stats

* Sound keys use the same `AbstractAnimID_e` space (`FUN_10045069(key)`, the footstep/impact player: 0xb, 0xf, 0x25..0x27, 0x36..0x3b, 0x73..0x7f, 0x85, …, material =
  ground tile type; `CharDie_t` ctor `FUN_1007b2ba` plays key **0x1e** (death) when a record exists, key 0x1f is the hit sound (`FUN_1009b4ac`)). Values are Sandy sound ids
  (`sound_id(name)`, `crates/ao-audio/src/sbf.rs`): all 10 ids of 22794 exist in `sound/SourceFiles/SM_Sandy_Game_Dummy.sbf`; 0x1e → `sfx/creatures/lizard/liz_die.wav`, 0x1f/0x73..0x77 are
  child-lists (random variant), 0xf/0x26/0x36 look like step/hit sets [GUESS]. Exposed raw as `NpcRecord::sounds`.
* Stats (`NpcRecord::stat`, names from `data/stat_names.txt`): record-read by the client: **12 `Mesh`** (`FUN_10058078`: model override, 0 = keep the dynel's own `Mesh`; two
  records have 0: 163119 `molokh`, 205481 `lctower_supplymasters_control_tower`), **64 `HeadMesh`** (rdb 1010001; non-zero ⇒ `n3VisualDynel_t::SetCatMesh(mesh, true)` +
  `VisualCATMesh_t::SetSkinData(breed, sex, race)` = the naked skin textures of the head's breed/race, `FUN_100c2c15`), **41 `FabricType`** (impact effect type 1..17,
  `FUN_1009b4ac`). Present in the data: 0 (1326), 2 VolumeMass (1360), 12 (1360), 41 (1316), 64 (1157), 0xc5 (1324, unnamed), 0xe0 Features (1360), 0x165 (770, unnamed), 0x1a5 CharRadius (1360), 0x1c6
  ProximityRangeOutdoors (35). The consumer that copies the other stats (VolumeMass, CharRadius, Features, …) to the dynel was **not found** (not `FUN_100491f5`).

## 5. How the client skins a NPC (`TextureData_t`)

`SimpleChar` update `FUN_10077af2` / create `FUN_10077e13` store the wire list with `n3VisualDynel_t::SetTextureDataList` (N3 @0x1001a3fb; copy kept at `dynel+0xb0`,
`GetTextureDataList` @0x10007575). `SetCatMesh` (N3 @0x10019fb2, `Identity{0xf6952 = 1010002, mesh}`) hands that copy to `VisualCATMesh_t::SetMesh(identity, list)`
(DisplaySystem @0x100728b7) → `AsyncCATMesh` (0x6c bytes, ctor `FUN_10070368`) → `FUN_10070247` (= `SetCATTextures(list, layer)`), which for each 0x2c-byte element:

```
name = elem+0x00  (char[32])      tex = elem+0x20     tex2 = elem+0x24     alpha = elem+0x28
if tex  != 0: SetCATTexture(name, material=-1, tex , layer 1 (SetMesh) / caller's layer, alphaMode = alpha)   // FUN_100700e9
if tex2 != 0: SetCATTexture(name, material=-1, tex2, layer 3,                           alphaMode = 0)
```

* The **element has the layout of a `CatMesh` part-table entry** (`name32, texture, env_texture, alpha_aux` = `Part`): wire `name` ↔ `Part::name`/`Material::name`
  (`RCATMesh_t::GetMaterialIndex(name)`, exact byte compare; material = `-1` is resolved through the name), `tex` ↔ `Part::texture` (rdb 1010004, layer 1 = diffuse),
  `tex2` ↔ `Part::env_texture` (layer 3 = environment/second texture). `alpha` (stored as 5 when the wire int ≠ 0) is passed as `AlphaMode`; its effect is **not resolved**
  (compare `Part::alpha_aux`). A later entry for the same (name, layer) updates the first (`FUN_100700e9` searches the list at `+0x28` and replaces the texture).
  Entries whose name matches no material are stored but never used.
* Evidence: NPC 22794's wire texture `lizard_green` → 22768 (`lizard_brown.png`); mesh 22773's only part is `lizard_green` with default texture 22774 (`lizard_green.png`);
  humanoid parts are `arms/feet/hands/legs/body` (`ClothData_t::GetName(0..4)`, 5 parts in `FUN_10070439` = `SetSkinData`, which writes layer 0 = naked skin per part).
  Layer 0 = skin base, 1 = diffuse, 3 = environment [CODE: constants 0/1/3 in the three call sites].
* `ao_formats::character::texture_overrides(&CatMesh, &[TextureOverride])` returns `(part index, TextureOverride)`; a texture of 0 keeps the part's own.
  (The `[0x10150828]` compare in `FUN_10070247` — substitute id 0x440aa/alpha 5 when `tex` equals a global — is a zero in the static image; treated as never true.)

## 6. Cloth, attractors, scale, visibility on monsters

* **Cloth** (`ClothData_t`, 5 ints): `FUN_100480fc(clothComponent, vector)` (called unconditionally from `FUN_10077e13` for every dynel, NPC included): for each entry,
  slot = `page(+0x10) * 5 + part(+0)`; table record `{+0x14 = e[1], +0x18 = e[2], +0x1c = e[3]}` at `(slot*0xc) + this`, set `this+0x18c = 1` (dirty) when `e[1]` changed.
  The component is `dynel+0x1b8` (`FUN_10058078` sets the same flag). Which consumer turns the table into textures for a *morphed* NPC was not found: monster models never go through
  the player equipment path (`FUN_10058078`: when a record exists the breed/sex/equipment model selection `FUN_10057eb3/10057ff7` is skipped).
* **Attractors**: `FUN_10077e13`, only when message flag bit 2 (`SET_DYNEL_800`) is clear: `CharacterMesh::AddAttractorMesh(cm, place 0, headMesh(+0xac), 4, 0)`, then
  `CharacterMesh::ClearAttractors` [DS 0x10071dd0], then `AddAttractors(cm, vector<AttractorMeshData_t>)`; an attractor = (place, rdb 1010001 mesh, int, byte) mounted on the model's `AttractorNN_*` point
  (lizard mesh: `Attractor01_head`, `Attractor06_back`; 5907: head, left/right hand, shoulders, back). Same code for players and NPCs [CODE].
  **`CharacterMesh::ClearAttractors` walks the attractor list (`this+4`, the list `AddAttractorMesh` [DS 0x10071cce] inserts into, ordered by place, new node before the first node with place >= its own) and deletes
  every node: the head added one call earlier is gone.** The mounted set is therefore exactly the wire list; the head is its place-0 entry (`HeadMesh` and the place-0 entry are equal in every capture), a list without place 0 shows no head,
  and the `HeadMesh` stat of the NPC *record* only feeds `SetSkinData` (`FUN_10058078`). (`VisualCATMesh_t::ClearAttractors` [DS 0x10073d8a], called by the appearance update `FUN_10071679` before `AddAttractors`, is a separate method that also unmounts from the render mesh and sets `+0x78`.)
  The runtime head change `FUN_10059376` = `RemoveAttractorMesh(0, old)` + `AddAttractorMesh(0, new, 4, ...)`. Implemented as `actor::attractor_list` (test `clear_attractors_drops_the_head_mesh`), used by `dynels::build_char`.
* **MonsterScale** (stat 0x168 = 360; wire percent): `FUN_1005bea6` ends with `n3VisualDynel_t::SetBodyScale(stat(0x168, kind 3) / 100.0)` (`_DAT_10158670` = 100.0);
  the stat setter clamps to **≥ 20** (`FUN_10059e6a`: `0x168` → 0x14) and `CharRadius` (0x1a5) = `MonsterScale × value / 100` (same function). Uniform scale of the whole model.
* **Visibility**: `DisableVisibility` = `n3VisualDynel+0xc9 = 0` (N3 @0x1001954a; Enable sets 1; the VisualCATMesh has its own `+0x70`). `FUN_10077e13` calls it for
  every non-own dynel when stat `InPlay` (0xC2) is 0; the update `FUN_10077af2` writes stat 0xC2 = `VisualFlags >> 1 & 1`, 0xDF = bit 3, 0x159 = bit 0x1c, and sets Features
  bits `0x800` (flag bit 2) and `0x800000` (bit 0x12). VisualFlags bit 0 = NPC (branch that also sets stat 0x21 `Side`, 0x1c7/0x1d2/0x200/0x184).

## 7. Survey result (`npc_stats`, client v0.7.2)

```
records 1360 parsed 1360 (100.00%)
with mesh stat 1358 / no mesh override 2
mesh exists in 1010002: 1358/1358 (100.00%)
head mesh exists in 1010001: 391/391
anim ids 114730: exist in 1010003 114730 (100.00%), skeleton signature == model 114698 (99.97%)
non-empty 4th list 0, unnamed 0, records with trailing bytes 1360 {13: 1360}
```

The 32 signature mismatches are 3 records: 257292 `unicorn lander` (mesh 257288 vs clip 257290 `Unicorn_Lander_cat_anim.ani`, all 26 keys), 259859 `pre-order mech`
(keys 0x409/0x40a/0x40d → `pre_order_fire.ani`), 260117 `mech - recon` (keys 0x3f6/0x3f7/0x401/0x40d → `recon_mesh_attack_stomp.ani`): data errors; the client's
`CATRender_t::SetAnim` rejects them the same way.

## 8. Unresolved (addresses searched)

* Marker pairs (`0x0f,0x17` …), the 13 trailing bytes, the fourth list: Gamecode `FUN_1004d919` ignores them; no other reader exists (`0xfde97` occurs only in `FUN_1004dc30`, searched all DLLs for the 4-byte constant and the decimal 1040023: only Gamecode 0x1004dc4c).
* Consumer of the record's other stats (Flags 0x4000, VolumeMass, Features, 0xc5, 0x165, CharRadius): the only direct readers found are `FUN_1004d8e6(stat)` callers `FUN_10058078`, `FUN_1006fb56`, `FUN_1009b4ac`; likely folded into the dynel's stat object (vtable `+0x3c` GetStat) [GUESS].
* Cloth table → textures for morphed chars; `AlphaMode` 5; the global compared in `FUN_10070247`; sound key names; the stat-0x167 → `this+8` setter (`thunk_FUN_10152f70` @0x10152f70 decompiles to garbage); `FUN_10057b41` return (monster id when Features & 0x800).
* `AbstractAnimID_e` enumerator names (no symbols; derived from clip names above); the effect-driven `srand` calls (`ProcessStuff`) that reseed the real `rand()` stream; the exact call moment of `FUN_1011bb4a`'s `srand(time)` ([GUESS]: zone start).
