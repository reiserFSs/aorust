
## installer

Ported from the decompiled PRK launcher (`MainWindow.DownloadLatestVersion`, `HttpPatchServerService`, `VersionMap`, `VersionIdentifier`, `DeletionHandler`, `RDBController.ApplyRDBPatch`). Implementation: `crates/ao-install`.

- **Map**: `GET https://patches.project-rk.com/patches/patches.map`, lines `src = dst` (split on `=`; lines not giving exactly 2 parts are ignored). Many sources map to `0.3.0`/`0.5.0`; the main chain is `0.0.0=0.6.1`, `0.6.1=0.6.2` … `0.6.36=0.7.0`, `0.7.0=0.7.1`, `0.7.1=0.7.2`.
- **Version** (`VersionIdentifier`): `M.m.p[-pre][+build]`, equality is field-wise (pre-release parts after the first `-` are concatenated). Current version = `patch.version` in the client root, else `0.0.0`.
- **Loop**: while a patch with `src == current` exists: download `patches/<dst>.zip` → extract over the client dir with overwrite → if `<dst>.rdbpatch` in client root: apply and delete it → delete zip → if `<dst>.df` in client root: process it (file stays) → write `patch.version = dst` (tmp file + rename). Any failure before that leaves `patch.version` untouched: extraction overwrites and the rdbpatch is one `INSERT OR REPLACE` transaction, so a rerun redoes the step; an unreadable zip is deleted so it is re-downloaded. Unit test `failed_patch_keeps_version` injects a corrupt zip, a zip-slip entry and a truncated rdbpatch.
- **Zip**: paths relative to client root (e.g. `cd_image/...`); we reject entries that escape it (zip-slip). Resume: partial zip is kept in `<client>/../downloads/<dst>.zip`, continued with HTTP `Range`. Verified live: `0.7.0.zip` (140 733 024 B) killed at 25 173 681 B, rerun sent `Range: bytes=25173681-` → server `206`; result sha256 `22a5f7e7…edf1de` equals a fresh `curl` download (server sends `Accept-Ranges: bytes`, `ETag`). A rerun on a complete file gets 416 and is a no-op.
- **`.rdbpatch`**: `i32 count`, then per record `u32 type, u32 instance, u32 version, i32 len, bytes[len]` (little endian). The launcher ignores the per-record version and runs `INSERT OR REPLACE INTO [rdb_<type>] (id, version, data) VALUES (<instance>, 1, <bytes>)` on `cd_image/rdb.db`. We do it in one transaction (missing tables are created with the same schema).
- **`.df`**: one client-relative path per line; existing files are deleted, then emptied parent directories (never the client root) are removed. We reject absolute / `..` entries. Backslashes are treated as separators (the shipped `0.6.3.df` is `cd_image\twk\Tweak_….txt`; the launcher only maps `/`, so on non-Windows it would never match). Of all 50 zips in patches.map (central directories read by ranged GET) only `0.6.3.zip` contains a `.df`; `.rdbpatch` is in 0.5.1–0.5.3 and 0.6.2–0.6.36. Scratch run `0.6.2→0.7.2` in `/tmp/aoi-scratch`: the listed file was deleted, sibling kept, `0.6.3.df` left in place and every `.rdbpatch` removed — identical to the launcher (`RDBController.ApplyRDBPatch(deleteWhenDone=true)` deletes the rdbpatch; `MainWindow` never deletes the `.df`).

## renderer

`ao-render` consumes `ao_scene::Scene` only (no format knowledge). wgpu 24 / Metal, winit 0.30.

- GPU layout: per mesh one vertex buffer (48 B: pos, normal, uv, linear RGBA colour) + one u32 index buffer; instances (mat4, step mode Instance) live in a 3-buffer ring rewritten every frame with the frustum-visible instances, grouped by mesh, so one `draw_indexed` per submesh covers all visible instances of that mesh. Per (texture, `base_color`) one bind group (texture, sampler, 16 B uniform). Textures: RGBA8 sRGB, CPU box-filter mip chain, **Repeat** addressing, linear mip filter, 16x anisotropy (one shared sampler, so terrain textures included); submeshes without a texture (or with an invalid one) use 1x1 white. 4x MSAA, Depth32Float.
- Pipelines: 4 blend modes x {culled, two-sided} = 8. Front = CCW, back faces culled unless `two_sided`. Opaque / AlphaTest (`discard` at alpha < 0.5, depth write) draw first, sorted by pipeline/mesh/material. AlphaBlend (src-alpha, no depth write, constant -2 / slope -2 depth bias so coplanar overlays win against the opaque base) and Additive (src-alpha x dst-one, glow fades out with fog rather than tinting) draw afterwards as one draw per (visible instance, submesh), sorted far-to-near by instance bounding-sphere centre; ties (submeshes of one instance) keep submesh order. Fragment = texel x vertex colour x `base_color`; lit = ambient + sun_colour * max(N.L, 0) with the normal flipped toward the eye; fog is linear (`fog_start`..`fog_end`) toward `fog_color`. Clear colour = `Environment::sky_color`; far plane = 1.1 x `fog_end`. `Environment` None -> defaults (sky sRGB 0.53/0.72/0.92, fog start/end = 0.5/1.0 x max(2 x scene radius, 300 m), ambient 0.35/0.38/0.45, sun 0.75/0.71/0.64 from (0.4, 0.8, 0.3)).
- Culling: per-mesh bounding sphere (AABB centre, max vertex distance) scaled by the largest transform axis length, tested against the 6 view-frustum planes on the CPU each frame.
- wgpu 29 (not 24): `block v0.1.6` (future-incompat warning) is pulled in by `metal` 0.31-0.33, i.e. by wgpu-hal 24-28; wgpu-hal 29 uses objc2-metal and the warning is gone.
- Camera: `Scene::spawn` (looking at `spawn_look_at`, else horizontally toward the bounds centre) if set, else a 3/4 view framing the instance bounds. `--eye/--at` override it for both the screenshot and the interactive viewer.
- Viewer controls: WASD move, Space/E up, Ctrl/Q down, Shift ×5 speed, scroll = speed ×1.15 per notch, hold right mouse = mouse-look (cursor locked), Esc quits. Title shows FPS, position, speed. `AOMAC_NOVSYNC=1` disables vsync, `AOMAC_PERF=1` prints start camera, scene bounds and per 0.5 s ms/frame, `render()` CPU ms, visible instances and draw calls to stderr.
- Headless: `ao_render::render_to_png(scene, eye, look_at, w, h, path)`.
- CLI (`--client DIR` default `~/Games/ProjectRubiKa/client`):
  - `aomac install [--client DIR]` → `ao_install::run`
  - `aomac view mesh <id>` / `view pf <id>` / `view pf --list` / `view demo [--count N]`
  - on any view: `--screenshot out.png [--eye x,y,z] [--at x,y,z] [--size WxH]` renders offscreen and exits (defaults: `ao_render::default_view`, 1280x800).
