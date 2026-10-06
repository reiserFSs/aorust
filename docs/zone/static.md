# Non-character dynels: where their look comes from

Corpses, vending machines, weapon items, doors, terminals and the other `n3Dynel_t` subclasses that
are not `SimpleChar_t`. Code: `crates/ao-formats/src/dynel_visual.rs` (pure resolver), decoder
`ao_net::n3::world::Corpse`; tests `dynel_visual::tests`, `tests/dynel_visual_real.rs` (real rdb, skips
without the client), `n3::world::tests::corpses`. Addresses: `GC` = Gamecode.dll, `N3` = N3.dll, `GD` =
GameData.dll, `DS` = DisplaySystem.dll (PE base 0x10000000). Stat names come from
`crates/ao-formats/data/stat_names.txt` (the client's own table).

## 1. The class family

`CreateDynel` [N3 0x10003f80] calls the ribosome's slot `+0xc` to allocate the body. The three messages
of the capture allocate (ribosome vftables GC 0x10166a40 / 0x10166df0 / 0x10166e40):

| message | kind | ribosome `+0xc` | class | message vftable | ctor |
|---|---|---|---|---|---|
| `CorpseFullUpdateIIR_t` | 0xC76A | `FUN_1009f3c0` → `FUN_1007e652` | `Corpse_t` (vftable 0x101622d4) | 0x10166a5c | `FUN_1009f46c` |
| `VendingMachineFullUpdateIIR_t` | 0xC75B | `FUN_100a25b9` → `FUN_100998a1` | `VendingMachine_t` (0x10166274) | 0x10166e0c | `FUN_100a2590` |
| `WeaponItemFullUpdateIIR_t` | 0xC74A | `FUN_100a2705` → `FUN_1009c0ca` | `WeaponItem_t` (0x101665ec) | 0x10166e5c | `FUN_100a26bf` |

`Corpse_t` and `VendingMachine_t` derive from `Chest_t` (`FUN_1007e36e`), which derives from the
`SimpleItem_t` family. Every one of these vftables has the same **visual initialiser at slot `+0x7c` =
`FUN_10086f20`** (plus `+0x84` `SetMesh(id)` = `FUN_10086ea2`, `+0x8c` `SetCatMesh(id)` = `FUN_100871d3`):
AccessCard, CentralController, Chest, Door, LockableItem, Mine, PlayerShop, QuestBooth, ReclaimBooth,
SimpleItem, TrapItem, VendingMachine, WeaponItem, WearableItem, CityTerminal (found with a vtable-slot
search over Gamecode). `Corpse_t` overrides `+0x7c` (`FUN_1007e7e2`), `+0x50`, `+0x58`, `+0xac`, `+0xb4`.

The registered `*FullUpdate` classes (`docs/zone/outgoing.md` registry) in this family: Corpse, Vending
Machine, WeaponItem, Door (decoded; the door's messages and animation: docs/zone/doors.md) and `SimpleItem`, `Chest`, `TrapItem`, `Mine`, `CentralController`
`FullUpdate` (not decoded: they share the `DynelBase` chain and the apply path below, so their stat list
is all the look needs).

## 2. The shared apply path (`FUN_100a15f8` [GC 0x100a15f8])

Called after every `*FullUpdate` creates its dynel (`FUN_100a0698` ← corpse `FUN_1009f2b3`; `FUN_100a08c8`;
`FUN_100a246c`; weapon `FUN_100a2773`):

1. `StaticInstance` := the message's stat 23 (`FUN_100a15f8` scans the pair list for id 0x17).
2. `SetStat(InventoryId 0x37, byte_70)`, `SetStat(CurrBodyLocation 0xdc, byte_71)`.
3. `FUN_10088d28({0xF4254, StaticInstance})` stores stat 0x19 = 0xF4254 and 0x17 = StaticInstance.
4. If StaticInstance != 0: **`FUN_10080169`** loads rdb **1000020** record `{0xF4254, StaticInstance}`
   (`ResourceDatabase_t::GetBinaryStream`), reads it into the dynel (vtable `+0x70`) and flips negative
   `0x126`/`0xd2`/`0xd3` (damage stats) positive. This is the record's stat list.
5. Every message stat pair `(id, v)` is applied on top (`HasStat`/`AddStat`/`SetStat`): **message overrides
   template**.
6. `ACGItemLevel 0x2bd` → `Level 0x36`; stat 0x1c2 selects a state machine (`StateMachine_t`).
7. If `CATMesh (0x2a)` is set: `vtable+0x8c` = `SetCatMesh(CATMesh, CanChangeClothes > 0)`.
8. `vtable+0x7c` = the visual initialiser (`FUN_10086f20`, or the corpse's `FUN_1007e7e2`).

`FUN_10086f20` (item family; `crate::dynel_visual::visual`):

| step | rule | record |
|---|---|---|
| default | `Mesh == 0 && CATMesh == 0` ⇒ `Mesh :=` name lookup `pickupbox_misc.abiff` (`InstanceManager_t::GetTypeInstance(0xf6951, name)`) | rdb 1010001, **id 9013** |
| mesh | `Mesh != 0` ⇒ `n3VisualDynel_t::SetMesh(Mesh, kind != 0xC748)` [N3 0x10019e8f]: `VisualMesh_t::SetMesh({0xF6951, id}, 0x50, …)` | rdb **1010001** `*.abiff` |
| cat mesh | `CATMesh > 0` ⇒ `vtable+0x8c` ⇒ `SetCatMesh` [N3 0x10019fb2]: `VisualCATMesh_t::SetMesh({0xF6952, id}, textureDataList)` | rdb **1010002** `*.cir` |
| texture | stat 336 ≠ 0 ⇒ `SetOverrideTexture(id)` | rdb 1010004 |
| visibility | `Flags (stat 0) & 1` clear ⇒ `DisableVisibility()` | |
| scale | `MonsterScale (360)` present ⇒ `SetBodyScale(v / 100.0)` (`DAT_10158670` = 100.0) | |

Ground truth for the pickup box: weapon records (§4) all carry `Mesh = 9013 = pickupbox_misc.abiff`.
The unresolved `bool` of `SetMesh` (`kind != 0xC748`, i.e. "not a door") is ignored by the SetMesh body that was
decompiled (it only builds `{0xf6951,id}`).

## 3. Item/dynel template records: rdb 1000020 (`0xF4254`)

Key = `StaticInstance` (stat 23). 119 540 records (21601…500111). Also the key of `n3ObjectFactory_t::CreateFromTemplate`
[N3 0x1000bebb] (`CreateRibosome` [N3 0x1000bd48] reads the first `u32` = kind, registry lookup, ctor).

```
u32 kind                      ; 0xC74A weapon, 0xC75B vending, 0xC74E, 0xC73D, ...  (dynel_class)
u32 element count
elements: { u32 type; u32 sub; payload }       ; FUN_1002b297 [GC] (switch on type/sub)
  {0x0F,0x17}  stats:  u32 size word (n+1)*0x3F1, n × (u32 stat, i32 value)     <- ALWAYS the first element (119540/119540)
  {0x15,0x21}  names:  u16 name len, u16 description len, name, description
  {0x02,*}     SpellData_t list (FUN_100a6c58)        {0x04,4} {0x14,0x13} {0x16,0x24} {0x17,0x25} ... (not needed)
```

Example (rdb 1000020:248371, 271 bytes, the captured vending machine): `5b c7 00 00 | 03 00 00 00 | 0f 00 00 00 17 00
00 00 | 5b 2b 00 00` = kind 0xC75B, 3 elements, stat list of 10 pairs: ItemClass 0, Can 8, Flags 0x80023203, Placement 0,
DefaultPos 0, **Mesh 93117**, Level 1, BuyModifier 4, SellModifier 105, AnimPlay 2; name "Newcomer's Nano Programs".

Real-data survey (`tests/dynel_visual_real.rs`): all 119 540 records parse; 107 607 have `Mesh`, every one is in rdb 1010001;
none has `CATMesh`; 10 922 have `WeaponMesh (209)`, 3 of those name a mesh missing from 1010001 (client data, not a parse error).

### Kinds (`dynel_class`)

Ribosome templates registered with `RegisterTemplateCtor` (GC 0x101528ad…0x10152a08, factories 0x10116960…0x10116ad3):

| kinds | ribosome |
|---|---|
| 0xC75B | `VendingMachineRibosome_t` |
| 0xC790 | `PlayerShopRibosome_t` |
| 0xC748, 0xDAC6, 0xC73A | `DoorRibosome_t` |
| 0xC73D, 0xC758, 0xC77E, 0xC77F, 0xC757, 0xC75C, 0xC76F, 0xC353, 0xC76D, 0xC76E | `SimpleItemRibosome_t` |
| 0xC418 | `CityTerminalRibosome_t` |
| 0xDAC1 | `QuestBoothRibosome_t` |
| 0xC773 | `ReclaimBoothRibosome_t` |
| 0xC742 | `CentralControllerRibosome_t` |

0xC74A (weapon, message check `FUN_100a27a5`) and 0xC76A (corpse, `GetItemByTemplate`/`FUN_1009f5a4`) are message-created.
Kinds 0xC73C, 0xC749, 0xC770, 0xC74E (59 482 records, the bulk of rdb 1000020; what they are was not traced) have a
`DbObject` registration (0x10153130) but **no ribosome**, so `CreateFromTemplate` makes nothing for them.

## 4. Weapon items (`WeaponItemFullUpdate`, kind 0xC74A)

* `parent != 0` (always in the capture: held by an NPC/player): `FUN_100a1216` [GC] hides the dynel (`DisableVisibility`) and
  attaches it to the parent (`vtable+0x24`); for a `SimpleChar_t` parent `FUN_10047873` equips it by `CurrBodyLocation`
  (message `byte_71`, stat 0xdc): slot 6 or 8 (and 0x3f/0x3d) are the weapon slots. The dynel never draws its own mesh.
* The mesh a holder shows comes from the item template: record rdb 1000020 `[StaticInstance]` → stat **`WeaponMesh` (209)** →
  rdb 1010001. This is exactly the mesh in the holder's `AppearanceUpdate` attractor (`docs/zone/dynel.md` §1.3):

| capture | `StaticInstance` | record | `WeaponMesh` | name table | attractor in the holder's update |
|---|---|---|---|---|---|
| ICC Shuttle Guards, 8 items | 0x40b82 = 265090 | "Ofab Shark Mk 5" (0xC74A) | 262556 | `EP3_assault_rifle_03.abiff` | `(1, 262556, 0, 2)` |
| player Stanko, 2 items | 0x3ca19 = 248345 | "Polished Eliminator" (0xC74A) | 7796 | `weapon_shotgunsmall01.abiff` | `(1, 7796, 0, 2)` ×2 |

* Attractor place: `byte_71` 6 → place 1 (`Attractor02_righthand`), 8 → place 2 (`Attractor03_lefthand`)
  (`weapon_attractor_place`). **[INFERENCE]**: both captured players show places 1/2 for the slots 6/8 pair; the enum was not traced.
* Lying on the ground the dynel shows `Mesh` = 9013, `pickupbox_misc.abiff` (§2). The record's `AnimSet` (3 for the rifle, 0 for the shotgun) is presumably the holder's animation set: **[GUESS]**, not traced.
* `template` identity `{1000015, 0}` (`DynelBase.template`, +0x4C) is in every capture and is **not used** by this path (its
  instance is 0; rdb 1000015 has 7 records with unrelated ids). **[UNRESOLVED]**.

## 5. Corpses (`CorpseFullUpdate`, kind 0xC76A)

Wire (`FUN_1009f502`): `u32 8`, `DynelBase`, spell list (`FUN_100a6c58`), owner `Identity` (+0x84), `ClothData_t` vector (+0x8c),
`i32 n; if n != 0 TextureData_t vector` (+0x90). Decoded by `Corpse` (all 7 capture corpses to the last byte).

Apply: `FUN_1009f41d` [GC, message slot 9]: cloth vector → corpse +0x1e4 (`FUN_1007eaa4`), `SetTextureDataList(textures)` if non-empty,
the shared apply path of §2 (stat 0x2a `CATMesh` ⇒ `SetCatMesh`, which passes the texture list to `VisualCATMesh_t::SetMesh`),
`DisableVisibility`; placement/visibility by `FUN_100a1216`. Then the **`Corpse_t` initialiser `FUN_1007e7e2`** (`corpse_visual`):

| input | use | record |
|---|---|---|
| stat `CATMesh` 42 | the model: the dead character's own CAT model | rdb **1010002** (`junkbot.cir`, `cutecreature.cir`, `giant_snake.cir`) |
| `ClothData_t` entries (part, +4, +8) | `SetCATTexture(ClothData_t::GetName(part), +4, layer 2)`; if `+8 != 0` also layer 3 | rdb 1010004 |
| `TextureData_t` list (name[32], id) | material retexture at `SetMesh` | rdb 1010004 |
| stat `HeadMesh` 64 ≠ 0 | `SetSkinData(Breed 4, Sex 59, Race 89)` + `AddAttractorMesh(place 0, HeadMesh)` | rdb 1010001 |
| stat `MonsterScale` 360 | `SetBodyScale(v/100)`; constructor `FUN_1007e652` pre-sets 100 | |

The constructor also pre-sets `CanChangeClothes 223 = 1`, `HeadMesh 64 = -1`, `DeadTimer 34 = 0x3c`, stats 0x19f/0x1a0/0x3d = 0
(`FUN_1007e652`). The message stats (every capture): `Flags 0x181805`, `StaticInstance 0`, `CorpseType 415 = 50000`, `CorpseInstance 416` =
owner id (equal to the `owner` identity: asserted for all 7), `CATMesh`, `MonsterScale`, `Breed 4 = 6`, `Sex 59 = 1`, `Race 89 = 1`,
`Cash 61`, `DeadTimer 34 = 600`, `TimeExist 8 = 18000/180000`, `CanChangeClothes 223 = 0`. There is **no `Mesh` stat and no template**
(StaticInstance 0), so only `CATMesh` selects the model and the pickup box is not used.

Captured (`docs/captures/zone_ithaca.rec`, playfield 4582):

| header `{0xC76A, id}` | name blob | owner | CATMesh → name table | MonsterScale | spell id |
|---|---|---|---|---|---|
| 5663 | Remains of Cross-Wired Junkbot | {0xC350, 1025286} | 45857 `junkbot.cir` | 92 | 0x1238 |
| 5628 | Remains of Pwnsfoot the Jolly Hunter | 1024636 | 15222 `cutecreature.cir` | 181 | 0x1215 |
| 5191 | Remains of Scuttleflutter of the Phat Loot Exterminator | 1022984 | 15222 `cutecreature.cir` | 181 | 0x1060 |
| 5634 | Remains of Beach Leet | 1019892 | 15222 `cutecreature.cir` | 90 | 0x121b |
| 5636 | Remains of Beach Leet | 1019907 | 15222 `cutecreature.cir` | 90 | 0x121d |
| 5163 | Remains of Poisontongue of the Junglescale | 1022941 | 23353 `giant_snake.cir` | 181 | 0x1044 |
| 5691 | Remains of Cross-Wired Junkbot | 1026282 | 45857 `junkbot.cir` | 93 | 0x1254 |

All 7: 5 empty cloth entries (part 0..4, texture 0), no `TextureData`, a `HeadMesh = 0` stat only on the two junkbots (absent on the rest), all models exist in rdb 1010002.
The cloth/texture lists of NPC corpses are therefore empty: the model's default skin/textures show. A **player** corpse would
carry cloth textures and a head (not captured).

**Pose / animation [CODE, resolved]: none, the corpse is drawn unanimated (bind pose).** `Corpse_t`'s overrides never start a clip:
`FUN_1007e7e2` (visual init, vtable `+0x7c`) = cloth `SetCATTexture`, then (stat `HeadMesh` 0x40 != 0) `SetSkinData` +
`AddAttractorMesh(0, head, 2, 0)` (no `ClearAttractors` here), then `SetBodyScale(MonsterScale/100)`; `FUN_1007e622` / `FUN_1007e642`
(vtable `+0xc0` / `+0xc8`) set / clear the looter flag `+0x1f0` and call `FUN_1007e11e` / `FUN_1007e198` (register / unregister the looting
character in its `+0x1d0` component via `FUN_10058816` + `FUN_1003f5b2` / `FUN_1003f3b2`, dynel flag 0x80 / stat 0x62, effects 0x12 / 0x13: no
animation call); `FUN_1007e8e1` (slot `+0x6c`) = a clamped stat read (0..100000, else 100) and an `LDBformat` text feed; `+0x50` returns 2
(`FUN_1007e61c`); `FUN_1007e072` = collision flags; the ctor `FUN_1007e652` only sets stats. `VisualCATMesh_t::SetMesh` [DS 0x100728b7] only
issues the async load (`FUN_10070656`); its completion `FUN_100704b8` creates the `RCATMesh` / `CATRender_t::SetMesh` without an animation, and
`VisualCATMesh_t::SetAnimation` [DS 0x10073f23] has no caller inside DisplaySystem (export only).
The only Gamecode path that animates an item is the spell **0xCF27** (`FUN_100a59f5` case 0xcf0b / 0xcf27 / 0xcf2a -> `FUN_100a4dcc`; every
captured corpse carries one): `stat 5 != 0` targets `GetDynel(stat5, stat6)`, else the executing dynel (= the corpse); for a `SimpleItem_t`
target with `stat 0x2d == 0`: `key` = the item's stat 0x1a1 (`CorpseAnimKey`) unless it is -1, then the spell's stat 7; **only `key < 100`** runs
`FUN_10010e36(item, key, 1)` = `FUN_10010c57(key)` name rule (social ids 1..0x46) -> `VisualCATMesh_t::SetAnimation`. The captured spell
arguments `(5,6,7,0x2d,0x2f,0x30,0xb) = (1,0,0,0,0,0,0x1f7)` and the absent `CorpseAnimKey` give key 0 (no social name) or, if the 0x1f7 were
stat 7, 503 (the die animation id, but >= 100): no clip in both readings. The pose is therefore what an unanimated `CATRender` shows: the
vertices as stored (bind pose); `build_corpse` uses `ActorRig::pose(None)` (test `corpse_is_unanimated`). The earlier "last frame of the die
clip" guess is removed. **[UNRESOLVED]** a server-sent `CorpseAnimKey` 1..99 (never captured): the social clip would start (meaning of the
`SetTime(handle, 0.0, total)` arguments not traced).

### Spell record (`GameData::SpellData_t`, GC list reader `FUN_100a6c58`, GD `operator>>` 0x1000d686)

`SpellFormats_c::ReadBinary` [GD 0x1000f4a6]: `ReadBinaryHeader` [GD 0x1000eae1] = `i32 type, i32 id, i32 version` (version must equal the format's:
4), `SpellData_t::ReadBinaryCriteria` [GD 0x1000d49f] = `i32 count (≤ 1000)` + `count × Criterion_t`, then `SpellFormat_c::ReadBinary`
[GD 0x1000f39e/0x1000f95c]: one value per argument of the type's format (`BinaryToValue` [GD 0x1000f01d]: `ComplexType` 1 = `i32 len + bytes`,
anything else `i32`; `GetBaseType` GD 0x1000eac0). The formats are built in `SpellFormats_c::SpellFormats_c` [GD 0x1000fb0a]. Type 0xCF27
has 7 integer arguments (stats 5, 6, 7, 0x2d, 0x2f, 0x30, 0xb). The corpse's spell is (all 7):
`cf27 <id> 4 | 0 | 1 0 0 0 0 0 0x1f7 | 1 4 <x> 0` with `x` = 0xb331 / 0x44f7 / 0x762c for junkbot / cutecreature / giant_snake.
**[UNRESOLVED]** the last four integers (`1 4 x 0`): the decoded format has 7 arguments, `SpellData_t::operator>>` reads nothing after
them and the corpse owner identity (`{0xC350, CorpseInstance}`) provably starts after them in all 7 captures. `Corpse` therefore keeps them as
`CorpseSpell::tail` and refuses any spell type other than 0xCF27 v4 or with criteria. Item records show the same shape for other types
(`cf16 0 4 | 0 | 9 values`, `cf0a`, `cf35`), so the same gap applies to them.

## 6. Vending machines (`VendingMachineFullUpdate`, kind 0xC75B)

`VendingMachine_t` takes the item path unchanged (§2): the message stat list is `Flags 0x80023203, StaticInstance 248371, 701/702/703 = 0,
MultipleCount 412 = 1, AnimPlay 501 = 2, AnimPos 500 = 0, Mesh 12 = 93117`. Template rdb 1000020:248371 (kind 0xC75B, "Newcomer's Nano
Programs") has the same `Mesh 93117` and `AnimPlay 2`. Result: **static mesh rdb 1010001:93117 `shop_neutral_nano_general.abiff`**, scale 1, visible
(Flags bit 0), at `position`/`rotation` of the message (940.23, 47.21, 875.34; quaternion y/w = 0.7133/−0.7009). `item_visual` returns it.
`AnimPlay`/`AnimPos` (stats 501/500, stored at dynel +0x1b0 by `FUN_10088d80`) drive the item's mesh animation clock `FUN_10088426` (`AnimPlay` 2 = play forward forever; docs/zone/doors.md §4); `dynels_doors::ItemAnim` runs it for every item whose mesh has node keyframes.

## 7. Dynels the playfield creates itself (doors, terminals, shops)

`PlayfieldAnarchy_t::CreateRDBDynels` [GC 0x10121dcb] (called from `ReadPlayfield` [GC 0x10122c4c] with `{0xF425A, playfield}`) walks
`PlayfieldDynelData_t` (rdb **1000026**, key = playfield id, 601 records, 12 509 dynels): for each entry with `template != 0`:
`CreateFromTemplate(template, identity)` (§3, kind from the rdb 1000020 record), apply the entry's stat blob (`FUN_1002b297(stream, dynel, 1)`),
`AddChildDynel(position, rotation)`, `vtable+0x7c` (visual initialiser), `SetRelRot`.

```
u32 n ; n × { u32 size ; DynelData_t } ; 12 zero bytes                 (little endian; GD operator>> 0x10005fbf)
DynelData_t: i32 kind, instance (0xC0000000 | index<<16 | playfield), ?, +0xc, +8, playfield;
             f32 x y z;  f32 w qx qy qz;  i32 +0x30, template (+0x34);  i32 blob length; blob bytes
```

`blob` is a big-endian `BinaryStream`: 12 prefix bytes (`0x66 0x67 0x3F1`; the loader starts at `blob+0xC`), element count, elements as in §3.
Its `{0xF,0x18}` stat list overrides the template (`blob_stats`). Walking the blob needs the spell lists that precede it
(§5 gap), so `blob_stats` **locates the element by scanning** for its header: **[GUESS]**, validated by: exactly one valid header in each of
the 8 415 blobs that have stats, and every `Mesh` they set exists in rdb 1010001.

Playfield 4582 (the capture's zone) places two: `{0xC748 Door, 0xC00011E6}` at (936.15, 47.56, 888.38), template 41565 (record `Door`, `Mesh` 41798
`door_omni_med_blue2.abiff`, **overridden by the blob to 245910**), and `{0xC73D SimpleItem}` "Interactive Billboard" at (943.10, 48.40, 875.10), template
41560 (no `Mesh`; the blob supplies **258365**). Kinds seen over all playfields: 0xC748 4886, 0xC73D 2941, 0xDAC6 2259, 0xC75B 900, 0xC418 490,
0xDAC1 407, 0xC773 383, 0xC790 210, 0xC73C 25 (no ribosome → nothing is created), 0xC770 3 (no ribosome), 0xC758 3, 0xC76D 1, 0xC749 1 (no ribosome);
34 templates are absent from rdb 1000020 (the client creates nothing for them either: `GetBinaryStream` fails).

## 8. API (`ao_formats::dynel_visual`)

```rust
pub fn item_template(store: &RecordStore, static_instance: u32) -> Result<Option<ItemTemplate>>;
pub fn parse_item_template(rec: &[u8]) -> Result<ItemTemplate>;            // kind, stats, name
pub fn effective_stats(template: Option<&ItemTemplate>, message: &[(u32, i32)]) -> Vec<(u32, i32)>;
pub fn static_instance(message: &[(u32, i32)]) -> Option<u32>;
pub fn visual(stats: &[(u32, i32)], default_mesh: u32) -> DynelVisual;     // FUN_10086f20
pub fn default_mesh(names: &NameTable) -> Result<u32>;                     // 9013
pub fn item_visual(store: &RecordStore, names: &NameTable, message: &[(u32, i32)]) -> Result<DynelVisual>;
pub fn weapon_mesh(template: &ItemTemplate) -> Option<u32>;                // rdb 1010001
pub fn weapon_attractor_place(body_location: i32) -> Option<u8>;           // 6 -> 1, 8 -> 2
pub fn corpse_visual(stats: &[(u32, i32)], cloth: &[(i32, i32, i32)], textures: &[(&str, i32)]) -> Result<CorpseVisual>;
pub fn placed_dynels(store: &RecordStore, playfield: u32) -> Result<Vec<PlacedDynel>>;
pub fn placed_visual(store: &RecordStore, names: &NameTable, d: &PlacedDynel) -> Result<Option<DynelVisual>>;
pub fn blob_stats(blob: &[u8]) -> Result<Vec<(u32, i32)>>;
pub fn dynel_class(kind: u32) -> Option<&'static str>;
```

`DynelVisual { mesh: Option<u32> /*1010001*/, cat_mesh: Option<u32> /*1010002*/, can_change_clothes, override_texture, scale, visible }`;
`CorpseVisual { cat_mesh, scale, head_mesh, breed, sex, race, cloth: Vec<ClothLayer{part,texture,texture2}>, textures: Vec<(String,u32)> }`.
Render with `ao_formats::mesh::decode_mesh_into(store, mesh, &mut scene)` (static) or `ActorRig` (CAT).

## Unresolved (explicit)

* Vending machine `AnimPlay`/`AnimPos` (§6). Weapon attractor place enum (§4, inferred). A server-sent `CorpseAnimKey` < 100 (§5).
* The four trailing integers of a 0xCF27 spell (§5); spells of other types are not decoded (`Corpse::read` errors on them, `parse_item_template` never reads them).
* Full walk of placed-dynel blobs (§7, scan instead). Position/rotation of placed dynels are as stored (Y up); the client's `AddChildDynel` transform to the
  scene axes was not checked against a render.
* `DynelBase.template` `{1000015, 0}` (§4). Stat 336 meaning (only "texture id for `SetOverrideTexture`").
