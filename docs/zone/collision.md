# Ground following and collision (N3.dll surfaces, rdb 1000013)

Port of the data and queries behind `ao_formats::playfield::collision` (`crates/ao-formats/src/playfield/collision.rs`,
`collision/kd.rs`). Everything below was read from the client DLLs (Ghidra, N3.dll image base `0x10000000`) and checked
against the shipped data. Labels: **[CODE]** read in the decompile, **[DATA]** verified on the files, **[GUESS]** not resolved.

## 1. What the client collides with

The playfield owns one `Surface_i` (`n3Playfield_t::InitializeSurface` N3 @0x1000c64b, stored at `playfield+0x60`):

| playfield | surface | closest-point / ground query | note |
|---|---|---|---|
| outdoor (`n3Tilemap_t::IsGround`) | `n3TilemapSurface_t` (`Initialize` @0x1001879d, owns a `CellSurface_t` grid of the zones, `0x30` bytes) | `CalculateClosestPoint` @0x10018f0c | heightfield + KD surface of the zone cell |
| dungeon | `n3RoomSurface_t` (`SetPlayfield` @0x10013e10) | `CalculateClosestPoint` @0x10013ee6 | room tile triangles + KD surface of the room |

`Collision.dll` (113 KB) only holds the generic geometry (`CellSurface_t`, `CollPrim_t` spheres/boxes/cylinders/lines,
`Face_t`, `VolumeMesh_t`, `KDSurfaceBuilder_t::WriteBlobV5`); **PathFinder.dll** is NPC route planning (`GraphPathFinder_t`,
A*) and is not involved in the player's ground following. The KD surface class `KDTreeSurface_c` lives in N3.dll (vtable
@0x1004134c: `[1]` CalculateClosestPoint @0x1002e48e, `[4]` GetLineIntersection @0x1002e4fa, `[6]` GetSphereIntersection
@0x1002e587, `[8]` VetoPosition @0x1002e640) and is a base class of `n3SurfaceResource_t` (rdb record, base at `+0x18`).

### 1.1 Zone collision records (rdb 1000013)

`n3Zone_t::LoadSurface` (@0x1001a947) acquires `Identity_t{type 0xf424d = 1000013, id = zone->id}` and hands the
`KDTreeSurface_c` (`+0x18` of the record) to `CellSurface_t::SetSurfaceForCell`. The zone id is **`playfield << 16 | index`**:
outdoor zones in `RDBPlayfield_t::ReadBlob` @0x1001c115 (`pnVar7[1] = playfield << 0x10 | index`), dungeon rooms in the room
reader @0x10012803 (`this+4 = playfield << 16 | this+0xc`). **[DATA]** the table has 234 349 records (ids `0x00640003`...),
439 playfields in version 5, 166 in version 4 (the `version` column of the sqlite table; the new accessor
`RecordStore::get_versioned`). Centroids of the decoded volumes of Newland City (566) lie inside their zone rectangle (+-20 m)
for 1368 of 1377 volumes: the volumes are in **AO world coordinates**.

Dungeon rooms: `n3Room_t::CalcTemplatePosFromGlobalPos` (@0x115ab) = `Ry(rot - trot) (g - pos) + pos0` with `trot == rot` after
loading (reader sets `+0x40 = +0x44 = flags & 3`), i.e. **template space = world space** for the static rooms; the surface of
room `i` is the record `playfield << 16 | i` in world coordinates (4604: records 0..2 for its 3 rooms).

## 2. Record format (`n3SurfaceResource_t::ReadBlob` @0x10016179)

Dispatch on the version column (`DbObject_t` +0x10): 1 → plain `FUN_1002e93e`, 3 → `ReadVersion3` @0x10015a21, 4 →
`ReadVersion4` @0x10015e17, 5 → `ReadVersion5` @0x10015fc1. Only 4 and 5 occur in the shipped data (4: 4127 records, 5: 230 222).