- Perf (M4 Pro, release, 1280x800 logical = 2560x1600 px window, 4x MSAA, `AOMAC_NOVSYNC=1`, ms/frame; before = old renderer that drew every instance, no culling, treated all non-opaque blends as cutouts): pf 505 spawn view 2.95 -> 2.1-2.5; pf 566 spawn view 1.8 -> 0.85 (wgpu 29) ; pf 505 from 2.5 km up with ~26.6k statels visible 2.3 -> 5.3 (6689 draws; 5.7k of them are per-instance sorted AlphaBlend submeshes, CPU 2.7 ms; with those batched as opaque it is 4.2, so true sorted blending costs ~1 ms per 5.7k instances: producers should prefer AlphaTest for foliage cutouts); pf 566 from 500 m up 1.7 -> 1.9. Spawn-view numbers include scene changes by other slices between measurements. Demo `--count 10000`: 1.5-2.2 ms.

## meshes

Implementation: `crates/ao-formats/src/{archive.rs,mesh.rs}`; survey tool `cargo run --release -p ao-formats --example mesh_stats`.
Ground truth: Ghidra 12.1.4 decompilation of `randy31.dll` (the "Randy" engine; PE image base 0x10000000, addresses below are VAs): `RRefFrame_t::Archive` @10044da8, `RRefFrame_t::RRefFrame_t(ObjectArchive_c*)` @10045a2f, `RRefFrame_t::UpdateWorldMatrix` @10044fa2, `RVisualData_t::Archive` @1004ea04 (the `FAFTriMeshData_t` base), vertex-buffer (de)serialisers @1004883e / @10048a5c, `RDeltaState::Archive` @1002f145. No community documentation of this format was found; everything here comes from the DLL plus the data.

### Record types
- `rdb_1010001` (9925 records, 352 MB): "static meshes" — scenery, buildings, props, trees, doors, lamps, *and* animated/effect meshes. Ids 1414…289317. A record is a whole node tree (one model), not a single triangle soup.
- `rdb_1010026` (699 records, 14.6 MB): same container and classes, **every id also exists in 1010001**, ~3× fewer triangles (e.g. 1495: 176 → 12 tris, 134 → 18 verts), single node, keeps the same textures: a **reduced-detail mesh**. Code evidence (type ids 1010001 = 0xf6951, 1010026 = 0xf696a): `n3VisualDynel_t::HasMesh` (N3.dll @10019514) accepts both as "plain mesh" (the CAT pair 1010002/1010027 = 0xf6952/0xf696b is `HasCatMesh` @100194f9); `RDBMesh_t::ReduceMesh` (DisplaySystem.dll @10077095) derives a 0xf696a `DbObject` with the same id from a 0xf6951 one by keeping only the four `SimpleMesh`es with the most vertices (returns null when the source has < 4) and cloning the rest of the node; the one consumer found is `AOPlayfieldCityHolderClient_c::EnableVisibility` (Gamecode.dll @1012e58b), which builds `VisualMesh_t`s from `Identity{0xf696a, id}` for the `HouseVisualData` list of a player-city holder and registers them with `LODManager::AddMesh` (distant city houses). So it is a LOD proxy used by the city/LOD path; statels and ordinary visuals reference 1010001. `[INFERENCE: the shipped records do not all have ≤ 4 submeshes (num_meshes 1…21), so they were generated by a different/earlier tool than ReduceMesh; the distance switch inside LODManager is in a DLL that was not decompiled]`. `mesh::decode_record_into(.., MESH_LOW_TYPE, ..)` decodes it; `decode_mesh_into`/`load_mesh` use 1010001.
- Textures referenced by meshes: `rdb_1010004` (32 629 refs: JPEG or PNG-with-alpha) and `rdb_1010011` (5 refs); decoded by `texture::load_texture`.
- Out of scope (only noted): `1010002`/`1010003` (character CAT meshes, 770/3190 records) and `1010027` (8 records) do **not** start with the archive header; they start with 32-byte zero/0xCD-padded names (`Bip01_ac`, `body`, `maintower`, `buffbase2a`) followed by what looks like `CATMesh_t` data (class names `CATMesh_t`, `CATAnim_t`, `CATAnimBlend_t`, `CATKeyframeAnim(Data)_t` exist in randy31.dll). `1010013` (363 small records beginning `02 00 00 00`, floats follow) and `1010025` (one record of animal names "Beetle/Bird/Bunny") are not meshes of this format.

