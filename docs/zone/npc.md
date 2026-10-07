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
  (`N3Msg_IsCharacterMorphed` @0x100187c0 = "accessor != 0", so a NPC is a *morphed character*). If stat `Features` (0xE0) bit `0x800` is set,
  `FUN_10051f6e` instead looks up **99902**: `FUN_10057b41` ends at `0x10057b60` with `MOV EAX,0x1863e; RET`, regardless of its preceding stat `0xa1` read [CODE].
  Otherwise the accessor returns the holder's `this+8`. This flag-driven morph is not a fallback for a missing record.
  Stat `MonsterData` (0x167) is written by `FUN_10077af2` (SimpleChar update, value `*(msg+0x8c)`) and `FUN_100a754c`.
  Its stat-change handler `FUN_10059e6a` invokes `thunk_FUN_10152f70` when MechData (`0x296`) is zero; that reaches
  `FUN_10052147` → `FUN_10051d2d` and marks the visual dirty (`dynel+0x2c4 = 1`) even when lookup returned null [CODE].
  `FUN_10051d2d` stores a null lookup result, but retries on the next setter call because the holder remains null.

### Missing MonsterData: no cross-type or default-creature fallback

`FUN_1004dc30` asks **ResourceDatabase**, not ResourceManager, for `{1040023,id}`. A null binary stream destroys the temporary
record and returns failure without inserting into the global cache; `FUN_1004dd31` returns null [CODE]. Consequently
ResourceManager mesh fallbacks cannot rescue this lookup.
The only `AddFallback` callsite found in the retail DLL scan is Interfaces `DatabaseInterfaceModule_t` constructor
`0x1000b322`, call `0x1000b39c`: `ResourceManager::AddFallback(0xf696a,0xf6951,null)` (**1010026 → 1010001**).
Its imported slot `0x10015c2c` has only that reference. It registers no 1040023 fallback [CODE].


`FUN_10058078` first reads the dynel's Mesh (`0xc`) and HeadMesh (`0x40`). With a record, nonzero record overrides replace
those values. **Without a record**, breed > 4 returns without changing the visual; breed ≤ 4 discards the initial Mesh and
runs the ordinary humanoid name resolver `FUN_10057eb3` / `FUN_10057ff7` using breed, sex, stat `0x2f`, the build byte
`dynel+0x208`, and optional stat `0x378`. A failed nonzero-`0x378` variant retries without that suffix. There is no retry of
MonsterData as an item template, direct mesh/CATMesh id, or arbitrary default creature [CODE].

Initial creation supplies no hidden default CAT: Gamecode factory `0x10077a84` → SimpleChar constructor `0x1005cb6a` →
N3 visual constructor `0x10019365` initializes the CAT pointer (`+0xc0`) to null. N3 `GetCatMesh` (`0x1001952f`) reads that
pointer; `SetCatMesh` (`0x10019fb2`) creates a CAT visual only for a nonzero selected mesh. Thus a newly created
non-humanoid with null resolved MonsterData has **no body**, rather than retaining a default creature. The unassigned Mesh
stat is the StatHolder sentinel `0x499602d2`, not a default resource (`0x10009e29`, `0x1002e46a`, `0x10058e52`).
Creation's initializers `0x10077af2` / `0x1005f4f0` and appearance application `0x10077e13` do not supply a substitute;
cloth/head/attractor work is guarded by the existing CAT pointer [CODE]. This conclusion is conditional on null effective
MonsterData and breed > 4; later independent appearance/stat messages, and the flag-driven 99902 morph, remain distinct.


The selected mesh goes through N3 `0x10019fb2` → DisplaySystem `VisualCATMesh_t::SetMesh` (`0x100728b7`) →
`FUN_10070656` → `ResourceManager::GetAsync({1010002,mesh},...)`. Wire textures remain attached to this visual; this
downstream mesh load does not revisit MonsterData. Missing spell-animation records also leave the corpse's mesh intact
(`FUN_100a4dcc` returns when `FUN_1004dd31` is null).