### 2.1 Version 4 (zlib + `KDTreeFile`)
`u32 inflated size (<= 4 000 000)` + zlib stream (`FUN_1002a180`). Inflated: `u32 a; if a { KDTreeFile }; u32 b; ...`.
In the data **[DATA]** `a = 0, b = 0x80` and a single tree follows; the decompile reads the first tree when `a != 0`, then
`b`, then (`b != 0`, `a != 0`) a second tree into the *invisible surface* (`+0x2c`, `GetInvisibleSurface` @0x10015b9c),
and for `a == 0 && b != 0` reads the one tree into the main surface. We merge both trees' volumes as ordinary geometry
(**[GUESS]** the invisible surface only blocks movement, `n3SurfaceResource_t::EnableInvisCheck` @0x1000ac13; no record of the
shipped data has `a != 0`, not verified). After the tree(s): `u32 portal flag; PortalArea (FUN_1001b3e6) + 2 x u32`.

`KDTreeFile` (`FUN_1002e93e` @0x1002e93e): NUL-terminated `"KDTreeFile"`, `u32 volumes, u32 nodes, u32 version (=3), u32
vertices`; `f32 x0`: **-99999.0** (`_DAT_100413d4`) marks raw `f32` vertices (three arrays x[], y[], z[]), otherwise `x0, y0, z0`
are the bases of `u16` vertices (arrays x[], y[], z[], `value * 0.01 + base`, `_DAT_1003d628`); per volume `u32 triangles
(1..=999), f32 min[3], f32 max[3], triangles*3` indices (`u8` if vertices < 256, `u16` < 65536, else `u32`; into the shared
vertex array, `FUN_1002fda1` compacts the used vertices); then the BSP: `f32 split, u32 axis, u32 count` per node, `count == 0`
= inner node (left child follows immediately, then the right subtree), else a leaf with `count` volume indices (`u8`/`u16`/`u32`
by volume count). **[DATA]** sample record 296486101: 1 volume, 5 vertices, 6 triangles (unit test `kd_version4_record`).

### 2.2 Version 5 (range coded)
The record is one range-coded stream (`FUN_1002f610` init: `range = 1, code = 0`; `FUN_1002f952` frequency decode,
`FUN_1002f7b6` binary decode). **Decoder (exact port, `kd.rs::Rc`)**: normalise `while range < 2^24 { range <<= 8; code =
code << 8 | next byte (0 past the end) }`; `freq(n)`: `q = code*n/range; up = (q+1)*range/n; if up <= code { up =
(q+2)*range/n; q += 1 }; lo = q*range/n; code -= lo; range = up - lo` (64 bit products); `bit(a, b)`: `thr = a*range/(a+b)`,
`code < thr` → `range = thr`, result 0, else `range -= thr; code -= thr`, result 1 (disassembly of @0x1002f7b6: `CMP; SBB AL,AL;
INC AL`). Stream (`FUN_1002f1b0` @0x1002f1b0, `ReadVersion5`):

1. `freq(15)` (ignored), `T.step = _DAT_1003d618 = 0.01` m.
2. counts: `volumes = bit(1,1)==0 ? freq(100) : freq(1000000)+100`, then the same for the BSP `nodes`. 0 volumes = empty record.
3. bounds: six `freq(0x1000000) - 0x800000` (centimetres): `min = (b0,b1,b2)`, `max = (b3,b4,b5)`.
4. per volume (`FUN_1003517f` @0x1003517f, a connectivity-coded triangle mesh):
   * cell origin `o[a] = min[a] + freq(max[a]-min[a]+1)`, cell size `s[a] = 1 + freq(max[a]-o[a]+1)` (`FUN_10035acc`); the
     volume's box is `(o+0.5)*0.01 .. (o+s-1+0.5)*0.01` (`+0.5 = _DAT_1003c868`).
   * `sparse = bit(3,2)`. Seed triangle with three new vertices (`FUN_1003680f`); each pushes its edge `(v[k], v[k+1])` on a FIFO
     of open edges. Vertex position (`FUN_100363b5`) per axis: `bit(0x44,0x11)==0 ? 1+freq(s-2) : (bit(3,3)==0 ? s-1 : 0)`, `+ o`.
   * loop: pop `e = (a, b)`; if `sparse && bit(2,6)==0` the edge stays open (dropped). `prev` = first queued edge with
     second `== a`, `next` = first queued edge with first `== b`. With a neighbour: `c1/c2 = FUN_10036319` (0 if absent,
     else the sign of the **32 bit wrapping** integer dot product `(V[B.b]-V[A.b]) . (V[A.a]-V[A.b])`: 0x61 if 0, 0xa7 if > 0, 0x31 if
     < 0), `bit(200, c1+c2) != 0` → connect: if `prev.a != next.b` (or one missing): `bit(c2, c1)==0` → triangle `(e.b, e.a,
     next.b)` and `next := (e.a, next.b)`, else triangle `(prev.b, prev.a, e.b)` and `prev := (prev.a, e.b)`; if `prev.a ==
     next.b` the corner closes: triangle `(prev.b, prev.a, e.b)`, both neighbours removed. Otherwise (no neighbour or
     bit 0): `FUN_10036895`: apex `v` = existing vertex (`bit(1,15)==0`, `freq(vertex count)`) or a new vertex with a coded
     position; triangle `(v, e.b, e.a)`, push `(e.a, v)` then `(v, e.b)`.
   * queue empty: `bit(0x18,1)==0` ends the volume (else another seed component), then `bit(1,999)==1` = non-empty result.
     Vertices are `(q + 0.5) * 0.01`.
   * the stored `0x38` byte face of the client is `{triangles, vertices, index block, box}` (`FUN_1002ffcd`).