### Container: Funcom `ObjectArchive` (little endian)
```
u32 version = 3
u32 range_count
range_count x { u32 value; char name[32] }   named animation ranges ("idle","walk","shotgun_reload"…), 0xCD padded; 36 bytes each
u32 w0, u32 w1(=1)                           not used by the reader (w0 is 0 or a small packed value such as 0x07000700)
u32 name_count (<=256)
name_count x { cstr kind; cstr name }        kind = "" starts a class (name = class), else a member "digit"; names are indexed globally 0..name_count
u32 object_count
(object_count + 1) x object                  object 0 is a holder with a single member "obj" (ref) = index of the root object
object := u32 type_id, u32 version(=1), u32 member_count, member_count x member
member := u8 name_index, u32 type, u32 elem_size, u32 total_size, bytes[total_size]
trailing: 12+ zero bytes
```
Evidence: record 1010001/287726 (311 B, an empty `RRefFrame_t`): table of 9 names (`RRefFrame_t`, `__class_id__`, `grp_mask`, `local_pos`, `local_rot`, `scale`, `conn`, `chld_cnt`, `obj`), holder member `obj`=0, object 0 has 7 members, file ends exactly 12 bytes after the last member. Records with `range_count` > 0: 7798 (3 ranges), 36077 (idle, walk), 75392 …; before handling the ranges 16 records failed (grouped as "implausible name table"/"no root"), after: all parse.

Member `type` codes seen (self-describing; matches `fun::Message_c::Add*` in the DLL): `0` bool/u8, `3` int32 (arrays when size > 4), `6` string (`u32 len` + bytes, no NUL), `9` raw blob (`u32 len`-prefixed for `vertices`/`triangles`, bare 16 bytes for `vb_desc`), `0xa` f32, `0xc` Vector3, `0xd` Quaternion (x,y,z,w), `0xf` 4x4 matrix (16 f32), `0x10` RGB f32 colour, `0x11` object reference(s) (i32 each; `-1` = null; one member holds all refs of an array, e.g. `mesh`, `chld`, `tch_text`). Repeated scalar members (`rst_type`, `rst_value`, `tch_type`, `tch_text`) appear once per element and must be read in order. `__class_id__` is a per-file running number, not a class tag; objects are classified by their member names. The 1-byte `name_index` limits a record to 256 names.

### Object classes (identified by members)
| object | members used | meaning |
|---|---|---|
| frame node (`RRefFrame_t`, `RTriMesh_t`, `FAFPointLight_t`, `FAFCollisionSphere_c`, `FAFAttractor_t`, `RRefFrameConnector`, …) | `local_pos` v3, `local_rot` quat, `scale` f32 (always 1.0 in the data), `anim_matrix` m4 (optional), `chld`/`chld_cnt`, `conn`, `anim`, `grp_mask` (always -1 on meshes) | transform tree. Roots may be `RTriMesh_t` itself (class list then lacks `RRefFrame_t`) |
| mesh node (`RTriMesh_t`) | node members + `data` ref, `delta_state` ref, `prio` (3/5/6 draw order), `enable_light`, `is_cloned` | |
| mesh data (`FAFTriMeshData_t`) | `name`, `anim_pos`, `anim_rot`, `num_meshes`, `mesh` refs, `bvol` | holds `num_meshes` (1…57) `SimpleMesh` refs |
| `SimpleMesh` | `material` ref, `trilist` ref, `vb_desc`, `vertices` | one draw call |
| `FAFMaterial_t` | `delta_state` ref, `env_texture` ref, `diff/spec/ambi/emis` RGB, `shin`, `shin_str`, `opac` | `RMaterial_t::Archive` @10040916 (offsets +0x2c diff, +0x38 spec, +0x44 ambi, +0x50 emis, +0x5c env, +0x60 shin, +0x64 shin_str, +0x68 opac, +0x70 delta_state). `RMaterial_t::InitD3DMaterial` @100409c6 fills a `_D3DMATERIAL7`: diffuse = (`diff`, alpha = `opac`), specular = `spec`×`shin_str`, ambient, emissive, power = `shin`. **Diffuse and opacity are used** by the loader (→ `base_color`) |
| `RDeltaState` | `rst_count`,`rst_type`,`rst_value` pairs (D3D7 `D3DRENDERSTATE_*`), `tstv_count`,`tstm_count`,`tst_type`,`tst_value`, `tch_count`, `tch_type`+`tch_text` per channel | render-state delta; applied by `RVisual_t::ApplyStateChanges` @1004cbe1 (node, before the mesh loop) and `FUN_10040aa0` ← `RViewPort_t::SetMaterial` @1004b199 (material, per `SimpleMesh`, later ⇒ wins) |
| `FAFTexture_t` → `AnarchyTexCreator_t` | `creator` ref → `type` (rdb type, 1010004) + `inst` (rdb id) | texture record |
| `TriList` | `triangles` = `u32 byte_len` + u16 triples | triangle list, 16-bit indices |
| `BVolume_t` | `sph_pos`, `sph_radius`, `min_pos`, `max_pos` | bounds, ignored |
| `FAFAnim_t` | `name`, `tot_time`, `loop`, `rot_keys`, `trans_keys`, `vis_keys`, `uv_keys` | keyframes, ignored |

`vb_desc` = `{u32 16, u32 0x10000, u32 FVF, u32 vertex_count}`; FVF is **0x112** (D3DFVF_XYZ | NORMAL | TEX1) in all 50 159 SimpleMeshes → 32-byte vertices `pos[3] normal[3] uv[2]` (f32). `vertices` = `u32 byte_len` + `vertex_count * 32` bytes. The loader also understands diffuse/specular/extra UV sets generally from the FVF bits.