The reported **288560** is absent from installed `rdb_1040023`; an all-56-table read-only search finds it only in
`rdb_1000020` (461 bytes), named **"Terrifying Leet Pet (Halloween Leet Series 2)"** [DATA]. That item-template identity is
not a usable visual fallback. None of the retained captures includes MonsterData 288560, so its live breed/head/texture
fields must be captured before claiming which humanoid model, or unchanged non-humanoid visual, retail selects.

A subsequent FxFrames Borealis NPC probe (`AOMAC_NET_TRACE`, 2026-10-07) captured **302** frames, **255** incoming,
including **30** SimpleCharFullUpdate messages. No packet contained MonsterData/instance/raw big-endian value 288560.
MonsterData counts were `0:1, 17655:5, 17687:4, 17708:1, 22794:6, 30365:2, 154136:11` [DATA].
Therefore the real reported entity's branch and rendering remain **unverified**, not a live PASS. The runnable
`missing_monster_data_builds_humanoid_with_named_clips` regression uses real absent id 288560 with an explicit humanoid
look; it verifies model/clip resolution and missing corpse-animation handling, not the absent live creature's appearance.


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

After movement-state Play, `FUN_1006be27` calls `FUN_1006fb56` even for idle: speed zero means vehicle maximum velocity (`Vehicle_t+0x3c`), not stationary playback. Calibration × inverse MonsterScale × maximum/reference velocity is set once on the new handle; ordinary idle is not unconditionally rate 1.0. Direct stance idle Play (`FUN_1003cc15`/`FUN_1003cad0`) remains authored-rate playback; see the clock distinction in `avatar.md`.

The test-only live harness can inspect the sampled NPC clock without forcing movement:
`selname=<NPC>,npcprobe=target,npcwait=walk:30,frames=npc-cycle:2` captures 120 fixed-60Hz frames and logs
the selected abstract clip key, RDB source id, CAT root name, duration/loop markers,
absolute clock, sampled pose time, rate, model, body scale and movement status per frame.
Use a naturally walking NPC; holding the own avatar's W key does not drive that NPC.
The CAT root name is skeleton metadata, not an asset filename.
`npcwait=idle[:timeout]` gates an idle capture instead; neither wait guarantees the NPC will
remain in that state for the entire capture.

Live offscreen evidence (Ithaca PF 800, captured log `artifact://21582`, lines 25–506):
Viviparous Lizard 1019367, model 22773, scale 0.900 stayed Walk throughout
`final-npc-walk-0000..0119`: abstract key 0x64, CAT source 22783/root `Bone01`,
duration 4000 ms, loop 1333..2666 ms, rate 1.111111. The absolute clock advanced
37.037 → 2240.742 ms: 2203.705 ms across 119/60 simulation seconds, matching
`119/60 × 1000 × 1.111111 = 2203.704` ms (rounded). The authored repeated span
therefore has a 1.1997 s wall/simulation cycle at this rate; total clip duration is
not the repeated-cycle length. The capture agent inspected all 120 frames in four
cropped chunks and reported visible leg/tail walking (body still visible at the upper
edge in the last chunk); this is offscreen evidence, not a real-window or retail comparison.
The subsequent `final-npc-idle` sequence began Idle (0x78/source 22777,
3333 ms, no loop markers, same rate), but ended Walk: it does **not** establish
two seconds of uninterrupted idle.