5. `freq(255) == 0x5a` marker, then the BSP (`FUN_1002eefc` @0x1002eefc): `bit(1,1)==0` inner node: `axis = freq(3)`, `split =
   min[axis] + freq(max[axis]-min[axis]+1)` (plane at `(split+0.5)*0.01`), left child = region with `max[axis] = split`, right = `min[axis] =
   split`; else leaf: `n = freq(volumes+1)`, then `n` volume indices coded from alternating ends of the remaining interval
   (`k` even: `v = lo + freq(hi-lo+1); hi = v-1`, odd: `lo = v+1`). The node count must equal the announced one.
6. optional portal: `bit(7,1) != 0`: `freq(10)`, `n = freq(1000)` points of three `freq(0x1000000)-0x800000` (x 0.01 m) (`FUN_1001b473`
   @0x1001b473), `dest = freq(65536) << 16 | freq(65536)` (`+0x40`, `GetTeleportDestinationPlayfield` = low 16 bits); finally `freq(254) == 0x79`.

**[DATA] verification**: all 234 349 records decode with every marker (0x5a, node count, 0x79) and the volume box checks
(`collision_survey_all_records_decode`, 41 617 313 triangles; largest record 4165 volumes / 8051 nodes, largest volume 901
triangles / 486 vertices). The 32 bit wrap of the dot product matters: with 64 bit arithmetic 179 records of the 505 zone row
desynchronise (cells wider than ~460 m overflow).

## 3. Ground (what `CalculateClosestPoint` returns)

### 3.1 Outdoor, `n3TilemapSurface_t::CalculateClosestPoint` @0x10018f0c
1. terrain height `h` and normal from `FUN_10017800` @0x10017800 (see below); outside the map the position is clamped by
   `VetoPosition` @0x10018a7c (`0 <= x < width*cell`, `0 <= z < height*cell`, `0 <= y < 2000`).
2. If the position is above the terrain (`pos.y - h >= 0`) the KD surface of the zone cell (`GetSurfaceForCell`, first
   non-null) is asked with a downward ray of length `pos.y - h + 0.3` (`_DAT_1003d8b8` f64 0.3; `FUN_1002e148` @0x1002e148
   called with `param_3 = -1.0`, `param_4 = length`): the nearest volume triangle under the point; its hit replaces the terrain
   when it is higher (`DAT_1005c7c0 <= h` test).
3. liquid: `n3Playfield_t::GetLiquidSurfaceHeight` @0x1000cb1e; if `closest.y < level - 1.2` (`_DAT_1003d370` f64 1.2) then
   `closest.y = level - 1.2` (the character cannot stand deeper than 1.2 m below the surface).