Render states observed (`rst_type`/`rst_value`; the engine is **D3D7**: `_D3DMATERIAL7`, `_D3DRENDERSTATETYPE`, ids below are D3D7's) over all of 1010001: 15 ALPHATESTENABLE=1 (1363 deltas), 27 ALPHABLENDENABLE=1 (1639) / =0 (1591), 22 CULLMODE=1 (D3DCULL_NONE, 1708), 29 SPECULARENABLE=1 (8395), 145 DIFFUSEMATERIALSOURCE=0 (519). **Never present: 19/20 SRC/DESTBLEND, 24/25 ALPHAREF/FUNC, 14 ZWRITEENABLE, any CULLMODE other than 1.** (145–148 are D3D7's `DIFFUSE/SPECULAR/AMBIENT/EMISSIVE_MATERIALSOURCE`; 0 = take it from the material instead of the vertex colour; `FUN_1004ac5f` @1004ac5f sets 145..147=0 and 148 = (emissive ≤ threshold) on every material, `RenderWithTransparency` sets 146/148=1 when a per-instance colour is bound.) Foliage keeps its states on the **node's** `delta_state` ("alpha", node 11 of 1010001/6273 → `rst 15=1`) while the texture is on the material's `delta_state`; the loader merges both, **material wins** per state id. `tch_type` is the channel (0 = diffuse). Texture-stage deltas only occur as `tst (1, 7)` (COLOROP=ADD, 1011×) and `tst (2, 34)` (COLORARG1=TEXTURE|ALPHAREPLICATE, 1953×).

**Where the states come from (`FUN_10040645` @10040645 = RMaterial transparency preset, mode chosen from a `[n]` prefix of the material name or automatically from the texture's alpha channel, `FUN_1004761a`):** mode 0 texture alpha = transparency (`0x74`/`0xbd` flags, 27=1); 1 alpha test (27=0, 15=1, ALPHAFUNC=GREATER, ALPHAREF=0x80); 2 alpha test ref 0x1e **and** blend (27=1,15=1), z-write off; 3 blend only, z-write off; 4 additive (27=1, SRC=ONE, DEST=ONE, TSS colour op = SELECTARG1(texture)), z-write off; 5 (automatic for textures with alpha when ≥ 2 texture stages and no transparency flag): stage 0 = `texture.alpha(replicated) + diffuse` (COLOROP ADD), stage 1 = `texture.rgb × stage0` (COLOROP MODULATE) with the same texture bound twice, 27=0 — i.e. **the texture alpha channel is a self-illumination mask** (this is the `(27,0)` + `tst (1,7)` pair in the data, ≈1000 materials). `RMaterial_t::IsTwoSided` @10040aff = rst 22 == 1 (NONE); `RMaterial_t::SetOpacity` @10040f61 sets the "transparent" flag `0xbd` (render list 6, back-to-front sort) when `opac` < threshold or the texture has alpha, but if a delta state exists it is overwritten by rst 27==1. `RTriMesh_t::Render` @10049761 puts the mesh in render list 6 when any material is transparent or frame transparency < 1, else list 3 (the `prio` member of the node is 3/5/6: opaque / ? / transparent, i.e. the same sort-list choice; the renderer sorts blended draws itself, `prio` is not exported). Randy never sets SRC/DESTBLEND for mesh materials (only for the sprite/glow classes: `RSprite::EnableAdditiveRendering` @10013128, DisplaySystem @859: SRC=SRCALPHA(5), DEST=ONE(2), z-write off), so plain blend uses the D3D default pair `[INFERENCE: SRCALPHA/INVSRCALPHA, the only pair that makes 27=1 visible]`. Randy never changes CULLMODE globally, so the device default (D3DCULL_CCW; front = clockwise in D3D's left-handed space) applies to every material without `rst 22=1`.

### Transform rule (from `UpdateWorldMatrix` @10044fa2)
Row-vector D3D convention (`v' = v · M`, translation in row 3): `local = R(local_rot)ᵀ · scale, translation = local_pos`; if `anim_matrix` exists: `local = anim_matrix · local`; `world = local · parent_world`. Quaternion → matrix is the standard one, transposed (decompiled @1006e393). `anim_matrix` of mesh nodes equals the object's 3ds-Max node transform (= `FAFTriMeshData_t.anim_pos/anim_rot`, e.g. `Rx(+90°)` for Max Z-up → Y-up: 1010001/214381, `anim_pos` (-0.539, 0.452, -0.454) = `anim_matrix` row 3) and is what makes the vertices upright; `anim_pos/anim_rot` themselves are **not** applied again. Vertices are stored in the mesh node's object space.

### Coordinate conversion (loader output)
Source space is **left-handed, Y up, 1 unit = 1 m** (D3D9; triangles are clockwise-front: for record 214381 the numeric cross product of tri (0,1,2) equals the stored vertex normal −Z). The loader bakes the node tree into one `Mesh`, then **negates Z** of positions and normals (→ right-handed, +Y up) and **reverses triangle winding** (flipped back when a node matrix has negative determinant). UVs are untouched (D3D origin top-left == `Texture` row order). Verified: over all 7.4 M triangles of 1010001 the CCW geometric normal agrees with the vertex normals for 97.7 % (7 217 373 vs 172 241, the rest are flat/double-sided strips); barrel texture 6153 (1010004) renders un-mirrored. **Anything positioned in AO world space (statel placements!) must undergo the same Z negation** (`p' = (x, y, -z)`, rotations conjugated accordingly) to line up with the converted meshes.