Uninterrupted idle was subsequently captured on clean origin/main `839c057`
(Ithaca PF 800, `artifact://22268`): `selname=Aleksei Innokenti,approach=target,
npcprobe=target,npcwait=idle:30,frames=town-idle:2`. Aleksei 1000216 stayed Idle
in every `town-idle-0000..0119` frame: key 0x78, source 10173/root `Bip01_ac`,
model 5907, scale 1.180, duration 3799 ms, no loop markers, rate 1.178429.
Absolute clock 9228.418 → 11565.652 ms advanced 2337.234 ms across 119/60 s,
versus 2337.218 ms from the logged rate (0.016 ms accumulated float difference).
The full-duration repeat is therefore 3.223784 s; sampled pose wrapped from
3790.887 ms at frame110 to 11.527 ms at frame111 without changing clip or state.
All 120 frames were inspected in six 20-frame crops: head/torso remained visible
with subtle idle sway and no walking; the own avatar partially occluded the legs.
This is muted, locked, offscreen live evidence, not a retail/window comparison.

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
  `VisualCATMesh_t::SetSkinData(breed, sex, race)` = the naked skin textures of the head's breed/race, `FUN_100c2c15`), **41 `FabricType`** (impact material 1..17, `FUN_1009b4ac`; [DATA] only 19 of the 1360 records carry one, always 1; sound path: combat-anim.md section 6.1). Present in the data: 0 (1326), 2 VolumeMass (1360), 12 (1360), 41 (1316), 64 (1157), 0xc5 (1324, unnamed), 0xe0 Features (1360), 0x165 (770, unnamed), 0x1a5 CharRadius (1360), 0x1c6
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
  `tex2` ↔ `Part::env_texture` (layer 3 = environment texture). `alpha` is the `AlphaMode` of the layer-1 texture (the GameData reader stores 5 for any non-zero wire int,
  0 otherwise; see *AlphaMode* below). A later entry for the same (name, layer) updates the first (`FUN_100700e9` searches the list at `+0x28` and replaces texture and alpha).
  The mesh's own part table goes through the same function (`FUN_100704b8` → `FUN_10070247(list at CATMesh +0x28)`), the wire list after it replaces matching entries.
  Entries whose name matches no material are stored but never used.
* Evidence: NPC 22794's wire texture `lizard_green` → 22768 (`lizard_brown.png`); mesh 22773's only part is `lizard_green` with default texture 22774 (`lizard_green.png`);
  humanoid parts are `arms/feet/hands/legs/body` (`ClothData_t::GetName(0..4)`, 5 parts in `FUN_10070439` = `SetSkinData`, which writes layer 0 = naked skin per part).
  Layer 0 = skin base, 1 = diffuse, 3 = environment [CODE: constants 0/1/3 in the three call sites].
* `ao_formats::character::texture_overrides(&CatMesh, &[TextureOverride])` returns `(part index, TextureOverride)`; a texture of 0 keeps the part's own.
  (The `[0x10150828]` compare in `FUN_10070247` — substitute id 0x440aa/alpha 5 when `tex` equals a global — is a zero in the static image; treated as never true.)
* **Humanoid green-hand fix**: `npc_part_textures(store, names, cat, skin_head, textures, cloth)` now selects layer 0 from the record's `HeadMesh` (including named humanoid heads), then composites layer 2 if worn, otherwise the wire replacement/model-default layer 1. Previously it omitted layer 0 and drew solid-green default hands directly; cloth also keyed against the default rather than skin. The key remains only source overlay `RGB565 0x07e0` (`FUN_10074393`); creature materials and mounted head/hair textures are unchanged. Regression `icc_guard_hands_show_record_head_skin` uses record 254118, model 5907 and head 40627 (skin follows the head, not the body's breed/sex); the cross-breed offline survey is in `player.rs`. CharVisuals integrated verification: 35/35 character tests passed, including these regressions and the humanoid NPC-record survey. CharVisuals inspected the live ICC offscreen approach to Brandon Thorn: both hands skin-coloured in the teal suit, unlike the user's prior pure-green-hand screenshot. Legitimate green player hair was not changed.