**Terrain triangulation** (`FUN_10017800` @0x10017800 + `FUN_1001769d` @0x1001769d): the cell `(x, z)` has corners `P0 (x,z)`,
`P1 (x+1,z)`, `P2 (x+1,z+1)`, `P3 (x,z+1)` at the heights of `FUN_10017c3e` (`u16 * scale`, indices clamped to the map);
the cell is split by the parity `(~z ^ x) & 1`: **1** → diagonal `P0-P2`: `dx <= dz` → `(P0,P2,P3)` else `(P0,P1,P2)`;
**0** → diagonal `P1-P3`: `dx + dz > cell` → `(P1,P2,P3)` else `(P0,P1,P3)`. The height is the plane of that triangle
(normal = cross product, flipped up). Note: this differs from the **render mesh**, which uses bit 14 of the tile word
(`diagonal_p10_p01`); the collision uses the parity, so on steep cells the two surfaces differ by up to a diagonal's error
(in 4582 the difference is 0 on flat ground and up to ~2.3 m next to cliffs). `Terrain::at` in `collision.rs`.

### 3.2 Dungeon, `n3RoomSurface_t::CalculateClosestPoint` @0x10013ee6
`PosToRoom` (@0x1000c8aa, first room for which `n3Room_t::IsPosInside` @0x10011664 holds; ported in `zone.rs::room_contains`).
Tile floor: atlas tile `(tx, tz)` (tile = 2 m; `rect.x1 + floor(local/2)`), corners
`A = (tx*2, h(tx-1,tz-1))`, `B = (tx*2+2, h(tx,tz-1))`, `C = (tx*2+2, h(tx,tz))`, `D = (tx*2, h(tx-1,tz))` with
`h = DHGA * height_scale - room.min_floor` (indices clamped at 0; `FUN_10016454` @0x10016454) + the room's `pos.y`;
triangles `(A,C,D)` for `dx <= dz` else `(A,B,C)` (diagonal A-C; the alternative diagonal needs bit 14 `0x4000` of the tile word,
which the 8 bit `DCGA` layer never has **[GUESS]**). The ray is cast from `y + 100` (`_DAT_1003d5a0` f64 100.0) straight down onto
that plane; the KD surface of the room (`piVar1 = room+8`) is asked with a 1 m downward ray (`FUN_1002e48e`: `param_3[1] = 1.0`)
and its hit wins when it is higher (`local_c <= local_a0`); finally `y` is clamped to `[0.001, 1999.9]`.
World placement of an atlas point: `pos + Ry(rot) (atlas - rect.origin*2 - (W'+1, H'+1))`, `y + pos.y`
(`CalcGlobalFromLocalPos` @0x109b6, `W' = ((x2-x1-1) & !1) + 1`; same frame as `room_contains`). **[DATA]** In 4604 the KD volumes
of the hall contain no floor at the spawn (rays hit only the hull top at y 54 and the ceiling at 9.56): the floor is the tile surface.

