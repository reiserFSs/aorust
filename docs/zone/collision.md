# Ground following and collision (N3.dll surfaces, rdb 1000013)

Port of the data and queries behind `ao_formats::playfield::collision` (`crates/ao-formats/src/playfield/collision.rs`,
`collision/kd.rs`). Everything below was read from the client DLLs (Ghidra, N3.dll image base `0x10000000`) and checked
against the shipped data. Labels: **[CODE]** read in the decompile, **[DATA]** verified on the files, **[GUESS]** not resolved.

## Native Shadow3008 / WaterRipples3005 context

**[CODE]** `Vehicle_t::EnsureSurfaceAlignment` @Vehicle `0x1000d1aa` passes `vehicle+0x10c` as
`LiquidMediumData_t*` to `Surface_i::CalculateClosestPoint`; the retained
submersion is overwritten after the liquid-medium state machine when the final feet position
is below the liquid.
N3 outdoor `0x10018f0c` and room `0x10013ee6` write liquid flags at struct `+4` (Vehicle `+0x110`)
and the collision-plane direction at struct `+8/+0xc/+0x10` (Vehicle `+0x114/+0x118/+0x11c`).
The port retains these authored polygon flags/normals in `Closest::liquid_info` and
`SurfaceState::liquid_info`; `Movement::effect_liquid` passes them with the existing native
submersion, not a fresh render-time depth estimate. No liquid is represented by `None`.
**[CODE]** The outdoor query now uses native authored order, not highest-surface selection.
GC `0x100b7f61` calls `VisualWater_t::Create` for the authored triangles in order, decodes
depth as `(kind >> 5)/10` (zero means 100000 m), and registers each collision info in every
zone intersecting its expanded bounding rectangle via N3 `SetWater` `0x1001abf2`.
DS `0x1003a3b4` recursively splits cross-product magnitude above 10000 into midpoint
corner A/B/C/centre children; `0x1003a0d6` emits collision infos only for even liquid
types and appends them using `0x10071fc4` (sentinel tail insertion). The collision plane
normal is flipped upwards by `0x10039f14`; flags contain only `kind & 0x1f`.
Its centroid bucket test uses `trunc(x/100)-60*trunc(z/-100)` in `[0,3600)`, with
double constants at `0x10089e48` (+100), `0x1008c030` (-100), and `0x1008a690` (3).
N3 `GetLiquidSurfaceHeight` `0x1000cb1e` → zone `0x1001ab68` walks this appended
list, returns the first positive collision satisfying authored depth, and stops on the first
positive collision that fails depth. DS `PerformCollisionTest` `0x10039d3c` rejects
triangle edges, points at/above the triangle top, and nonpositive plane heights.
The port builds the same collision-only subdivision/order and uses these strict/depth tests.
Testing triangle membership against this ordered list is equivalent to restricting it to
the registered zone list: every containing collision triangle was registered into the
point's zone by the bounding-rectangle registration.

`outdoor_liquids_use_authored_order_depth_and_strict_edges` covers first-not-highest
selection, depth-failure short circuit, low-five-bit flags, excluded edges, four-child
subdivision and upwards normal. It was added but not run by the implementation worker.

**[CODE/DATA]** Shadow `GC 0x100ecee4` reads VisualEnvFX sun Light `+0x104`.
DisplaySystem's GAME tweak update `FUN_1005cdcf` (the `TweakedSunDirection` variable copy to Light
`+0x104/+0x108/+0x10c`) writes `Tweak_GAME.txt` line 71:
`Unit1ZDirection [ROT] Sun1Rotation`, where `Unit1ZDirection` is **+Z**.
The existing scene sky uses **−Z** for its vector towards the sun, so the Host passes
the negated loaded environment direction and updates it with each received live sky.
It does not substitute the renderer's generic default environment when no scene environment exists.
The scene sun direction now uses the native binary-sun ephemeris (GC `0x100b879b` /
`0x100b9044`, constructor parameters `0x100b86ca` / `0x100b8ea6`; see the sky documentation).
Dungeon state comes from the loaded
`ZoneLocator::is_dungeon`; head height comes from the current avatar rig's root/head
attractor via `Avatar::head_height`, not a camera-pivot constant.