**AlphaMode** (resolved, randy31 + DisplaySystem). `FUN_10070247` calls `FUN_100700e9(name, -1, tex, layer, alpha, flag)`; the node (`FUN_1006fc70`: name, material, RTexture, tex id, layer, alpha) is applied by
`FUN_1006fdae` as `FUN_10073e4f(material, tex, layer)` (**layer 3 → `RMaterial_t::SetEnvTexture`** @0x10040e5f on the mesh's substitute material, `RCATMesh::CreateSubstMaterial`; layers 0/1/2 → texture slots
of a record, composited by `FUN_1007457f`) and `FUN_10073dd8(material, alpha, layer)` (`FUN_10072818`: the alpha is kept only for layer 1 (`+0x1c`) and 2 (`+0x20`)). `FUN_10074b03` then runs
`RMaterial_t::SetTexture(mat, tex, 0, alpha)` (@0x10040bec, which for stage 0 calls `FUN_10040645` = **`SetAlphaMode`**). Its table (D3D7 ids decoded; `has alpha` = texture `+0xb4` bit 0 = the file carries an alpha channel):

| mode | state set by `FUN_10040645` | scene |
|---|---|---|
| 0 | needs `has alpha`: `UseAlphaAsTransparency`, `ALPHABLENDENABLE` on | `AlphaBlend` (0/255-only alpha drawn as cutout, the contract has no blend+z-write) |
| 1 | `ALPHATESTENABLE`, `ALPHAFUNC GREATER`, `ALPHAREF 0x80`, no blend | `AlphaTest` |
| 2 | blend + alpha test (ref 0x1e), z-write off | `AlphaTest` [GUESS: no blend+test in the contract; `mesh.rs` makes the same choice] |
| 3 | blend, z-write off | `AlphaBlend` |
| 4 | `ONE, ONE`, z-write off, fog colour black, stage 0 `SELECTARG1` | `Additive` |
| 5 | needs `has alpha`: stage 0 `COLOROP ADD`, `COLORARG1 TEXTURE\|ALPHAREPLICATE`, stage 1 `MODULATE` with the same texture | opaque + `glow_mask` (`tex * saturate(lighting + alpha)`) |
| other | returns without a change | opaque |

The `RMaterial_t` constructor itself calls `SetTexture(tex, 0, -1)` (@0x10041128; `-1` = the `mode < 0` branch of `FUN_10040645`): for a textured material with `has alpha`, `UseAlphaAsTransparency`
(`!(flags >> 3 & 1)`) → mode 0, else (≥ 2 texture stages) mode 5. This is `ao_formats::character::{alpha_mode_blend, default_alpha_mode}`; a wire entry with `texture ≠ 0` replaces the mode of that part
(`PartLayer::alpha_mode`), every other part keeps the default. **Evidence**: the survey of all 770 models (716 materials with `flags & 8`) has 115 textures whose alpha is < 128 on ≥ 90 % of the texels
(e.g. model 15263 `head`, 97.9 %); drawn as glow mask the head is a body with a cyan glowing visor (screenshot `view char 15263`), alpha-tested it would vanish and ignored (the previous behaviour for `flags & 8`) it
is a flat texture. `Part::alpha_aux` is 0/1 only (1 = 747 of 3 738 parts, 96 % of them `flags & 8`); its use as a mode is contradicted by the data above, so it is **not** applied [GUESS: the exporter's copy of
`flags & 8`]; searched: `FUN_100704b8`/`FUN_10070247` call chain (the wire/part alpha reaches only layer 1).