### 3.3 Dungeon walls = room membership
`n3RoomSurface_t::VetoPosition` @0x10015587 → `VetoRoomTransition` @0x1001462f: a position outside every room (`PosToRoom == null`:
outside the room rectangle, on an empty `DCGA` tile, or outside the tile's height window) is rejected and the dynel returns to its
last allowed position (`n3Dynel_t::GetLastAllowedGlobalPositionInZone`). Moving from one room to another additionally asks
the playfield (`playfield vtable +0x38`, the door rules) **[GUESS: not ported, every transition is allowed]**. These tile boundaries
(plus the KD volumes of statels in the rooms) are the walls; `Collision::inside`.

## 4. API (`ao_formats::playfield::collision`, scene coordinates)

* `Collision::load(&RecordStore, playfield) -> Result<Collision>`: heightfield (outdoor) or room tile floors + room list
  (dungeon), the KD volumes of every zone/room, the liquid polygons of the record tail. 4604: 3 ms, 4582: 41 ms, 566: 8 ms.
* `Collision::from_scene(&Scene)`: **fallback** (documented deviation): only the identity-placed meshes (terrain, room shells), no statels.
* `support(p) -> Option<Support{y, normal}>` / `ground(p) -> Option<f32>`: highest walkable surface (`normal.y >= 0.5`) at or
  below `p.y` (+1 mm) under `(p.x, p.z)`: heightfield (also when `p` is under it: the client clamps up to the terrain) and every walkable
  KD / tile triangle. The caller lifts the ray start (the client starts 0.4 m above the old position, `RAY_LIFT`).
  200k queries take 35 ms (grid of 4 m cells, big triangles in an always-tested list).
* `wading_ground(p)`: `max(ground, liquid level - 1.2)`; `liquid_at(p) -> Option<Liquid{level, kind}>`: highest liquid polygon above
  `p`. Outdoor polygons only (room liquids of the dungeon record are skipped by `record.rs`; **[GUESS/gap]**).
* `slide(from, to, radius, height) -> [f32; 3]`: horizontal move of a vertical capsule: sub-steps of one radius, each pushed out
  of wall triangles (`|normal.y| < 0.5`) by spheres from `STEP_HEIGHT` above the feet to the head, per sub-step `can_stand`
  (never into a room-less dungeon position, never onto ground steeper than `MIN_FLOOR_NY` that climbs), falling back to single-axis
  moves. `to.y` is kept. `sphere_hit(centre, radius)` is `GetSphereIntersection`'s contract (contact point, normal, depth).
* `inside(p)`: dungeon room membership (always true outdoors), `triangle_count()`.
* `kd::parse(version, bytes) -> Surface{volumes, nodes, portal, portal_dest}`: the decoder of section 2; `Surface::portal` /
  `portal_dest` are the teleport portal polygon and destination bits (playfield-change data for the world flow, not used here).

Constants (`pub const`): `MIN_FLOOR_NY = 0.5` (Vehicle.dll f32 @0x10012134, `Avatar.Movement` RE: below it ground is non-walkable),
`STEP_HEIGHT = 0.48` (Vehicle.dll f32 @0x100127e0; the client adds `2 * step length` while walking: the caller does),
`RAY_LIFT = 0.4` (Vehicle.dll f64 @0x100127f8), `WADE_DEPTH = 1.2` (N3 f64 @0x1003d370).

## 5. Deviations / unresolved

* **`slide` is not the client's algorithm.** The client resolves movement in `Vehicle_t` (Vehicle.dll `EnsureSurfaceAlignment`
  @0x1000d1aa) with the body collision sphere (`n3Dynel_t::GetBodyCollSphereRadi`, per dynel `CollPrim_t`) against
  `GetSphereIntersection` / `GetLineIntersection` of the surface; the radius is not a constant, the caller passes it. Our
  push-out of spheres against walls reproduces the contract, not the iteration. **[GUESS]**
* `support` returns the highest surface at or below the point; the client's KD query is a ray of limited length
  (terrain delta + 0.3 m outdoors, 1.0 m in dungeons) and dungeon tile floors are cast from +100 m. Equivalent when the
  position is on the ground, different for deep falls onto statels.
* Dungeon room-to-room door permission and per-room liquids are not ported; the tile-word diagonal bit is assumed 0.
* v4 invisible surfaces are treated like visible ones; teleport portals are decoded but not interpreted here.
* Not found: any `.cim` / PathFinder navmesh for the player (PathFinder.dll only `GraphPathFinder_t`/`VisibilityGraph_t`).

## 6. Verification (commands, observed)

* `cargo test --release -p ao-formats collision`: unit tests (`kd_version4_record`, `kd_version5_record`, `kd_garbage_never_panics`,
  synthetic floor / wall / step / platform, terrain parity, closest point) and `tests/collision_real.rs` (skip without the client):
  * 4604 Arrival Hall: spawn (server 205.2, 1.0, 255.8) → floor within 0.5 m; a flood fill on a 0.5 m lattice with `slide` + `ground`
    (capsule 0.35 x 1.8) reaches both ends of the hall (corridor end at server z 261.8, north end at z 157.3, 11 936 cells), the path
    back to the spawn never jumps more than `STEP_HEIGHT + 0.12` and always has ground (no fall through); a 40 m push to +x from
    the corridor stops at its wall (tile boundary 213.5).
  * 4582: collision ground within 3 m of the rendered terrain around the spawn, ground under the spawn eye, none far outside.
  * records of both versions decode; `liquid_at` is well defined.
* `cargo test --release -p ao-formats --test collision_real -- --ignored` (about 15 s): all 234 349 records decode with their markers.