Submeshes are merged per `(texture, blend, two_sided, base_color)` (the loader's `MatKey`). Mapping onto `ao-scene::Submesh`:
- `blend`: rst 15=1 → `AlphaTest` (also when 27=1, see mode 2: the cutout survives without depth sorting); else rst 27=1 → `Additive` if DESTBLEND=ONE and SRCBLEND ∈ {ONE, SRCALPHA}, else `AlphaBlend`; else `Opaque`. Untextured submeshes can be blended/tested too (alpha = `opac`).
- `two_sided` = rst 22 == NONE; **default is culled** (back faces of the CCW-front scene-space triangles).
- `base_color` = (`diff` rgb converted sRGB→linear, `opac`). D3D multiplies gamma-space values with a non-sRGB texture; the renderer samples sRGB textures to linear and encodes at the end, so converting the colour with the same power curve reproduces D3D's result. Only flat-colour (untextured) materials actually have non-white diffuse in the data (see Results); `ambi == diff` in 98 % of materials.
- Not representable in the contract and dropped: `spec/shin/shin_str` (specular, 29 SPECULARENABLE), `emis` (emissive, non-zero in 2482 materials), texture-alpha self-illumination (mode 5), `env_texture` (268 materials; only drawn when the device has ≥ 2 texture stages and the visual's env flag `+0xe1` is set), detail/secondary texture channels (`tst_*`), node `prio`, z-write state, `anim_matrix`/uv/visibility animation keys. Vertex colours: **no mesh uses them** — every `SimpleMesh` has FVF 0x112 (no diffuse/specular), and rst 145=0 takes the diffuse from the material anyway — so `Vertex::color` is white. `FvfLayout` still skips diffuse/specular bytes if another layout shows up.

### Results
`cargo run --release -p ao-formats --example mesh_stats` (all ids):
- 1010001: 9925 records → **9925 decoded** (9906 with geometry: 7 785 496 vertices, 7 398 795 triangles; 19 contain only animation/helper nodes: lights, collision spheres, connectors), 0 failures. 31 084 merged submeshes: opaque 28 788, alpha-test 1071, alpha-blend 1225, additive 0 (no 19/20 states in the data), two-sided 1691 (5 %); 733 untextured submeshes, 653 of them with a non-white diffuse (flat colours, previously grey), 0 textured submeshes with a tinted diffuse.
- 1010026: 699 / 699 decoded, 321 410 vertices, 224 910 triangles; 2310 submeshes: opaque 2032, alpha-test 236, alpha-blend 42, additive 0, two-sided 82; 14 untextured (8 coloured).
- Visual check (`aomac view mesh <id> --screenshot`): 1418 and 22854 (buildings, upright, windows textured), 6273 (tree: trunk + alpha-cut leaves), 6306 (desert tree), 1535 (rock), 3480 (barrels, logo un-mirrored), 3722 (building with lattice masts via alpha test), 13937 (lamp), 29216 (alpha ivy), 41920 (banner with correct dragon art), 36128 (house).; materials (after the renderer's blend/cull/colour support landed): 6616 (hovercraft: dark flat-colour hull parts now green/black instead of grey), 9918 (flat pink plane), 11007 (translucent magenta ring: opac 0.5 blend), 2175 (35 % translucent column with a magenta flat-colour object), 3722 (culled walls, alpha-blended windows, cutout lattice masts two-sided).
- Tests: `archive::tests`, `mesh::tests` (hand-built byte fixtures incl. an end-to-end quad with `anim_matrix`), `tests/mesh_real.rs` (skips without the client).

## playfields
Code: `crates/ao-formats/src/playfield.rs` + `playfield/{record,ground,statel}.rs`. Everything below was derived from Ghidra decompiles of the original client DLLs (N3.dll, DisplaySystem.dll, Gamecode.dll, GameData.dll; addresses are the 32-bit image addresses of the PRK 0.6.1 client) plus data inspection; no community source was needed.

### Record types (rdb `rdb_<type>(id, version, data)`; all keyed by playfield id unless noted)
| type | content | evidence |
|---|---|---|
| 1000001 | playfield resource `RDBPlayfield_t` (name, tilemap id, zone/room list) | `ReadBlob` N3 @0x1001c115; 601 ids = the playfield set |
| 1000003 | **statel file** (placed static meshes) | `n3StatelController_t::OpenStatelFile` @0x100253d4 requests `Identity{0xf4243=1000003, playfield id}`; 600 ids |
| 1000009 | tilemap `RDBTilemap_t`: magic `CHGA` = outdoor heightfield, `GNDA` = dungeon template atlas | `n3TilemapController_t::GetTilemap` @0x10017ff8 requests `Identity{0xf4249=1000009, tilemap id}`; 325 CHGA + 111 GNDA records |
| 1010006 / 1010021 / 1010022 | ground tile textures 256² / 128² / 64² (24 byte header, then JPEG at the first `FF D8 FF`) | `FUN_10036180` returns 0xf6956/0xf6965/0xf6966 by quality (DisplaySystem @0x10036150); all 849 ids exist |
| 1010001 | static mesh referenced by statels | every statel mesh id of playfields 566/705/120 is in 1010001 (see `## meshes`) |
| 1000046 / 1000047 | 28 byte records `f32 centre xyz, f32 radius` = bounding sphere per mesh id | inspected (not needed for loading) |
| 1000014 / 1000029 | district / area name tables (`GameData::PlayfieldDistrictInfo_t/AreaInfo_t::ReadBlob`) | [INFERENCE: type mapping from content, names "District", "zone1", "Area"] |
| 1000007 / 1000008 | map/minimap PNG sets | inspected, unused |
`data/Statels/<id>.pf` are **stale copies** of 1000003 (2003/2004 timestamps): 368 of 473 are byte-identical to the rdb record, 105 differ; the engine reads the rdb record. `data/Shadows/<id>.sdw` (244 files) = `u32 offsets[6]` then PNG blobs (8 bit grey, e.g. 96×192, chunk `CIDA`): baked shadow maps, not used by the loader.

### Playfield record (1000001)
```
u32 version (7..10)  u32 id  char name[32]
u32 tilemap id (rdb 1000009)  u32 zone size in tiles (10)  u32 count (zones, or rooms for dungeons)
if version > 8: 5 x i32 + 0x18 bytes (PFWorldX/Z, environment id; not used)
if tilemap id != id: count x room            (dungeon / indoor)
else (outdoor): per zone camera attractors (skipped, record ends)
room := u8 flags (rot = flags&3, 0x80 = has name)  u8
        u16 x1,z1,x2,z2 (tile rect in the dungeon tilemap)  f32 pos[3] (room centre, world)
        u16 ndoors, ndoors x (u16,u16)   [char name[32] if flags&0x80]
        i32 lightmap size+4, i32 present, [size bytes]
        u32 (n+1)*0x3f1 liquid blocks: n x { u32, u32 n1, n1 x vec3, u32 n2, n2 x 3 u16 }
        if version > 4: u32 n, n x (vec3 + quat + [vec3 if version>=6] + f32)   camera attractors
```
(`n3Room_t` reader N3 @0x10012803.) Rule found empirically over all 601 records: **tilemap id == playfield id ⇔ outdoor heightfield playfield** (325 records; for 324 the statel zones use the outdoor layout, 4622 has its own heightfield but dungeon-style zones); the other 276 are dungeons/indoor playfields built from rooms. Every dungeon record has an unparsed tail after the rooms (58–1318 bytes, ignored).

### Heightfield tilemap (1000009, `CHGA`)
```
"CHGA" u32 size u32 version(=1) u16 width u16 height   valid size in CELLS (Newland City 150x150)
f32 cell size (4.0 m) f32 height scale (0.2)
u16 n; n x u16 tile index -> ground texture id (1010006/21/22)
u32 (=1), then a Funcom object archive (same member encoding as `## meshes`, but without the version/range header):
  u32 name_count, name_count x {cstr kind, cstr name}, u32 object_count, (object_count+1) x object
  object 1 = AnarchyGroundData: map_width, map_height (vertices = cells+1 padded to whole patches),
  map_modulo, tiletexture_count, heightmap_compressed_data, heightmap_small_data,
  tilemap_compressed_data, [buildingmap_compressed_data], texture refs, tile_type_data
```
*Patches.* `m = min(2^tz(map_width-1), 2^tz(map_height-1), 64)` (`tz` = trailing zeros; DisplaySystem @0x10036f17 and @0x10035966); a patch is `m×m` cells with `(m+1)²` vertices; `(map_width-1)/m × (map_height-1)/m` patches, row-major. The three `*_data` members are concatenations of `u32 length + bytes`, one entry per patch.
*Heights* (`Patch_t::DecompressHeightMap` @0x10035df0): zlib stream of `(m+1)²` bytes (when `heightmap_small_data` is 4 bytes) or `(m+1)²` i16 (8 bytes = four corner heights). The samples are the **second order prefix sum** of the stream — first running down every column, then along every row, wrapping at 8/16 bits. 8 bit data become `byte << 8` (`FUN_10035db8`). The result is an **unsigned** 16 bit height: `y = value / 256 * height_scale` metres (0..51 m at 0.2). Verified: correlation 0.90 between terrain height and statel y in 566; heights are continuous across patch borders; vertices outside the valid `width×height` window are prefix-sum padding (vertical streaks) and are not emitted.
*Tiles* (`tilemap_compressed_data`): zlib `m×m` u16 per patch. The tile index is `value & mask` with **mask 0x3fff if the archive has a non-empty `tile_type_data` member, else 0xff** (ctor sets `+0x60`; verified: 325/325 CHGA records have all indices < n with this rule, 182 would violate it with a fixed 0x3fff). Index → texture id via the header `u16` list; every cell is one textured quad (the engine bakes per-patch textures from the same tiles, `FUN_10037957`). Bit 14 of the raw value selects the quad diagonal (`n3RoomSurface_t::GetTileTriangles` N3 @0x10014888: `(tile >> 14) & 1`: 0 → diagonal (x,z)-(x+1,z+1), 1 → (x+1,z)-(x,z+1)); bit 15 is not used by the loader.
*World mapping*: AO x = column × 4 m, z = row × 4 m (statels sit on the terrain with this mapping: all 1090 statels of 566 lie in x −41..592, z 0..599 for a 150-cell = 600 m map). Scene space negates z (see `## meshes`).

### Statel file (1000003)
```
u32 version (=1)  u32 offset[count]  (count = record count; zone/room i = [offset[i], offset[i+1]), last to end)
global section (only if offset[0] > end of the table): u32 v; if v != 4 { u16 n; n statels; 2 x {u16 n; n x 18 byte fog/sound entry} }
outdoor zone (tilemap id == id):  u16 k; k x u16; 4 x { u16 n; n x statel }; u16 nlights; lights
dungeon zone (room):              u32 size; size bytes (compressed room index list); 2 x {u16 n; n x 18 byte entry};
                                  2 x { u16 n; n x statel }; u16 nlights; lights
statel := f32 x,y,z  u32 flags  u32 mesh id (rdb 1010001; 0 = none)  u8 scale  u8 nattr
          nattr values encoded as (u32 mask; one u32 per set bit) repeated until nattr is consumed; [u32 colour if flags & 4]
light  := f32 x,y,z  u32 flags  u8 type,r,g,b  [4 (type 2) or 6 (type 4) u16 if (type & 0x1f) in {2,4}]
```
Readers: `n3StatelLoader_t` run function N3 @0x10028029, `FUN_10027f16`/`FUN_10027cb2` (zone headers), statel reader `FUN_1002777a`, light reader `FUN_10026e66`, global `n3StatelController_t::ReadGlobalData` @0x100250ac / `ReadStatelIndices` @0x10026191. Outdoor zone i covers tiles `(i % w, i / w)` of `zone size` tiles (566: 15×15 zones of 10 tiles = 40 m); statel coordinates are absolute world coordinates. Dungeon zone i belongs to room i: global = `room.pos + Ry(rot·90°)·local`, orientation `Ry(rot·90°)·R` (`FUN_100271b6`). Every zone of all 600 statel records parses with exactly one of the two layouts (outdoor ⇔ tilemap id == id, except 4622).
*Orientation* (`FUN_1002777a`): `u = flags >> 7`. If `flags & 1 == 0`: `heading = (u/180 % 360)°` about Y, `pitch = (u%180 − 90)°` about X, `roll = (u/180/360)°` about Z, `R = Rz·Rx·Ry`, uniform scale `scale/100 + 0.1` (byte 90 → 1.0). If `flags & 1`: heading `(u % 630)/630 turn`, stretch `k = ((u/630) % 211)/100 + 0.5` applied to x (the trailing translation term `FUN_10026d5c` is not applied; 133 of 28 905 sampled statels use this form). Rotations are right-hand-rule numerics in AO's left-handed space (`FUN_100014e4` is the textbook Hamilton rotation, `FUN_10003dc3(a,b)` computes `b⊗a`); e.g. flags 0x2d00 = identity, heading 90° maps +z to +x. Verified visually: Omni-1 Entertainment avenues and building rows line up at 45°, Newland City walls enclose courtyards.
*Scene transform*: `M_scene = F · (T·R·S) · F`, `F = diag(1,1,−1)`, matching the z negation baked into the meshes.

### Not decoded / gaps
* Outdoor water (`VisualWater_t`, `n3Zone_t::SetWater`): no outdoor water source was found in the records read by the loader; lakes/oceans are not drawn (terrain basins are visible).
* Lights, fog/sound entries, `buildingmap_compressed_data`, `tile_type_data` (walkability), statel attribute values and colours are parsed over but not used. Per-patch `.sdw` shadow maps are not used.
* Statels with mesh id 0 (433) are lights/markers; mesh 44795 (playfield 1721) does not exist in 1010001.

### Results
`cargo run --release -p ao-formats --example pf_stats [client] [ids…]` (Apple Silicon, release):
* `list_playfields`: **601** playfields with names.
* Load of all 601: **ok=601 failed=0**, 2 355 238 statels, 2 369 852 instances, 435 statels without mesh (mesh id 0 / 44795), 0 mesh decode failures; whole run ≈ 20 s.
* Largest terrains: Avalon (505) 997 600 cells, 32 418 statels, 223 unique meshes: **0.13 s**; Coast of Peace (556) 952 000 cells + 79 845 statels: 0.13 s; slowest of all: 6013 Central Gateway 0.25 s.
* Visual (`aomac view pf <id> --screenshot`): 566 Newland City (cliff-ringed city on the desert, buildings on the ground, platform + walkway over the crater), 705 Omni-1 Entertainment (octagonal plaza, radial avenues, tiled streets), 505 Avalon (domes, roads, plot grid), 600 Varmint Woods (forest, plot grid with roads).
* Tests: unit tests in `playfield::{record,ground,statel}` and `playfield::tests` (hand-built fixtures: header/room parse, prefix sums + byte wrap + patch layout, zone layouts, orientation, z-flip transform); `tests/playfield_real.rs` (skips without the client).

### Dungeon rooms
Code: `playfield/dungeon.rs` (`parse_gnda`, `parse_piece`, `Gnda::compute_cells`, `build_room`). A room shell is **not stored as a mesh**: the client (`n3GroundRenderer_t::CreateDungeonRoom` N3 @0x10008563) stitches prefabricated cell meshes chosen by an autotiling pass over the dungeon tilemap and bends them with two height layers. Everything is evaluated per cell of the room's `rect` (`[x1,z1,x2,z2)`, image rows = z).

**`GNDA` tilemap** (`RDBTilemap_t::ReadBlob` DisplaySystem @0x10078391, header reader `FUN_10077aaa`; 111 records, ids 121…):
```
"GNDA" u32 size u32 version(=1)  u16 w u16 h (280)  f32 cell size (2.0)  f32 height scale (0.2)
u16 n (<=32)  n x u16 tile id (ids[0] = 0 = empty)
n x { u8 a, u8 b,  a x { u16 neighbour tile id, 16 x u16 mesh id },  b x u16 }        b is always 0 in the data
sections { tag[4] u32 size(incl. 12 byte header) u32 version, payload }, padded to 4 bytes by `size`:
  DCGA PNG L8   tile index per cell (bits 0..6; bit 7 = "partition/door" flag); 0 = no cell
  DHGA PNG L8   floor height  (x height scale)           HCDA PNG L8  ceiling stretch (x height scale; client stores +0x20)
  IRHA u32[]    door edges: v>>1 = cell, v&1 = 0: edge to x+1, 1: edge to z+1 (both cells get the door bit)
  TAHA u16 k, k x {u8 tile, u8 c, c x u16 mesh}, then PNG L8: per cell override: (value>>2) selects list[value>>2 - 1]; value&1 door-frame side hint, value&3==2 = plain floor
  XTHA u16 m, m x u16 (version 1) | u32 (version 2) material ids, [u16 layer count (18) unless a PNG follows], ceil(layers/3) RGB PNGs = `layers` material indices per cell (R,G,B = layers 3i..3i+2)
  HSTA  u16 tile heights /10 (only used for room height bounds)
```
*Mesh table.* The 16 words of a neighbour row go to table slots `0..4, 6..15` (stream word `k < 5` -> slot `k`, else `k+1`; word 15 is dropped, slot 5 is never read from the file). Slot 5 of the *self* row (`table[t][t]`) is the tile id: **the flat floor+ceiling piece of tile `t` is mesh id == tile id**. Slots 0..4/6..15 of row `[t][0]` (walls against the void) correspond to the autotile keys below, shifted by one (`slot = key index`, key index 1-based in `KEYS`).

**Cell meshes** (rdb **1010013**, 363 records, `RDBMesh`-like object `FUN_1006ee4e` DisplaySystem): `u16 count`, `count` x { `u16 n`, n x 32 byte vertex (pos, normal, uv f32), `u16 index count`, indices u16 }, then 12 zero bytes. Block `i` is drawn with the material of layer `i` of the cell (block 0 = floor quad at y=0, block 1 = ceiling quad at the room height 4/8/16/32/100 m, further blocks = wall levels of 4 m; empty blocks have `n = 0`). Pieces are authored in a 2×2 m cell, y up, wall slab on −z.

**Autotiling** (`FUN_100169ab`, N3 @0x100169ab, dungeon branch; sweep over image rows from the last to the first, x ascending). Per cell with `tile = DCGA & 0x7f != 0`: mask `m` bit0 = z+1 empty, bit1 = x+1, bit2 = z−1, bit3 = x−1 (empty = tile 0). Walls added towards a flagged (bit 7) partition neighbour are carried to the neighbour cell (`up`/`left` carries; carry = 0 when the cell has a wall on that side): `flagged(x,z) && flagged(x,z+1)` -> bit1, `flagged(x,z) && flagged(x−1,z)` -> bit2. A door-edge cell with a wall bit whose neighbour across it is also a door cell gets flag `0x10`; with `TAHA & 1` the frame variant `0x30/0x50/0x70` follows from the TAHA bits of the neighbours (jamb left/right/both). Key = min over 4 right-rotations of `m & 0xf` (`n` rotations -> rot), `| (m & 0x70)`; slot = index of the key in `KEYS = [0f 07 05 03 01 00 31 51 71 1d 33 11 0f 1f]` (N3 `DAT_1005bce0`) + 1; `nb` = 0 if `m != 0` else the tile itself; mesh = `table[tile][nb][slot−1]`; keys 00 (flat), 01 (single wall), 03 (corner), 11 (lintel), 31/51/71 (frame with jambs) are the used ones; 05/07/0f (corridor, dead end, closed) have no mesh in any record (87 corridor + 8 dead-end cells of ~676 000 cells in all dungeon rooms), those cells get the floor only. Rotation angle of the piece about +Y: `rot·90° + 180°` (Hamilton rotation as in `statel::ry`): bit0 -> wall on +z, bit1 -> +x, bit2 −z, bit3 −x. If block 0 of the chosen piece is empty the flat floor piece (block 0 only) is added.
*Corner posts* (`DAT_1003d718` = `[0f 09 03 01 06 00 02 00 0c 08 00 00 04 …]`): each cell claims the corner(s) away from its walls (corner 0 = −x−z, 1 = −x+z, 2 = +x+z, 3 = +x−z); a claim on corner 1 is cancelled together with the claim of the cell at (x−1, z+1) on its corner 3, a claim on corner 2 with corner 0 of (x+1, z+1); remaining claims place mesh `table[tile][0][13]` (key `1f`) rotated by `corner·90° + 180°`. Net effect: posts fill the notch at concave room corners.

**Vertex warp** (`DungeonMesh_t` @0x10001a91 / `FUN_10001c3b` N3 @0x10001c3b): cell-local position `p` is rotated, then `u = x/cell + 0.5`, `v = z/cell + 0.5`; `y' = y + F(u,v) + 0.25·y·C(u,v)` with `F`/`C` the bilinear patches over the DHGA·hs (+ `−floor_min`) / HCDA·hs values of the corners `(x−1..x, z−1..z)` of the cell (heights live on cell corners); normals get `n −= n_y·(∂F + 0.25·y·∂C)` with the Catmull-Rom slope polynomials `FUN_100023ad/10002479/1000244e/1000251c` (ported literally, `slope`/`eval_*`). `floor_min` = lowest `DHGA·hs` of the room's cells (`RDBPlayfield_t::CalculateRoomHeights` N3 @0x1001bd82). The vertex welding pass `FUN_10001645` (merge equal positions, smooth normals) is not ported.
*Placement*: cell `(x,z)` centre = `((x−x1)·cell − W'/2·cell, …)` with `W' = ((w−1) & ~1) + 1` (`w = x2−x1`; same for z), then `room.pos + Ry(rot·90°)·local` — identical to statels. Verified: over 173 361 statels of 2000 rooms the 10th percentile of `statel.y − (DHGA·hs − floor_min)` is exactly 0.0, and props lie inside the shell footprint (e.g. 127 room 4: shell x −10..8, props −10..8).
*Materials*: `XTHA` material id `m`: `m & 0x80000000` -> texture rdb **1010004**/`m & 0x7fffffff`, else rdb **1010009** (both JPEG; 1010016/1010017 and 1010023/1010024 are the lower mip sets).

**Evidence.** `aomac view pf 127 --screenshot` (Condemned Subway): the 46 rooms line up into the subway network (corridors meet at doors with the stored rotation), Ladies' Room shows tiled walls with baseboard, sink/mirror/stalls props on the walls, partition with a doorway (IRHA + flagged row), Grand Dome with vaulted ceiling, pillars and the doorway into the restroom; 346 ACD Omnilab (72 separate lab modules, corridors with lit floor strips), 1406/1211 apartments (room + props); concave corners have posts (crack seen before the corner pass). Tests: `playfield::dungeon::tests` (fixture GNDA with PNG sections, piece format, autotile/corner/door states, placement + z mirror + winding) and a real-data build of playfield 127.

**Gaps.** Not ported: vertex welding / smoothing (`FUN_10001645`), the secondary `HSTA`/lightmap (`rdb room lightmap`, parsed over in `record.rs`), liquids, key variants `1d/33` with unreachable canonical masks, per-tile `b` random floor variants (never present), the region clamp at the map border (`local_2c`), door frames whose key (`canonical|flags`) is not in `KEYS` (e.g. door on a 2-wall cell: no piece, floor only). Room textures use the highest mip set (1010009/1010004). Ceilings are drawn (two-sided renderer): from above, dungeon interiors are hidden — fly inside (`aomac view pf <id> --eye … --at …`).