Regression coverage: `native_shadow_sun_uses_the_loaded_environment_ray_direction` checks
the environment sign and scene-replacement reset; `collision_room_liquids_are_found_in_dungeons`
checks real playfield 120's retained liquid flags/direction against its collision query.
These checks were added but not run by the implementation worker.


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

### 3.3 Dungeon walls = room membership, door rule
`n3RoomSurface_t::VetoPosition` @0x10015587 keeps the fractional part of x / z inside `[0.01, 0.99]` (f64 @0x1003d628 / @0x1003d368), clamps to
`[0.1, 7999.9]` / y `[0.01, 1999.9]`, then `VetoRoomTransition` @0x1001462f: `cur = PosToRoom(pos)` (-1 outside every room: empty `DCGA` tile, outside the
rectangle or the tile's height window). With `last = n3Dynel_t::GetLastAllowedZoneInst`:
`cur == last`: `-1` is refused (position := `PlayfieldAnarchy_t::GetSafePos` @0x10121815), otherwise accepted and remembered
(`UpdateLastAllowedPosition`); `cur != last`: playfield vtable `+0x38` = `PlayfieldAnarchy_t::IsDynelRoomTransitionAllowed` (Gamecode @0x10122371, base
`n3Playfield_t` @0x1000c4d1 = `from == -1`) decides; refused moves put the dynel at the last allowed position nudged by `0.1 * radius / 2` away from the
wall. The rule for a character: the rooms must be joined by a door link (`n3Room_t::GetDoorConnectZone` @0x10010990 = first `u16` of each 4 byte door
entry of the room record; the link map is built in `n3Playfield_t::InitializeSpace`, key `(min, max)`), directly or through one intermediate room; every
registered `Door_t` on the way must satisfy `Door_t::CanPass` (Gamecode `FUN_1007f74d`: feature bit 0x80 / `FUN_1007f58e`); a link without a registered door
is free; `to == -1` is always refused. Ported: `Collision::veto`, `room_transition_allowed`, `set_door_passable` (the `Door_t` state is the caller's:
nothing calls it yet). The camera's attractor test reads a **different** flag, the link entry's open byte (`IsDoorOpenBetweenRooms` @0x1000d1e9, set by `DoorOpened`/`DoorClosed` @0x1000d2bf/@0x1000d2e8 →
`ChangeRoomStatus` @0x1000d17e; entries start closed): `Collision::set_door_open` / `door_open_between` / `pos_to_room` (docs/zone/camera.md §7). **[DATA]** 4604 has three rooms but no door entries at all; the whole hall, corridor and shuttle tunnel are room 2.

### 3.4 `Vehicle_t::EnsureSurfaceAlignment` (Vehicle.dll @0x1000d1aa) and its sweep `FUN_1000b2e5` (@0x1000b2e5)
Port: `collision/vehicle.rs` (`Collision::align`, `sweep`, `line`, `closest`, `veto`). Vehicle.dll was imported into a private Ghidra project;
`this+0x50` is *falling enabled*, `+0x52` airborne, `+0x54` vertical speed, `+0x13c` steep slopes allowed (0 for characters), `+0xb0` orientation mode (0).

**Body sphere.** One sphere for every vehicle, radius **0.4** (f32 @0x100127a0, the `param_2` of the sweep), centre 0.4 above the feet (f64 @0x100127f8). There is no
head sphere or capsule height, and no scaling by monster scale. `n3Dynel_t::GetBodyCollSphereRadi` (@0x10004dd3, `CollPrim_t` radius at dynel+0x5c+0x18)
and `GetBodyCollSphereDisplacement` only feed the dynel against dynel test `CheckBodyCollision` (@0x1000483b).

**Surface queries** (`Surface_i` vtable): `+0x10` `GetLineIntersection(from, to, hit, normal, flag, source)`: nearest hit, one sided (a face is hit when `n . (to - from) < 0`,
`FUN_10031c6a`: `t` in `[-eps, 1 + eps]`, point inside the three edge planes, eps = FLT_EPSILON; KD data winding verified outward on all 228 volumes of record
301727744). Outdoor `FUN_10018b72`: terrain cells (`Intersect_Tile`, two triangles per cell, hit from above) against the cell KD surfaces, terrain wins ties; dungeon
`FUN_10015018`: only when start or end is in a room, tile floor triangles + the room KD volume. `+0x04` `CalculateClosestPoint` (3.1, 3.2); `+0x20` `VetoPosition` (3.1, 3.3).

**Step** (`Run` calls it with `param_2 = false`; `true` = teleport, no sweep):
1. veto loop: `cur = new`; while `VetoPosition(cur)` refuses, `cur = old + (new - old) * 0.1 * n` for `n = 10 .. 1`.
2. `flag = !fallingEnabled || airborne || teleport`; `delta = cur - old`; budgets `H = |delta.xz|`, `T = flag ? |delta| : (H < 1e-6 ? |delta| : 10000)`;
   heading `dir = normalize(delta.x, delta.y * flag, delta.z)` (`(0,-1,0)` when zero); slope limit 0.5 when falling is enabled (f32 @0x10012134), else none.
3. `FUN_1000b2e5(centre = old + 0.4 up, r = 0.4, dir, &H, &T, &maxY, mode = fallingEnabled ? 3 : 1, 10 iterations, slope)`; per iteration, aiming at `q = p + 10 dir`
   (f32 @0x10012790): mode 3 first casts two side probes of length `r` (`dir x up`), shortens them to walls, moves the centre to their midpoint when the lengths
   differ, halves them and casts two rays parallel to the heading from `p +- side`, whose hits are projected onto the centre line through the hit plane (back faces,
   `side . n > 0`, and `d . n == 0` ignored); then the centre ray `p -> q`. The nearest of the three (ties: centre, then B, then A) is the obstacle `(hp, n)`; none:
   fly `|q - p| - r` along `d` within the budgets. When moving up (`d.y > 0`) into `n.y < slope` the face is treated as a vertical wall (`n = normalize(n.x, 0, n.z)`,
   head-on / `n.y <= -0.99`: `-d`; effective radius `r (1 + n.y^2)`). The sphere moves to the plane (`hp - d r_eff / |d . n|`, or stays when already closer),
   spending `H` by the horizontal part and `T` by the full length (a budget that runs out ends the sweep). The new heading is the tangent `((nc x n) x n)` with
   `nc = normalize(p - hp)`, only when `1e-5 <= |nc - n|^2 <= 3.99999` (head-on contact stops) and `dir . tangent > 0`; a vertical wall (`n.y == 0`) keeps `q.y = p.y`.
   Measured on synthetic geometry: a 1.97 m diagonal step into a wall reaches it after 1.65 m and slides the remaining 0.31 m (test `walks_head_on_into_a_wall_and_slides_along_it`).
4. feet := `centre.y - 0.4`; `CalculateClosestPoint(x, feet, z)`; `feet = max(feet, closest.y + 0.01)` (f64 @0x100124e0); `maxY = max(maxY, feet)`.
5. tolerance `tol = 0.48` (f32 @0x100127e0), while walking on the ground `min(|delta.xz| * 1.1547005 + 0.48, maxY)` (f64 @0x100127d8 / @0x100127d0; the *Avatar.Movement* RE
   read the first as the f32 `2.0`, the instruction is `FMUL double ptr`, so `docs/zone/movement.md` §9 "2 * step" was wrong).
6. three ground rays at offsets `0.04 * (1,0,0), (-1,0,1)/sqrt2, (-1,0,-1)/sqrt2` (f32 @0x100127cc) from `maxY (+0.4 when falling is enabled)` straight down to y = 0
   (miss: y = 0); their points give the ground normal `(b - a) x (c - a)` (flipped up) and `ray_y = max y`.
7. supported when `feet - tol <= max(ray_y, liquid level)`; then, with `vy <= 0.1` (f64 @0x100127c0) and falling enabled, `feet = max(ray_y, closest.y) + 0.01`; the
   ground normal must be `>= 0.5` and `vy <= 0.1` to count as ground (`airborne = false`, the caller lands a falling body: `LandNow`), otherwise the body is airborne (the caller starts a
   fall: `FUN_1000a1a7`). Hover guard: same height, `vy <= -0.1`, five frames in a row -> counted as ground (`Vehicle+0x138`).
8. final `VetoPosition` on `(x, feet, z)`.
Sliding up a single steep plane is therefore possible for one step, but the body is not supported there (normal < 0.5) and falls back down.

### 3.5 Authored rock effects (class 1027)

**[CODE]** Gamecode `FUN_101017b6` asks `FUN_100ad4bd` for the advanced rock point's correction and normal. `FUN_100af659` is the surface-wrapper singleton accessor, not another collision test. `FUN_100ad4bd` calls the playfield surface's `VetoPosition` (`+0x20`, null source arguments), then `CalculateClosestPoint` (`+0x04`), and raises only the veto-corrected point's y when the closest point is higher. The normal comes from that surface query; no invented horizontal plane or previous-to-current segment is involved. N3 `VetoRoomTransition` @0x1001462f accepts a null-source point inside a room and uses `GetSafePos` outside rooms; a fresh `SurfaceState` with room -1 reproduces this without remembered dynel or door transitions.

Runtime: `Player`'s loaded `Collision` → `Flow`'s optional query capability → `Dynels::update_with_collision` → effect renderer. The query reuses `Collision::closest` for outdoor heightfield / dungeon tiles and their native KD volumes and liquid-depth rule. Geometry absence is an absent capability, not a successful no-hit answer. Regression: `player::tests::effect_collision_uses_surface_normal_and_never_lowers_the_rock` checks a sloped collision mesh's actual normal, preserved high point and off-mesh no hit.

## 4. API (`ao_formats::playfield::collision`, scene coordinates)

* `Collision::load(&RecordStore, playfield) -> Result<Collision>`: heightfield (outdoor) or room tile floors + room list with door links (dungeon), the KD volumes of every
  zone / room, the liquid polygons of the record tail. 4604: 3 ms, 4582: 41 ms, 566: 8 ms. `Collision::from_scene(&Scene)`: **fallback** (documented deviation): only the
  identity-placed meshes (terrain, room shells), no statels; `closest` then uses the nearest face within 100 m below.
* `align(old, new, &Body, &mut SurfaceState) -> Aligned{pos, airborne, normal, liquid}`: 3.4. `walk(from, to) -> Aligned`: the same with a standing body and fresh state
  (tests, autopilot). `Body::WALKING`, `RADIUS = 0.4`, `FOOT_CLEARANCE = 0.01`.
* `line(a, b) -> Option<Hit{p, n}>` (`GetLineIntersection`), `closest(feet, room_hint)`, `veto(&mut p, &mut SurfaceState) -> bool`, `ground(feet) -> Option<f32>` (= closest point
  height; replaces the "highest surface at or below" query, the KD ray is limited to terrain delta + 0.3 m outdoors and 1 m in dungeons, dungeon tile floors are cast from the plane),
  `liquid_at`, `inside`, `room_of`, `pos_to_room`, `room_links`, `room_transition_allowed`, `set_door_passable`, `set_door_open`, `door_open_between`, `sphere_hit` (camera boom only), `triangle_count`,
  `door_link_from_pos(scene_pos) -> Option<(u16, u16)>` (`n3Room_t::GetDoorLinkFromPos` @0x100105f9 over every room: the door entry `tile << 2 | orientation` word (2nd `u16` of the record's door entries, `Room::door_tiles`) -> the tile of the room rectangle
  (row length `x2 - x1`, 2 m cells), pushed 0.99 m (f64 @0x1003d368) to the tile edge `orientation` (0 +z, 1 +x, 2 -z, 3 -x), room-local with the room centre as origin, turned `rot` quarter turns about +Y, plus the room position; a match is within 1.2 m (f64 @0x1003d370) in x and z, y is not tested;
  result `(room, connected room)`, connected `0xffff` = the entry leads nowhere; real data: playfield 6131 finds exactly its one link),
  `in_teleportal(scene_pos)` (`n3Zone_t::IsPosInTeleportal` N3 0x1001a86a: the portal polygon of the zone holding the point, x/z parity test `FUN_1001b21a`, see docs/zone/world.md §10.2).
* `kd::parse(version, bytes) -> Surface{volumes, nodes, portal, portal_dest}`: the decoder of section 2.
* Removed with the cutover: `support`, `wading_ground`, `slide(from, to, radius, height)` (capsule push-out). `MIN_FLOOR_NY` (0.5), `STEP_HEIGHT` (0.48, the base tolerance), `RAY_LIFT` (0.4), `WADE_DEPTH` (1.2).

## 5. Deviations / unresolved

Closed (AfCollision):
* **Room liquids** (N3 `FUN_10012803` @0x10012803 room reader): `u32 (n+1)*0x3f1` (else "broken water data"), per liquid `kind, nv, nv x vec3 (y snapped to 1 cm), nt, nt x 3 u16`; even kinds with `nv != 3` become a fan around the vertex centroid. `n3Zone_t::AddLiquidCollisionData` @0x1001a9c5 multiplies the rotation `rot*pi/2` by the f64 constant 0.0 @0x1003cb08: vertices are room-local, **never rotated** (world = `room.pos + v`; verified on data). `FUN_1000b0d1` skips odd kinds. Test: `n3Zone_t::PerformLiquidCollisionTest` @0x1001a80c / `FUN_1000b498`: first triangle holding x/z with `ground.y < top` and local plane height > 0; counts when `level - depth < y`, `depth = (kind>>5)/10` (none: 1e5). `Collision::liquid_probe`, `Room::waters`. Outdoor polygons: still highest surface (the creation of the outdoor `WaterCollisionInfo_t` @DisplaySystem 0x10039d3c objects was not traced).
* **Liquid medium** (`Vehicle+0xfc`, EnsureSurfaceAlignment tail 0x1000dd8c..): `vehicle.rs::medium`; modes 0..4, wade threshold `+0x100` = 1.19 (ctor), `+0x120` callback flag, `+0x10c` submersion. Player vehicle: mode 0 (no writer exists); callbacks `+0x80` = `FUN_1006f99e` (clear MechData, Transition 0x1a), `+0x84` = `FUN_1006ef74` (Transition 0x23); wired in `Movement::liquid_callbacks`; SwitchToSwim also writes WaitState 0.
* **VetoRoomTransition radius** = `dynel->vtbl[+0x10]` = `n3Dynel_t::GetBodyCollSphereRadi` @0x10004dd3: torso `CollPrim` radius from `n3VisualDynel_t::UpdateCollision` @0x10019be4 (model torso sphere x body scale, 0.5 if negative). `SurfaceState::radius` defaults to 0.5. **Torso sphere** (verified): `VisualCATMesh_t::GetTorsoSphereRadi` @DisplaySystem 0x10072c22 = `this+0xbc`, set by `FUN_1007494b` from the CAT resource wrapper `+0x30` (4 floats: radius, x, y, z), which `FUN_10071ae2` reads with `FUN_1006aab4` from the **one `u32` after the part table** (the word `CatMesh::parse` used to skip): four signed bytes `c`, each `0.05 c + 6e-6 c^3` (`FUN_1006aa89`; byte 0..2 = x, y, z, byte 3 = radius) = `CatMesh::torso_sphere`; NOT `col_spheres[0]`. Wired: `Avatar::body_radius` (x body scale, 0.5 if negative) -> `Movement::set_body_radius` (kept across `teleport`). Push-back is `last + dir*0.1*R/2`, else `last - nudge`, else `GetSafePos`; standstill uses the backwards heading vector.
* **GetSafePos** (`playfield+0x44` = resource `+0x4c`, zero from the ctor, never written): room 0, centre, y = max of 4 tile corners; not the last position. **Dynel `+0x74/+0x78`** = parent dynel id (`SetParentDynelID`); veto skipped while parented (`SurfaceState::parented`).
* **Per-cell closest**: KD triangles are tagged with their zone/room; the closest-point query only sees the surface of the position's zone (`n3TilemapSurface_t::CalculateClosestPoint` @0x10018f0c `GetSurfaceForCell`).

Open:
* Dungeon `line` uses exact triangle tests (the client marches 1 m steps). KD miss outdoors not reproduced (never beats terrain).
* Orientation modes 1/3/4, `Vehicle+0x13c`.
* Tile diagonal bit 0x4000 assumed 0; v4 invisible surfaces (`EnableInvisCheck`) treated as visible.
* `Door_t::CanPass` is ported as "unlocked" only (`FUN_1007f74d`: open and `+0xfc` (= `IsLocked`, `Flags` bit 0x40; `FUN_100850c9` unlock tests it) false -> pass; `FUN_1007f58e`: stat 0xbd == 0 or not locked -> pass; the locked rest (stat 0x103 bit 0x10 refuses, bit 0x20 key check, owned buildings, team members) is "refused"). Doors that never get a model with node keyframes (no `ItemRig`) never reach the collision.

Closed (AfDoors):
* **Door wiring**: `Dynels` (`dynels_doors::PropAnim::take_room_state`) reports `(scene pos, open, passable)` of every door whose `RoomOpened/RoomClosed` effect fired (and of a new door, `LinkDoorToRooms`); `Player::door_rooms` maps the position with `Collision::door_link_from_pos` and calls `set_door_open` (camera attractor test) + `set_door_passable` (`VetoRoomTransition`). A new `Player` (new collision) asks the doors to report again (`Dynels::resync_doors`).
* **Fall-start callback** `FUN_1006ef34` = Player vehicle vtable `+0x70`, called by `Vehicle_t::Run` (Vehicle.dll 0x1000e849) when the speed `+0xcc` (3D length of the velocity, `Impact` does not touch it) goes from 0 to > 0 over the frame in the free branch (`+0x108 == 0`): `Transition(1)` ForwardStart, `3` ReverseStart when `Vehicle_t::GetDir < 0`. The speed is the length of the horizontal-plane velocity `+0x64..+0x6c` only (`FUN_1000e3d3` writes `+0xcc` from it; the vertical speed is the separate `+0x54`), so a standing jump (vertical only) does **not** fire it: retail jumps in place (Ghidra decompile of both functions). `Movement::start_moving_callback` (through the permission table; no-op while already moving forward).

## 6. Verification (commands, observed)

* `cargo test --release -p ao-formats collision`: unit tests (KD decoders, one sided line, wall head-on / diagonal slide numbers of 3.4, 0.3 m step passes and 0.6 m blocks, 30 degree
  ramp carries / 70 degree ramp does not support, edge fall, closest-point ray limits with terrain, outdoor veto) and `tests/collision_real.rs` (skip without the client):
  * 4604 Arrival Hall: flood fill with `walk` on a 0.5 m lattice reaches both ends of the hall, no height jump above `STEP_HEIGHT + 0.12`, a 40 m push stops at the corridor wall;
    all of it is room 2, no door links;
  * 4582: one frame steps across 169 lattice walks: feet = ground + 0.01 wherever supported, 8% of steps airborne (slopes), 87% unobstructed; KD volumes wound outward (228/228);
  * door rule on the link graph (`room_transition_allowed`).
* `cargo test --release -p aomac autopilot_crosses_the_arrival_hall`: the route over `walk` cells drives the headless character through the hall.
* `cargo test --release -p ao-formats --test collision_real -- --ignored` (about 15 s): all 234 349 records decode with their markers.