**Environment layer** (`Part::env_texture` 127 parts / wire `tex2`, layer 3). `RMaterial_t +0x5c` (`GetEnvTexture` @0x10040afb). `FUN_10056ed6` (the CAT render, randy31 @0x10056ed6) draws every submesh, then if the
material has an env texture redraws the same triangles with the state blob `RCATMesh +0x224` (`FUN_10055a3e`): stage 0 `TEXCOORDINDEX = CAMERASPACENORMAL`, 2 coords, `COLOROP SELECTARG1` /
`COLORARG1 TEXTURE`, `SRCBLEND = DESTBLEND = ONE`, `ALPHABLENDENABLE` on (no fog colour override, so fog tints it), texture matrix `scale(0.5, 0.5, 1)` + translation `(0.5, 0.5, 0)` (`_DAT_1008a5d8` = 0.5f, doubles
@0x1009fd98 = 0.5, @0x1009fde8 = 0): `uv = (0.5 nx + 0.5, 0.5 ny + 0.5)` of the camera-space normal, no v flip. Implemented as `Submesh::env_texture` (`ao_scene::env_uv`, `shader.wgsl::vs_env/fs_env`, additive second draw
of the same triangles in the submesh's own phase, depth `LESS_EQUAL`). The CAT render also splits submeshes into two passes by the material's transparent flag (`+0xbd`: 0 first, then 1), and ends with
`SetRenderPriority(3)` (opaque) or `SetRenderPriority(6)` (frame opacity < 1 or colour modifiers): see dynels.md §1 for the list order.

## 6. Cloth, attractors, scale, visibility on monsters

* **Cloth** (`ClothData_t`, 5 ints): `FUN_100480fc(clothComponent, vector)` (called unconditionally from `FUN_10077e13` for every dynel, NPC included): for each entry,
  slot = `page(+0x10) * 5 + part(+0)`; table record `{+0x14 = e[1], +0x18 = e[2], +0x1c = e[3]}` at `(slot*0xc) + this`, set `this+0x18c = 1` (dirty) when `e[1]` changed.
  The component is `dynel+0x1b8` (`FUN_10058078` sets the same flag). Which consumer turns the table into textures for a *morphed* NPC was not found: monster models never go through
  the player equipment path (`FUN_10058078`: when a record exists the breed/sex/equipment model selection `FUN_10057eb3/10057ff7` is skipped).
* **Attractors**: `FUN_10077e13`, only when message flag bit 2 (`SET_DYNEL_800`) is clear: `CharacterMesh::AddAttractorMesh(cm, place 0, headMesh(+0xac), 4, 0)`, then
  `CharacterMesh::ClearAttractors` [DS 0x10071dd0], then `AddAttractors(cm, vector<AttractorMeshData_t>)`; an attractor = (place, rdb 1010001 mesh, int, byte) mounted on the model's `AttractorNN_*` point
  (lizard mesh: `Attractor01_head`, `Attractor06_back`; 5907: head, left/right hand, shoulders, back). Same code for players and NPCs [CODE].
  `CharacterMesh::ClearAttractors` deletes bookkeeping nodes but does not remove render children.
  Unlike that base method, `VisualCATMesh_t::ClearAttractors` [DS 0x10073d8a] calls `FUN_10072873` →
  `RCATMesh_t::RemoveAttractorChild` for mounted entries and sets `+0x78`. Thus a full update retains
  its separately mounted `HeadMesh` when place 0 is absent (captured Xantarr, head 223820, empty list;
  dynel.md §1.3); an appearance update replaces the rendered set and can clear the head.
  The NPC record's `HeadMesh` only feeds `SetSkinData` (`FUN_10058078`).
  The runtime head change `FUN_10059376` removes/adds place 0. `CharLook::from_update` retains the
  full-update head before `actor::attractor_list` orders the wire bookkeeping list.
* **MonsterScale** (stat 0x168 = 360; wire percent): `FUN_1005bea6` ends with `n3VisualDynel_t::SetBodyScale(stat(0x168, kind 3) / 100.0)` (`_DAT_10158670` = 100.0);
  the stat setter clamps to **≥ 20** (`FUN_10059e6a`: `0x168` → 0x14) and `CharRadius` (0x1a5) = `MonsterScale × value / 100` (same function). Uniform scale of the whole model; `Dynels` applies the same minimum to full-update body scale.
  The own-avatar constructor's zero-to-1.0 fallback and the animation formula's zero-to-1.0 inverse factor (`avatar.md` §3) do not prove that a received non-own full-update zero bypasses this stat-setter clamp. The clamp is therefore retained; changing it requires evidence from the full-update stat path, not a timing symptom.
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

### Missing-record survey (2026-10-07)

A fresh read-only survey of all **22** retained `.rec` files (1,869 server-direction frames) decoded **144**
SimpleCharFullUpdate messages, **124** NPC messages with **29** distinct MonsterData IDs. **All 29 resolve in 1040023**;
each appears in no other RDB table. Thus **zero retained NPC messages/IDs** exercise the missing-record branch; the
reported live 288560 is additional evidence, not one of those captures.
No retained packet sets full-update flag bit 2, and all 20 captured players have MonsterData 0. Thus effective-record
selection leaves this survey's IDs unchanged. Forced record **99902** itself exists and selects CAT **99894**, also present
(`SELECT version,data FROM rdb_1040023 WHERE id=99902`; `SELECT 1 FROM rdb_1010002 WHERE id=99894`).


IDs: `17655,22794,26080,26088,26090,30252,30365,45873,165178,165179,165180,165181,165182,165185,165186,165187,165191,165192,165193,165194,165195,165196,165212,204067,204985,220406,247041,251782,254118`.

All **1,360** installed NPC records parsed; the two zero-Mesh records are **163119** `molokh` and **205481**
`lctower_supplymasters_control_tower`. Neither occurs in a retained capture; these are **present** records retaining the
dynel's own Mesh, not missing-record humanoid fallbacks. The leading stat vectors of all **119,540** item templates parsed
with zero errors and **zero stat-359 (MonsterData) references**; no captured StatIIR sets stat 359. This database is a
resource collection, not a list of every server NPC appearance, so it cannot bound uncaptured missing IDs.

Method: network big-endian SimpleCharFullUpdate decode through texture/cloth/attractor fields, using the existing
`ao-net` layout; little-endian NPC/item leading stat-vector decode using `npc.rs` / `dynel_visual.rs`. SQLite opened with
`mode=ro`; queries `SELECT id,data FROM rdb_1040023`, `SELECT id,data FROM rdb_1000020`, and
`SELECT 1 FROM <table> WHERE id=?` over `sqlite_master`'s 56 tables. No game data was copied.

The same fresh presence survey found all **93** positive wire texture IDs and **33** positive wire attractor IDs present;
NPC-only subsets are **90** textures / **29** attractors, with all **20** selected NPC body CAT meshes present. All **241**
cloth entries use page 0. This does not reproduce the unidentified pink character, but excludes absent positive wire assets
in these captures. The prior decoded-texel/part-table survey remains in `dynel.md` §ICC pink-appearance investigation:
four missing CAT image references belong to piranha/unicorn/mech/hoverbike models, not ordinary player bodies; only eight
magenta-filter texels occur in head image 40268, none in captured body images. Missing GPU texture bindings are white,
not magenta, and untextured mesh 9918's authored pink diffuse is intentionally not used by the retail material path.
No colour substitution or invented pink fallback is warranted; the uncaptured forearm/side report remains unidentified.


## 8. Unresolved (addresses searched)

* Marker pairs (`0x0f,0x17` …), the 13 trailing bytes, the fourth list: Gamecode `FUN_1004d919` ignores them; no other reader exists (`0xfde97` occurs only in `FUN_1004dc30`, searched all DLLs for the 4-byte constant and the decimal 1040023: only Gamecode 0x1004dc4c).
* Consumer of the record's other stats (Flags 0x4000, VolumeMass, Features, 0xc5, 0x165, CharRadius): the only direct readers found are `FUN_1004d8e6(stat)` callers `FUN_10058078`, `FUN_1006fb56`, `FUN_1009b4ac`; likely folded into the dynel's stat object (vtable `+0x3c` GetStat) [GUESS].
* Cloth table → textures for morphed chars; the global compared in `FUN_10070247`; sound key names; the meaning of `Part::alpha_aux` (see AlphaMode).
* `AbstractAnimID_e` enumerator names (no symbols; derived from clip names above); the effect-driven `srand` calls (`ProcessStuff`) that reseed the real `rand()` stream; the exact call moment of `FUN_1011bb4a`'s `srand(time)` ([GUESS]: zone start).
